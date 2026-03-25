use std::path::Path;
use crate::safe_cmd::safe_command;

/// RAII guard that deletes a temp file on drop, ensuring cleanup on all code paths.
struct TempFileGuard {
    path: String,
}

impl TempFileGuard {
    fn create(label: &str, content: &str) -> Result<Self, String> {
        let random_suffix: u64 = rand::random();
        let path = format!("/tmp/.dockpanel-s3-{}-{:016x}", label, random_suffix);
        std::fs::write(&path, content)
            .map_err(|e| format!("Failed to write S3 config: {e}"))?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600)).ok();
        }
        Ok(Self { path })
    }
}

impl Drop for TempFileGuard {
    fn drop(&mut self) {
        std::fs::remove_file(&self.path).ok();
    }
}

/// Upload a backup file to S3-compatible storage using curl --aws-sigv4.
pub async fn upload_s3(
    filepath: &str,
    bucket: &str,
    region: &str,
    endpoint: &str,
    access_key: &str,
    secret_key: &str,
    remote_path: &str,
) -> Result<String, String> {
    let path = Path::new(filepath);
    if !path.exists() {
        return Err(format!("Backup file not found: {filepath}"));
    }

    let filename = path
        .file_name()
        .and_then(|n| n.to_str())
        .ok_or("Invalid filename")?;

    // Build the S3 URL: endpoint/bucket/prefix/filename
    let prefix = remote_path.trim_matches('/');
    let url = if prefix.is_empty() {
        format!("{endpoint}/{bucket}/{filename}")
    } else {
        format!("{endpoint}/{bucket}/{prefix}/{filename}")
    };

    tracing::info!("Uploading {filename} to {url}");

    // Write credentials to a temp file so they don't appear in process listing
    let config_content = format!("user = \"{}:{}\"", access_key, secret_key);
    let config_guard = TempFileGuard::create("upload", &config_content)?;

    let output = tokio::time::timeout(
        std::time::Duration::from_secs(600),
        safe_command("curl")
            .args([
                "--aws-sigv4",
                &format!("aws:amz:{region}:s3"),
                "-K",
                &config_guard.path,
                "-X",
                "PUT",
                "-H",
                "Content-Type: application/gzip",
                "-T",
                filepath,
                "--fail",
                "--silent",
                "--show-error",
                &url,
            ])
            .output(),
    )
    .await
    .map_err(|_| "Upload timed out (10 min limit)".to_string())?
    .map_err(|e| format!("Failed to run curl: {e}"))?;

    drop(config_guard);

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(format!("S3 upload failed: {stderr}"));
    }

    tracing::info!("Upload complete: {filename}");
    Ok(url)
}

/// Upload a backup file via SCP.
pub async fn upload_sftp(
    filepath: &str,
    host: &str,
    port: u16,
    username: &str,
    password: Option<&str>,
    key_path: Option<&str>,
    remote_path: &str,
) -> Result<String, String> {
    let path = Path::new(filepath);
    if !path.exists() {
        return Err(format!("Backup file not found: {filepath}"));
    }

    let filename = path
        .file_name()
        .and_then(|n| n.to_str())
        .ok_or("Invalid filename")?;

    let remote_dest = format!(
        "{username}@{host}:{}/{}",
        remote_path.trim_end_matches('/'),
        filename
    );

    tracing::info!("Uploading {filename} via SCP to {remote_dest}");

    let mut cmd_args: Vec<String> = vec![
        "-o".into(),
        "StrictHostKeyChecking=no".into(),
        "-o".into(),
        "BatchMode=yes".into(),
        "-P".into(),
        port.to_string(),
    ];

    if let Some(key) = key_path {
        cmd_args.push("-i".into());
        cmd_args.push(key.into());
    }

    cmd_args.push(filepath.into());
    cmd_args.push(remote_dest.clone());

    // If password auth, use sshpass with -e flag (reads SSHPASS env var, not visible in ps)
    let (program, final_args, sshpass_env) = if let Some(pw) = password {
        if key_path.is_some() {
            // Key takes priority
            ("scp".to_string(), cmd_args, None)
        } else {
            let mut args = vec!["-e".into(), "scp".into()];
            args.extend(cmd_args);
            ("sshpass".to_string(), args, Some(pw.to_string()))
        }
    } else {
        ("scp".to_string(), cmd_args, None)
    };

    let mut cmd = safe_command(&program);
    cmd.args(&final_args);
    if let Some(ref pw) = sshpass_env {
        cmd.env("SSHPASS", pw);
    }

    let output = tokio::time::timeout(
        std::time::Duration::from_secs(600),
        cmd.output(),
    )
    .await
    .map_err(|_| "Upload timed out (10 min limit)".to_string())?
    .map_err(|e| format!("Failed to run {program}: {e}"))?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(format!("SCP upload failed: {stderr}"));
    }

    tracing::info!("SCP upload complete: {filename}");
    Ok(remote_dest)
}

/// Test S3 connection by listing the bucket.
pub async fn test_s3(
    bucket: &str,
    region: &str,
    endpoint: &str,
    access_key: &str,
    secret_key: &str,
) -> Result<(), String> {
    // HEAD request on the bucket to check access
    let url = format!("{endpoint}/{bucket}/");

    // Write credentials to a temp file so they don't appear in process listing
    let config_content = format!("user = \"{}:{}\"", access_key, secret_key);
    let config_guard = TempFileGuard::create("test", &config_content)?;

    let output = tokio::time::timeout(
        std::time::Duration::from_secs(15),
        safe_command("curl")
            .args([
                "--aws-sigv4",
                &format!("aws:amz:{region}:s3"),
                "-K",
                &config_guard.path,
                "-I",
                "--fail",
                "--silent",
                "--show-error",
                &url,
            ])
            .output(),
    )
    .await
    .map_err(|_| "Connection test timed out".to_string())?
    .map_err(|e| format!("Connection test failed: {e}"))?;

    drop(config_guard);

    if output.status.success() {
        Ok(())
    } else {
        let stderr = String::from_utf8_lossy(&output.stderr);
        Err(format!("S3 connection test failed: {stderr}"))
    }
}

/// Test SFTP connection.
pub async fn test_sftp(
    host: &str,
    port: u16,
    username: &str,
    password: Option<&str>,
    key_path: Option<&str>,
) -> Result<(), String> {
    let mut cmd_args: Vec<String> = vec![
        "-o".into(),
        "StrictHostKeyChecking=no".into(),
        "-o".into(),
        "BatchMode=yes".into(),
        "-o".into(),
        "ConnectTimeout=10".into(),
        "-p".into(),
        port.to_string(),
        format!("{username}@{host}"),
        "exit".into(),
    ];

    let (program, final_args, sshpass_env) = if let Some(pw) = password {
        if key_path.is_some() {
            ("ssh".to_string(), cmd_args, None)
        } else {
            let mut args = vec!["-e".into(), "ssh".into()];
            args.extend(cmd_args);
            ("sshpass".to_string(), args, Some(pw.to_string()))
        }
    } else {
        if let Some(key) = key_path {
            cmd_args.insert(6, "-i".into());
            cmd_args.insert(7, key.into());
        }
        ("ssh".to_string(), cmd_args, None)
    };

    let mut cmd = safe_command(&program);
    cmd.args(&final_args);
    if let Some(ref pw) = sshpass_env {
        cmd.env("SSHPASS", pw);
    }

    let output = tokio::time::timeout(
        std::time::Duration::from_secs(15),
        cmd.output(),
    )
    .await
    .map_err(|_| "Connection test timed out".to_string())?
    .map_err(|e| format!("SSH test failed: {e}"))?;

    if output.status.success() {
        Ok(())
    } else {
        let stderr = String::from_utf8_lossy(&output.stderr);
        Err(format!("SFTP connection test failed: {stderr}"))
    }
}

/// List remote backups in S3 bucket with given prefix.
pub async fn list_s3(
    bucket: &str,
    region: &str,
    endpoint: &str,
    access_key: &str,
    secret_key: &str,
    prefix: &str,
) -> Result<Vec<String>, String> {
    let prefix_clean = prefix.trim_matches('/');
    let url = if prefix_clean.is_empty() {
        format!("{endpoint}/{bucket}/?list-type=2")
    } else {
        format!("{endpoint}/{bucket}/?list-type=2&prefix={prefix_clean}/")
    };

    // Write credentials to a temp file so they don't appear in process listing
    let config_content = format!("user = \"{}:{}\"", access_key, secret_key);
    let config_guard = TempFileGuard::create("list", &config_content)?;

    let output = tokio::time::timeout(
        std::time::Duration::from_secs(30),
        safe_command("curl")
            .args([
                "--aws-sigv4",
                &format!("aws:amz:{region}:s3"),
                "-K",
                &config_guard.path,
                "--fail",
                "--silent",
                &url,
            ])
            .output(),
    )
    .await
    .map_err(|_| "List timed out".to_string())?
    .map_err(|e| format!("List failed: {e}"))?;

    drop(config_guard);

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(format!("S3 list failed: {stderr}"));
    }

    // Parse XML response — extract <Key> elements
    let body = String::from_utf8_lossy(&output.stdout);
    let keys: Vec<String> = body
        .split("<Key>")
        .skip(1)
        .filter_map(|s| s.split("</Key>").next().map(|k| k.to_string()))
        .collect();

    Ok(keys)
}

/// Delete a file from S3.
pub async fn delete_s3(
    bucket: &str,
    region: &str,
    endpoint: &str,
    access_key: &str,
    secret_key: &str,
    key: &str,
) -> Result<(), String> {
    let url = format!("{endpoint}/{bucket}/{key}");

    // Write credentials to a temp file so they don't appear in process listing
    let config_content = format!("user = \"{}:{}\"", access_key, secret_key);
    let config_guard = TempFileGuard::create("delete", &config_content)?;

    let output = tokio::time::timeout(
        std::time::Duration::from_secs(15),
        safe_command("curl")
            .args([
                "--aws-sigv4",
                &format!("aws:amz:{region}:s3"),
                "-K",
                &config_guard.path,
                "-X",
                "DELETE",
                "--fail",
                "--silent",
                "--show-error",
                &url,
            ])
            .output(),
    )
    .await
    .map_err(|_| "Delete timed out".to_string())?
    .map_err(|e| format!("Delete failed: {e}"))?;

    drop(config_guard);

    if output.status.success() {
        Ok(())
    } else {
        let stderr = String::from_utf8_lossy(&output.stderr);
        Err(format!("S3 delete failed: {stderr}"))
    }
}
