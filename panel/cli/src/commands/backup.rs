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
