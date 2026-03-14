use axum::{
    extract::FromRequestParts,
    http::{header, request::Parts, StatusCode},
};
use jsonwebtoken::{decode, DecodingKey, Validation};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::error::{err, ApiError};
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
        let token = parts
            .headers
            .get(header::AUTHORIZATION)
            .and_then(|v| v.to_str().ok())
            .and_then(|v| v.strip_prefix("Bearer "))
            .map(|t| t.to_string())
            .or_else(|| {
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
