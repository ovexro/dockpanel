use crate::client;
use serde_json::json;

pub(crate) fn rand_byte() -> u8 {
    use std::io::Read;
    let mut buf = [0u8; 1];
    if let Ok(mut f) = std::fs::File::open("/dev/urandom") {
        f.read_exact(&mut buf).ok();
    }
    buf[0]
}

pub async fn cmd_db_list(token: &str, output: &str, filter: Option<&str>) -> Result<(), String> {
    let dbs = client::agent_get("/databases", token).await?;
    let dbs = dbs.as_array().ok_or("Expected array from /databases")?;

    // Apply filter
    let filtered: Vec<_> = if let Some(f) = filter {
        let f_lower = f.to_lowercase();
        dbs.iter()
            .filter(|db| {
                db["name"]
                    .as_str()
                    .unwrap_or("")
                    .to_lowercase()
                    .contains(&f_lower)
            })
            .collect()
    } else {
        dbs.iter().collect()
    };

    if output == "json" {
        let json_arr: Vec<_> = filtered.into_iter().cloned().collect();
        println!("{}", serde_json::to_string_pretty(&json_arr).unwrap_or_default());
        return Ok(());
    }

    if filtered.is_empty() {
        println!("No databases.");
        return Ok(());
    }

    println!(
        "\x1b[1m{:<20} {:<12} {:<8} {:<12}\x1b[0m",
        "NAME", "ENGINE", "PORT", "STATUS"
    );

    for db in &filtered {
        let name = db["name"].as_str().unwrap_or("-");
        let engine = db["engine"].as_str().unwrap_or("-");
        let port = db["port"].as_u64().unwrap_or(0);
        let status = db["status"].as_str().unwrap_or("-");

        let color = if status == "running" {
            "\x1b[32m"
        } else {
            "\x1b[31m"
        };

        println!(
            "{:<20} {:<12} {:<8} {color}{:<12}\x1b[0m",
            name, engine, port, status
        );
    }

    println!("\n{} database(s)", filtered.len());
    Ok(())
}

pub async fn cmd_db_create(
    token: &str,
    name: &str,
    engine: &str,
    password: &str,
    port: u16,
) -> Result<(), String> {
    match engine {
        "mysql" | "mariadb" | "postgres" => {}
        _ => return Err(format!("Invalid engine '{engine}'. Use: mysql, mariadb, or postgres")),
    }

    println!("Creating {engine} database '{name}' on port {port}...");
    let body = json!({
        "name": name,
        "engine": engine,
        "password": password,
        "port": port,
    });

    let result = client::agent_post("/databases", &body, token).await?;

    if result["success"].as_bool() == Some(true) {
        let cid = result["container_id"].as_str().unwrap_or("unknown");
        println!("\x1b[32m✓\x1b[0m Database created");
        println!("  Name:         {name}");
        println!("  Engine:       {engine}");
        println!("  Port:         {port}");
        println!("  Container:    {}", &cid[..cid.len().min(12)]);
    } else {
        let msg = result["message"].as_str().unwrap_or("Unknown error");
        return Err(format!("Failed to create database: {msg}"));
    }

    Ok(())
}

pub async fn cmd_db_delete(token: &str, container_id: &str) -> Result<(), String> {
    println!("Deleting database container {container_id}...");
    let result = client::agent_delete(&format!("/databases/{container_id}"), token).await?;

    if result["success"].as_bool() == Some(true) {
        println!("\x1b[32m✓\x1b[0m Database deleted");
    } else {
        let msg = result["message"].as_str().unwrap_or("Unknown error");
        return Err(format!("Failed to delete database: {msg}"));
    }

    Ok(())
}
