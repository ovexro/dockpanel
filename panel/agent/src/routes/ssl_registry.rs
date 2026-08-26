//! The named certificate registry (#104).
//!
//! A certificate the operator supplies is registered ONCE under an alias and
//! referenced by that alias from every stack that claims a domain under it —
//! never by filesystem path, and never as a per-domain upload that has to be
//! repeated for every name a wildcard covers. The PEM pair lives only here, on
//! the agent's disk, in its own root beside the per-domain tree; the panel
//! keeps metadata and the alias. See [`crate::services::ssl::SSL_REGISTRY_DIR`]
//! for why the root is a sibling and not a subdirectory.
//!
//! Every refusal is a 4xx carrying a sentence, because the panel passes a 4xx
//! through to the operator's screen untouched and turns a 5xx into an incident
//! id. And NOTHING is written before every check has passed: a refused upload
//! leaves the registry byte-identical.

use axum::{
    extract::Path,
    http::StatusCode,
    routing::{delete, post},
    Json, Router,
};
use serde::Deserialize;

use super::{is_valid_domain, AppState};
use crate::services::{nginx, ownership, ssl};

type Refusal = (StatusCode, Json<serde_json::Value>);

fn refuse(status: StatusCode, sentence: impl Into<String>) -> Refusal {
    (status, Json(serde_json::json!({ "error": sentence.into() })))
}

#[derive(Deserialize)]
struct RegisterRequest {
    alias: String,
    certificate: String,
    private_key: String,
    /// Overwrite a pair already registered under this alias. Off by default so
    /// two operators registering the same name do not silently race.
    #[serde(default)]
    replace: bool,
    /// Domains the new pair must still cover — on a replace, every domain a
    /// stack already claims through this alias. A replacement that drops one
    /// of them would take that site's TLS down at the next reload, so it is
    /// refused up front with the domain named.
    #[serde(default)]
    must_cover: Vec<String>,
}

#[derive(Deserialize)]
struct CoversRequest {
    domain: String,
}

/// The directory a registered alias occupies, and the scratch directory a write
/// is staged in beside it. Staged under a dot-name inside the same root so the
/// final `rename` is within one filesystem and therefore atomic; suffixed with
/// a fresh nonce PER REQUEST, because every request in this one process shares
/// a pid, and two operators replacing the same alias at once must not stage —
/// or park the incumbent — in one directory.
fn alias_dirs(alias: &str, tag: &str) -> (String, String) {
    (
        format!("{}/{alias}", ssl::SSL_REGISTRY_DIR),
        format!("{}/.{alias}.{tag}-{}", ssl::SSL_REGISTRY_DIR, uuid::Uuid::new_v4().simple()),
    )
}

/// Remove what an earlier request left behind for this alias: a staging or
/// aside directory that outlived a crash between its rename and its cleanup.
/// Each holds a full key pair, and nothing else ever enumerates the root.
/// Only entries older than a request could plausibly still be running are
/// touched, so a concurrent register or replace of the same alias keeps its own.
async fn sweep_leftovers(alias: &str) {
    let Ok(mut entries) = tokio::fs::read_dir(ssl::SSL_REGISTRY_DIR).await else {
        return;
    };
    let tmp_prefix = format!(".{alias}.tmp-");
    let prev_prefix = format!(".{alias}.prev-");
    let stale = std::time::Duration::from_secs(600);
    while let Ok(Some(entry)) = entries.next_entry().await {
        let name = entry.file_name().to_string_lossy().to_string();
        if !(name.starts_with(&tmp_prefix) || name.starts_with(&prev_prefix)) {
            continue;
        }
        let old_enough = entry
            .metadata()
            .await
            .ok()
            .and_then(|m| m.modified().ok())
            .and_then(|t| t.elapsed().ok())
            .map(|age| age > stale)
            .unwrap_or(false);
        if old_enough {
            tracing::warn!("SSL registry: removing leftover {name} from an earlier request");
            tokio::fs::remove_dir_all(entry.path()).await.ok();
        }
    }
}

/// POST /ssl/registry — Register a certificate under an alias.
async fn register(
    Json(body): Json<RegisterRequest>,
) -> Result<(StatusCode, Json<serde_json::Value>), Refusal> {
    let alias = body.alias.as_str();
    if !ssl::is_valid_cert_alias(alias) {
        return Err(refuse(
            StatusCode::BAD_REQUEST,
            "the alias must be 1 to 64 lowercase letters, digits or hyphens, starting and ending \
             with a letter or digit — a DNS label shape, like wildcard-2026",
        ));
    }
    if body.certificate.trim().is_empty() || body.private_key.trim().is_empty() {
        return Err(refuse(
            StatusCode::BAD_REQUEST,
            "both the certificate and the private key are required",
        ));
    }
    if !body.private_key.contains("BEGIN") {
        return Err(refuse(
            StatusCode::BAD_REQUEST,
            "the private key is not PEM — it should start with a BEGIN line",
        ));
    }

    // What the certificate says about itself, and whether the key is its own.
    // Both are answered before anything touches the disk; the second is the
    // check nginx would otherwise make at the first reload after a claim.
    let meta = ssl::cert_metadata(&body.certificate)
        .map_err(|reason| refuse(StatusCode::BAD_REQUEST, reason))?;
    ssl::key_matches_cert(&body.certificate, &body.private_key)
        .map_err(|reason| refuse(StatusCode::BAD_REQUEST, reason))?;
    for domain in &body.must_cover {
        if let Err(reason) = ssl::cert_covers_domain(&body.certificate, domain) {
            return Err(refuse(
                StatusCode::BAD_REQUEST,
                format!("{domain} is served under this alias and the new certificate does not cover it: {reason}"),
            ));
        }
    }

    let (final_dir, tmp_dir) = alias_dirs(alias, "tmp");
    let exists = tokio::fs::metadata(&final_dir).await.is_ok();
    if exists && !body.replace {
        return Err(refuse(
            StatusCode::CONFLICT,
            format!("a certificate named {alias} is already registered on this server"),
        ));
    }

    // Every check has passed. Stage the pair beside its destination and rename
    // it into place, so no reader ever sees a directory holding one file of two.
    sweep_leftovers(alias).await;
    let (cert_path, key_path) = ssl::registry_paths(alias);
    if let Err(e) = stage_pair(&tmp_dir, &body.certificate, &body.private_key).await {
        tokio::fs::remove_dir_all(&tmp_dir).await.ok();
        return Err(refuse(StatusCode::INTERNAL_SERVER_ERROR, e));
    }

    // On a replace the incumbent is moved aside rather than deleted, so a
    // reload that refuses the new pair can put it back. A directory cannot be
    // renamed over a non-empty one, which is why the swap takes two steps.
    let (_, aside_dir) = alias_dirs(alias, "prev");
    let mut previous_kept = false;
    if exists {
        tokio::fs::remove_dir_all(&aside_dir).await.ok();
        if let Err(e) = tokio::fs::rename(&final_dir, &aside_dir).await {
            tokio::fs::remove_dir_all(&tmp_dir).await.ok();
            return Err(refuse(
                StatusCode::INTERNAL_SERVER_ERROR,
                format!("could not move the previous certificate aside: {e}"),
            ));
        }
        previous_kept = true;
    }
    if let Err(e) = tokio::fs::rename(&tmp_dir, &final_dir).await {
        tokio::fs::remove_dir_all(&tmp_dir).await.ok();
        if previous_kept {
            tokio::fs::rename(&aside_dir, &final_dir).await.ok();
        }
        return Err(refuse(
            StatusCode::INTERNAL_SERVER_ERROR,
            format!("could not install the certificate: {e}"),
        ));
    }

    let mut response = serde_json::json!({
        "ok": true,
        "alias": alias,
        "dns_names": meta.dns_names,
        "issuer": meta.issuer,
        "not_before": meta.not_before.to_rfc3339(),
        "not_after": meta.not_after.to_rfc3339(),
        "fingerprint_sha256": meta.fingerprint_sha256,
        "cert_path": cert_path,
        "key_path": key_path,
    });

    // A replaced pair that a live vhost already names has to reach nginx, or
    // the site keeps serving the old certificate from memory until something
    // unrelated reloads it — and if the new pair is one nginx refuses, that
    // unrelated reload is where the outage would surface. Test now, and put
    // the previous pair back if the answer is no.
    if previous_kept && ownership::registry_cert_in_use(alias) {
        let verdict = match nginx::test_config().await {
            Ok(output) if output.success => Ok(()),
            Ok(output) => Err(output.stderr),
            Err(e) => Err(e.to_string()),
        };
        if let Err(stderr) = verdict {
            let restored = tokio::fs::remove_dir_all(&final_dir).await.is_ok()
                && tokio::fs::rename(&aside_dir, &final_dir).await.is_ok();
            tracing::warn!("SSL registry: nginx refused the replaced pair for {alias}: {stderr}");
            return Err(refuse(
                StatusCode::BAD_GATEWAY,
                format!(
                    "nginx refused the configuration with the replaced certificate: {}{}",
                    stderr.trim(),
                    if restored {
                        " (the previous certificate was put back)"
                    } else {
                        " (and the previous certificate could NOT be put back — check the registry directory)"
                    }
                ),
            ));
        }
        if let Err(e) = nginx::reload().await {
            tracing::warn!("SSL registry: nginx reload failed after replacing {alias}: {e}");
            response["warning"] = serde_json::json!(format!(
                "the certificate was replaced but nginx did not reload: {e}"
            ));
        } else {
            response["reloaded"] = serde_json::json!(true);
        }
    }
    if previous_kept {
        tokio::fs::remove_dir_all(&aside_dir).await.ok();
    }

    tracing::info!(
        "SSL registry: registered {alias} for {} (expires {})",
        response["dns_names"],
        response["not_after"]
    );
    Ok((StatusCode::CREATED, Json(response)))
}

/// Write the pair into a fresh staging directory. The key goes through the
/// 0600-at-creation writer every other key on this box uses; the registry root
/// itself is created owner-only, like the per-domain tree beside it.
async fn stage_pair(tmp_dir: &str, certificate: &str, private_key: &str) -> Result<(), String> {
    tokio::fs::create_dir_all(ssl::SSL_REGISTRY_DIR)
        .await
        .map_err(|e| format!("could not create the registry directory: {e}"))?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        tokio::fs::set_permissions(ssl::SSL_REGISTRY_DIR, std::fs::Permissions::from_mode(0o700))
            .await
            .ok();
    }
    tokio::fs::remove_dir_all(tmp_dir).await.ok();
    tokio::fs::create_dir_all(tmp_dir)
        .await
        .map_err(|e| format!("could not stage the certificate: {e}"))?;
    tokio::fs::write(format!("{tmp_dir}/fullchain.pem"), certificate)
        .await
        .map_err(|e| format!("could not write the certificate: {e}"))?;
    ssl::write_key_file(&format!("{tmp_dir}/privkey.pem"), private_key).await
}

/// POST /ssl/registry/{alias}/covers — Does the registered certificate name
/// this domain? Binding point 3 of #104: asked by the panel at claim time,
/// before any row is written, and again by the deploy path itself.
async fn covers(
    Path(alias): Path<String>,
    Json(body): Json<CoversRequest>,
) -> Result<Json<serde_json::Value>, Refusal> {
    if !ssl::is_valid_cert_alias(&alias) {
        return Err(refuse(StatusCode::BAD_REQUEST, "Invalid certificate alias"));
    }
    if !is_valid_domain(&body.domain) {
        return Err(refuse(StatusCode::BAD_REQUEST, "Invalid domain format"));
    }
    let (cert_path, _) = ssl::registry_paths(&alias);
    let pem = match tokio::fs::read_to_string(&cert_path).await {
        Ok(pem) => pem,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            return Err(refuse(
                StatusCode::NOT_FOUND,
                format!("no certificate named {alias} is registered on this server"),
            ));
        }
        Err(e) => {
            return Err(refuse(
                StatusCode::INTERNAL_SERVER_ERROR,
                format!("could not read the registered certificate: {e}"),
            ));
        }
    };
    let names = ssl::cert_covers_domain(&pem, &body.domain)
        .map_err(|reason| refuse(StatusCode::BAD_REQUEST, reason))?;
    Ok(Json(serde_json::json!({ "ok": true, "dns_names": names })))
}

/// DELETE /ssl/registry/{alias} — Remove a registered certificate, unless a
/// vhost still names it.
async fn remove(Path(alias): Path<String>) -> Result<Json<serde_json::Value>, Refusal> {
    if !ssl::is_valid_cert_alias(&alias) {
        return Err(refuse(StatusCode::BAD_REQUEST, "Invalid certificate alias"));
    }
    let (final_dir, _) = alias_dirs(&alias, "tmp");
    match tokio::fs::metadata(&final_dir).await {
        Ok(_) => {}
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            return Err(refuse(
                StatusCode::NOT_FOUND,
                format!("no certificate named {alias} is registered on this server"),
            ));
        }
        // Anything else is not "absent": answering 404 here would have the
        // panel drop its row while the pair stays on the disk.
        Err(e) => {
            return Err(refuse(
                StatusCode::INTERNAL_SERVER_ERROR,
                format!("could not read the registered certificate: {e}"),
            ));
        }
    }
    // The same guard the per-domain teardown honours for a shared wildcard: a
    // vhost pointing at a missing pair passes unnoticed until the next
    // `nginx -t`, and then takes every site on the box down with it.
    match ownership::registry_cert_references(&alias) {
        Ok(refs) if refs.is_empty() => {}
        Ok(refs) => {
            return Err(refuse(
                StatusCode::CONFLICT,
                format!(
                    "the certificate named {alias} is still referenced by {} vhost file(s) on \
                     this server ({}); move those domains to another certificate, or remove \
                     the parked configuration, first",
                    refs.len(),
                    refs.join(", ")
                ),
            ));
        }
        Err(reason) => {
            return Err(refuse(
                StatusCode::CONFLICT,
                format!("cannot tell whether a vhost still names {alias}: {reason}"),
            ));
        }
    }
    sweep_leftovers(&alias).await;
    tokio::fs::remove_dir_all(&final_dir).await.map_err(|e| {
        refuse(
            StatusCode::INTERNAL_SERVER_ERROR,
            format!("could not remove the registered certificate: {e}"),
        )
    })?;
    tracing::info!("SSL registry: removed {alias}");
    Ok(Json(serde_json::json!({ "ok": true })))
}

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/ssl/registry", post(register))
        .route("/ssl/registry/{alias}/covers", post(covers))
        .route("/ssl/registry/{alias}", delete(remove))
}

#[cfg(test)]
mod tests {
    /// The registry shares the `/ssl` prefix with the per-domain router, whose
    /// `{domain}` segment sits where `registry` does. axum refuses an ambiguous
    /// pair at merge time — by panicking in `main`, which no test reaches. So
    /// the merge is performed here, where a conflict fails a test instead of
    /// the daemon's start-up.
    #[test]
    fn the_registry_routes_merge_beside_the_per_domain_ssl_routes() {
        let _ = crate::routes::ssl::router().merge(super::router());
    }
}
