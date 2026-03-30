use axum::{
    extract::FromRequestParts,
    http::{header, request::Parts, StatusCode},
};
use jsonwebtoken::{decode, DecodingKey, Validation};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::error::{err, ApiError};
use crate::services::agent::AgentHandle;
use crate::AppState;

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct Claims {
    pub sub: Uuid,
    pub email: String,
    pub role: String,
    pub exp: usize,
    pub iat: usize,
    /// JWT ID for token blacklisting on logout.
    #[serde(default)]
    pub jti: Option<String>,
}

/// JWT extractor — reads token from Authorization header or `token` cookie.
pub struct AuthUser(pub Claims);

impl FromRequestParts<AppState> for AuthUser {
    type Rejection = ApiError;

    async fn from_request_parts(
        parts: &mut Parts,
        state: &AppState,
    ) -> Result<Self, Self::Rejection> {
        // Try Authorization: Bearer <token> first
        let bearer_token = parts
            .headers
            .get(header::AUTHORIZATION)
            .and_then(|v| v.to_str().ok())
            .and_then(|v| v.strip_prefix("Bearer "))
            .map(|t| t.to_string());

        let token = bearer_token.clone().or_else(|| {
                // Fall back to cookie
                parts
                    .headers
                    .get(header::COOKIE)
                    .and_then(|v| v.to_str().ok())
                    .and_then(|cookies| {
                        cookies
                            .split(';')
                            .find_map(|s| s.trim().strip_prefix("token=").map(|v| v.to_string()))
                    })
            })
            .ok_or_else(|| err(StatusCode::UNAUTHORIZED, "Authentication required"))?;

        // CSRF protection: cookie-based auth on mutating methods requires X-Requested-With header.
        // Bearer token auth (API keys) is exempt since it cannot be sent by cross-origin forms.
        if bearer_token.is_none() {
            let method = &parts.method;
            if method == "POST" || method == "PUT" || method == "DELETE" || method == "PATCH" {
                let has_custom_header = parts
                    .headers
                    .get("x-requested-with")
                    .is_some();
                if !has_custom_header {
                    return Err(err(StatusCode::FORBIDDEN, "Missing CSRF header"));
                }
            }
        }

        let mut validation = Validation::new(jsonwebtoken::Algorithm::HS256);
        validation.validate_exp = true;
        validation.leeway = 0;

        let claims = decode::<Claims>(
            &token,
            &DecodingKey::from_secret(state.config.jwt_secret.as_bytes()),
            &validation,
        )
        .map_err(|_| err(StatusCode::UNAUTHORIZED, "Invalid or expired token"))?
        .claims;

        // Check token blacklist (revoked JTIs)
        if let Some(ref jti) = claims.jti {
            let blacklist = state.token_blacklist.read().await;
            if blacklist.contains(jti) {
                return Err(err(StatusCode::UNAUTHORIZED, "Token has been revoked"));
            }
        }

        // Check global session revocation (revoke_all_sessions)
        {
            let revoked_at = state.sessions_revoked_at.read().await;
            if let Some(ts) = *revoked_at {
                if (claims.iat as i64) < ts {
                    return Err(err(StatusCode::UNAUTHORIZED, "Session revoked. Please log in again."));
                }
            }
        }

        Ok(AuthUser(claims))
    }
}

/// Admin-only JWT extractor — extracts Claims then verifies role == "admin".
pub struct AdminUser(pub Claims);

impl FromRequestParts<AppState> for AdminUser {
    type Rejection = ApiError;

    async fn from_request_parts(
        parts: &mut Parts,
        state: &AppState,
    ) -> Result<Self, Self::Rejection> {
        let AuthUser(claims) = AuthUser::from_request_parts(parts, state)
            .await
            .map_err(|_| err(StatusCode::UNAUTHORIZED, "Authentication required"))?;

        if claims.role != "admin" {
            return Err(err(StatusCode::FORBIDDEN, "Admin access required"));
        }

        Ok(AdminUser(claims))
    }
}

/// Reseller-or-admin JWT extractor — allows role == "admin" OR role == "reseller".
/// Used for endpoints accessible to both admins and resellers.
pub struct ResellerUser(pub Claims);

impl FromRequestParts<AppState> for ResellerUser {
    type Rejection = ApiError;

    async fn from_request_parts(
        parts: &mut Parts,
        state: &AppState,
    ) -> Result<Self, Self::Rejection> {
        let AuthUser(claims) = AuthUser::from_request_parts(parts, state)
            .await
            .map_err(|_| err(StatusCode::UNAUTHORIZED, "Authentication required"))?;

        if claims.role != "admin" && claims.role != "reseller" {
            return Err(err(StatusCode::FORBIDDEN, "Admin or reseller access required"));
        }

        Ok(ResellerUser(claims))
    }
}

/// Server scope extractor — reads `X-Server-Id` header to determine which server
/// the request targets. Falls back to the local server if the header is absent.
///
/// Usage in handlers:
/// ```
/// async fn my_handler(
///     State(state): State<AppState>,
///     AuthUser(claims): AuthUser,
///     ServerScope(server_id, agent): ServerScope,
/// ) -> Result<..., ApiError> { ... }
/// ```
pub struct ServerScope(pub Uuid, pub AgentHandle);

impl FromRequestParts<AppState> for ServerScope {
    type Rejection = ApiError;

    async fn from_request_parts(
        parts: &mut Parts,
        state: &AppState,
    ) -> Result<Self, Self::Rejection> {
        // Extract JWT claims to verify server ownership
        let token = parts
            .headers
            .get(header::AUTHORIZATION)
            .and_then(|v| v.to_str().ok())
            .and_then(|v| v.strip_prefix("Bearer "))
            .map(|t| t.to_string())
            .or_else(|| {
                parts.headers.get(header::COOKIE)
                    .and_then(|v| v.to_str().ok())
                    .and_then(|cookies| {
                        cookies.split(';').find_map(|s| s.trim().strip_prefix("token=").map(|v| v.to_string()))
                    })
            });

        let user_id = if let Some(ref tok) = token {
            let mut validation = jsonwebtoken::Validation::new(jsonwebtoken::Algorithm::HS256);
            validation.validate_exp = true;
            validation.leeway = 0;
            jsonwebtoken::decode::<Claims>(
                tok,
                &jsonwebtoken::DecodingKey::from_secret(state.config.jwt_secret.as_bytes()),
                &validation,
            )
            .ok()
            .map(|data| data.claims.sub)
        } else {
            None
        };

        // Read X-Server-Id header
        let server_id = parts
            .headers
            .get("x-server-id")
            .and_then(|v| v.to_str().ok())
            .and_then(|v| Uuid::parse_str(v).ok());

        match server_id {
            Some(sid) => {
                // Verify server belongs to the authenticated user
                let uid = user_id.ok_or_else(|| {
                    err(StatusCode::UNAUTHORIZED, "Authentication required when X-Server-Id is provided")
                })?;

                let owns: Option<(Uuid,)> = sqlx::query_as(
                    "SELECT id FROM servers WHERE id = $1 AND user_id = $2",
                )
                .bind(sid)
                .bind(uid)
                .fetch_optional(&state.db)
                .await
                .map_err(|_| err(StatusCode::INTERNAL_SERVER_ERROR, "Server lookup failed"))?;

                if owns.is_none() {
                    return Err(err(StatusCode::FORBIDDEN, "Server not found or access denied"));
                }

                // Resolve agent for this server
                let agent = state
                    .agents
                    .for_server(sid)
                    .await
                    .map_err(|e| err(StatusCode::BAD_GATEWAY, &e.to_string()))?;
                Ok(ServerScope(sid, agent))
            }
            None => {
                // Default to local server
                let local_id = state
                    .agents
                    .local_server_id()
                    .await
                    .ok_or_else(|| {
                        err(
                            StatusCode::SERVICE_UNAVAILABLE,
                            "Local server not yet registered",
                        )
                    })?;
                let agent = AgentHandle::Local(state.agents.local().clone());
                Ok(ServerScope(local_id, agent))
            }
        }
    }
}
