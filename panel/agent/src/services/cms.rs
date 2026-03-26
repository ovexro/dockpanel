use std::process::Stdio;
use crate::safe_cmd::safe_command;

const COMPOSER: &str = "/usr/local/bin/composer";
const SITE_ROOT: &str = "/var/www";

/// Run a shell command, return stdout on success or stderr on failure.
async fn run_cmd(program: &str, args: &[&str]) -> Result<String, String> {
    let out = safe_command(program)
        .args(args)
        .env("HOME", "/root")
        .env("COMPOSER_HOME", "/root/.composer")
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .output()
        .await
        .map_err(|e| format!("Failed to execute {program}: {e}"))?;

    if !out.status.success() {
        let stderr = String::from_utf8_lossy(&out.stderr).trim().to_string();
        let stdout = String::from_utf8_lossy(&out.stdout).trim().to_string();
        // Some tools (e.g. Joomla CLI) output errors to stdout
        return Err(if stderr.is_empty() { stdout } else { stderr });
    }
    Ok(String::from_utf8_lossy(&out.stdout).trim().to_string())
}

/// Run a shell command in a specific working directory.
async fn run_cmd_in(dir: &str, program: &str, args: &[&str]) -> Result<String, String> {
    let out = safe_command(program)
        .args(args)
        .current_dir(dir)
        .env("HOME", "/root")
        .env("COMPOSER_HOME", "/root/.composer")
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .output()
        .await
        .map_err(|e| format!("Failed to execute {program}: {e}"))?;

    if !out.status.success() {
        let stderr = String::from_utf8_lossy(&out.stderr).trim().to_string();
        let stdout = String::from_utf8_lossy(&out.stdout).trim().to_string();
        return Err(if stderr.is_empty() { stdout } else { stderr });
    }
    Ok(String::from_utf8_lossy(&out.stdout).trim().to_string())
}

/// Split a `host:port` string into (host, port). Defaults to port 3306.
fn split_host_port(db_host: &str) -> (&str, &str) {
    match db_host.rsplit_once(':') {
        Some((h, p)) => (h, p),
        None => (db_host, "3306"),
    }
}

fn validate_domain(domain: &str) -> Result<(), String> {
    if domain.is_empty() || domain.contains("..") || domain.contains('/')
        || domain.contains('\\') || domain.contains('\0')
        || !domain.chars().all(|c| c.is_ascii_alphanumeric() || c == '.' || c == '-') {
        return Err("Invalid domain format".to_string());
    }
    Ok(())
}

/// Fix ownership to www-data for a site directory.
async fn chown_site(path: &str) {
    safe_command("chown")
        .args(["-R", "www-data:www-data", path])
        .output()
        .await
        .ok();
}

/// Ensure Composer is installed at /usr/local/bin/composer.
pub async fn ensure_composer() -> Result<(), String> {
    if std::path::Path::new(COMPOSER).exists() {
        return Ok(());
    }
    let out = safe_command("curl")
        .args([
            "-sS",
            "-L",
            "-o",
            COMPOSER,
            "https://getcomposer.org/download/latest-stable/composer.phar",
        ])
        .output()
        .await
        .map_err(|e| format!("Download failed: {e}"))?;
    if !out.status.success() {
        return Err("Failed to download Composer".into());
    }
    safe_command("chmod")
        .args(["+x", COMPOSER])
        .output()
        .await
        .ok();
    Ok(())
}

/// Install Laravel into /var/www/{domain}/.
pub async fn install_laravel(
    domain: &str,
    db_name: &str,
    db_user: &str,
    db_pass: &str,
    db_host: &str,
    title: &str,
) -> Result<String, String> {
    validate_domain(domain)?;
    ensure_composer().await?;

    let site_dir = format!("{SITE_ROOT}/{domain}");

    // Create project
    run_cmd(
        COMPOSER,
        &[
            "create-project",
            "laravel/laravel",
            &format!("{site_dir}/"),
            "--no-interaction",
            "--prefer-dist",
        ],
    )
    .await?;

    // Copy .env.example -> .env
    let env_example = format!("{site_dir}/.env.example");
    let env_file = format!("{site_dir}/.env");
    tokio::fs::copy(&env_example, &env_file)
        .await
        .map_err(|e| format!("Failed to copy .env.example: {e}"))?;

    // Read and update .env
    let env_content = tokio::fs::read_to_string(&env_file)
        .await
        .map_err(|e| format!("Failed to read .env: {e}"))?;

    let (host, port) = split_host_port(db_host);

    let env_content = replace_env_line(&env_content, "APP_NAME", title);
    let env_content = replace_env_line(&env_content, "APP_URL", &format!("https://{domain}"));
    let env_content = replace_env_line(&env_content, "DB_HOST", host);
    let env_content = replace_env_line(&env_content, "DB_PORT", port);
    let env_content = replace_env_line(&env_content, "DB_DATABASE", db_name);
    let env_content = replace_env_line(&env_content, "DB_USERNAME", db_user);
    let env_content = replace_env_line(&env_content, "DB_PASSWORD", db_pass);

    tokio::fs::write(&env_file, env_content)
        .await
        .map_err(|e| format!("Failed to write .env: {e}"))?;

    // Generate application key
    run_cmd_in(&site_dir, "php", &["artisan", "key:generate", "--force"]).await?;

    // Run migrations (allow failure — DB might not be ready)
    let _ = run_cmd_in(&site_dir, "php", &["artisan", "migrate", "--force"]).await;

    // Create public symlink for nginx (Laravel's web root is public/ by default)
    // No symlink needed — Laravel already uses public/ as document root.

    chown_site(&site_dir).await;

    Ok("Laravel installed successfully".into())
}

/// Install Drupal into /var/www/{domain}/.
pub async fn install_drupal(
    domain: &str,
    db_name: &str,
    db_user: &str,
    db_pass: &str,
    db_host: &str,
    title: &str,
    admin_user: &str,
    admin_pass: &str,
    admin_email: &str,
) -> Result<String, String> {
    validate_domain(domain)?;
    ensure_composer().await?;

    let site_dir = format!("{SITE_ROOT}/{domain}");

    // Create project
    run_cmd(
        COMPOSER,
        &[
            "create-project",
            "drupal/recommended-project",
            &format!("{site_dir}/"),
            "--no-interaction",
            "--prefer-dist",
        ],
    )
    .await?;

    // Install Drush
    run_cmd(
        COMPOSER,
        &[
            "require",
            "drush/drush",
            &format!("--working-dir={site_dir}"),
            "--no-interaction",
        ],
    )
    .await?;

    // Create public symlink: nginx expects public/, Drupal uses web/
    let symlink_target = format!("{site_dir}/public");
    if !std::path::Path::new(&symlink_target).exists() {
        tokio::fs::symlink("web", &symlink_target)
            .await
            .map_err(|e| format!("Failed to create public symlink: {e}"))?;
    }

    // Run drush site install
    let db_url = format!("mysql://{db_user}:{db_pass}@{db_host}/{db_name}");
    run_cmd_in(
        &site_dir,
        "vendor/bin/drush",
        &[
            "site:install",
            "standard",
            &format!("--db-url={db_url}"),
            &format!("--account-name={admin_user}"),
            &format!("--account-pass={admin_pass}"),
            &format!("--account-mail={admin_email}"),
            &format!("--site-name={title}"),
            "-y",
        ],
    )
    .await?;

    chown_site(&site_dir).await;

    Ok("Drupal installed successfully".into())
}

/// Install Joomla into /var/www/{domain}/public/.
pub async fn install_joomla(
    domain: &str,
    db_name: &str,
    db_user: &str,
    db_pass: &str,
    db_host: &str,
    title: &str,
    admin_user: &str,
    admin_pass: &str,
    admin_email: &str,
) -> Result<String, String> {
    validate_domain(domain)?;
    let public_dir = format!("{SITE_ROOT}/{domain}/public");

    // Create document root
    tokio::fs::create_dir_all(&public_dir)
        .await
        .map_err(|e| format!("Failed to create directory: {e}"))?;

    // Get latest Joomla version tag from GitHub redirect
    let redirect_output = run_cmd(
        "curl",
        &["-sI", "https://github.com/joomla/joomla-cms/releases/latest"],
    )
    .await?;

    let tag = redirect_output
        .lines()
        .find(|line| line.to_lowercase().starts_with("location:"))
        .and_then(|line| line.trim().rsplit('/').next())
        .map(|t| t.trim().to_string())
        .ok_or_else(|| "Failed to determine latest Joomla version".to_string())?;

    // Download Joomla zip with random suffix to prevent symlink attacks
    let random_suffix: u64 = rand::random();
    let zip_path = format!("/tmp/joomla-{domain}-{random_suffix:016x}.zip");
    let download_url = format!(
        "https://github.com/joomla/joomla-cms/releases/download/{tag}/Joomla_{tag}-Stable-Full_Package.zip"
    );
    run_cmd("curl", &["-sL", "-o", &zip_path, &download_url]).await?;

    // Extract
    run_cmd("unzip", &["-o", &zip_path, "-d", &public_dir]).await?;

    // Clean up zip
    tokio::fs::remove_file(&zip_path).await.ok();

    // CLI install
    let install_php = format!("{public_dir}/installation/joomla.php");
    run_cmd(
        "php",
        &[
            &install_php,
            "install",
            &format!("--site-name={title}"),
            &format!("--admin-user={admin_user}"),
            &format!("--admin-username={admin_user}"),
            &format!("--admin-password={admin_pass}"),
            &format!("--admin-email={admin_email}"),
            "--db-type=mysqli",
            &format!("--db-host={db_host}"),
            &format!("--db-user={db_user}"),
            &format!("--db-pass={db_pass}"),
            &format!("--db-name={db_name}"),
            "--db-prefix=j_",
            "--db-encryption=0",
        ],
    )
    .await?;

    chown_site(&public_dir).await;

    Ok("Joomla installed successfully".into())
}

/// Install Symfony skeleton into /var/www/{domain}/.
pub async fn install_symfony(domain: &str, title: &str) -> Result<String, String> {
    validate_domain(domain)?;
    ensure_composer().await?;

    let site_dir = format!("{SITE_ROOT}/{domain}");

    // Create project
    run_cmd(
        COMPOSER,
        &[
            "create-project",
            "symfony/skeleton",
            &format!("{site_dir}/"),
            "--no-interaction",
            "--prefer-dist",
        ],
    )
    .await?;

    // Symfony's web root is public/ by default — no symlink needed.

    // Set APP_NAME in .env if it exists
    let env_file = format!("{site_dir}/.env");
    if std::path::Path::new(&env_file).exists() {
        let env_content = tokio::fs::read_to_string(&env_file)
            .await
            .map_err(|e| format!("Failed to read .env: {e}"))?;
        let env_content = replace_env_line(&env_content, "APP_ENV", "prod");
        tokio::fs::write(&env_file, env_content).await.ok();
    }

    let _ = title; // title noted for future use; Symfony skeleton has no site-name concept

    chown_site(&site_dir).await;

    Ok("Symfony installed successfully".into())
}

/// Install CodeIgniter 4 into /var/www/{domain}/.
pub async fn install_codeigniter(
    domain: &str,
    db_name: &str,
    db_user: &str,
    db_pass: &str,
    db_host: &str,
    title: &str,
) -> Result<String, String> {
    validate_domain(domain)?;
    ensure_composer().await?;

    let site_dir = format!("{SITE_ROOT}/{domain}");

    // Create project
    run_cmd(
        COMPOSER,
        &[
            "create-project",
            "codeigniter4/appstarter",
            &format!("{site_dir}/"),
            "--no-interaction",
            "--prefer-dist",
        ],
    )
    .await?;

    // Copy env template -> .env
    let env_template = format!("{site_dir}/env");
    let env_file = format!("{site_dir}/.env");
    tokio::fs::copy(&env_template, &env_file)
        .await
        .map_err(|e| format!("Failed to copy env template: {e}"))?;

    // Read and update .env
    let env_content = tokio::fs::read_to_string(&env_file)
        .await
        .map_err(|e| format!("Failed to read .env: {e}"))?;

    let (host, port) = split_host_port(db_host);

    // CodeIgniter .env uses comments by default; uncomment and set values
    let env_content = set_ci_env(&env_content, "CI_ENVIRONMENT", "production");
    let env_content = set_ci_env(&env_content, "database.default.hostname", host);
    let env_content = set_ci_env(&env_content, "database.default.database", db_name);
    let env_content = set_ci_env(&env_content, "database.default.username", db_user);
    let env_content = set_ci_env(&env_content, "database.default.password", db_pass);
    let env_content = set_ci_env(&env_content, "database.default.DBDriver", "MySQLi");
    let env_content = set_ci_env(&env_content, "database.default.port", port);
    let env_content = set_ci_env(&env_content, "app.baseURL", &format!("'https://{domain}'"));

    let _ = title; // CI4 has no site-title in env

    tokio::fs::write(&env_file, env_content)
        .await
        .map_err(|e| format!("Failed to write .env: {e}"))?;

    chown_site(&site_dir).await;

    Ok("CodeIgniter installed successfully".into())
}

/// Replace or add a KEY=value line in a .env file (Laravel-style: KEY=value).
/// Quotes values that contain spaces.
fn replace_env_line(content: &str, key: &str, value: &str) -> String {
    let prefix = format!("{key}=");
    let quoted = if value.contains(' ') {
        format!("\"{value}\"")
    } else {
        value.to_string()
    };
    let mut found = false;
    let mut lines: Vec<String> = content
        .lines()
        .map(|line| {
            if line.starts_with(&prefix) || line.starts_with(&format!("# {prefix}")) {
                found = true;
                format!("{key}={quoted}")
            } else {
                line.to_string()
            }
        })
        .collect();

    if !found {
        lines.push(format!("{key}={quoted}"));
    }
    let mut result = lines.join("\n");
    if content.ends_with('\n') {
        result.push('\n');
    }
    result
}

/// Set a CodeIgniter .env value, uncommenting if necessary.
/// CI4 .env lines look like `# database.default.hostname = localhost` (commented) or
/// `database.default.hostname = localhost` (active).
fn set_ci_env(content: &str, key: &str, value: &str) -> String {
    let mut found = false;
    let mut lines: Vec<String> = content
        .lines()
        .map(|line| {
            let trimmed = line.trim();
            // Match both commented and uncommented forms
            if trimmed.starts_with(&format!("# {key}"))
                || trimmed.starts_with(&format!("#{key}"))
                || trimmed.starts_with(key)
            {
                // Only match if the key is followed by a space+= or just =
                let after_key = if trimmed.starts_with('#') {
                    trimmed.trim_start_matches('#').trim()
                } else {
                    trimmed
                };
                if after_key.starts_with(key)
                    && after_key[key.len()..]
                        .trim_start()
                        .starts_with('=')
                {
                    found = true;
                    return format!("{key} = {value}");
                }
            }
            line.to_string()
        })
        .collect();

    if !found {
        lines.push(format!("{key} = {value}"));
    }
    let mut result = lines.join("\n");
    if content.ends_with('\n') {
        result.push('\n');
    }
    result
}
