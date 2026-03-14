use crate::client;
use super::iac::urlenc;

pub async fn cmd_logs(
    token: &str,
    domain: Option<&str>,
    log_type: &str,
    lines: u32,
    filter: Option<&str>,
    search: Option<&str>,
) -> Result<(), String> {
    // Search mode
    if let Some(pattern) = search {
        let query = format!("/logs/search?pattern={}&type={log_type}", urlenc(pattern));
        let result = client::agent_get(&query, token).await?;
        let entries = result.as_array().ok_or("Expected array from /logs/search")?;

        if entries.is_empty() {
            println!("No matches found.");
        } else {
            for entry in entries {
                if let Some(line) = entry.as_str() {
                    println!("{line}");
                }
            }
            println!("\n{} match(es)", entries.len());
        }
        return Ok(());
    }

    // Domain-specific or system logs
    let path = if let Some(domain) = domain {
        let mut p = format!("/logs/{domain}?type={log_type}&lines={lines}");
        if let Some(f) = filter {
            p.push_str(&format!("&filter={}", urlenc(f)));
        }
        p
    } else {
        let mut p = format!("/logs?type={log_type}&lines={lines}");
        if let Some(f) = filter {
            p.push_str(&format!("&filter={}", urlenc(f)));
        }
        p
    };

    let result = client::agent_get(&path, token).await?;
    let entries = result.as_array().ok_or("Expected array from /logs")?;

    if entries.is_empty() {
        println!("No log entries.");
    } else {
        for entry in entries {
            if let Some(line) = entry.as_str() {
                println!("{line}");
            }
        }
    }

    Ok(())
}
