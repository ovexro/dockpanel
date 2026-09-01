use crate::safe_cmd::{safe_command, DockerEnvFile};

/// Result of an end-to-end backup drill (extract → scratch container → HTTP probe → teardown).
/// Distinct from `backup_verify::VerificationResult`: a drill *runs* the restored backup,
/// it doesn't just validate the archive.
#[derive(serde::Serialize)]
pub struct DrillResult {
    pub passed: bool,
    pub http_status: Option<i32>,
    pub body_excerpt: Option<String>,
    pub error_message: Option<String>,
    pub duration_ms: u64,
}

fn drill_failure(start: std::time::Instant, msg: impl Into<String>) -> DrillResult {
    DrillResult {
        passed: false,
        http_status: None,
        body_excerpt: None,
        error_message: Some(msg.into()),
        duration_ms: start.elapsed().as_millis() as u64,
    }
}

/// Site drill: extract the backup tar to a scratch dir, mount it read-only into a fresh
/// `nginx:alpine` container with `--network none`, probe via `docker exec wget`, tear everything down.
///
/// Probe success criteria: nginx returns *any* HTTP response (even 403/404 means
/// the container booted and the mount worked). Total HTTP failure (no response,
/// connection refused, exec error) is the failure signal.
pub async fn drill_site_backup(domain: &str, filename: &str) -> Result<DrillResult, String> {
    let start = std::time::Instant::now();

    // Validation mirrors backup_verify::verify_site_backup.
    if filename.is_empty() || filename.contains("..") || filename.contains('/') {
        return Err("Invalid filename".to_string());
    }

    let backup_path = format!("/var/backups/dockpanel/{domain}/{filename}");
    if !std::path::Path::new(&backup_path).exists() {
        return Err("Backup file not found".to_string());
    }

    let drill_id = uuid::Uuid::new_v4().to_string();
    let scratch_dir = format!("/var/lib/dockpanel/drills/{drill_id}");
    let container_name = format!("dockpanel-drill-{}", &drill_id[..8]);

    // Always tear down on exit. Use a guard pattern via an inner async block.
    let result = run_site_drill(&backup_path, &scratch_dir, &container_name, start).await;

    // Cleanup container (best-effort — `--rm` should already handle it but make sure).
    let _ = safe_command("docker")
        .args(["rm", "-f", &container_name])
        .output()
        .await;

    // Cleanup scratch dir (best-effort).
    let _ = std::fs::remove_dir_all(&scratch_dir);

    Ok(result)
}

async fn run_site_drill(
    backup_path: &str,
    scratch_dir: &str,
    container_name: &str,
    start: std::time::Instant,
) -> DrillResult {
    // 1. Create scratch dir.
    if let Err(e) = std::fs::create_dir_all(scratch_dir) {
        return drill_failure(start, format!("scratch dir: {e}"));
    }

    // 2. Extract tar with timeout.
    let extract = tokio::time::timeout(
        std::time::Duration::from_secs(120),
        safe_command("tar")
            .args(["xzf", backup_path, "-C", scratch_dir, "--no-same-owner", "--no-same-permissions"])
            .output(),
    )
    .await;

    let extract_ok = extract
        .map(|r| r.map(|o| o.status.success()).unwrap_or(false))
        .unwrap_or(false);
    if !extract_ok {
        return drill_failure(start, "tar extract failed");
    }

    // 3. Spin nginx:alpine on the scratch dir, read-only mount, no network.
    //    --network none is intentional: a malicious backup can't phone home.
    //    Loopback inside the container still works so wget localhost still does.
    let run = tokio::time::timeout(
        std::time::Duration::from_secs(30),
        safe_command("docker")
            .args([
                "run", "--rm", "-d",
                "--name", container_name,
                "--network", "none",
                "--memory=128m",
                "--cpus=0.5",
                "--read-only",
                "--tmpfs", "/var/cache/nginx",
                "--tmpfs", "/var/run",
                "-v", &format!("{scratch_dir}:/usr/share/nginx/html:ro"),
                "nginx:alpine",
            ])
            .output(),
    )
    .await;

    let started = run
        .map(|r| r.map(|o| o.status.success()).unwrap_or(false))
        .unwrap_or(false);
    if !started {
        return drill_failure(start, "nginx scratch container failed to start");
    }

    // 4. Wait briefly for nginx to bind. Alpine nginx is fast (~200ms on a warm node).
    tokio::time::sleep(std::time::Duration::from_millis(500)).await;

    // 5. Probe via `docker exec wget`. nginx:alpine ships busybox wget.
    //    --server-response prints the status line to stderr; -O - emits body to stdout.
    //    -T 5 caps probe at 5s.
    let probe = tokio::time::timeout(
        std::time::Duration::from_secs(15),
        safe_command("docker")
            .args([
                "exec", container_name,
                "wget", "-q", "-O", "-", "--server-response", "-T", "5",
                "http://localhost/",
            ])
            .output(),
    )
    .await;

    match probe {
        Ok(Ok(out)) => {
            let stderr = String::from_utf8_lossy(&out.stderr);
            let stdout = String::from_utf8_lossy(&out.stdout);
            let http_status = parse_http_status(&stderr);
            let body_excerpt = if stdout.is_empty() { None } else {
                Some(stdout.chars().take(500).collect::<String>())
            };

            // wget exit 0 = 2xx response, exit 8 = server returned non-2xx.
            // Both mean "nginx is alive and serving". Exit 4 = network failure, that's a fail.
            let passed = http_status.is_some();

            DrillResult {
                passed,
                http_status,
                body_excerpt,
                error_message: if passed { None } else {
                    Some(format!("probe got no HTTP response (wget stderr: {})", stderr.chars().take(200).collect::<String>()))
                },
                duration_ms: start.elapsed().as_millis() as u64,
            }
        }
        Ok(Err(e)) => drill_failure(start, format!("docker exec failed: {e}")),
        Err(_) => drill_failure(start, "probe timeout"),
    }
}

/// Parse HTTP status from busybox wget --server-response stderr. Looks for
/// the first line matching `  HTTP/1.x NNN`.
fn parse_http_status(stderr: &str) -> Option<i32> {
    for line in stderr.lines() {
        let trimmed = line.trim_start();
        if let Some(rest) = trimmed.strip_prefix("HTTP/") {
            // rest: "1.0 200 OK" or "1.1 404 Not Found"
            let mut parts = rest.split_whitespace();
            let _ver = parts.next()?;
            let code = parts.next()?.parse::<i32>().ok()?;
            return Some(code);
        }
    }
    None
}

// ── Database drills (W1.2.b) ────────────────────────────────────────────────
//
// Distinct from `verify_db_backup` which only confirms the dump *applies*
// (table count from information_schema > 0). A drill goes one step further:
// after restore, it sums actual row counts across user tables. Pass = data
// is queryable post-restore, not just schema applied.

fn is_valid_db_name(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= 128
        && name.chars().all(|c| c.is_alphanumeric() || c == '-' || c == '_' || c == '.')
}

/// DB drill: spin a temp engine container, restore the gzipped dump, then
/// sum row counts across user tables. Pass = restore succeeded AND total
/// rows > 0 (or schema-only databases pass with rows == 0 IFF tables > 0).
pub async fn drill_db_backup(
    db_type: &str,
    db_name: &str,
    filename: &str,
) -> Result<DrillResult, String> {
    let start = std::time::Instant::now();

    if !is_valid_db_name(db_name) {
        return Err("Invalid database name".to_string());
    }
    if !is_valid_db_name(filename) {
        return Err("Invalid filename".to_string());
    }

    let backup_path = format!("/var/backups/dockpanel/databases/{db_name}/{filename}");
    if !std::path::Path::new(&backup_path).exists() {
        return Err("Backup file not found".to_string());
    }

    let drill_id = uuid::Uuid::new_v4().to_string();
    let container_name = format!("dockpanel-drill-db-{}", &drill_id[..8]);
    let test_password = "drill_test_pass_12345";

    let result = match db_type {
        "mysql" | "mariadb" => {
            run_mysql_drill(&backup_path, &container_name, db_name, test_password, start).await
        }
        "postgres" | "postgresql" => {
            run_postgres_drill(&backup_path, &container_name, db_name, test_password, start).await
        }
        _ => drill_failure(start, format!("Unsupported DB type for drill: {db_type}")),
    };

    // Best-effort cleanup of the scratch container.
    let _ = safe_command("docker")
        .args(["rm", "-f", &container_name])
        .output()
        .await;

    Ok(result)
}

async fn run_mysql_drill(
    backup_path: &str,
    container_name: &str,
    db_name: &str,
    password: &str,
    start: std::time::Instant,
) -> DrillResult {
    // 1. Spin temp mariadb. Hardened: --network none (loopback works for the
    //    in-container psql/mariadb client; the engine itself binds 127.0.0.1).
    //    Credential via --env-file, not `-e KEY=value`: see `DockerEnvFile`.
    let run_env_file = match DockerEnvFile::new(&[
        ("MYSQL_DATABASE", db_name),
        ("MYSQL_ROOT_PASSWORD", password),
    ]) {
        Ok(f) => f,
        Err(e) => return drill_failure(start, format!("Failed to prepare credentials: {e}")),
    };
    let start_ok = safe_command("docker")
        .arg("run").arg("-d").arg("--name").arg(container_name)
        .arg("--network").arg("none")
        .arg("--env-file").arg(run_env_file.path())
        .args(["--memory=256m", "--cpus=1.0", "mariadb:11"])
        .output()
        .await
        .map(|o| o.status.success())
        .unwrap_or(false);

    if !start_ok {
        return drill_failure(start, "mariadb scratch container failed to start");
    }

    let exec_env_file = match DockerEnvFile::new(&[("MYSQL_PWD", password)]) {
        Ok(f) => f,
        Err(e) => return drill_failure(start, format!("Failed to prepare credentials: {e}")),
    };

    // 2. Wait for ready (up to 40s).
    let mut ready = false;
    for _ in 0..40 {
        tokio::time::sleep(std::time::Duration::from_secs(1)).await;
        let check = safe_command("docker")
            .arg("exec").arg("--env-file").arg(exec_env_file.path())
            .args([container_name, "mariadb", "-u", "root", "-e", "SELECT 1"])
            .output()
            .await;
        if check.map(|o| o.status.success()).unwrap_or(false) {
            ready = true;
            break;
        }
    }
    if !ready {
        return drill_failure(start, "mariadb container not ready within 40s");
    }

    // 3. Restore via zcat → docker exec mariadb (direct fd pipe, no shell).
    let zcat_child = safe_command("zcat")
        .arg(backup_path)
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::null())
        .spawn();

    let restore_ok = match zcat_child {
        Ok(mut zcat) => match zcat.stdout.take() {
            Some(stdout) => {
                let docker_ok = tokio::time::timeout(
                    std::time::Duration::from_secs(180),
                    safe_command("docker")
                        .arg("exec").arg("-i")
                        .arg("--env-file").arg(exec_env_file.path())
                        .args([container_name, "mariadb", "-u", "root", db_name])
                        .stdin(stdout.into_owned_fd().unwrap())
                        .output(),
                )
                .await
                .map(|x| x.map(|o| o.status.success()).unwrap_or(false)).unwrap_or(false);
                // Require the decompressor to have succeeded: a boundary-truncated .gz makes
                // zcat exit non-zero while mariadb sees a clean EOF and exits 0 (lesson #51).
                // Otherwise the drill — billed as the strongest #51 rehearsal — certifies a
                // corrupt backup as restorable and an operator may prune the only good copy.
                // Bound the decompressor wait: safe_command sets no kill_on_drop, so a timed-out
                // (dropped) docker future leaves docker orphaned holding zcat's pipe read-end — an
                // UNbounded zcat.wait() could then hang the drill forever. zcat exits ~instantly on
                // every non-hang path, so 5s is ample headroom.
                let zcat_ok = tokio::time::timeout(std::time::Duration::from_secs(5), zcat.wait())
                    .await.ok().and_then(|r| r.ok()).map(|s| s.success()).unwrap_or(false);
                docker_ok && zcat_ok
            }
            None => false,
        },
        Err(_) => false,
    };
    if !restore_ok {
        return drill_failure(start, "mariadb restore failed");
    }

    // 4. Smoke query: count user tables AND sum rows. Schema-only dumps pass
    //    with rows == 0 as long as tables > 0 (legitimate edge case).
    let table_count = run_mysql_scalar(
        container_name, password, db_name,
        "SELECT COUNT(*) FROM information_schema.tables WHERE table_schema=DATABASE()",
    ).await;

    if table_count == 0 {
        return drill_failure(start, "no tables present after restore");
    }

    // information_schema.tables.table_rows is approximate for InnoDB but good
    // enough for drill semantics ("is there *any* data?"). Cheaper than
    // SELECT COUNT(*) per table.
    let row_total = run_mysql_scalar(
        container_name, password, db_name,
        "SELECT IFNULL(SUM(table_rows),0) FROM information_schema.tables WHERE table_schema=DATABASE()",
    ).await;

    DrillResult {
        passed: true,
        http_status: None,
        body_excerpt: Some(format!("{table_count} tables, ~{row_total} rows restored")),
        error_message: None,
        duration_ms: start.elapsed().as_millis() as u64,
    }
}

async fn run_mysql_scalar(container_name: &str, password: &str, db_name: &str, sql: &str) -> i64 {
    let Ok(env_file) = DockerEnvFile::new(&[("MYSQL_PWD", password)]) else {
        return 0;
    };
    safe_command("docker")
        .arg("exec").arg("--env-file").arg(env_file.path())
        .args([
            container_name, "mariadb", "-u", "root", db_name,
            "-e", sql, "--batch", "--skip-column-names",
        ])
        .output()
        .await
        .map(|o| String::from_utf8_lossy(&o.stdout).trim().parse::<i64>().unwrap_or(0))
        .unwrap_or(0)
}

async fn run_postgres_drill(
    backup_path: &str,
    container_name: &str,
    db_name: &str,
    password: &str,
    start: std::time::Instant,
) -> DrillResult {
    // 1. Spin temp postgres. Credential via --env-file, not `-e KEY=value`:
    //    see `DockerEnvFile`.
    let run_env_file = match DockerEnvFile::new(&[
        ("POSTGRES_DB", db_name),
        ("POSTGRES_USER", "drill"),
        ("POSTGRES_PASSWORD", password),
    ]) {
        Ok(f) => f,
        Err(e) => return drill_failure(start, format!("Failed to prepare credentials: {e}")),
    };
    let start_ok = safe_command("docker")
        .arg("run").arg("-d").arg("--name").arg(container_name)
        .arg("--network").arg("none")
        .arg("--env-file").arg(run_env_file.path())
        .args(["--memory=256m", "--cpus=1.0", "postgres:16-alpine"])
        .output()
        .await
        .map(|o| o.status.success())
        .unwrap_or(false);

    if !start_ok {
        return drill_failure(start, "postgres scratch container failed to start");
    }

    let exec_env_file = match DockerEnvFile::new(&[("PGPASSWORD", password)]) {
        Ok(f) => f,
        Err(e) => return drill_failure(start, format!("Failed to prepare credentials: {e}")),
    };

    // 2. Wait for ready.
    let mut ready = false;
    for _ in 0..30 {
        tokio::time::sleep(std::time::Duration::from_secs(1)).await;
        let check = safe_command("docker")
            .arg("exec").arg("--env-file").arg(exec_env_file.path())
            .args([container_name, "psql", "-U", "drill", "-d", db_name, "-c", "SELECT 1"])
            .output()
            .await;
        if check.map(|o| o.status.success()).unwrap_or(false) {
            ready = true;
            break;
        }
    }
    if !ready {
        return drill_failure(start, "postgres container not ready within 30s");
    }

    // 3. Restore via zcat → psql.
    let zcat_child = safe_command("zcat")
        .arg(backup_path)
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::null())
        .spawn();

    let restore_ok = match zcat_child {
        Ok(mut zcat) => match zcat.stdout.take() {
            Some(stdout) => {
                let docker_ok = tokio::time::timeout(
                    std::time::Duration::from_secs(180),
                    safe_command("docker")
                        .arg("exec").arg("-i")
                        .arg("--env-file").arg(exec_env_file.path())
                        .args([
                            container_name,
                            "psql", "-U", "drill", "-d", db_name, "--quiet",
                            // ON_ERROR_STOP alone stops at the first failed
                            // statement but leaves everything before it applied,
                            // so the drill's table/row counts below could still
                            // describe a partial restore. --single-transaction
                            // makes the drill an all-or-nothing rehearsal of the
                            // real thing (lesson #51).
                            "-v", "ON_ERROR_STOP=1", "--single-transaction",
                        ])
                        .stdin(stdout.into_owned_fd().unwrap())
                        .output(),
                )
                .await
                .map(|x| x.map(|o| o.status.success()).unwrap_or(false)).unwrap_or(false);
                // --single-transaction rolls back a mid-statement failure, but a .gz cut on a
                // clean statement boundary still lets psql COMMIT the partial prefix and exit 0.
                // Require the decompressor to have succeeded too (lesson #51) — otherwise the
                // strongest restore rehearsal green-lights a corrupt backup.
                // Bound the decompressor wait: safe_command sets no kill_on_drop, so a timed-out
                // (dropped) docker future leaves docker orphaned holding zcat's pipe read-end — an
                // UNbounded zcat.wait() could then hang the drill forever. zcat exits ~instantly on
                // every non-hang path, so 5s is ample headroom.
                let zcat_ok = tokio::time::timeout(std::time::Duration::from_secs(5), zcat.wait())
                    .await.ok().and_then(|r| r.ok()).map(|s| s.success()).unwrap_or(false);
                docker_ok && zcat_ok
            }
            None => false,
        },
        Err(_) => false,
    };
    if !restore_ok {
        return drill_failure(start, "postgres restore failed");
    }

    // 4. Table count + row total via pg_class.reltuples (planner stats; cheap).
    let table_count = run_psql_scalar(
        container_name, password, db_name,
        "SELECT COUNT(*) FROM information_schema.tables WHERE table_schema='public'",
    ).await;
    if table_count == 0 {
        return drill_failure(start, "no tables present in public schema after restore");
    }

    // ANALYZE to populate reltuples (fresh restore has stats=0). Bounded to 30s
    // so a giant dump doesn't hold the drill open.
    let _ = tokio::time::timeout(
        std::time::Duration::from_secs(30),
        safe_command("docker")
            .arg("exec").arg("--env-file").arg(exec_env_file.path())
            .args([container_name, "psql", "-U", "drill", "-d", db_name, "-c", "ANALYZE"])
            .output(),
    )
    .await;

    let row_total = run_psql_scalar(
        container_name, password, db_name,
        "SELECT COALESCE(SUM(reltuples)::bigint, 0) FROM pg_class WHERE relkind='r' AND relnamespace=(SELECT oid FROM pg_namespace WHERE nspname='public')",
    ).await;

    DrillResult {
        passed: true,
        http_status: None,
        body_excerpt: Some(format!("{table_count} tables, ~{row_total} rows restored")),
        error_message: None,
        duration_ms: start.elapsed().as_millis() as u64,
    }
}

async fn run_psql_scalar(container_name: &str, password: &str, db_name: &str, sql: &str) -> i64 {
    let Ok(env_file) = DockerEnvFile::new(&[("PGPASSWORD", password)]) else {
        return 0;
    };
    safe_command("docker")
        .arg("exec").arg("--env-file").arg(env_file.path())
        .args([container_name, "psql", "-U", "drill", "-d", db_name, "-t", "-A", "-c", sql])
        .output()
        .await
        .map(|o| String::from_utf8_lossy(&o.stdout).trim().parse::<i64>().unwrap_or(0))
        .unwrap_or(0)
}

// ── Volume drills (W1.2.c) ──────────────────────────────────────────────────
//
// Distinct from `verify_volume_backup` which only extracts the tar to a /tmp
// dir on the host filesystem. A drill goes one step further: it restores into
// a real Docker volume (parity with `restore_volume`'s actual restore path),
// then mounts that volume into a fresh probe container that read-tests a
// sample of files. Pass = tar extracts into the volume AND a sample of files
// is byte-readable through a fresh mount.

/// Volume drill: scratch Docker volume + restore container + probe container.
///
/// Stronger than verify because verify extracts to a host-side /tmp dir;
/// drill exercises the actual Docker volume driver / mount path that real
/// restores use. The probe's read-test catches filesystem-level corruption
/// that a pure file-count check would miss.
pub async fn drill_volume_backup(
    container_name: &str,
    filename: &str,
) -> Result<DrillResult, String> {
    let start = std::time::Instant::now();

    if !is_valid_db_name(container_name) {
        return Err("Invalid container name".to_string());
    }
    if !is_valid_db_name(filename) {
        return Err("Invalid filename".to_string());
    }

    let backup_dir = format!("/var/backups/dockpanel/volumes/{container_name}");
    let backup_path = format!("{backup_dir}/{filename}");
    if !std::path::Path::new(&backup_path).exists() {
        return Err("Backup file not found".to_string());
    }

    let drill_id = uuid::Uuid::new_v4().to_string();
    let scratch_volume = format!("dockpanel-drill-vol-{}", &drill_id[..8]);
    let restore_container = format!("dockpanel-drill-vrestore-{}", &drill_id[..8]);
    let probe_container = format!("dockpanel-drill-vprobe-{}", &drill_id[..8]);

    let result = run_volume_drill(
        &backup_dir, filename, &scratch_volume,
        &restore_container, &probe_container, start,
    ).await;

    // Best-effort cleanup on every exit path. Containers first (they hold
    // the volume open), then the volume itself.
    let _ = safe_command("docker").args(["rm", "-f", &restore_container]).output().await;
    let _ = safe_command("docker").args(["rm", "-f", &probe_container]).output().await;
    let _ = safe_command("docker").args(["volume", "rm", "-f", &scratch_volume]).output().await;

    Ok(result)
}

async fn run_volume_drill(
    backup_dir: &str,
    filename: &str,
    scratch_volume: &str,
    restore_container: &str,
    probe_container: &str,
    start: std::time::Instant,
) -> DrillResult {
    // 1. Create scratch volume.
    let vol_ok = safe_command("docker")
        .args(["volume", "create", scratch_volume])
        .output()
        .await
        .map(|o| o.status.success())
        .unwrap_or(false);
    if !vol_ok {
        return drill_failure(start, "scratch volume create failed");
    }

    // 2. Restore container: alpine, --network none, mounts scratch volume RW
    //    + backup dir RO. Pipes the tar through tar xzf — same shape as the
    //    real `restore_volume`. tmpfs writable spots so rootfs can stay RO.
    let restore = tokio::time::timeout(
        std::time::Duration::from_secs(300),
        safe_command("docker")
            .args([
                "run", "--rm",
                "--name", restore_container,
                "--network", "none",
                "--memory=128m",
                "--cpus=0.5",
                "-v", &format!("{scratch_volume}:/vol"),
                "-v", &format!("{backup_dir}:/backup:ro"),
                "alpine:3.19",
                "tar", "xzf", &format!("/backup/{filename}"),
                "-C", "/vol",
                "--no-same-owner", "--no-same-permissions",
            ])
            .output(),
    )
    .await;

    let restore_ok = restore
        .map(|r| r.map(|o| o.status.success()).unwrap_or(false))
        .unwrap_or(false);
    if !restore_ok {
        return drill_failure(start, "tar restore into scratch volume failed");
    }

    // 3. Probe container: alpine, --network none, --read-only rootfs, mounts
    //    the scratch volume RO. Counts files, sums bytes, AND read-tests up
    //    to 20 sample files (1 byte each — enough to fault filesystem-level
    //    corruption without scanning multi-GB volumes).
    //
    //    `set -e` so any pipeline failure kills the probe; explicit `wc -l`
    //    output is captured separately for the body excerpt.
    let probe = tokio::time::timeout(
        std::time::Duration::from_secs(60),
        safe_command("docker")
            .args([
                "run", "--rm",
                "--name", probe_container,
                "--network", "none",
                "--memory=128m",
                "--cpus=0.5",
                "--read-only",
                "--tmpfs", "/tmp",
                "-v", &format!("{scratch_volume}:/vol:ro"),
                "alpine:3.19",
                "sh", "-c",
                "set -e; \
                 files=$(find /vol -type f 2>/dev/null | wc -l); \
                 bytes=$(du -sb /vol 2>/dev/null | awk '{print $1}'); \
                 find /vol -type f 2>/dev/null | head -20 | xargs -r -I{} head -c 1 \"{}\" > /dev/null; \
                 echo \"FILES=$files BYTES=$bytes\"",
            ])
            .output(),
    )
    .await;

    match probe {
        Ok(Ok(out)) if out.status.success() => {
            let stdout = String::from_utf8_lossy(&out.stdout);
            let (files, bytes) = parse_volume_probe(&stdout);
            if files == 0 {
                return drill_failure(start, "no files in restored volume");
            }
            DrillResult {
                passed: true,
                http_status: None,
                body_excerpt: Some(format!("{files} files, {bytes} bytes restored")),
                error_message: None,
                duration_ms: start.elapsed().as_millis() as u64,
            }
        }
        Ok(Ok(out)) => {
            let stderr = String::from_utf8_lossy(&out.stderr);
            drill_failure(start, format!("probe failed: {}", stderr.chars().take(200).collect::<String>()))
        }
        Ok(Err(e)) => drill_failure(start, format!("docker probe error: {e}")),
        Err(_) => drill_failure(start, "volume probe timeout"),
    }
}

/// Parse `FILES=N BYTES=M` line emitted by the probe shell.
fn parse_volume_probe(stdout: &str) -> (i64, i64) {
    let mut files = 0i64;
    let mut bytes = 0i64;
    for line in stdout.lines() {
        for tok in line.split_whitespace() {
            if let Some(rest) = tok.strip_prefix("FILES=") {
                files = rest.parse().unwrap_or(0);
            } else if let Some(rest) = tok.strip_prefix("BYTES=") {
                bytes = rest.parse().unwrap_or(0);
            }
        }
    }
    (files, bytes)
}
