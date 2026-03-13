use axum::{
    extract::{Query, State},
    http::StatusCode,
    Json,
};
use jsonwebtoken::{encode, EncodingKey, Header};

use crate::auth::AuthUser;
use crate::error::{err, ApiError};
use crate::AppState;

#[derive(serde::Deserialize)]
pub struct TerminalQuery {
    pub site_id: Option<String>,
}

#[derive(serde::Serialize)]
struct TerminalTicket {
    sub: String,
    purpose: String,
    exp: usize,
}

/// GET /api/terminal/token — Generate a short-lived terminal ticket.
/// Returns a 60-second JWT signed with the agent token (never exposes the raw agent token).
pub async fn ws_token(
    State(state): State<AppState>,
    AuthUser(claims): AuthUser,
    Query(q): Query<TerminalQuery>,
) -> Result<Json<serde_json::Value>, ApiError> {
    // Server-level terminal requires admin role
    if q.site_id.is_none() && claims.role != "admin" {
        return Err(err(
            StatusCode::FORBIDDEN,
            "Admin access required for server terminal",
        ));
    }

    // Optionally resolve domain from site_id
    let domain = if let Some(ref sid) = q.site_id {
        let site_id: uuid::Uuid = sid
            .parse()
            .map_err(|_| err(StatusCode::BAD_REQUEST, "Invalid site_id"))?;

        let row: Option<(String,)> =
            sqlx::query_as("SELECT domain FROM sites WHERE id = $1 AND user_id = $2")
                .bind(site_id)
                .bind(claims.sub)
                .fetch_optional(&state.db)
                .await
                .map_err(|e| err(StatusCode::INTERNAL_SERVER_ERROR, &e.to_string()))?;

        row.map(|(d,)| d)
    } else {
        None
    };

    // Generate a short-lived JWT ticket (60 seconds) signed with the agent token
    let ticket = TerminalTicket {
        sub: claims.email,
        purpose: "terminal".to_string(),
        exp: (chrono::Utc::now() + chrono::Duration::seconds(60)).timestamp() as usize,
    };

    let token = encode(
        &Header::default(),
        &ticket,
        &EncodingKey::from_secret(state.agent.token().as_bytes()),
    )
    .map_err(|e| err(StatusCode::INTERNAL_SERVER_ERROR, &e.to_string()))?;

    Ok(Json(serde_json::json!({
        "token": token,
        "domain": domain,
    })))
}
