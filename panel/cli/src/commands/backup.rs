use crate::client;

pub async fn cmd_backup_create(token: &str, domain: &str) -> Result<(), String> {
    println!("Creating backup for {domain}...");

    let result = client::agent_post_empty(&format!("/backups/{domain}/create"), token).await?;

    let filename = result["filename"].as_str().unwrap_or("unknown");
    let size = result["size_bytes"].as_u64().unwrap_or(0);
    let size_mb = size as f64 / 1_048_576.0;

    println!("\x1b[32m✓\x1b[0m Backup created");
    println!("  File:    {filename}");
    println!("  Size:    {size_mb:.1} MB");

    Ok(())
}

pub async fn cmd_backup_list(token: &str, domain: &str, output: &str) -> Result<(), String> {
    let backups = client::agent_get(&format!("/backups/{domain}/list"), token).await?;

    if output == "json" {
        println!("{}", serde_json::to_string_pretty(&backups).unwrap_or_default());
        return Ok(());
    }

    let backups = backups.as_array().ok_or("Expected array from /backups")?;

    if backups.is_empty() {
        println!("No backups for {domain}.");
        return Ok(());
    }

    println!(
        "\x1b[1m{:<40} {:<12} {:<20}\x1b[0m",
        "FILENAME", "SIZE", "CREATED"
    );

    for b in backups {
        let filename = b["filename"].as_str().unwrap_or("-");
        let size = b["size_bytes"].as_u64().unwrap_or(0);
        let size_mb = size as f64 / 1_048_576.0;
        let created = b["created"].as_str().unwrap_or("-");

        println!(
            "{:<40} {:<12} {:<20}",
            filename,
            format!("{size_mb:.1} MB"),
            created
        );
    }

    println!("\n{} backup(s)", backups.len());
    Ok(())
}

pub async fn cmd_backup_restore(token: &str, domain: &str, filename: &str) -> Result<(), String> {
    println!("Restoring {domain} from {filename}...");

    let result = client::agent_post_empty(
        &format!("/backups/{domain}/restore/{filename}"),
        token,
    )
    .await?;

    if result["success"].as_bool() == Some(true) {
        println!("\x1b[32m✓\x1b[0m Backup restored");
    } else {
        let msg = result["message"].as_str().unwrap_or("Unknown error");
        return Err(format!("Failed to restore backup: {msg}"));
    }

    Ok(())
}

pub async fn cmd_backup_delete(token: &str, domain: &str, filename: &str) -> Result<(), String> {
    println!("Deleting backup {filename}...");

    let result = client::agent_delete(
        &format!("/backups/{domain}/{filename}"),
        token,
    )
    .await?;

    if result["success"].as_bool() == Some(true) {
        println!("\x1b[32m✓\x1b[0m Backup deleted");
    } else {
        let msg = result["message"].as_str().unwrap_or("Unknown error");
        return Err(format!("Failed to delete backup: {msg}"));
    }

    Ok(())
}

pub async fn cmd_db_backup_create(
    token: &str, container: &str, db_name: &str, db_type: &str, user: &str, password: &str,
) -> Result<(), String> {
    println!("Creating {db_type} backup for {db_name}...");

    let body = serde_json::json!({
        "container_name": container,
        "db_name": db_name,
        "db_type": db_type,
        "user": user,
        "password": password,
    });

    let result = client::agent_post("/db-backups/dump", &body, token).await?;

    let filename = result["filename"].as_str().unwrap_or("unknown");
    let size = result["size_bytes"].as_u64().unwrap_or(0);
    let size_mb = size as f64 / 1_048_576.0;

    println!("\x1b[32m✓\x1b[0m Database backup created");
    println!("  File:    {filename}");
    println!("  Size:    {size_mb:.1} MB");

    Ok(())
}

pub async fn cmd_db_backup_list(token: &str, db_name: &str, output: &str) -> Result<(), String> {
    let backups = client::agent_get(&format!("/db-backups/{db_name}/list"), token).await?;

    if output == "json" {
        println!("{}", serde_json::to_string_pretty(&backups).unwrap_or_default());
        return Ok(());
    }

    let backups = backups.as_array().ok_or("Expected array")?;

    if backups.is_empty() {
        println!("No database backups for {db_name}.");
        return Ok(());
    }

    println!("\x1b[1m{:<50} {:<12} {:<20}\x1b[0m", "FILENAME", "SIZE", "CREATED");

    for b in backups {
        let filename = b["filename"].as_str().unwrap_or("-");
        let size = b["size_bytes"].as_u64().unwrap_or(0);
        let size_mb = size as f64 / 1_048_576.0;
        let created = b["created_at"].as_str().unwrap_or("-");
        println!("{:<50} {:<12} {:<20}", filename, format!("{size_mb:.1} MB"), created);
    }

    println!("\n{} backup(s)", backups.len());
    Ok(())
}

pub async fn cmd_vol_backup_create(token: &str, volume: &str, container: &str) -> Result<(), String> {
    println!("Creating volume backup for {volume}...");

    let body = serde_json::json!({
        "volume_name": volume,
        "container_name": container,
    });

    let result = client::agent_post("/volume-backups/create", &body, token).await?;

    let filename = result["filename"].as_str().unwrap_or("unknown");
    let size = result["size_bytes"].as_u64().unwrap_or(0);
    let size_mb = size as f64 / 1_048_576.0;

    println!("\x1b[32m✓\x1b[0m Volume backup created");
    println!("  File:    {filename}");
    println!("  Size:    {size_mb:.1} MB");

    Ok(())
}

pub async fn cmd_vol_backup_list(token: &str, container: &str, output: &str) -> Result<(), String> {
    let backups = client::agent_get(&format!("/volume-backups/{container}/list"), token).await?;

    if output == "json" {
        println!("{}", serde_json::to_string_pretty(&backups).unwrap_or_default());
        return Ok(());
    }

    let backups = backups.as_array().ok_or("Expected array")?;

    if backups.is_empty() {
        println!("No volume backups for {container}.");
        return Ok(());
    }

    println!("\x1b[1m{:<50} {:<12} {:<20}\x1b[0m", "FILENAME", "SIZE", "CREATED");

    for b in backups {
        let filename = b["filename"].as_str().unwrap_or("-");
        let size = b["size_bytes"].as_u64().unwrap_or(0);
        let size_mb = size as f64 / 1_048_576.0;
        let created = b["created_at"].as_str().unwrap_or("-");
        println!("{:<50} {:<12} {:<20}", filename, format!("{size_mb:.1} MB"), created);
    }

    println!("\n{} backup(s)", backups.len());
    Ok(())
}

pub async fn cmd_backup_verify(token: &str, backup_type: &str, name: &str, filename: &str) -> Result<(), String> {
    println!("Verifying {backup_type} backup: {filename}...");

    let body = match backup_type {
        "site" => serde_json::json!({ "domain": name, "filename": filename }),
        "database" => serde_json::json!({ "db_type": "postgres", "db_name": name, "filename": filename }),
        "volume" => serde_json::json!({ "container_name": name, "filename": filename }),
        _ => return Err(format!("Invalid backup type: {backup_type}. Use site, database, or volume.")),
    };

    let result = client::agent_post(&format!("/backups/verify/{backup_type}"), &body, token).await?;

    let passed = result["passed"].as_bool().unwrap_or(false);
    let checks_run = result["checks_run"].as_i64().unwrap_or(0);
    let checks_passed = result["checks_passed"].as_i64().unwrap_or(0);
    let duration = result["duration_ms"].as_u64().unwrap_or(0);

    if passed {
        println!("\x1b[32m✓\x1b[0m Verification PASSED ({checks_passed}/{checks_run} checks, {duration}ms)");
    } else {
        println!("\x1b[31m✗\x1b[0m Verification FAILED ({checks_passed}/{checks_run} checks, {duration}ms)");
    }

    if let Some(details) = result["details"].as_array() {
        for check in details {
            let name = check["name"].as_str().unwrap_or("-");
            let ok = check["passed"].as_bool().unwrap_or(false);
            let msg = check["message"].as_str().unwrap_or("-");
            let icon = if ok { "\x1b[32m✓\x1b[0m" } else { "\x1b[31m✗\x1b[0m" };
            println!("  {icon} {name}: {msg}");
        }
    }

    Ok(())
}

pub async fn cmd_backup_health(token: &str) -> Result<(), String> {
    // This calls the API, not the agent. For CLI simplicity, show local backup counts.
    let site_dirs = std::fs::read_dir("/var/backups/dockpanel")
        .map(|d| d.filter_map(|e| e.ok()).filter(|e| e.path().is_dir()).count())
        .unwrap_or(0);
    let db_dirs = std::fs::read_dir("/var/backups/dockpanel/databases")
        .map(|d| d.filter_map(|e| e.ok()).filter(|e| e.path().is_dir()).count())
        .unwrap_or(0);
    let vol_dirs = std::fs::read_dir("/var/backups/dockpanel/volumes")
        .map(|d| d.filter_map(|e| e.ok()).filter(|e| e.path().is_dir()).count())
        .unwrap_or(0);

    // Count total files
    let count_files = |dir: &str| -> (usize, u64) {
        let mut count = 0usize;
        let mut size = 0u64;
        if let Ok(entries) = std::fs::read_dir(dir) {
            for entry in entries.flatten() {
                if entry.path().is_dir() {
                    if let Ok(sub) = std::fs::read_dir(entry.path()) {
                        for f in sub.flatten() {
                            if let Ok(m) = f.metadata() {
                                if m.is_file() {
                                    count += 1;
                                    size += m.len();
                                }
                            }
                        }
                    }
                }
            }
        }
        (count, size)
    };

    let (site_count, site_size) = count_files("/var/backups/dockpanel");
    let (db_count, db_size) = count_files("/var/backups/dockpanel/databases");
    let (vol_count, vol_size) = count_files("/var/backups/dockpanel/volumes");

    let total_size = site_size + db_size + vol_size;
    let total_count = site_count + db_count + vol_count;

    println!("\x1b[1mBackup Health Overview\x1b[0m\n");
    println!("  Site backups:     {site_count:>4} files across {site_dirs} domains ({:.1} MB)", site_size as f64 / 1_048_576.0);
    println!("  Database backups: {db_count:>4} files across {db_dirs} databases ({:.1} MB)", db_size as f64 / 1_048_576.0);
    println!("  Volume backups:   {vol_count:>4} files across {vol_dirs} containers ({:.1} MB)", vol_size as f64 / 1_048_576.0);
    println!("  ─────────────────────────────────────");
    println!("  Total:            {total_count:>4} files ({:.1} MB)", total_size as f64 / 1_048_576.0);

    // Suppress unused variable warning for token
    let _ = token;

    Ok(())
}
