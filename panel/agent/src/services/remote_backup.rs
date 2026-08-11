use std::path::Path;
use crate::safe_cmd::safe_command;

/// Returned when the binary a password-authenticated SFTP destination execs is
/// absent from the box the backup runs on.
///
/// It is a named constant because the caller has to recognise this specific
/// condition to answer with a 4xx. Reporting it as a 5xx makes the panel replace
/// the whole sentence with an incident reference, so the operator is told their
/// agent broke when the agent is working perfectly and the box is simply missing
/// a package — which is the state s289 measured and the message below was
/// written to end.
pub const SSHPASS_MISSING: &str =
    "sshpass is not installed on this server, and it is required for \
     password-authenticated SFTP destinations. The panel can install it for you from \
     Settings, or use an SSH key instead.";

/// RAII guard that deletes a temp file on drop, ensuring cleanup on all code paths.
struct TempFileGuard {
    path: String,
}

impl TempFileGuard {
    fn create(label: &str, content: &str) -> Result<Self, String> {
        let random_suffix: u64 = rand::random();
        let path = format!("/tmp/.dockpanel-s3-{}-{:016x}", label, random_suffix);
        std::fs::write(&path, content)
            .map_err(|e| format!("Failed to write S3 config: {e}"))?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600)).ok();
        }
        Ok(Self { path })
    }
}

impl Drop for TempFileGuard {
    fn drop(&mut self) {
        std::fs::remove_file(&self.path).ok();
    }
}

/// Run a curl invocation against the S3 endpoint and return `(status, body)`.
///
/// # Why every S3 call goes through here
///
/// These calls used `--fail`, which fails on 4xx/5xx and says nothing about **3xx**.
/// Without `-L`, curl does not follow a redirect either: it prints the redirect body,
/// transfers no payload, and **exits 0**. So an endpoint that answers 301 — an S3
/// bucket addressed in the wrong region, or an `http://` endpoint whose provider
/// redirects to `https://` — made `upload_s3` return `Ok`, the agent answer
/// `{"success":true}`, and the panel light the "remote" badge for an object that was
/// never written. Measured on a live box (s289): policy run reported
/// *"1 successes, 0 failures, 0 not uploaded off-site"* against an empty bucket.
///
/// Following redirects is NOT the fix: an AWS SigV4 signature is bound to the host
/// and path it was computed for, so a followed redirect authenticates as garbage —
/// and on 301/302 curl would downgrade the `PUT` to `GET`. The fix is to stop
/// treating "not an error status" as "the bytes arrived": require an explicit 2xx.
///
/// One runner for all four S3 operations on purpose. The same latent bug sat in
/// `test_s3`, `list_s3` and `delete_s3`, and fixing the copy under investigation
/// while leaving its siblings is a mistake this file has already shipped once.
async fn s3_curl(
    args: Vec<String>,
    timeout_secs: u64,
    label: &str,
) -> Result<(u16, String), String> {
    // `--fail` is deliberately absent: it suppresses the response body, and the body
    // is where S3 puts the reason. The status check below replaces it.
    let mut full: Vec<String> = vec![
        "--silent".into(),
        "--show-error".into(),
        "-w".into(),
        "\n%{http_code}".into(),
    ];
    full.extend(args);

    let output = tokio::time::timeout(
        std::time::Duration::from_secs(timeout_secs),
        safe_command("curl").args(&full).output(),
    )
    .await
    .map_err(|_| format!("{label} timed out"))?
    .map_err(|e| format!("Failed to run curl: {e}"))?;

    let stdout = String::from_utf8_lossy(&output.stdout);
    let (body, code_str) = match stdout.rsplit_once('\n') {
        Some((b, c)) => (b.trim().to_string(), c.trim().to_string()),
        None => (String::new(), stdout.trim().to_string()),
    };

    // curl itself failed (DNS, refused connection, TLS) — there is no HTTP status.
    let code: u16 = code_str.parse().map_err(|_| {
        let stderr = String::from_utf8_lossy(&output.stderr);
        let detail = if stderr.trim().is_empty() { body.clone() } else { stderr.trim().to_string() };
        format!("{label} failed: {detail}")
    })?;

    Ok((code, body))
}

/// True only for 2xx. A 3xx is not success — see [`s3_curl`].
fn s3_ok(code: u16) -> bool {
    (200..300).contains(&code)
}

/// Trim an S3 error body down to something a log line can carry.
fn s3_detail(code: u16, body: &str) -> String {
    let b = body.trim();
    if b.is_empty() {
        format!("HTTP {code}")
    } else {
        format!("HTTP {code}: {}", b.chars().take(300).collect::<String>())
    }
}

/// Upload a backup file to S3-compatible storage using curl --aws-sigv4.
pub async fn upload_s3(
    filepath: &str,
    bucket: &str,
    region: &str,
    endpoint: &str,
    access_key: &str,
    secret_key: &str,
    remote_path: &str,
) -> Result<String, String> {
    let path = Path::new(filepath);
    if !path.exists() {
        return Err(format!("Backup file not found: {filepath}"));
    }

    let filename = path
        .file_name()
        .and_then(|n| n.to_str())
        .ok_or("Invalid filename")?;

    // Build the S3 URL: endpoint/bucket/prefix/filename
    let prefix = remote_path.trim_matches('/');
    let url = if prefix.is_empty() {
        format!("{endpoint}/{bucket}/{filename}")
    } else {
        format!("{endpoint}/{bucket}/{prefix}/{filename}")
    };

    tracing::info!("Uploading {filename} to {url}");

    // Write credentials to a temp file so they don't appear in process listing
    let config_content = format!("user = \"{}:{}\"", access_key, secret_key);
    let config_guard = TempFileGuard::create("upload", &config_content)?;

    let (code, body) = s3_curl(
        vec![
            "--aws-sigv4".into(),
            format!("aws:amz:{region}:s3"),
            "-K".into(),
            config_guard.path.clone(),
            "-X".into(),
            "PUT".into(),
            "-H".into(),
            "Content-Type: application/gzip".into(),
            "-T".into(),
            filepath.to_string(),
            url.clone(),
        ],
        600,
        "Upload",
    )
    .await?;

    drop(config_guard);

    if !s3_ok(code) {
        return Err(format!("S3 upload failed: {}", s3_detail(code, &body)));
    }

    tracing::info!("Upload complete: {filename} (HTTP {code})");
    Ok(url)
}

/// Build the options every SFTP operation shares.
///
/// # Why this is one function
///
/// `upload_sftp` and `test_sftp` each built this list by hand, with a comment
/// promising they were "kept in step deliberately". They were not: `ConnectTimeout`
/// was set on the test and missing from the upload, so the two operations the
/// operator is told are equivalent could hang differently. A hand-maintained
/// invariant between two copies is the thing that keeps failing here — so there is
/// now one copy and the promise is structural.
///
/// There is no longer a port-flag parameter: both operations speak the SFTP
/// subsystem now, so both take the same uppercase port flag. When this list served
/// `ssh` (lowercase flag) as well, that difference was the one argument each caller
/// had to supply.
///
/// The two settings encoded here each cost a release to find (s288):
///
/// * `UserKnownHostsFile=/etc/dockpanel/known_hosts` — `accept-new` means "trust on
///   first use, then pin", and pinning is a WRITE. ssh's default target is
///   `~/.ssh/known_hosts`, which under this unit's `ProtectHome=yes` cannot be
///   created: *"Could not create directory '/root/.ssh' (Read-only file system)"*.
///   `/etc/dockpanel` is in the unit's `ReadWritePaths` and is where `deploy.rs`
///   already points git's ssh, so one trust store serves both. Redirecting the file
///   keeps first-use pinning; `/dev/null` or `StrictHostKeyChecking=no` would have
///   "fixed" it by discarding host verification entirely.
///
/// * `BatchMode=yes` ONLY when authenticating by key. It means "never prompt", which
///   is right for a key and fatal for a password: it disables password and
///   keyboard-interactive auth outright, so `sshpass` has nothing left to answer.
///   Each setting is correct alone and together they made password SFTP impossible.
///
///   That trap has a second entrance, found s339: **`sftp -b` sets `BatchMode` by
///   itself**, so the password case has to say `BatchMode=no` out loud or every
///   password-authenticated destination dies at `Permission denied` — the identical
///   s288 failure, arriving through the flag instead of through this list.
fn sftp_opts(port: u16, password: Option<&str>, key_path: Option<&str>) -> Vec<String> {
    let mut args: Vec<String> = vec![
        "-o".into(),
        "StrictHostKeyChecking=accept-new".into(),
        "-o".into(),
        "UserKnownHostsFile=/etc/dockpanel/known_hosts".into(),
        "-o".into(),
        "ConnectTimeout=10".into(),
        "-P".into(),
        port.to_string(),
    ];
    if key_path.is_some() || password.is_none() {
        args.push("-o".into());
        args.push("BatchMode=yes".into());
    } else {
        args.push("-o".into());
        args.push("BatchMode=no".into());
    }
    if let Some(key) = key_path {
        args.push("-i".into());
        args.push(key.into());
    }
    args
}

/// Quote one path as a single `sftp` batch argument.
///
/// A batch line is parsed by sftp's own tokeniser, **not by a shell**, and the
/// difference is not cosmetic: an unquoted space splits the path in two, and
/// `put local /backups/my dir/f.tgz` then writes to `/backups/my` under the
/// SOURCE filename and **exits 0**. Measured s339 — a success report for an
/// archive that landed somewhere else under another name, which is a worse
/// outcome than the failure this whole change is fixing.
///
/// Double quotes carry spaces, single quotes and glob characters literally; a
/// backslash or a double quote inside must itself be escaped.
fn sftp_quote(path: &str) -> String {
    let mut out = String::with_capacity(path.len() + 2);
    out.push('"');
    for ch in path.chars() {
        if ch == '\\' || ch == '"' {
            out.push('\\');
        }
        out.push(ch);
    }
    out.push('"');
    out
}

/// Reject a path that cannot be expressed as one batch line.
///
/// In a batch script a newline **starts a new command**, so a `remote_path`
/// carrying one appends attacker-chosen sftp operations to the script the agent
/// built (verified s339 by planting a second `put`). `remote_path` reaches here
/// straight from the request body with no validation anywhere upstream, so this
/// is the only place that can refuse it.
fn reject_unquotable_path(path: &str) -> Result<(), String> {
    if path.contains('\n') || path.contains('\r') {
        return Err("Remote path may not contain a line break".to_string());
    }
    Ok(())
}

/// One `-mkdir` per ancestor, deepest last.
///
/// `sftp` has no `mkdir -p`: `mkdir /a/b/c` fails outright when `/a/b` is absent,
/// and the `put` after it then fails too. So each component is created in turn.
/// The `-` prefix means "continue past this command's failure", which is required
/// rather than tidy: a plain `mkdir` of an **existing** directory reports Failure
/// and **aborts the whole batch** (both measured s339), and every component of an
/// already-provisioned destination exists.
fn sftp_mkdir_script(dir: &str) -> String {
    let absolute = dir.starts_with('/');
    let mut out = String::new();
    let mut acc = String::new();
    for comp in dir.split('/').filter(|c| !c.is_empty()) {
        if acc.is_empty() {
            if absolute {
                acc.push('/');
            }
        } else {
            acc.push('/');
        }
        acc.push_str(comp);
        out.push_str("-mkdir ");
        out.push_str(&sftp_quote(&acc));
        out.push('\n');
    }
    out
}

/// Makes each Test Connection's local probe filename unique within the process,
/// so two destinations tested at once cannot delete each other's file.
static PROBE_SEQ: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

/// Join both of a finished command's streams into one operator-facing detail.
///
/// Reading only `stderr` is what made issue #102 unreadable: `internal-sftp`
/// writes its refusal — *"This service allows sftp connections only."* — to
/// **stdout**, leaving stderr holding nothing but ssh's known-hosts banner. The
/// operator was shown `Warning: Permanently added '[host]:port' ...` as the entire
/// cause of a failed backup, and on any later connection, with the key already
/// pinned, stderr is empty and the message degraded to "no error output from ssh".
fn command_detail(output: &std::process::Output) -> String {
    let stderr = String::from_utf8_lossy(&output.stderr);
    let stdout = String::from_utf8_lossy(&output.stdout);
    let mut parts: Vec<&str> = Vec::new();
    let e = stderr.trim();
    let o = stdout.trim();
    if !e.is_empty() {
        parts.push(e);
    }
    if !o.is_empty() {
        parts.push(o);
    }
    if parts.is_empty() {
        "no output from sftp".to_string()
    } else {
        parts.join(" | ")
    }
}

/// Run `ssh`/`scp`, routing through `sshpass` when a password is the credential.
///
/// `sshpass -e` reads the password from the environment, so it never appears in a
/// process listing. A key beats a password when both are supplied.
async fn run_sftp(
    program: &str,
    args: Vec<String>,
    password: Option<&str>,
    key_path: Option<&str>,
    timeout_secs: u64,
    label: &str,
    batch_script: Option<&str>,
) -> Result<std::process::Output, String> {
    let (prog, final_args, pw_env) = match password {
        Some(pw) if key_path.is_none() => {
            let mut a: Vec<String> = vec!["-e".into(), program.into()];
            a.extend(args);
            ("sshpass".to_string(), a, Some(pw.to_string()))
        }
        _ => (program.to_string(), args, None),
    };

    let mut cmd = safe_command(&prog);
    cmd.args(&final_args);
    if let Some(ref pw) = pw_env {
        cmd.env("SSHPASS", pw);
    }

    // `sftp -b -` reads its command script from stdin, which keeps the script out
    // of the filesystem entirely — no temp file to create under `ProtectSystem=
    // strict`, and no window where a world-readable batch names the destination.
    // Writing before reading is safe only because these scripts are a few hundred
    // bytes: a script large enough to fill the pipe buffer would deadlock against
    // an sftp that is blocked writing its own progress to a full stdout.
    let run = async {
        match batch_script {
            Some(script) => {
                cmd.stdin(std::process::Stdio::piped());
                cmd.stdout(std::process::Stdio::piped());
                cmd.stderr(std::process::Stdio::piped());
                let mut child = cmd.spawn()?;
                if let Some(mut sink) = child.stdin.take() {
                    use tokio::io::AsyncWriteExt;
                    sink.write_all(script.as_bytes()).await?;
                    sink.shutdown().await?;
                    // Dropped here on purpose: sftp runs its script to completion
                    // and only then exits, so it needs to see EOF on stdin.
                    drop(sink);
                }
                child.wait_with_output().await
            }
            None => cmd.output().await,
        }
    };

    tokio::time::timeout(std::time::Duration::from_secs(timeout_secs), run)
        .await
        .map_err(|_| format!("{label} timed out"))?
        .map_err(|e| {
            // `sshpass` is a separate package and the agent shells out to it for
            // every password-authenticated destination. When it is absent the raw
            // error is "No such file or directory (os error 2)", which names neither
            // the binary nor the remedy — s289 measured exactly that, as an opaque
            // 502, on a fresh install where nothing had installed it.
            if prog == "sshpass" && e.kind() == std::io::ErrorKind::NotFound {
                SSHPASS_MISSING.to_string()
            } else {
                format!("Failed to run {prog}: {e}")
            }
        })
}

/// Upload a backup file via SCP.
pub async fn upload_sftp(
    filepath: &str,
    host: &str,
    port: u16,
    username: &str,
    password: Option<&str>,
    key_path: Option<&str>,
    remote_path: &str,
) -> Result<String, String> {
    let path = Path::new(filepath);
    if !path.exists() {
        return Err(format!("Backup file not found: {filepath}"));
    }

    let filename = path
        .file_name()
        .and_then(|n| n.to_str())
        .ok_or("Invalid filename")?;

    let dir = remote_path.trim_end_matches('/');
    reject_unquotable_path(dir)?;
    reject_unquotable_path(filepath)?;

    let remote_file = format!("{dir}/{filename}");
    let remote_dest = format!("{username}@{host}:{remote_file}");

    tracing::info!("Uploading {filename} via SFTP to {remote_dest}");

    // Create the destination directory, then transfer — both over the SFTP
    // subsystem, in one connection.
    //
    // Both steps used to be SSH **exec** requests (`ssh host "mkdir -p ..."`, then
    // `scp`), and an sshd with `ForceCommand internal-sftp` — OpenSSH's own
    // documented way to run a chrooted, SFTP-only account — replaces whatever
    // command the client asked for with the SFTP subsystem. So the `mkdir` was
    // discarded and the upload failed for every such destination (issue #102).
    //
    // `scp` is gone rather than merely re-ordered, because it carries the same
    // defect on exactly the hosts that are hardest to notice: OpenSSH 9.0 switched
    // scp to the SFTP protocol, so a modern agent host transfers fine and only the
    // `mkdir` fails, while an agent on RHEL 8 or Ubuntu 20.04 still speaks legacy
    // `scp -t` and would keep failing after a mkdir-only fix. Measured both ways
    // s339 against a chrooted sshd: default scp exit 0, `scp -O` exit 1.
    //
    // Nothing created the directory at all before s289; that fix is preserved here,
    // one component at a time — see `sftp_mkdir_script` for why it cannot be one call.
    let mut script = sftp_mkdir_script(dir);
    script.push_str(&format!(
        "put {} {}\n",
        sftp_quote(filepath),
        sftp_quote(&remote_file)
    ));

    let mut cmd_args = sftp_opts(port, password, key_path);
    cmd_args.push("-b".into());
    cmd_args.push("-".into());
    cmd_args.push(format!("{username}@{host}"));

    let output = run_sftp(
        "sftp",
        cmd_args,
        password,
        key_path,
        600,
        "Upload",
        Some(&script),
    )
    .await?;

    if !output.status.success() {
        return Err(format!("SFTP upload failed: {}", command_detail(&output)));
    }

    tracing::info!("SFTP upload complete: {filename}");
    Ok(remote_dest)
}

/// Test S3 access by performing the operation an upload performs.
///
/// # Why this writes instead of HEADing
///
/// The test used to `HEAD` the bucket root, while an upload `PUT`s an object into
/// `path_prefix`. Those need different permissions and different paths, so a
/// read-only key, a key scoped to another prefix, and a bucket that answers 301 all
/// reported "S3 connection successful" and then failed — or silently did nothing —
/// at the first real backup. A test that connects differently from the upload is a
/// test of nothing; s289 measured a green Test against a destination whose bucket
/// stayed empty through a full policy run.
///
/// So: PUT a tiny probe object where the backups will go, then delete it. The probe
/// is removed on every path, including when the delete fails (it is reported, not
/// swallowed, because a destination that accumulates probes is worth knowing about).
pub async fn test_s3(
    bucket: &str,
    region: &str,
    endpoint: &str,
    access_key: &str,
    secret_key: &str,
    path_prefix: &str,
) -> Result<(), String> {
    let prefix = path_prefix.trim_matches('/');
    let probe_name = format!(".dockpanel-write-probe-{:016x}", rand::random::<u64>());
    let key = if prefix.is_empty() {
        probe_name.clone()
    } else {
        format!("{prefix}/{probe_name}")
    };
    let url = format!("{endpoint}/{bucket}/{key}");

    // Write credentials to a temp file so they don't appear in process listing
    let config_content = format!("user = \"{}:{}\"", access_key, secret_key);
    let config_guard = TempFileGuard::create("test", &config_content)?;

    let sigv4 = format!("aws:amz:{region}:s3");
    let (code, body) = s3_curl(
        vec![
            "--aws-sigv4".into(),
            sigv4.clone(),
            "-K".into(),
            config_guard.path.clone(),
            "-X".into(),
            "PUT".into(),
            "-H".into(),
            "Content-Type: text/plain".into(),
            "--data-binary".into(),
            "dockpanel write probe".into(),
            url.clone(),
        ],
        15,
        "Connection test",
    )
    .await?;

    if !s3_ok(code) {
        drop(config_guard);
        return Err(format!("S3 connection test failed: {}", s3_detail(code, &body)));
    }

    // Clean up the probe. Reported rather than ignored: a destination we can write
    // but not delete cannot enforce retention either, and the operator should hear
    // it here rather than from a storage bill.
    let (del_code, del_body) = s3_curl(
        vec![
            "--aws-sigv4".into(),
            sigv4,
            "-K".into(),
            config_guard.path.clone(),
            "-X".into(),
            "DELETE".into(),
            url,
        ],
        15,
        "Connection test cleanup",
    )
    .await?;

    drop(config_guard);

    if !s3_ok(del_code) && del_code != 404 {
        return Err(format!(
            "S3 write succeeded but the test object could not be removed ({}) — \
             retention will not be able to delete old backups either",
            s3_detail(del_code, &del_body)
        ));
    }

    Ok(())
}

/// Test SFTP access by doing what an upload does.
///
/// Connecting and running `exit` proved only that the credential authenticates. It
/// never touched `remote_path`, so a destination whose directory did not exist —
/// including the `/backups` default, which exists on almost no server — reported
/// "SFTP connection successful" and then failed every real upload with
/// *"No such file or directory"* (measured s289). The test now creates the
/// directory and writes a probe file into it, which is exactly the sequence
/// `upload_sftp` performs.
pub async fn test_sftp(
    host: &str,
    port: u16,
    username: &str,
    password: Option<&str>,
    key_path: Option<&str>,
    remote_path: &str,
) -> Result<(), String> {
    let dir = remote_path.trim_end_matches('/');
    reject_unquotable_path(dir)?;

    // Create the directory, write a probe, remove it — the three things an upload
    // does, over the same transport an upload uses. Testing over a DIFFERENT
    // transport is how issue #102 stayed invisible from the panel: the test and the
    // upload both spoke SSH exec, so a server that refuses exec failed both, but a
    // test that had spoken sftp while uploads spoke scp would have been green over a
    // broken destination — the s289 defect this function was written to end.
    //
    // A local file is required: `put /dev/null` is refused outright ("not a regular
    // file", measured s339), so the probe is a real zero-byte file in the agent's
    // own tmp. `PrivateTmp=yes` makes that namespace private to the unit and the
    // sftp child inherits it.
    let probe_local = std::env::temp_dir().join(format!(
        ".dockpanel-sftp-probe-{}-{}",
        std::process::id(),
        PROBE_SEQ.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
    ));
    let probe_local_str = probe_local.to_string_lossy().to_string();

    let script = if dir.is_empty() {
        // No path to probe, so this proves only that the SFTP subsystem opens —
        // which is still more than an auth handshake proves.
        "pwd\n".to_string()
    } else {
        tokio::fs::write(&probe_local, b"")
            .await
            .map_err(|e| format!("Could not stage a local probe file: {e}"))?;
        let remote_probe = format!("{dir}/.dockpanel-write-probe");
        format!(
            "{}put {} {}\nrm {}\n",
            sftp_mkdir_script(dir),
            sftp_quote(&probe_local_str),
            sftp_quote(&remote_probe),
            sftp_quote(&remote_probe)
        )
    };

    let mut cmd_args = sftp_opts(port, password, key_path);
    cmd_args.push("-b".into());
    cmd_args.push("-".into());
    cmd_args.push(format!("{username}@{host}"));

    let output = run_sftp(
        "sftp",
        cmd_args,
        password,
        key_path,
        20,
        "Connection test",
        Some(&script),
    )
    .await;

    // Removed on every path, including the error ones — a probe left in the agent's
    // tmp for each failed Test Connection is a slow leak nobody would look for.
    let _ = tokio::fs::remove_file(&probe_local).await;

    let output = output?;

    if output.status.success() {
        Ok(())
    } else {
        // Authentication failures and permission failures read very differently to
        // an operator, and the raw output distinguishes them — so pass it through
        // rather than collapsing both into "connection failed".
        Err(format!("SFTP test failed: {}", command_detail(&output)))
    }
}

/// List remote backups in S3 bucket with given prefix.
pub async fn list_s3(
    bucket: &str,
    region: &str,
    endpoint: &str,
    access_key: &str,
    secret_key: &str,
    prefix: &str,
) -> Result<Vec<String>, String> {
    let prefix_clean = prefix.trim_matches('/');
    let url = if prefix_clean.is_empty() {
        format!("{endpoint}/{bucket}/?list-type=2")
    } else {
        format!("{endpoint}/{bucket}/?list-type=2&prefix={prefix_clean}/")
    };

    // Write credentials to a temp file so they don't appear in process listing
    let config_content = format!("user = \"{}:{}\"", access_key, secret_key);
    let config_guard = TempFileGuard::create("list", &config_content)?;

    let (code, body) = s3_curl(
        vec![
            "--aws-sigv4".into(),
            format!("aws:amz:{region}:s3"),
            "-K".into(),
            config_guard.path.clone(),
            url,
        ],
        30,
        "List",
    )
    .await?;

    drop(config_guard);

    if !s3_ok(code) {
        return Err(format!("S3 list failed: {}", s3_detail(code, &body)));
    }

    // Parse XML response — extract <Key> elements
    let keys: Vec<String> = body
        .split("<Key>")
        .skip(1)
        .filter_map(|s| s.split("</Key>").next().map(|k| k.to_string()))
        .collect();

    Ok(keys)
}

/// Delete a file from S3.
pub async fn delete_s3(
    bucket: &str,
    region: &str,
    endpoint: &str,
    access_key: &str,
    secret_key: &str,
    key: &str,
) -> Result<(), String> {
    let url = format!("{endpoint}/{bucket}/{key}");

    // Write credentials to a temp file so they don't appear in process listing
    let config_content = format!("user = \"{}:{}\"", access_key, secret_key);
    let config_guard = TempFileGuard::create("delete", &config_content)?;

    let (code, body) = s3_curl(
        vec![
            "--aws-sigv4".into(),
            format!("aws:amz:{region}:s3"),
            "-K".into(),
            config_guard.path.clone(),
            "-X".into(),
            "DELETE".into(),
            url,
        ],
        15,
        "Delete",
    )
    .await?;

    drop(config_guard);

    // S3 answers 204 for a delete, and 404 for an object already gone — both mean
    // "it is not there", which is what the caller asked for.
    if s3_ok(code) || code == 404 {
        Ok(())
    } else {
        Err(format!("S3 delete failed: {}", s3_detail(code, &body)))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The shell escaping this replaced was correct FOR A SHELL. sftp's batch
    /// parser is not one, and the failure it produces is silent: measured s339,
    /// an unquoted `put local /backups/my dir/f.tgz` wrote to `/backups/my`
    /// under the SOURCE filename and exited 0.
    #[test]
    fn quotes_paths_a_batch_parser_would_otherwise_split() {
        assert_eq!(sftp_quote("/backups"), "\"/backups\"");
        assert_eq!(sftp_quote("/my backups"), "\"/my backups\"");
        // Single quotes are literal inside double quotes — the shell form had to
        // escape these and the batch form must not.
        assert_eq!(sftp_quote("/od'brien"), "\"/od'brien\"");
        // These two are the only characters that need escaping.
        assert_eq!(sftp_quote("/say\"hi"), "\"/say\\\"hi\"");
        assert_eq!(sftp_quote("/back\\slash"), "\"/back\\\\slash\"");
        // Glob metacharacters survive literally, so a real directory named with
        // one is still addressed rather than expanded.
        assert_eq!(sftp_quote("/we*ird?"), "\"/we*ird?\"");
    }

    /// `sftp` has no `mkdir -p`, so every ancestor is created in its own command,
    /// shallowest first, each prefixed with `-` because a `mkdir` of an existing
    /// directory FAILS and would abort the rest of the batch.
    #[test]
    fn mkdir_script_creates_every_ancestor_shallowest_first() {
        assert_eq!(
            sftp_mkdir_script("/srv/backups/dockpanel"),
            "-mkdir \"/srv\"\n-mkdir \"/srv/backups\"\n-mkdir \"/srv/backups/dockpanel\"\n"
        );
        assert_eq!(sftp_mkdir_script("/backups"), "-mkdir \"/backups\"\n");
        // A relative path must not acquire a leading slash.
        assert_eq!(
            sftp_mkdir_script("backups/nightly"),
            "-mkdir \"backups\"\n-mkdir \"backups/nightly\"\n"
        );
        assert_eq!(sftp_mkdir_script(""), "");
        // Every line carries the ignore-failure prefix; one that did not would
        // abort the batch on an already-provisioned destination.
        for line in sftp_mkdir_script("/a/b/c").lines() {
            assert!(line.starts_with("-mkdir "), "unprefixed mkdir: {line}");
        }
    }

    /// A newline in a batch script STARTS A NEW COMMAND. `remote_path` arrives
    /// from the request body with no validation upstream, so this is the only
    /// place that can refuse one.
    #[test]
    fn rejects_a_remote_path_that_would_append_a_second_command() {
        assert!(reject_unquotable_path("/backups").is_ok());
        assert!(reject_unquotable_path("/my backups").is_ok());
        assert!(reject_unquotable_path("/backups\nput /etc/shadow /pub/x").is_err());
        assert!(reject_unquotable_path("/backups\rrm /pub/x").is_err());
    }

    /// `sftp -b` sets BatchMode itself, and BatchMode disables the password auth
    /// `sshpass` exists to answer — the s288 defect, reachable a second way.
    #[test]
    fn batchmode_is_turned_off_for_password_auth_and_on_for_keys() {
        let pw = sftp_opts(22, Some("secret"), None);
        assert!(
            pw.windows(2).any(|w| w[0] == "-o" && w[1] == "BatchMode=no"),
            "password auth must disable BatchMode explicitly: {pw:?}"
        );
        let key = sftp_opts(22, None, Some("/etc/dockpanel/id"));
        assert!(
            key.windows(2).any(|w| w[0] == "-o" && w[1] == "BatchMode=yes"),
            "key auth must keep BatchMode on: {key:?}"
        );
        // sftp takes -P for the port; -p would be "preserve times".
        assert!(pw.windows(2).any(|w| w[0] == "-P" && w[1] == "22"));
    }

    /// Reading only stderr is what made issue #102 unreadable: `internal-sftp`
    /// refuses an exec request on STDOUT, leaving stderr holding nothing but
    /// ssh's known-hosts banner.
    #[test]
    fn detail_reports_a_refusal_that_arrived_on_stdout() {
        use std::os::unix::process::ExitStatusExt;
        let only_stdout = std::process::Output {
            status: std::process::ExitStatus::from_raw(256),
            stdout: b"This service allows sftp connections only.".to_vec(),
            stderr: Vec::new(),
        };
        assert!(command_detail(&only_stdout).contains("sftp connections only"));

        let neither = std::process::Output {
            status: std::process::ExitStatus::from_raw(256),
            stdout: Vec::new(),
            stderr: Vec::new(),
        };
        assert_eq!(command_detail(&neither), "no output from sftp");

        let both = std::process::Output {
            status: std::process::ExitStatus::from_raw(256),
            stdout: b"out".to_vec(),
            stderr: b"err".to_vec(),
        };
        assert_eq!(command_detail(&both), "err | out");
    }

    /// End-to-end against a real sshd running `ForceCommand internal-sftp`.
    ///
    /// Opt-in because it needs that server: export
    /// `DOCKPANEL_SFTP_LAB=user:password@host:port:/remote/path` and run
    /// `cargo test -- --ignored`. Run s339 against a chrooted Debian sshd in a
    /// container. CI is unaffected because the test is `#[ignore]`d, not because
    /// it quietly returns — that distinction is the whole point of the note below.
    ///
    /// v2.95.1 shipped a reverted transport partly because this test used to
    /// return early when the variable was unset: a run that executed NOTHING
    /// printed the same `ok` as a run that proved the transport, and the summary
    /// line counted it as a pass. Two changes remove that state:
    ///
    /// - `#[ignore]` puts it in the summary's `N ignored` column, which is a
    ///   counted, visible state rather than a silent pass.
    /// - the variable is now `expect`ed rather than matched, so running it
    ///   (`cargo test -- --ignored`) without a lab PANICS instead of passing
    ///   vacuously. A test that cannot run must not be able to report success.
    ///
    /// Treat the transport arms in `backup-lands-pin-e2e.sh` as the CI-side
    /// guard; this is the deeper check you run by hand when touching the
    /// transport, and if you are touching it, set the variable.
    #[tokio::test]
    #[ignore = "needs an sshd running ForceCommand internal-sftp; set DOCKPANEL_SFTP_LAB and run with --ignored"]
    async fn lab_upload_and_test_survive_forcecommand() {
        let spec = std::env::var("DOCKPANEL_SFTP_LAB").expect(
            "DOCKPANEL_SFTP_LAB must be set to run this test \
             (user:password@host:port:/remote/path). It is #[ignore]d by default; \
             being asked to run means a lab was expected, so an unset variable is \
             a configuration error, not a reason to pass.",
        );
        let (creds, rest) = spec.split_once('@').expect("user:pass@host:port:path");
        let (user, pass) = creds.split_once(':').expect("user:pass");
        let parts: Vec<&str> = rest.splitn(3, ':').collect();
        let (host, port, path) = (parts[0], parts[1].parse::<u16>().unwrap(), parts[2]);

        test_sftp(host, port, user, Some(pass), None, path)
            .await
            .expect("Test Connection must succeed through ForceCommand internal-sftp");

        let local = std::env::temp_dir().join("dockpanel-sftp-lab-upload.txt");
        tokio::fs::write(&local, b"s339").await.unwrap();
        let dest = upload_sftp(
            local.to_str().unwrap(),
            host,
            port,
            user,
            Some(pass),
            None,
            path,
        )
        .await
        .expect("upload must succeed through ForceCommand internal-sftp");
        assert!(dest.contains("dockpanel-sftp-lab-upload.txt"));
        let _ = tokio::fs::remove_file(&local).await;

        // A path that cannot exist must still FAIL — otherwise the arms above only
        // prove the happy direction, and a function that always returns Ok would
        // pass every one of them.
        let err = test_sftp(host, port, user, Some(pass), None, "/proc/nope")
            .await
            .expect_err("an uncreatable remote path must fail");
        assert!(!err.is_empty());
    }
}
