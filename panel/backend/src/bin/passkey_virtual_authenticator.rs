//! A standalone WebAuthn client+authenticator, independent of the server's own
//! implementation, that drives the REAL passkey HTTP ceremony end to end
//! against a running dockpanel-api — the harness §G4 of
//! `tests/passkey-ceremony-pin-e2e.sh` named as the missing proof for the
//! `uv_capable` fix: "needs a `uv` column and a virtual-authenticator harness
//! to prove the flip."
//!
//! It does NOT call any internal server function or read the database
//! directly — it only holds P-256 keypairs and constructs the exact bytes a
//! real browser + hardware authenticator would produce (CBOR attestation
//! object, WebAuthn authenticatorData, DER-signed assertions), then talks to
//! the public API. That is what makes this a proof of the *enforcement*
//! rather than of the code that enforces it: if `auth_complete` stopped
//! checking the UV bit, this would start passing a case it must fail, with no
//! shared code path to also be wrong in the same way.
//!
//! Scenarios proven, in order:
//! 1. Register credential A with the UV flag set → the row must be
//!    `uv_capable = true` (proven indirectly: a later login without UV must
//!    be refused).
//! 2. Log in as credential A WITHOUT presenting UV → must be REJECTED. This
//!    is the enforcement itself; without the fix in `auth_complete`, this
//!    step is what would wrongly succeed.
//! 3. Log in as credential A WITH UV presented → must succeed.
//! 4. Register credential B WITHOUT the UV flag (a legacy-shaped,
//!    possession-only key) → log in WITHOUT UV → must still succeed. This is
//!    the grandfathering guarantee: a credential that never proved it could
//!    verify its holder is not newly locked out by this ship.
//!
//! Run via `tests/passkey-uv-enforcement-e2e.sh`, which builds and invokes
//! this binary against a live local instance — matching how
//! `nginx-headers-pin-e2e.sh`/`update-rollback-pin-e2e.sh` are this repo's
//! other EXECUTE-class pin suites, per `reference_dockpanel_ops`.

use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine};
use p256::ecdsa::{signature::Signer, Signature, SigningKey, VerifyingKey};
// `p256`'s `ecdsa` 0.16 pulls in `rand_core` 0.6 (its `CryptoRngCore` bound),
// which is a DIFFERENT major version than the `rand = "0.9"` crate this
// backend otherwise uses — `rand::rng()`'s `ThreadRng` does not implement the
// 0.6 traits. Use p256's own re-exported RNG for key generation; `rand`'s
// `RngCore` is still fine for filling plain credential-id bytes below.
use p256::elliptic_curve::rand_core::OsRng as P256OsRng;
use rand::RngCore;
use sha2::{Digest, Sha256};

const UP: u8 = 0x01; // user present
const UV: u8 = 0x04; // user verified (PIN / biometric)
const AT: u8 = 0x40; // attested credential data included

struct Ctx {
    base: String,
    origin: String,
    client: reqwest::Client,
    token: String,
}

impl Ctx {
    fn auth_headers(&self) -> reqwest::header::HeaderMap {
        let mut h = reqwest::header::HeaderMap::new();
        h.insert("Cookie", format!("token={}", self.token).parse().unwrap());
        h.insert("X-Requested-With", "XMLHttpRequest".parse().unwrap());
        h.insert("Origin", self.origin.parse().unwrap());
        h
    }

    fn anon_headers(&self) -> reqwest::header::HeaderMap {
        let mut h = reqwest::header::HeaderMap::new();
        h.insert("Origin", self.origin.parse().unwrap());
        h
    }
}

/// One virtual security key: a real P-256 keypair plus a fixed credential id,
/// so the same "hardware" can register once and be asked to sign again later.
struct VirtualAuthenticator {
    signing_key: SigningKey,
    credential_id: Vec<u8>,
}

impl VirtualAuthenticator {
    fn new() -> Self {
        let signing_key = SigningKey::random(&mut P256OsRng);
        let mut credential_id = vec![0u8; 16];
        rand::rng().fill_bytes(&mut credential_id);
        Self { signing_key, credential_id }
    }

    fn cred_id_b64(&self) -> String {
        URL_SAFE_NO_PAD.encode(&self.credential_id)
    }

    /// COSE_Key CBOR for this authenticator's public key — kty=EC2(2),
    /// crv=P-256(1), alg=ES256(-7), plus the x/y coordinates
    /// `parse_cose_p256_key` actually reads.
    fn cose_public_key(&self) -> Vec<u8> {
        let point = VerifyingKey::from(&self.signing_key).to_encoded_point(false);
        let x = point.x().expect("uncompressed point has x").to_vec();
        let y = point.y().expect("uncompressed point has y").to_vec();
        let value = ciborium::Value::Map(vec![
            (ciborium::Value::Integer(1.into()), ciborium::Value::Integer(2.into())),
            (ciborium::Value::Integer(3.into()), ciborium::Value::Integer((-7i32).into())),
            (ciborium::Value::Integer((-1i32).into()), ciborium::Value::Integer(1.into())),
            (ciborium::Value::Integer((-2i32).into()), ciborium::Value::Bytes(x)),
            (ciborium::Value::Integer((-3i32).into()), ciborium::Value::Bytes(y)),
        ]);
        let mut out = Vec::new();
        ciborium::ser::into_writer(&value, &mut out).expect("serialise COSE key");
        out
    }

    /// Registration-shaped authenticatorData: rpIdHash‖flags‖signCount‖aaguid‖
    /// credIdLen‖credId‖COSE key. `uv` selects whether the UV flag is set —
    /// this is the ONLY thing distinguishing the two credentials in this file.
    fn registration_auth_data(&self, rp_id_hash: &[u8; 32], uv: bool, sign_count: u32) -> Vec<u8> {
        let mut flags = UP | AT;
        if uv { flags |= UV; }
        let mut out = Vec::new();
        out.extend_from_slice(rp_id_hash);
        out.push(flags);
        out.extend_from_slice(&sign_count.to_be_bytes());
        out.extend_from_slice(&[0u8; 16]); // aaguid — value is unchecked by the server
        out.extend_from_slice(&(self.credential_id.len() as u16).to_be_bytes());
        out.extend_from_slice(&self.credential_id);
        out.extend_from_slice(&self.cose_public_key());
        out
    }

    /// Assertion-shaped authenticatorData: just the fixed 37-byte header — no
    /// attested-credential-data block, matching a real `.get()` ceremony.
    fn assertion_auth_data(&self, rp_id_hash: &[u8; 32], uv: bool, sign_count: u32) -> Vec<u8> {
        let mut flags = UP;
        if uv { flags |= UV; }
        let mut out = Vec::new();
        out.extend_from_slice(rp_id_hash);
        out.push(flags);
        out.extend_from_slice(&sign_count.to_be_bytes());
        out
    }

    fn sign(&self, auth_data: &[u8], client_data_bytes: &[u8]) -> Vec<u8> {
        let mut signed = auth_data.to_vec();
        signed.extend_from_slice(&Sha256::digest(client_data_bytes));
        let sig: Signature = self.signing_key.sign(&signed);
        sig.to_der().as_bytes().to_vec()
    }
}

fn client_data_json(ty: &str, challenge: &str, origin: &str) -> Vec<u8> {
    serde_json::json!({ "type": ty, "challenge": challenge, "origin": origin })
        .to_string()
        .into_bytes()
}

async fn login(ctx_base: &str, origin: &str, email: &str, password: &str) -> Result<String, String> {
    let client = reqwest::Client::builder().redirect(reqwest::redirect::Policy::none()).build().unwrap();
    let resp = client
        .post(format!("{ctx_base}/api/auth/login"))
        .header("Content-Type", "application/json")
        .header("Origin", origin)
        .json(&serde_json::json!({ "email": email, "password": password }))
        .send()
        .await
        .map_err(|e| e.to_string())?;

    if !resp.status().is_success() {
        return Err(format!("login failed: HTTP {}", resp.status()));
    }

    let set_cookie = resp
        .headers()
        .get_all("set-cookie")
        .iter()
        .find_map(|v| {
            let s = v.to_str().ok()?;
            s.strip_prefix("token=").map(|rest| rest.split(';').next().unwrap_or(rest).to_string())
        })
        .ok_or("no token cookie in login response")?;

    Ok(set_cookie)
}

async fn register_begin(ctx: &Ctx, password: &str) -> Result<String, String> {
    let resp = ctx
        .client
        .post(format!("{}/api/auth/passkey/register/begin", ctx.base))
        .headers(ctx.auth_headers())
        .header("Content-Type", "application/json")
        .json(&serde_json::json!({ "current_password": password }))
        .send()
        .await
        .map_err(|e| e.to_string())?;

    if !resp.status().is_success() {
        return Err(format!("register/begin failed: HTTP {}", resp.status()));
    }
    let body: serde_json::Value = resp.json().await.map_err(|e| e.to_string())?;
    body["publicKey"]["challenge"]
        .as_str()
        .map(String::from)
        .ok_or_else(|| "register/begin: no challenge in response".to_string())
}

fn rp_id_hash_for(origin_host: &str) -> [u8; 32] {
    Sha256::digest(origin_host.as_bytes()).into()
}

#[tokio::main]
async fn main() {
    let base = std::env::var("DOCKPANEL_TEST_BASE_URL").unwrap_or_else(|_| "http://127.0.0.1:3080".into());
    let email = std::env::var("DOCKPANEL_TEST_EMAIL").unwrap_or_else(|_| "admin@dockpanel.dev".into());
    let password = std::env::var("DOCKPANEL_TEST_PASSWORD").unwrap_or_else(|_| "testpassword".into());
    // Matches get_rp_id_from_headers' Origin-header fallback (this box has no
    // BASE_URL configured) — an explicit Origin makes the derivation
    // deterministic instead of depending on how the HTTP client fills Host.
    let origin = "https://127.0.0.1".to_string();
    let rp_id_hash = rp_id_hash_for("127.0.0.1");

    let mut pass = 0u32;
    let mut fail = 0u32;
    let mut ok = |msg: &str| { pass += 1; println!("  \x1b[32m\u{2713}\x1b[0m {msg}"); };
    let mut bad = |msg: &str| { fail += 1; println!("  \x1b[31m\u{2717}\x1b[0m {msg}"); };

    println!("==============================================");
    println!("  Passkey UV enforcement — virtual authenticator");
    println!("==============================================\n");

    let token = match login(&base, &origin, &email, &password).await {
        Ok(t) => t,
        Err(e) => { eprintln!("FATAL: could not log in ({e}) — is dockpanel-api running at {base}?"); std::process::exit(2); }
    };
    let ctx = Ctx { base: base.clone(), origin: origin.clone(), client: reqwest::Client::new(), token };

    // This box is a real (demo) admin account, so every credential this
    // binary plants is removed again before exit, success or failure — see
    // the cleanup block at the bottom, which always runs.
    let mut created_ids: Vec<String> = Vec::new();

    // ── Credential A: registers WITH user verification ─────────────────
    let auth_a = VirtualAuthenticator::new();
    let mut have_a = false;
    match run_registration(&ctx, &auth_a, &rp_id_hash, true, "Virtual UV key", &password).await {
        Ok(id) => { created_ids.push(id); have_a = true; ok("A0 credential A (UV-capable) registered"); }
        Err(e) => bad(&format!("A0 registration failed: {e}")),
    }

    if have_a {
        match run_login(&ctx, &auth_a, &rp_id_hash, false, 2).await {
            Ok(()) => bad("A1 login WITHOUT UV on a UV-capable credential was ACCEPTED — enforcement is not working"),
            Err(e) if e.contains("401") || e.contains("400") => ok(&format!("A1 (context) login WITHOUT UV on a UV-capable credential was refused ({e})")),
            Err(e) => bad(&format!("A1 login without UV failed for the WRONG reason: {e}")),
        }

        match run_login(&ctx, &auth_a, &rp_id_hash, true, 2).await {
            Ok(()) => ok("A2 login WITH UV on the same credential succeeded"),
            Err(e) => bad(&format!("A2 login with UV should have succeeded, got: {e}")),
        }
    } else {
        println!("  \x1b[33m~\x1b[0m A1/A2 skipped — A0 registration did not succeed");
    }

    // ── Credential B: registers WITHOUT user verification (legacy-shaped) ──
    let auth_b = VirtualAuthenticator::new();
    let mut have_b = false;
    match run_registration(&ctx, &auth_b, &rp_id_hash, false, "Virtual legacy key", &password).await {
        Ok(id) => { created_ids.push(id); have_b = true; ok("B0 credential B (possession-only) registered"); }
        Err(e) => bad(&format!("B0 registration failed: {e}")),
    }

    if have_b {
        match run_login(&ctx, &auth_b, &rp_id_hash, false, 2).await {
            Ok(()) => ok("B1 login WITHOUT UV on a possession-only credential still succeeds (no regression)"),
            Err(e) => bad(&format!("B1 grandfathering broken — possession-only login was refused: {e}")),
        }
    } else {
        println!("  \x1b[33m~\x1b[0m B1 skipped — B0 registration did not succeed");
    }

    // ── Cleanup — always, regardless of pass/fail, since this runs against a
    //    real admin account's real passkey list. ──────────────────────────
    for id in &created_ids {
        let resp = ctx
            .client
            .delete(format!("{}/api/auth/passkeys/{id}", ctx.base))
            .headers(ctx.auth_headers())
            .send()
            .await;
        match resp {
            Ok(r) if r.status().is_success() => println!("  (cleanup) removed virtual credential {id}"),
            Ok(r) => { fail += 1; println!("  \x1b[31m\u{2717}\x1b[0m (cleanup) failed to remove {id}: HTTP {}", r.status()); }
            Err(e) => { fail += 1; println!("  \x1b[31m\u{2717}\x1b[0m (cleanup) failed to remove {id}: {e}"); }
        }
    }

    print_summary(pass, fail);
    if fail > 0 { std::process::exit(1); }
}

fn print_summary(pass: u32, fail: u32) {
    println!("\n{pass} passed, {fail} failed");
}

async fn run_registration(
    ctx: &Ctx,
    auth: &VirtualAuthenticator,
    rp_id_hash: &[u8; 32],
    uv: bool,
    name: &str,
    password: &str,
) -> Result<String, String> {
    let challenge = register_begin(ctx, password).await?;

    let client_data = client_data_json("webauthn.create", &challenge, &ctx.origin);
    let auth_data = auth.registration_auth_data(rp_id_hash, uv, 1);

    let att_obj = ciborium::Value::Map(vec![
        (ciborium::Value::Text("fmt".into()), ciborium::Value::Text("none".into())),
        (ciborium::Value::Text("attStmt".into()), ciborium::Value::Map(vec![])),
        (ciborium::Value::Text("authData".into()), ciborium::Value::Bytes(auth_data)),
    ]);
    let mut att_obj_bytes = Vec::new();
    ciborium::ser::into_writer(&att_obj, &mut att_obj_bytes).map_err(|e| e.to_string())?;

    let body = serde_json::json!({
        "id": auth.cred_id_b64(),
        "rawId": auth.cred_id_b64(),
        "response": {
            "attestationObject": URL_SAFE_NO_PAD.encode(&att_obj_bytes),
            "clientDataJson": URL_SAFE_NO_PAD.encode(&client_data),
        },
        "name": name,
    });

    let resp = ctx
        .client
        .post(format!("{}/api/auth/passkey/register/complete", ctx.base))
        .headers(ctx.auth_headers())
        .header("Content-Type", "application/json")
        .json(&body)
        .send()
        .await
        .map_err(|e| e.to_string())?;

    if !resp.status().is_success() {
        let status = resp.status();
        let text = resp.text().await.unwrap_or_default();
        return Err(format!("HTTP {status}: {text}"));
    }
    let parsed: serde_json::Value = resp.json().await.map_err(|e| e.to_string())?;
    parsed["id"]
        .as_str()
        .map(String::from)
        .ok_or_else(|| "register/complete: no id in response".to_string())
}

async fn run_login(
    ctx: &Ctx,
    auth: &VirtualAuthenticator,
    rp_id_hash: &[u8; 32],
    uv: bool,
    sign_count: u32,
) -> Result<(), String> {
    let resp = ctx
        .client
        .post(format!("{}/api/auth/passkey/auth/begin", ctx.base))
        .headers(ctx.anon_headers())
        .send()
        .await
        .map_err(|e| e.to_string())?;
    if !resp.status().is_success() {
        return Err(format!("auth/begin: HTTP {}", resp.status()));
    }
    let begin: serde_json::Value = resp.json().await.map_err(|e| e.to_string())?;
    let challenge = begin["publicKey"]["challenge"]
        .as_str()
        .ok_or("auth/begin: no challenge in response")?
        .to_string();

    let client_data = client_data_json("webauthn.get", &challenge, &ctx.origin);
    let auth_data = auth.assertion_auth_data(rp_id_hash, uv, sign_count);
    let signature = auth.sign(&auth_data, &client_data);

    let body = serde_json::json!({
        "id": auth.cred_id_b64(),
        "rawId": auth.cred_id_b64(),
        "response": {
            "authenticatorData": URL_SAFE_NO_PAD.encode(&auth_data),
            "clientDataJson": URL_SAFE_NO_PAD.encode(&client_data),
            "signature": URL_SAFE_NO_PAD.encode(&signature),
            "userHandle": serde_json::Value::Null,
        },
    });

    let resp = ctx
        .client
        .post(format!("{}/api/auth/passkey/auth/complete", ctx.base))
        .headers(ctx.anon_headers())
        .header("Content-Type", "application/json")
        .json(&body)
        .send()
        .await
        .map_err(|e| e.to_string())?;

    if !resp.status().is_success() {
        let status = resp.status();
        let text = resp.text().await.unwrap_or_default();
        return Err(format!("HTTP {status}: {text}"));
    }
    Ok(())
}
