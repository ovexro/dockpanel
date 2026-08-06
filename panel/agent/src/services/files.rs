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
    resolve_within(&base, relative_path)
}

/// Resolve a path that must name something INSIDE the site root — never the root itself.
///
/// `resolve_within` deliberately permits the site root, because listing it is the file
/// manager's default view (`?path=/`). Containment and identity are different questions,
/// though, and only the containment one was being asked: `.`, `""` and `/` all canonicalise
/// to the webroot, and a path always starts with itself, so the traversal check passes them.
/// A destructive verb handed that path is a request to erase the entire site in one call —
/// `remove_dir_all` on the webroot takes the site with it, and the caller sees `success`.
///
/// So the read verbs keep using the permissive resolver and the destructive ones use this,
/// which refuses the root and lets everything below it through unchanged.
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
/// sits in the to-be-created portion of the path — otherwise a create/upload sink
/// (which opens O_CREAT and follows the final symlink) could write OUTSIDE the site
/// root as root. A LIVE symlink escaping the root is caught by canonicalize + the
/// `starts_with` check; a DANGLING symlink is invisible to `exists()`/canonicalize
/// (stat follows it, sees the absent target) but visible to lstat, so it is rejected
/// explicitly here.
pub(crate) fn resolve_within(base: &Path, relative_path: &str) -> Result<PathBuf, String> {
    // Normalize: strip leading slashes (traversal via ".." is rejected below).
    let cleaned = relative_path.trim_start_matches('/');
    let candidate = base.join(cleaned);

    // Canonicalize base (must exist).
    let canon_base = base
        .canonicalize()
        .map_err(|_| format!("Site root does not exist: {}", base.display()))?;

    let canon = if candidate.exists() {
        // Existing target: canonicalize fully resolves symlinks, and the
        // starts_with check below then guarantees the resolved target is in-root.
        candidate
            .canonicalize()
            .map_err(|e| format!("Path error: {e}"))?
    } else {
        // Target does not exist yet (create/write/upload). Walk up to the first
        // existing ancestor.
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
        // `exists()` follows symlinks, so a DANGLING symlink read as "absent" and
        // became a trailing component. Re-check each trailing component on the REAL
        // filesystem path with lstat and refuse ANY symlink — a create/upload sink
        // would otherwise follow it out of the site root (sandbox escape).
        let mut real = existing.clone();
        for component in trail.iter().rev() {
            real = real.join(component);
            if let Ok(meta) = std::fs::symlink_metadata(&real) {
                if meta.file_type().is_symlink() {
                    return Err("Path traversal denied (symlink in target path)".into());
                }
            }
        }
        // Build the returned path on the CANONICAL ancestor so starts_with is
        // symlink-free.
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

// `ensure_site_root` used to live here, and `list` called it before resolving. It was the
// only silent failure in the file manager: asked for a domain this host does not serve, it
// created `/var/www/<domain>` as root and answered 200 with an empty listing, which then
// unblocked write/create/upload into a directory no vhost serves. Every other verb already
// fails loudly ("Site root does not exist"), and site provisioning creates the webroot, so
// nothing legitimate depended on creating it here. Removing it turns that silence into an
// error. Note it also means a site whose document root was pointed somewhere other than
// `/var/www/<domain>` now reports that plainly instead of showing an empty invented folder.

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

        // Return paths relative to the site root to avoid leaking server paths.
        // `path` is already canonical, so canonicalize the root too (in case
        // /var/www is itself a symlink) and strip that. Fail CLOSED to the bare
        // name — never the absolute server path — if the strip misses.
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
    // Write atomically via a collision-free temp sibling in the same (validated)
    // directory, then rename over the target. `with_extension("tmp")` would REPLACE
    // the extension and could clobber a distinct real sibling (saving report.php
    // would destroy an existing report.tmp). The UUID alone guarantees uniqueness, so
    // keep the temp name a FIXED short length — embedding the real filename could push
    // the component past NAME_MAX (255) for a long-but-valid basename -> ENAMETOOLONG
    // on save. Clean up the temp on any failure so a failed write can't leak orphans.
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

#[cfg(all(test, unix))]
mod tests {
    use super::*;
    use std::os::unix::fs::symlink;

    fn tmpbase() -> PathBuf {
        // std::env::temp_dir() is /tmp on Linux (canonical). Canonicalize anyway so
        // the assertions hold regardless of the host layout.
        let d = std::env::temp_dir().join(format!("dp-fmtest-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&d).unwrap();
        d.canonicalize().unwrap()
    }

    // The sandbox-escape primitive: a DANGLING symlink leaf (target absent) must be
    // rejected — otherwise create/upload would fs::write THROUGH it as root.
    #[test]
    fn rejects_dangling_symlink_leaf() {
        let base = tmpbase();
        symlink("/nonexistent-dp-escape-target-xyz", base.join("evil")).unwrap();
        let r = resolve_within(&base, "evil");
        assert!(r.is_err(), "dangling symlink leaf must be rejected, got {r:?}");
        // Also when the escape target is a real-but-privileged path that does not yet
        // exist as a file (the /etc/cron.d root-RCE vector).
        symlink("/etc/cron.d/dp-pwn-xyz", base.join("evil2")).unwrap();
        assert!(resolve_within(&base, "evil2").is_err());
        // And when reached through a real subdir.
        std::fs::create_dir_all(base.join("pub")).unwrap();
        symlink("/etc/dp-passwd-xyz", base.join("pub/link")).unwrap();
        assert!(resolve_within(&base, "pub/link").is_err());
        std::fs::remove_dir_all(&base).ok();
    }

    // A LIVE symlink escaping the root (target exists) stays rejected via canonicalize.
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

    // Normal creates must still work.
    #[test]
    fn allows_new_files() {
        let base = tmpbase();
        assert!(resolve_within(&base, "newfile.txt").unwrap().starts_with(&base));
        std::fs::create_dir_all(base.join("wp-content")).unwrap();
        let r = resolve_within(&base, "wp-content/x.php").unwrap();
        assert!(r.starts_with(&base) && r.ends_with("wp-content/x.php"));
        std::fs::remove_dir_all(&base).ok();
    }

    // A legit IN-ROOT symlink directory (e.g. `current -> releases/v1`) must still
    // allow creating files beneath it — the escape fix must not break this.
    #[test]
    fn allows_create_through_in_root_symlink_dir() {
        let base = tmpbase();
        std::fs::create_dir_all(base.join("releases/v1")).unwrap();
        symlink(base.join("releases/v1"), base.join("current")).unwrap();
        let r = resolve_within(&base, "current/new.txt").unwrap();
        assert!(r.starts_with(&base), "in-root symlink-dir create must be allowed, got {r:?}");
        std::fs::remove_dir_all(&base).ok();
    }

    // Regression: the atomic-write temp name must not overflow NAME_MAX for a
    // long-but-legal basename (the collision-free-temp fix must not embed the name).
    #[tokio::test]
    async fn write_file_handles_long_basename() {
        let base = tmpbase();
        let target = base.join("a".repeat(230)); // valid (<=255), long enough to overflow if embedded
        write_file(&target, "hello").await.expect("save of a long-named file must succeed");
        assert_eq!(std::fs::read_to_string(&target).unwrap(), "hello");
        std::fs::remove_dir_all(&base).ok();
    }
}
