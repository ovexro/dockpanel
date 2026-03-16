use crate::client;
use serde_json::json;

pub async fn cmd_php_list(token: &str, output: &str) -> Result<(), String> {
    let result = client::agent_get("/php/versions", token).await?;

    if output == "json" {
        println!("{}", serde_json::to_string_pretty(&result).unwrap_or_default());
        return Ok(());
    }

    let versions = result["versions"]
        .as_array()
        .ok_or("Expected versions array from /php/versions")?;

    println!(
        "\x1b[1m{:<10} {:<12} {:<12} {:<30}\x1b[0m",
        "VERSION", "INSTALLED", "FPM", "SOCKET"
    );

    for v in versions {
        let version = v["version"].as_str().unwrap_or("-");
        let installed = v["installed"].as_bool().unwrap_or(false);
        let fpm = v["fpm_running"].as_bool().unwrap_or(false);
        let socket = v["socket"].as_str().unwrap_or("-");

        let inst_color = if installed { "\x1b[32m" } else { "\x1b[90m" };
        let fpm_color = if fpm { "\x1b[32m" } else { "\x1b[90m" };

        println!(
            "{:<10} {inst_color}{:<12}\x1b[0m {fpm_color}{:<12}\x1b[0m {:<30}",
            version,
            if installed { "yes" } else { "no" },
            if fpm { "running" } else { "stopped" },
            if installed { socket } else { "-" }
        );
    }

    Ok(())
}

pub async fn cmd_php_install(token: &str, version: &str) -> Result<(), String> {
    match version {
        "8.1" | "8.2" | "8.3" | "8.4" => {}
        _ => return Err(format!("Invalid PHP version '{version}'. Supported: 8.1, 8.2, 8.3, 8.4")),
    }

    println!("Installing PHP {version}...");

    let body = json!({ "version": version });
    let result = client::agent_post("/php/install", &body, token).await?;

    if result["success"].as_bool() == Some(true) {
        let msg = result["message"].as_str().unwrap_or("Installed successfully");
        println!("\x1b[32m✓\x1b[0m {msg}");
    } else {
        let msg = result["message"].as_str().unwrap_or("Unknown error");
        return Err(format!("Failed to install PHP {version}: {msg}"));
    }

    Ok(())
}
