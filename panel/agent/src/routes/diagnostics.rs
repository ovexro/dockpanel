use axum::{
    http::StatusCode,
    routing::{get, post},
    Json, Router,
};
use serde::Deserialize;

use super::AppState;
use crate::services::diagnostics;

type ApiErr = (StatusCode, Json<serde_json::Value>);

fn err(status: StatusCode, msg: &str) -> ApiErr {
    (status, Json(serde_json::json!({ "error": msg })))
}

/// GET /diagnostics — Run all diagnostic checks.
async fn run_diagnostics() -> Json<diagnostics::DiagnosticReport> {
    Json(diagnostics::run_diagnostics().await)
}

#[derive(Deserialize)]
struct FixRequest {
    fix_id: String,
}

/// POST /diagnostics/fix — Apply a one-click fix.
async fn apply_fix(
    Json(body): Json<FixRequest>,
) -> Result<Json<serde_json::Value>, ApiErr> {
    // Validate fix_id format (action:target)
    if body.fix_id.is_empty() || body.fix_id.len() > 256 {
        return Err(err(StatusCode::BAD_REQUEST, "Invalid fix_id"));
    }

    diagnostics::apply_fix(&body.fix_id)
        .await
        .map(|msg| Json(serde_json::json!({ "success": true, "message": msg })))
        .map_err(|e| err(StatusCode::INTERNAL_SERVER_ERROR, &e))
}

/// GET /diagnostics/recommendations — Auto-optimization recommendations.
async fn recommendations() -> Json<serde_json::Value> {
    use crate::safe_cmd::safe_command;

    let mut recs: Vec<serde_json::Value> = Vec::new();

    // 1. System memory analysis
    let mem_output = tokio::time::timeout(
        std::time::Duration::from_secs(5),
        safe_command("free").args(["-m"]).output()
    ).await;

    let (total_mem_mb, _used_mem_mb, avail_mem_mb) = mem_output.ok()
        .and_then(|r| r.ok())
        .map(|o| {
            let s = String::from_utf8_lossy(&o.stdout);
            let parts: Vec<u64> = s.lines().nth(1).unwrap_or("")
                .split_whitespace().skip(1).filter_map(|v| v.parse().ok()).collect();
            (parts.first().copied().unwrap_or(0), parts.get(1).copied().unwrap_or(0), parts.get(5).copied().unwrap_or(0))
        })
        .unwrap_or((0, 0, 0));

    // 2. CPU count
    let cpus: u32 = tokio::time::timeout(
        std::time::Duration::from_secs(3),
        safe_command("nproc").output()
    ).await.ok()
        .and_then(|r| r.ok())
        .and_then(|o| String::from_utf8_lossy(&o.stdout).trim().parse().ok())
        .unwrap_or(1);

    // 3. PHP-FPM pool analysis
    let pools_dir = "/etc/php";
    if let Ok(versions) = std::fs::read_dir(pools_dir) {
        for ver_entry in versions.flatten() {
            let pool_dir = ver_entry.path().join("fpm/pool.d");
            if !pool_dir.exists() { continue; }

            if let Ok(pools) = std::fs::read_dir(&pool_dir) {
                let mut total_max_children: u32 = 0;
                let mut total_memory_per_worker: u32 = 0;
                let mut pool_count: u32 = 0;

                for pool_entry in pools.flatten() {
                    if let Ok(content) = std::fs::read_to_string(pool_entry.path()) {
                        let max_children: u32 = content.lines()
                            .find(|l| l.starts_with("pm.max_children"))
                            .and_then(|l| l.split('=').nth(1))
                            .and_then(|v| v.trim().parse().ok())
                            .unwrap_or(5);
                        let memory: u32 = content.lines()
                            .find(|l| l.contains("memory_limit"))
                            .and_then(|l| l.split('=').nth(1))
                            .and_then(|v| v.trim().trim_end_matches('M').parse().ok())
                            .unwrap_or(256);

                        total_max_children += max_children;
                        total_memory_per_worker += memory;
                        pool_count += 1;
                    }
                }

                if pool_count > 0 {
                    let avg_memory = total_memory_per_worker / pool_count;
                    let max_possible_mb = total_max_children as u64 * avg_memory as u64;

                    // Check if PHP workers could OOM the system
                    if max_possible_mb > total_mem_mb * 80 / 100 {
                        let safe_workers = (total_mem_mb * 60 / 100) / avg_memory as u64;
                        recs.push(serde_json::json!({
                            "category": "php",
                            "severity": "warning",
                            "title": "PHP workers may exhaust memory",
                            "description": format!(
                                "Total max PHP workers ({total_max_children}) × {avg_memory}MB = {max_possible_mb}MB, \
                                 but system only has {total_mem_mb}MB RAM. Reduce to ~{safe_workers} total workers or lower memory_limit."
                            ),
                            "current": format!("{total_max_children} workers × {avg_memory}MB"),
                            "recommended": format!("{safe_workers} workers × {avg_memory}MB"),
                        }));
                    }

                    // Suggest OPcache if not configured
                    if pool_count > 0 {
                        recs.push(serde_json::json!({
                            "category": "php",
                            "severity": "info",
                            "title": "Ensure OPcache is enabled",
                            "description": "OPcache caches compiled PHP bytecode, reducing CPU usage by 50-70% for repeat requests.",
                            "current": "Check with phpinfo()",
                            "recommended": "opcache.enable=1, opcache.memory=128MB",
                        }));
                    }
                }
            }
        }
    }

    // 4. Nginx worker analysis
    let nginx_conf = std::fs::read_to_string("/etc/nginx/nginx.conf").unwrap_or_default();
    let nginx_workers: u32 = nginx_conf.lines()
        .find(|l| l.contains("worker_processes") && !l.trim_start().starts_with('#'))
        .and_then(|l| l.split_whitespace().nth(1))
        .and_then(|v| v.trim_end_matches(';').parse().ok())
        .unwrap_or(0);

    if nginx_workers > 0 && nginx_workers != cpus {
        recs.push(serde_json::json!({
            "category": "nginx",
            "severity": if nginx_workers < cpus { "warning" } else { "info" },
            "title": "Nginx worker count doesn't match CPU cores",
            "description": format!("nginx has {nginx_workers} workers but system has {cpus} CPUs. Set to 'auto' or {cpus}."),
            "current": format!("{nginx_workers} workers"),
            "recommended": format!("{cpus} workers (or 'auto')"),
        }));
    }

    // 5. Swap recommendation
    if avail_mem_mb < total_mem_mb / 4 && total_mem_mb > 0 {
        recs.push(serde_json::json!({
            "category": "system",
            "severity": "warning",
            "title": "Low available memory",
            "description": format!("Only {avail_mem_mb}MB available out of {total_mem_mb}MB. Consider adding swap or reducing services."),
            "current": format!("{avail_mem_mb}MB available"),
            "recommended": "Ensure at least 25% memory is free",
        }));
    }

    // 6. Gzip recommendation (check if already enabled)
    if !nginx_conf.contains("gzip on") {
        recs.push(serde_json::json!({
            "category": "nginx",
            "severity": "warning",
            "title": "Gzip compression not enabled globally",
            "description": "Enable gzip in nginx.conf for 60-80% bandwidth savings on text assets.",
            "current": "gzip off",
            "recommended": "gzip on; gzip_types text/plain text/css application/json application/javascript;",
        }));
    }

    // 7. Disk usage recommendations
    let df = tokio::time::timeout(
        std::time::Duration::from_secs(5),
        safe_command("df").args(["--output=pcent", "/"]).output()
    ).await;
    if let Some(pct) = df.ok().and_then(|r| r.ok()).and_then(|o| {
        String::from_utf8_lossy(&o.stdout).lines().nth(1)
            .and_then(|l| l.trim().trim_end_matches('%').parse::<u32>().ok())
    }) {
        if pct > 80 {
            recs.push(serde_json::json!({
                "category": "system",
                "severity": if pct > 90 { "critical" } else { "warning" },
                "title": "Disk usage is high",
                "description": format!("Root partition is {pct}% full. Clean up old backups, logs, or Docker images."),
                "current": format!("{pct}% used"),
                "recommended": "Keep below 80%",
            }));
        }
    }

    Json(serde_json::json!({
        "recommendations": recs,
        "total": recs.len(),
        "system": {
            "total_memory_mb": total_mem_mb,
            "available_memory_mb": avail_mem_mb,
            "cpus": cpus,
        },
    }))
}

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/diagnostics", get(run_diagnostics))
        .route("/diagnostics/fix", post(apply_fix))
        .route("/diagnostics/recommendations", get(recommendations))
}
