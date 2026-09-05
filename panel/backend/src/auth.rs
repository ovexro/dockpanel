use axum::{
    extract::FromRequestParts,
    http::{header, request::Parts, StatusCode},
};
use jsonwebtoken::{decode, DecodingKey, Validation};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use uuid::Uuid;

use crate::error::{err, err_coded, ApiError, CODE_SESSION_INVALID};
use crate::services::agent::AgentHandle;
use crate::AppState;

/// The caller has no usable session — the only refusal in this codebase that
/// entitles the frontend to navigate someone to `/login`.
///
/// Every such refusal is minted here rather than inline, so that "which 401s
/// mean *log in again*" is a question with one answer in one place instead of a
/// property to be re-derived from seven call sites. A handler's own 401 must
/// never use this: by the time a handler runs, this extractor has already
/// accepted the session, so the thing being refused there is a credential the
/// caller just presented, not the session itself.
///
/// The agent-token refusals at the bottom of this file are deliberately NOT
/// minted here — an agent is not a browser and has no session to lose.
fn session_invalid(msg: &str) -> ApiError {
    err_coded(StatusCode::UNAUTHORIZED, msg, CODE_SESSION_INVALID)
}

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

        // A `dp_`-prefixed bearer token is a long-lived API key (routes/api_keys.rs),
        // not a session JWT — authenticate it against `api_keys` directly. Trying to
        // `decode::<Claims>` it below would always fail, so without this branch every
        // API key request would 401 as "Invalid or expired token" for the wrong
        // reason and this scaffold would stay unwired, exactly as FEATURES.md's
        // "Withdrawn Claims" section already documents it.
        if let Some(key) = bearer_token.as_deref().filter(|t| t.starts_with("dp_")) {
            return authenticate_api_key(state, key).await.map(AuthUser);
        }

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
            .ok_or_else(|| session_invalid("Authentication required"))?;

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
        .map_err(|_| session_invalid("Invalid or expired token"))?
        .claims;

        // Check token blacklist (revoked JTIs)
        if let Some(ref jti) = claims.jti {
            let blacklist = state.token_blacklist.read().await;
            if blacklist.contains(jti) {
                return Err(session_invalid("Token has been revoked"));
            }
        }

        // Check global session revocation (revoke_all_sessions)
        {
            let revoked_at = state.sessions_revoked_at.read().await;
            if let Some(ts) = *revoked_at {
                if (claims.iat as i64) < ts {
                    return Err(session_invalid("Session revoked. Please log in again."));
                }
            }
        }

        // A suspended account's token must not authenticate. A pre-suspension token
        // carries the old role and is killed by the blacklist sweep on suspend; this
        // is defense-in-depth against any suspended-role token ever being minted (the
        // login/2FA/OAuth paths also reject "suspended" up front).
        if claims.role == "suspended" {
            return Err(err(StatusCode::FORBIDDEN, "Account suspended"));
        }

        Ok(AuthUser(claims))
    }
}

/// Authenticate a `dp_`-prefixed API key (routes/api_keys.rs) and build the same
/// `Claims` shape a JWT would, so it works for free on every `AuthUser`-gated
/// handler. Unlike a JWT, a key has no `jti` of its own and is never on the
/// per-token blacklist — it is revoked by deleting the `api_keys` row (or, for
/// the global "revoke all sessions" / Panic Button controls below, by having
/// been minted before that revocation fired: see the `created_at` check).
///
/// Failures here deliberately do NOT use `session_invalid()` — that marker
/// means "the caller has no usable *session*" and its own doc comment says the
/// same of the agent-token refusals below: a key is not a browser session that
/// can be lost, so a bad key gets a plain 401 like `authenticate_agent` does,
/// not the marker that tells a frontend to redirect to `/login`.
async fn authenticate_api_key(state: &AppState, key: &str) -> Result<Claims, ApiError> {
    let mut hasher = Sha256::new();
    hasher.update(key.as_bytes());
    let key_hash = hex::encode(hasher.finalize());

    let row: Option<(Uuid, Uuid, chrono::DateTime<chrono::Utc>)> = sqlx::query_as(
        "SELECT id, user_id, created_at FROM api_keys WHERE key_hash = $1",
    )
    .bind(&key_hash)
    .fetch_optional(&state.db)
    .await
    .map_err(|e| crate::error::internal_error("api_key lookup", e))?;

    let (key_id, user_id, created_at) =
        row.ok_or_else(|| err(StatusCode::UNAUTHORIZED, "Invalid or expired token"))?;

    // Parity with the JWT path's own revocation check (above) — "Revoke all
    // sessions" and the Panic Button both promise a total logout, and a key
    // minted before that moment fired is exactly the credential class an
    // incident responder reaching for either control means to kill. A key
    // rotated (or newly minted) AFTER the revocation naturally has a fresh
    // `created_at` and keeps working, same as a JWT from a fresh login would.
    {
        let revoked_at = state.sessions_revoked_at.read().await;
        if revoked_at.is_some_and(|ts| created_at.timestamp() < ts) {
            return Err(err(StatusCode::UNAUTHORIZED, "Key revoked. Mint a new one."));
        }
    }

    let user: Option<(String, String)> =
        sqlx::query_as("SELECT email, role FROM users WHERE id = $1")
            .bind(user_id)
            .fetch_optional(&state.db)
            .await
            .map_err(|e| crate::error::internal_error("api_key user lookup", e))?;

    let (email, role) =
        user.ok_or_else(|| err(StatusCode::UNAUTHORIZED, "Invalid or expired token"))?;

    // Parity with the JWT path's own post-decode check (this file, below) — an
    // account suspended after a key was minted must not keep authenticating.
    if role == "suspended" {
        return Err(err(StatusCode::FORBIDDEN, "Account suspended"));
    }

    // Best-effort bookkeeping — a failed UPDATE must not fail the request that
    // is itself proof the key still works.
    let _ = sqlx::query("UPDATE api_keys SET last_used_at = NOW() WHERE id = $1")
        .bind(key_id)
        .execute(&state.db)
        .await;

    let now = chrono::Utc::now().timestamp() as usize;
    Ok(Claims {
        sub: user_id,
        email,
        role,
        // An API key has no session lifetime of its own — it lives until its
        // `api_keys` row is deleted (or the revocation check above fires).
        // `exp` still has to be a value in the future because nothing
        // downstream re-checks how this Claims was built; it is never
        // re-validated against a signature, so this is not a forgeable
        // expiry, just a field every consumer of `Claims` expects.
        exp: now + 315_360_000, // ~10 years
        iat: now,
        jti: None,
    })
}

/// Admin-only JWT extractor — extracts Claims then verifies role == "admin".
pub struct AdminUser(pub Claims);

impl FromRequestParts<AppState> for AdminUser {
    type Rejection = ApiError;

    async fn from_request_parts(
        parts: &mut Parts,
        state: &AppState,
    ) -> Result<Self, Self::Rejection> {
        // Propagate the inner rejection verbatim — flattening it to a generic
        // 401 hid "Missing CSRF header" (403) and "Invalid or expired token"
        // behind "Authentication required" on every admin endpoint, which sends
        // clients debugging a stale session instead of the real cause.
        let AuthUser(claims) = AuthUser::from_request_parts(parts, state).await?;

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
        // Same rationale as AdminUser: keep the inner rejection intact.
        let AuthUser(claims) = AuthUser::from_request_parts(parts, state).await?;

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
                    session_invalid("Authentication required when X-Server-Id is provided")
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

/// Authenticate an AGENT-originated request from its `servers` row token, and
/// return the server's id.
///
/// This is a different credential from every extractor above: an agent holds a
/// random hex string issued at install time (`install-agent.sh`), not a signed
/// user JWT, so `AuthUser` can never accept one. Hash-based lookup first, with a
/// plaintext fallback for rows predating the hashing migration.
///
/// It lives here, as the single implementation, because it used to live in
/// `routes/agent_commands.rs` while `routes/agent_updates.rs` guarded the very
/// same class of caller with `AuthUser` — so `/api/agent/version` demanded a JWT
/// from a caller that structurally cannot hold one and 401'd on every request
/// for four releases. One copy, used by every agent route (s233).
pub async fn authenticate_agent(
    state: &AppState,
    headers: &axum::http::HeaderMap,
) -> Result<Uuid, ApiError> {
    let token = headers
        .get("authorization")
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.strip_prefix("Bearer "))
        .ok_or_else(|| err(StatusCode::UNAUTHORIZED, "Missing authorization"))?;

    let token_hash = crate::helpers::hash_agent_token(token);
    let row: Option<(Uuid,)> =
        sqlx::query_as("SELECT id FROM servers WHERE agent_token_hash = $1")
            .bind(&token_hash)
            .fetch_optional(&state.db)
            .await
            .map_err(|e| crate::error::internal_error("unknown", e))?;

    if let Some(r) = row {
        return Ok(r.0);
    }

    // Fallback: plaintext lookup for pre-migration rows.
    let row: Option<(Uuid,)> =
        sqlx::query_as("SELECT id FROM servers WHERE agent_token = $1 AND agent_token_hash IS NULL")
            .bind(token)
            .fetch_optional(&state.db)
            .await
            .map_err(|e| crate::error::internal_error("unknown", e))?;

    row.map(|r| r.0)
        .ok_or_else(|| err(StatusCode::UNAUTHORIZED, "Invalid token"))
}

/// Per-server rate limit shared by the agent routes: 120 requests/minute.
/// Returns `Err(429)` when exceeded.
pub fn agent_rate_limit(state: &AppState, server_id: Uuid) -> Result<(), ApiError> {
    let mut limits = state
        .agent_rate_limits
        .lock()
        .unwrap_or_else(|e| e.into_inner());
    let now = std::time::Instant::now();
    let entry = limits.entry(server_id).or_insert((0, now));
    if now.duration_since(entry.1).as_secs() >= 60 {
        *entry = (1, now);
        return Ok(());
    }
    entry.0 += 1;
    if entry.0 > 120 {
        return Err(err(StatusCode::TOO_MANY_REQUESTS, "Rate limit exceeded"));
    }
    Ok(())
}

#[cfg(test)]
mod session_marker_tests {
    use super::*;

    fn body(e: &ApiError) -> serde_json::Value {
        e.1 .0.clone()
    }

    #[test]
    fn session_invalid_is_a_401_carrying_the_marker() {
        let e = session_invalid("Invalid or expired token");
        assert_eq!(e.0, StatusCode::UNAUTHORIZED);
        assert_eq!(body(&e)["code"], CODE_SESSION_INVALID);
        assert_eq!(body(&e)["error"], "Invalid or expired token");
    }

    /// The sentence must survive alongside the marker. The whole defect being
    /// repaired is a client that had the status and threw the sentence away.
    #[test]
    fn the_sentence_is_not_replaced_by_the_marker() {
        let e = session_invalid("Session revoked. Please log in again.");
        assert_eq!(body(&e)["error"], "Session revoked. Please log in again.");
    }

    /// An agent presenting a bad token is not a browser losing a session, and
    /// must not be able to make the client navigate a human to /login.
    #[test]
    fn a_plain_401_carries_no_marker() {
        let e = err(StatusCode::UNAUTHORIZED, "Invalid token");
        assert_eq!(e.0, StatusCode::UNAUTHORIZED);
        assert!(body(&e).get("code").is_none());
    }
}
