use std::process::Stdio;
use std::time::Duration;
use sha2::{Digest, Sha256};
use crate::safe_cmd::safe_command;

const COMPOSER: &str = "/usr/local/bin/composer";
const SITE_ROOT: &str = "/var/www";

/// `composer create-project`/`composer require`: full dependency resolution
/// plus a network fetch of every package. The heaviest operation this file runs.
const COMPOSER_TIMEOUT: Duration = Duration::from_secs(300);
/// A CLI installer that does DB writes/migrations (`drush site:install`,
/// Joomla's `installation/joomla.php install`).
const CLI_INSTALL_TIMEOUT: Duration = Duration::from_secs(180);
/// A single network fetch of a package archive.
const DOWNLOAD_TIMEOUT: Duration = Duration::from_secs(120);
/// A quick local/PHP-CLI operation (`artisan key:generate`, a metadata `curl`).
const QUICK_TIMEOUT: Duration = Duration::from_secs(60);

/// Run a shell command, return stdout on success or stderr on failure.
///
/// `kill_on_drop` matters because of the `timeout` wrapper: without it, a
/// child that outlives `timeout` is not killed when the future is dropped —
/// it orphans, running to completion (or forever) unattended. Same class as
/// the `kill_on_drop`-missing bugs already fixed elsewhere in this project.
async fn run_cmd(program: &str, args: &[&str], timeout: Duration) -> Result<String, String> {
    let fut = safe_command(program)
        .args(args)
        .env("HOME", "/root")
        .env("COMPOSER_HOME", "/root/.composer")
        .kill_on_drop(true)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .output();
    let out = tokio::time::timeout(timeout, fut)
        .await
        .map_err(|_| format!("{program} timed out after {}s", timeout.as_secs()))?
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
async fn run_cmd_in(dir: &str, program: &str, args: &[&str], timeout: Duration) -> Result<String, String> {
    let fut = safe_command(program)
        .args(args)
        .current_dir(dir)
        .env("HOME", "/root")
        .env("COMPOSER_HOME", "/root/.composer")
        .kill_on_drop(true)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .output();
    let out = tokio::time::timeout(timeout, fut)
        .await
        .map_err(|_| format!("{program} timed out after {}s", timeout.as_secs()))?
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

/// Fix ownership to www-data for a site directory, without handing over `.git`.
///
/// No caller today has a `.git` present the moment this runs — Composer refuses
/// a non-empty target and Joomla's release zip ships none — so this is
/// prospective: once the CMS is live and running as www-data, a future
/// compromise of the app itself could create one, and this keeps that from
/// becoming a route to root (see `deploy.rs::hand_tree_to_web_user`, and
/// `deploy.rs::clone_or_pull`, which is what would run `.git`'s hooks as root).
async fn chown_site(path: &str) {
    if let Ok(mut entries) = tokio::fs::read_dir(path).await {
        while let Ok(Some(entry)) = entries.next_entry().await {
            if entry.file_name() == std::ffi::OsStr::new(".git") {
                continue;
            }
            safe_command("chown")
                .args(["-R", "www-data:www-data", &entry.path().to_string_lossy()])
                .output()
                .await
                .ok();
        }
    }
    let git_dir = format!("{path}/.git");
    if std::path::Path::new(&git_dir).exists() {
        safe_command("chown").args(["-R", "root:root", &git_dir]).output().await.ok();
        safe_command("chmod").args(["-R", "go-rwx", &git_dir]).output().await.ok();
    }
}

/// Tighten a freshly-written DB-credential file (an app's .env/settings.php/
/// configuration.php) to 640 so it isn't left at the process umask's default
/// 644. `chown_site` above already sets both owner AND group to www-data, so
/// this closes reads from any OTHER local account on the box — it does not
/// and cannot isolate one tenant's www-data-identity process from another's,
/// since every site's PHP-FPM pool and every client-role shell share that
/// same identity (nginx.rs hardcodes user=www-data/group=www-data for every
/// pool); that cross-tenant isolation gap is the separate, already-tracked
/// per-site-OS-user architecture question (#85), unchanged by this fix.
async fn secure_credential_file(path: &str) {
    safe_command("chmod").args(["640", path]).output().await.ok();
}

/// Extract the SHA-256 checksum GitHub Releases' own markdown notes publish
/// for one specific package file, from a release's `body` field.
///
/// GitHub does not publish a discrete checksum FILE for release assets the
/// way `getcomposer.org` does for `composer.phar` — Joomla instead documents
/// per-package SHA-256 hashes as a markdown table inside the free-text
/// release notes (stable across the 8 most recent releases sampled,
/// including betas/RCs, at the time this was written). Ties the extraction
/// directly to the exact filename about to be downloaded (rather than a
/// generic "ZIP Archive" label match) so it can't cross-match the sibling
/// "Update Packages" table's own same-labelled ZIP row for a different file.
/// Only the first backtick-delimited token on the SAME line as the filename
/// is considered, since each table row is one line.
fn extract_release_sha256(body: &str, filename: &str) -> Option<String> {
    let idx = body.find(filename)?;
    let after = &body[idx + filename.len()..];
    let line_end = after.find('\n').unwrap_or(after.len());
    let same_line = &after[..line_end];
    let start = same_line.find('`')? + 1;
    let rest = &same_line[start..];
    let end = rest.find('`')?;
    let candidate = &rest[..end];
    (candidate.len() == 64 && candidate.chars().all(|c| c.is_ascii_hexdigit()))
        .then(|| candidate.to_lowercase())
}

/// Ensure Composer is installed at /usr/local/bin/composer.
///
/// Verifies the downloaded phar's sha256 against the sidecar Composer itself
/// publishes at the same path (`<url>.sha256`) before making it executable —
/// a bare `curl` had no integrity check at all. Fails closed: any download,
/// checksum-fetch, or mismatch error removes the (possibly-tampered) phar
/// rather than leaving it in place for a retry to silently reuse.
pub async fn ensure_composer() -> Result<(), String> {
    if std::path::Path::new(COMPOSER).exists() {
        return Ok(());
    }
    const COMPOSER_URL: &str = "https://getcomposer.org/download/latest-stable/composer.phar";

    let fut = safe_command("curl")
        .args(["-sS", "-L", "-o", COMPOSER, COMPOSER_URL])
        .kill_on_drop(true)
        .output();
    let out = tokio::time::timeout(DOWNLOAD_TIMEOUT, fut)
        .await
        .map_err(|_| "Downloading Composer timed out".to_string())?
        .map_err(|e| format!("Download failed: {e}"))?;
    if !out.status.success() {
        return Err("Failed to download Composer".into());
    }

    let sha_fut = safe_command("curl")
        .args(["-sS", "-L", &format!("{COMPOSER_URL}.sha256")])
        .kill_on_drop(true)
        .output();
    let sha_out = tokio::time::timeout(QUICK_TIMEOUT, sha_fut)
        .await
        .map_err(|_| "Fetching Composer's checksum timed out".to_string())?
        .map_err(|e| format!("Failed to fetch Composer's checksum: {e}"))?;
    if !sha_out.status.success() {
        tokio::fs::remove_file(COMPOSER).await.ok();
        return Err("Failed to fetch Composer's checksum".into());
    }
    let expected = String::from_utf8_lossy(&sha_out.stdout).trim().to_lowercase();
    if expected.len() != 64 || !expected.chars().all(|c| c.is_ascii_hexdigit()) {
        tokio::fs::remove_file(COMPOSER).await.ok();
        return Err("Composer's checksum response was not a valid sha256 digest".into());
    }

    let phar_bytes = tokio::fs::read(COMPOSER)
        .await
        .map_err(|e| format!("Failed to read downloaded Composer: {e}"))?;
    let actual = hex::encode(Sha256::digest(&phar_bytes));
    if actual != expected {
        tokio::fs::remove_file(COMPOSER).await.ok();
        return Err(format!(
            "Composer checksum mismatch: expected {expected}, got {actual}"
        ));
    }

    safe_command("chmod")
        .args(["+x", COMPOSER])
        .kill_on_drop(true)
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
        COMPOSER_TIMEOUT,
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
    run_cmd_in(&site_dir, "php", &["artisan", "key:generate", "--force"], QUICK_TIMEOUT).await?;

    // Run migrations (allow failure — DB might not be ready)
    let _ = run_cmd_in(&site_dir, "php", &["artisan", "migrate", "--force"], CLI_INSTALL_TIMEOUT).await;

    // Create public symlink for nginx (Laravel's web root is public/ by default)
    // No symlink needed — Laravel already uses public/ as document root.

    chown_site(&site_dir).await;
    secure_credential_file(&env_file).await;

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
        COMPOSER_TIMEOUT,
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
        COMPOSER_TIMEOUT,
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
    //
    // db_user/db_pass/db_host are validated at the route layer
    // (routes/cms.rs) to reject '#', '/', '?' — the characters proven,
    // empirically against PHP's own parse_url() (which this URL eventually
    // goes through via Drupal's Connection::createConnectionOptionsFromUrl),
    // to either break parsing outright or — worse, for a '#' in the host —
    // silently truncate it into a URL fragment. Percent-encoding was
    // considered and rejected: Drupal's parser never calls
    // urldecode()/rawurldecode() on the parsed components, so an encoded
    // '#'/'/' would reach the database driver as a literal "%23"/"%2F"
    // instead of decoding back to the real credential.
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
        CLI_INSTALL_TIMEOUT,
    )
    .await?;

    chown_site(&site_dir).await;
    // drush writes settings.php under the recommended-project's web/ docroot.
    secure_credential_file(&format!("{site_dir}/web/sites/default/settings.php")).await;

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

    // Resolve the latest release via GitHub's Releases API (not the old
    // `-sI` redirect-header trick): the same response carries both the tag
    // AND a SHA-256 table for each package in `body`, needed below since
    // GitHub does not publish a discrete checksum FILE for release assets
    // the way getcomposer.org does for composer.phar.
    let release_json = run_cmd(
        "curl",
        &[
            "-sS",
            "-L",
            "-H",
            "Accept: application/vnd.github+json",
            "https://api.github.com/repos/joomla/joomla-cms/releases/latest",
        ],
        QUICK_TIMEOUT,
    )
    .await?;

    let release: serde_json::Value = serde_json::from_str(&release_json)
        .map_err(|e| format!("Failed to parse Joomla release info: {e}"))?;
    let tag = release
        .get("tag_name")
        .and_then(|v| v.as_str())
        .ok_or_else(|| "Joomla release info missing tag_name".to_string())?;
    if tag.is_empty()
        || tag.len() > 64
        || !tag.chars().all(|c| c.is_ascii_alphanumeric() || c == '.' || c == '-')
    {
        return Err(format!("Unexpected Joomla release tag format: {tag}"));
    }
    let body = release.get("body").and_then(|v| v.as_str()).unwrap_or("");

    let zip_filename = format!("Joomla_{tag}-Stable-Full_Package.zip");
    let expected_sha = extract_release_sha256(body, &zip_filename).ok_or_else(|| {
        format!("Could not find a published SHA-256 checksum for {zip_filename} in the release notes")
    })?;

    // Download Joomla zip with random suffix to prevent symlink attacks
    let random_suffix: u64 = rand::random();
    let zip_path = format!("/tmp/joomla-{domain}-{random_suffix:016x}.zip");
    let download_url =
        format!("https://github.com/joomla/joomla-cms/releases/download/{tag}/{zip_filename}");
    run_cmd("curl", &["-sL", "-o", &zip_path, &download_url], DOWNLOAD_TIMEOUT).await?;

    // Verify BEFORE extracting — fail closed on any mismatch or read error.
    let zip_bytes = tokio::fs::read(&zip_path)
        .await
        .map_err(|e| format!("Failed to read downloaded Joomla package: {e}"))?;
    let actual_sha = hex::encode(Sha256::digest(&zip_bytes));
    if actual_sha != expected_sha {
        tokio::fs::remove_file(&zip_path).await.ok();
        return Err(format!(
            "Joomla package checksum mismatch: expected {expected_sha}, got {actual_sha}"
        ));
    }
    drop(zip_bytes);

    // Extract
    run_cmd("unzip", &["-o", &zip_path, "-d", &public_dir], DOWNLOAD_TIMEOUT).await?;

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
        CLI_INSTALL_TIMEOUT,
    )
    .await?;

    chown_site(&public_dir).await;
    secure_credential_file(&format!("{public_dir}/configuration.php")).await;

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
        COMPOSER_TIMEOUT,
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
        COMPOSER_TIMEOUT,
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
    let env_content = set_ci_env(&env_content, "app.baseURL", &format!("https://{domain}"));

    let _ = title; // CI4 has no site-title in env

    tokio::fs::write(&env_file, env_content)
        .await
        .map_err(|e| format!("Failed to write .env: {e}"))?;

    chown_site(&site_dir).await;
    secure_credential_file(&env_file).await;

    Ok("CodeIgniter installed successfully".into())
}

/// Quote a value for a Laravel-style (`vlucas/phpdotenv`) `.env` file so it
/// round-trips byte-for-byte regardless of content.
///
/// Unquoted values are unsafe here for more than the space case this used to
/// guard against: phpdotenv's parser treats a bare `#` as a comment-start
/// with NO whitespace precondition (`EntryParser::processToken`,
/// `UNQUOTED_STATE` → `COMMENT_STATE` on any `#` token), silently truncating
/// everything after it. Double-quoting alone is not sufficient either — a
/// `$` inside a double-quoted value starts variable *interpolation*
/// (`DOUBLE_QUOTED_STATE` → emits with `$var = true`), which phpdotenv
/// resolves against the process environment when the app loads. `\`, `"`,
/// and `$` must all be backslash-escaped for a double-quoted value to be
/// pure-literal in this parser — verified against the real library (`composer
/// require vlucas/phpdotenv`, round-tripping `#`, space, `"`, `'`, `\`, `$`,
/// and all of them combined in one value).
fn quote_laravel_env_value(value: &str) -> String {
    let escaped = value.replace('\\', "\\\\").replace('"', "\\\"").replace('$', "\\$");
    format!("\"{escaped}\"")
}

/// Replace or add a KEY=value line in a .env file (Laravel-style: KEY=value).
fn replace_env_line(content: &str, key: &str, value: &str) -> String {
    let prefix = format!("{key}=");
    let quoted = quote_laravel_env_value(value);
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

/// Quote a value for CodeIgniter 4's OWN `.env` parser (`system/Config/DotEnv.php`
/// — not phpdotenv; CI4 rolls its own, with different escaping rules).
///
/// `sanitizeValue()` accepts, inside a quoted value, only `\\` (a literal
/// backslash) and `\"` (a literal quote) as escapes — its unquoting regex
/// only recognizes those two backslash sequences, so escaping anything else
/// (in particular a `\$`, which phpdotenv WOULD want) desyncs that regex
/// instead of producing a literal `$`. So `$` is deliberately left
/// un-escaped here. That leaves one narrow, unfixable-from-this-side gap:
/// `resolveNestedVariables()` runs unconditionally after unquoting and
/// substitutes any literal `${SOME_VAR}` substring against the process
/// environment, with no way to escape it — CI4 itself provides none. A
/// value that happens to contain that exact substring, for an env var that
/// happens to be set at load time, would be silently rewritten. Given
/// `safe_command`'s child environment is a fixed, minimal set (PATH, HOME,
/// LANG, LC_ALL, DOCKER_CONFIG — nothing password-shaped), this is a
/// theoretical residual, not a practical one; documented rather than
/// silently left unmentioned. Verified against the real parser logic
/// (`sanitizeValue`/`resolveNestedVariables`, pulled from CodeIgniter4's own
/// source) round-tripping `#`, space, `"`, `'`, `\`, and `$`.
fn quote_ci4_env_value(value: &str) -> String {
    let escaped = value.replace('\\', "\\\\").replace('"', "\\\"");
    format!("\"{escaped}\"")
}

/// Set a CodeIgniter .env value, uncommenting if necessary.
/// CI4 .env lines look like `# database.default.hostname = localhost` (commented) or
/// `database.default.hostname = localhost` (active).
fn set_ci_env(content: &str, key: &str, value: &str) -> String {
    let quoted = quote_ci4_env_value(value);
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
                    return format!("{key} = {quoted}");
                }
            }
            line.to_string()
        })
        .collect();

    if !found {
        lines.push(format!("{key} = {quoted}"));
    }
    let mut result = lines.join("\n");
    if content.ends_with('\n') {
        result.push('\n');
    }
    result
}
