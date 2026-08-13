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
//! `tests/credential-key-survival-pin-e2e.sh` §D asserts that every route
//! module containing an `encrypt_credential` call appears in
//! [`COVERED_MODULES`], and fails when one does not.

use sqlx::{PgPool, Row};

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
];

/// Route modules known to write an encrypted credential. Enforced against the
/// tree by `every_credential_writer_is_covered` below — that test failing IS
/// the anti-drift mechanism, the same shape as the generated-guidance test.
pub const COVERED_MODULES: &[&str] = &[
    "settings",
    "databases",
    "whmcs",
    "system",
    "sites",
    "migration",
    "mail",
    "dns",
    "backup_destinations",
    "auth",
];

/// `settings` rows whose `value` is ciphertext. Mirrors the predicate the two
/// writers in `routes::settings` apply (`SENSITIVE_KEYS` plus the
/// `_client_secret` suffix) and the `pdns_api_key` written by `routes::system`.
const SENSITIVE_SETTINGS_SQL: &str =
    "SELECT key::text AS id, value FROM settings \
     WHERE (key IN ('smtp_password', 'pdns_api_key') OR key LIKE '%\\_client\\_secret') \
       AND value IS NOT NULL AND value <> ''";

/// JSON keys inside `backup_destinations.config` that hold ciphertext. Mirrors
/// `routes::backup_destinations::CONFIG_SENSITIVE_KEYS`.
const DESTINATION_SECRET_KEYS: &[&str] = &["secret_key", "password"];

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
}

impl SubjectReport {
    fn new(subject: impl Into<String>) -> Self {
        Self {
            subject: subject.into(),
            examined: 0,
            rewritten: 0,
            already_current: 0,
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

    let update = format!("UPDATE {table} SET {value_col} = $1 WHERE {id_col}::text = $2");
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
                    .execute(pool)
                    .await
                {
                    Ok(_) => report.rewritten += 1,
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
                let res = sqlx::query("UPDATE settings SET value = $1, updated_at = NOW() WHERE key = $2")
                    .bind(&fresh)
                    .bind(&key)
                    .execute(pool)
                    .await;
                match res {
                    Ok(_) => report.rewritten += 1,
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
        let mut config: serde_json::Value = row.get("config");
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

        match sqlx::query("UPDATE backup_destinations SET config = $1 WHERE id::text = $2")
            .bind(&config)
            .bind(&id)
            .execute(pool)
            .await
        {
            Ok(_) => report.rewritten += 1,
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
                let res = sqlx::query(
                    "UPDATE secrets SET encrypted_value = $1, version = version + 1, \
                     updated_at = NOW() WHERE id::text = $2",
                )
                .bind(&fresh)
                .bind(&id)
                .execute(pool)
                .await;
                match res {
                    Ok(_) => report.rewritten += 1,
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
        let routes = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src/routes");
        let mut writers: Vec<String> = Vec::new();

        for entry in std::fs::read_dir(&routes).expect("routes dir is readable") {
            let path = entry.expect("dir entry").path();
            if path.extension().and_then(|e| e.to_str()) != Some("rs") {
                continue;
            }
            let body = std::fs::read_to_string(&path).expect("route file is readable");
            // `decrypt_credential(` cannot match `encrypt_credential(` — different
            // letters — but `reencrypt_credential(` contains it verbatim, so it is
            // removed before the test rather than reasoned around.
            let body = body.replace("reencrypt_credential(", "");
            if body.contains("encrypt_credential(") {
                let stem = path
                    .file_stem()
                    .and_then(|s| s.to_str())
                    .expect("route file has a name")
                    .to_string();
                if stem != "mod" {
                    writers.push(stem);
                }
            }
        }
        writers.sort();
        writers.dedup();

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
            .filter(|m| !COVERED_MODULES.contains(&m.as_str()))
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
        let routes = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src/routes");
        for module in COVERED_MODULES {
            let path = routes.join(format!("{module}.rs"));
            assert!(
                path.exists(),
                "COVERED_MODULES names `{module}`, but src/routes/{module}.rs does not exist"
            );
        }
    }
}
