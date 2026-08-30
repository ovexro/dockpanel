//! Re-encrypt every stored credential under the CURRENT primary key.
//!
//! ## Why this exists
//!
//! `secrets_crypto` now decrypts through a UNION of candidate keys, so adding,
//! changing or removing `SECRETS_ENCRYPTION_KEY` no longer strands data. That
//! makes the variable *survivable*. It does not make it *finished*: until every
//! row is rewritten under the new primary key, the whole install depends on a
//! fallback arm, and the old derivation can never be retired.
//!
//! This is the missing migration half. Before v2.112.0 there was no
//! re-encryption path of any kind — verified, not assumed: a tree-wide search
//! for `re.?encrypt|rotate_key|rekey|key_rotation` returned only comments and
//! one test name.
//!
//! ## The registry is a CENSUS, and its completeness is pinned
//!
//! [`SIMPLE_SUBJECTS`] plus the three special shapes below must cover every
//! module that calls `encrypt_credential`. A registry that merely *lists* what
//! someone remembered is a changelog: the next feature to encrypt a column
//! joins the install silently and is then the one row this sweep skips.
//! `every_credential_writer_is_covered`, in this file's own test module, asserts
//! that every route module containing an `encrypt_credential` call appears in
//! [`COVERED_MODULES`], and fails when one does not. (This paragraph used to
//! name `tests/credential-key-survival-pin-e2e.sh` §D, a file that has never
//! existed in any commit — the guarantee was real, its cited enforcer was not.)

use sqlx::{PgPool, Row};

use crate::routes::backup_destinations::CONFIG_SENSITIVE_KEYS as DESTINATION_SECRET_KEYS;
use crate::services::secrets_crypto;

/// A credential column this sweep rewrites: (table, id column, value column).
///
/// The id column is compared and returned as text so one code path serves both
/// `uuid` ids and `settings`' text key.
const SIMPLE_SUBJECTS: &[(&str, &str, &str)] = &[
    ("databases", "id", "db_password_enc"),
    ("mail_domains", "id", "dkim_private_key"),
    ("dns_zones", "id", "cf_api_token"),
    ("users", "id", "totp_secret"),
    ("whmcs_config", "id", "api_secret_encrypted"),
    ("cdn_zones", "id", "api_key"),
    ("git_deploys", "id", "github_token"),
    ("servers", "id", "agent_token"),
    ("alert_rules", "id", "notify_pagerduty_key"),
    ("alert_rules", "id", "notify_webhook_url"),
    ("alert_rules", "id", "notify_slack_url"),
    ("alert_rules", "id", "notify_discord_url"),
    ("monitors", "id", "alert_slack_url"),
    ("monitors", "id", "alert_discord_url"),
    ("webhook_endpoints", "id", "verify_secret"),
    ("extensions", "id", "webhook_secret"),
    ("whmcs_config", "id", "webhook_secret"),
    ("deploy_configs", "id", "webhook_secret"),
    ("git_deploys", "id", "webhook_secret"),
];

/// Subjects the sweep visits through a hand-written arm rather than
/// [`SIMPLE_SUBJECTS`], because their ciphertext is not one row-one column.
///
/// `subject_tokens_match_the_sweep` asserts each of these is really passed to a
/// `SubjectReport::new(..)` in this file, so the list cannot drift away from the
/// arms it claims to describe.
const SPECIAL_SUBJECTS: &[&str] = &[
    "settings.value",
    "backup_destinations.config",
    "secrets.encrypted_value",
];

/// Modules known to write an encrypted credential, each paired with **the
/// subject that re-keys it**.
///
/// ⚠ The pairing is the point, and it was added after the registry proved it
/// could be satisfied without doing its job. It was previously a bare list of
/// module names checked by `every_credential_writer_is_covered`, so a new
/// encrypting module could be made to pass by adding its NAME here — while the
/// column it wrote was never added to `SIMPLE_SUBJECTS` and so was silently
/// skipped by every re-key for ever. That is the exact failure this file's own
/// header warns about ("a registry that merely lists what someone remembered is
/// a changelog"), reproduced inside the mechanism meant to prevent it. Naming
/// the subject makes the two halves one edit: `subject_tokens_match_the_sweep`
/// rejects a subject the sweep does not actually visit.
pub const COVERED_MODULES: &[(&str, &str)] = &[
    ("settings", "settings.value"),
    ("databases", "databases.db_password_enc"),
    ("whmcs", "whmcs_config.api_secret_encrypted"),
    ("system", "settings.value"),
    ("sites", "databases.db_password_enc"),
    ("migration", "databases.db_password_enc"),
    ("mail", "mail_domains.dkim_private_key"),
    ("dns", "dns_zones.cf_api_token"),
    ("backup_destinations", "backup_destinations.config"),
    ("auth", "users.totp_secret"),
    ("cdn", "cdn_zones.api_key"),
    ("git_deploys", "git_deploys.github_token"),
    ("servers", "servers.agent_token"),
    ("agent", "servers.agent_token"),
    ("alerts", "alert_rules.notify_pagerduty_key"),
    ("alerts", "alert_rules.notify_webhook_url"),
    ("alerts", "alert_rules.notify_slack_url"),
    ("alerts", "alert_rules.notify_discord_url"),
    ("monitors", "monitors.alert_slack_url"),
    ("monitors", "monitors.alert_discord_url"),
    ("webhook_gateway", "webhook_endpoints.verify_secret"),
    ("extensions", "extensions.webhook_secret"),
    ("whmcs", "whmcs_config.webhook_secret"),
    ("deploy", "deploy_configs.webhook_secret"),
    ("git_deploys", "git_deploys.webhook_secret"),
];

/// The module names alone, for the operator-facing settings endpoint.
pub fn covered_module_names() -> Vec<&'static str> {
    COVERED_MODULES.iter().map(|(m, _)| *m).collect()
}

/// Every subject this sweep re-keys, as `table.column`.
///
/// Reported beside the module list so the endpoint answers "did it touch my X?"
/// about COLUMNS and not only about modules — the module half alone was what
/// let a module be declared covered while its column was skipped. Deriving both
/// halves from the same two constants is also what keeps `SPECIAL_SUBJECTS`
/// honest: it is production data, so it cannot rot behind a `cfg(test)`.
pub fn swept_subjects() -> Vec<String> {
    SIMPLE_SUBJECTS
        .iter()
        .map(|(table, _, col)| format!("{table}.{col}"))
        .chain(SPECIAL_SUBJECTS.iter().map(|s| s.to_string()))
        .collect()
}

/// `settings` rows whose `value` is ciphertext. Mirrors the predicate the two
/// writers in `routes::settings` apply (`SENSITIVE_KEYS` plus the
/// `_client_secret` suffix) and the `pdns_api_key` written by `routes::system`.
///
/// The `IN (...)` list is a hand-copied literal — SQL text, not Rust — so it
/// cannot `use` `SETTINGS_SENSITIVE_KEYS` directly the way `DESTINATION_SECRET_KEYS`
/// below now imports its source of truth. `sensitive_settings_sql_covers_settings_keys`
/// in this file's own test module asserts the two haven't drifted instead.
const SENSITIVE_SETTINGS_SQL: &str =
    "SELECT key::text AS id, value FROM settings \
     WHERE (key IN ('smtp_password', 'pdns_api_key') OR key LIKE '%\\_client\\_secret') \
       AND value IS NOT NULL AND value <> ''";

#[derive(Debug, serde::Serialize)]
pub struct SubjectReport {
    pub subject: String,
    /// Rows carrying a non-empty value that we examined.
    pub examined: i64,
    /// Rows rewritten under the current primary key.
    pub rewritten: i64,
    /// Rows already under the primary key — nothing to do.
    pub already_current: i64,
    /// Rows no candidate key could open. These are NOT rewritten; a failed
    /// re-encrypt must never overwrite the only copy of the ciphertext.
    pub unreadable: i64,
    /// Rows a normal write (a password reset, a token rotation, a settings
    /// save) changed between our SELECT and our UPDATE. The CAS guard on every
    /// UPDATE in this file catches this and skips rather than overwrites the
    /// newer value with a re-encrypted copy of the stale one we read — these
    /// are correctly re-keyed on the NEXT run, once nothing races them.
    pub raced: i64,
}

impl SubjectReport {
    fn new(subject: impl Into<String>) -> Self {
        Self {
            subject: subject.into(),
            examined: 0,
            rewritten: 0,
            already_current: 0,
            raced: 0,
            unreadable: 0,
        }
    }
}

/// Sweep every subject. Never fails the whole run because one subject failed —
/// a missing optional table (whmcs on an install that never configured it) must
/// not stop the rest, and a partially-completed sweep is safe to re-run.
pub async fn reencrypt_all(pool: &PgPool, jwt_secret: &str) -> Vec<SubjectReport> {
    let mut reports = Vec::new();

    for (table, id_col, value_col) in SIMPLE_SUBJECTS {
        reports.push(reencrypt_column(pool, jwt_secret, table, id_col, value_col).await);
    }
    reports.push(reencrypt_settings(pool, jwt_secret).await);
    reports.push(reencrypt_destinations(pool, jwt_secret).await);
    reports.push(reencrypt_vault_secrets(pool, jwt_secret).await);

    reports
}

async fn reencrypt_column(
    pool: &PgPool,
    jwt_secret: &str,
    table: &str,
    id_col: &str,
    value_col: &str,
) -> SubjectReport {
    let mut report = SubjectReport::new(format!("{table}.{value_col}"));

    // Table/column names come from the const registry above, never from input.
    let select = format!(
        "SELECT {id_col}::text AS id, {value_col} AS value FROM {table} \
         WHERE {value_col} IS NOT NULL AND {value_col} <> ''"
    );
    let rows = match sqlx::query(&select).fetch_all(pool).await {
        Ok(r) => r,
        Err(e) => {
            tracing::warn!("re-encrypt: skipping {table}.{value_col}: {e}");
            return report;
        }
    };

    // CAS on the value read below: the guard means a normal write landing
    // between our SELECT and this UPDATE makes the UPDATE affect 0 rows
    // instead of clobbering the newer plaintext's ciphertext with a
    // re-encrypted copy of what we read.
    let update = format!(
        "UPDATE {table} SET {value_col} = $1 WHERE {id_col}::text = $2 AND {value_col} = $3"
    );
    for row in rows {
        let id: String = row.get("id");
        let value: String = row.get("value");
        report.examined += 1;

        match secrets_crypto::reencrypt_credential(&value, jwt_secret) {
            Ok(None) => report.already_current += 1,
            Ok(Some(fresh)) => {
                match sqlx::query(&update)
                    .bind(&fresh)
                    .bind(&id)
                    .bind(&value)
                    .execute(pool)
                    .await
                {
                    Ok(res) if res.rows_affected() > 0 => report.rewritten += 1,
                    Ok(_) => {
                        report.raced += 1;
                        tracing::warn!(
                            "re-encrypt: {table}.{value_col} id={id} changed concurrently — \
                             skipped this run, will be picked up by the next one"
                        );
                    }
                    Err(e) => {
                        report.unreadable += 1;
                        tracing::error!("re-encrypt: {table}.{value_col} id={id} write failed: {e}");
                    }
                }
            }
            Err(e) => {
                report.unreadable += 1;
                // The VALUE is never logged, only its address.
                tracing::error!("re-encrypt: {table}.{value_col} id={id} unreadable: {e}");
            }
        }
    }

    report
}

async fn reencrypt_settings(pool: &PgPool, jwt_secret: &str) -> SubjectReport {
    let mut report = SubjectReport::new("settings.value");

    let rows = match sqlx::query(SENSITIVE_SETTINGS_SQL).fetch_all(pool).await {
        Ok(r) => r,
        Err(e) => {
            tracing::warn!("re-encrypt: skipping settings.value: {e}");
            return report;
        }
    };

    for row in rows {
        let key: String = row.get("id");
        let value: String = row.get("value");
        report.examined += 1;

        match secrets_crypto::reencrypt_credential(&value, jwt_secret) {
            Ok(None) => report.already_current += 1,
            Ok(Some(fresh)) => {
                let res = sqlx::query(
                    "UPDATE settings SET value = $1, updated_at = NOW() \
                     WHERE key = $2 AND value = $3",
                )
                    .bind(&fresh)
                    .bind(&key)
                    .bind(&value)
                    .execute(pool)
                    .await;
                match res {
                    Ok(r) if r.rows_affected() > 0 => report.rewritten += 1,
                    Ok(_) => {
                        report.raced += 1;
                        tracing::warn!(
                            "re-encrypt: settings[{key}] changed concurrently — skipped this run"
                        );
                    }
                    Err(e) => {
                        report.unreadable += 1;
                        tracing::error!("re-encrypt: settings[{key}] write failed: {e}");
                    }
                }
            }
            Err(e) => {
                report.unreadable += 1;
                tracing::error!("re-encrypt: settings[{key}] unreadable: {e}");
            }
        }
    }

    report
}

/// `backup_destinations.config` is JSON with ciphertext at known keys, so the
/// unit of work is a key inside a document rather than a column.
async fn reencrypt_destinations(pool: &PgPool, jwt_secret: &str) -> SubjectReport {
    let mut report = SubjectReport::new("backup_destinations.config");

    let rows = match sqlx::query("SELECT id::text AS id, config FROM backup_destinations")
        .fetch_all(pool)
        .await
    {
        Ok(r) => r,
        Err(e) => {
            tracing::warn!("re-encrypt: skipping backup_destinations.config: {e}");
            return report;
        }
    };

    for row in rows {
        let id: String = row.get("id");
        let original_config: serde_json::Value = row.get("config");
        let mut config = original_config.clone();
        let mut changed = false;
        let mut row_failed = false;
        let mut row_examined = false;

        for key in DESTINATION_SECRET_KEYS {
            let Some(value) = config.get(*key).and_then(|v| v.as_str()) else {
                continue;
            };
            if value.is_empty() {
                continue;
            }
            row_examined = true;

            match secrets_crypto::reencrypt_credential(value, jwt_secret) {
                Ok(None) => {}
                Ok(Some(fresh)) => {
                    config[*key] = serde_json::Value::String(fresh);
                    changed = true;
                }
                Err(e) => {
                    row_failed = true;
                    tracing::error!("re-encrypt: backup_destinations[{id}].{key} unreadable: {e}");
                }
            }
        }

        if row_examined {
            report.examined += 1;
        }
        if row_failed {
            report.unreadable += 1;
            // Never write back a document we could only partly re-encrypt.
            continue;
        }
        if !changed {
            if row_examined {
                report.already_current += 1;
            }
            continue;
        }

        // CAS on the config we read at the top of this iteration, before any
        // key was rewritten — a concurrent edit to this destination (e.g.
        // renaming it or changing an unrelated field) between our SELECT and
        // this UPDATE makes the guard fail instead of clobbering it.
        match sqlx::query(
            "UPDATE backup_destinations SET config = $1 WHERE id::text = $2 AND config = $3",
        )
            .bind(&config)
            .bind(&id)
            .bind(&original_config)
            .execute(pool)
            .await
        {
            Ok(res) if res.rows_affected() > 0 => report.rewritten += 1,
            Ok(_) => {
                report.raced += 1;
                tracing::warn!(
                    "re-encrypt: backup_destinations[{id}] changed concurrently — skipped this run"
                );
            }
            Err(e) => {
                report.unreadable += 1;
                tracing::error!("re-encrypt: backup_destinations[{id}] write failed: {e}");
            }
        }
    }

    report
}

/// The Secrets Manager vault. Different key ladder from the credential path —
/// see `secrets_crypto::vault_key_sources` — so it needs its own arm rather
/// than sharing `reencrypt_credential`.
async fn reencrypt_vault_secrets(pool: &PgPool, jwt_secret: &str) -> SubjectReport {
    let mut report = SubjectReport::new("secrets.encrypted_value");

    let rows = match sqlx::query(
        "SELECT id::text AS id, encrypted_value FROM secrets \
         WHERE encrypted_value IS NOT NULL AND encrypted_value <> ''",
    )
    .fetch_all(pool)
    .await
    {
        Ok(r) => r,
        Err(e) => {
            tracing::warn!("re-encrypt: skipping secrets.encrypted_value: {e}");
            return report;
        }
    };

    for row in rows {
        let id: String = row.get("id");
        let value: String = row.get("encrypted_value");
        report.examined += 1;

        match secrets_crypto::reencrypt_vault(&value, jwt_secret) {
            Ok(None) => report.already_current += 1,
            Ok(Some(fresh)) => {
                // No `version` bump here: the plaintext is unchanged, only its
                // encryption wrapper is. `routes::secrets`' real edit path
                // pairs every `version` increment with an INSERT into
                // `secret_versions` under that same version number — bumping
                // `version` here without one would desync the two on every
                // successful re-key, not only a raced one. CAS on the
                // ciphertext read above catches the race the same way the
                // other three sweeps do.
                let res = sqlx::query(
                    "UPDATE secrets SET encrypted_value = $1, updated_at = NOW() \
                     WHERE id::text = $2 AND encrypted_value = $3",
                )
                .bind(&fresh)
                .bind(&id)
                .bind(&value)
                .execute(pool)
                .await;
                match res {
                    Ok(r) if r.rows_affected() > 0 => report.rewritten += 1,
                    Ok(_) => {
                        report.raced += 1;
                        tracing::warn!(
                            "re-encrypt: secrets[{id}] changed concurrently — skipped this run"
                        );
                    }
                    Err(e) => {
                        report.unreadable += 1;
                        tracing::error!("re-encrypt: secrets[{id}] write failed: {e}");
                    }
                }
            }
            Err(e) => {
                report.unreadable += 1;
                tracing::error!("re-encrypt: secrets[{id}] unreadable: {e}");
            }
        }
    }

    report
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Discover every route module that writes an encrypted credential and
    /// assert this sweep covers it.
    ///
    /// ⚠ This ENUMERATES; it does not confirm a number I already believe. The
    /// registry above is what someone remembered, and a registry that is only
    /// a list is a changelog — the next feature to encrypt a column joins the
    /// install silently and becomes the one row the sweep skips.
    ///
    /// The discovery is asserted NON-EMPTY first. An arm that examines nothing
    /// is green everywhere, and "found no writers" and "failed to look" are the
    /// same output without a floor.
    #[test]
    fn every_credential_writer_is_covered() {
        // BOTH trees. An earlier cut walked only `src/routes`, so a service
        // that encrypted a credential joined the install completely unseen —
        // and this diff created the first one (`services::agent` encrypts
        // `servers.agent_token` when it registers the local server). A
        // discovery scoped to one directory is a list of the places someone
        // remembered to look, which is the same defect one level up.
        //
        // `secrets_crypto` is the primitive and `credential_reencrypt` is this
        // sweep; both necessarily name the function and neither stores a
        // credential of its own, so they are excluded BY NAME and the floors
        // below keep that exclusion from quietly emptying the census.
        // The doors are DERIVED, not listed. An earlier cut walked
        // `["src/routes", "src/services"]` and asserted that both names
        // appeared in what it had walked — which is a tautology, because the
        // walked set IS that list. A mutation shortening it to routes-only
        // passed: no service was scanned, so no service writer was discovered,
        // so nothing was "missing". A census whose scope is a literal can
        // always be narrowed silently; one that walks the whole crate cannot.
        //
        // `secrets_crypto` is the primitive and `credential_reencrypt` is this
        // sweep; both necessarily name the function and neither stores a
        // credential of its own, so they are excluded BY NAME — and the floor
        // below keeps that exclusion from quietly emptying the census.
        const MACHINERY: &[&str] = &["secrets_crypto", "credential_reencrypt"];
        let src_root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
        let mut writers: Vec<String> = Vec::new();
        let mut scanned = 0usize;
        let mut stack = vec![src_root.clone()];

        while let Some(dir) = stack.pop() {
            for entry in std::fs::read_dir(&dir)
                .unwrap_or_else(|e| panic!("{} is readable: {e}", dir.display()))
            {
                let path = entry.expect("dir entry").path();
                if path.is_dir() {
                    stack.push(path);
                    continue;
                }
                if path.extension().and_then(|e| e.to_str()) != Some("rs") {
                    continue;
                }
                scanned += 1;
                let body = std::fs::read_to_string(&path).expect("source file is readable");
                // `decrypt_credential(` cannot match `encrypt_credential(` — different
                // letters — but `reencrypt_credential(` contains it verbatim, so it is
                // removed before the test rather than reasoned around.
                let body = body.replace("reencrypt_credential(", "");
                if body.contains("encrypt_credential(") {
                    let stem = path
                        .file_stem()
                        .and_then(|s| s.to_str())
                        .expect("source file has a name")
                        .to_string();
                    if stem != "mod" && !MACHINERY.contains(&stem.as_str()) {
                        writers.push(stem);
                    }
                }
            }
        }
        writers.sort();
        writers.dedup();

        // Positive control for the WALK itself, separate from the floor on its
        // RESULT below: a moved layout or a traversal that stops descending
        // yields few files, and "no writers found" would then read as a covered
        // tree rather than as an instrument that examined nothing.
        assert!(
            scanned >= 90,
            "the walk examined only {scanned} source file(s) under src/ — the crate layout moved \
             or the traversal stopped descending, so this census proves nothing about the tree"
        );

        // Floor: the tree is known to hold at least this many writers. A
        // discovery that collapses is a broken instrument, not a clean tree.
        assert!(
            writers.len() >= 8,
            "credential-writer discovery found only {} module(s) ({writers:?}) — the enumeration \
             is broken, not the tree",
            writers.len()
        );

        let missing: Vec<&String> = writers
            .iter()
            .filter(|m| !COVERED_MODULES.iter().any(|(name, _)| name == m))
            .collect();

        assert!(
            missing.is_empty(),
            "these route modules encrypt a credential but are not covered by the re-encryption \
             sweep: {missing:?}. Add the table/column to SIMPLE_SUBJECTS (or a special-shape arm) \
             and then add the module to COVERED_MODULES — in that order."
        );
    }

    /// The registry must not name a module that no longer writes anything —
    /// a stale entry makes the census above pass vacuously for that module.
    #[test]
    fn covered_modules_all_still_exist() {
        let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"));
        for (module, _) in COVERED_MODULES {
            let in_routes = root.join(format!("src/routes/{module}.rs"));
            let in_services = root.join(format!("src/services/{module}.rs"));
            assert!(
                in_routes.exists() || in_services.exists(),
                "COVERED_MODULES names `{module}`, but neither src/routes/{module}.rs nor \
                 src/services/{module}.rs exists"
            );
        }
    }

    /// Every subject a module is paired with must be one the sweep really visits.
    ///
    /// This is the arm that makes the pairing load-bearing rather than
    /// decorative. Adding `("cdn", "cdn_zones.api_key")` without also adding
    /// `("cdn_zones", "id", "api_key")` to SIMPLE_SUBJECTS fails HERE — which is
    /// the whole reason the pairing exists, because the old bare-name registry
    /// went green on exactly that half-edit and the column was then skipped by
    /// every re-key.
    #[test]
    fn subject_tokens_match_the_sweep() {
        let swept = swept_subjects();

        for (module, subject) in COVERED_MODULES {
            assert!(
                swept.iter().any(|s| s == subject),
                "COVERED_MODULES pairs `{module}` with subject `{subject}`, which the sweep never \
                 visits — add it to SIMPLE_SUBJECTS (or give it an arm and list it in \
                 SPECIAL_SUBJECTS). Swept subjects are: {swept:?}"
            );
        }

        // And the special arms must exist. Read from THIS file's source rather
        // than from the constant, so the list cannot drift away from the
        // `SubjectReport::new(..)` calls it claims to name — the two are
        // separate declarations, so this can actually fail.
        let src = std::fs::read_to_string(
            std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
                .join("src/services/credential_reencrypt.rs"),
        )
        .expect("this file is readable");
        let arms = src.matches("SubjectReport::new(").count();
        assert!(
            arms >= 4,
            "found {arms} SubjectReport::new call(s) — the sweep's arms moved and this check is \
             measuring an empty set"
        );
        for subject in SPECIAL_SUBJECTS {
            assert!(
                src.contains(&format!("SubjectReport::new(\"{subject}\")")),
                "SPECIAL_SUBJECTS names `{subject}` but no arm in this file reports it"
            );
        }
    }

    /// `SENSITIVE_SETTINGS_SQL`'s `IN (...)` list is SQL text, so it cannot
    /// `use` `SETTINGS_SENSITIVE_KEYS` the way `DESTINATION_SECRET_KEYS` above
    /// now imports its source of truth directly. This test keeps the two from
    /// drifting instead: this file used to carry a hand-copied SQL literal
    /// with nothing tying it back to `routes::settings::SENSITIVE_KEYS` — a
    /// settings key that module encrypts, added there alone, would have
    /// passed CI while never being re-keyed by this sweep.
    #[test]
    fn sensitive_settings_sql_covers_settings_sensitive_keys() {
        use crate::routes::settings::SENSITIVE_KEYS as SETTINGS_SENSITIVE_KEYS;
        // Floor: an empty or broken import must not make the loop below pass
        // vacuously, the same reasoning `scanned >= 90` and `writers.len() >=
        // 8` apply above.
        assert!(
            SETTINGS_SENSITIVE_KEYS.len() >= 2,
            "SETTINGS_SENSITIVE_KEYS is empty or unexpectedly small — the import from \
             routes::settings is broken, not the tree"
        );
        for key in SETTINGS_SENSITIVE_KEYS {
            assert!(
                SENSITIVE_SETTINGS_SQL.contains(&format!("'{key}'")),
                "routes::settings::SENSITIVE_KEYS contains `{key}` but SENSITIVE_SETTINGS_SQL's \
                 IN(...) list does not — add it there too"
            );
        }
    }
}
