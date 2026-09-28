use std::path::{Path, PathBuf};
use tokio::fs;
use crate::safe_cmd::safe_command;

const WEBROOT: &str = "/var/www";

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct FileEntry {
    pub name: String,
    pub path: String,
    pub is_dir: bool,
    pub size: u64,
    pub modified: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub permissions: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub owner: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub group: Option<String>,
    pub is_symlink: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub symlink_target: Option<String>,
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct FileContent {
    pub content: String,
    pub size: u64,
    pub modified: String,
    pub is_binary: bool,
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct FileStat {
    pub name: String,
    pub path: String,
    pub is_dir: bool,
    pub is_symlink: bool,
    pub symlink_target: Option<String>,
    pub size: u64,
    pub mime_type: String,
    pub permissions_octal: String,
    pub permissions_symbolic: String,
    pub owner: String,
    pub group: String,
    pub uid: u32,
    pub gid: u32,
    pub created: String,
    pub modified: String,
    pub accessed: String,
    pub sha256: Option<String>,
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct BulkResult {
    pub succeeded: Vec<String>,
    pub failed: Vec<BulkFailure>,
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct BulkFailure {
    pub path: String,
    pub error: String,
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct ArchiveEntry {
    pub name: String,
    pub size: u64,
    pub is_dir: bool,
    pub modified: Option<String>,
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct ExtractResult {
    pub success: bool,
    pub extracted_count: usize,
    pub destination: String,
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct SearchResult {
    pub name: String,
    pub path: String,
    pub is_dir: bool,
    pub size: u64,
    pub modified: String,
    pub line_matches: Vec<LineMatch>,
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct LineMatch {
    pub line_number: usize,
    pub line_content: String,
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct StorageStats {
    pub total_bytes: u64,
    pub file_count: usize,
    pub dir_count: usize,
    pub breakdown: Vec<StorageItem>,
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct StorageItem {
    pub name: String,
    pub path: String,
    pub is_dir: bool,
    pub size: u64,
}

/// Resolve a user-provided path to a safe absolute path within /var/www/{domain}/.
/// Prevents path traversal attacks.
pub fn resolve_safe_path(domain: &str, relative_path: &str) -> Result<PathBuf, String> {
    let base = PathBuf::from(format!("{WEBROOT}/{domain}"));
    resolve_within(&base, relative_path)
}

/// Resolve a path that must name something INSIDE the site root — never the root itself.
pub fn resolve_safe_child(domain: &str, relative_path: &str) -> Result<PathBuf, String> {
    let base = PathBuf::from(format!("{WEBROOT}/{domain}"));
    let resolved = resolve_within(&base, relative_path)?;
    let canon_base = base
        .canonicalize()
        .map_err(|_| format!("Site root does not exist: {}", base.display()))?;
    if resolved == canon_base {
        return Err("Refusing to operate on the site root itself".into());
    }
    Ok(resolved)
}

/// Core of [`resolve_safe_path`], parameterized on the base directory so it is
/// unit-testable. Guarantees the returned path is inside `base` AND that no symlink
/// sits in the to-be-created portion of the path.
pub(crate) fn resolve_within(base: &Path, relative_path: &str) -> Result<PathBuf, String> {
    let cleaned = relative_path.trim_start_matches('/');
    let candidate = base.join(cleaned);

    let canon_base = base
        .canonicalize()
        .map_err(|_| format!("Site root does not exist: {}", base.display()))?;

    let canon = if candidate.exists() {
        candidate
            .canonicalize()
            .map_err(|e| format!("Path error: {e}"))?
    } else {
        let mut existing = candidate.clone();
        let mut trail = Vec::new();
        while !existing.exists() {
            if let Some(name) = existing.file_name() {
                trail.push(name.to_owned());
            } else {
                return Err("Invalid path".into());
            }
            existing = existing.parent().ok_or("Invalid path")?.to_path_buf();
        }
        let mut real = existing.clone();
        for component in trail.iter().rev() {
            real = real.join(component);
            if let Ok(meta) = std::fs::symlink_metadata(&real) {
                if meta.file_type().is_symlink() {
                    return Err("Path traversal denied (symlink in target path)".into());
                }
            }
        }
        let mut resolved = existing
            .canonicalize()
            .map_err(|e| format!("Path resolution error: {e}"))?;
        for component in trail.into_iter().rev() {
            resolved = resolved.join(component);
        }
        resolved
    };

    if !canon.starts_with(&canon_base) {
        return Err("Path traversal denied".into());
    }

    Ok(canon)
}

/// Determine MIME type from file extension.
pub fn mime_type_for_path(path: &Path) -> &'static str {
    let ext = path
        .extension()
        .and_then(|e| e.to_str())
        .map(|e| e.to_ascii_lowercase())
        .unwrap_or_default();

    match ext.as_str() {
        "html" | "htm" => "text/html",
        "css" => "text/css",
        "js" | "mjs" => "application/javascript",
        "ts" | "tsx" => "text/typescript",
        "jsx" => "text/jsx",
        "json" => "application/json",
        "php" | "phtml" => "application/x-httpd-php",
        "png" => "image/png",
        "jpg" | "jpeg" => "image/jpeg",
        "gif" => "image/gif",
        "svg" => "image/svg+xml",
        "webp" => "image/webp",
        "ico" => "image/x-icon",
        "pdf" => "application/pdf",
        "zip" => "application/zip",
        "tar" => "application/x-tar",
        "gz" => "application/gzip",
        "tgz" => "application/gzip",
        "7z" => "application/x-7z-compressed",
        "mp4" => "video/mp4",
        "webm" => "video/webm",
        "mp3" => "audio/mpeg",
        "wav" => "audio/wav",
        "ogg" => "audio/ogg",
        "txt" => "text/plain",
        "md" => "text/markdown",
        "xml" => "application/xml",
        "yaml" | "yml" => "application/yaml",
        "sh" | "bash" => "application/x-sh",
        "sql" => "application/sql",
        "env" => "text/plain",
        "htaccess" | "conf" | "ini" => "text/plain",
        _ => "application/octet-stream",
    }
}

/// Format mode bits as symbolic string (e.g. "-rw-r--r--", "drwxr-xr-x").
#[cfg(unix)]
fn format_symbolic_permissions(mode: u32, is_dir: bool, is_symlink: bool) -> String {
    let prefix = if is_symlink {
        'l'
    } else if is_dir {
        'd'
    } else {
        '-'
    };

    let rwx = |shift: u32| -> (char, char, char) {
        let r = if (mode >> (shift + 2)) & 1 == 1 { 'r' } else { '-' };
        let w = if (mode >> (shift + 1)) & 1 == 1 { 'w' } else { '-' };
        let x = if (mode >> shift) & 1 == 1 { 'x' } else { '-' };
        (r, w, x)
    };

    let (ur, uw, ux) = rwx(6);
    let (gr, gw, gx) = rwx(3);
    let (or, ow, ox) = rwx(0);

    format!("{prefix}{ur}{uw}{ux}{gr}{gw}{gx}{or}{ow}{ox}")
}

#[cfg(not(unix))]
fn format_symbolic_permissions(_mode: u32, is_dir: bool, is_symlink: bool) -> String {
    if is_symlink { "lrwxrwxrwx".into() } else if is_dir { "drwxr-xr-x".into() } else { "-rw-r--r--".into() }
}

/// List directory contents.
pub async fn list_directory(path: &Path, site_root: Option<&Path>) -> Result<Vec<FileEntry>, String> {
    let mut entries = Vec::new();
    let mut reader = fs::read_dir(path)
        .await
        .map_err(|e| format!("Cannot read directory: {e}"))?;

    while let Some(entry) = reader
        .next_entry()
        .await
        .map_err(|e| format!("Read entry error: {e}"))?
    {
        let symlink_meta = fs::symlink_metadata(entry.path()).await.ok();
        let is_symlink = symlink_meta.as_ref().map(|m| m.file_type().is_symlink()).unwrap_or(false);
        let meta = entry.metadata().await.ok();
        let is_dir = meta.as_ref().map(|m| m.is_dir()).unwrap_or(false);
        let size = meta.as_ref().map(|m| m.len()).unwrap_or(0);
        let modified = meta
            .as_ref()
            .and_then(|m| m.modified().ok())
            .map(|t| {
                let dt: chrono::DateTime<chrono::Utc> = t.into();
                dt.format("%Y-%m-%d %H:%M:%S").to_string()
            })
            .unwrap_or_default();

        let symlink_target = if is_symlink {
            fs::read_link(entry.path())
                .await
                .ok()
                .map(|p| p.to_string_lossy().to_string())
        } else {
            None
        };

        #[cfg(unix)]
        let (permissions, owner, group) = {
            use std::os::unix::fs::MetadataExt;
            if let Some(ref m) = meta {
                let mode = m.mode() & 0o777;
                let octal = format!("{:04o}", mode);
                let uid = m.uid();
                let gid = m.gid();
                (Some(octal), Some(uid.to_string()), Some(gid.to_string()))
            } else {
                (None, None, None)
            }
        };

        #[cfg(not(unix))]
        let (permissions, owner, group) = (Some("0644".to_string()), Some("www-data".to_string()), Some("www-data".to_string()));

        let name = entry.file_name().to_string_lossy().to_string();
        let abs_path = format!("{}/{}", path.display(), &name);

        let relative_path = if let Some(root) = site_root {
            let canon_root = root.canonicalize().unwrap_or_else(|_| root.to_path_buf());
            match std::path::Path::new(&abs_path).strip_prefix(&canon_root) {
                Ok(rel) => rel.to_string_lossy().to_string(),
                Err(_) => name.clone(),
            }
        } else {
            name.clone()
        };

        entries.push(FileEntry {
            path: relative_path,
            name,
            is_dir,
            size,
            modified,
            permissions,
            owner,
            group,
            is_symlink,
            symlink_target,
        });
    }

    // Sort: directories first, then alphabetical
    entries.sort_by(|a, b| b.is_dir.cmp(&a.is_dir).then(a.name.cmp(&b.name)));
    Ok(entries)
}

/// Read file content (text only, max 5MB).
pub async fn read_file(path: &Path) -> Result<FileContent, String> {
    let meta = fs::metadata(path)
        .await
        .map_err(|e| format!("File not found: {e}"))?;

    if meta.len() > 5 * 1024 * 1024 {
        return Err("File too large (max 5MB)".into());
    }

    let bytes = fs::read(path)
        .await
        .map_err(|e| format!("Read failed: {e}"))?;

    let is_binary = bytes.iter().take(1024).any(|&b| b == 0);
    let content = if is_binary {
        String::new()
    } else {
        String::from_utf8(bytes).map_err(|_| "File is binary or not readable as text".to_string())?
    };

    let modified = meta
        .modified()
        .ok()
        .map(|t| {
            let dt: chrono::DateTime<chrono::Utc> = t.into();
            dt.format("%Y-%m-%d %H:%M:%S").to_string()
        })
        .unwrap_or_default();

    Ok(FileContent {
        content,
        size: meta.len(),
        modified,
        is_binary,
    })
}

/// Write file content atomically. Creates parent directories if needed.
pub async fn write_file(path: &Path, content: &str) -> Result<(), String> {
    if let Some(parent) = path.parent() {
        if !parent.exists() {
            fs::create_dir_all(parent)
                .await
                .map_err(|e| format!("Failed to create directory: {e}"))?;
        }
    }
    let tmp = match path.parent() {
        Some(parent) => parent.join(format!(".dptmp.{}", uuid::Uuid::new_v4())),
        None => return Err("Invalid path".into()),
    };
    if let Err(e) = fs::write(&tmp, content).await {
        let _ = fs::remove_file(&tmp).await;
        return Err(format!("Failed to write: {e}"));
    }
    if let Err(e) = fs::rename(&tmp, path).await {
        let _ = fs::remove_file(&tmp).await;
        return Err(format!("Failed to finalize write: {e}"));
    }
    Ok(())
}

/// Create a file or directory.
pub async fn create_entry(path: &Path, is_dir: bool) -> Result<(), String> {
    if path.exists() {
        return Err("Path already exists".into());
    }
    if is_dir {
        fs::create_dir_all(path)
            .await
            .map_err(|e| format!("Failed to create directory: {e}"))?;
    } else {
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).await.ok();
        }
        fs::write(path, "")
            .await
            .map_err(|e| format!("Failed to create file: {e}"))?;
    }
    Ok(())
}

/// Rename or move an entry.
pub async fn rename_entry(from: &Path, to: &Path) -> Result<(), String> {
    if !from.exists() {
        return Err("Source does not exist".into());
    }
    if to.exists() {
        return Err("Destination already exists".into());
    }
    if let Some(parent) = to.parent() {
        if !parent.exists() {
            fs::create_dir_all(parent)
                .await
                .map_err(|e| format!("Failed to create parent directory: {e}"))?;
        }
    }
    fs::rename(from, to)
        .await
        .map_err(|e| format!("Rename failed: {e}"))?;
    Ok(())
}

/// Delete a file or directory.
pub async fn delete_entry(path: &Path) -> Result<(), String> {
    if !path.exists() {
        return Err("Path does not exist".into());
    }
    if path.is_dir() {
        fs::remove_dir_all(path)
            .await
            .map_err(|e| format!("Failed to delete directory: {e}"))?;
    } else {
        fs::remove_file(path)
            .await
            .map_err(|e| format!("Failed to delete file: {e}"))?;
    }
    Ok(())
}

/// Copy a file or directory recursively.
pub async fn copy_entry(from: &Path, to: &Path) -> Result<(), String> {
    if !from.exists() {
        return Err("Source does not exist".into());
    }
    if to.exists() {
        return Err("Destination already exists".into());
    }
    if let Some(parent) = to.parent() {
        if !parent.exists() {
            fs::create_dir_all(parent)
                .await
                .map_err(|e| format!("Failed to create parent directory: {e}"))?;
        }
    }

    if from.is_dir() {
        copy_dir_recursive(from, to).await
    } else {
        fs::copy(from, to)
            .await
            .map_err(|e| format!("Copy failed: {e}"))?;
        Ok(())
    }
}

/// Recursive directory copy.
pub async fn copy_dir_recursive(from: &Path, to: &Path) -> Result<(), String> {
    fs::create_dir_all(to)
        .await
        .map_err(|e| format!("Failed to create directory {}: {e}", to.display()))?;

    let mut reader = fs::read_dir(from)
        .await
        .map_err(|e| format!("Failed to read directory {}: {e}", from.display()))?;

    while let Some(entry) = reader
        .next_entry()
        .await
        .map_err(|e| format!("Read entry error: {e}"))?
    {
        let src = entry.path();
        let dst = to.join(entry.file_name());

        if src.is_dir() {
            Box::pin(copy_dir_recursive(&src, &dst)).await?;
        } else {
            fs::copy(&src, &dst)
                .await
                .map_err(|e| format!("Failed to copy file {}: {e}", src.display()))?;
        }
    }

    Ok(())
}

/// Duplicate a file or directory within its parent directory.
pub async fn duplicate_entry(path: &Path) -> Result<String, String> {
    if !path.exists() {
        return Err("Path does not exist".into());
    }
    let parent = path.parent().ok_or("Invalid path")?;
    let stem = path.file_stem().and_then(|s| s.to_str()).unwrap_or("file");
    let ext = path.extension().and_then(|e| e.to_str());

    let is_dir = path.is_dir();
    let mut candidate_name = if is_dir || ext.is_none() {
        format!("{stem} (copy)")
    } else {
        format!("{stem} (copy).{}", ext.unwrap())
    };

    let mut counter = 2;
    while parent.join(&candidate_name).exists() {
        candidate_name = if is_dir || ext.is_none() {
            format!("{stem} (copy {counter})")
        } else {
            format!("{stem} (copy {counter}).{}", ext.unwrap())
        };
        counter += 1;
    }

    let target = parent.join(&candidate_name);
    copy_entry(path, &target).await?;

    Ok(candidate_name)
}

/// Detailed file inspection & stats with optional checksum calculation.
pub async fn stat_entry(path: &Path, rel_path: &str, calculate_checksums: bool) -> Result<FileStat, String> {
    let symlink_meta = fs::symlink_metadata(path)
        .await
        .map_err(|e| format!("File not found: {e}"))?;
    let is_symlink = symlink_meta.file_type().is_symlink();

    let meta = fs::metadata(path)
        .await
        .map_err(|e| format!("File not found: {e}"))?;

    let is_dir = meta.is_dir();
    let size = meta.len();
    let name = path.file_name().and_then(|n| n.to_str()).unwrap_or("").to_string();
    let mime_type = if is_dir {
        "inode/directory".to_string()
    } else {
        mime_type_for_path(path).to_string()
    };

    let symlink_target = if is_symlink {
        fs::read_link(path)
            .await
            .ok()
            .map(|p| p.to_string_lossy().to_string())
    } else {
        None
    };

    #[cfg(unix)]
    let (permissions_octal, permissions_symbolic, uid, gid, owner, group) = {
        use std::os::unix::fs::MetadataExt;
        let mode = meta.mode();
        let octal = format!("{:04o}", mode & 0o777);
        let symbolic = format_symbolic_permissions(mode, is_dir, is_symlink);
        let uid = meta.uid();
        let gid = meta.gid();
        (octal, symbolic, uid, gid, uid.to_string(), gid.to_string())
    };

    #[cfg(not(unix))]
    let (permissions_octal, permissions_symbolic, uid, gid, owner, group) = (
        "0644".to_string(),
        if is_dir { "drwxr-xr-x".to_string() } else { "-rw-r--r--".to_string() },
        1000,
        1000,
        "www-data".to_string(),
        "www-data".to_string(),
    );

    let modified = meta
        .modified()
        .ok()
        .map(|t| {
            let dt: chrono::DateTime<chrono::Utc> = t.into();
            dt.format("%Y-%m-%d %H:%M:%S").to_string()
        })
        .unwrap_or_default();

    let created = meta
        .created()
        .ok()
        .map(|t| {
            let dt: chrono::DateTime<chrono::Utc> = t.into();
            dt.format("%Y-%m-%d %H:%M:%S").to_string()
        })
        .unwrap_or_else(|| modified.clone());

    let accessed = meta
        .accessed()
        .ok()
        .map(|t| {
            let dt: chrono::DateTime<chrono::Utc> = t.into();
            dt.format("%Y-%m-%d %H:%M:%S").to_string()
        })
        .unwrap_or_else(|| modified.clone());

    let sha256 = if !is_dir && calculate_checksums && size <= 50 * 1024 * 1024 {
        if let Ok(bytes) = fs::read(path).await {
            use sha2::{Digest, Sha256};
            let mut hasher = Sha256::new();
            hasher.update(&bytes);
            Some(format!("{:x}", hasher.finalize()))
        } else {
            None
        }
    } else {
        None
    };

    Ok(FileStat {
        name,
        path: rel_path.to_string(),
        is_dir,
        is_symlink,
        symlink_target,
        size,
        mime_type,
        permissions_octal,
        permissions_symbolic,
        owner,
        group,
        uid,
        gid,
        created,
        modified,
        accessed,
        sha256,
    })
}

/// Change permissions of a file or directory (chmod).
pub async fn chmod_entry(path: &Path, mode: u32, recursive: bool) -> Result<(), String> {
    if !path.exists() {
        return Err("Path does not exist".into());
    }

    let mode_str = format!("{:04o}", mode & 0o777);
    let mut cmd = safe_command("chmod");
    if recursive && path.is_dir() {
        cmd.arg("-R");
    }
    cmd.arg(&mode_str);
    cmd.arg(path.as_os_str());

    let out = cmd.output().await.map_err(|e| format!("chmod failed: {e}"))?;
    if !out.status.success() {
        let err_msg = String::from_utf8_lossy(&out.stderr);
        return Err(format!("chmod failed: {err_msg}"));
    }
    Ok(())
}

/// Change owner/group of a file or directory (chown).
pub async fn chown_entry(path: &Path, owner: &str, group: &str, recursive: bool) -> Result<(), String> {
    if !path.exists() {
        return Err("Path does not exist".into());
    }

    // Sanitize owner and group strings
    let is_safe_name = |s: &str| !s.is_empty() && s.chars().all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-');
    if !is_safe_name(owner) || !is_safe_name(group) {
        return Err("Invalid owner or group format".into());
    }

    let owner_group = format!("{owner}:{group}");
    let mut cmd = safe_command("chown");
    if recursive && path.is_dir() {
        cmd.arg("-R");
    }
    cmd.arg(&owner_group);
    cmd.arg(path.as_os_str());

    let out = cmd.output().await.map_err(|e| format!("chown failed: {e}"))?;
    if !out.status.success() {
        let err_msg = String::from_utf8_lossy(&out.stderr);
        return Err(format!("chown failed: {err_msg}"));
    }
    Ok(())
}

/// Compress selected paths into an archive (ZIP, TAR, TAR.GZ).
pub async fn compress_entries(
    domain: &str,
    source_rels: &[String],
    archive_name: &str,
    target_dir_rel: &str,
) -> Result<String, String> {
    if source_rels.is_empty() {
        return Err("No source files specified".into());
    }

    let clean_archive = archive_name.trim();
    if clean_archive.is_empty() || clean_archive.contains('/') || clean_archive.contains('\\') || clean_archive.contains("..") {
        return Err("Invalid archive name".into());
    }

    let site_root = PathBuf::from(format!("{WEBROOT}/{domain}"));
    let target_dir = resolve_safe_path(domain, target_dir_rel)?;
    let archive_path = target_dir.join(clean_archive);

    if archive_path.exists() {
        return Err("Destination archive already exists".into());
    }

    let is_zip = clean_archive.ends_with(".zip");
    let is_tar_gz = clean_archive.ends_with(".tar.gz") || clean_archive.ends_with(".tgz");
    let is_tar = clean_archive.ends_with(".tar");

    if !is_zip && !is_tar_gz && !is_tar {
        return Err("Unsupported archive format. Use .zip, .tar.gz, or .tar".into());
    }

    // Resolve every source and ensure it is inside the site root
    let mut resolved_sources = Vec::new();
    for src in source_rels {
        let p = resolve_safe_path(domain, src)?;
        if !p.exists() {
            return Err(format!("Source does not exist: {src}"));
        }
        let rel_from_site = p.strip_prefix(&site_root).map_err(|_| "Source outside site root")?;
        resolved_sources.push(rel_from_site.to_string_lossy().to_string());
    }

    if is_zip {
        let mut cmd = safe_command("zip");
        cmd.current_dir(&site_root);
        cmd.arg("-r");
        cmd.arg(&archive_path);
        for src in &resolved_sources {
            cmd.arg(src);
        }
        let out = cmd.output().await.map_err(|e| format!("zip command failed: {e}"))?;
        if !out.status.success() {
            let err_msg = String::from_utf8_lossy(&out.stderr);
            return Err(format!("Compression failed: {err_msg}"));
        }
    } else {
        let mut cmd = safe_command("tar");
        cmd.current_dir(&site_root);
        if is_tar_gz {
            cmd.arg("czf");
        } else {
            cmd.arg("cf");
        }
        cmd.arg(&archive_path);
        for src in &resolved_sources {
            cmd.arg(src);
        }
        let out = cmd.output().await.map_err(|e| format!("tar command failed: {e}"))?;
        if !out.status.success() {
            let err_msg = String::from_utf8_lossy(&out.stderr);
            return Err(format!("Compression failed: {err_msg}"));
        }
    }

    Ok(clean_archive.to_string())
}

/// Extract an archive (.zip, .tar, .tar.gz, .tgz) safely into a destination directory.
pub async fn extract_archive(
    domain: &str,
    archive_rel: &str,
    target_dir_rel: &str,
    overwrite: bool,
) -> Result<ExtractResult, String> {
    let archive_path = resolve_safe_path(domain, archive_rel)?;
    if !archive_path.exists() {
        return Err("Archive file does not exist".into());
    }

    let target_dir = resolve_safe_path(domain, target_dir_rel)?;
    if !target_dir.exists() {
        fs::create_dir_all(&target_dir)
            .await
            .map_err(|e| format!("Failed to create destination directory: {e}"))?;
    }

    let name = archive_path.file_name().and_then(|n| n.to_str()).unwrap_or("");
    let is_zip = name.ends_with(".zip");
    let is_tar_gz = name.ends_with(".tar.gz") || name.ends_with(".tgz");
    let is_tar = name.ends_with(".tar");

    if !is_zip && !is_tar_gz && !is_tar {
        return Err("Unsupported archive format. Use .zip, .tar.gz, or .tar".into());
    }

    if is_zip {
        let mut cmd = safe_command("unzip");
        cmd.current_dir(&target_dir);
        if overwrite {
            cmd.arg("-o");
        } else {
            cmd.arg("-n");
        }
        cmd.arg(&archive_path);
        cmd.arg("-d");
        cmd.arg(&target_dir);

        let out = cmd.output().await.map_err(|e| format!("unzip failed: {e}"))?;
        // unzip returns 1 for minor warnings (e.g. skipped files), which is acceptable
        if !out.status.success() && out.status.code() != Some(1) {
            let err_msg = String::from_utf8_lossy(&out.stderr);
            return Err(format!("Extraction failed: {err_msg}"));
        }
    } else {
        let mut cmd = safe_command("tar");
        cmd.current_dir(&target_dir);
        if is_tar_gz {
            cmd.arg("xzf");
        } else {
            cmd.arg("xf");
        }
        cmd.arg(&archive_path);
        cmd.arg("-C");
        cmd.arg(&target_dir);
        if !overwrite {
            cmd.arg("--skip-old-files");
        }

        let out = cmd.output().await.map_err(|e| format!("tar failed: {e}"))?;
        if !out.status.success() {
            let err_msg = String::from_utf8_lossy(&out.stderr);
            return Err(format!("Extraction failed: {err_msg}"));
        }
    }

    Ok(ExtractResult {
        success: true,
        extracted_count: 0,
        destination: target_dir_rel.to_string(),
    })
}

/// Inspect archive contents without full extraction.
pub async fn inspect_archive(domain: &str, archive_rel: &str) -> Result<Vec<ArchiveEntry>, String> {
    let archive_path = resolve_safe_path(domain, archive_rel)?;
    if !archive_path.exists() {
        return Err("Archive does not exist".into());
    }

    let name = archive_path.file_name().and_then(|n| n.to_str()).unwrap_or("");
    let mut entries = Vec::new();

    if name.ends_with(".zip") {
        let mut cmd = safe_command("unzip");
        cmd.arg("-l");
        cmd.arg(&archive_path);
        let out = cmd.output().await.map_err(|e| format!("unzip inspection failed: {e}"))?;
        if out.status.success() {
            let text = String::from_utf8_lossy(&out.stdout);
            for line in text.lines().skip(3) {
                let parts: Vec<&str> = line.split_whitespace().collect();
                if parts.len() >= 4 && parts[0] != "Archive:" && parts[0] != "Length" && !parts[0].starts_with("---") {
                    if let Ok(size) = parts[0].parse::<u64>() {
                        let file_name = parts[3..].join(" ");
                        let is_dir = file_name.ends_with('/');
                        entries.push(ArchiveEntry {
                            name: file_name,
                            size,
                            is_dir,
                            modified: Some(format!("{} {}", parts[1], parts[2])),
                        });
                    }
                }
            }
        }
    } else if name.ends_with(".tar.gz") || name.ends_with(".tgz") || name.ends_with(".tar") {
        let mut cmd = safe_command("tar");
        if name.ends_with(".tar.gz") || name.ends_with(".tgz") {
            cmd.arg("-ztvf");
        } else {
            cmd.arg("-tvf");
        }
        cmd.arg(&archive_path);
        let out = cmd.output().await.map_err(|e| format!("tar inspection failed: {e}"))?;
        if out.status.success() {
            let text = String::from_utf8_lossy(&out.stdout);
            for line in text.lines() {
                let parts: Vec<&str> = line.split_whitespace().collect();
                if parts.len() >= 6 {
                    let size = parts[2].parse::<u64>().unwrap_or(0);
                    let file_name = parts[5..].join(" ");
                    let is_dir = parts[0].starts_with('d') || file_name.ends_with('/');
                    entries.push(ArchiveEntry {
                        name: file_name,
                        size,
                        is_dir,
                        modified: Some(format!("{} {}", parts[3], parts[4])),
                    });
                }
            }
        }
    }

    Ok(entries)
}

/// Search files by name or content inside a site directory.
pub async fn search_files(
    domain: &str,
    base_rel: &str,
    query: &str,
    in_content: bool,
    max_results: usize,
) -> Result<Vec<SearchResult>, String> {
    if query.trim().is_empty() {
        return Ok(Vec::new());
    }

    let site_root = PathBuf::from(format!("{WEBROOT}/{domain}"));
    let base_dir = resolve_safe_path(domain, base_rel)?;
    let query_lower = query.to_lowercase();
    let limit = max_results.clamp(1, 200);

    let mut results = Vec::new();
    let mut queue = vec![base_dir];

    while let Some(dir) = queue.pop() {
        if results.len() >= limit {
            break;
        }

        let mut reader = match fs::read_dir(&dir).await {
            Ok(r) => r,
            Err(_) => continue,
        };

        while let Ok(Some(entry)) = reader.next_entry().await {
            if results.len() >= limit {
                break;
            }

            let path = entry.path();
            let meta = match entry.metadata().await {
                Ok(m) => m,
                Err(_) => continue,
            };

            let is_dir = meta.is_dir();
            let name = entry.file_name().to_string_lossy().to_string();
            let rel_path = match path.strip_prefix(&site_root) {
                Ok(rel) => rel.to_string_lossy().to_string(),
                Err(_) => name.clone(),
            };

            if is_dir {
                queue.push(path.clone());
            }

            let name_matched = name.to_lowercase().contains(&query_lower);
            let mut line_matches = Vec::new();

            if in_content && !is_dir && meta.len() <= 2 * 1024 * 1024 {
                if let Ok(content) = fs::read_to_string(&path).await {
                    for (idx, line) in content.lines().enumerate() {
                        if line.to_lowercase().contains(&query_lower) {
                            line_matches.push(LineMatch {
                                line_number: idx + 1,
                                line_content: line.trim().chars().take(120).collect(),
                            });
                            if line_matches.len() >= 5 {
                                break;
                            }
                        }
                    }
                }
            }

            if name_matched || !line_matches.is_empty() {
                let modified = meta
                    .modified()
                    .ok()
                    .map(|t| {
                        let dt: chrono::DateTime<chrono::Utc> = t.into();
                        dt.format("%Y-%m-%d %H:%M:%S").to_string()
                    })
                    .unwrap_or_default();

                results.push(SearchResult {
                    name,
                    path: rel_path,
                    is_dir,
                    size: meta.len(),
                    modified,
                    line_matches,
                });
            }
        }
    }

    Ok(results)
}

/// Compute directory storage breakdown and total stats.
pub async fn directory_storage(domain: &str, rel_path: &str) -> Result<StorageStats, String> {
    let site_root = PathBuf::from(format!("{WEBROOT}/{domain}"));
    let target_dir = resolve_safe_path(domain, rel_path)?;

    let mut total_bytes: u64 = 0;
    let mut file_count: usize = 0;
    let mut dir_count: usize = 0;
    let mut breakdown = Vec::new();

    let mut reader = fs::read_dir(&target_dir)
        .await
        .map_err(|e| format!("Failed to read directory: {e}"))?;

    while let Some(entry) = reader.next_entry().await.ok().flatten() {
        let meta = match entry.metadata().await {
            Ok(m) => m,
            Err(_) => continue,
        };

        let is_dir = meta.is_dir();
        let name = entry.file_name().to_string_lossy().to_string();
        let rel = match entry.path().strip_prefix(&site_root) {
            Ok(p) => p.to_string_lossy().to_string(),
            Err(_) => name.clone(),
        };

        let item_size = if is_dir {
            dir_count += 1;
            compute_dir_size(&entry.path()).await
        } else {
            file_count += 1;
            meta.len()
        };

        total_bytes += item_size;
        breakdown.push(StorageItem {
            name,
            path: rel,
            is_dir,
            size: item_size,
        });
    }

    breakdown.sort_by(|a, b| b.size.cmp(&a.size));

    Ok(StorageStats {
        total_bytes,
        file_count,
        dir_count,
        breakdown,
    })
}

async fn compute_dir_size(path: &Path) -> u64 {
    let mut total = 0u64;
    let mut stack = vec![path.to_path_buf()];
    while let Some(current) = stack.pop() {
        if let Ok(mut reader) = fs::read_dir(&current).await {
            while let Ok(Some(entry)) = reader.next_entry().await {
                if let Ok(m) = entry.metadata().await {
                    if m.is_dir() {
                        stack.push(entry.path());
                    } else {
                        total += m.len();
                    }
                }
            }
        }
    }
    total
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;
    use std::os::unix::fs::symlink;

    fn tmpbase() -> PathBuf {
        let d = std::env::temp_dir().join(format!("dp-fmtest-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&d).unwrap();
        d.canonicalize().unwrap()
    }

    #[test]
    fn rejects_dangling_symlink_leaf() {
        let base = tmpbase();
        symlink("/nonexistent-dp-escape-target-xyz", base.join("evil")).unwrap();
        let r = resolve_within(&base, "evil");
        assert!(r.is_err(), "dangling symlink leaf must be rejected, got {r:?}");
        symlink("/etc/cron.d/dp-pwn-xyz", base.join("evil2")).unwrap();
        assert!(resolve_within(&base, "evil2").is_err());
        std::fs::create_dir_all(base.join("pub")).unwrap();
        symlink("/etc/dp-passwd-xyz", base.join("pub/link")).unwrap();
        assert!(resolve_within(&base, "pub/link").is_err());
        std::fs::remove_dir_all(&base).ok();
    }

    #[test]
    fn rejects_live_symlink_escaping_root() {
        let base = tmpbase();
        symlink("/etc", base.join("etclink")).unwrap();
        assert!(resolve_within(&base, "etclink").is_err());
        assert!(resolve_within(&base, "etclink/passwd").is_err());
        std::fs::remove_dir_all(&base).ok();
    }

    #[test]
    fn rejects_dotdot_traversal() {
        let base = tmpbase();
        assert!(resolve_within(&base, "../../etc/passwd").is_err());
        assert!(resolve_within(&base, "a/../../../etc/passwd").is_err());
        std::fs::remove_dir_all(&base).ok();
    }

    #[test]
    fn allows_new_files() {
        let base = tmpbase();
        assert!(resolve_within(&base, "newfile.txt").unwrap().starts_with(&base));
        std::fs::create_dir_all(base.join("wp-content")).unwrap();
        let r = resolve_within(&base, "wp-content/x.php").unwrap();
        assert!(r.starts_with(&base) && r.ends_with("wp-content/x.php"));
        std::fs::remove_dir_all(&base).ok();
    }

    #[test]
    fn allows_create_through_in_root_symlink_dir() {
        let base = tmpbase();
        std::fs::create_dir_all(base.join("releases/v1")).unwrap();
        symlink(base.join("releases/v1"), base.join("current")).unwrap();
        let r = resolve_within(&base, "current/new.txt").unwrap();
        assert!(r.starts_with(&base), "in-root symlink-dir create must be allowed, got {r:?}");
        std::fs::remove_dir_all(&base).ok();
    }

    #[tokio::test]
    async fn write_file_handles_long_basename() {
        let base = tmpbase();
        let target = base.join("a".repeat(230));
        write_file(&target, "hello").await.expect("save of a long-named file must succeed");
        assert_eq!(std::fs::read_to_string(&target).unwrap(), "hello");
        std::fs::remove_dir_all(&base).ok();
    }

    #[tokio::test]
    async fn copy_and_duplicate_entries() {
        let base = tmpbase();
        let src = base.join("hello.txt");
        std::fs::write(&src, "test content").unwrap();

        let dst = base.join("copied.txt");
        copy_entry(&src, &dst).await.unwrap();
        assert_eq!(std::fs::read_to_string(&dst).unwrap(), "test content");

        let dup_name = duplicate_entry(&src).await.unwrap();
        assert!(base.join(&dup_name).exists());
        assert_eq!(std::fs::read_to_string(base.join(&dup_name)).unwrap(), "test content");

        std::fs::remove_dir_all(&base).ok();
    }
}
