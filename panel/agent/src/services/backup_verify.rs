use crate::safe_cmd::safe_command;

#[derive(serde::Serialize)]
pub struct VerificationResult {
    pub passed: bool,
    pub checks_run: i32,
    pub checks_passed: i32,
    pub details: Vec<VerificationCheck>,
    pub duration_ms: u64,
}

#[derive(serde::Serialize)]
pub struct VerificationCheck {
    pub name: String,
    pub passed: bool,
    pub message: String,
}

/// Verify a site backup: extract to temp dir, check file count and structure.
pub async fn verify_site_backup(domain: &str, filename: &str) -> Result<VerificationResult, String> {
    let start = std::time::Instant::now();
    let mut checks = Vec::new();

    let backup_path = format!("/var/backups/dockpanel/{domain}/{filename}");
    if !std::path::Path::new(&backup_path).exists() {
        return Err("Backup file not found".to_string());
    }

    // Check 1: File exists and has reasonable size
    let meta = std::fs::metadata(&backup_path)
        .map_err(|e| format!("Cannot read file: {e}"))?;
    checks.push(VerificationCheck {
        name: "file_exists".into(),
        passed: meta.len() > 100,
        message: format!("Backup file size: {} bytes", meta.len()),
    });

    // Check 2: Extract to temp dir and verify contents
    let temp_dir = format!("/tmp/dockpanel-verify-{}", uuid::Uuid::new_v4());
    std::fs::create_dir_all(&temp_dir).map_err(|e| format!("Temp dir: {e}"))?;

    let extract_ok = tokio::time::timeout(
        std::time::Duration::from_secs(120),
        safe_command("tar")
            .args(["xzf", &backup_path, "-C", &temp_dir])
            .output(),
    )
    .await
    .map(|r| r.map(|o| o.status.success()).unwrap_or(false))
    .unwrap_or(false);

    checks.push(VerificationCheck {
        name: "extract_success".into(),
        passed: extract_ok,
        message: if extract_ok { "Archive extracts cleanly".into() } else { "Failed to extract archive".into() },
    });

    // Check 3: Count files in extracted directory
    if extract_ok {
        let count_output = safe_command("find")
            .args([&temp_dir, "-type", "f"])
            .output()
            .await;
        let file_count = count_output
            .map(|o| String::from_utf8_lossy(&o.stdout).lines().count())
            .unwrap_or(0);

        checks.push(VerificationCheck {
            name: "file_count".into(),
            passed: file_count > 0,
            message: format!("{file_count} files extracted"),
        });

        // Check 4: Look for common web files (index.php/html, wp-config.php, etc.)
        let has_index = std::path::Path::new(&format!("{temp_dir}/index.php")).exists()
            || std::path::Path::new(&format!("{temp_dir}/index.html")).exists()
            || std::path::Path::new(&format!("{temp_dir}/public/index.html")).exists()
            || std::path::Path::new(&format!("{temp_dir}/public/index.php")).exists();

        checks.push(VerificationCheck {
            name: "has_entry_point".into(),
            passed: has_index,
            message: if has_index { "Entry point found (index.php/html)".into() } else { "No index file found (may be expected for non-web backups)".into() },
        });
    }

    // Cleanup temp dir
    std::fs::remove_dir_all(&temp_dir).ok();

    let checks_run = checks.len() as i32;
    let checks_passed = checks.iter().filter(|c| c.passed).count() as i32;
    let passed = checks_passed >= checks_run - 1; // Allow one non-critical check to fail

    Ok(VerificationResult {
        passed,
        checks_run,
        checks_passed,
        details: checks,
        duration_ms: start.elapsed().as_millis() as u64,
    })
}

/// Validate a resource name (database, app, container, etc.).
fn is_valid_name(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= 128
        && name.chars().all(|c| c.is_alphanumeric() || c == '-' || c == '_' || c == '.')
}

/// Verify a database backup by restoring to a temporary container.
pub async fn verify_db_backup(
    db_type: &str,
    db_name: &str,
    filename: &str,
) -> Result<VerificationResult, String> {
    let start = std::time::Instant::now();
    let mut checks = Vec::new();

    if !is_valid_name(db_name) {
        return Err("Invalid database name".to_string());
    }
    if !is_valid_name(filename) {
        return Err("Invalid filename".to_string());
    }

    let backup_path = format!("/var/backups/dockpanel/databases/{db_name}/{filename}");
    if !std::path::Path::new(&backup_path).exists() {
        return Err("Backup file not found".to_string());
    }

    // Check 1: File size
    let meta = std::fs::metadata(&backup_path)
        .map_err(|e| format!("Cannot read file: {e}"))?;
    checks.push(VerificationCheck {
        name: "file_exists".into(),
        passed: meta.len() > 30,
        message: format!("Dump file size: {} bytes", meta.len()),
    });

    let container_name = format!("dockpanel-verify-{}", &uuid::Uuid::new_v4().to_string()[..8]);
    let test_password = "verify_test_pass_12345";

    // Check 2: Spin up temporary database container and restore
    let restore_ok = match db_type {
        "mysql" | "mariadb" => verify_mysql_restore(&backup_path, &container_name, db_name, test_password, &mut checks).await,
        "postgres" | "postgresql" => verify_postgres_restore(&backup_path, &container_name, db_name, test_password, &mut checks).await,
        _ => {
            checks.push(VerificationCheck {
                name: "restore".into(),
                passed: false,
                message: format!("Unsupported DB type for verification: {db_type}"),
            });
            false
        }
    };

    // Cleanup: remove temporary container
    safe_command("docker")
        .args(["rm", "-f", &container_name])
        .output()
        .await
        .ok();

    let checks_run = checks.len() as i32;
    let checks_passed = checks.iter().filter(|c| c.passed).count() as i32;

    Ok(VerificationResult {
        passed: restore_ok && checks_passed == checks_run,
        checks_run,
        checks_passed,
        details: checks,
        duration_ms: start.elapsed().as_millis() as u64,
    })
}

async fn verify_mysql_restore(
    backup_path: &str,
    container_name: &str,
    db_name: &str,
    password: &str,
    checks: &mut Vec<VerificationCheck>,
) -> bool {
    // Start temp MySQL container
    let start_ok = safe_command("docker")
        .args([
            "run", "-d", "--name", container_name,
            "-e", &format!("MYSQL_DATABASE={db_name}"),
            "-e", &format!("MYSQL_ROOT_PASSWORD={password}"),
            "--memory=256m",
            "mariadb:11",
        ])
        .output()
        .await
        .map(|o| o.status.success())
        .unwrap_or(false);

    if !start_ok {
        checks.push(VerificationCheck {
            name: "temp_container".into(),
            passed: false,
            message: "Failed to start temporary MySQL container".into(),
        });
        return false;
    }

    // Wait for MySQL to be ready (up to 40s)
    let mut ready = false;
    for _ in 0..40 {
        tokio::time::sleep(std::time::Duration::from_secs(1)).await;
        let check = safe_command("docker")
            .args([
                "exec", "-e", &format!("MYSQL_PWD={password}"),
                container_name, "mariadb", "-u", "root", "-e", "SELECT 1",
            ])
            .output()
            .await;
        if check.map(|o| o.status.success()).unwrap_or(false) {
            ready = true;
            break;
        }
    }

    checks.push(VerificationCheck {
        name: "temp_container".into(),
        passed: ready,
        message: if ready { "Temporary MySQL container ready".into() } else { "MySQL container failed to start within 40s".into() },
    });

    if !ready {
        return false;
    }

    // Restore the dump
    let safe_password = password.replace('\'', "'\\''");
    let restore_cmd = format!(
        "zcat '{}' | docker exec -i -e MYSQL_PWD='{}' {} mariadb -u root {}",
        backup_path.replace('\'', "'\\''"),
        safe_password,
        container_name,
        db_name,
    );

    let restore_ok = tokio::time::timeout(
        std::time::Duration::from_secs(120),
        safe_command("bash").args(["-c", &restore_cmd]).output(),
    )
    .await
    .map(|r| r.map(|o| o.status.success()).unwrap_or(false))
    .unwrap_or(false);

    checks.push(VerificationCheck {
        name: "restore_dump".into(),
        passed: restore_ok,
        message: if restore_ok { "Dump restored successfully".into() } else { "Failed to restore dump".into() },
    });

    if !restore_ok {
        return false;
    }

    // Verify: count tables
    let table_count = safe_command("docker")
        .args([
            "exec", "-e", &format!("MYSQL_PWD={password}"),
            container_name, "mariadb", "-u", "root", db_name,
            "-e", "SELECT COUNT(*) FROM information_schema.tables WHERE table_schema=DATABASE()",
            "--batch", "--skip-column-names",
        ])
        .output()
        .await
        .map(|o| {
            String::from_utf8_lossy(&o.stdout).trim().parse::<i32>().unwrap_or(0)
        })
        .unwrap_or(0);

    checks.push(VerificationCheck {
        name: "table_count".into(),
        passed: table_count > 0,
        message: format!("{table_count} tables found after restore"),
    });

    table_count > 0
}

async fn verify_postgres_restore(
    backup_path: &str,
    container_name: &str,
    db_name: &str,
    password: &str,
    checks: &mut Vec<VerificationCheck>,
) -> bool {
    // Start temp PostgreSQL container
    let start_ok = safe_command("docker")
        .args([
            "run", "-d", "--name", container_name,
            "-e", &format!("POSTGRES_DB={db_name}"),
            "-e", "POSTGRES_USER=verify",
            "-e", &format!("POSTGRES_PASSWORD={password}"),
            "--memory=256m",
            "postgres:16-alpine",
        ])
        .output()
        .await
        .map(|o| o.status.success())
        .unwrap_or(false);

    if !start_ok {
        checks.push(VerificationCheck {
            name: "temp_container".into(),
            passed: false,
            message: "Failed to start temporary PostgreSQL container".into(),
        });
        return false;
    }

    // Wait for PostgreSQL to be ready (up to 30s)
    let mut ready = false;
    for _ in 0..30 {
        tokio::time::sleep(std::time::Duration::from_secs(1)).await;
        let check = safe_command("docker")
            .args([
                "exec", "-e", &format!("PGPASSWORD={password}"),
                container_name, "psql", "-U", "verify", "-d", db_name, "-c", "SELECT 1",
            ])
            .output()
            .await;
        if check.map(|o| o.status.success()).unwrap_or(false) {
            ready = true;
            break;
        }
    }

    checks.push(VerificationCheck {
        name: "temp_container".into(),
        passed: ready,
        message: if ready { "Temporary PostgreSQL container ready".into() } else { "PostgreSQL container failed to start within 30s".into() },
    });

    if !ready {
        return false;
    }

    // Restore the dump
    let safe_password = password.replace('\'', "'\\''");
    let restore_cmd = format!(
        "zcat '{}' | docker exec -i -e PGPASSWORD='{}' {} psql -U verify -d {} --quiet",
        backup_path.replace('\'', "'\\''"),
        safe_password,
        container_name,
        db_name,
    );

    let restore_ok = tokio::time::timeout(
        std::time::Duration::from_secs(120),
        safe_command("bash").args(["-c", &restore_cmd]).output(),
    )
    .await
    .map(|r| r.map(|o| o.status.success()).unwrap_or(false))
    .unwrap_or(false);

    checks.push(VerificationCheck {
        name: "restore_dump".into(),
        passed: restore_ok,
        message: if restore_ok { "Dump restored successfully".into() } else { "Failed to restore dump".into() },
    });

    if !restore_ok {
        return false;
    }

    // Verify: count tables
    let table_count = safe_command("docker")
        .args([
            "exec", "-e", &format!("PGPASSWORD={password}"),
            container_name, "psql", "-U", "verify", "-d", db_name,
            "-t", "-c", "SELECT COUNT(*) FROM information_schema.tables WHERE table_schema='public'",
        ])
        .output()
        .await
        .map(|o| {
            String::from_utf8_lossy(&o.stdout).trim().parse::<i32>().unwrap_or(0)
        })
        .unwrap_or(0);

    checks.push(VerificationCheck {
        name: "table_count".into(),
        passed: table_count > 0,
        message: format!("{table_count} tables found after restore"),
    });

    table_count > 0
}

/// Verify a volume backup: extract to temp dir, check contents.
pub async fn verify_volume_backup(
    container_name: &str,
    filename: &str,
) -> Result<VerificationResult, String> {
    let start = std::time::Instant::now();
    let mut checks = Vec::new();

    if !is_valid_name(container_name) {
        return Err("Invalid container name".to_string());
    }
    if !is_valid_name(filename) {
        return Err("Invalid filename".to_string());
    }

    let backup_path = format!("/var/backups/dockpanel/volumes/{container_name}/{filename}");
    if !std::path::Path::new(&backup_path).exists() {
        return Err("Backup file not found".to_string());
    }

    // Check 1: File size
    let meta = std::fs::metadata(&backup_path)
        .map_err(|e| format!("Cannot read file: {e}"))?;
    checks.push(VerificationCheck {
        name: "file_exists".into(),
        passed: meta.len() > 50,
        message: format!("Volume backup size: {} bytes", meta.len()),
    });

    // Check 2: Extract to temp dir
    let temp_dir = format!("/tmp/dockpanel-verify-{}", uuid::Uuid::new_v4());
    std::fs::create_dir_all(&temp_dir).map_err(|e| format!("Temp dir: {e}"))?;

    let extract_ok = tokio::time::timeout(
        std::time::Duration::from_secs(120),
        safe_command("tar")
            .args(["xzf", &backup_path, "-C", &temp_dir])
            .output(),
    )
    .await
    .map(|r| r.map(|o| o.status.success()).unwrap_or(false))
    .unwrap_or(false);

    checks.push(VerificationCheck {
        name: "extract_success".into(),
        passed: extract_ok,
        message: if extract_ok { "Archive extracts cleanly".into() } else { "Failed to extract archive".into() },
    });

    // Check 3: Count files
    if extract_ok {
        let count_output = safe_command("find")
            .args([&temp_dir, "-type", "f"])
            .output()
            .await;
        let file_count = count_output
            .map(|o| String::from_utf8_lossy(&o.stdout).lines().count())
            .unwrap_or(0);

        checks.push(VerificationCheck {
            name: "file_count".into(),
            passed: file_count > 0,
            message: format!("{file_count} files in volume backup"),
        });
    }

    // Cleanup
    std::fs::remove_dir_all(&temp_dir).ok();

    let checks_run = checks.len() as i32;
    let checks_passed = checks.iter().filter(|c| c.passed).count() as i32;

    Ok(VerificationResult {
        passed: checks_passed == checks_run,
        checks_run,
        checks_passed,
        details: checks,
        duration_ms: start.elapsed().as_millis() as u64,
    })
}
