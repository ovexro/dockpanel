use axum::{
    extract::State,
    http::StatusCode,
    Json,
};
use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine};
use p256::ecdsa::{signature::Verifier, Signature, VerifyingKey};
use rand::RngCore;
use serde::{Deserialize, Serialize};
use sha2::{Sha256, Digest};
use std::collections::HashMap;
use std::time::Instant;

use crate::auth::{AuthUser, Claims};
use crate::error::{internal_error, err, ApiError};
use crate::AppState;

// ─── Challenge State ───────────────────────────────────────────

/// In-memory challenge store with 5-minute TTL.
/// Key = base64url-encoded challenge, Value = (user_id, created_at)
pub type ChallengeStore = std::sync::Arc<std::sync::Mutex<HashMap<String, (ChallengeData, Instant)>>>;

#[derive(Clone)]
pub enum ChallengeData {
    Registration { user_id: uuid::Uuid, user_email: String },
    Authentication,
}

pub fn new_challenge_store() -> ChallengeStore {
    std::sync::Arc::new(std::sync::Mutex::new(HashMap::new()))
}

/// Purge expired challenges (>5 min old). Called on each operation.
/// Also enforces a max size of 10,000 entries to prevent memory exhaustion.
fn purge_expired(store: &ChallengeStore) {
    if let Ok(mut map) = store.lock() {
        let now = Instant::now();
        map.retain(|_, (_, created)| now.duration_since(*created).as_secs() < 300);
        // Hard cap to prevent DoS via rapid challenge generation
        if map.len() > 10_000 {
            let excess = map.len() - 5_000;
            let keys_to_remove: Vec<String> = map.keys().take(excess).cloned().collect();
            for k in keys_to_remove { map.remove(&k); }
        }
    }
}

// ─── Request / Response types ──────────────────────────────────

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct PublicKeyCredentialCreationOptions {
    rp: RelyingParty,
    user: PublicKeyUser,
    challenge: String,
    pub_key_cred_params: Vec<PubKeyCredParam>,
    timeout: u64,
    attestation: &'static str,
    authenticator_selection: AuthenticatorSelection,
    exclude_credentials: Vec<CredentialDescriptor>,
}

#[derive(Serialize)]
struct RelyingParty {
    name: String,
    id: String,
}

#[derive(Serialize)]
struct PublicKeyUser {
    id: String,
    name: String,
    #[serde(rename = "displayName")]
    display_name: String,
}

#[derive(Serialize)]
struct PubKeyCredParam {
    #[serde(rename = "type")]
    ty: &'static str,
    alg: i32,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct AuthenticatorSelection {
    authenticator_attachment: Option<&'static str>,
    resident_key: &'static str,
    require_resident_key: bool,
    user_verification: &'static str,
}

#[derive(Serialize)]
struct CredentialDescriptor {
    #[serde(rename = "type")]
    ty: &'static str,
    id: String,
    transports: Option<Vec<String>>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct PublicKeyCredentialRequestOptions {
    challenge: String,
    timeout: u64,
    rp_id: String,
    allow_credentials: Vec<CredentialDescriptor>,
    user_verification: &'static str,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RegisterCompleteRequest {
    pub id: String,
    pub raw_id: String,
    pub response: AttestationResponse,
    pub name: Option<String>,
    pub transports: Option<Vec<String>>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AttestationResponse {
    pub attestation_object: String,
    pub client_data_json: String,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AuthCompleteRequest {
    pub id: String,
    pub raw_id: String,
    pub response: AssertionResponse,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AssertionResponse {
    pub authenticator_data: String,
    pub client_data_json: String,
    pub signature: String,
    pub user_handle: Option<String>,
}

// ─── Helpers ───────────────────────────────────────────────────

fn generate_challenge() -> Vec<u8> {
    let mut challenge = vec![0u8; 32];
    rand::thread_rng().fill_bytes(&mut challenge);
    challenge
}

/// Extract the RP ID for WebAuthn. Prefers server-side BASE_URL config (trusted) over
/// client-controlled headers to prevent RP ID manipulation by attackers.
fn get_rp_id_from_headers(headers: &axum::http::HeaderMap, state: &AppState) -> String {
    // Prefer server-side BASE_URL (trusted, not client-controlled)
    if !state.config.base_url.is_empty() {
        if let Ok(parsed) = url::Url::parse(&state.config.base_url) {
            if let Some(host) = parsed.host_str() {
                return host.to_string();
            }
        }
    }
    // Fall back to Origin header only if BASE_URL is not configured
    if let Some(origin) = headers.get("origin").and_then(|v| v.to_str().ok()) {
        if let Ok(parsed) = url::Url::parse(origin) {
            if let Some(host) = parsed.host_str() {
                return host.to_string();
            }
        }
    }
    // Last resort: Host header
    if let Some(host) = headers.get("host").and_then(|v| v.to_str().ok()) {
        return host.split(':').next().unwrap_or(host).to_string();
    }
    "localhost".to_string()
}

/// Extract the origin URL from the request's Origin header.
/// Extract the origin URL. Prefers server-side BASE_URL over client headers.
fn get_rp_origin_from_headers(headers: &axum::http::HeaderMap, state: &AppState) -> String {
    // Prefer server-side BASE_URL (trusted)
    if !state.config.base_url.is_empty() {
        return state.config.base_url.trim_end_matches('/').to_string();
    }
    // Fall back to Origin header only if BASE_URL not configured
    if let Some(origin) = headers.get("origin").and_then(|v| v.to_str().ok()) {
        return origin.trim_end_matches('/').to_string();
    }
    if let Some(host) = headers.get("host").and_then(|v| v.to_str().ok()) {
        return format!("https://{}", host.split(':').next().unwrap_or(host));
    }
    "https://localhost".to_string()
}

/// Parse the COSE public key from attestation authData.
/// Returns (credential_id, cose_key_cbor, aaguid).
fn parse_auth_data(auth_data: &[u8]) -> Result<(Vec<u8>, Vec<u8>, [u8; 16]), String> {
    // authData structure:
    // 32 bytes: rpIdHash
    // 1 byte:   flags
    // 4 bytes:  signCount
    // if AT flag set (bit 6):
    //   16 bytes: aaguid
    //   2 bytes:  credentialIdLength
    //   N bytes:  credentialId
    //   variable: credentialPublicKey (CBOR)

    if auth_data.len() < 37 {
        return Err("authData too short".to_string());
    }

    let flags = auth_data[32];
    let has_attested_data = (flags & 0x40) != 0;

    if !has_attested_data {
        return Err("No attested credential data in authData".to_string());
    }

    if auth_data.len() < 55 {
        return Err("authData too short for attested data".to_string());
    }

    let mut aaguid = [0u8; 16];
    aaguid.copy_from_slice(&auth_data[37..53]);

    let cred_id_len = u16::from_be_bytes([auth_data[53], auth_data[54]]) as usize;
    if auth_data.len() < 55 + cred_id_len + 1 {
        return Err("authData too short for credential ID".to_string());
    }

    let credential_id = auth_data[55..55 + cred_id_len].to_vec();
    let cose_key_cbor = auth_data[55 + cred_id_len..].to_vec();

    Ok((credential_id, cose_key_cbor, aaguid))
}

/// Parse a COSE key (CBOR map) and extract the P-256 verifying key.
fn parse_cose_p256_key(cbor_bytes: &[u8]) -> Result<VerifyingKey, String> {
    let value: ciborium::Value = ciborium::de::from_reader(cbor_bytes)
        .map_err(|e| format!("CBOR parse error: {e}"))?;

    let map = match value {
        ciborium::Value::Map(m) => m,
        _ => return Err("COSE key is not a map".to_string()),
    };

    // COSE key parameters for EC2/P-256:
    // 1 (kty) = 2 (EC2)
    // 3 (alg) = -7 (ES256)
    // -1 (crv) = 1 (P-256)
    // -2 (x) = bytes (32)
    // -3 (y) = bytes (32)

    let mut x_coord: Option<Vec<u8>> = None;
    let mut y_coord: Option<Vec<u8>> = None;

    for (key, val) in &map {
        let key_int = match key {
            ciborium::Value::Integer(i) => {
                let v: i128 = (*i).into();
                v as i32
            }
            _ => continue,
        };
        match key_int {
            -2 => {
                if let ciborium::Value::Bytes(b) = val {
                    x_coord = Some(b.clone());
                }
            }
            -3 => {
                if let ciborium::Value::Bytes(b) = val {
                    y_coord = Some(b.clone());
                }
            }
            _ => {}
        }
    }

    let x = x_coord.ok_or("Missing x coordinate in COSE key")?;
    let y = y_coord.ok_or("Missing y coordinate in COSE key")?;

    if x.len() != 32 || y.len() != 32 {
        return Err("Invalid coordinate length".to_string());
    }

    // Build uncompressed point: 0x04 || x || y
    let mut point = Vec::with_capacity(65);
    point.push(0x04);
    point.extend_from_slice(&x);
    point.extend_from_slice(&y);

    VerifyingKey::from_sec1_bytes(&point)
        .map_err(|e| format!("Invalid P-256 key: {e}"))
}

// ─── Registration Endpoints ────────────────────────────────────

/// POST /api/auth/passkey/register/begin — Start passkey registration ceremony.
pub async fn register_begin(
    State(state): State<AppState>,
    headers: axum::http::HeaderMap,
    AuthUser(claims): AuthUser,
) -> Result<Json<serde_json::Value>, ApiError> {
    purge_expired(&state.passkey_challenges);

    let rp_id = get_rp_id_from_headers(&headers, &state);

    // Get existing passkeys to exclude
    let existing: Vec<(String, Option<String>)> = sqlx::query_as(
        "SELECT credential_id, transports FROM passkeys WHERE user_id = $1"
    )
    .bind(claims.sub)
    .fetch_all(&state.db)
    .await
    .map_err(|e| internal_error("passkey register", e))?;

    let exclude_credentials: Vec<CredentialDescriptor> = existing.iter().map(|(cid, t)| {
        CredentialDescriptor {
            ty: "public-key",
            id: cid.clone(),
            transports: t.as_ref().map(|ts| ts.split(',').map(String::from).collect()),
        }
    }).collect();

    let challenge = generate_challenge();
    let challenge_b64 = URL_SAFE_NO_PAD.encode(&challenge);

    // Store challenge
    {
        let mut store = state.passkey_challenges.lock().unwrap_or_else(|e| e.into_inner());
        store.insert(challenge_b64.clone(), (
            ChallengeData::Registration {
                user_id: claims.sub,
                user_email: claims.email.clone(),
            },
            Instant::now(),
        ));
    }

    let options = PublicKeyCredentialCreationOptions {
        rp: RelyingParty {
            name: "DockPanel".to_string(),
            id: rp_id,
        },
        user: PublicKeyUser {
            id: URL_SAFE_NO_PAD.encode(claims.sub.as_bytes()),
            name: claims.email.clone(),
            display_name: claims.email.clone(),
        },
        challenge: challenge_b64,
        pub_key_cred_params: vec![
            PubKeyCredParam { ty: "public-key", alg: -7 }, // ES256 (P-256)
        ],
        timeout: 300_000, // 5 minutes
        attestation: "none",
        authenticator_selection: AuthenticatorSelection {
            authenticator_attachment: None,
            resident_key: "preferred",
            require_resident_key: false,
            user_verification: "preferred",
        },
        exclude_credentials,
    };

    Ok(Json(serde_json::json!({ "publicKey": options })))
}

/// POST /api/auth/passkey/register/complete — Finish passkey registration.
pub async fn register_complete(
    State(state): State<AppState>,
    headers: axum::http::HeaderMap,
    AuthUser(claims): AuthUser,
    Json(body): Json<RegisterCompleteRequest>,
) -> Result<Json<serde_json::Value>, ApiError> {
    purge_expired(&state.passkey_challenges);

    let rp_id = get_rp_id_from_headers(&headers, &state);
    let rp_origin = get_rp_origin_from_headers(&headers, &state);

    // Decode clientDataJSON
    let client_data_bytes = URL_SAFE_NO_PAD.decode(&body.response.client_data_json)
        .map_err(|_| err(StatusCode::BAD_REQUEST, "Invalid clientDataJSON encoding"))?;
    let client_data: serde_json::Value = serde_json::from_slice(&client_data_bytes)
        .map_err(|_| err(StatusCode::BAD_REQUEST, "Invalid clientDataJSON"))?;

    // Verify type
    let cd_type = client_data.get("type").and_then(|v| v.as_str()).unwrap_or("");
    if cd_type != "webauthn.create" {
        return Err(err(StatusCode::BAD_REQUEST, "Invalid ceremony type"));
    }

    // Verify challenge
    let cd_challenge = client_data.get("challenge").and_then(|v| v.as_str()).unwrap_or("");
    let challenge_data = {
        let mut store = state.passkey_challenges.lock().unwrap_or_else(|e| e.into_inner());
        store.remove(cd_challenge)
    };
    let challenge_data = challenge_data.ok_or_else(|| err(StatusCode::BAD_REQUEST, "Unknown or expired challenge"))?;

    // Verify it's a registration challenge for this user
    match &challenge_data.0 {
        ChallengeData::Registration { user_id, .. } => {
            if *user_id != claims.sub {
                return Err(err(StatusCode::BAD_REQUEST, "Challenge user mismatch"));
            }
        }
        _ => return Err(err(StatusCode::BAD_REQUEST, "Wrong challenge type")),
    }

    // Verify origin
    let cd_origin = client_data.get("origin").and_then(|v| v.as_str()).unwrap_or("");
    if cd_origin != rp_origin {
        return Err(err(StatusCode::BAD_REQUEST, "Origin mismatch"));
    }

    // Decode attestationObject (CBOR)
    let att_obj_bytes = URL_SAFE_NO_PAD.decode(&body.response.attestation_object)
        .map_err(|_| err(StatusCode::BAD_REQUEST, "Invalid attestationObject encoding"))?;
    let att_obj: ciborium::Value = ciborium::de::from_reader(&att_obj_bytes[..])
        .map_err(|_| err(StatusCode::BAD_REQUEST, "Invalid attestationObject CBOR"))?;

    // Extract authData from attestationObject
    let auth_data_bytes = match &att_obj {
        ciborium::Value::Map(m) => {
            m.iter().find_map(|(k, v)| {
                if let ciborium::Value::Text(key) = k {
                    if key == "authData" {
                        if let ciborium::Value::Bytes(b) = v {
                            return Some(b.clone());
                        }
                    }
                }
                None
            })
        }
        _ => None,
    }.ok_or_else(|| err(StatusCode::BAD_REQUEST, "Missing authData in attestation"))?;

    // Verify rpIdHash
    let expected_rp_hash = Sha256::digest(rp_id.as_bytes());
    if auth_data_bytes.len() < 32 || auth_data_bytes[..32] != expected_rp_hash[..] {
        return Err(err(StatusCode::BAD_REQUEST, "RP ID hash mismatch"));
    }

    // Verify user-present flag
    let flags = auth_data_bytes[32];
    if flags & 0x01 == 0 {
        return Err(err(StatusCode::BAD_REQUEST, "User not present"));
    }

    // Parse credential data
    let (credential_id, cose_key_cbor, aaguid) = parse_auth_data(&auth_data_bytes)
        .map_err(|e| { tracing::warn!("Passkey authData parse error: {e}"); err(StatusCode::BAD_REQUEST, "Invalid attestation data") })?;

    // Verify the COSE key is a valid P-256 key
    parse_cose_p256_key(&cose_key_cbor)
        .map_err(|e| { tracing::warn!("Passkey invalid public key: {e}"); err(StatusCode::BAD_REQUEST, "Invalid credential key") })?;

    let sign_count = u32::from_be_bytes([
        auth_data_bytes[33], auth_data_bytes[34],
        auth_data_bytes[35], auth_data_bytes[36],
    ]) as i64;

    let cred_id_b64 = URL_SAFE_NO_PAD.encode(&credential_id);
    let aaguid_hex = hex::encode(aaguid);
    let transports = body.transports.as_ref().map(|t| t.join(","));
    let name = body.name.as_deref().unwrap_or("My Passkey");

    // Limit: max 10 passkeys per user
    let count: (i64,) = sqlx::query_as("SELECT COUNT(*) FROM passkeys WHERE user_id = $1")
        .bind(claims.sub)
        .fetch_one(&state.db)
        .await
        .map_err(|e| internal_error("passkey count", e))?;
    if count.0 >= 10 {
        return Err(err(StatusCode::BAD_REQUEST, "Maximum 10 passkeys per account"));
    }

    // Store passkey
    let passkey_id: (uuid::Uuid,) = sqlx::query_as(
        "INSERT INTO passkeys (user_id, credential_id, public_key_cbor, sign_count, name, transports, aaguid) \
         VALUES ($1, $2, $3, $4, $5, $6, $7) RETURNING id"
    )
    .bind(claims.sub)
    .bind(&cred_id_b64)
    .bind(&cose_key_cbor)
    .bind(sign_count)
    .bind(name)
    .bind(&transports)
    .bind(&aaguid_hex)
    .fetch_one(&state.db)
    .await
    .map_err(|e| internal_error("store passkey", e))?;

    // Audit log
    crate::services::activity::log_activity(
        &state.db, claims.sub, &claims.email, "passkey.registered",
        Some("passkey"), Some(name), None, None,
    ).await;

    tracing::info!("Passkey registered for user {} ({})", claims.email, cred_id_b64);

    Ok(Json(serde_json::json!({
        "ok": true,
        "id": passkey_id.0,
        "name": name,
    })))
}

// ─── Authentication Endpoints ──────────────────────────────────

/// POST /api/auth/passkey/auth/begin — Start passkey authentication ceremony.
pub async fn auth_begin(
    State(state): State<AppState>,
    headers: axum::http::HeaderMap,
) -> Result<Json<serde_json::Value>, ApiError> {
    purge_expired(&state.passkey_challenges);

    let rp_id = get_rp_id_from_headers(&headers, &state);

    let challenge = generate_challenge();
    let challenge_b64 = URL_SAFE_NO_PAD.encode(&challenge);

    // Store challenge
    {
        let mut store = state.passkey_challenges.lock().unwrap_or_else(|e| e.into_inner());
        store.insert(challenge_b64.clone(), (ChallengeData::Authentication, Instant::now()));
    }

    let options = PublicKeyCredentialRequestOptions {
        challenge: challenge_b64,
        timeout: 300_000,
        rp_id,
        allow_credentials: vec![], // Empty = discoverable credential (resident key)
        user_verification: "preferred",
    };

    Ok(Json(serde_json::json!({ "publicKey": options })))
}

/// POST /api/auth/passkey/auth/complete — Finish passkey authentication, issue JWT.
pub async fn auth_complete(
    State(state): State<AppState>,
    headers: axum::http::HeaderMap,
    Json(body): Json<AuthCompleteRequest>,
) -> Result<(StatusCode, [(axum::http::header::HeaderName, String); 1], Json<serde_json::Value>), ApiError> {
    purge_expired(&state.passkey_challenges);

    let rp_id = get_rp_id_from_headers(&headers, &state);
    let rp_origin = get_rp_origin_from_headers(&headers, &state);

    // Rate limit passkey auth: reuse login_attempts (same IP-based)
    let ip = headers
        .get("x-real-ip")
        .and_then(|v| v.to_str().ok())
        .map(|s| s.trim().to_string())
        .unwrap_or_else(|| "unknown".to_string());
    {
        let mut attempts = state.login_attempts.lock().unwrap_or_else(|e| e.into_inner());
        let now = Instant::now();
        let entry = attempts.entry(ip.clone()).or_default();
        entry.retain(|t| now.duration_since(*t).as_secs() < 900);
        if entry.len() >= 5 {
            return Err(err(StatusCode::TOO_MANY_REQUESTS, "Too many login attempts. Try again in 15 minutes."));
        }
    }

    // Decode clientDataJSON
    let client_data_bytes = URL_SAFE_NO_PAD.decode(&body.response.client_data_json)
        .map_err(|_| err(StatusCode::BAD_REQUEST, "Invalid clientDataJSON encoding"))?;
    let client_data: serde_json::Value = serde_json::from_slice(&client_data_bytes)
        .map_err(|_| err(StatusCode::BAD_REQUEST, "Invalid clientDataJSON"))?;

    // Verify type
    let cd_type = client_data.get("type").and_then(|v| v.as_str()).unwrap_or("");
    if cd_type != "webauthn.get" {
        return Err(err(StatusCode::BAD_REQUEST, "Invalid ceremony type"));
    }

    // Verify challenge
    let cd_challenge = client_data.get("challenge").and_then(|v| v.as_str()).unwrap_or("");
    let challenge_data = {
        let mut store = state.passkey_challenges.lock().unwrap_or_else(|e| e.into_inner());
        store.remove(cd_challenge)
    };
    let _challenge_data = challenge_data
        .ok_or_else(|| err(StatusCode::BAD_REQUEST, "Unknown or expired challenge"))?;

    // Verify origin
    let cd_origin = client_data.get("origin").and_then(|v| v.as_str()).unwrap_or("");
    if cd_origin != rp_origin {
        return Err(err(StatusCode::BAD_REQUEST, "Origin mismatch"));
    }

    // Look up the credential
    let cred_id_b64 = &body.id;
    let passkey: Option<(uuid::Uuid, uuid::Uuid, Vec<u8>, i64)> = sqlx::query_as(
        "SELECT id, user_id, public_key_cbor, sign_count FROM passkeys WHERE credential_id = $1"
    )
    .bind(cred_id_b64)
    .fetch_optional(&state.db)
    .await
    .map_err(|e| internal_error("passkey lookup", e))?;

    let (passkey_id, user_id, cose_key_cbor, stored_count) = passkey
        .ok_or_else(|| {
            // Record failed attempt
            if let Ok(mut map) = state.login_attempts.lock() {
                map.entry(ip.clone()).or_default().push(Instant::now());
            }
            err(StatusCode::UNAUTHORIZED, "Unknown credential")
        })?;

    // Decode authenticator data
    let auth_data_bytes = URL_SAFE_NO_PAD.decode(&body.response.authenticator_data)
        .map_err(|_| err(StatusCode::BAD_REQUEST, "Invalid authenticatorData encoding"))?;

    // Verify rpIdHash
    let expected_rp_hash = Sha256::digest(rp_id.as_bytes());
    if auth_data_bytes.len() < 37 || auth_data_bytes[..32] != expected_rp_hash[..] {
        return Err(err(StatusCode::BAD_REQUEST, "RP ID hash mismatch"));
    }

    // Verify user-present flag
    let flags = auth_data_bytes[32];
    if flags & 0x01 == 0 {
        return Err(err(StatusCode::BAD_REQUEST, "User not present"));
    }

    // Check sign counter (anti-cloning)
    let new_count = u32::from_be_bytes([
        auth_data_bytes[33], auth_data_bytes[34],
        auth_data_bytes[35], auth_data_bytes[36],
    ]) as i64;

    if stored_count > 0 && new_count > 0 && new_count <= stored_count {
        tracing::warn!("Passkey counter regression for credential {cred_id_b64}: stored={stored_count}, new={new_count}");
        return Err(err(StatusCode::UNAUTHORIZED, "Credential may be cloned"));
    }

    // Verify signature: sig over (authData || SHA256(clientDataJSON))
    let client_data_hash = Sha256::digest(&client_data_bytes);
    let mut signed_data = auth_data_bytes.clone();
    signed_data.extend_from_slice(&client_data_hash);

    let verifying_key = parse_cose_p256_key(&cose_key_cbor)
        .map_err(|e| { tracing::error!("Passkey stored key invalid: {e}"); err(StatusCode::INTERNAL_SERVER_ERROR, "Authentication failed") })?;

    let sig_bytes = URL_SAFE_NO_PAD.decode(&body.response.signature)
        .map_err(|_| err(StatusCode::BAD_REQUEST, "Invalid signature encoding"))?;

    // WebAuthn signatures are DER-encoded, convert to fixed-size for p256
    let signature = Signature::from_der(&sig_bytes)
        .map_err(|_| err(StatusCode::UNAUTHORIZED, "Invalid signature format"))?;

    verifying_key.verify(&signed_data, &signature)
        .map_err(|_| {
            if let Ok(mut map) = state.login_attempts.lock() {
                map.entry(ip.clone()).or_default().push(Instant::now());
            }
            err(StatusCode::UNAUTHORIZED, "Signature verification failed")
        })?;

    // Update counter
    sqlx::query("UPDATE passkeys SET sign_count = $1 WHERE id = $2")
        .bind(new_count)
        .bind(passkey_id)
        .execute(&state.db)
        .await
        .ok();

    // Look up the user
    let user: crate::models::User = sqlx::query_as("SELECT * FROM users WHERE id = $1")
        .bind(user_id)
        .fetch_optional(&state.db)
        .await
        .map_err(|e| internal_error("passkey user lookup", e))?
        .ok_or_else(|| err(StatusCode::UNAUTHORIZED, "User not found"))?;

    // Check user is not suspended
    if user.role == "suspended" {
        return Err(err(StatusCode::FORBIDDEN, "Account suspended"));
    }

    // Check approval (field not in User model, query DB directly)
    if let Ok(Some((approved,))) = sqlx::query_as::<_, (bool,)>(
        "SELECT COALESCE(approved, TRUE) FROM users WHERE id = $1"
    ).bind(user.id).fetch_optional(&state.db).await {
        if !approved {
            return Err(err(StatusCode::FORBIDDEN, "Account pending admin approval"));
        }
    }

    // Check lockdown
    if user.role != "admin" && crate::services::security_hardening::is_locked_down(&state.db).await {
        return Err(err(StatusCode::SERVICE_UNAVAILABLE, "System is in lockdown mode"));
    }

    // Clear rate limit
    {
        let mut attempts = state.login_attempts.lock().unwrap_or_else(|e| e.into_inner());
        attempts.remove(&ip);
    }

    // Passkey login bypasses 2FA (the passkey IS the strong second factor)
    let (_token, cookie, jti) = super::auth::issue_session_pub(&state, &user)?;

    // Record session
    let user_agent = headers.get(axum::http::header::USER_AGENT)
        .and_then(|v| v.to_str().ok())
        .map(|s| s.to_string());
    let _ = sqlx::query(
        "INSERT INTO user_sessions (user_id, jti, ip_address, user_agent) VALUES ($1, $2, $3, $4)"
    )
    .bind(user.id)
    .bind(&jti)
    .bind(&ip)
    .bind(&user_agent)
    .execute(&state.db)
    .await;

    // Audit log
    crate::services::activity::log_activity(
        &state.db, user.id, &user.email, "auth.passkey_login",
        None, None, None, Some(&ip),
    ).await;

    crate::services::security_hardening::audit_log(
        &state.db, "passkey_login", Some(&user.email), Some(&ip),
        Some("user"), None, None, None, "info",
    ).await;

    tracing::info!("Passkey login for user {}", user.email);

    Ok((
        StatusCode::OK,
        [(axum::http::header::SET_COOKIE, cookie)],
        Json(serde_json::json!({
            "user": { "id": user.id, "email": user.email, "role": user.role },
        })),
    ))
}

// ─── Passkey Management ────────────────────────────────────────

/// GET /api/auth/passkeys — List the authenticated user's passkeys.
pub async fn list_passkeys(
    State(state): State<AppState>,
    AuthUser(claims): AuthUser,
) -> Result<Json<serde_json::Value>, ApiError> {
    let passkeys: Vec<(uuid::Uuid, String, Option<String>, Option<String>, chrono::DateTime<chrono::Utc>)> =
        sqlx::query_as(
            "SELECT id, name, transports, aaguid, created_at FROM passkeys WHERE user_id = $1 ORDER BY created_at"
        )
        .bind(claims.sub)
        .fetch_all(&state.db)
        .await
        .map_err(|e| internal_error("list passkeys", e))?;

    let items: Vec<serde_json::Value> = passkeys.iter().map(|(id, name, transports, aaguid, created)| {
        serde_json::json!({
            "id": id,
            "name": name,
            "transports": transports,
            "aaguid": aaguid,
            "created_at": created,
        })
    }).collect();

    Ok(Json(serde_json::json!({ "passkeys": items })))
}

/// DELETE /api/auth/passkeys/{id} — Remove a passkey.
pub async fn delete_passkey(
    State(state): State<AppState>,
    AuthUser(claims): AuthUser,
    axum::extract::Path(id): axum::extract::Path<uuid::Uuid>,
) -> Result<Json<serde_json::Value>, ApiError> {
    let result = sqlx::query("DELETE FROM passkeys WHERE id = $1 AND user_id = $2")
        .bind(id)
        .bind(claims.sub)
        .execute(&state.db)
        .await
        .map_err(|e| internal_error("delete passkey", e))?;

    if result.rows_affected() == 0 {
        return Err(err(StatusCode::NOT_FOUND, "Passkey not found"));
    }

    crate::services::activity::log_activity(
        &state.db, claims.sub, &claims.email, "passkey.deleted",
        Some("passkey"), None, None, None,
    ).await;

    Ok(Json(serde_json::json!({ "ok": true })))
}

/// PUT /api/auth/passkeys/{id} — Rename a passkey.
pub async fn rename_passkey(
    State(state): State<AppState>,
    AuthUser(claims): AuthUser,
    axum::extract::Path(id): axum::extract::Path<uuid::Uuid>,
    Json(body): Json<serde_json::Value>,
) -> Result<Json<serde_json::Value>, ApiError> {
    let name = body.get("name").and_then(|v| v.as_str()).unwrap_or("").trim();
    if name.is_empty() || name.len() > 255 {
        return Err(err(StatusCode::BAD_REQUEST, "Name must be 1-255 characters"));
    }

    let result = sqlx::query("UPDATE passkeys SET name = $1 WHERE id = $2 AND user_id = $3")
        .bind(name)
        .bind(id)
        .bind(claims.sub)
        .execute(&state.db)
        .await
        .map_err(|e| internal_error("rename passkey", e))?;

    if result.rows_affected() == 0 {
        return Err(err(StatusCode::NOT_FOUND, "Passkey not found"));
    }

    crate::services::activity::log_activity(
        &state.db, claims.sub, &claims.email, "passkey.renamed",
        Some("passkey"), Some(name), None, None,
    ).await;

    Ok(Json(serde_json::json!({ "ok": true })))
}
