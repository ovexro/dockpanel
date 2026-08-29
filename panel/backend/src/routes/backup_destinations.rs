use axum::{
    extract::{Path, State},
    http::StatusCode,
    Json,
};
use uuid::Uuid;

use crate::auth::{AdminUser, AuthUser};
use crate::error::{internal_error, err, agent_actionable, agent_error, ApiError};
use crate::AppState;

/// Which destinations a caller may send a site's backups to. `$1` is the caller.
///
/// The writer decided this first: `backup_schedules::set_schedule` accepts an
/// unscoped destination from any authenticated caller, on the stated grounds
/// that destinations are created and deleted through admin-only routes, so a
/// site owner can upload to one but never read it. That was the right call and
/// it shipped — but the only route that could tell a caller WHICH destinations
/// those are stayed `AdminUser`, so a non-admin's picker was empty and the
/// permission the writer granted could not be exercised. Reader and writer now
/// interpolate this one string, the way every site route shares
/// [`crate::helpers::SITE_CALLER_PREDICATE`], so the set a caller may CHOOSE
/// from cannot drift from the set the writer will ACCEPT.
///
/// Requires `LEFT JOIN servers s ON bd.server_id = s.id` in scope.
pub const DESTINATION_CALLER_PREDICATE: &str = "(bd.server_id IS NULL OR s.user_id = $1)";

/// The subset of a destination a non-admin needs to pick one: enough to choose,
/// nothing to steal. `config` holds the bucket credentials and is absent from
/// this struct BY CONSTRUCTION rather than by a masking pass — a field that does
/// not exist cannot be forgotten in a future edit.
#[derive(serde::Serialize, sqlx::FromRow)]
pub struct SelectableDestination {
    pub id: Uuid,
    pub name: String,
    pub dtype: String,
}

/// `encryption_enabled` and `encryption_key` are schema columns from the
/// `20260322000000_backup_orchestrator.sql` migration's original per-destination
/// encryption design; both are dead and are excluded from this struct BY
/// CONSTRUCTION, the same defence `SelectableDestination` above uses for `config`.
/// The design that shipped instead encrypts per-POLICY (`backup_policies.encrypt`)
/// with one key derived from the process JWT secret
/// (`backup_policy_executor::derive_backup_encryption_key`) — never per-destination,
/// so neither column was ever wired to anything. `encryption_key`'s deadness was
/// found and documented at three call sites (v2.24.0, lesson #70); this comment is
/// `encryption_enabled`'s first documentation anywhere — it had zero readers or
/// writers and no comment, and rode along unnoticed through that same fix and a
/// second pass on this table, because nothing named it as a thing to check.
#[derive(serde::Serialize, serde::Deserialize, sqlx::FromRow, Clone)]
pub struct BackupDestination {
    pub id: Uuid,
    pub name: String,
    pub dtype: String,
    pub config: serde_json::Value,
    pub created_at: chrono::DateTime<chrono::Utc>,
    pub updated_at: chrono::DateTime<chrono::Utc>,
}

#[derive(serde::Deserialize)]
pub struct CreateDestinationRequest {
    pub name: String,
    pub dtype: String,
    pub config: serde_json::Value,
    /// Which server this destination belongs to, or `None` for one shared by the
    /// whole fleet.
    ///
    /// The column has been nullable since the multi-server migration, on the
    /// stated grounds that a destination "can be shared" — but nothing ever wrote
    /// it, so NULL was the default for every row rather than a declaration, and a
    /// reader could not tell "shared" from "never answered". That ambiguity is
    /// what left `test_connection` with no way to know which host to ask.
    ///
    /// Making it an explicit request field is the whole fix: absent still means
    /// shared, but now because the caller said so.
    #[serde(default)]
    pub server_id: Option<uuid::Uuid>,
}

#[derive(serde::Deserialize)]
pub struct UpdateDestinationRequest {
    pub name: Option<String>,
    pub config: Option<serde_json::Value>,
}

/// GET /api/backup-destinations — List all backup destinations (admin).
pub async fn list(
    State(state): State<AppState>,
    AdminUser(_claims): AdminUser,
) -> Result<Json<Vec<BackupDestination>>, ApiError> {

    let dests: Vec<BackupDestination> = sqlx::query_as(
        "SELECT * FROM backup_destinations ORDER BY created_at DESC LIMIT 200",
    )
    .fetch_all(&state.db)
    .await
    .map_err(|e| internal_error("list backup_destinations", e))?;

    // Mask secret keys in response
    let masked: Vec<BackupDestination> = dests
        .into_iter()
        .map(|mut d| {
            mask_config_secrets(&mut d);
            d
        })
        .collect();

    Ok(Json(masked))
}

/// GET /api/backup-destinations/selectable — the destinations this caller may
/// schedule a site backup to, id/name/type only.
///
/// `AuthUser`, deliberately: the per-site schedule form is reachable by a site
/// owner and its writer already admits one, so refusing to name the choices was
/// the half that had never been connected. An administrator gets the same rows
/// this returns to anyone else plus their own servers' pinned destinations —
/// the predicate does the deciding, so there is no role branch to keep in step.
pub async fn selectable(
    State(state): State<AppState>,
    AuthUser(claims): AuthUser,
) -> Result<Json<Vec<SelectableDestination>>, ApiError> {
    let dests: Vec<SelectableDestination> = sqlx::query_as(&format!(
        "SELECT bd.id, bd.name, bd.dtype FROM backup_destinations bd \
         LEFT JOIN servers s ON bd.server_id = s.id \
         WHERE {DESTINATION_CALLER_PREDICATE} \
         ORDER BY bd.name ASC LIMIT 200"
    ))
    .bind(claims.sub)
    .fetch_all(&state.db)
    .await
    .map_err(|e| internal_error("list selectable backup_destinations", e))?;

    Ok(Json(dests))
}

/// POST /api/backup-destinations — Create a new backup destination.
pub async fn create(
    State(state): State<AppState>,
    AdminUser(claims): AdminUser,
    Json(body): Json<CreateDestinationRequest>,
) -> Result<(StatusCode, Json<BackupDestination>), ApiError> {


    if !["s3", "sftp"].contains(&body.dtype.as_str()) {
        return Err(err(StatusCode::BAD_REQUEST, "Type must be s3 or sftp"));
    }
    if body.name.trim().is_empty() {
        return Err(err(StatusCode::BAD_REQUEST, "Name is required"));
    }

    reject_empty_secrets(&body.config)?;

    // Encrypt sensitive fields in config before storing
    let encrypted_config = encrypt_config_secrets(&body.config, &state.config.jwt_secret)?;

    // Refuse a server the caller does not operate, rather than storing an id that
    // silently widens where this destination's credentials get sent.
    if let Some(sid) = body.server_id {
        let owned: Option<(uuid::Uuid,)> = sqlx::query_as(
            "SELECT id FROM servers WHERE id = $1 AND (is_local OR user_id = $2)",
        )
        .bind(sid)
        .bind(claims.sub)
        .fetch_optional(&state.db)
        .await
        .map_err(|e| internal_error("resolve destination server", e))?;
        if owned.is_none() {
            return Err(err(StatusCode::NOT_FOUND, "Server not found or access denied"));
        }
    }

    let mut dest: BackupDestination = sqlx::query_as(
        "INSERT INTO backup_destinations (name, dtype, config, server_id) VALUES ($1, $2, $3, $4) RETURNING *",
    )
    .bind(body.name.trim())
    .bind(&body.dtype)
    .bind(&encrypted_config)
    .bind(body.server_id)
    .fetch_one(&state.db)
    .await
    .map_err(|e| internal_error("create backup_destinations", e))?;

    tracing::info!("Backup destination created: {} ({})", dest.name, dest.dtype);
    mask_config_secrets(&mut dest);
    Ok((StatusCode::CREATED, Json(dest)))
}

/// PUT /api/backup-destinations/{id} — Update a destination.
pub async fn update(
    State(state): State<AppState>,
    AdminUser(_claims): AdminUser,
    Path(id): Path<Uuid>,
    Json(body): Json<UpdateDestinationRequest>,
) -> Result<Json<BackupDestination>, ApiError> {


    // ENCRYPT FIRST, THEN carry the masked values across.
    //
    // The order matters and the other one is silently destructive. This used to
    // merge first — copying the stored, ALREADY-ENCRYPTED secret into the config
    // and then handing the whole object to `encrypt_config_secrets`, which skips
    // only "" and the mask sentinel. A stored ciphertext is neither, so it was
    // encrypted a SECOND time. One decrypt on the way out then yields ciphertext,
    // and the destination authenticates with gibberish while the row looks fine.
    //
    // Nothing had ever exercised it: this endpoint had no caller in the panel at
    // all until the Destinations tab gained an Edit button (GH #93), so the very
    // first edit of a destination would have corrupted its credential.
    let encrypted_config = match body.config {
        None => None,
        Some(ref incoming) => {
            // An empty secret is refused rather than stored. It is neither the
            // mask nor a value `encrypt_config_secrets` touches, so it used to
            // sail through and overwrite a working credential with nothing.
            reject_empty_secrets(incoming)?;

            // Whatever arrived as plaintext gets encrypted exactly once. Masked
            // fields are left as the sentinel by design.
            let mut cfg = encrypt_config_secrets(incoming, &state.config.jwt_secret)?;

            let still_masked: Vec<String> = cfg
                .as_object()
                .map(|o| {
                    o.iter()
                        .filter(|(_, v)| v.as_str() == Some(MASK))
                        .map(|(k, _)| k.clone())
                        .collect()
                })
                .unwrap_or_default();

            // This UPDATE REPLACES the whole config column, and the form sends a
            // fixed key set per transport — so any stored key the form does not
            // know about was silently erased by an unrelated edit. `key_path` is
            // the live case: the agent supports it, the panel form has no field
            // for it, and renaming an SFTP destination dropped it.
            let existing: Option<(serde_json::Value,)> =
                sqlx::query_as("SELECT config FROM backup_destinations WHERE id = $1")
                    .bind(id)
                    .fetch_optional(&state.db)
                    .await
                    .map_err(|e| internal_error("update backup_destinations", e))?;

            if let Some((existing_cfg,)) = existing {
                if !still_masked.is_empty() {
                    carry_masked_secrets(&mut cfg, &existing_cfg, &still_masked);
                }
                carry_unmentioned_keys(&mut cfg, &existing_cfg);
            }
            Some(cfg)
        }
    };

    let mut dest: BackupDestination = sqlx::query_as(
        "UPDATE backup_destinations SET \
         name = COALESCE($1, name), \
         config = COALESCE($2, config), \
         updated_at = NOW() \
         WHERE id = $3 RETURNING *",
    )
    .bind(body.name.as_deref())
    .bind(&encrypted_config)
    .bind(id)
    .fetch_optional(&state.db)
    .await
    .map_err(|e| internal_error("update backup_destinations", e))?
    .ok_or_else(|| err(StatusCode::NOT_FOUND, "Destination not found"))?;

    mask_config_secrets(&mut dest);
    Ok(Json(dest))
}

/// DELETE /api/backup-destinations/{id} — Delete a destination.
pub async fn remove(
    State(state): State<AppState>,
    AdminUser(_claims): AdminUser,
    Path(id): Path<Uuid>,
) -> Result<Json<serde_json::Value>, ApiError> {


    // Check for dependent backup schedules
    let dependent_count: (i64,) = sqlx::query_as(
        "SELECT COUNT(*) FROM backup_schedules WHERE destination_id = $1"
    )
    .bind(id)
    .fetch_one(&state.db)
    .await
    .map_err(|e| internal_error("remove backup_destinations", e))?;

    let deleted = sqlx::query("DELETE FROM backup_destinations WHERE id = $1")
        .bind(id)
        .execute(&state.db)
        .await
        .map_err(|e| internal_error("remove backup_destinations", e))?;

    if deleted.rows_affected() == 0 {
        return Err(err(StatusCode::NOT_FOUND, "Destination not found"));
    }

    // Nullify destination_id on dependent schedules (FK is SET NULL)
    let mut resp = serde_json::json!({ "ok": true });
    if dependent_count.0 > 0 {
        resp["warning"] = serde_json::json!(format!(
            "{} backup schedule(s) were using this destination and are now unassigned",
            dependent_count.0
        ));
    }

    Ok(Json(resp))
}

/// POST /api/backup-destinations/{id}/test — Test connection FROM the host that
/// will actually use this destination.
///
/// This handler is where an operator's confidence in a destination comes from,
/// and it asked the wrong machine. It posted through `state.agent` — the panel's
/// own local `AgentClient`, unconditionally, with no header and no row consulted
/// — so two things were true at once: the DECRYPTED credential (the S3 secret
/// key, the SFTP password) was handed to the PANEL host for a destination a
/// fleet member was going to use, and the green "connection OK" was a statement
/// about the panel's egress rather than the member's. A member behind a
/// different firewall, or with no route to the bucket at all, still lit the
/// button green and then failed every night.
///
/// The SCHEDULED path never had this defect: `backup_policy_executor` resolves
/// `agents.for_server(...)` per row for sites (:328), databases (:450) and
/// volumes (:568) alike, and refuses rather than falling back to local. That
/// asymmetry is the thing to notice — the button an operator PRESSES is the
/// surface that never adopted the rule the background loop was already
/// following, and it stayed hidden because this handler contains no `ServerScope`
/// token for a census of header-derived dispatch to match on. It was not using
/// the wrong scope; it had no scope at all.
pub async fn test_connection(
    State(state): State<AppState>,
    AdminUser(_claims): AdminUser,
    Path(id): Path<Uuid>,
) -> Result<Json<serde_json::Value>, ApiError> {


    // Columns rather than `SELECT *` into `BackupDestination`: the struct is the
    // API's response shape and has no `server_id`, and the payload builder takes
    // `dtype` + `config` precisely so a caller holding loose columns need not
    // rebuild a row to use it.
    let row: Option<(String, String, serde_json::Value, Option<Uuid>)> = sqlx::query_as(
        "SELECT name, dtype, config, server_id FROM backup_destinations WHERE id = $1",
    )
    .bind(id)
    .fetch_optional(&state.db)
    .await
    .map_err(|e| internal_error("test connection", e))?;

    let (name, dtype, config, server_id) =
        row.ok_or_else(|| err(StatusCode::NOT_FOUND, "Destination not found"))?;

    // Build agent request
    let agent_body = serde_json::json!({
        "destination": agent_destination_payload(&dtype, &config),
    });

    // A NULL `server_id` means "no host has claimed this destination". It does
    // NOT mean "the panel owns it". `20260807000000_sleep_and_destination_server_scope`
    // backfilled every pre-existing row to the local server, so a NULL row today
    // is one created since — and `create` above still does not set the column, so
    // NULL is the DEFAULT for every new destination rather than a deliberate
    // declaration that it is shared. Reading it as "test from the panel" would
    // re-spell, for the majority of rows, the exact defect this handler is fixing.
    //
    // So an unclaimed destination is tested from every online member instead. For
    // one that really is shared that is the honest reading of the question — can
    // the hosts that will upload to it reach it — and the panel's own box is in
    // that fleet like any other member, so nothing is lost for a single-server
    // install, where the fleet is exactly one host and the answer is identical to
    // the old one.
    let Some(server_id) = server_id else {
        return test_from_whole_fleet(&state, &name, agent_body).await;
    };

    // Refuse rather than substitute — the rule `helpers::agent_for_site_server`
    // states for sites. Answering with a different host's reachability is how a
    // destination gets green-lit for a machine nobody asked about.
    let agent = state.agents.for_server(server_id).await.map_err(|e| {
        tracing::warn!(
            "Refusing to test destination {name}: its server {server_id} is unreachable ({e}) — \
             testing from another host would answer a question nobody asked"
        );
        err(
            StatusCode::BAD_GATEWAY,
            "The server this destination belongs to is unreachable",
        )
    })?;

    let server_name: Option<(String,)> = sqlx::query_as("SELECT name FROM servers WHERE id = $1")
        .bind(server_id)
        .fetch_optional(&state.db)
        .await
        .ok()
        .flatten();

    let mut result = agent
        .post("/backups/test-destination", Some(agent_body))
        .await
        .map_err(|e| agent_error("Backup destination test", e))?;

    // The agent's own `{"success": …, "message": …}` is passed through unchanged
    // and the host is added beside it. An unattributed "connection successful" is
    // what let a panel-egress result be read as a fact about the fleet.
    if let Some(obj) = result.as_object_mut() {
        obj.insert("ok".to_string(), serde_json::json!(true));
        obj.insert(
            "server".to_string(),
            serde_json::json!(server_name.map(|(n,)| n)),
        );
    }

    Ok(Json(result))
}

/// Test an unclaimed destination from every online member, reporting per host.
///
/// Partial stays partial. One member that cannot reach the bucket is a real
/// problem — its backups will fail nightly — but it does not make the destination
/// broken for the hosts that can reach it, and turning the whole button red would
/// be as false a summary as the panel-only green it replaces. So the request
/// fails only when NOTHING reached the destination, which preserves what the old
/// handler got right: a test that cannot connect must not answer 200. Anything
/// else is a 200 carrying the names, in the same `warning` field `remove` above
/// already uses to say a delete left schedules unassigned.
async fn test_from_whole_fleet(
    state: &AppState,
    dest_name: &str,
    agent_body: serde_json::Value,
) -> Result<Json<serde_json::Value>, ApiError> {
    let fleet = state.agents.online_fleet().await;
    if fleet.is_empty() {
        return Err(err(
            StatusCode::BAD_GATEWAY,
            "No server is online to test this destination from",
        ));
    }

    let mut reachable: Vec<String> = Vec::new();
    let mut failed: Vec<(String, String)> = Vec::new();
    // Tracked beside the display strings so a refusal every member agrees on can
    // keep its own status. `unanimous` drops to false the moment one member fails
    // for a reason the operator cannot act on, or for a different one than the
    // rest — either way there is no single answer left to propagate.
    let mut shared: Option<StatusCode> = None;
    let mut unanimous = true;
    for member in &fleet {
        match member
            .agent
            .post("/backups/test-destination", Some(agent_body.clone()))
            .await
        {
            Ok(_) => reachable.push(member.name.clone()),
            Err(e) => {
                tracing::warn!("Destination {dest_name}: {} could not reach it: {e}", member.name);
                match agent_actionable(&e) {
                    Some((status, msg)) => {
                        if *shared.get_or_insert(status) != status {
                            unanimous = false;
                        }
                        failed.push((member.name.clone(), msg));
                    }
                    None => {
                        unanimous = false;
                        failed.push((member.name.clone(), e.to_string()));
                    }
                }
            }
        }
    }

    if reachable.is_empty() {
        let detail = failed
            .iter()
            .map(|(server, e)| format!("{server}: {e}"))
            .collect::<Vec<_>>()
            .join("; ");
        // Every host refused for the same thing the operator can fix, so answer
        // with THAT — the single-server path above already does, via
        // `agent_error`. Flattening it here is what tells somebody their fleet
        // broke while every host in it is working and saying the same actionable
        // sentence. A mixed bag, or any genuine agent fault, still reads 502.
        let status = match shared {
            Some(s) if unanimous => s,
            _ => StatusCode::BAD_GATEWAY,
        };
        return Err(err(
            status,
            &format!("No server could reach \"{dest_name}\" — {detail}"),
        ));
    }

    // Counted by difference from the hosts actually asked, so there is no second
    // predicate to drift away from `online_fleet`'s. A registered server that was
    // never asked is not a pass: it is the one host whose answer is still unknown.
    let asked: Vec<Uuid> = fleet.iter().map(|m| m.id).collect();
    let not_asked: Vec<(String, String)> = sqlx::query_as(
        "SELECT name, status FROM servers WHERE NOT (id = ANY($1)) ORDER BY name",
    )
    .bind(asked.as_slice())
    .fetch_all(&state.db)
    .await
    .unwrap_or_default();

    let listed = |v: &[(String, String)]| {
        v.iter()
            .map(|(server, why)| format!("{server} ({why})"))
            .collect::<Vec<_>>()
            .join(", ")
    };
    let mut parts = Vec::new();
    if !failed.is_empty() {
        parts.push(format!("could not reach it: {}", listed(&failed)));
    }
    if !not_asked.is_empty() {
        parts.push(format!("not tested: {}", listed(&not_asked)));
    }

    let mut resp = serde_json::json!({
        "ok": parts.is_empty(),
        "shared": true,
        "reachable": reachable,
        "failed": failed.iter()
            .map(|(server, error)| serde_json::json!({ "server": server, "error": error }))
            .collect::<Vec<_>>(),
        "not_asked": not_asked.iter()
            .map(|(server, status)| serde_json::json!({ "server": server, "status": status }))
            .collect::<Vec<_>>(),
    });

    if !parts.is_empty() {
        resp["warning"] = serde_json::json!(format!(
            "Reached from {} of {} servers — {}.",
            reachable.len(),
            reachable.len() + failed.len() + not_asked.len(),
            parts.join("; "),
        ));
    }

    Ok(Json(resp))
}

/// The `destination` object the agent's `/backups/*` handlers expect: the stored
/// config with its secrets decrypted and `type` folded in.
///
/// Everything that hands a destination to the agent must come through here.
/// Three places built this object and only one of them — `test_connection` —
/// decrypted first. The other two, the per-site scheduler and the policy
/// executor, cloned the row's `config` straight out of the database and posted
/// the CIPHERTEXT as the S3 secret key or the SFTP password. The failure is
/// shaped to survive review: Test Connection uses the decrypting path and
/// reports success, so a destination verifies green and then every actual upload
/// authenticates with an encrypted blob. Taking `dtype` and `config` rather than
/// a `BackupDestination` is deliberate — every caller holds the two columns from
/// a query of its own and would otherwise have kept hand-rolling this. That now
/// includes `test_connection`, which dropped its `build_agent_destination`
/// wrapper when it started selecting `server_id` alongside them: reassembling a
/// response struct to reach a payload builder was the only thing that wrapper did.
pub fn agent_destination_payload(dtype: &str, config: &serde_json::Value) -> serde_json::Value {
    let mut d = decrypt_config_secrets(config);
    if let Some(obj) = d.as_object_mut() {
        obj.insert("type".to_string(), serde_json::json!(dtype));
    } else {
        d = serde_json::json!({ "type": dtype });
    }
    d
}

/// Replace each still-masked key in `cfg` with the value already stored for it,
/// verbatim.
///
/// "Verbatim" is the whole point: the stored value is ciphertext, and putting it
/// through the encryptor again is exactly the bug this function exists to make
/// impossible to write by accident. A key the caller masked but which the stored
/// config does not have is dropped rather than left as the sentinel, so the mask
/// can never reach the agent as if it were a credential.
fn carry_masked_secrets(
    cfg: &mut serde_json::Value,
    existing: &serde_json::Value,
    masked_keys: &[String],
) {
    let (Some(obj), Some(stored)) = (cfg.as_object_mut(), existing.as_object()) else {
        return;
    };
    for k in masked_keys {
        match stored.get(k) {
            Some(v) => { obj.insert(k.clone(), v.clone()); }
            None => { obj.remove(k); }
        }
    }
}

/// Carry across any stored key the incoming config does not mention.
///
/// The PUT replaces the entire JSONB column, but the edit form only ever sends
/// the fixed key set for the selected transport. Anything else a destination had
/// — `key_path` being the real one — vanished on the first unrelated edit. Keys
/// the caller DID send win, including one sent as JSON null, so this preserves
/// without preventing a deliberate change.
fn carry_unmentioned_keys(cfg: &mut serde_json::Value, existing: &serde_json::Value) {
    let (Some(obj), Some(stored)) = (cfg.as_object_mut(), existing.as_object()) else {
        return;
    };
    for (k, v) in stored {
        if !obj.contains_key(k) {
            obj.insert(k.clone(), v.clone());
        }
    }
}

/// Sensitive keys within the backup destination config JSON.
const CONFIG_SENSITIVE_KEYS: &[&str] = &["secret_key", "password"];

/// The sentinel the list endpoint substitutes for a stored secret, and the only
/// value that means "keep what is already there".
const MASK: &str = "********";

/// Replace every stored secret in a row with the mask, in place.
///
/// `list` masks; `create` and `update` returned the row straight from
/// `RETURNING *`, so a PUT that merely renamed a destination echoed the stored
/// credential back over the wire — ciphertext on a modern row, and the REAL
/// secret on any row created before credential encryption landed, since nothing
/// backfills that column. An admin who never knew the credential could read it
/// out of the response to a rename.
fn mask_config_secrets(dest: &mut BackupDestination) {
    let Some(obj) = dest.config.as_object_mut() else { return };
    for key in CONFIG_SENSITIVE_KEYS {
        if let Some(v) = obj.get(*key) {
            if v.as_str().map(|s| !s.is_empty()).unwrap_or(false) {
                obj.insert((*key).to_string(), serde_json::json!(MASK));
            }
        }
    }
}

/// Reject an empty string submitted for a secret.
///
/// `encrypt_config_secrets` deliberately skips `""`, and `""` is not the mask,
/// so `carry_masked_secrets` never sees it either — an empty secret therefore
/// travelled all the way into the config column and REPLACED a working
/// credential with nothing. There is no destination for which an empty secret is
/// valid, so the honest answer is to refuse rather than to guess whether the
/// operator meant "clear it" or "leave it alone".
fn reject_empty_secrets(config: &serde_json::Value) -> Result<(), ApiError> {
    let Some(obj) = config.as_object() else { return Ok(()) };
    for key in CONFIG_SENSITIVE_KEYS {
        if obj.get(*key).and_then(|v| v.as_str()) == Some("") {
            return Err(err(
                StatusCode::BAD_REQUEST,
                &format!("{key} cannot be empty — leave the field masked to keep the stored value, or type a new one"),
            ));
        }
    }
    Ok(())
}

/// Encrypt sensitive fields within a backup destination config JSON.
fn encrypt_config_secrets(config: &serde_json::Value, jwt_secret: &str) -> Result<serde_json::Value, ApiError> {
    let mut cfg = config.clone();
    if let Some(obj) = cfg.as_object_mut() {
        for key in CONFIG_SENSITIVE_KEYS {
            if let Some(v) = obj.get(*key) {
                if let Some(s) = v.as_str() {
                    if !s.is_empty() && s != "********" {
                        let encrypted = crate::services::secrets_crypto::encrypt_credential(s, jwt_secret)
                            .map_err(|e| err(StatusCode::INTERNAL_SERVER_ERROR, &format!("Encryption failed: {e}")))?;
                        obj.insert(key.to_string(), serde_json::json!(encrypted));
                    }
                }
            }
        }
    }
    Ok(cfg)
}

/// Decrypt sensitive fields within a backup destination config JSON.
/// Falls back to plaintext for legacy unencrypted values.
fn decrypt_config_secrets(config: &serde_json::Value) -> serde_json::Value {
    let mut cfg = config.clone();
    if let Some(obj) = cfg.as_object_mut() {
        for key in CONFIG_SENSITIVE_KEYS {
            if let Some(v) = obj.get(*key) {
                if let Some(s) = v.as_str() {
                    if !s.is_empty() {
                        let decrypted = crate::services::secrets_crypto::decrypt_credential_from_env(s);
                        obj.insert(key.to_string(), serde_json::json!(decrypted));
                    }
                }
            }
        }
    }
    cfg
}

#[cfg(test)]
mod tests {
    use super::*;

    const JWT: &str = "test-jwt-secret-at-least-32-bytes-long-for-hkdf";

    /// The shape an edit takes when the operator changed the bucket but left the
    /// secret masked — which is what the Destinations tab sends for every edit
    /// that does not rotate the credential.
    #[test]
    fn a_masked_secret_is_carried_across_not_re_encrypted() {
        let stored = encrypt_config_secrets(
            &serde_json::json!({ "bucket": "old", "secret_key": "s3cr3t" }),
            JWT,
        )
        .unwrap();
        let stored_cipher = stored["secret_key"].as_str().unwrap().to_string();
        assert_ne!(stored_cipher, "s3cr3t", "precondition: it really was encrypted");

        // What the UI sends back: new bucket, masked secret.
        let incoming = serde_json::json!({ "bucket": "new", "secret_key": "********" });
        let mut cfg = encrypt_config_secrets(&incoming, JWT).unwrap();
        assert_eq!(cfg["secret_key"], "********", "the sentinel is not encrypted");

        carry_masked_secrets(&mut cfg, &stored, &["secret_key".to_string()]);

        assert_eq!(cfg["bucket"], "new");
        // The bug: merging BEFORE encrypting fed this ciphertext back through the
        // encryptor, so one decrypt returned ciphertext and the destination
        // authenticated with gibberish. It must come out byte-identical.
        assert_eq!(cfg["secret_key"].as_str().unwrap(), stored_cipher);
        assert_eq!(
            crate::services::secrets_crypto::decrypt_credential(&stored_cipher, JWT).unwrap(),
            "s3cr3t",
            "and one decrypt must still recover the original"
        );
    }

    #[test]
    fn a_rotated_secret_replaces_the_stored_one() {
        let stored = encrypt_config_secrets(&serde_json::json!({ "secret_key": "old" }), JWT).unwrap();
        let mut cfg =
            encrypt_config_secrets(&serde_json::json!({ "secret_key": "new" }), JWT).unwrap();
        // Nothing is masked, so nothing is carried.
        carry_masked_secrets(&mut cfg, &stored, &[]);
        let got = crate::services::secrets_crypto::decrypt_credential(
            cfg["secret_key"].as_str().unwrap(),
            JWT,
        )
        .unwrap();
        assert_eq!(got, "new");
    }

    #[test]
    fn a_mask_with_nothing_stored_is_dropped_never_passed_through() {
        // Otherwise "********" would reach the agent as the literal credential.
        let mut cfg = serde_json::json!({ "host": "h", "password": "********" });
        carry_masked_secrets(&mut cfg, &serde_json::json!({ "host": "h" }), &["password".to_string()]);
        assert!(cfg.get("password").is_none());
    }

    #[test]
    fn the_agent_payload_carries_its_type() {
        // Only the `type` fold is asserted here. The decrypt half reads JWT_SECRET
        // from the process environment, and setting that from a test would race
        // every other test in the binary — it is covered by the round-trip above
        // and by the live drive on a real box.
        let payload = agent_destination_payload("sftp", &serde_json::json!({ "host": "h" }));
        assert_eq!(payload["type"], "sftp");
        assert_eq!(payload["host"], "h");
    }

    #[test]
    fn an_empty_secret_is_refused_not_stored() {
        // The destructive path: encrypt_config_secrets skips "", and "" is not the
        // mask, so nothing carried the stored value across and the UPDATE wrote the
        // empty string over a working credential.
        assert!(reject_empty_secrets(&serde_json::json!({ "secret_key": "" })).is_err());
        assert!(reject_empty_secrets(&serde_json::json!({ "password": "" })).is_err());
        // A real value and the mask are both legitimate.
        assert!(reject_empty_secrets(&serde_json::json!({ "secret_key": "AKIAsecret" })).is_ok());
        assert!(reject_empty_secrets(&serde_json::json!({ "secret_key": MASK })).is_ok());
        // A destination with no secret at all (SFTP by key) is not an error.
        assert!(reject_empty_secrets(&serde_json::json!({ "host": "h" })).is_ok());
    }

    #[test]
    fn a_stored_key_the_form_does_not_send_survives_an_edit() {
        // The form sends a fixed key set per transport; the UPDATE replaces the
        // whole column. key_path is agent-supported with no panel field, so an
        // unrelated rename used to erase it.
        let mut cfg = serde_json::json!({ "host": "h2", "username": "u" });
        let stored = serde_json::json!({ "host": "h1", "username": "u", "key_path": "/root/.ssh/id_ed25519" });
        carry_unmentioned_keys(&mut cfg, &stored);
        assert_eq!(cfg["key_path"], "/root/.ssh/id_ed25519");
        // What the caller DID send still wins.
        assert_eq!(cfg["host"], "h2");
    }

    #[test]
    fn create_and_update_do_not_echo_the_stored_secret() {
        // list() masked; create/update returned RETURNING * verbatim, so a rename
        // handed back the stored credential — plaintext on any row predating
        // credential encryption, since nothing backfills that column.
        let mut dest = BackupDestination {
            id: Uuid::nil(),
            name: "d".into(),
            dtype: "s3".into(),
            config: serde_json::json!({ "bucket": "b", "secret_key": "PLAINTEXT-LEGACY", "password": "" }),
            created_at: chrono::Utc::now(),
            updated_at: chrono::Utc::now(),
        };
        mask_config_secrets(&mut dest);
        assert_eq!(dest.config["secret_key"], MASK);
        assert_eq!(dest.config["bucket"], "b");
        // An empty stored value is left alone — masking it would claim a
        // credential exists where none does.
        assert_eq!(dest.config["password"], "");
    }
}
