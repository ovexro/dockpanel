use std::path::{Path, PathBuf};
use tokio::fs;

const WEBROOT: &str = "/var/www";

#[derive(serde::Serialize)]
pub struct FileEntry {
    pub name: String,
    pub path: String,
    pub is_dir: bool,
    pub size: u64,
    pub modified: String,
}

#[derive(serde::Serialize)]
pub struct FileContent {
    pub content: String,
    pub size: u64,
    pub modified: String,
}

/// Resolve a user-provided path to a safe absolute path within /var/www/{domain}/.
/// Prevents path traversal attacks.
pub fn resolve_safe_path(domain: &str, relative_path: &str) -> Result<PathBuf, String> {
    let base = PathBuf::from(format!("{WEBROOT}/{domain}"));

    // Normalize: strip leading slashes, reject obvious traversal
    let cleaned = relative_path.trim_start_matches('/');

    let candidate = base.join(cleaned);

    // Canonicalize base (must exist)
    let canon_base = base
        .canonicalize()
        .map_err(|_| format!("Site root does not exist: {}", base.display()))?;

    // For the candidate, canonicalize the parent (it must exist) and append the filename
    // This handles cases where the file itself doesn't exist yet (create/write)
    let canon = if candidate.exists() {
        candidate
            .canonicalize()
            .map_err(|e| format!("Path error: {e}"))?
    } else {
        // Walk up to find the first existing ancestor, then append the remainder
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

/// Ensure site root directory exists.
pub fn ensure_site_root(domain: &str) -> Result<PathBuf, String> {
    let root = PathBuf::from(format!("{WEBROOT}/{domain}"));
    std::fs::create_dir_all(&root)
        .map_err(|e| format!("Failed to create site root: {e}"))?;
    Ok(root)
}

/// List directory contents.
/// The `site_root` parameter is used to strip absolute paths to relative paths in the response.
/// If `None`, paths are returned relative to the listed directory.
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

        let name = entry.file_name().to_string_lossy().to_string();
        let abs_path = format!("{}/{}", path.display(), &name);

        // Return paths relative to the site root to avoid leaking server paths
        let relative_path = if let Some(root) = site_root {
            let root_str = format!("{}/", root.display());
            abs_path
                .strip_prefix(&root_str)
                .unwrap_or(&abs_path)
                .to_string()
        } else {
            name.clone()
        };

        entries.push(FileEntry {
            path: relative_path,
            name,
            is_dir,
            size,
            modified,
        });
    }

    // Sort: directories first, then alphabetical
    entries.sort_by(|a, b| b.is_dir.cmp(&a.is_dir).then(a.name.cmp(&b.name)));
    Ok(entries)
}

/// Read file content (text only, max 2MB).
pub async fn read_file(path: &Path) -> Result<FileContent, String> {
    let meta = fs::metadata(path)
        .await
        .map_err(|e| format!("File not found: {e}"))?;

    if meta.len() > 2 * 1024 * 1024 {
        return Err("File too large (max 2MB)".into());
    }

    let content = fs::read_to_string(path)
        .await
        .map_err(|_| "File is binary or not readable as text".to_string())?;

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
    })
}

/// Write file content. Creates parent directories if needed.
pub async fn write_file(path: &Path, content: &str) -> Result<(), String> {
    // Ensure parent directory exists
    if let Some(parent) = path.parent() {
        if !parent.exists() {
            fs::create_dir_all(parent)
                .await
                .map_err(|e| format!("Failed to create directory: {e}"))?;
        }
    }
    // Write atomically via temp file
    let tmp = path.with_extension("tmp");
    fs::write(&tmp, content)
        .await
        .map_err(|e| format!("Failed to write: {e}"))?;
    fs::rename(&tmp, path)
        .await
        .map_err(|e| format!("Failed to finalize write: {e}"))?;
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
        // Ensure parent exists
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).await.ok();
        }
        fs::write(path, "")
            .await
            .map_err(|e| format!("Failed to create file: {e}"))?;
    }
    Ok(())
}

/// Rename/move an entry.
pub async fn rename_entry(from: &Path, to: &Path) -> Result<(), String> {
    if !from.exists() {
        return Err("Source does not exist".into());
    }
    if to.exists() {
        return Err("Destination already exists".into());
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
