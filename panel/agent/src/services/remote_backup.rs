use std::path::Path;
use crate::safe_cmd::safe_command;

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

/// Build the ssh/scp options every SFTP operation shares.
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
/// `port_flag` is the single genuine difference: `scp` takes `-P`, `ssh` takes `-p`.
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
fn sftp_opts(
    port_flag: &str,
    port: u16,
    password: Option<&str>,
    key_path: Option<&str>,
) -> Vec<String> {
    let mut args: Vec<String> = vec![
        "-o".into(),
        "StrictHostKeyChecking=accept-new".into(),
        "-o".into(),
        "UserKnownHostsFile=/etc/dockpanel/known_hosts".into(),
        "-o".into(),
        "ConnectTimeout=10".into(),
        port_flag.into(),
        port.to_string(),
    ];
    if key_path.is_some() || password.is_none() {
        args.push("-o".into());
        args.push("BatchMode=yes".into());
    }
    if let Some(key) = key_path {
        args.push("-i".into());
        args.push(key.into());
    }
    args
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

    tokio::time::timeout(std::time::Duration::from_secs(timeout_secs), cmd.output())
        .await
        .map_err(|_| format!("{label} timed out"))?
        .map_err(|e| {
            // `sshpass` is a separate package and the agent shells out to it for
            // every password-authenticated destination. When it is absent the raw
            // error is "No such file or directory (os error 2)", which names neither
            // the binary nor the remedy — s289 measured exactly that, as an opaque
            // 502, on a fresh install where nothing had installed it.
            if prog == "sshpass" && e.kind() == std::io::ErrorKind::NotFound {
                "sshpass is not installed on this server, and it is required for \
                 password-authenticated SFTP destinations. Install it (apt-get install \
                 sshpass / dnf install sshpass) or use an SSH key instead."
                    .to_string()
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

    let remote_dest = format!(
        "{username}@{host}:{}/{}",
        remote_path.trim_end_matches('/'),
        filename
    );

    tracing::info!("Uploading {filename} via SCP to {remote_dest}");

    // Create the destination directory first.
    //
    // Nothing ever did. `scp` will not create a missing target directory, so a
    // destination whose `remote_path` did not already exist failed every upload with
    // *"dest open ...: No such file or directory"* — while Test Connection, which
    // never went near `remote_path`, reported success. The default is `/backups`,
    // an absolute path that exists on almost no server, so this was the common case
    // rather than an edge one (measured s289).
    let dir = remote_path.trim_end_matches('/');
    if !dir.is_empty() {
        let mut mkdir_args = sftp_opts("-p", port, password, key_path);
        mkdir_args.push(format!("{username}@{host}"));
        // Single-quoted so a path with spaces survives the remote shell; embedded
        // single quotes are escaped the POSIX way.
        mkdir_args.push(format!("mkdir -p '{}'", dir.replace('\'', "'\\''")));

        let mk = run_sftp("ssh", mkdir_args, password, key_path, 30, "Remote directory creation").await?;
        if !mk.status.success() {
            let stderr = String::from_utf8_lossy(&mk.stderr);
            return Err(format!(
                "Could not create remote directory '{dir}': {}",
                stderr.trim()
            ));
        }
    }

    let mut cmd_args = sftp_opts("-P", port, password, key_path);
    cmd_args.push(filepath.into());
    cmd_args.push(remote_dest.clone());

    let output = run_sftp("scp", cmd_args, password, key_path, 600, "Upload").await?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(format!("SCP upload failed: {}", stderr.trim()));
    }

    tracing::info!("SCP upload complete: {filename}");
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
    let quoted = dir.replace('\'', "'\\''");

    // mkdir -p, write a probe, remove it — the three things an upload needs, in one
    // round trip. `cd` first so a relative `remote_path` resolves the same way scp
    // resolves it (against the login directory).
    let script = if dir.is_empty() {
        "exit 0".to_string()
    } else {
        format!(
            "mkdir -p '{quoted}' && : > '{quoted}/.dockpanel-write-probe' && rm -f '{quoted}/.dockpanel-write-probe'"
        )
    };

    let mut cmd_args = sftp_opts("-p", port, password, key_path);
    cmd_args.push(format!("{username}@{host}"));
    cmd_args.push(script);

    let output = run_sftp("ssh", cmd_args, password, key_path, 20, "Connection test").await?;

    if output.status.success() {
        Ok(())
    } else {
        let stderr = String::from_utf8_lossy(&output.stderr);
        let detail = stderr.trim();
        // Authentication failures and permission failures read very differently to
        // an operator, and the raw stderr distinguishes them — so pass it through
        // rather than collapsing both into "connection failed".
        Err(format!(
            "SFTP test failed: {}",
            if detail.is_empty() { "no error output from ssh" } else { detail }
        ))
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
