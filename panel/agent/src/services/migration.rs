use serde::Serialize;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use tokio::process::Command;

const MIGRATION_DIR: &str = "/tmp/dockpanel-migration";

#[derive(Serialize, Clone)]
pub struct MigrationInventory {
    pub id: String,
    pub source: String,
    pub sites: Vec<MigrationSite>,
    pub databases: Vec<MigrationDatabase>,
    pub mail_accounts: Vec<MigrationMailAccount>,
    pub warnings: Vec<String>,
}

#[derive(Serialize, Clone)]
pub struct MigrationSite {
    pub domain: String,
    pub doc_root: String,
    pub size_bytes: u64,
    pub runtime: String,
    pub file_count: u64,
}

#[derive(Serialize, Clone)]
pub struct MigrationDatabase {
    pub name: String,
    pub file: String,
    pub size_bytes: u64,
    pub engine: String,
}

#[derive(Serialize, Clone)]
pub struct MigrationMailAccount {
    pub email: String,
    pub domain: String,
}

/// Extract and analyze a backup file.
pub async fn analyze(backup_path: &str, source: &str) -> Result<MigrationInventory, String> {
    let id = uuid::Uuid::new_v4().to_string();
    let extract_dir = format!("{MIGRATION_DIR}-{id}");

    // Create extraction directory
    std::fs::create_dir_all(&extract_dir).map_err(|e| format!("Failed to create dir: {e}"))?;

    // Extract the backup
    tracing::info!("Extracting backup {backup_path} to {extract_dir}");
    let output = Command::new("tar")
        .args(["xzf", backup_path, "-C", &extract_dir])
        .output()
        .await
        .map_err(|e| format!("Failed to extract: {e}"))?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        // Try without gzip (plain tar)
        let output2 = Command::new("tar")
            .args(["xf", backup_path, "-C", &extract_dir])
            .output()
            .await
            .map_err(|e| format!("Extraction failed: {e}"))?;
        if !output2.status.success() {
            return Err(format!("Failed to extract backup: {stderr}"));
        }
    }

    // Find the actual root directory (backups often have a subdirectory)
    let root = find_backup_root(&extract_dir).await;

    let inventory = match source {
        "cpanel" => parse_cpanel(&id, &root).await?,
        "plesk" => parse_plesk(&id, &root).await?,
        "hestiacp" => parse_hestiacp(&id, &root).await?,
        _ => return Err(format!("Unknown source: {source}")),
    };

    Ok(inventory)
}

/// Find the actual root of the extracted backup (skip single top-level dir)
async fn find_backup_root(extract_dir: &str) -> String {
    let entries: Vec<_> = std::fs::read_dir(extract_dir)
        .ok()
        .map(|rd| rd.filter_map(|e| e.ok()).collect())
        .unwrap_or_default();

    // If there's exactly one subdirectory, use it as root
    if entries.len() == 1 {
        let entry = &entries[0];
        if entry.file_type().map(|ft| ft.is_dir()).unwrap_or(false) {
            return entry.path().to_string_lossy().to_string();
        }
    }
    extract_dir.to_string()
}

/// Parse a cPanel backup structure
async fn parse_cpanel(id: &str, root: &str) -> Result<MigrationInventory, String> {
    let root = Path::new(root);
    let mut sites = Vec::new();
    let mut databases = Vec::new();
    let mut mail_accounts = Vec::new();
    let mut warnings = Vec::new();

    // 1. Find sites from homedir/
    let homedir = root.join("homedir");
    if homedir.exists() {
        // Main domain: public_html/
        let public_html = homedir.join("public_html");
        if public_html.exists() {
            let (size, count) = dir_stats(&public_html).await;
            let runtime = detect_runtime(&public_html);
            // Try to get the main domain from cp/ config
            let domain = find_cpanel_main_domain(root).unwrap_or_else(|| "main-domain.com".to_string());
            sites.push(MigrationSite {
                domain,
                doc_root: "homedir/public_html".to_string(),
                size_bytes: size,
                runtime,
                file_count: count,
            });
        }

        // Additional domains from addon_domains or subdomains in homedir/
        for entry in std::fs::read_dir(&homedir).ok().into_iter().flatten().flatten() {
            let name = entry.file_name().to_string_lossy().to_string();
            if name == "public_html" || name.starts_with('.') || name == "mail" || name == "etc" || name == "tmp" || name == "logs" || name == "ssl" {
                continue;
            }
            if entry.file_type().map(|ft| ft.is_dir()).unwrap_or(false) {
                let dir_path = entry.path();
                let (size, count) = dir_stats(&dir_path).await;
                if count > 0 {
                    let runtime = detect_runtime(&dir_path);
                    sites.push(MigrationSite {
                        domain: name.clone(),
                        doc_root: format!("homedir/{name}"),
                        size_bytes: size,
                        runtime,
                        file_count: count,
                    });
                }
            }
        }
    } else {
        warnings.push("No homedir/ found in backup".to_string());
    }

    // 2. Find databases from mysql/
    let mysql_dir = root.join("mysql");
    if mysql_dir.exists() {
        for entry in std::fs::read_dir(&mysql_dir).ok().into_iter().flatten().flatten() {
            let name = entry.file_name().to_string_lossy().to_string();
            if name.ends_with(".sql") || name.ends_with(".sql.gz") {
                let db_name = name.trim_end_matches(".gz").trim_end_matches(".sql").to_string();
                let size = entry.metadata().map(|m| m.len()).unwrap_or(0);
                databases.push(MigrationDatabase {
                    name: db_name,
                    file: format!("mysql/{name}"),
                    size_bytes: size,
                    engine: "mysql".to_string(),
                });
            }
        }
    }

    // Also check for mysql.sql (grants/users dump — skip it, just note)
    if root.join("mysql.sql").exists() {
        warnings.push("mysql.sql (user grants) found but not imported — create DB users manually".to_string());
    }

    // 3. Find mail accounts from etc/
    let etc_dir = root.join("etc");
    if etc_dir.exists() {
        // cPanel stores email accounts in etc/{domain}/shadow or etc/{domain}/passwd
        for domain_entry in std::fs::read_dir(&etc_dir).ok().into_iter().flatten().flatten() {
            if domain_entry.file_type().map(|ft| ft.is_dir()).unwrap_or(false) {
                let domain = domain_entry.file_name().to_string_lossy().to_string();
                let shadow = domain_entry.path().join("shadow");
                let passwd = domain_entry.path().join("passwd");
                let accounts_file = if shadow.exists() { shadow } else if passwd.exists() { passwd } else { continue };

                if let Ok(content) = std::fs::read_to_string(&accounts_file) {
                    for line in content.lines() {
                        let user = line.split(':').next().unwrap_or("").trim();
                        if !user.is_empty() && user != "root" {
                            mail_accounts.push(MigrationMailAccount {
                                email: format!("{user}@{domain}"),
                                domain: domain.clone(),
                            });
                        }
                    }
                }
            }
        }
    }

    if sites.is_empty() && databases.is_empty() {
        warnings.push("No sites or databases found — backup may be empty or in an unexpected format".to_string());
    }

    Ok(MigrationInventory {
        id: id.to_string(),
        source: "cpanel".to_string(),
        sites,
        databases,
        mail_accounts,
        warnings,
    })
}

/// Parse Plesk backup (stub)
async fn parse_plesk(id: &str, root: &str) -> Result<MigrationInventory, String> {
    let root = Path::new(root);
    let mut sites = Vec::new();
    let mut databases = Vec::new();
    let mut warnings = vec!["Plesk import is in beta — some items may not be detected".to_string()];

    // Plesk: look for domains/ directory
    let domains_dir = root.join("domains");
    if domains_dir.exists() {
        for entry in std::fs::read_dir(&domains_dir).ok().into_iter().flatten().flatten() {
            if entry.file_type().map(|ft| ft.is_dir()).unwrap_or(false) {
                let domain = entry.file_name().to_string_lossy().to_string();
                let httpdocs = entry.path().join("httpdocs");
                let doc_root = if httpdocs.exists() { httpdocs } else { entry.path() };
                let (size, count) = dir_stats(&doc_root).await;
                let runtime = detect_runtime(&doc_root);
                sites.push(MigrationSite {
                    domain,
                    doc_root: doc_root.to_string_lossy().to_string(),
                    size_bytes: size,
                    runtime,
                    file_count: count,
                });
            }
        }
    }

    // Plesk: look for databases/ directory
    let db_dir = root.join("databases");
    if db_dir.exists() {
        for entry in std::fs::read_dir(&db_dir).ok().into_iter().flatten().flatten() {
            let name = entry.file_name().to_string_lossy().to_string();
            if name.ends_with(".sql") || name.ends_with(".sql.gz") {
                let db_name = name.trim_end_matches(".gz").trim_end_matches(".sql").to_string();
                let size = entry.metadata().map(|m| m.len()).unwrap_or(0);
                databases.push(MigrationDatabase { name: db_name, file: name, size_bytes: size, engine: "mysql".to_string() });
            }
        }
    }

    if sites.is_empty() && databases.is_empty() {
        warnings.push("No data found — check that this is a valid Plesk backup".to_string());
    }

    Ok(MigrationInventory { id: id.to_string(), source: "plesk".to_string(), sites, databases, mail_accounts: vec![], warnings })
}

/// Parse HestiaCP backup (stub)
async fn parse_hestiacp(id: &str, root: &str) -> Result<MigrationInventory, String> {
    let root = Path::new(root);
    let mut sites = Vec::new();
    let mut databases = Vec::new();
    let mut warnings = vec!["HestiaCP import is in beta — some items may not be detected".to_string()];

    // HestiaCP: web/ directory
    let web_dir = root.join("web");
    if web_dir.exists() {
        for entry in std::fs::read_dir(&web_dir).ok().into_iter().flatten().flatten() {
            if entry.file_type().map(|ft| ft.is_dir()).unwrap_or(false) {
                let domain = entry.file_name().to_string_lossy().to_string();
                let public = entry.path().join("public_html");
                let doc_root = if public.exists() { public } else { entry.path() };
                let (size, count) = dir_stats(&doc_root).await;
                let runtime = detect_runtime(&doc_root);
                sites.push(MigrationSite { domain, doc_root: doc_root.to_string_lossy().to_string(), size_bytes: size, runtime, file_count: count });
            }
        }
    }

    // HestiaCP: db/ directory
    let db_dir = root.join("db");
    if db_dir.exists() {
        for entry in std::fs::read_dir(&db_dir).ok().into_iter().flatten().flatten() {
            let name = entry.file_name().to_string_lossy().to_string();
            if name.ends_with(".sql") || name.ends_with(".sql.gz") {
                let db_name = name.trim_end_matches(".gz").trim_end_matches(".sql").to_string();
                let size = entry.metadata().map(|m| m.len()).unwrap_or(0);
                databases.push(MigrationDatabase { name: db_name, file: name, size_bytes: size, engine: "mysql".to_string() });
            }
        }
    }

    if sites.is_empty() && databases.is_empty() {
        warnings.push("No data found — check that this is a valid HestiaCP backup".to_string());
    }

    Ok(MigrationInventory { id: id.to_string(), source: "hestiacp".to_string(), sites, databases, mail_accounts: vec![], warnings })
}

/// Try to find the main domain from cPanel config files
fn find_cpanel_main_domain(root: &Path) -> Option<String> {
    // Check cp/ directory for user config
    let cp_dir = root.join("cp");
    if cp_dir.exists() {
        for entry in std::fs::read_dir(&cp_dir).ok()?.flatten() {
            if let Ok(content) = std::fs::read_to_string(entry.path()) {
                for line in content.lines() {
                    if line.starts_with("DNS=") || line.starts_with("DOMAIN=") {
                        let domain = line.split('=').nth(1)?.trim().to_string();
                        if !domain.is_empty() { return Some(domain); }
                    }
                }
            }
        }
    }
    // Check userdata/main
    let userdata = root.join("userdata").join("main");
    if userdata.exists() {
        if let Ok(content) = std::fs::read_to_string(&userdata) {
            for line in content.lines() {
                if line.starts_with("main_domain:") {
                    return Some(line.split(':').nth(1)?.trim().to_string());
                }
            }
        }
    }
    None
}

/// Detect runtime from file contents (PHP, static, node, python)
fn detect_runtime(dir: &Path) -> String {
    if dir.join("wp-config.php").exists() || dir.join("index.php").exists() || dir.join("artisan").exists() {
        return "php".to_string();
    }
    if dir.join("package.json").exists() { return "node".to_string(); }
    if dir.join("requirements.txt").exists() || dir.join("manage.py").exists() { return "python".to_string(); }
    "static".to_string()
}

/// Get total size and file count of a directory
async fn dir_stats(dir: &Path) -> (u64, u64) {
    let output = Command::new("du")
        .args(["-sb", &dir.to_string_lossy()])
        .output()
        .await
        .ok();
    let size = output.as_ref()
        .and_then(|o| String::from_utf8_lossy(&o.stdout).split_whitespace().next().map(|s| s.parse::<u64>().unwrap_or(0)))
        .unwrap_or(0);

    let count_output = Command::new("find")
        .args([&dir.to_string_lossy().to_string(), "-type", "f"])
        .output()
        .await
        .ok();
    let count = count_output.as_ref()
        .map(|o| String::from_utf8_lossy(&o.stdout).lines().count() as u64)
        .unwrap_or(0);

    (size, count)
}

/// Import site files from extracted backup to /var/www/{domain}/
pub async fn import_site_files(migration_id: &str, domain: &str, source_dir: &str) -> Result<String, String> {
    let extract_root = format!("{MIGRATION_DIR}-{migration_id}");
    let source = format!("{extract_root}/{source_dir}");
    let dest = format!("/var/www/{domain}");

    // Ensure source exists
    if !Path::new(&source).exists() {
        return Err(format!("Source directory not found: {source}"));
    }

    // Create destination
    std::fs::create_dir_all(&dest).map_err(|e| format!("Failed to create {dest}: {e}"))?;

    // Copy files with rsync
    let output = Command::new("rsync")
        .args(["-a", "--delete", &format!("{source}/"), &format!("{dest}/")])
        .output()
        .await
        .map_err(|e| format!("rsync failed: {e}"))?;

    if !output.status.success() {
        // Fall back to cp -a
        let output2 = Command::new("cp")
            .args(["-a", &format!("{source}/."), &dest])
            .output()
            .await
            .map_err(|e| format!("cp failed: {e}"))?;
        if !output2.status.success() {
            return Err(format!("File copy failed: {}", String::from_utf8_lossy(&output2.stderr)));
        }
    }

    // Fix ownership
    let _ = Command::new("chown")
        .args(["-R", "www-data:www-data", &dest])
        .output()
        .await;

    Ok(format!("Copied files to {dest}"))
}

/// Import a SQL dump into a database container
pub async fn import_database(migration_id: &str, sql_file: &str, container_name: &str, db_name: &str, engine: &str, user: &str, password: &str) -> Result<String, String> {
    let extract_root = format!("{MIGRATION_DIR}-{migration_id}");
    let sql_path = format!("{extract_root}/{sql_file}");

    if !Path::new(&sql_path).exists() {
        return Err(format!("SQL file not found: {sql_path}"));
    }

    // Decompress if .gz
    let actual_path = if sql_path.ends_with(".gz") {
        let decompressed = sql_path.trim_end_matches(".gz").to_string();
        let output = Command::new("gunzip")
            .args(["-k", &sql_path])
            .output()
            .await
            .map_err(|e| format!("gunzip failed: {e}"))?;
        if !output.status.success() {
            return Err(format!("Failed to decompress: {}", String::from_utf8_lossy(&output.stderr)));
        }
        decompressed
    } else {
        sql_path.clone()
    };

    // Import into container
    let cmd = if engine == "mysql" || engine == "mariadb" {
        format!("docker exec -i {container_name} mariadb -u {user} -p{password} {db_name} < {actual_path}")
    } else {
        format!("docker exec -i {container_name} psql -U {user} -d {db_name} < {actual_path}")
    };

    let output = tokio::time::timeout(
        std::time::Duration::from_secs(600),
        Command::new("sh").arg("-c").arg(&cmd).output(),
    )
    .await
    .map_err(|_| "Database import timed out (600s)".to_string())?
    .map_err(|e| format!("Import command failed: {e}"))?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        // Some MySQL warnings are OK — check for actual errors
        if stderr.contains("ERROR") {
            return Err(format!("Database import error: {stderr}"));
        }
    }

    Ok(format!("Imported {db_name} from {sql_file}"))
}

/// Clean up a migration's temp directory
pub async fn cleanup(migration_id: &str) -> Result<(), String> {
    let dir = format!("{MIGRATION_DIR}-{migration_id}");
    if Path::new(&dir).exists() {
        std::fs::remove_dir_all(&dir).map_err(|e| format!("Cleanup failed: {e}"))?;
        tracing::info!("Cleaned up migration directory: {dir}");
    }
    Ok(())
}
