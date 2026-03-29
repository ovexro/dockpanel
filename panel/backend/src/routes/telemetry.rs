use axum::{
    extract::{Query, State},
    http::StatusCode,
    Json,
};
use serde::Deserialize;

use crate::auth::{AdminUser, ServerScope};
use crate::error::{agent_error, err, internal_error, paginate, ApiError};
use crate::AppState;

// ── List Events ────────────────────────────────────────────────────────

#[derive(Deserialize)]
pub struct ListParams {
    pub limit: Option<i64>,
    pub offset: Option<i64>,
    pub category: Option<String>,
    pub event_type: Option<String>,
}

/// GET /api/telemetry/events — List local telemetry events (admin only).
pub async fn list_events(
    State(state): State<AppState>,
    AdminUser(_claims): AdminUser,
    Query(params): Query<ListParams>,
) -> Result<Json<serde_json::Value>, ApiError> {
    let (limit, offset) = paginate(params.limit, params.offset);

    let (rows, total): (Vec<_>, i64) = if let Some(ref cat) = params.category {
        let rows: Vec<(uuid::Uuid, String, String, String, serde_json::Value, Option<chrono::DateTime<chrono::Utc>>, chrono::DateTime<chrono::Utc>)> = sqlx::query_as(
            "SELECT id, event_type, category, message, context, sent_at, created_at \
             FROM telemetry_events WHERE category = $1 \
             ORDER BY created_at DESC LIMIT $2 OFFSET $3",
        )
        .bind(cat)
        .bind(limit)
        .bind(offset)
        .fetch_all(&state.db)
        .await
        .map_err(|e| internal_error("list telemetry events", e))?;

        let total: (i64,) = sqlx::query_as(
            "SELECT COUNT(*) FROM telemetry_events WHERE category = $1",
        )
        .bind(cat)
        .fetch_one(&state.db)
        .await
        .map_err(|e| internal_error("count telemetry events", e))?;

        (rows, total.0)
    } else if let Some(ref et) = params.event_type {
        let rows: Vec<(uuid::Uuid, String, String, String, serde_json::Value, Option<chrono::DateTime<chrono::Utc>>, chrono::DateTime<chrono::Utc>)> = sqlx::query_as(
            "SELECT id, event_type, category, message, context, sent_at, created_at \
             FROM telemetry_events WHERE event_type = $1 \
             ORDER BY created_at DESC LIMIT $2 OFFSET $3",
        )
        .bind(et)
        .bind(limit)
        .bind(offset)
        .fetch_all(&state.db)
        .await
        .map_err(|e| internal_error("list telemetry events", e))?;

        let total: (i64,) = sqlx::query_as(
            "SELECT COUNT(*) FROM telemetry_events WHERE event_type = $1",
        )
        .bind(et)
        .fetch_one(&state.db)
        .await
        .map_err(|e| internal_error("count telemetry events", e))?;

        (rows, total.0)
    } else {
        let rows: Vec<(uuid::Uuid, String, String, String, serde_json::Value, Option<chrono::DateTime<chrono::Utc>>, chrono::DateTime<chrono::Utc>)> = sqlx::query_as(
            "SELECT id, event_type, category, message, context, sent_at, created_at \
             FROM telemetry_events \
             ORDER BY created_at DESC LIMIT $1 OFFSET $2",
        )
        .bind(limit)
        .bind(offset)
        .fetch_all(&state.db)
        .await
        .map_err(|e| internal_error("list telemetry events", e))?;

        let total: (i64,) =
            sqlx::query_as("SELECT COUNT(*) FROM telemetry_events")
                .fetch_one(&state.db)
                .await
                .map_err(|e| internal_error("count telemetry events", e))?;

        (rows, total.0)
    };

    let events: Vec<serde_json::Value> = rows
        .into_iter()
        .map(|(id, event_type, category, message, context, sent_at, created_at)| {
            serde_json::json!({
                "id": id,
                "event_type": event_type,
                "category": category,
                "message": message,
                "context": context,
                "sent_at": sent_at,
                "created_at": created_at,
            })
        })
        .collect();

    Ok(Json(serde_json::json!({
        "events": events,
        "total": total,
    })))
}

// ── Stats ──────────────────────────────────────────────────────────────

/// GET /api/telemetry/stats — Event counts by category and type (admin only).
pub async fn stats(
    State(state): State<AppState>,
    AdminUser(_claims): AdminUser,
) -> Result<Json<serde_json::Value>, ApiError> {
    let by_category: Vec<(String, i64)> = sqlx::query_as(
        "SELECT category, COUNT(*) FROM telemetry_events \
         GROUP BY category ORDER BY count DESC",
    )
    .fetch_all(&state.db)
    .await
    .map_err(|e| internal_error("telemetry stats", e))?;

    let by_type: Vec<(String, i64)> = sqlx::query_as(
        "SELECT event_type, COUNT(*) FROM telemetry_events \
         GROUP BY event_type ORDER BY count DESC",
    )
    .fetch_all(&state.db)
    .await
    .map_err(|e| internal_error("telemetry stats", e))?;

    let unsent: (i64,) = sqlx::query_as(
        "SELECT COUNT(*) FROM telemetry_events WHERE sent_at IS NULL",
    )
    .fetch_one(&state.db)
    .await
    .map_err(|e| internal_error("telemetry stats", e))?;

    let total: (i64,) =
        sqlx::query_as("SELECT COUNT(*) FROM telemetry_events")
            .fetch_one(&state.db)
            .await
            .map_err(|e| internal_error("telemetry stats", e))?;

    // Recent events (last 24h)
    let recent: (i64,) = sqlx::query_as(
        "SELECT COUNT(*) FROM telemetry_events WHERE created_at > NOW() - INTERVAL '24 hours'",
    )
    .fetch_one(&state.db)
    .await
    .map_err(|e| internal_error("telemetry stats", e))?;

    Ok(Json(serde_json::json!({
        "total": total.0,
        "unsent": unsent.0,
        "last_24h": recent.0,
        "by_category": by_category.iter().map(|(c, n)| serde_json::json!({"category": c, "count": n})).collect::<Vec<_>>(),
        "by_type": by_type.iter().map(|(t, n)| serde_json::json!({"type": t, "count": n})).collect::<Vec<_>>(),
    })))
}

// ── Config ─────────────────────────────────────────────────────────────

/// GET /api/telemetry/config — Get telemetry configuration (admin only).
pub async fn get_config(
    State(state): State<AppState>,
    AdminUser(_claims): AdminUser,
) -> Result<Json<serde_json::Value>, ApiError> {
    let keys = [
        "telemetry_enabled",
        "telemetry_endpoint",
        "telemetry_installation_id",
        "update_available_version",
        "update_release_notes",
        "update_release_url",
        "update_checked_at",
    ];

    let rows: Vec<(String, String)> =
        sqlx::query_as("SELECT key, value FROM settings WHERE key = ANY($1)")
            .bind(&keys[..])
            .fetch_all(&state.db)
            .await
            .map_err(|e| internal_error("get telemetry config", e))?;

    let mut config = serde_json::Map::new();
    for (key, value) in rows {
        config.insert(key, serde_json::Value::String(value));
    }

    // Add current version
    config.insert(
        "current_version".to_string(),
        serde_json::Value::String(env!("CARGO_PKG_VERSION").to_string()),
    );

    Ok(Json(serde_json::Value::Object(config)))
}

// ── Update Config ──────────────────────────────────────────────────────

#[derive(Deserialize)]
pub struct ConfigUpdate {
    pub telemetry_enabled: Option<String>,
    pub telemetry_endpoint: Option<String>,
}

/// PUT /api/telemetry/config — Update telemetry configuration (admin only).
pub async fn update_config(
    State(state): State<AppState>,
    AdminUser(claims): AdminUser,
    Json(body): Json<ConfigUpdate>,
) -> Result<Json<serde_json::Value>, ApiError> {
    // Validate endpoint URL if provided
    if let Some(ref endpoint) = body.telemetry_endpoint {
        if endpoint.is_empty() {
            // Empty string is valid — means "disable remote telemetry"
        } else if !endpoint.starts_with("https://") {
            return Err(err(
                StatusCode::BAD_REQUEST,
                "Telemetry endpoint must use HTTPS",
            ));
        }
        if endpoint.len() > 512 {
            return Err(err(
                StatusCode::BAD_REQUEST,
                "Endpoint URL too long (max 512 chars)",
            ));
        }
    }

    // Validate enabled value
    if let Some(ref enabled) = body.telemetry_enabled {
        if enabled != "true" && enabled != "false" {
            return Err(err(
                StatusCode::BAD_REQUEST,
                "telemetry_enabled must be 'true' or 'false'",
            ));
        }
    }

    let mut tx = state
        .db
        .begin()
        .await
        .map_err(|e| internal_error("update telemetry config", e))?;

    if let Some(ref enabled) = body.telemetry_enabled {
        sqlx::query(
            "INSERT INTO settings (key, value) VALUES ('telemetry_enabled', $1) \
             ON CONFLICT (key) DO UPDATE SET value = EXCLUDED.value",
        )
        .bind(enabled)
        .execute(&mut *tx)
        .await
        .map_err(|e| internal_error("update telemetry config", e))?;
    }

    if let Some(ref endpoint) = body.telemetry_endpoint {
        sqlx::query(
            "INSERT INTO settings (key, value) VALUES ('telemetry_endpoint', $1) \
             ON CONFLICT (key) DO UPDATE SET value = EXCLUDED.value",
        )
        .bind(endpoint)
        .execute(&mut *tx)
        .await
        .map_err(|e| internal_error("update telemetry config", e))?;
    }

    tx.commit()
        .await
        .map_err(|e| internal_error("commit telemetry config", e))?;

    tracing::info!(user = %claims.email, "Telemetry config updated");

    Ok(Json(serde_json::json!({ "success": true })))
}

// ── Preview Report ─────────────────────────────────────────────────────

/// GET /api/telemetry/preview — Preview what would be sent (admin only).
pub async fn preview(
    State(state): State<AppState>,
    AdminUser(_claims): AdminUser,
    ServerScope(_server_id, agent): ServerScope,
) -> Result<Json<serde_json::Value>, ApiError> {
    // Get unsent events
    let events: Vec<(uuid::Uuid, String, String, String, serde_json::Value, chrono::DateTime<chrono::Utc>)> =
        sqlx::query_as(
            "SELECT id, event_type, category, message, context, created_at \
             FROM telemetry_events WHERE sent_at IS NULL \
             ORDER BY created_at ASC LIMIT 50",
        )
        .fetch_all(&state.db)
        .await
        .map_err(|e| internal_error("preview telemetry", e))?;

    // Get system info from agent
    let system_info = match agent.get("/telemetry/system-info").await {
        Ok(data) => data,
        Err(e) => return Err(agent_error("get telemetry system info", e)),
    };

    let installation_id: Option<(String,)> = sqlx::query_as(
        "SELECT value FROM settings WHERE key = 'telemetry_installation_id'",
    )
    .fetch_optional(&state.db)
    .await
    .ok()
    .flatten();

    let batch: Vec<serde_json::Value> = events
        .iter()
        .map(|(id, event_type, category, message, context, created_at)| {
            serde_json::json!({
                "id": id.to_string(),
                "event_type": event_type,
                "category": category,
                "message": message,
                "context": crate::services::telemetry_collector::strip_pii(context),
                "created_at": created_at.to_rfc3339(),
            })
        })
        .collect();

    Ok(Json(serde_json::json!({
        "system": system_info,
        "installation_id": installation_id.map(|r| r.0).unwrap_or_else(|| "not yet generated".to_string()),
        "dockpanel_version": env!("CARGO_PKG_VERSION"),
        "events": batch,
        "event_count": batch.len(),
        "note": "This is exactly what would be sent to the configured endpoint. All PII has been stripped.",
    })))
}

// ── Send Now ───────────────────────────────────────────────────────────

/// POST /api/telemetry/send — Send pending events now (admin only).
pub async fn send_now(
    State(state): State<AppState>,
    AdminUser(claims): AdminUser,
) -> Result<Json<serde_json::Value>, ApiError> {
    let endpoint: Option<(String,)> = sqlx::query_as(
        "SELECT value FROM settings WHERE key = 'telemetry_endpoint'",
    )
    .fetch_optional(&state.db)
    .await
    .map_err(|e| internal_error("get telemetry endpoint", e))?;

    let endpoint = endpoint
        .map(|r| r.0)
        .unwrap_or_default();

    if endpoint.is_empty() {
        return Err(err(
            StatusCode::BAD_REQUEST,
            "No telemetry endpoint configured",
        ));
    }

    let enabled: Option<(String,)> = sqlx::query_as(
        "SELECT value FROM settings WHERE key = 'telemetry_enabled'",
    )
    .fetch_optional(&state.db)
    .await
    .ok()
    .flatten();

    if enabled.as_ref().map(|r| r.0.as_str()) != Some("true") {
        return Err(err(
            StatusCode::BAD_REQUEST,
            "Telemetry is not enabled",
        ));
    }

    // Trigger send in background
    let pool = state.db.clone();
    tokio::spawn(async move {
        crate::services::telemetry_collector::send_pending_events_public(&pool, &endpoint).await;
    });

    tracing::info!(user = %claims.email, "Telemetry manual send triggered");

    Ok(Json(
        serde_json::json!({ "success": true, "message": "Sending telemetry events in background" }),
    ))
}

// ── Export Report ──────────────────────────────────────────────────────

/// GET /api/telemetry/export — Export all unsent events as a downloadable JSON (admin only).
pub async fn export_report(
    State(state): State<AppState>,
    AdminUser(_claims): AdminUser,
    ServerScope(_server_id, agent): ServerScope,
) -> Result<Json<serde_json::Value>, ApiError> {
    let events: Vec<(uuid::Uuid, String, String, String, serde_json::Value, chrono::DateTime<chrono::Utc>)> =
        sqlx::query_as(
            "SELECT id, event_type, category, message, context, created_at \
             FROM telemetry_events WHERE sent_at IS NULL \
             ORDER BY created_at ASC LIMIT 500",
        )
        .fetch_all(&state.db)
        .await
        .map_err(|e| internal_error("export telemetry", e))?;

    // Get system info from agent
    let sys = agent
        .get("/telemetry/system-info")
        .await
        .unwrap_or(serde_json::json!({}));

    let batch: Vec<serde_json::Value> = events
        .iter()
        .map(|(id, event_type, category, message, context, created_at)| {
            serde_json::json!({
                "id": id.to_string(),
                "event_type": event_type,
                "category": category,
                "message": message,
                "context": crate::services::telemetry_collector::strip_pii(context),
                "created_at": created_at.to_rfc3339(),
            })
        })
        .collect();

    Ok(Json(serde_json::json!({
        "dockpanel_version": env!("CARGO_PKG_VERSION"),
        "export_date": chrono::Utc::now().to_rfc3339(),
        "system": sys,
        "events": batch,
        "event_count": batch.len(),
    })))
}

// ── Clear Events ───────────────────────────────────────────────────────

#[derive(Deserialize)]
pub struct ClearParams {
    pub before_days: Option<i64>,
}

/// DELETE /api/telemetry/events — Clear old telemetry events (admin only).
pub async fn clear_events(
    State(state): State<AppState>,
    AdminUser(claims): AdminUser,
    Query(params): Query<ClearParams>,
) -> Result<Json<serde_json::Value>, ApiError> {
    let days = params.before_days.unwrap_or(0).max(0).min(3650);

    let deleted = if days > 0 {
        sqlx::query_scalar::<_, i64>(
            "WITH d AS (DELETE FROM telemetry_events WHERE created_at < NOW() - ($1 || ' days')::interval RETURNING 1) SELECT COUNT(*) FROM d",
        )
        .bind(days.to_string())
        .fetch_one(&state.db)
        .await
        .map_err(|e| internal_error("clear telemetry events", e))?
    } else {
        // Clear all
        sqlx::query_scalar::<_, i64>(
            "WITH d AS (DELETE FROM telemetry_events RETURNING 1) SELECT COUNT(*) FROM d",
        )
        .fetch_one(&state.db)
        .await
        .map_err(|e| internal_error("clear telemetry events", e))?
    };

    tracing::info!(user = %claims.email, deleted, "Telemetry events cleared");

    Ok(Json(
        serde_json::json!({ "success": true, "deleted": deleted }),
    ))
}

// ── Update Status ──────────────────────────────────────────────────────

/// GET /api/telemetry/update-status — Check if a DockPanel update is available (admin only).
pub async fn update_status(
    State(state): State<AppState>,
    AdminUser(_claims): AdminUser,
) -> Result<Json<serde_json::Value>, ApiError> {
    let keys = [
        "update_available_version",
        "update_release_notes",
        "update_release_url",
        "update_checked_at",
    ];

    let rows: Vec<(String, String)> =
        sqlx::query_as("SELECT key, value FROM settings WHERE key = ANY($1)")
            .bind(&keys[..])
            .fetch_all(&state.db)
            .await
            .map_err(|e| internal_error("get update status", e))?;

    let mut result = serde_json::Map::new();
    result.insert(
        "current_version".to_string(),
        serde_json::Value::String(env!("CARGO_PKG_VERSION").to_string()),
    );

    let mut has_update = false;
    for (key, value) in rows {
        if key == "update_available_version" && !value.is_empty() {
            has_update = true;
        }
        result.insert(key, serde_json::Value::String(value));
    }
    result.insert(
        "update_available".to_string(),
        serde_json::Value::Bool(has_update),
    );

    Ok(Json(serde_json::Value::Object(result)))
}

/// POST /api/telemetry/check-updates — Force an update check now (admin only).
pub async fn check_updates(
    State(state): State<AppState>,
    AdminUser(claims): AdminUser,
) -> Result<Json<serde_json::Value>, ApiError> {
    let pool = state.db.clone();
    tokio::spawn(async move {
        crate::services::telemetry_collector::check_for_updates_public(&pool).await;
    });

    tracing::info!(user = %claims.email, "Manual update check triggered");

    Ok(Json(
        serde_json::json!({ "success": true, "message": "Update check started" }),
    ))
}
