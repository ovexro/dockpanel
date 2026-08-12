use axum::{
    http::StatusCode,
    routing::{get, post},
    Json, Router,
};
use serde::Deserialize;
use crate::safe_cmd::{safe_command, safe_command_unsandboxed};
use std::path::Path;

use super::AppState;

type ApiErr = (StatusCode, Json<serde_json::Value>);

fn err(status: StatusCode, msg: &str) -> ApiErr {
    (status, Json(serde_json::json!({ "error": msg })))
}

fn ok(msg: &str) -> Json<serde_json::Value> {
    Json(serde_json::json!({ "ok": true, "message": msg }))
}

// ── Request types ───────────────────────────────────────────────────────

#[derive(Deserialize)]
pub struct DkimRequest {
    pub domain: String,
    pub selector: String,
}

#[derive(Deserialize)]
pub struct DomainRequest {
    pub domain: String,
}

#[derive(Deserialize)]
pub struct SyncRequest {
    pub domains: Vec<SyncDomain>,
    pub accounts: Vec<SyncAccount>,
    pub aliases: Vec<SyncAlias>,
}

#[derive(Deserialize)]
pub struct SyncDomain {
    pub domain: String,
    pub enabled: bool,
    pub catch_all: Option<String>,
}

#[derive(Deserialize)]
pub struct SyncAccount {
    pub email: String,
    pub password_hash: String,
    pub quota_mb: i32,
    pub enabled: bool,
    pub forward_to: Option<String>,
}

#[derive(Deserialize)]
pub struct SyncAlias {
    pub source: String,
    pub destination: String,
}

#[derive(Deserialize)]
pub struct QueueDeleteRequest {
    pub id: String,
}

#[derive(Deserialize)]
struct RateLimitRequest {
    rate: String, // e.g., "100/hour", "500/day"
}

#[derive(Deserialize)]
struct MailboxBackupRequest {
    email: String,
}

const VMAIL_DIR: &str = "/var/vmail";
const POSTFIX_VIRTUAL_DOMAINS: &str = "/etc/postfix/virtual_domains";
const POSTFIX_VIRTUAL_MAILBOX: &str = "/etc/postfix/virtual_mailbox_maps";
const POSTFIX_VIRTUAL_ALIAS: &str = "/etc/postfix/virtual_alias_maps";
/// Which SASL login may use which envelope sender. Nothing bound the two until
/// this file existed, so any mailbox on the box could send as any address on
/// any other tenant's hosted domain — and leave DKIM-signed with that tenant's
/// key, because the signing table is keyed on the sender domain alone.
const POSTFIX_SENDER_LOGIN: &str = "/etc/postfix/sender_login_maps";
const DOVECOT_USERS: &str = "/etc/dovecot/users";
const DKIM_KEYS_DIR: &str = "/etc/dockpanel/dkim";
/// OpenDKIM's own config lives under the DockPanel data dir, not at the
/// distro's `/etc/opendkim.conf`, so it can be written under the hardened
/// agent sandbox (ProtectSystem=strict) — the unit's ReadWritePaths covers
/// /etc/dockpanel but not bare files in /etc. Same reasoning as the scanner
/// dir in services/image_scanner.rs. A systemd drop-in points the daemon here.
const OPENDKIM_CONF: &str = "/etc/dockpanel/opendkim.conf";
const OPENDKIM_DROPIN_DIR: &str = "/etc/systemd/system/opendkim.service.d";
/// OpenDKIM's milter endpoint, in OpenDKIM's own `Socket` syntax, and the
/// matching Postfix `smtpd_milters` value. A loopback port rather than a Unix
/// socket under Postfix's spool — see [`write_opendkim_config`] for why the
/// spool arrangement is Debian-only and SELinux-forbidden on RHEL.
const OPENDKIM_SOCKET: &str = "inet:8891@127.0.0.1";
const OPENDKIM_MILTER: &str = "inet:127.0.0.1:8891";
/// Rspamd's milter endpoint — the bind of its `rspamd_proxy` worker in milter
/// mode, which is what Postfix talks to. Named alongside [`OPENDKIM_MILTER`]
/// on purpose: these two are the only values `smtpd_milters` ever holds, and
/// keeping them together is what stops one moving without the other.
const RSPAMD_MILTER: &str = "inet:127.0.0.1:11332";
const KEY_TABLE: &str = "/etc/dockpanel/dkim/key.table";
const SIGNING_TABLE: &str = "/etc/dockpanel/dkim/signing.table";
/// Ports the mail stack listens on once installed. Opened in the firewall by
/// the installer — starting a listener the firewall drops is not an install.
const MAIL_PORTS: &[&str] = &["25", "587", "465", "143", "993", "110", "995"];
/// The Roundcube webmail image, deliberately NOT `:latest`. Rationale and the
/// digest this was verified against live at the single use site in
/// [`webmail_install`]; pinned here so the version is greppable from one place.
const ROUNDCUBE_IMAGE: &str = "roundcube/roundcubemail:1.7.x-apache";

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/mail/status", get(mail_status))
        .route("/mail/install", post(mail_install))
        .route("/mail/dkim/generate", post(dkim_generate))
        .route("/mail/domains/configure", post(domain_configure))
        .route("/mail/domains/remove", post(domain_remove))
        .route("/mail/sync", post(sync_config))
        .route("/mail/queue", get(queue_list))
        .route("/mail/queue/flush", post(queue_flush))
        .route("/mail/queue/delete", post(queue_delete))
        // Rspamd spam filter
        .route("/mail/rspamd/install", post(rspamd_install))
        .route("/mail/rspamd/status", get(rspamd_status))
        .route("/mail/rspamd/toggle", post(rspamd_toggle))
        // Webmail (Roundcube)
        .route("/mail/webmail/install", post(webmail_install))
        .route("/mail/webmail/status", get(webmail_status))
        .route("/mail/webmail/remove", post(webmail_remove))
        // SMTP Relay
        .route("/mail/relay/configure", post(relay_configure))
        .route("/mail/relay/status", get(relay_status))
        .route("/mail/relay/remove", post(relay_remove))
        // Logs & Storage
        .route("/mail/logs", get(mail_logs))
        .route("/mail/storage", get(storage_usage))
        // Rate Limiting
        .route("/mail/rate-limit/set", post(rate_limit_set))
        .route("/mail/rate-limit/status", get(rate_limit_status))
        .route("/mail/rate-limit/remove", post(rate_limit_remove))
        // Mailbox Backup/Restore
        .route("/mail/backup", post(mailbox_backup))
        .route("/mail/restore", post(mailbox_restore))
        .route("/mail/backups", get(mailbox_backups))
        .route("/mail/backups/delete", post(mailbox_backup_delete))
        // TLS Enforcement
        .route("/mail/tls/status", get(tls_status))
        .route("/mail/tls/enforce", post(tls_enforce))
        // Uninstall
        .route("/mail/uninstall", post(mail_uninstall))
}

// ── Mail server status + installation ────────────────────────────────────

async fn mail_status() -> Result<Json<serde_json::Value>, ApiErr> {
    let postfix = is_service_active("postfix").await;
    let dovecot = is_service_active("dovecot").await;
    let opendkim = is_service_active("opendkim").await;
    let postfix_installed = is_installed("postfix").await;
    let dovecot_installed = is_installed("dovecot-imapd").await;
    let opendkim_installed = is_installed("opendkim").await;
    let vmail_exists = Path::new(VMAIL_DIR).exists();
    let password_schemes = dovecot_password_schemes().await;

    // Packages present and services up is what apt gives you for free — its
    // postinst starts all three. It is NOT evidence that this installer ran:
    // for the whole life of the product `mail_install` aborted partway and
    // this endpoint still answered installed+running, so a failed install was
    // indistinguishable from a working one. Ask instead whether the
    // configuration the mail stack actually depends on is on disk.
    let configured = Path::new(OPENDKIM_CONF).exists()
        && Path::new(&format!("{DKIM_KEYS_DIR}/trusted.hosts")).exists()
        && tokio::fs::read_to_string("/etc/postfix/main.cf").await
            .map(|c| c.contains("DockPanel mail configuration"))
            .unwrap_or(false);

    // OpenDKIM counts. It used to be reported in its own sub-object and
    // excluded from the summary, so a box whose DKIM milter was in a restart
    // loop answered `installed:true, running:true, configured:true` — with
    // `opendkim.running:false` sitting in the same response, unread (measured
    // on Rocky 9.8, s268, where the milter exited 78/CONFIG on every start).
    // Mail still flows in that state because `milter_default_action = accept`,
    // so nothing looks wrong until a receiver rejects the unsigned mail. That
    // is the same "healthy while delivering nothing" shape `configured` was
    // added to close in s262 — closed one layer further here.
    let installed = postfix_installed && dovecot_installed && opendkim_installed && configured;
    let running = postfix && dovecot && opendkim && configured;

    Ok(Json(serde_json::json!({
        "installed": installed,
        "running": running,
        "configured": configured,
        "postfix": { "installed": postfix_installed, "running": postfix },
        "dovecot": { "installed": dovecot_installed, "running": dovecot },
        "opendkim": { "installed": opendkim_installed, "running": opendkim },
        "vmail_user": vmail_exists,
        // What THIS box's Dovecot can actually verify. The panel hashes
        // mailbox passwords centrally but the verifier is per-box, so the
        // scheme has to be chosen from here — see dovecot_password_schemes.
        "password_schemes": password_schemes,
    })))
}

async fn mail_install() -> Result<Json<serde_json::Value>, ApiErr> {
    use crate::services::pkg;

    if let Some(why) = pkg::no_installer_reason("Installing the mail server").await {
        return Err(err(StatusCode::NOT_IMPLEMENTED, &why));
    }
    // The packages resolve on the RHEL family, but nothing past them has been
    // driven there. Refusing is the honest answer until it has been — see the
    // reason's own comment for why a half-configured mail stack is worse than
    // an absent one.
    if let Some(why) = pkg::mail_refusal_reason().await {
        return Err(err(StatusCode::NOT_IMPLEMENTED, &why));
    }
    tracing::info!("Starting mail server installation...");

    // 1. Install packages.
    // The transaction runs unsandboxed so the package manager can take its own
    // locks — the agent unit's ProtectSystem=strict makes /var/lib/dpkg
    // read-only inside the namespace, which left apt with "Not using locking
    // for read only lock file" warnings followed by chown failures. Same #54-A
    // pattern as the vmail useradd/groupadd below. (issue #57 follow-up from
    // WiskeyPapa, v2.8.19.)
    //
    // Debian splits Dovecot into one package per protocol while the RHEL family
    // ships a single `dovecot`, so all three names collapse to one there —
    // handled by the name map rather than by branching here.
    if let Err(e) = pkg::install(&[
        "postfix",
        "dovecot-imapd",
        "dovecot-pop3d",
        "dovecot-lmtpd",
        "opendkim",
        "opendkim-tools",
    ])
    .await
    {
        // `nothing provides …` on the RHEL family almost always means CRB is
        // not enabled. Say so here rather than gating the installer behind a
        // probe that cannot run reliably inside the sandbox — see
        // pkg::explain_package_failure.
        let detail = pkg::explain_package_failure(&e).await;
        return Err(err(
            StatusCode::INTERNAL_SERVER_ERROR,
            &format!("Package install failed: {detail}"),
        ));
    }

    // 2. Create vmail user (uid/gid 5000).
    // groupadd/useradd write /etc/passwd, /etc/shadow, /etc/group — all too
    // sensitive to put in the agent's ReadWritePaths. Run unsandboxed via
    // systemd-run, the same escape used for apt/dpkg (#54-A pattern, v2.8.14).
    let _ = safe_command_unsandboxed("groupadd", &[]).args(["-g", "5000", "vmail"]).output().await;
    let _ = safe_command_unsandboxed("useradd", &[]).args(["-g", "5000", "-u", "5000", "-d", VMAIL_DIR, "-s", "/usr/sbin/nologin", "-m", "vmail"]).output().await;
    // Created via the unsandboxed escape, not tokio::fs.
    //
    // The agent unit lists `-/var/vmail`, and systemd's `-` prefix means "bind
    // this read-write IF IT EXISTS". For a directory the agent merely writes
    // INTO that is fine; for one the agent must CREATE it is not — on a box
    // where /var/vmail is absent at unit start, the path is simply not in the
    // namespace and `mkdir` fails with EROFS. Today setup.sh happens to
    // pre-create it, so the installer's ability to work depends on a mirror in
    // a different file staying in step (measured s268 by removing the
    // directory: "Failed to create vmail dir: Read-only file system").
    let _ = safe_command_unsandboxed("mkdir", &[]).args(["-p", VMAIL_DIR]).output().await;
    if !Path::new(VMAIL_DIR).exists() {
        return Err(err(
            StatusCode::INTERNAL_SERVER_ERROR,
            &format!("Failed to create {VMAIL_DIR} — mail cannot be stored"),
        ));
    }
    let _ = safe_command_unsandboxed("chown", &[]).args(["-R", "vmail:vmail", VMAIL_DIR]).output().await;
    label_vmail_for_selinux().await;

    // 3. Create config directories
    tokio::fs::create_dir_all(DKIM_KEYS_DIR).await
        .map_err(|e| err(StatusCode::INTERNAL_SERVER_ERROR, &format!("Failed to create DKIM dir: {e}")))?;
    tokio::fs::create_dir_all("/etc/dockpanel/mail").await
        .map_err(|e| err(StatusCode::INTERNAL_SERVER_ERROR, &format!("Failed to create mail config dir: {e}")))?;

    // 4. Write Postfix main.cf additions for virtual mailbox hosting
    let postfix_config = format!(r#"
# DockPanel mail configuration
virtual_mailbox_domains = /etc/postfix/virtual_domains
virtual_mailbox_maps = hash:/etc/postfix/virtual_mailbox_maps
# Nothing is delivered by the local transport. mydestination defaults to
# include $myhostname, and when the panel host is also a hosted mail domain
# that default silently outranks virtual_mailbox_domains: mail for a real
# mailbox is handed to `local`, which has no such Unix user, and bounces
# "unknown user". A virtual-mailbox host must claim nothing but localhost.
mydestination = localhost
virtual_alias_maps = hash:/etc/postfix/virtual_alias_maps
virtual_mailbox_base = /var/vmail
virtual_uid_maps = static:5000
virtual_gid_maps = static:5000
virtual_transport = lmtp:unix:private/dovecot-lmtp

# SMTP authentication via Dovecot
smtpd_sasl_type = dovecot
smtpd_sasl_path = private/auth
smtpd_sasl_auth_enable = yes
# Trust only this host's own addresses (not the whole subnet) so permit_mynetworks can't let a
# same-subnet neighbour relay unauthenticated on shared-network hosts.
mynetworks_style = host
smtpd_recipient_restrictions = permit_sasl_authenticated, permit_mynetworks, reject_unauth_destination

# TLS
smtpd_tls_security_level = may
smtpd_tls_auth_only = yes

# SMTP smuggling prevention (CVE-2023-51764)
smtpd_forbid_bare_newline = yes

# OpenDKIM milter
milter_protocol = 6
milter_default_action = accept
smtpd_milters = {OPENDKIM_MILTER}
non_smtpd_milters = {OPENDKIM_MILTER}
"#);

    // Append to main.cf if not already configured
    let main_cf = tokio::fs::read_to_string("/etc/postfix/main.cf").await.unwrap_or_default();
    if !main_cf.contains("DockPanel mail configuration") {
        let new_content = format!("{main_cf}\n{postfix_config}");
        write_file_atomic("/etc/postfix/main.cf", &new_content).await
            .map_err(|e| err(StatusCode::INTERNAL_SERVER_ERROR, &format!("Failed to write main.cf: {e}")))?;
    }

    // 4a. Listen on every interface.
    //
    // Debian's postfix debconf writes `inet_interfaces = all` during its
    // "Internet Site" setup; the RHEL package ships `localhost`, so Postfix
    // bound 127.0.0.1:25 only and NO mail could ever arrive from another host —
    // with the firewall correctly opened and the installer reporting success
    // (measured on Rocky 9.8, s268).
    //
    // Set with `postconf -e` rather than appended to the config block above:
    // main.cf already carries a value on the RHEL family, and appending a
    // second one leaves Postfix warning "overriding earlier entry" on every
    // single postconf invocation. Correct behaviour, permanent noise.
    let _ = safe_command("postconf").arg("inet_interfaces=all").output().await;

    // 4b. HELO name. Left unset, Postfix uses the short OS hostname, which
    // receivers score heavily against: on a stock cloud image the delivered
    // mail earned HFILTER_HELO_5 + HFILTER_HOSTNAME_UNKNOWN + MID_RHS_NOT_FQDN
    // — six spam points before any content was examined.
    if let Some(host) = panel_server_name() {
        let _ = safe_command("postconf").arg(format!("myhostname={host}")).output().await;
        // Applied here too, not only in the block above: that block is appended
        // once and skipped forever after, so an install that already has it
        // would take the new myhostname and keep the dangerous default
        // mydestination — the combination that bounces real mailboxes.
        let _ = safe_command("postconf").arg("mydestination=localhost").output().await;
        tracing::info!("Postfix myhostname set to {host}, mydestination narrowed to localhost");
    }

    // 4c. Serve the box's real certificate where it has one, so that clients
    // which verify their peer can connect at all.
    if let Some((cert, key)) = panel_tls_paths() {
        let _ = safe_command("postconf").arg(format!("smtpd_tls_cert_file={cert}")).output().await;
        let _ = safe_command("postconf").arg(format!("smtpd_tls_key_file={key}")).output().await;
    }

    // 5. Enable submission port (587) in master.cf.
    // Test for an ACTIVE entry, not merely the string: the stock Ubuntu file
    // ships the service commented out, and the old test matched that comment
    // forever — so every re-run appended another live block and Postfix warned
    // "duplicate master.cf entry for service submission". Since a failed
    // install is retried by hand, re-running was the normal case.
    let master_cf = tokio::fs::read_to_string("/etc/postfix/master.cf").await.unwrap_or_default();
    let has_active_submission = master_cf.lines().any(|l| {
        let t = l.trim_start();
        !t.starts_with('#') && t.starts_with("submission") && t.contains("inet")
    });
    if !has_active_submission {
        let submission_config = "\nsubmission inet n - y - - smtpd\n  -o syslog_name=postfix/submission\n  -o smtpd_tls_security_level=encrypt\n  -o smtpd_sasl_auth_enable=yes\n  -o smtpd_recipient_restrictions=permit_sasl_authenticated,reject\n";
        let new_master = format!("{master_cf}\n{submission_config}");
        write_file_atomic("/etc/postfix/master.cf", &new_master).await
            .map_err(|e| err(StatusCode::INTERNAL_SERVER_ERROR, &format!("Failed to write master.cf: {e}")))?;
    }

    // 6. Write Dovecot configuration for virtual users.
    // `ssl = required` without a trusted certificate is not a working IMAPS
    // service: Dovecot falls back to the distro snakeoil and every client that
    // verifies its peer is turned away with "unknown ca" — including the
    // Roundcube this product installs and points at ssl://<domain>:993. Where
    // the box already holds a Let's Encrypt certificate for the panel host,
    // serve that.
    let dovecot_tls = match panel_tls_paths() {
        Some((cert, key)) => {
            tracing::info!("Dovecot will serve the panel's Let's Encrypt certificate");
            format!("ssl = required\nssl_cert = <{cert}\nssl_key = <{key}\n")
        }
        None => "ssl = required\n".to_string(),
    };
    let dovecot_base = r#"# DockPanel Dovecot configuration
protocols = imap pop3 lmtp

mail_location = maildir:/var/vmail/%d/%n
mail_uid = 5000
mail_gid = 5000
first_valid_uid = 5000

# Authentication
passdb {
  driver = passwd-file
  args = /etc/dovecot/users
}

userdb {
  driver = passwd-file
  args = /etc/dovecot/users
  default_fields = uid=5000 gid=5000 home=/var/vmail/%d/%n
}

# LMTP for Postfix delivery
service lmtp {
  unix_listener /var/spool/postfix/private/dovecot-lmtp {
    mode = 0600
    user = postfix
    group = postfix
  }
}

# SASL auth for Postfix
service auth {
  unix_listener /var/spool/postfix/private/auth {
    mode = 0660
    user = postfix
    group = postfix
  }
}

# SSL
"#;
    let dovecot_config = format!("{dovecot_base}{dovecot_tls}");

    write_file_atomic("/etc/dovecot/conf.d/99-dockpanel.conf", &dovecot_config).await
        .map_err(|e| err(StatusCode::INTERNAL_SERVER_ERROR, &format!("Failed to write dovecot config: {e}")))?;

    // 7. Create the map files if they do not exist yet.
    // Never truncate: these hold every hosted domain, every mailbox path and
    // every password hash. Writing them unconditionally meant re-running the
    // installer erased all mail routing on the box, and since the installer
    // used to fail with a 500, re-running it was the ordinary thing to do.
    for path in [POSTFIX_VIRTUAL_DOMAINS, POSTFIX_VIRTUAL_MAILBOX, POSTFIX_VIRTUAL_ALIAS] {
        if !Path::new(path).exists() {
            write_file_atomic(path, "").await
                .map_err(|e| err(StatusCode::INTERNAL_SERVER_ERROR, &format!("Failed to write {path}: {e}")))?;
        }
    }
    if !Path::new(DOVECOT_USERS).exists() {
        write_dovecot_users("").await
            .map_err(|e| err(StatusCode::INTERNAL_SERVER_ERROR, &format!("Failed to write dovecot users: {e}")))?;
    }
    let _ = safe_command("postmap").arg(POSTFIX_VIRTUAL_MAILBOX).output().await;
    let _ = safe_command("postmap").arg(POSTFIX_VIRTUAL_ALIAS).output().await;

    // 7a. Arm sender-identity enforcement, so a box is protected from its first
    // mailbox rather than from its first mailbox CHANGE. Deliberately not part
    // of the create-if-missing loop above: that loop discards postmap's exit
    // status, and this is the one map whose hash must be known good before the
    // restriction naming it is written. Reading the file first is what makes a
    // re-run of the installer preserve an existing map — the same reason the
    // loop above never truncates.
    let existing_sender_login =
        tokio::fs::read_to_string(POSTFIX_SENDER_LOGIN).await.unwrap_or_default();
    ensure_sender_login_enforcement(&existing_sender_login).await;

    // 8. Configure OpenDKIM. Its config and the drop-in that points the daemon
    // at it both live inside ReadWritePaths — writing the distro's
    // /etc/opendkim.conf is refused under ProtectSystem=strict, and that single
    // EROFS is what aborted this installer on every install ever made, taking
    // the DKIM tables, the socket directory and step 9 down with it.
    write_opendkim_config().await
        .map_err(|e| err(StatusCode::INTERNAL_SERVER_ERROR, &format!("Failed to configure OpenDKIM: {e}")))?;

    let trusted_hosts = "127.0.0.1\nlocalhost\n";
    write_file_atomic(&format!("{DKIM_KEYS_DIR}/trusted.hosts"), trusted_hosts).await
        .map_err(|e| err(StatusCode::INTERNAL_SERVER_ERROR, &format!("Failed to write trusted.hosts: {e}")))?;

    // The milter is a loopback port now, so there is no shared socket
    // directory to create, no `opendkim` group for Postfix to join, and no
    // stale socket to clean up. All three went away with the chroot coupling
    // — see write_opendkim_config.

    // 9. Enable and start services
    if let Ok(out) = safe_command("systemctl").args(["enable", "postfix", "dovecot", "opendkim"]).output().await {
        if !out.status.success() {
            tracing::warn!("Failed to enable mail services: {}", String::from_utf8_lossy(&out.stderr));
        }
    }
    for service in &["postfix", "dovecot", "opendkim"] {
        if let Ok(out) = safe_command("systemctl").args(["restart", service]).output().await {
            if !out.status.success() {
                tracing::warn!("Failed to restart {service}: {}", String::from_utf8_lossy(&out.stderr));
            }
        } else {
            tracing::warn!("Failed to execute systemctl restart {service}");
        }
    }

    // 10. Bind whatever keys already exist to their domains. On a first install
    // there are none and both tables are written empty; when mail is
    // reinstalled on a box that already has domains, this restores signing
    // without making the operator re-add each one.
    if let Err(e) = rebuild_dkim_tables().await {
        tracing::warn!("Failed to build DKIM tables: {e}");
    }

    // 11. A listener the firewall drops is not an installed service.
    open_mail_ports().await;

    tracing::info!("Mail server installation complete");

    Ok(ok("Mail server installed and configured"))
}

/// POST /mail/uninstall — Remove mail server packages and configuration.
async fn mail_uninstall() -> Result<Json<serde_json::Value>, ApiErr> {
    tracing::info!("Starting mail server uninstall...");

    // 1. Stop and disable services
    for service in &["postfix", "dovecot", "opendkim"] {
        let _ = tokio::time::timeout(
            std::time::Duration::from_secs(120),
            safe_command("systemctl")
                .args(["stop", service])
                .output()
        ).await;
        let _ = tokio::time::timeout(
            std::time::Duration::from_secs(120),
            safe_command("systemctl")
                .args(["disable", service])
                .output()
        ).await;
    }

    // 2. Remove packages through the package abstraction.
    //
    // This call site shelled straight to `apt-get purge` and `apt-get
    // autoremove`. s266 converted the INSTALLERS and missed the uninstaller, so
    // on an RPM box uninstall failed with "Failed to find executable apt-get" —
    // a true sentence that tells an operator nothing. `pkg::remove` already
    // exists and maps the three Debian Dovecot package names onto the single
    // RPM `dovecot`; counting the call sites, not just fixing the one in front
    // of you, is what turns a bug into a class (#92b).
    crate::services::pkg::remove(&[
        "postfix", "dovecot-imapd", "dovecot-pop3d", "dovecot-lmtpd",
        "opendkim", "opendkim-tools",
    ])
    .await
    .map_err(|e| err(StatusCode::INTERNAL_SERVER_ERROR, &format!("Package removal failed: {e}")))?;

    // 4. Remove DockPanel mail config dirs (NOT /var/vmail — user mail data)
    let _ = tokio::fs::remove_dir_all("/etc/dockpanel/mail").await;
    let _ = tokio::fs::remove_dir_all("/etc/dockpanel/dkim").await;

    tracing::info!("Mail server uninstalled (user mail data preserved in /var/vmail)");

    Ok(ok("Mail server uninstalled. Note: /var/vmail (user mail data) was NOT removed. Delete it manually if no longer needed."))
}

/// The password schemes THIS box's Dovecot can verify, upper-cased.
///
/// **Argon2id is a build option, not a version.** The panel hashes mailbox
/// passwords as `{ARGON2ID}` and the code that does it says Dovecot ">= 2.3.11
/// verifies natively" — which is false in the way that matters. Rocky 9.8
/// ships Dovecot **2.3.16**, comfortably past that version, built WITHOUT
/// libsodium: `doveadm pw -l` lists no ARGON2I or ARGON2ID at all, and every
/// login fails with `Unknown scheme ARGON2ID` while the panel reports the
/// account created successfully. Debian's 2.3.21 does list them.
///
/// That is s259's finding on a different family — a mailbox nobody could open,
/// with file ownership and password hash both perfect — so the fix is to ask
/// the box rather than to assume a version implies a capability.
///
/// An empty vector means "could not tell" (Dovecot not installed yet, or
/// `doveadm` missing); callers treat that as "use the safe default" rather
/// than as "supports nothing".
async fn dovecot_password_schemes() -> Vec<String> {
    let Ok(out) = safe_command("doveadm").args(["pw", "-l"]).output().await else {
        return Vec::new();
    };
    if !out.status.success() {
        return Vec::new();
    }
    String::from_utf8_lossy(&out.stdout)
        .split_whitespace()
        .map(|s| s.trim().to_ascii_uppercase())
        .filter(|s| !s.is_empty())
        .collect()
}

/// Give `/var/vmail` the SELinux type Dovecot is allowed to write.
///
/// **This is the one that silently ate every message.** `/var/vmail` is created
/// by this installer, so it inherits `var_t` from `/var`. Dovecot's delivery
/// process runs as `dovecot_t`, which the shipped policy permits to write
/// `mail_spool_t` and not `var_t` — so LMTP fails `mkdir(.../cur)` with
/// "Permission denied" on a directory whose UNIX ownership is `vmail:vmail`
/// and mode 0755. Every message is `deferred`, retried, and never delivered,
/// while `postfix` and `dovecot` are both active and the panel calls the mail
/// server healthy (measured on Rocky 9.8, s268; settled with `setenforce 0`
/// per #94a rather than by reading logs).
///
/// A no-op where SELinux is not enforcing, which is why it is safe to call on
/// every distro rather than behind an RPM branch (#94d): `selinuxenabled`
/// exits non-zero on Debian and the function returns immediately.
///
/// `semanage` records the rule so it survives a filesystem relabel;
/// `restorecon` applies it now. `policycoreutils-python-utils` carries
/// `semanage` and is not installed by default on a minimal RHEL box, so it is
/// pulled in first — but its absence is not fatal, because `chcon` still fixes
/// the running system even when the durable rule cannot be written.
async fn label_vmail_for_selinux() {
    let enforcing = safe_command("selinuxenabled")
        .output()
        .await
        .map(|o| o.status.success())
        .unwrap_or(false);
    if !enforcing {
        return;
    }

    if safe_command("semanage").arg("--help").output().await.map(|o| !o.status.success()).unwrap_or(true) {
        let _ = crate::services::pkg::install_available(&["policycoreutils-python-utils"]).await;
    }

    let spec = format!("{VMAIL_DIR}(/.*)?");
    let out = safe_command_unsandboxed("semanage", &[])
        .args(["fcontext", "-a", "-t", "mail_spool_t", &spec])
        .output()
        .await;
    match out {
        Ok(o) if o.status.success() => tracing::info!("SELinux: {VMAIL_DIR} recorded as mail_spool_t"),
        // Already recorded is the normal case on a re-install, not a failure.
        Ok(o) => {
            let e = String::from_utf8_lossy(&o.stderr);
            if e.contains("already defined") {
                tracing::info!("SELinux: {VMAIL_DIR} fcontext rule already present");
            } else {
                tracing::warn!("SELinux: could not record fcontext for {VMAIL_DIR}: {}", e.trim());
            }
        }
        Err(e) => tracing::warn!("SELinux: semanage unavailable ({e}) — falling back to chcon"),
    }

    // Apply now. restorecon uses the rule above; chcon is the fallback when the
    // rule could not be written, and is enough to make mail flow until a
    // relabel. Without either, delivery fails silently.
    let restored = safe_command("restorecon")
        .args(["-R", VMAIL_DIR])
        .output()
        .await
        .map(|o| o.status.success())
        .unwrap_or(false);
    if !restored {
        let _ = safe_command("chcon")
            .args(["-R", "-t", "mail_spool_t", VMAIL_DIR])
            .output()
            .await;
    }
}

async fn is_service_active(name: &str) -> bool {
    safe_command("systemctl")
        .args(["is-active", "--quiet", name])
        .output()
        .await
        .map(|o| o.status.success())
        .unwrap_or(false)
}

/// Package presence, on dpkg and rpm boxes alike — see `services::pkg`, which
/// also maps Debian's three Dovecot packages onto the single RPM `dovecot`.
async fn is_installed(package: &str) -> bool {
    crate::services::pkg::is_installed(package).await
}

// ── OpenDKIM wiring ─────────────────────────────────────────────────────

/// Render OpenDKIM's config and the systemd drop-in that makes the daemon read
/// it from the DockPanel data dir instead of the distro path.
///
/// Both targets are inside the agent's ReadWritePaths; the distro path is not,
/// which is why writing it aborted the whole installer before anything below
/// step 8 could run.
async fn write_opendkim_config() -> Result<(), String> {
    // Run as the packaged `opendkim` user, which owns the keys.
    //
    // The socket is a LOOPBACK TCP port, not a Unix socket in Postfix's spool.
    // The old arrangement — `local:/var/spool/postfix/opendkim/opendkim.sock` —
    // exists only because Debian runs smtpd CHROOTED to /var/spool/postfix, so
    // the milter has to live inside that tree. On the RHEL family `master.cf`
    // ships `smtp inet … n …` (not chrooted), the chroot buys nothing, and
    // SELinux actively forbids the arrangement: the packaged policy runs
    // OpenDKIM as `dkim_milter_t`, which is denied `search` on
    // `postfix_spool_t` (measured on Rocky 9.8, s268), so the daemon cannot
    // create its socket and never starts.
    //
    // A loopback port is reachable from inside a chroot and outside one alike,
    // needs no shared group, no socket directory and no ownership dance — so
    // BOTH families run the same path (#94d). `dkim_milter_port_t` already
    // covers 8891 in the shipped policy, so nothing needs relabelling.
    //
    // TrustAnchorFile is emitted only when an anchor actually exists.
    // `/usr/share/dns/root.key` is Debian's (`dns-root-data`); RHEL keeps its
    // anchor at `/var/lib/unbound/root.key` and may have neither. OpenDKIM
    // treats a missing anchor as a FATAL config error — `status=78/CONFIG`, so
    // the daemon never starts and nothing is ever signed (measured s268).
    let trust_anchor = ["/usr/share/dns/root.key", "/var/lib/unbound/root.key"]
        .into_iter()
        .find(|p| Path::new(p).exists())
        .map(|p| format!("TrustAnchorFile {p}\n"))
        .unwrap_or_default();
    let conf = format!(
        "Syslog yes\nUserID opendkim\nUMask 007\n\
         Socket {OPENDKIM_SOCKET}\n\
         PidFile /run/opendkim/opendkim.pid\nOversignHeaders From\n\
         {trust_anchor}\
         KeyTable {KEY_TABLE}\nSigningTable refile:{SIGNING_TABLE}\n\
         ExternalIgnoreList {DKIM_KEYS_DIR}/trusted.hosts\n\
         InternalHosts {DKIM_KEYS_DIR}/trusted.hosts\n"
    );
    write_file_atomic(OPENDKIM_CONF, &conf).await?;

    tokio::fs::create_dir_all(OPENDKIM_DROPIN_DIR).await
        .map_err(|e| format!("Failed to create {OPENDKIM_DROPIN_DIR}: {e}"))?;
    // The drop-in owns BOTH `Type` and `ExecStart`, so the packaged unit's
    // choice stops mattering — which is the whole point. Debian ships
    // `Type=forking` (a foreground daemon never returns and systemd times the
    // start out) while EPEL ships `Type=simple` with `-f` already in its own
    // ExecStart (a backgrounding daemon makes systemd reap the parent and log
    // `Deactivated successfully` while the unit goes INACTIVE — a failure that
    // does not even register as failed). Pinning Type=simple and passing `-f`
    // is one arrangement that is correct on both, instead of a flag whose
    // rightness depends on a file we do not control.
    let dropin = format!(
        "[Service]\nType=simple\nExecStart=\nExecStart=/usr/sbin/opendkim -f -x {OPENDKIM_CONF}\n"
    );
    write_file_atomic(&format!("{OPENDKIM_DROPIN_DIR}/dockpanel.conf"), &dropin).await?;
    let _ = safe_command("systemctl").arg("daemon-reload").output().await;
    Ok(())
}

/// Rebuild `key.table` and `signing.table` from the keys actually present on
/// disk, then restart OpenDKIM so it loads them.
///
/// Derived from the filesystem rather than accumulated incrementally, so it is
/// idempotent and cannot drift from reality no matter which caller runs it —
/// domain added, domain removed, or a re-run of the installer. Without these
/// two tables OpenDKIM holds no keys and signs nothing, which is the state
/// every install shipped in.
async fn rebuild_dkim_tables() -> Result<(), String> {
    let mut key_lines = Vec::new();
    let mut signing_lines = Vec::new();

    // Make the key tree reachable and owned by the daemon before listing it.
    // Two things bite here and both are silent:
    //   * /etc/dockpanel is 0700 root:root, so opendkim cannot even traverse to
    //     a key whose own mode is perfect. 0710 with the group grants traverse
    //     without making the directory listable or exposing api.env.
    //   * dkim_generate's chown is best-effort and runs while `apt` may still
    //     be creating the opendkim user, so keys can be left root-owned.
    // Re-applying both here means every caller of this helper converges on a
    // readable tree, instead of each one having to remember.
    let _ = safe_command("chgrp").args(["opendkim", "/etc/dockpanel"]).output().await;
    let _ = safe_command("chmod").args(["0710", "/etc/dockpanel"]).output().await;
    let _ = safe_command("chown").args(["-R", "opendkim:opendkim", DKIM_KEYS_DIR]).output().await;
    let _ = safe_command("chmod").args(["0750", DKIM_KEYS_DIR]).output().await;

    let mut entries = match tokio::fs::read_dir(DKIM_KEYS_DIR).await {
        Ok(e) => e,
        Err(e) => return Err(format!("Failed to read {DKIM_KEYS_DIR}: {e}")),
    };
    while let Ok(Some(entry)) = entries.next_entry().await {
        if !entry.path().is_dir() { continue; }
        let domain = entry.file_name().to_string_lossy().to_string();
        // Defence in depth: the domain came from a validated request, but this
        // string is about to become a line in a config file.
        if domain.is_empty()
            || !domain.chars().all(|c| c.is_ascii_alphanumeric() || c == '.' || c == '-') {
            continue;
        }
        let mut keys = match tokio::fs::read_dir(entry.path()).await {
            Ok(k) => k,
            Err(_) => continue,
        };
        while let Ok(Some(kf)) = keys.next_entry().await {
            let name = kf.file_name().to_string_lossy().to_string();
            let Some(selector) = name.strip_suffix(".private") else { continue };
            if selector.is_empty()
                || !selector.chars().all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_') {
                continue;
            }
            let path = kf.path();
            let path = path.to_string_lossy();
            key_lines.push(format!("{selector}._domainkey.{domain} {domain}:{selector}:{path}"));
            signing_lines.push(format!("*@{domain} {selector}._domainkey.{domain}"));
        }
    }

    write_file_atomic(KEY_TABLE, &key_lines.join("\n")).await?;
    write_file_atomic(SIGNING_TABLE, &signing_lines.join("\n")).await?;

    // OpenDKIM reads both tables at startup only.
    if let Ok(out) = safe_command("systemctl").args(["restart", "opendkim"]).output().await {
        if !out.status.success() {
            tracing::warn!("Failed to restart opendkim after table rebuild: {}",
                String::from_utf8_lossy(&out.stderr));
        }
    }
    tracing::info!("DKIM tables rebuilt: {} key(s) across {} entr(ies)",
        key_lines.len(), signing_lines.len());
    Ok(())
}

/// Allow the ports the mail stack listens on. `setup.sh` opens 80/443 and the
/// panel port and nothing else, so without this the installer finishes with
/// Postfix and Dovecot listening behind a firewall that drops every packet.
async fn open_mail_ports() {
    // This used to shell out to `ufw` and discard every result, then log
    // "Mail ports opened in firewall" unconditionally — a sentence that was
    // false on every RHEL-family box, where firewalld is the firewall and ufw
    // is usually not even installed (s265). Say what actually happened.
    let failed = crate::services::firewall::allow_tcp_ports(MAIL_PORTS).await;
    if failed.is_empty() {
        tracing::info!(
            "Mail ports opened in {:?}: {}",
            crate::services::firewall::detect().await,
            MAIL_PORTS.join(", ")
        );
    } else {
        tracing::warn!(
            "Mail ports NOT opened: {} — mail will not be deliverable until they are",
            failed.join(", ")
        );
    }
}

/// The panel's own Let's Encrypt certificate, when this box has one for the
/// host the panel is served on. Dovecot and Postfix otherwise fall back to the
/// distro's self-signed snakeoil, which any IMAP client that verifies its peer
/// refuses — Roundcube among them.
fn panel_tls_paths() -> Option<(String, String)> {
    let host = panel_server_name()?;
    let dir = format!("/etc/letsencrypt/live/{host}");
    let cert = format!("{dir}/fullchain.pem");
    let key = format!("{dir}/privkey.pem");
    (Path::new(&cert).exists() && Path::new(&key).exists()).then_some((cert, key))
}

// ── DKIM key generation ─────────────────────────────────────────────────

async fn dkim_generate(
    Json(body): Json<DkimRequest>,
) -> Result<Json<serde_json::Value>, ApiErr> {
    let domain = body.domain.trim();
    let selector = body.selector.trim();

    if domain.is_empty() || selector.is_empty() {
        return Err(err(StatusCode::BAD_REQUEST, "Domain and selector required"));
    }

    if domain.contains('/') || domain.contains('\\') || domain.contains("..")
        || !domain.chars().all(|c| c.is_ascii_alphanumeric() || c == '.' || c == '-') {
        return Err(err(StatusCode::BAD_REQUEST, "Invalid domain format"));
    }
    if selector.contains('/') || selector.contains('\\') || selector.contains("..")
        || !selector.chars().all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_') {
        return Err(err(StatusCode::BAD_REQUEST, "Invalid selector format"));
    }

    // Create DKIM directory
    let key_dir = format!("{DKIM_KEYS_DIR}/{domain}");
    tokio::fs::create_dir_all(&key_dir).await
        .map_err(|e| err(StatusCode::INTERNAL_SERVER_ERROR, &format!("Failed to create DKIM dir: {e}")))?;

    let private_path = format!("{key_dir}/{selector}.private");
    let public_path = format!("{key_dir}/{selector}.public");

    // Generate RSA key pair
    let output = safe_command("openssl")
        .args(["genrsa", "-out", &private_path, "2048"])
        .output()
        .await
        .map_err(|e| err(StatusCode::INTERNAL_SERVER_ERROR, &format!("openssl genrsa failed: {e}")))?;

    if !output.status.success() {
        return Err(err(StatusCode::INTERNAL_SERVER_ERROR, "Failed to generate DKIM private key"));
    }

    // Extract public key
    let output = safe_command("openssl")
        .args(["rsa", "-in", &private_path, "-pubout", "-out", &public_path])
        .output()
        .await
        .map_err(|e| err(StatusCode::INTERNAL_SERVER_ERROR, &format!("openssl rsa failed: {e}")))?;

    if !output.status.success() {
        return Err(err(StatusCode::INTERNAL_SERVER_ERROR, "Failed to extract DKIM public key"));
    }

    // Read keys
    let private_key = tokio::fs::read_to_string(&private_path).await
        .map_err(|e| err(StatusCode::INTERNAL_SERVER_ERROR, &format!("Failed to read private key: {e}")))?;
    let public_key = tokio::fs::read_to_string(&public_path).await
        .map_err(|e| err(StatusCode::INTERNAL_SERVER_ERROR, &format!("Failed to read public key: {e}")))?;

    // Set permissions
    let _ = safe_command("chmod").args(["600", &private_path]).output().await;
    let _ = safe_command("chown").args(["opendkim:opendkim", &private_path]).output().await;

    // Bind the new key to its domain. Generating a keypair and publishing the
    // public half in DNS is not DKIM: until the domain appears in OpenDKIM's
    // tables nothing signs with it, and the DNS record verifies green while
    // every message leaves unsigned.
    if let Err(e) = rebuild_dkim_tables().await {
        tracing::warn!("DKIM keys generated for {domain} but tables not rebuilt: {e}");
    }

    tracing::info!("DKIM keys generated for {domain} (selector: {selector})");

    Ok(Json(serde_json::json!({
        "private_key": private_key,
        "public_key": public_key,
        "selector": selector,
    })))
}

// ── Domain configuration ────────────────────────────────────────────────

async fn domain_configure(
    Json(body): Json<DomainRequest>,
) -> Result<Json<serde_json::Value>, ApiErr> {
    let domain = body.domain.trim();

    if domain.contains('/') || domain.contains('\\') || domain.contains("..")
        || !domain.chars().all(|c| c.is_ascii_alphanumeric() || c == '.' || c == '-') {
        return Err(err(StatusCode::BAD_REQUEST, "Invalid domain format"));
    }

    // Create vmail directory for domain
    let maildir = format!("{VMAIL_DIR}/{domain}");
    tokio::fs::create_dir_all(&maildir).await
        .map_err(|e| err(StatusCode::INTERNAL_SERVER_ERROR, &format!("Failed to create maildir: {e}")))?;

    // Set ownership to vmail user
    let _ = safe_command("chown").args(["-R", "vmail:vmail", &maildir]).output().await;

    tracing::info!("Mail domain configured: {domain}");
    Ok(ok(&format!("Domain {domain} configured")))
}

async fn domain_remove(
    Json(body): Json<DomainRequest>,
) -> Result<Json<serde_json::Value>, ApiErr> {
    let domain = body.domain.trim();

    if domain.contains('/') || domain.contains('\\') || domain.contains("..")
        || !domain.chars().all(|c| c.is_ascii_alphanumeric() || c == '.' || c == '-') {
        return Err(err(StatusCode::BAD_REQUEST, "Invalid domain format"));
    }

    // Remove DKIM keys
    let key_dir = format!("{DKIM_KEYS_DIR}/{domain}");
    let _ = tokio::fs::remove_dir_all(&key_dir).await;

    // Same helper as the add path, so a removed domain stops being signed for.
    if let Err(e) = rebuild_dkim_tables().await {
        tracing::warn!("DKIM keys removed for {domain} but tables not rebuilt: {e}");
    }

    // Note: we don't delete the maildir — that's destructive.
    // The sync_config will remove the domain from Postfix/Dovecot maps.

    tracing::info!("Mail domain removed: {domain}");
    Ok(ok(&format!("Domain {domain} removed")))
}

// ── Full sync (rebuild all Postfix/Dovecot config) ──────────────────────

/// True if the local part of `addr` is safe to use as a PATH COMPONENT and a MAP KEY.
///
/// `domain_configure` already refuses a domain component containing `..`, `/` or
/// `\` before it joins it onto `VMAIL_DIR`. The local part is the other half of
/// exactly the same path — `format!("{VMAIL_DIR}/{}/{}", parts[1], parts[0])` in
/// step 7 below — and nothing checked it, so:
///
/// * `..@example.com` produced `/var/vmail/example.com/..`, i.e. `/var/vmail`,
///   one level above the domain directory, and the `chown -R` beside it then
///   walked the entire tree on every sync;
/// * `@example.com` produced the map key `@example.com`, which is byte-identical
///   to the domain's catch-all key. Account lines are written before catch-all
///   lines, `postmap` keeps the first, and its duplicate-entry warning goes to a
///   stderr this file discards — so the mailbox silently replaced the catch-all.
///
/// The panel refuses these too (`is_wellformed_address`), but the agent builds
/// the path, so the agent checks independently.
fn local_part_is_safe(addr: &str) -> bool {
    match addr.split_once('@') {
        Some((local, domain)) => {
            !local.is_empty() && !domain.is_empty() && local.chars().any(|c| c != '.')
        }
        None => false,
    }
}

async fn sync_config(
    Json(body): Json<SyncRequest>,
) -> Result<Json<serde_json::Value>, ApiErr> {
    // Validate account and alias fields for injection attacks
    for acc in &body.accounts {
        // Strict email character set: only safe chars for Postfix maps
        if !acc.email.chars().all(|c| c.is_ascii_alphanumeric() || matches!(c, '@' | '.' | '_' | '-' | '+')) {
            return Err(err(StatusCode::BAD_REQUEST, "Invalid characters in email address"));
        }
        if !acc.email.contains('@') || acc.email.matches('@').count() != 1 {
            return Err(err(StatusCode::BAD_REQUEST, "Invalid email format"));
        }
        if !local_part_is_safe(&acc.email) {
            return Err(err(StatusCode::BAD_REQUEST, "Invalid email local part"));
        }
        // Dovecot users file uses ':' as field separator — reject in password hash
        if acc.password_hash.contains(':') || acc.password_hash.contains('\n')
            || acc.password_hash.contains('\r') || acc.password_hash.contains('\0')
            || acc.password_hash.contains('\t') {
            return Err(err(StatusCode::BAD_REQUEST, "Invalid characters in password hash"));
        }
        // Validate forward_to if present
        if let Some(fwd) = &acc.forward_to {
            if !fwd.is_empty() && !fwd.chars().all(|c| c.is_ascii_alphanumeric() || matches!(c, '@' | '.' | '_' | '-' | '+')) {
                return Err(err(StatusCode::BAD_REQUEST, "Invalid characters in forward_to address"));
            }
        }
    }
    for alias in &body.aliases {
        if !alias.source.chars().all(|c| c.is_ascii_alphanumeric() || matches!(c, '@' | '.' | '_' | '-' | '+'))
            || !alias.destination.chars().all(|c| c.is_ascii_alphanumeric() || matches!(c, '@' | '.' | '_' | '-' | '+' | ',')) {
            return Err(err(StatusCode::BAD_REQUEST, "Invalid characters in alias data"));
        }
        // An alias source is a virtual_alias_maps KEY, so `@example.com` collides
        // with the catch-all key the same way an account address does.
        if !local_part_is_safe(&alias.source) {
            return Err(err(StatusCode::BAD_REQUEST, "Invalid alias source local part"));
        }
    }
    // Validate catch-all entries
    for domain in &body.domains {
        if let Some(catch_all) = &domain.catch_all {
            if !catch_all.is_empty() && !catch_all.chars().all(|c| c.is_ascii_alphanumeric() || matches!(c, '@' | '.' | '_' | '-' | '+' | '/')) {
                return Err(err(StatusCode::BAD_REQUEST, "Invalid characters in catch-all address"));
            }
        }
    }

    // Ensure directories exist
    tokio::fs::create_dir_all(VMAIL_DIR).await
        .map_err(|e| err(StatusCode::INTERNAL_SERVER_ERROR, &format!("Failed to create vmail dir: {e}")))?;
    tokio::fs::create_dir_all("/etc/postfix").await
        .map_err(|e| err(StatusCode::INTERNAL_SERVER_ERROR, &format!("Failed to create postfix dir: {e}")))?;
    tokio::fs::create_dir_all("/etc/dovecot").await
        .map_err(|e| err(StatusCode::INTERNAL_SERVER_ERROR, &format!("Failed to create dovecot dir: {e}")))?;

    // 1. Write virtual_domains (one domain per line)
    let domains_content: String = body.domains.iter()
        .filter(|d| d.enabled)
        .map(|d| d.domain.clone())
        .collect::<Vec<_>>()
        .join("\n");
    write_file_atomic(POSTFIX_VIRTUAL_DOMAINS, &domains_content).await
        .map_err(|e| err(StatusCode::INTERNAL_SERVER_ERROR, &format!("Failed to write virtual_domains: {e}")))?;

    // 2. Write virtual_mailbox_maps (email → maildir path)
    let mut mailbox_lines = Vec::new();
    for acc in &body.accounts {
        if !acc.enabled { continue; }
        let parts: Vec<&str> = acc.email.splitn(2, '@').collect();
        if parts.len() == 2 {
            mailbox_lines.push(format!("{}\t{}/{}/", acc.email, parts[1], parts[0]));
        }
    }
    // Add catch-all entries
    for domain in &body.domains {
        if let Some(catch_all) = &domain.catch_all {
            if !catch_all.is_empty() && domain.enabled {
                mailbox_lines.push(format!("@{}\t{}", domain.domain, catch_all));
            }
        }
    }
    write_file_atomic(POSTFIX_VIRTUAL_MAILBOX, &mailbox_lines.join("\n")).await
        .map_err(|e| err(StatusCode::INTERNAL_SERVER_ERROR, &format!("Failed to write virtual_mailbox_maps: {e}")))?;

    // 3. Write virtual_alias_maps
    let mut alias_lines: Vec<String> = body.aliases.iter()
        .map(|a| format!("{}\t{}", a.source, a.destination))
        .collect();
    // Add forwarding from accounts
    for acc in &body.accounts {
        if let Some(fwd) = &acc.forward_to {
            if !fwd.is_empty() && acc.enabled {
                alias_lines.push(format!("{}\t{}", acc.email, fwd));
            }
        }
    }
    write_file_atomic(POSTFIX_VIRTUAL_ALIAS, &alias_lines.join("\n")).await
        .map_err(|e| err(StatusCode::INTERNAL_SERVER_ERROR, &format!("Failed to write virtual_alias_maps: {e}")))?;

    // 4. Write Dovecot users file (email:{password_hash}::::/var/vmail/domain/user::quota=XM)
    let dovecot_lines: Vec<String> = body.accounts.iter()
        .filter(|a| a.enabled)
        .map(|a| {
            let parts: Vec<&str> = a.email.splitn(2, '@').collect();
            let maildir = if parts.len() == 2 {
                format!("{VMAIL_DIR}/{}/{}", parts[1], parts[0])
            } else {
                format!("{VMAIL_DIR}/{}", a.email)
            };
            format!("{}:{}::::{}::userdb_quota_rule=*:storage={}M", a.email, a.password_hash, maildir, a.quota_mb)
        })
        .collect();
    write_dovecot_users(&dovecot_lines.join("\n")).await
        .map_err(|e| err(StatusCode::INTERNAL_SERVER_ERROR, &format!("Failed to write dovecot users: {e}")))?;

    // 5. Bind each SASL login to the senders it may use, and arm the
    // restriction. Done here rather than at install time because this is the
    // only code path that runs again on a box already carrying mail: it fires
    // on every domain, account and alias mutation, so an existing install
    // repairs itself on the next mailbox change without a heal of its own.
    // Self-contained and fail-safe — see `ensure_sender_login_enforcement`.
    ensure_sender_login_enforcement(&build_sender_login_map(&body)).await;

    // 6. Run postmap to rebuild hash tables
    let _ = safe_command("postmap").arg(POSTFIX_VIRTUAL_MAILBOX).output().await;
    let _ = safe_command("postmap").arg(POSTFIX_VIRTUAL_ALIAS).output().await;

    // 7. Reload Postfix and Dovecot
    for service in &["postfix", "dovecot"] {
        if let Ok(out) = safe_command("systemctl").args(["reload", service]).output().await {
            if !out.status.success() {
                tracing::warn!("Failed to reload {service}: {}", String::from_utf8_lossy(&out.stderr));
            }
        } else {
            tracing::warn!("Failed to execute systemctl reload {service}");
        }
    }

    // 8. Create maildir directories for each account
    for acc in &body.accounts {
        if !acc.enabled { continue; }
        let parts: Vec<&str> = acc.email.splitn(2, '@').collect();
        if parts.len() == 2 {
            let maildir = format!("{VMAIL_DIR}/{}/{}", parts[1], parts[0]);
            if let Err(e) = tokio::fs::create_dir_all(&maildir).await {
                tracing::warn!("Failed to create maildir {maildir}: {e}");
            }
            let _ = safe_command("chown").args(["-R", "vmail:vmail", &maildir]).output().await;
        }
    }

    tracing::info!("Mail config synced: {} domains, {} accounts, {} aliases",
        body.domains.len(), body.accounts.len(), body.aliases.len());

    Ok(ok("Mail configuration synced"))
}

// ── Mail queue management ───────────────────────────────────────────────

async fn queue_list() -> Result<Json<serde_json::Value>, ApiErr> {
    let output = safe_command("mailq")
        .output()
        .await
        .map_err(|e| err(StatusCode::INTERNAL_SERVER_ERROR, &format!("mailq failed: {e}")))?;

    let stdout = String::from_utf8_lossy(&output.stdout);

    if stdout.contains("Mail queue is empty") || stdout.trim().is_empty() {
        return Ok(Json(serde_json::json!({ "queue": [], "count": 0 })));
    }

    // Parse mailq output
    let mut items = Vec::new();
    let mut current_id = String::new();
    let mut current_sender = String::new();
    let mut current_size = String::new();
    let mut current_time = String::new();
    let mut current_recipients = Vec::new();
    let mut current_status = String::new();

    for line in stdout.lines() {
        let trimmed = line.trim();
        if trimmed.starts_with('-') || trimmed.is_empty() || trimmed.starts_with("-- ") {
            if !current_id.is_empty() {
                items.push(serde_json::json!({
                    "id": current_id,
                    "sender": current_sender,
                    "size": current_size,
                    "arrival_time": current_time,
                    "recipients": current_recipients.join(", "),
                    "status": current_status,
                }));
                current_id.clear();
                current_recipients.clear();
                current_status.clear();
            }
            continue;
        }

        // Queue ID line: "A1B2C3D4E5*    1234 Mon Mar 15 10:00:00  sender@example.com"
        if trimmed.len() > 10 && trimmed.chars().next().map(|c| c.is_alphanumeric()).unwrap_or(false) {
            let parts: Vec<&str> = trimmed.splitn(2, char::is_whitespace).collect();
            if parts.len() >= 2 {
                let id_part = parts[0].trim_end_matches('*').trim_end_matches('!');
                current_id = id_part.to_string();
                current_status = if parts[0].contains('*') { "active".to_string() } else if parts[0].contains('!') { "hold".to_string() } else { "deferred".to_string() };

                // Parse size, time, sender from remaining
                let rest = parts[1].trim();
                let fields: Vec<&str> = rest.splitn(2, char::is_whitespace).collect();
                if fields.len() >= 2 {
                    current_size = fields[0].to_string();
                    // Find sender (last word)
                    let words: Vec<&str> = rest.split_whitespace().collect();
                    if let Some(sender) = words.last() {
                        current_sender = sender.to_string();
                    }
                    current_time = words[1..words.len().saturating_sub(1)].join(" ");
                }
            }
        } else if trimmed.contains('@') && !trimmed.contains(' ') {
            // Recipient line
            current_recipients.push(trimmed.to_string());
        }
    }

    // Don't forget the last entry
    if !current_id.is_empty() {
        items.push(serde_json::json!({
            "id": current_id,
            "sender": current_sender,
            "size": current_size,
            "arrival_time": current_time,
            "recipients": current_recipients.join(", "),
            "status": current_status,
        }));
    }

    Ok(Json(serde_json::json!({ "queue": items, "count": items.len() })))
}

async fn queue_flush() -> Result<Json<serde_json::Value>, ApiErr> {
    let output = safe_command("postqueue")
        .arg("-f")
        .output()
        .await
        .map_err(|e| err(StatusCode::INTERNAL_SERVER_ERROR, &format!("postqueue -f failed: {e}")))?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(err(StatusCode::INTERNAL_SERVER_ERROR, &format!("Flush failed: {stderr}")));
    }

    tracing::info!("Mail queue flushed");
    Ok(ok("Queue flushed"))
}

async fn queue_delete(
    Json(body): Json<QueueDeleteRequest>,
) -> Result<Json<serde_json::Value>, ApiErr> {
    let id = body.id.trim();
    if id.is_empty() || !id.chars().all(|c| c.is_ascii_hexdigit()) || id.len() > 20 {
        return Err(err(StatusCode::BAD_REQUEST, "Invalid queue ID format"));
    }

    let output = safe_command("postsuper")
        .args(["-d", id])
        .output()
        .await
        .map_err(|e| err(StatusCode::INTERNAL_SERVER_ERROR, &format!("postsuper -d failed: {e}")))?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(err(StatusCode::INTERNAL_SERVER_ERROR, &format!("Delete failed: {stderr}")));
    }

    tracing::info!("Queued message {} deleted", body.id);
    Ok(ok("Message deleted from queue"))
}

// ── Rspamd spam filter ───────────────────────────────────────────────────

/// Append `milter` to Postfix's `smtpd_milters` / `non_smtpd_milters` lists,
/// preserving whatever endpoint is already configured.
///
/// Reads the value that is actually on the line rather than matching a literal,
/// so a change to [`OPENDKIM_MILTER`] can never silently orphan this edit the
/// way it did between s268 and s270.
///
/// Returns `None` when neither key is present — that means Postfix has no mail
/// configuration to extend, which the caller must treat as an error rather than
/// writing a file back unchanged. Idempotent: a milter already in the list is
/// left exactly once.
fn add_milter(main_cf: &str, milter: &str) -> Option<String> {
    let mut found = false;
    let rewritten: Vec<String> = main_cf
        .lines()
        .map(|line| {
            for key in ["smtpd_milters", "non_smtpd_milters"] {
                let Some(rest) = line.strip_prefix(key) else { continue };
                let Some(value) = rest.trim_start().strip_prefix('=') else { continue };
                found = true;
                let value = value.trim();
                if value.split(',').any(|m| m.trim() == milter) {
                    return line.to_string();
                }
                return if value.is_empty() {
                    format!("{key} = {milter}")
                } else {
                    format!("{key} = {value}, {milter}")
                };
            }
            line.to_string()
        })
        .collect();
    if !found {
        return None;
    }
    let mut out = rewritten.join("\n");
    if main_cf.ends_with('\n') {
        out.push('\n');
    }
    Some(out)
}

/// POST /mail/rspamd/install — Install and configure Rspamd.
async fn rspamd_install() -> Result<Json<serde_json::Value>, ApiErr> {
    tracing::info!("Installing Rspamd spam filter...");

    // rspamd is not in EPEL. On a stock Rocky 9 with EPEL *and* CRB enabled,
    // `dnf install rspamd` answers "Unable to find a match: rspamd" — so the
    // spam filter could never be installed on the RHEL family at all, and the
    // panel offered the button anyway (measured s270). Upstream publishes an
    // rpm repo; add it only when the package is genuinely unreachable, so a
    // box that already carries rspamd from anywhere else is left alone.
    //
    // Gated on the RPM family EXPLICITLY, not on the availability probe alone.
    // Debian and Ubuntu package rspamd themselves and that path has worked
    // since s262 — if a probe failure were allowed to reach `add_repo` there it
    // would break the family that works in order to fix the one that does not,
    // which is exactly lesson #95c.
    if crate::services::pkg::manager().await == crate::services::pkg::PkgMgr::Rpm
        && !crate::services::pkg::available("rspamd").await
    {
        crate::services::pkg::add_repo(crate::services::pkg::Repo::Rspamd)
            .await
            .map_err(|e| err(StatusCode::INTERNAL_SERVER_ERROR, &format!("Could not add the Rspamd repository: {e}")))?;
    }

    // Install rspamd through the package abstraction. Another call site s266's
    // conversion missed: it shelled to `apt-get` directly AND passed
    // `redis-server`, whose RPM package and unit are both `redis` — the exact
    // trap `pkg::service_name`'s own doc comment describes.
    crate::services::pkg::install(&["rspamd", "redis-server"])
        .await
        .map_err(|e| err(StatusCode::INTERNAL_SERVER_ERROR, &format!("Rspamd install failed: {e}")))?;

    // Wire Rspamd's milter alongside whatever milter is already configured.
    //
    // This was a literal string replace against
    // `smtpd_milters = unix:opendkim/opendkim.sock`. s268 moved OpenDKIM's
    // milter to a loopback port because the Unix socket lived inside Postfix's
    // chroot — a Debian arrangement SELinux forbids on RHEL — and this sibling
    // kept matching the old literal. `str::replace` with an absent needle
    // returns the string UNCHANGED, so main.cf was rewritten byte-identical,
    // the handler reported success, and Postfix never consulted Rspamd on ANY
    // family. Deriving the edit from the line that is actually present cannot
    // rot the same way; see [`add_milter`].
    let main_cf = tokio::fs::read_to_string("/etc/postfix/main.cf").await.unwrap_or_default();
    match add_milter(&main_cf, RSPAMD_MILTER) {
        Some(new_cf) => {
            if new_cf != main_cf {
                write_file_atomic("/etc/postfix/main.cf", &new_cf).await
                    .map_err(|e| err(StatusCode::INTERNAL_SERVER_ERROR, &format!("Config write failed: {e}")))?;
            }
        }
        // No milter list at all means the mail server was never installed.
        // Silently writing nothing is what produced the defect above.
        None => return Err(err(
            StatusCode::PRECONDITION_FAILED,
            "Postfix has no smtpd_milters setting — install the mail server before the spam filter",
        )),
    }

    // Enable and start. The Redis UNIT is `redis-server` on Debian and `redis`
    // on the RHEL family, which is what `pkg::service_name` translates.
    let redis_unit = crate::services::pkg::service_name("redis-server").await;
    if let Ok(out) = safe_command("systemctl").args(["enable", "rspamd", &redis_unit]).output().await {
        if !out.status.success() {
            tracing::warn!("Failed to enable rspamd/redis: {}", String::from_utf8_lossy(&out.stderr));
        }
    }
    for service in &[redis_unit.as_str(), "rspamd"] {
        if let Ok(out) = safe_command("systemctl").args(["restart", service]).output().await {
            if !out.status.success() {
                tracing::warn!("Failed to restart {service}: {}", String::from_utf8_lossy(&out.stderr));
            }
        } else {
            tracing::warn!("Failed to execute systemctl restart {service}");
        }
    }
    if let Ok(out) = safe_command("systemctl").args(["reload", "postfix"]).output().await {
        if !out.status.success() {
            tracing::warn!("Failed to reload postfix: {}", String::from_utf8_lossy(&out.stderr));
        }
    }

    tracing::info!("Rspamd installed and configured");
    Ok(ok("Rspamd spam filter installed"))
}

/// GET /mail/rspamd/status — Check Rspamd status.
async fn rspamd_status() -> Json<serde_json::Value> {
    let installed = is_installed("rspamd").await;
    let running = is_service_active("rspamd").await;
    // Same unit-name translation as the installer, or this reports Redis down
    // on every RHEL box (the unit there is `redis`, not `redis-server`).
    let redis = is_service_active(&crate::services::pkg::service_name("redis-server").await).await;
    Json(serde_json::json!({ "installed": installed, "running": running, "redis": redis }))
}

/// POST /mail/rspamd/toggle — Enable/disable Rspamd.
async fn rspamd_toggle(Json(body): Json<serde_json::Value>) -> Result<Json<serde_json::Value>, ApiErr> {
    let enable = body.get("enable").and_then(|v| v.as_bool()).unwrap_or(true);
    if enable {
        for (action, service) in &[("start", "rspamd"), ("enable", "rspamd")] {
            if let Ok(out) = safe_command("systemctl").args([*action, service]).output().await {
                if !out.status.success() {
                    tracing::warn!("Failed to {action} {service}: {}", String::from_utf8_lossy(&out.stderr));
                }
            } else {
                tracing::warn!("Failed to execute systemctl {action} {service}");
            }
        }
    } else {
        for (action, service) in &[("stop", "rspamd"), ("disable", "rspamd")] {
            if let Ok(out) = safe_command("systemctl").args([*action, service]).output().await {
                if !out.status.success() {
                    tracing::warn!("Failed to {action} {service}: {}", String::from_utf8_lossy(&out.stderr));
                }
            } else {
                tracing::warn!("Failed to execute systemctl {action} {service}");
            }
        }
    }
    Ok(ok(if enable { "Rspamd enabled" } else { "Rspamd disabled" }))
}

// ── Webmail (Roundcube) ─────────────────────────────────────────────────

const WEBMAIL_NGINX_CONF: &str = "/etc/nginx/conf.d/dockpanel-panel.locations/webmail.conf";
const WEBMAIL_NGINX_DIR: &str = "/etc/nginx/conf.d/dockpanel-panel.locations";

/// Read the panel vhost's `server_name` directive. Returns `None` for
/// `_` (IP-based installs) or if no panel vhost exists yet.
fn panel_server_name() -> Option<String> {
    for conf in &[
        "/etc/nginx/sites-enabled/dockpanel-panel.conf",
        "/etc/nginx/conf.d/dockpanel-panel.conf",
    ] {
        if let Ok(content) = std::fs::read_to_string(conf) {
            for line in content.lines() {
                let trimmed = line.trim();
                if let Some(rest) = trimmed.strip_prefix("server_name ") {
                    let name = rest.trim_end_matches(';').trim();
                    if !name.is_empty() && name != "_" {
                        if let Some(first) = name.split_whitespace().next() {
                            return Some(first.to_string());
                        }
                    }
                }
            }
        }
    }
    None
}

/// The `/webmail/` reverse-proxy fragment, for a given Roundcube host port.
///
/// **The single source of this file's contents.** `scripts/update.sh` used to
/// carry a hand-copied mirror of it, which is precisely how it rotted: the
/// mirror was frozen at the v2.10.1 shape and never learned the header set
/// v2.36.0 added, so its "heal" wrote a fragment that renders the inbox empty.
/// Callers that need the file on disk go through [`write_webmail_nginx`] or
/// [`heal_webmail_nginx`]; nothing else may spell this block out.
fn webmail_nginx_block(port: u16) -> String {
    // v2.10.1: Roundcube emits root-anchored URLs (form action="/?_task=...",
    // JS comm_path="/?_task=..."). Without sub_filter, browser navigation
    // and AJAX from Roundcube hit the panel's `location /` (the React SPA),
    // not /webmail/ — symptom: Open lands on dashboard, login form posts
    // to /?_task=login. proxy_redirect handles 30x Location: headers from
    // Roundcube; sub_filter rewrites embedded URLs in HTML/JSON bodies.
    format!(
        "# DockPanel webmail (Roundcube) reverse-proxy — managed by agent, do not edit\n\
         # v2.10.1: sub_filter rewrites Roundcube's root-anchored URLs (form action,\n\
         # comm_path) under /webmail/ so navigation doesn't land on the panel root.\n\
         location /webmail/ {{\n\
         \x20   proxy_pass http://127.0.0.1:{port}/;\n\
         \x20   proxy_http_version 1.1;\n\
         \x20   proxy_set_header Host $host;\n\
         \x20   proxy_set_header X-Real-IP $remote_addr;\n\
         \x20   proxy_set_header X-Forwarded-For $proxy_add_x_forwarded_for;\n\
         \x20   proxy_set_header X-Forwarded-Proto $scheme;\n\
         \x20   proxy_set_header X-Forwarded-Host $host;\n\
         \x20   proxy_set_header Accept-Encoding \"\";\n\
         \x20   proxy_redirect / /webmail/;\n\
         \x20   sub_filter '\"/?_task=' '\"/webmail/?_task=';\n\
         \x20   sub_filter_once off;\n\
         \x20   sub_filter_types text/html application/json application/javascript text/javascript;\n\
         \x20   proxy_read_timeout 300s;\n\
         \x20   client_max_body_size 25M;\n\
         \x20   # Roundcube's Elastic skin frames itself same-origin. The panel\n\
         \x20   # vhost sends X-Frame-Options DENY and frame-ancestors 'none',\n\
         \x20   # which this location inherits — the iframe is refused, the list\n\
         \x20   # JS throws a SecurityError reaching into it, and the inbox\n\
         \x20   # renders empty however much mail is really in it. Re-declare the\n\
         \x20   # whole header set here (nginx add_header in a location replaces\n\
         \x20   # the inherited set rather than adding to it) with framing\n\
         \x20   # narrowed to same-origin, not opened up.\n\
         \x20   add_header X-Content-Type-Options \"nosniff\" always;\n\
         \x20   add_header X-Frame-Options \"SAMEORIGIN\" always;\n\
         \x20   add_header Referrer-Policy \"strict-origin-when-cross-origin\" always;\n\
         \x20   add_header Permissions-Policy \"camera=(), microphone=(), geolocation=()\" always;\n\
         \x20   add_header Strict-Transport-Security \"max-age=31536000; includeSubDomains\" always;\n\
         \x20   add_header Content-Security-Policy \"frame-ancestors 'self'\" always;\n\
         \x20   add_header X-XSS-Protection \"1; mode=block\" always;\n\
         }}\n"
    )
}

/// Write the `/webmail/` reverse-proxy nginx fragment into the panel-locations
/// drop-in dir, validate with `nginx -t`, reload on success. Unlinks the
/// fragment if validation fails — never leaves nginx in a broken state.
async fn write_webmail_nginx(port: u16) -> Result<(), ApiErr> {
    if let Err(e) = std::fs::create_dir_all(WEBMAIL_NGINX_DIR) {
        return Err(err(StatusCode::INTERNAL_SERVER_ERROR, &format!("Failed to create {WEBMAIL_NGINX_DIR}: {e}")));
    }
    let block = webmail_nginx_block(port);
    if let Err(e) = std::fs::write(WEBMAIL_NGINX_CONF, &block) {
        return Err(err(StatusCode::INTERNAL_SERVER_ERROR, &format!("Failed to write {WEBMAIL_NGINX_CONF}: {e}")));
    }
    let test = safe_command("nginx").args(["-t"]).output().await
        .map_err(|e| err(StatusCode::INTERNAL_SERVER_ERROR, &format!("nginx -t failed: {e}")))?;
    if !test.status.success() {
        let _ = std::fs::remove_file(WEBMAIL_NGINX_CONF);
        let stderr = String::from_utf8_lossy(&test.stderr);
        return Err(err(StatusCode::INTERNAL_SERVER_ERROR, &format!("nginx config invalid: {}", &stderr[..400.min(stderr.len())])));
    }
    if let Err(e) = safe_command("nginx").args(["-s", "reload"]).output().await {
        tracing::warn!("nginx reload failed after webmail install: {e}");
    }
    Ok(())
}

/// Bring an existing `/webmail/` fragment up to the current template.
///
/// The fragment is written only on the Install click, so a fix to its contents
/// reaches nobody who already installed webmail. That is how s262's fix — the
/// re-declared header set, without which the location inherits the panel's
/// `frame-ancestors 'none'`, the Roundcube content frame is refused, and
/// `clear_message_list` throws a SecurityError that aborts `list_mailbox`
/// before the list is ever requested — never arrived on a single existing box.
/// `update.sh` had a heal for the *previous* shape and it made this worse: it
/// fired only when `sub_filter` was absent and wrote the v2.10.1 shape, which
/// is exactly the shape that renders the inbox empty.
///
/// Runs at agent startup, so an upgrade is all it takes. Rewrites only when the
/// on-disk bytes differ from the template, and only when the fragment already
/// exists — this never installs webmail for someone who does not have it.
pub async fn heal_webmail_nginx() {
    if !Path::new(WEBMAIL_NGINX_CONF).exists() {
        return;
    }
    let Ok(current) = std::fs::read_to_string(WEBMAIL_NGINX_CONF) else { return };

    // Keep the port the box is actually serving on. Falling back to the default
    // would silently repoint the proxy at nothing on a box that chose another.
    let port = current
        .lines()
        .find_map(|l| l.trim().strip_prefix("proxy_pass http://127.0.0.1:"))
        .and_then(|rest| rest.split('/').next())
        .and_then(|p| p.trim().parse::<u16>().ok())
        .unwrap_or(8888);

    let desired = webmail_nginx_block(port);
    if current == desired {
        return;
    }
    if std::fs::write(WEBMAIL_NGINX_CONF, &desired).is_err() {
        return;
    }
    match safe_command("nginx").args(["-t"]).output().await {
        Ok(out) if out.status.success() => {
            let _ = safe_command("nginx").args(["-s", "reload"]).output().await;
            tracing::info!("Healed the /webmail/ nginx fragment to the current template (port {port})");
        }
        _ => {
            // Never leave nginx unable to start: put back exactly what was there.
            let _ = std::fs::write(WEBMAIL_NGINX_CONF, &current);
            tracing::warn!("Webmail fragment heal failed nginx -t; restored the previous fragment");
        }
    }
}

async fn remove_webmail_nginx() {
    if Path::new(WEBMAIL_NGINX_CONF).exists() {
        let _ = std::fs::remove_file(WEBMAIL_NGINX_CONF);
        if let Ok(out) = safe_command("nginx").args(["-t"]).output().await {
            if out.status.success() {
                let _ = safe_command("nginx").args(["-s", "reload"]).output().await;
            }
        }
    }
}

/// POST /mail/webmail/install — Deploy Roundcube webmail via Docker.
///
/// Idempotent: tears down any prior `dockpanel-roundcube` container before
/// recreating, so env-var additions across releases apply on next Install
/// click. Also writes the panel-vhost reverse-proxy fragment so the Open
/// button at `${panel}/webmail/` works on both HTTP-on-IP and HTTPS-domain
/// installs.
async fn webmail_install(Json(body): Json<serde_json::Value>) -> Result<Json<serde_json::Value>, ApiErr> {
    let domain = body.get("domain").and_then(|v| v.as_str()).unwrap_or("localhost");
    let port = body.get("port").and_then(|v| v.as_u64()).unwrap_or(8888) as u16;

    tracing::info!("Installing Roundcube webmail on port {port}...");

    let _ = safe_command("docker").args(["rm", "-f", "dockpanel-roundcube"]).output().await;

    let panel_host = panel_server_name();
    let mut args: Vec<String> = vec![
        "run".into(), "-d".into(),
        "--name".into(), "dockpanel-roundcube".into(),
        "--restart".into(), "unless-stopped".into(),
        "-p".into(), format!("127.0.0.1:{port}:80"),
        "-e".into(), format!("ROUNDCUBEMAIL_DEFAULT_HOST=ssl://{domain}"),
        "-e".into(), "ROUNDCUBEMAIL_DEFAULT_PORT=993".into(),
        "-e".into(), format!("ROUNDCUBEMAIL_SMTP_SERVER=tls://{domain}"),
        "-e".into(), "ROUNDCUBEMAIL_SMTP_PORT=587".into(),
        "-e".into(), "ROUNDCUBEMAIL_UPLOAD_MAX_FILESIZE=25M".into(),
        "-e".into(), "ROUNDCUBEMAIL_PROXY_WHITELIST=127.0.0.1".into(),
    ];
    if let Some(host) = panel_host.as_deref() {
        args.push("-e".into());
        args.push(format!("ROUNDCUBEMAIL_TRUSTED_HOSTS={host}"));
        args.push("-e".into());
        args.push("ROUNDCUBEMAIL_FORWARDED_PROTO=https".into());
    }
    args.extend([
        "-l".into(), "dockpanel.managed=true".into(),
        "-l".into(), "dockpanel.app.template=roundcube".into(),
        "-l".into(), "dockpanel.app.name=roundcube".into(),
        // Pinned off `:latest`, which let a MAJOR Roundcube upgrade land on a
        // user's mail stack with no warning and no way back — the panel warns
        // users about exactly this in docker_apps.rs while floating itself.
        //
        // `1.7.x` rather than an exact `1.7.2`: this is a web-facing PHP app and
        // nothing in DockPanel would ever bump a frozen patch tag, so an exact
        // pin would quietly stop receiving CVE fixes. The `.x` line still gets
        // upstream's security rebuilds while ruling out the surprise major.
        //
        // `-apache` is load-bearing, not decoration: the container is published
        // on port 80 above, and the -fpm variants do not serve HTTP at all.
        // Verified by digest at the time of pinning — `latest`, `latest-apache`
        // and `1.7.2-apache` were all sha256:aed1b9b5dc34, so this changed the
        // guarantee without changing the image.
        ROUNDCUBE_IMAGE.into(),
    ]);

    let output = safe_command("docker")
        .args(args.iter().map(|s| s.as_str()))
        .output().await
        .map_err(|e| err(StatusCode::INTERNAL_SERVER_ERROR, &format!("Docker failed: {e}")))?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(err(StatusCode::INTERNAL_SERVER_ERROR, &format!("Roundcube deploy failed: {}", &stderr[..200.min(stderr.len())])));
    }

    write_webmail_nginx(port).await?;

    tracing::info!("Roundcube webmail deployed on port {port}, panel-vhost /webmail/ proxy active");
    Ok(Json(serde_json::json!({ "ok": true, "port": port })))
}

/// GET /mail/webmail/status — Check if Roundcube is running.
async fn webmail_status() -> Json<serde_json::Value> {
    let output = safe_command("docker")
        .args(["inspect", "--format", "{{.State.Running}}", "dockpanel-roundcube"])
        .output().await;
    let running = output.map(|o| String::from_utf8_lossy(&o.stdout).trim() == "true").unwrap_or(false);

    // Get port
    let port_output = safe_command("docker")
        .args(["inspect", "--format", "{{range .NetworkSettings.Ports}}{{range .}}{{.HostPort}}{{end}}{{end}}", "dockpanel-roundcube"])
        .output().await;
    let port = port_output.ok().and_then(|o| String::from_utf8_lossy(&o.stdout).trim().parse::<u16>().ok()).unwrap_or(0);

    Json(serde_json::json!({ "installed": running || port > 0, "running": running, "port": port }))
}

/// POST /mail/webmail/remove — Remove Roundcube container + panel-vhost fragment.
async fn webmail_remove() -> Result<Json<serde_json::Value>, ApiErr> {
    let _ = safe_command("docker").args(["rm", "-f", "dockpanel-roundcube"]).output().await;
    remove_webmail_nginx().await;
    Ok(ok("Roundcube removed"))
}

// ── SMTP Relay ──────────────────────────────────────────────────────────

#[derive(Deserialize)]
struct RelayConfig {
    host: String,
    port: u16,
    username: String,
    password: String,
}

/// POST /mail/relay/configure — Set up SMTP relay (smarthost).
async fn relay_configure(Json(body): Json<RelayConfig>) -> Result<Json<serde_json::Value>, ApiErr> {
    if body.host.is_empty() { return Err(err(StatusCode::BAD_REQUEST, "Relay host required")); }

    if body.host.contains('\n') || body.host.contains('\r') || body.host.contains('\0')
        || body.username.contains('\n') || body.username.contains('\0')
        || body.password.contains('\n') || body.password.contains('\0') {
        return Err(err(StatusCode::BAD_REQUEST, "Invalid characters in relay config"));
    }

    // Write SASL password file
    let sasl_content = format!("[{}]:{} {}:{}\n", body.host, body.port, body.username, body.password);
    write_file_atomic("/etc/postfix/sasl_passwd", &sasl_content).await
        .map_err(|e| err(StatusCode::INTERNAL_SERVER_ERROR, &format!("Failed to write sasl_passwd: {e}")))?;

    // Set permissions
    let _ = safe_command("chmod").args(["600", "/etc/postfix/sasl_passwd"]).output().await;
    let _ = safe_command("postmap").arg("/etc/postfix/sasl_passwd").output().await;

    // Update Postfix main.cf
    let main_cf = tokio::fs::read_to_string("/etc/postfix/main.cf").await.unwrap_or_default();

    // Remove existing relay config lines
    let cleaned: String = main_cf.lines()
        .filter(|l| !l.starts_with("relayhost") && !l.starts_with("smtp_sasl_") && !l.starts_with("smtp_tls_") && !l.contains("# DockPanel relay"))
        .collect::<Vec<_>>().join("\n");

    let relay_config = format!(
        "\n# DockPanel relay configuration\nrelayhost = [{}]:{}\nsmtp_sasl_auth_enable = yes\nsmtp_sasl_password_maps = hash:/etc/postfix/sasl_passwd\nsmtp_sasl_security_options = noanonymous\nsmtp_tls_security_level = encrypt\nsmtp_tls_CAfile = /etc/ssl/certs/ca-certificates.crt\n",
        body.host, body.port
    );

    write_file_atomic("/etc/postfix/main.cf", &format!("{cleaned}{relay_config}")).await
        .map_err(|e| err(StatusCode::INTERNAL_SERVER_ERROR, &format!("Config write failed: {e}")))?;

    let _ = safe_command("systemctl").args(["reload", "postfix"]).output().await;

    tracing::info!("SMTP relay configured: [{}]:{}", body.host, body.port);
    Ok(ok("SMTP relay configured"))
}

/// GET /mail/relay/status — Check current relay configuration.
async fn relay_status() -> Json<serde_json::Value> {
    let main_cf = tokio::fs::read_to_string("/etc/postfix/main.cf").await.unwrap_or_default();
    let relayhost = main_cf.lines()
        .find(|l| l.starts_with("relayhost"))
        .map(|l| l.split('=').nth(1).unwrap_or("").trim().to_string());

    Json(serde_json::json!({
        "configured": relayhost.is_some() && !relayhost.as_ref().unwrap().is_empty(),
        "relayhost": relayhost.unwrap_or_default(),
    }))
}

/// POST /mail/relay/remove — Remove SMTP relay configuration.
async fn relay_remove() -> Result<Json<serde_json::Value>, ApiErr> {
    let main_cf = tokio::fs::read_to_string("/etc/postfix/main.cf").await.unwrap_or_default();
    let cleaned: String = main_cf.lines()
        .filter(|l| !l.starts_with("relayhost") && !l.starts_with("smtp_sasl_") && !l.starts_with("smtp_tls_") && !l.contains("# DockPanel relay"))
        .collect::<Vec<_>>().join("\n");

    write_file_atomic("/etc/postfix/main.cf", &cleaned).await
        .map_err(|e| err(StatusCode::INTERNAL_SERVER_ERROR, &format!("Config write failed: {e}")))?;

    let _ = tokio::fs::remove_file("/etc/postfix/sasl_passwd").await;
    let _ = tokio::fs::remove_file("/etc/postfix/sasl_passwd.db").await;
    let _ = safe_command("systemctl").args(["reload", "postfix"]).output().await;

    Ok(ok("SMTP relay removed"))
}

// ── Mail Logs ───────────────────────────────────────────────────────────

/// Where this box's MTA log actually lives.
///
/// Probed rather than branched on the package manager, because the name is a
/// property of the running syslog configuration, not of the distro family — a
/// Debian box with an unusual rsyslog config, or an RHEL box with none at all,
/// both answer correctly here. Falls back to the Debian name so the failure
/// mode on a box with neither file is unchanged.
pub async fn mail_log_path() -> String {
    for candidate in ["/var/log/mail.log", "/var/log/maillog"] {
        if Path::new(candidate).exists() {
            return candidate.to_string();
        }
    }
    "/var/log/mail.log".to_string()
}

/// GET /mail/logs — Parse the mail log for recent activity and stats.
async fn mail_logs() -> Result<Json<serde_json::Value>, ApiErr> {
    // Debian's rsyslog writes `/var/log/mail.log`; the RHEL family writes
    // `/var/log/maillog`. Hardcoding the Debian name meant this endpoint
    // returned `sent:0, received:0` on every RHEL box — measured on Rocky 9.8
    // minutes after that box had sent and received a DKIM-signed message
    // (s268). It is the page an operator opens when mail is missing, so it
    // failed exactly when it was needed.
    let path = mail_log_path().await;
    // Read last portion of the log (tail -5000 to avoid reading huge files)
    let output = safe_command("tail")
        .args(["-n", "5000", &path])
        .output().await;
    let content = output.ok()
        .map(|o| String::from_utf8_lossy(&o.stdout).to_string())
        .unwrap_or_default();

    let mut sent = 0u32;
    let mut received = 0u32;
    let mut bounced = 0u32;
    let mut rejected = 0u32;
    let mut recent: Vec<serde_json::Value> = Vec::new();

    for line in content.lines().rev() {
        if line.contains("status=sent") { sent += 1; }
        if line.contains("status=bounced") { bounced += 1; }
        if line.contains("NOQUEUE: reject") || line.contains("status=rejected") { rejected += 1; }
        if line.contains("delivered to maildir") || line.contains("lmtp(") { received += 1; }

        // Collect recent entries (last 50 interesting lines)
        if recent.len() < 50 && (line.contains("status=") || line.contains("NOQUEUE") || line.contains("delivered")) {
            let time = if line.len() >= 15 { &line[..15] } else { "" };
            let is_error = line.contains("bounced") || line.contains("reject") || line.contains("error");
            recent.push(serde_json::json!({
                "time": time,
                "message": if line.len() > 16 { &line[16..line.len().min(200)] } else { line },
                "level": if is_error { "error" } else { "info" },
            }));
        }
    }

    Ok(Json(serde_json::json!({
        "stats": { "sent": sent, "received": received, "bounced": bounced, "rejected": rejected },
        "recent": recent,
    })))
}

// ── Storage Usage ───────────────────────────────────────────────────────

/// GET /mail/storage — Get storage usage for all mailboxes.
async fn storage_usage() -> Result<Json<serde_json::Value>, ApiErr> {
    let mut usage = Vec::new();

    // Scan /var/vmail for domain/user directories
    let mut domains = match tokio::fs::read_dir("/var/vmail").await {
        Ok(d) => d,
        Err(_) => return Ok(Json(serde_json::json!({ "accounts": [] }))),
    };

    while let Ok(Some(domain_entry)) = domains.next_entry().await {
        if !domain_entry.file_type().await.map(|t| t.is_dir()).unwrap_or(false) { continue; }
        let domain = domain_entry.file_name().to_string_lossy().to_string();

        let mut users = match tokio::fs::read_dir(domain_entry.path()).await {
            Ok(u) => u,
            Err(_) => continue,
        };

        while let Ok(Some(user_entry)) = users.next_entry().await {
            if !user_entry.file_type().await.map(|t| t.is_dir()).unwrap_or(false) { continue; }
            let user = user_entry.file_name().to_string_lossy().to_string();

            // Get directory size using du
            let output = safe_command("du")
                .args(["-sb", &user_entry.path().to_string_lossy()])
                .output().await;

            let bytes: u64 = output.ok()
                .map(|o| String::from_utf8_lossy(&o.stdout).split_whitespace().next().unwrap_or("0").parse().unwrap_or(0))
                .unwrap_or(0);

            usage.push(serde_json::json!({
                "email": format!("{user}@{domain}"),
                "bytes": bytes,
                "mb": (bytes as f64 / 1024.0 / 1024.0 * 10.0).round() / 10.0,
            }));
        }
    }

    Ok(Json(serde_json::json!({ "accounts": usage })))
}

// ── Rate Limiting ───────────────────────────────────────────────────────

/// POST /mail/rate-limit/set — Set global outbound rate limit.
async fn rate_limit_set(Json(body): Json<RateLimitRequest>) -> Result<Json<serde_json::Value>, ApiErr> {
    if body.rate.is_empty() { return Err(err(StatusCode::BAD_REQUEST, "Rate required")); }

    // Parse rate: "100/hour" → smtp_destination_rate_delay = 36s (3600/100)
    // "500/day" → smtp_destination_rate_delay = 172s (86400/500)
    let parts: Vec<&str> = body.rate.split('/').collect();
    if parts.len() != 2 { return Err(err(StatusCode::BAD_REQUEST, "Rate format: N/hour or N/day")); }
    let count: u32 = parts[0].parse().map_err(|_| err(StatusCode::BAD_REQUEST, "Invalid count"))?;
    if count == 0 { return Err(err(StatusCode::BAD_REQUEST, "Count must be > 0")); }
    let period_secs: u32 = match parts[1] {
        "hour" => 3600,
        "day" => 86400,
        "minute" => 60,
        _ => return Err(err(StatusCode::BAD_REQUEST, "Period must be minute, hour, or day")),
    };
    let delay = period_secs / count;

    // Update Postfix config
    let main_cf = tokio::fs::read_to_string("/etc/postfix/main.cf").await.unwrap_or_default();
    let cleaned: String = main_cf.lines()
        .filter(|l| !l.starts_with("smtp_destination_rate_delay") && !l.starts_with("smtp_extra_recipient_limit") && !l.contains("# DockPanel rate limit"))
        .collect::<Vec<_>>().join("\n");

    let rate_config = format!("\n# DockPanel rate limit: {}\nsmtp_destination_rate_delay = {}s\nsmtp_extra_recipient_limit = {}\n", body.rate, delay, count.min(50));

    write_file_atomic("/etc/postfix/main.cf", &format!("{cleaned}{rate_config}")).await
        .map_err(|e| err(StatusCode::INTERNAL_SERVER_ERROR, &format!("Config write failed: {e}")))?;

    let _ = safe_command("systemctl").args(["reload", "postfix"]).output().await;

    tracing::info!("Mail rate limit set: {} (delay: {}s)", body.rate, delay);
    Ok(ok(&format!("Rate limit set: {}", body.rate)))
}

/// GET /mail/rate-limit/status — Get current rate limit.
async fn rate_limit_status() -> Json<serde_json::Value> {
    let main_cf = tokio::fs::read_to_string("/etc/postfix/main.cf").await.unwrap_or_default();
    let rate_line = main_cf.lines().find(|l| l.contains("# DockPanel rate limit:"));
    let configured = rate_line.is_some();
    let rate = rate_line.and_then(|l| l.split(':').nth(1)).unwrap_or("").trim().to_string();
    Json(serde_json::json!({ "configured": configured, "rate": rate }))
}

/// POST /mail/rate-limit/remove — Remove rate limit.
async fn rate_limit_remove() -> Result<Json<serde_json::Value>, ApiErr> {
    let main_cf = tokio::fs::read_to_string("/etc/postfix/main.cf").await.unwrap_or_default();
    let cleaned: String = main_cf.lines()
        .filter(|l| !l.starts_with("smtp_destination_rate_delay") && !l.starts_with("smtp_extra_recipient_limit") && !l.contains("# DockPanel rate limit"))
        .collect::<Vec<_>>().join("\n");
    write_file_atomic("/etc/postfix/main.cf", &cleaned).await
        .map_err(|e| err(StatusCode::INTERNAL_SERVER_ERROR, &format!("Config write failed: {e}")))?;
    let _ = safe_command("systemctl").args(["reload", "postfix"]).output().await;
    Ok(ok("Rate limit removed"))
}

// ── Mailbox Backup/Restore ──────────────────────────────────────────────

/// POST /mail/backup — Create a backup of a mailbox (tar.gz of maildir).
async fn mailbox_backup(Json(body): Json<MailboxBackupRequest>) -> Result<Json<serde_json::Value>, ApiErr> {
    let email = body.email.trim();
    if email.is_empty() || !email.contains('@') { return Err(err(StatusCode::BAD_REQUEST, "Invalid email")); }

    let parts: Vec<&str> = email.splitn(2, '@').collect();
    let (user, domain) = (parts[0], parts[1]);

    if user.contains('/') || user.contains('\\') || user.contains("..")
        || domain.contains('/') || domain.contains('\\') || domain.contains("..") {
        return Err(err(StatusCode::BAD_REQUEST, "Invalid email format"));
    }

    let maildir = format!("/var/vmail/{domain}/{user}");

    if !Path::new(&maildir).exists() {
        return Err(err(StatusCode::NOT_FOUND, "Mailbox directory not found"));
    }

    let backup_dir = "/var/lib/dockpanel/mail-backups";
    tokio::fs::create_dir_all(backup_dir).await.ok();
    // The tarballs contain plaintext mail — keep the directory owner-only (0700).
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = tokio::fs::set_permissions(backup_dir, std::fs::Permissions::from_mode(0o700)).await;
    }

    let timestamp = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap_or_default().as_secs();
    let backup_file = format!("{backup_dir}/{user}_{domain}_{timestamp}.tar.gz");

    // `--` ends option parsing so a local-part beginning with '-' can't be read as a tar flag.
    let output = tokio::time::timeout(
        std::time::Duration::from_secs(300),
        safe_command("tar").args(["czf", &backup_file, "-C", &format!("/var/vmail/{domain}"), "--", user]).output()
    ).await
        .map_err(|_| err(StatusCode::GATEWAY_TIMEOUT, "Backup timed out"))?
        .map_err(|e| err(StatusCode::INTERNAL_SERVER_ERROR, &format!("Backup failed: {e}")))?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(err(StatusCode::INTERNAL_SERVER_ERROR, &format!("Backup failed: {stderr}")));
    }

    // 0600: the archive is a full copy of the mailbox's plaintext mail — never world-readable.
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = tokio::fs::set_permissions(&backup_file, std::fs::Permissions::from_mode(0o600)).await;
    }

    // Get file size
    let size = tokio::fs::metadata(&backup_file).await.map(|m| m.len()).unwrap_or(0);

    tracing::info!("Mailbox backed up: {email} -> {backup_file} ({size} bytes)");
    Ok(Json(serde_json::json!({ "ok": true, "file": backup_file, "size": size })))
}

/// POST /mail/restore — Restore a mailbox from backup.
async fn mailbox_restore(Json(body): Json<serde_json::Value>) -> Result<Json<serde_json::Value>, ApiErr> {
    let email = body.get("email").and_then(|v| v.as_str()).unwrap_or("");
    let backup_file = body.get("file").and_then(|v| v.as_str()).unwrap_or("");

    if email.is_empty() || !email.contains('@') { return Err(err(StatusCode::BAD_REQUEST, "Invalid email")); }
    if backup_file.is_empty() || !backup_file.starts_with("/var/lib/dockpanel/mail-backups/") {
        return Err(err(StatusCode::BAD_REQUEST, "Invalid backup file path"));
    }
    if backup_file.contains("..") { return Err(err(StatusCode::BAD_REQUEST, "Path traversal not allowed")); }
    if !Path::new(backup_file).exists() { return Err(err(StatusCode::NOT_FOUND, "Backup file not found")); }

    let parts: Vec<&str> = email.splitn(2, '@').collect();
    if parts.len() != 2 { return Err(err(StatusCode::BAD_REQUEST, "Invalid email format")); }
    let (user, domain) = (parts[0], parts[1]);
    // Reject path traversal in the email — otherwise `maildir` below could escape /var/vmail into
    // an agent-writable root-daemon config dir (the tar `-C` target + the recursive chown). This
    // mirrors the guard the sibling mailbox_backup already applies.
    if user.is_empty() || domain.is_empty()
        || user.contains('/') || user.contains('\\') || user.contains("..")
        || domain.contains('/') || domain.contains('\\') || domain.contains("..") {
        return Err(err(StatusCode::BAD_REQUEST, "Invalid email format"));
    }
    let maildir = format!("/var/vmail/{domain}");

    // Restore
    let output = tokio::time::timeout(
        std::time::Duration::from_secs(300),
        safe_command("tar").args(["xzf", backup_file, "-C", &maildir]).output()
    ).await
        .map_err(|_| err(StatusCode::GATEWAY_TIMEOUT, "Restore timed out"))?
        .map_err(|e| err(StatusCode::INTERNAL_SERVER_ERROR, &format!("Restore failed: {e}")))?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(err(StatusCode::INTERNAL_SERVER_ERROR, &format!("Restore failed: {stderr}")));
    }

    // Fix permissions
    let _ = safe_command("chown").args(["-R", "vmail:vmail", &format!("{maildir}/{user}")]).output().await;

    tracing::info!("Mailbox restored: {email} from {backup_file}");
    Ok(ok(&format!("Mailbox {email} restored")))
}

/// GET /mail/backups — List available mailbox backups.
async fn mailbox_backups() -> Result<Json<serde_json::Value>, ApiErr> {
    let backup_dir = "/var/lib/dockpanel/mail-backups";
    let mut backups = Vec::new();

    let mut entries = match tokio::fs::read_dir(backup_dir).await {
        Ok(e) => e,
        Err(_) => return Ok(Json(serde_json::json!({ "backups": [] }))),
    };

    while let Ok(Some(entry)) = entries.next_entry().await {
        let name = entry.file_name().to_string_lossy().to_string();
        if !name.ends_with(".tar.gz") { continue; }
        let size = entry.metadata().await.map(|m| m.len()).unwrap_or(0);
        let path = entry.path().to_string_lossy().to_string();

        // Parse email from filename: user_domain_timestamp.tar.gz
        let parts: Vec<&str> = name.trim_end_matches(".tar.gz").rsplitn(2, '_').collect();
        let email_hint = if parts.len() >= 2 { parts[1].replacen('_', "@", 1) } else { name.clone() };

        backups.push(serde_json::json!({ "file": path, "name": name, "email": email_hint, "size": size }));
    }

    // Sort by name (timestamp in filename = chronological)
    backups.sort_by(|a, b| b["name"].as_str().unwrap_or("").cmp(a["name"].as_str().unwrap_or("")));

    Ok(Json(serde_json::json!({ "backups": backups })))
}

/// POST /mail/backups/delete — Delete a backup file.
async fn mailbox_backup_delete(Json(body): Json<serde_json::Value>) -> Result<Json<serde_json::Value>, ApiErr> {
    let file = body.get("file").and_then(|v| v.as_str()).unwrap_or("");
    if file.is_empty() || !file.starts_with("/var/lib/dockpanel/mail-backups/") || file.contains("..") {
        return Err(err(StatusCode::BAD_REQUEST, "Invalid backup file"));
    }
    tokio::fs::remove_file(file).await.map_err(|e| err(StatusCode::INTERNAL_SERVER_ERROR, &format!("Delete failed: {e}")))?;
    Ok(ok("Backup deleted"))
}

// ── TLS Enforcement ─────────────────────────────────────────────────────

/// GET /mail/tls/status — Check TLS configuration in Postfix and Dovecot.
async fn tls_status() -> Json<serde_json::Value> {
    let main_cf = tokio::fs::read_to_string("/etc/postfix/main.cf").await.unwrap_or_default();

    let smtpd_tls = main_cf.lines().find(|l| l.starts_with("smtpd_tls_security_level"))
        .and_then(|l| l.split('=').nth(1)).unwrap_or("").trim().to_string();
    let smtp_tls = main_cf.lines().find(|l| l.starts_with("smtp_tls_security_level"))
        .and_then(|l| l.split('=').nth(1)).unwrap_or("").trim().to_string();

    // Check Dovecot SSL
    let dovecot_conf = tokio::fs::read_to_string("/etc/dovecot/conf.d/99-dockpanel.conf").await.unwrap_or_default();
    let dovecot_ssl = dovecot_conf.lines().find(|l| l.starts_with("ssl"))
        .and_then(|l| l.split('=').nth(1)).unwrap_or("").trim().to_string();

    Json(serde_json::json!({
        "inbound_tls": smtpd_tls,
        "outbound_tls": smtp_tls,
        "dovecot_ssl": dovecot_ssl,
        "inbound_enforced": smtpd_tls == "encrypt",
        "outbound_enforced": smtp_tls == "encrypt",
    }))
}

/// A value no SASL login can ever equal, used as the owner of a hosted domain
/// that holds no mailbox. Every login on this box is a full address (Dovecot's
/// passwd-file key is the address itself), so a token with no `@` is
/// unmatchable by construction rather than by convention.
const NO_AUTHORISED_SENDER: &str = "dockpanel-no-authorised-sender";

/// Build the Postfix `smtpd_sender_login_maps` table: which SASL login may use
/// which envelope sender.
///
/// Neither existing map can serve this, and reusing one would MANUFACTURE the
/// defect this closes. `virtual_mailbox_maps` values are maildir paths, so
/// pointing the restriction at it would refuse every authenticated send on the
/// box. `virtual_alias_maps` values are FORWARD TARGETS — so a mailbox that
/// merely set forwarding would lose the right to send as itself while the
/// forward target gained it, and any tenant could create
/// `sales@theirs -> victim@other-tenant` and thereby hand that tenant send-as
/// rights over their own address. The map has to be built, not borrowed.
///
/// Two kinds of entry. Postfix looks up the full address first and `@domain`
/// second, so the exact rows win where they exist:
///
/// 1. every enabled mailbox, owned by itself;
/// 2. one `@domain` row per enabled domain, owned by that domain's own enabled
///    logins.
///
/// ⚠ THIS TABLE IS AN ALLOW-LIST, and the first version of this comment said
/// the opposite. postconf(5), the authenticated form armed below: reject when
/// the client is SASL-authenticated but "either the MAIL FROM address is not
/// listed in $smtpd_sender_login_maps, or the SASL login name is not an owner
/// for that address". An address this table does not mention is REFUSED, not
/// permitted. The variant that checks only already-listed addresses is a
/// different parameter, and it is deliberately not the one used.
///
/// So the `@domain` row does not close a forgery hole — the allow-list closes
/// it, because an invented local part at another tenant's domain is unlisted
/// and refused either way. The row is what keeps a domain's own logins able to
/// send as their aliases, catch-alls and role addresses. Without it a mailbox
/// could send only as the exact string it authenticated with, and every alias
/// on the box would stop working.
///
/// The consequence an operator needs, written here rather than left to be
/// discovered from a bounce: an authenticated client may use an envelope sender
/// only if it is a mailbox on this box, or sits at a domain hosted on it. A
/// subdomain that was never added as its own mail domain is a different domain
/// and is refused; so is any external address. That includes the panel's own
/// notification sender when it relays through this box — `smtp_from` and
/// `smtp_username` are independent settings, so a `smtp_from` the authenticated
/// mailbox does not own is refused like any other unowned sender.
///
/// A domain with no enabled mailbox is owned by [`NO_AUTHORISED_SENDER`], so
/// nobody may send as it. Under the allow-list that row is belt-and-braces
/// rather than load-bearing — omitting it would leave the domain unlisted and
/// therefore refused anyway — but it states the intent inside the table, and it
/// keeps the answer correct if the restriction is ever changed to the variant
/// that only consults listed addresses.
///
/// The bound this does NOT claim, stated here rather than left to be discovered
/// from behaviour: two mailboxes in the SAME domain may still send as each
/// other. They belong to the same site owner, so that is not a tenant crossing.
/// The control is per-domain.
fn build_sender_login_map(body: &SyncRequest) -> String {
    let mut lines: Vec<String> = Vec::new();

    for acc in body.accounts.iter().filter(|a| a.enabled) {
        lines.push(format!("{}\t{}", acc.email, acc.email));
    }

    for domain in body.domains.iter().filter(|d| d.enabled) {
        // `ends_with("@example.com")` is an exact-domain test, not a suffix
        // one: it does not match `user@sub.example.com`, which is a different
        // domain and may belong to a different tenant.
        let suffix = format!("@{}", domain.domain);
        let owners: Vec<&str> = body.accounts.iter()
            .filter(|a| a.enabled && a.email.ends_with(&suffix))
            .map(|a| a.email.as_str())
            .collect();
        let owners = if owners.is_empty() {
            NO_AUTHORISED_SENDER.to_string()
        } else {
            owners.join(",")
        };
        lines.push(format!("@{}\t{}", domain.domain, owners));
    }

    lines.join("\n")
}

/// Write the sender-login map, rebuild its hash, and arm the restriction that
/// consults it — strictly in that order.
///
/// The order is the whole of the risk in this change. A `hash:` table named by
/// an smtpd restriction is opened when smtpd initialises: if the `.db` is
/// missing or stale, smtpd exits fatal and master throttles it — a total outage
/// on 25 and 587, not a per-message deferral. So the map is written and
/// postmapped first, its exit status is CHECKED — the four other `postmap` call
/// sites in this file discard theirs — and the keys are written only if that
/// succeeded. A failure here leaves the box exactly as it was: unprotected, and
/// still carrying mail. For a control being added underneath a running system
/// that is the correct direction to fail, and it is the reason this is not
/// simply two more lines in the config block.
///
/// `relay_configure` already keeps this discipline for `sasl_passwd` (map,
/// postmap, then the reference). Nothing asserted it; this makes it explicit.
async fn ensure_sender_login_enforcement(map_content: &str) {
    if let Err(e) = write_file_atomic(POSTFIX_SENDER_LOGIN, map_content).await {
        tracing::error!(
            "Sender-login map not written ({e}) — leaving sender identity unenforced rather \
             than arming a restriction against a map that is not there"
        );
        return;
    }

    match safe_command("postmap").arg(POSTFIX_SENDER_LOGIN).output().await {
        Ok(out) if out.status.success() => {}
        Ok(out) => {
            tracing::error!(
                "postmap {POSTFIX_SENDER_LOGIN} failed ({}) — leaving sender identity \
                 unenforced; arming the restriction against an unopenable map would take \
                 smtpd down on 25 and 587. stderr: {}",
                out.status,
                String::from_utf8_lossy(&out.stderr)
            );
            return;
        }
        Err(e) => {
            tracing::error!(
                "postmap {POSTFIX_SENDER_LOGIN} could not be run ({e}) — leaving sender \
                 identity unenforced"
            );
            return;
        }
    }

    // A map with no rows authorises NOBODY, and the restriction is an
    // allow-list, so arming against one does not leave the box unprotected — it
    // refuses EVERY authenticated submission on it, 553, with no other symptom.
    // This is the trap the old "an unowned sender is permitted" reading hid: it
    // made an empty map look like the harmless case when it is the total one.
    //
    // `mail_install` reaches here with whatever the map file already held, and
    // on every box upgrading into the release that introduced that file it held
    // nothing. Re-running the installer is the ordinary response to mail looking
    // broken, so without this the installer's own retry path would take the
    // box's authenticated mail off the air until the next mailbox change.
    //
    // Declining costs nothing. The map is empty only when the box has no enabled
    // mailbox and no enabled domain, and a mailbox that does not exist cannot
    // authenticate — so there is no send to refuse and no forge to prevent. The
    // first domain or account mutation calls this again through `sync_config`
    // with a populated map, and that is what arms it. The map itself is still
    // written above, so the table on disk keeps telling the truth either way.
    if map_content.trim().is_empty() {
        tracing::info!(
            "Sender-login map is empty (no enabled mailbox or domain) — writing it but NOT \
             arming the restriction. Arming an allow-list against an empty table would refuse \
             every authenticated submission on this box. The next mail sync arms it."
        );
        return;
    }

    // Written with `postconf`, not added to the config block in `mail_install`:
    // that block is appended once and skipped for ever after (the same reason
    // `mydestination` is re-applied there), so a template edit would reach new
    // installs only and no box already running mail. These two keys reach every
    // box that has mail installed, on its next mailbox change.
    //
    // No `-e`: Postfix assumes it whenever a value is given (2.8 and later), as
    // the `inet_interfaces` / `myhostname` / TLS calls in `mail_install` do.
    let _ = safe_command("postconf")
        .arg(format!("smtpd_sender_login_maps=hash:{POSTFIX_SENDER_LOGIN}"))
        .output()
        .await;
    // NO `permit_mynetworks` in front of this, and that is the whole difference
    // between a control and a decoration.
    //
    // The first cut of this fix wrote `permit_mynetworks, reject_…`, reasoning
    // that it protected the panel's own notifications and every site's PHP
    // mail(). Driven on a real two-tenant box, that version was INERT: every
    // forge was accepted, because `mynetworks_style = host` still covers
    // 127.0.0.0/8, and a `permit` matches before the reject is ever reached. On
    // a hosting box "connections from this machine" is not a trusted set — it
    // is every tenant's own code. Isolated by experiment: plain `hash:` with no
    // `permit_mynetworks` rejects the forge; `proxy:hash:` WITH it accepts.
    //
    // And it protected nothing. Local submission — `sendmail`, which is what
    // PHP mail() invokes — is `non_smtpd` and never passes through
    // `smtpd_sender_restrictions` at all. Measured on the same box: after this
    // parameter was removed, `sendmail -f anything@hosted.domain` was still
    // accepted, queued and delivered. The only thing the permit exempted was an
    // authenticated SMTP session from localhost, which is exactly the thing
    // being defended against.
    //
    // The AUTHENTICATED form of the check, though. The bare
    // `reject_sender_login_mismatch` would also refuse UNAUTHENTICATED inbound
    // mail on 25 whose envelope sender is a hosted address — ordinary for a
    // mailing list, a forwarder, or a tenant's own address used by an external
    // SaaS. The threat is an authenticated co-tenant; restricting more than
    // that would break delivery to pay for nothing.
    let _ = safe_command("postconf")
        .arg("smtpd_sender_restrictions=reject_authenticated_sender_login_mismatch")
        .output()
        .await;
}

/// POST /mail/tls/enforce — Set TLS enforcement level.
async fn tls_enforce(Json(body): Json<serde_json::Value>) -> Result<Json<serde_json::Value>, ApiErr> {
    let inbound = body.get("inbound").and_then(|v| v.as_str()).unwrap_or("may");
    let outbound = body.get("outbound").and_then(|v| v.as_str()).unwrap_or("may");

    if !["may", "encrypt", "none"].contains(&inbound) || !["may", "encrypt", "none"].contains(&outbound) {
        return Err(err(StatusCode::BAD_REQUEST, "Level must be 'may', 'encrypt', or 'none'"));
    }

    let main_cf = tokio::fs::read_to_string("/etc/postfix/main.cf").await.unwrap_or_default();
    let cleaned: String = main_cf.lines()
        .filter(|l| !l.starts_with("smtpd_tls_security_level") && !l.starts_with("smtp_tls_security_level"))
        .collect::<Vec<_>>().join("\n");

    let tls_config = format!("\nsmtpd_tls_security_level = {inbound}\nsmtp_tls_security_level = {outbound}\n");

    write_file_atomic("/etc/postfix/main.cf", &format!("{cleaned}{tls_config}")).await
        .map_err(|e| err(StatusCode::INTERNAL_SERVER_ERROR, &format!("Config write failed: {e}")))?;

    let _ = safe_command("systemctl").args(["reload", "postfix"]).output().await;

    tracing::info!("TLS enforcement: inbound={inbound}, outbound={outbound}");
    Ok(ok(&format!("TLS: inbound={inbound}, outbound={outbound}")))
}

// ── Helper ──────────────────────────────────────────────────────────────

async fn write_file_atomic(path: &str, content: &str) -> Result<(), String> {
    let tmp_path = format!("{path}.tmp");
    tokio::fs::write(&tmp_path, content).await.map_err(|e| e.to_string())?;
    tokio::fs::rename(&tmp_path, path).await.map_err(|e| e.to_string())?;
    Ok(())
}

/// Write the Dovecot passwd-file so that Dovecot can actually read it.
///
/// The file holds every mail account's password hash, so it must not be
/// world-readable. `0600 root:root` enforces that and is the instinctive
/// choice — but it is one notch too strict, and the cost is the entire mail
/// vertical: Dovecot's auth worker drops privileges to the `dovecot` user, so
/// it could not open this file at all. Every IMAP, POP3 and submission login
/// failed with `Temporary authentication failure` while the panel reported the
/// account as created successfully, and the only evidence was a
/// `Permission denied (euid=…(dovecot) … missing +r perm)` line in Dovecot's
/// own log. `0640` with group `dovecot` is the arrangement Dovecot documents:
/// still unreadable to every other user on the box, readable by the one
/// process whose job is to read it.
///
/// The group is set on the temporary file *before* the rename, so the real
/// path is never momentarily visible with ownership that would lock auth out.
/// A box that has no `dovecot` group has no Dovecot either, so a failing
/// `chown` is not worth losing the write over — the mode already keeps the
/// hashes private.
async fn write_dovecot_users(content: &str) -> Result<(), String> {
    let tmp_path = format!("{DOVECOT_USERS}.tmp");
    tokio::fs::write(&tmp_path, content).await.map_err(|e| e.to_string())?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        tokio::fs::set_permissions(&tmp_path, std::fs::Permissions::from_mode(0o640))
            .await
            .map_err(|e| e.to_string())?;
    }
    let _ = safe_command("chown").args(["root:dovecot", &tmp_path]).output().await;
    tokio::fs::rename(&tmp_path, DOVECOT_USERS).await.map_err(|e| e.to_string())?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The regression this helper exists for: between s268 and s270 the value
    /// on the line was `inet:127.0.0.1:8891`, and the old code matched the
    /// literal `unix:opendkim/opendkim.sock`. Whatever OpenDKIM's endpoint is,
    /// Rspamd's must end up beside it.
    #[test]
    fn appends_beside_whatever_milter_is_already_there() {
        for existing in [OPENDKIM_MILTER, "unix:opendkim/opendkim.sock", "inet:localhost:8891"] {
            let cf = format!("mydomain = x\nsmtpd_milters = {existing}\nnon_smtpd_milters = {existing}\n");
            let out = add_milter(&cf, RSPAMD_MILTER).expect("milter keys present");
            assert!(out.contains(&format!("smtpd_milters = {existing}, {RSPAMD_MILTER}")), "got: {out}");
            assert!(out.contains(&format!("non_smtpd_milters = {existing}, {RSPAMD_MILTER}")), "got: {out}");
            assert!(out.ends_with('\n'));
        }
    }

    #[test]
    fn is_idempotent() {
        let cf = format!("smtpd_milters = {OPENDKIM_MILTER}\nnon_smtpd_milters = {OPENDKIM_MILTER}\n");
        let once = add_milter(&cf, RSPAMD_MILTER).unwrap();
        let twice = add_milter(&once, RSPAMD_MILTER).unwrap();
        assert_eq!(once, twice);
        assert_eq!(twice.matches(RSPAMD_MILTER).count(), 2);
    }

    /// No milter list at all means mail was never installed. Returning `None`
    /// is what lets the caller fail loudly instead of writing back a file it
    /// did not change — the silent no-op that hid the defect for two ships.
    #[test]
    fn absent_keys_report_absence_rather_than_a_no_op() {
        assert!(add_milter("mydomain = example.com\n", RSPAMD_MILTER).is_none());
        assert!(add_milter("", RSPAMD_MILTER).is_none());
    }

    /// A commented-out or lookalike key must not be treated as the setting.
    #[test]
    fn ignores_comments_and_lookalike_keys() {
        assert!(add_milter("# smtpd_milters = inet:127.0.0.1:8891\n", RSPAMD_MILTER).is_none());
        assert!(add_milter("smtpd_milters_extra = x\n", RSPAMD_MILTER).is_none());
    }

    #[test]
    fn fills_an_empty_list_without_a_leading_comma() {
        let out = add_milter("smtpd_milters =\n", RSPAMD_MILTER).unwrap();
        assert!(out.contains(&format!("smtpd_milters = {RSPAMD_MILTER}")), "got: {out}");
        assert!(!out.contains(", "), "got: {out}");
    }

    // ── Sender identity ────────────────────────────────────────────────
    //
    // The property under test is the one no source grep can reach: given a
    // payload, does the generated table let tenant A claim tenant B's domain?
    // Proving the RESTRICTION rejects the forge needs a real Postfix and two
    // authenticated tenants — that is the fresh-VPS leg, not these.

    fn domain(name: &str, enabled: bool) -> SyncDomain {
        SyncDomain { domain: name.into(), enabled, catch_all: None }
    }

    fn account(email: &str, enabled: bool) -> SyncAccount {
        SyncAccount {
            email: email.into(),
            password_hash: "{PLAIN}x".into(),
            quota_mb: 100,
            enabled,
            forward_to: None,
        }
    }

    /// Look up a key the way Postfix does: the full address first, then
    /// `@domain`. Encoding the lookup order here is deliberate — a table is
    /// only correct with respect to how it is read, and every assertion below
    /// asks a question an SMTP session would actually ask.
    fn owners_of(table: &str, sender: &str) -> Option<String> {
        let domain_key = sender.split_once('@').map(|(_, d)| format!("@{d}"));
        let find = |key: &str| {
            table.lines()
                .filter_map(|l| l.split_once('\t'))
                .find(|(k, _)| *k == key)
                .map(|(_, v)| v.to_string())
        };
        find(sender).or_else(|| domain_key.and_then(|k| find(&k)))
    }

    fn may_send_as(table: &str, login: &str, sender: &str) -> bool {
        owners_of(table, sender)
            .map(|v| v.split(',').any(|o| o == login))
            // An absent key is REFUSAL, not permission: the armed restriction
            // rejects an authenticated client whose MAIL FROM "is not listed in
            // $smtpd_sender_login_maps". This model asserted the opposite until
            // the parameter's own manual page was read against it — and not one
            // of the tests below moved when it was corrected, because every one
            // of them names an address at a hosted domain, which is listed under
            // either reading. The two tests that DO discriminate were written
            // afterwards, and are the only reason this line is now testable.
            .unwrap_or(false)
    }

    fn two_tenants() -> SyncRequest {
        SyncRequest {
            domains: vec![domain("tenant-a.com", true), domain("tenant-b.com", true)],
            accounts: vec![account("sales@tenant-a.com", true), account("info@tenant-b.com", true)],
            aliases: vec![],
        }
    }

    #[test]
    fn a_mailbox_may_send_as_itself() {
        let t = build_sender_login_map(&two_tenants());
        assert!(may_send_as(&t, "sales@tenant-a.com", "sales@tenant-a.com"));
        assert!(may_send_as(&t, "info@tenant-b.com", "info@tenant-b.com"));
    }

    /// The defect, stated as a test: before this map existed, both of these
    /// were permitted, and the forged message left the box DKIM-signed with the
    /// victim's key.
    #[test]
    fn a_tenant_may_not_send_as_another_tenants_domain() {
        let t = build_sender_login_map(&two_tenants());
        assert!(!may_send_as(&t, "sales@tenant-a.com", "info@tenant-b.com"));
        assert!(!may_send_as(&t, "sales@tenant-a.com", "ceo@tenant-b.com"));
        assert!(!may_send_as(&t, "info@tenant-b.com", "sales@tenant-a.com"));
    }

    /// The `@domain` row is what covers every address that is not itself a
    /// mailbox — aliases, catch-alls, and any local part a forger invents.
    /// Without it the control would only defend addresses that already exist,
    /// which is the smaller half of the surface.
    #[test]
    fn an_invented_local_part_at_a_hosted_domain_is_still_owned() {
        let t = build_sender_login_map(&two_tenants());
        assert!(owners_of(&t, "no-such-mailbox@tenant-b.com").is_some());
        assert!(!may_send_as(&t, "sales@tenant-a.com", "no-such-mailbox@tenant-b.com"));
    }

    /// A domain nobody holds a mailbox on must not become sendable-as by
    /// whoever happens to hold one elsewhere on the box.
    #[test]
    fn a_domain_with_no_mailbox_is_owned_by_nobody() {
        let mut req = two_tenants();
        req.domains.push(domain("parked.com", true));
        let t = build_sender_login_map(&req);
        assert_eq!(owners_of(&t, "anyone@parked.com").as_deref(), Some(NO_AUTHORISED_SENDER));
        assert!(!may_send_as(&t, "sales@tenant-a.com", "anyone@parked.com"));
        assert!(!may_send_as(&t, "info@tenant-b.com", "anyone@parked.com"));
    }

    /// Disabled accounts are already excluded from the Dovecot users file, so
    /// they hold no login at all. Listing one as an owner would grant an
    /// identity to a credential that cannot authenticate — and, worse, would
    /// leave a domain looking owned when its only mailbox is switched off.
    #[test]
    fn a_disabled_mailbox_owns_neither_itself_nor_its_domain() {
        let req = SyncRequest {
            domains: vec![domain("tenant-a.com", true)],
            accounts: vec![account("sales@tenant-a.com", false)],
            aliases: vec![],
        };
        let t = build_sender_login_map(&req);
        assert!(!t.contains("sales@tenant-a.com\tsales@tenant-a.com"));
        assert_eq!(owners_of(&t, "sales@tenant-a.com").as_deref(), Some(NO_AUTHORISED_SENDER));
    }

    /// `ends_with("@example.com")` is an exact-domain test, not a suffix one.
    /// A subdomain is a different domain and may belong to a different tenant,
    /// so its mailboxes must not be counted among the parent's owners.
    #[test]
    fn a_subdomain_is_not_its_parent_domain() {
        let req = SyncRequest {
            domains: vec![domain("example.com", true), domain("mail.example.com", true)],
            accounts: vec![account("a@example.com", true), account("b@mail.example.com", true)],
            aliases: vec![],
        };
        let t = build_sender_login_map(&req);
        assert!(!may_send_as(&t, "b@mail.example.com", "a@example.com"));
        assert!(!may_send_as(&t, "a@example.com", "anything@mail.example.com"));
        // The one that actually bites, and the one the first two cannot reach:
        // both of those name addresses the exact rows already cover, so they
        // pass even when the WILDCARD is wrong. A relaxed domain test leaks
        // through the `@domain` row, on an invented local part — so that is
        // where it has to be asked. (Found by mutation, not by writing.)
        assert!(!may_send_as(&t, "b@mail.example.com", "invented@example.com"));
    }

    /// The two tests the original eight could not express, because all eight
    /// named addresses at hosted domains and those are listed under either
    /// reading of the restriction. These are the ones that fail if `may_send_as`
    /// goes back to treating an absent key as permission — i.e. they are the
    /// tests that would have caught the premise being wrong.
    #[test]
    fn an_address_at_no_hosted_domain_is_refused() {
        let t = build_sender_login_map(&two_tenants());
        assert!(owners_of(&t, "anyone@external.example").is_none());
        assert!(!may_send_as(&t, "sales@tenant-a.com", "anyone@external.example"));
    }

    /// The cheapest way a legitimate operator meets the bound: a shop or app on
    /// a subdomain that was never added as its own mail domain. Refused, and
    /// the fix is to add the subdomain — not to widen the map.
    #[test]
    fn a_subdomain_that_was_never_added_is_refused() {
        let t = build_sender_login_map(&two_tenants());
        assert!(owners_of(&t, "orders@shop.tenant-a.com").is_none());
        assert!(!may_send_as(&t, "sales@tenant-a.com", "orders@shop.tenant-a.com"));
    }

    /// Two mailboxes in the SAME domain belong to the same site owner, so this
    /// is not a tenant crossing. Pinned so the bound is a decision on the
    /// record rather than something a later reader has to infer.
    #[test]
    fn mailboxes_within_one_domain_may_send_as_each_other() {
        let req = SyncRequest {
            domains: vec![domain("tenant-a.com", true)],
            accounts: vec![account("sales@tenant-a.com", true), account("ceo@tenant-a.com", true)],
            aliases: vec![],
        };
        let t = build_sender_login_map(&req);
        assert!(may_send_as(&t, "sales@tenant-a.com", "invoices@tenant-a.com"));
    }

    /// Every line must be exactly `key<TAB>value`, or `postmap` builds a table
    /// that does not answer the question the restriction asks — and a bad hash
    /// is the failure mode that takes smtpd down.
    #[test]
    fn every_line_is_a_well_formed_two_column_row() {
        let mut req = two_tenants();
        req.domains.push(domain("disabled.com", false));
        let t = build_sender_login_map(&req);
        assert!(!t.is_empty());
        for line in t.lines() {
            let cols: Vec<&str> = line.split('\t').collect();
            assert_eq!(cols.len(), 2, "not two columns: {line:?}");
            assert!(!cols[0].is_empty() && !cols[1].is_empty(), "empty column: {line:?}");
            assert!(!cols[1].contains(",,"), "empty owner in list: {line:?}");
        }
        assert!(!t.contains("@disabled.com"), "a disabled domain must get no row");
    }
}
