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

use crate::auth::AuthUser;
use crate::error::{internal_error, err, ApiError};
use crate::models::User;
use crate::routes::auth::{require_enrolment_proof, EnrolmentProof};
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
#[allow(dead_code)]
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
#[allow(dead_code)]
pub struct AuthCompleteRequest {
    pub id: String,
    pub raw_id: String,
    pub response: AssertionResponse,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
#[allow(dead_code)]
pub struct AssertionResponse {
    pub authenticator_data: String,
    pub client_data_json: String,
    pub signature: String,
    pub user_handle: Option<String>,
}

// ─── Helpers ───────────────────────────────────────────────────

fn generate_challenge() -> Vec<u8> {
    let mut challenge = vec![0u8; 32];
    rand::rng().fill_bytes(&mut challenge);
    challenge
}

/// Extract the RP ID for WebAuthn.
///
/// ⚠ SECURITY (s435, closing the s429 finding): prefers server-side BASE_URL
/// config (trusted) — but when it's unset (a legitimate, common
/// configuration: `config.rs` deliberately defaults it to empty for
/// "IP-based access", not a misconfiguration to guard against), this falls
/// back to the client-supplied `Origin` header. That fallback is honest to
/// say out loud: it does NOT, by itself, "prevent RP ID manipulation by
/// attackers" — the comment here used to claim exactly that, which s429's
/// completeness critic correctly flagged as overstating what the code does.
/// What actually prevents exploitation is downstream and unconditional,
/// regardless of how `rp_id`/`rp_origin` were derived: `register_complete`/
/// `auth_complete` compare the CLIENT's signed `clientDataJSON.origin` —
/// which a browser sets from the page's real origin, not from any header,
/// and which is covered by the authenticator's own signature — against
/// whatever this function returned. An attacker who can only forge HTTP
/// headers (not actually serve a page at a spoofed origin) fails that
/// comparison every time, which is why this was assessed non-exploitable
/// as reported rather than fixed as an emergency.
///
/// What DOES fail closed now, and didn't before: if a request has NEITHER
/// an `Origin` NOR a `Host` header, there is no signal left to derive
/// anything from — no legitimate browser-driven WebAuthn ceremony omits
/// both — so this refuses outright instead of falling back to a hardcoded
/// `"localhost"` that satisfied nothing but let the ceremony proceed anyway.
fn get_rp_id_from_headers(headers: &axum::http::HeaderMap, state: &AppState) -> Result<String, ApiError> {
    // Prefer server-side BASE_URL (trusted, not client-controlled)
    if !state.config.base_url.is_empty() {
        if let Ok(parsed) = url::Url::parse(&state.config.base_url) {
            if let Some(host) = parsed.host_str() {
                return Ok(host.to_string());
            }
        }
    }
    // Fall back to Origin header only if BASE_URL is not configured
    if let Some(origin) = headers.get("origin").and_then(|v| v.to_str().ok()) {
        if let Ok(parsed) = url::Url::parse(origin) {
            if let Some(host) = parsed.host_str() {
                return Ok(host.to_string());
            }
        }
    }
    // Last resort: Host header
    if let Some(host) = headers.get("host").and_then(|v| v.to_str().ok()) {
        return Ok(host.split(':').next().unwrap_or(host).to_string());
    }
    Err(err(StatusCode::BAD_REQUEST, "Cannot determine relying party — request has neither an Origin nor a Host header"))
}

/// Extract the origin URL for WebAuthn's `clientDataJSON.origin` comparison.
/// Same trust model and the same s435 fail-closed fix as
/// [`get_rp_id_from_headers`] — read that function's doc for the full account.
fn get_rp_origin_from_headers(headers: &axum::http::HeaderMap, state: &AppState) -> Result<String, ApiError> {
    // Prefer server-side BASE_URL (trusted)
    if !state.config.base_url.is_empty() {
        return Ok(state.config.base_url.trim_end_matches('/').to_string());
    }
    // Fall back to Origin header only if BASE_URL not configured
    if let Some(origin) = headers.get("origin").and_then(|v| v.to_str().ok()) {
        return Ok(origin.trim_end_matches('/').to_string());
    }
    if let Some(host) = headers.get("host").and_then(|v| v.to_str().ok()) {
        return Ok(format!("https://{}", host.split(':').next().unwrap_or(host)));
    }
    Err(err(StatusCode::BAD_REQUEST, "Cannot determine relying party — request has neither an Origin nor a Host header"))
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

/// Whether an assertion's signature counter says the authenticator was cloned.
///
/// WebAuthn L2 §7.2 applies the comparison when *either* side is non-zero. This
/// panel tested both with `&&` from `adce010` until v2.86.0, which exempted the
/// one shape that matters: a stored counter above zero presented with **zero**.
/// That is the cheapest possible forgery — a cloner who cannot match the real
/// device's count sends 0 and is waved through, and the accepted assertion then
/// writes 0 back, so the real device's next login looks like a fresh increment.
/// Authenticators with no counter (synced passkeys, notably) report zero on both
/// sides forever; `||` still exempts them, because `new <= stored` is `0 <= 0`
/// only when nothing has ever incremented.
///
/// A free function, and public, so a unit test can hold the whole truth table
/// rather than a suite grepping the expression that computes it.
pub fn counter_regressed(stored: i64, new: i64) -> bool {
    (stored > 0 || new > 0) && new <= stored
}

// ─── Registration Endpoints ────────────────────────────────────

/// POST /api/auth/passkey/register/begin — Start passkey registration ceremony.
///
/// Guarded by [`require_enrolment_proof`]: a session alone is not enough to
/// plant a credential that outlives every reset this panel offers. The proof is
/// checked HERE rather than at `register_complete` because this is the only
/// place a registration challenge is ever constructed, and asking again after
/// the browser's own WebAuthn dialog would prompt the user twice for one act.
///
/// The body is optional by design — the first request carries no proof, and the
/// refusal it earns is what tells the card which fields to collect. A bare
/// `Json<T>` would reject a request with no `Content-Type` with a 415 raised
/// before this handler runs, carrying no code the client could branch on.
pub async fn register_begin(
    State(state): State<AppState>,
    headers: axum::http::HeaderMap,
    AuthUser(claims): AuthUser,
    body: Option<Json<EnrolmentProof>>,
) -> Result<Json<serde_json::Value>, ApiError> {
    purge_expired(&state.passkey_challenges);

    let user: User = sqlx::query_as("SELECT * FROM users WHERE id = $1")
        .bind(claims.sub)
        .fetch_one(&state.db)
        .await
        .map_err(|e| internal_error("passkey register", e))?;

    let proof = body.map(|Json(p)| p).unwrap_or_default();

    if let Err(e) = require_enrolment_proof(&state, &claims, &user, &proof).await {
        // A rejected proof is worth a trail; a bare challenge is not an event.
        if proof.code.is_some() || proof.current_password.is_some() {
            crate::services::activity::log_activity(
                &state.db,
                claims.sub,
                &claims.email,
                "passkey.reauth_failed",
                Some("passkey"),
                None,
                None,
                None,
            )
            .await;
        }
        return Err(e);
    }

    let rp_id = get_rp_id_from_headers(&headers, &state)?;

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

    let rp_id = get_rp_id_from_headers(&headers, &state)?;
    let rp_origin = get_rp_origin_from_headers(&headers, &state)?;

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

    // Verify rpIdHash.
    //
    // ⚠ The bound is 37, not 32, and the difference is a panic. `authData` is
    // rpIdHash(32) ‖ flags(1) ‖ signCount(4), so admitting a 32-byte buffer here
    // lets the very next line index `[32]` out of bounds — and the payload is
    // attacker-chosen, because SHA256(rp_id) is public. The twin in
    // `auth_complete` has always used 37; the asymmetry between the two doors
    // WAS the bug. Keep them identical.
    let expected_rp_hash = Sha256::digest(rp_id.as_bytes());
    if auth_data_bytes.len() < 37 || auth_data_bytes[..32] != expected_rp_hash[..] {
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

    // Whether THIS ceremony's authenticator performed user verification (PIN,
    // fingerprint, face) rather than merely user presence (a touch). Recorded
    // once, here, because UV is a property of the ceremony — nothing else
    // this row holds can answer it later. `auth_complete` re-checks this bit
    // on every future login for this credential, but ONLY if it's `true`
    // here: see the migration and `auth_complete` for the grandfathering
    // reasoning.
    let uv_capable = (flags & 0x04) != 0;

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
        "INSERT INTO passkeys (user_id, credential_id, public_key_cbor, sign_count, name, transports, aaguid, uv_capable) \
         VALUES ($1, $2, $3, $4, $5, $6, $7, $8) RETURNING id"
    )
    .bind(claims.sub)
    .bind(&cred_id_b64)
    .bind(&cose_key_cbor)
    .bind(sign_count)
    .bind(name)
    .bind(&transports)
    .bind(&aaguid_hex)
    .bind(uv_capable)
    .fetch_one(&state.db)
    .await
    .map_err(|e| internal_error("store passkey", e))?;

    // Audit log
    crate::services::activity::log_activity(
        &state.db, claims.sub, &claims.email, "passkey.registered",
        Some("passkey"), Some(name), None, None,
    ).await;

    // Tell the OWNER, not the admins. A planted credential is durable mainly
    // because it is silent: the activity log is not a surface anyone watches,
    // and no reset removes the row once it exists. `None` here would notify
    // every administrator — that is, everyone except the person affected.
    crate::services::notifications::notify_panel(
        &state.db,
        Some(claims.sub),
        "New passkey added",
        &format!(
            "A passkey named \"{name}\" was added to your account. If this wasn't you, \
             remove it in My Account and change your password."
        ),
        "warning",
        "security",
        Some("/account"),
    )
    .await;

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
    // Refuse before handing out a challenge, so an excluded address gets the same
    // answer here as at the password door rather than a usable WebAuthn challenge.
    super::auth::enforce_panel_ip_allowlist(&state.db, &headers).await?;

    purge_expired(&state.passkey_challenges);

    let rp_id = get_rp_id_from_headers(&headers, &state)?;

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

    // The panel IP allowlist gates the panel, not one handler: this door mints the
    // same session cookie as the password door, so it owes the same check.
    super::auth::enforce_panel_ip_allowlist(&state.db, &headers).await?;

    let rp_id = get_rp_id_from_headers(&headers, &state)?;
    let rp_origin = get_rp_origin_from_headers(&headers, &state)?;

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
    let challenge_data = challenge_data
        .ok_or_else(|| err(StatusCode::BAD_REQUEST, "Unknown or expired challenge"))?;

    // The challenge must be one THIS door issued. No attack is known through the
    // gap this closes, and the reasoning is worth keeping so nobody re-opens it
    // as dead weight: a registration challenge carries a user binding, but this
    // door never reads it — the account comes from `credential_id` → the
    // `passkeys` row below — so presenting one buys its holder exactly the
    // session they could already mint. What made it worth closing is symmetry.
    // `register_complete` has rejected the wrong variant since the feature
    // landed; only this side trusted the store's key alone. A third variant
    // added later would otherwise be accepted here by default, and the next
    // reader would have to re-derive which of the two doors checks.
    if !matches!(challenge_data.0, ChallengeData::Authentication) {
        return Err(err(StatusCode::BAD_REQUEST, "Wrong challenge type"));
    }

    // Verify origin
    let cd_origin = client_data.get("origin").and_then(|v| v.as_str()).unwrap_or("");
    if cd_origin != rp_origin {
        return Err(err(StatusCode::BAD_REQUEST, "Origin mismatch"));
    }

    // Look up the credential
    let cred_id_b64 = &body.id;
    let passkey: Option<(uuid::Uuid, uuid::Uuid, Vec<u8>, i64, bool)> = sqlx::query_as(
        "SELECT id, user_id, public_key_cbor, sign_count, uv_capable FROM passkeys WHERE credential_id = $1"
    )
    .bind(cred_id_b64)
    .fetch_optional(&state.db)
    .await
    .map_err(|e| internal_error("passkey lookup", e))?;

    let (passkey_id, user_id, cose_key_cbor, stored_count, uv_capable) = passkey
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

    // Read the signature counter. The regression test it feeds runs AFTER the
    // signature is verified — see the block below for why that order is the
    // whole point.
    let new_count = u32::from_be_bytes([
        auth_data_bytes[33], auth_data_bytes[34],
        auth_data_bytes[35], auth_data_bytes[36],
    ]) as i64;

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

    // Require user verification back from any credential that proved it could
    // give one. Deliberately AFTER the signature verify, for the same reason
    // the counter check below is: `flags` lives inside `auth_data_bytes`, which
    // is exactly what the signature covers, so trusting the UV bit before the
    // signature is checked would let an attacker learn this credential's UV
    // requirement from forged, unauthenticated assertions.
    //
    // `uv_capable` is read from the credential's OWN registration ceremony
    // (`register_complete`), never assumed — a credential registered before
    // this column existed, or one whose authenticator never demonstrated a
    // PIN/biometric check at enrolment, has `uv_capable = false` and is left
    // exactly as possession-only as it always was. This is the grandfathering
    // the migration comment describes: nothing that worked yesterday stops
    // working today, and only a credential that has ALREADY proven it can
    // verify its holder is now asked to prove it again.
    if uv_capable && (flags & 0x04) == 0 {
        tracing::warn!("Passkey UV requirement not met for credential {cred_id_b64}");

        if let Ok(mut map) = state.login_attempts.lock() {
            map.entry(ip.clone()).or_default().push(Instant::now());
        }

        let actor_email: Option<String> = sqlx::query_scalar("SELECT email FROM users WHERE id = $1")
            .bind(user_id)
            .fetch_optional(&state.db)
            .await
            .ok()
            .flatten();
        crate::services::security_hardening::audit_log(
            &state.db,
            "passkey.uv_not_provided",
            actor_email.as_deref(),
            Some(&ip),
            Some("passkey"),
            Some(cred_id_b64),
            None,
            None,
            "warning",
        )
        .await;

        return Err(err(StatusCode::UNAUTHORIZED, "This passkey requires verification (PIN, fingerprint, or face) that wasn't provided"));
    }

    // Check sign counter (anti-cloning) — deliberately AFTER the signature, which
    // is where WebAuthn L2 §7.2 puts it and what makes this refusal mean anything.
    //
    // Above the verify, the branch fired on forged `authData` carrying a garbage
    // signature. Everything it needed was public: `auth_begin` hands out
    // challenges to anyone, `rpIdHash` is SHA256 of a name printed in the URL bar,
    // and the UP bit is a constant — leaving only the credential id, which a
    // passive observer of one login has. So the single refusal in this file that
    // says "clone" was reachable by a stranger, and it returned WITHOUT recording
    // an attempt (unlike both siblings above), making it an unthrottled probe.
    // That ordering is also why the detector stayed mute for so long: wiring an
    // alert to a branch an outsider can trigger builds a fabricated-alert writer,
    // so the reorder had to come first and the alert second.
    if counter_regressed(stored_count, new_count) {
        tracing::warn!("Passkey counter regression for credential {cred_id_b64}: stored={stored_count}, new={new_count}");

        if let Ok(mut map) = state.login_attempts.lock() {
            map.entry(ip.clone()).or_default().push(Instant::now());
        }

        // `audit_log`, never `record_suspicious_event`: that one auto-activates
        // system-wide lockdown at five events in ten minutes, so routing a
        // credential failure through it would let a cloned key lock every
        // non-admin out of the panel by simply continuing to fail.
        let actor_email: Option<String> = sqlx::query_scalar("SELECT email FROM users WHERE id = $1")
            .bind(user_id)
            .fetch_optional(&state.db)
            .await
            .ok()
            .flatten();
        crate::services::security_hardening::audit_log(
            &state.db,
            "passkey.counter_regression",
            actor_email.as_deref(),
            Some(&ip),
            Some("passkey"),
            Some(cred_id_b64),
            Some(&format!("stored={stored_count}, presented={new_count}")),
            None,
            "warning",
        )
        .await;

        return Err(err(StatusCode::UNAUTHORIZED, "Credential may be cloned"));
    }

    // Update counter. The `AND $1 > sign_count` closes the window between the
    // read above and this write: with two assertions in flight the lower one
    // could otherwise land last and walk the stored counter backwards, which is
    // the state `counter_regressed` exists to notice.
    sqlx::query("UPDATE passkeys SET sign_count = $1 WHERE id = $2 AND $1 > sign_count")
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

    // Passkey login skips the separate TOTP step (the passkey IS the second
    // factor) — but as of `uv_capable`, that claim is now enforced rather than
    // assumed: a credential that has proven it can perform user verification
    // must present it again on every login (the check above), and only a
    // possession-only credential (never proven UV-capable) still logs in on
    // presence alone, same as before this fix.
    let (_token, cookie, jti) = super::auth::issue_session_pub(&state, &user, &headers)?;

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
    let passkeys: Vec<(uuid::Uuid, String, Option<String>, Option<String>, chrono::DateTime<chrono::Utc>, bool)> =
        sqlx::query_as(
            "SELECT id, name, transports, aaguid, created_at, uv_capable FROM passkeys WHERE user_id = $1 ORDER BY created_at"
        )
        .bind(claims.sub)
        .fetch_all(&state.db)
        .await
        .map_err(|e| internal_error("list passkeys", e))?;

    let items: Vec<serde_json::Value> = passkeys.iter().map(|(id, name, transports, aaguid, created, uv_capable)| {
        serde_json::json!({
            "id": id,
            "name": name,
            "transports": transports,
            "aaguid": aaguid,
            "created_at": created,
            "uvCapable": uv_capable,
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

// ─── Tests ─────────────────────────────────────────────────────────────
//
// This file had no tests at all until v2.86.0 — origin, rpIdHash, user-presence,
// the counter and the signature were five live checks with zero assertions
// anywhere in the repository. What follows covers the parts that are pure
// functions of their input; the properties that live inside the axum handlers
// (guard ORDER, and the byte bound that used to panic) are pinned in
// `tests/passkey-ceremony-pin-e2e.sh`, because they are facts about arrangement
// rather than about a return value.

#[cfg(test)]
mod tests {
    use super::*;

    // ── counter_regressed: the whole truth table ────────────────────────
    //
    // Written as a table rather than as separate asserts so a future edit to the
    // predicate cannot quietly satisfy some rows and drop others — every
    // (stored, new) class is named here with the reason it holds.

    #[test]
    fn counter_never_incremented_is_not_a_clone() {
        // Authenticators without a counter — synced passkeys are the common case —
        // report zero forever. Flagging them would break the majority of real
        // credentials, which is why the rule is not a bare `new <= stored`.
        assert!(!counter_regressed(0, 0));
    }

    #[test]
    fn first_increment_from_zero_is_accepted() {
        assert!(!counter_regressed(0, 1));
        assert!(!counter_regressed(0, 42));
        assert!(!counter_regressed(0, i64::from(u32::MAX)));
    }

    #[test]
    fn normal_increment_is_accepted() {
        assert!(!counter_regressed(1, 2));
        assert!(!counter_regressed(41, 42));
        assert!(!counter_regressed(i64::from(u32::MAX) - 1, i64::from(u32::MAX)));
    }

    #[test]
    fn zero_presented_against_a_live_counter_is_a_clone() {
        // THE REGRESSION THIS RELEASE CLOSES. The old `&&` form exempted every
        // one of these, so the cheapest forgery — send 0, match nothing — was
        // waved through, and the accepted assertion wrote 0 back.
        assert!(counter_regressed(1, 0));
        assert!(counter_regressed(42, 0));
        assert!(counter_regressed(i64::from(u32::MAX), 0));
    }

    #[test]
    fn repeat_or_rewind_is_a_clone() {
        assert!(counter_regressed(42, 42)); // replay of the same assertion
        assert!(counter_regressed(42, 41)); // rewind
        assert!(counter_regressed(42, 1));
    }

    // ── parse_auth_data: the length ladder ──────────────────────────────

    fn auth_data(flags: u8, extra: &[u8]) -> Vec<u8> {
        let mut v = vec![0xAAu8; 32]; // rpIdHash
        v.push(flags);
        v.extend_from_slice(&[0, 0, 0, 1]); // signCount
        v.extend_from_slice(extra);
        v
    }

    #[test]
    fn auth_data_shorter_than_the_fixed_header_is_rejected() {
        for len in [0usize, 1, 31, 32, 36] {
            assert!(
                parse_auth_data(&vec![0u8; len]).is_err(),
                "a {len}-byte authData must not parse: the fixed header is 37 bytes, \
                 and admitting a short one is what let register_complete index [32] \
                 out of bounds before v2.86.0"
            );
        }
    }

    #[test]
    fn attested_credential_data_flag_is_required() {
        // 0x01 is user-present; the attested-credential-data bit is 0x40. A
        // registration whose authData carries no credential must not parse.
        assert!(parse_auth_data(&auth_data(0x01, &[])).is_err());
    }

    #[test]
    fn attested_flag_without_the_payload_is_rejected() {
        // AT set, but nothing after the header, so aaguid/credIdLen are absent.
        assert!(parse_auth_data(&auth_data(0x41, &[])).is_err());
    }

    #[test]
    fn credential_id_length_beyond_the_buffer_is_rejected() {
        // Claims a 255-byte credential id and supplies four bytes of it. The
        // length is attacker-chosen, so this is the slice that must not panic.
        let mut extra = vec![0xBBu8; 16]; // aaguid
        extra.extend_from_slice(&[0x00, 0xFF]); // credentialIdLength = 255
        extra.extend_from_slice(&[1, 2, 3, 4]);
        assert!(parse_auth_data(&auth_data(0x41, &extra)).is_err());
    }

    #[test]
    fn well_formed_attested_data_parses_and_splits_correctly() {
        let mut extra = vec![0xBBu8; 16]; // aaguid
        extra.extend_from_slice(&[0x00, 0x04]); // credentialIdLength = 4
        extra.extend_from_slice(&[9, 8, 7, 6]); // credentialId
        extra.extend_from_slice(&[0xA0]); // trailing COSE bytes (empty CBOR map)

        let (cred_id, cose, aaguid) = parse_auth_data(&auth_data(0x41, &extra)).expect("must parse");
        assert_eq!(cred_id, vec![9, 8, 7, 6]);
        assert_eq!(aaguid, [0xBBu8; 16]);
        assert_eq!(cose, vec![0xA0], "the COSE key is everything past the credential id");
    }

    // ── parse_cose_p256_key ─────────────────────────────────────────────

    /// The standard P-256 base point — a known-valid (x, y) that needs no RNG.
    const P256_GX: [u8; 32] = [
        0x6B, 0x17, 0xD1, 0xF2, 0xE1, 0x2C, 0x42, 0x47, 0xF8, 0xBC, 0xE6, 0xE5,
        0x63, 0xA4, 0x40, 0xF2, 0x77, 0x03, 0x7D, 0x81, 0x2D, 0xEB, 0x33, 0xA0,
        0xF4, 0xA1, 0x39, 0x45, 0xD8, 0x98, 0xC2, 0x96,
    ];
    const P256_GY: [u8; 32] = [
        0x4F, 0xE3, 0x42, 0xE2, 0xFE, 0x1A, 0x7F, 0x9B, 0x8E, 0xE7, 0xEB, 0x4A,
        0x7C, 0x0F, 0x9E, 0x16, 0x2B, 0xCE, 0x33, 0x57, 0x6B, 0x31, 0x5E, 0xCE,
        0xCB, 0xB6, 0x40, 0x68, 0x37, 0xBF, 0x51, 0xF5,
    ];

    fn cose_key(x: &[u8], y: &[u8]) -> Vec<u8> {
        let value = ciborium::Value::Map(vec![
            (ciborium::Value::Integer(1.into()), ciborium::Value::Integer(2.into())),
            (ciborium::Value::Integer((-1i32).into()), ciborium::Value::Integer(1.into())),
            (ciborium::Value::Integer((-2i32).into()), ciborium::Value::Bytes(x.to_vec())),
            (ciborium::Value::Integer((-3i32).into()), ciborium::Value::Bytes(y.to_vec())),
        ]);
        let mut out = Vec::new();
        ciborium::ser::into_writer(&value, &mut out).expect("serialise");
        out
    }

    #[test]
    fn a_valid_p256_cose_key_parses() {
        assert!(parse_cose_p256_key(&cose_key(&P256_GX, &P256_GY)).is_ok());
    }

    #[test]
    fn cose_key_that_is_not_a_map_is_rejected() {
        let mut out = Vec::new();
        ciborium::ser::into_writer(&ciborium::Value::Integer(7.into()), &mut out).unwrap();
        assert!(parse_cose_p256_key(&out).is_err());
    }

    #[test]
    fn cose_key_missing_a_coordinate_is_rejected() {
        let value = ciborium::Value::Map(vec![(
            ciborium::Value::Integer((-2i32).into()),
            ciborium::Value::Bytes(P256_GX.to_vec()),
        )]);
        let mut out = Vec::new();
        ciborium::ser::into_writer(&value, &mut out).unwrap();
        assert!(parse_cose_p256_key(&out).is_err());
    }

    #[test]
    fn cose_key_with_short_coordinates_is_rejected() {
        assert!(parse_cose_p256_key(&cose_key(&[1u8; 31], &[2u8; 31])).is_err());
    }

    #[test]
    fn a_point_not_on_the_curve_is_rejected() {
        // Correct lengths, valid CBOR, but not a curve point — the check that
        // stops a caller storing a key no signature could ever verify against.
        assert!(parse_cose_p256_key(&cose_key(&[0xAAu8; 32], &[0xBBu8; 32])).is_err());
    }

    #[test]
    fn garbage_bytes_are_rejected_rather_than_panicking() {
        assert!(parse_cose_p256_key(&[]).is_err());
        assert!(parse_cose_p256_key(&[0xFF, 0xFF, 0xFF]).is_err());
    }
}
