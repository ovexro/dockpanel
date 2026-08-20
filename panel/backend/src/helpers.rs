/// Shared helper functions used across multiple route modules.
use sha2::{Sha256, Digest};

/// Resolve a site the caller is allowed to act on, and return its domain.
///
/// Ownership decides whose a site is; the caller's role decides what may be
/// reached. A site still belongs to exactly one account and a transfer still
/// moves it — but an administrator may act on any site on the server in front of
/// them, because an operator who runs a box has to be able to repair anything
/// running on it. That is the same boundary the admin all-sites read already
/// draws: what that view lists, its owner may now also act on. It stops at the
/// server in scope and does not reach a machine somebody else registered.
///
/// This replaces six separately-named private copies of one query — two called
/// `site_domain`, four called `get_site_domain` — plus the inline repeats beside
/// them. They had drifted: exactly one of the set was also server-scoped. A
/// guard duplicated per module is a guard that gets widened in one module.
///
/// ⚠ The non-admin arm is deliberately left as it was, predicate for predicate,
/// and must NOT also become server-scoped. No non-admin can own a `servers` row
/// — the only INSERT is admin-gated — so their scope always resolves to the
/// local machine. Adding `server_id` on this arm would hide a client's own site
/// whenever it lives on any other server, and it would do it by returning an
/// empty list rather than an error, which is the direction nobody checks.
///
/// ⚠ That paragraph is true and its conclusion — do not filter this predicate by
/// server — is still the right call. But it was read for years as though it also
/// said the SITE is always local, and it does not say that. A caller's scope being
/// local is a fact about the CALLER; the site's row names its own host and the two
/// part company as soon as a fleet exists. Resolving WHICH SITE from here and WHICH
/// HOST from the caller's selection is the actual defect, and filtering cannot fix
/// it — the row has to choose the host. Use `site_agent_for_caller` below for
/// anything that then talks to an agent.
pub async fn site_domain_for_caller(
    state: &crate::AppState,
    site_id: uuid::Uuid,
    claims: &crate::auth::Claims,
) -> Result<String, crate::error::ApiError> {
    let row: Option<(String,)> =
        sqlx::query_as(&format!("SELECT s.domain FROM sites s WHERE {SITE_CALLER_PREDICATE}"))
            .bind(site_id)
            .bind(claims.sub)
            .fetch_optional(&state.db)
            .await
            .map_err(|e| crate::error::internal_error("resolve site for caller", e))?;

    row.map(|(d,)| d)
        .ok_or_else(|| crate::error::err(axum::http::StatusCode::NOT_FOUND, "Site not found"))
}

/// Resolve a site the caller may act on, AND the agent for the host it actually runs on.
///
/// "Which server did the caller select" and "which server is this site on" are different
/// questions. They agree on a single-box install and whenever an operator happens to have
/// the right server selected, which is why the difference stayed invisible. They disagree
/// silently the rest of the time: the header-derived scope falls back to the local agent
/// when a caller owns no server row, while the site's row may name a fleet member — so the
/// panel resolves the right domain and then asks the wrong machine about it.
///
/// The row is the authority. That rule is already stated and followed by the webhook deploy
/// path, by the git-deploy update path, and by every background service that walks these
/// tables; the authenticated HTTP handlers are the layer that never adopted it. A host that
/// will not resolve is REFUSED — never quietly replaced with this one, because the failure
/// mode of substituting is writing one tenant's files onto another machine.
pub async fn site_agent_for_caller(
    state: &crate::AppState,
    site_id: uuid::Uuid,
    claims: &crate::auth::Claims,
) -> Result<(String, crate::services::agent::AgentHandle), crate::error::ApiError> {
    let row: Option<(String, Option<uuid::Uuid>)> = sqlx::query_as(&format!(
        "SELECT s.domain, s.server_id FROM sites s WHERE {SITE_CALLER_PREDICATE}"
    ))
    .bind(site_id)
    .bind(claims.sub)
    .fetch_optional(&state.db)
    .await
    .map_err(|e| crate::error::internal_error("resolve site agent for caller", e))?;

    let (domain, server_id) = row
        .ok_or_else(|| crate::error::err(axum::http::StatusCode::NOT_FOUND, "Site not found"))?;

    let agent = agent_for_site_server(state, server_id, &domain).await?;
    Ok((domain, agent))
}

/// Resolve the agent for the server a row names, refusing rather than substituting.
///
/// Split out because several handlers have already loaded the row (and so already hold its
/// server id) and only need this half. The column is `NOT NULL` in the schema but optional
/// in the structs that predate the fleet work, so the `None` arm is a real branch: it means
/// a row older than the backfill, and guessing a host for it is exactly what must not happen.
pub async fn agent_for_site_server(
    state: &crate::AppState,
    server_id: Option<uuid::Uuid>,
    domain: &str,
) -> Result<crate::services::agent::AgentHandle, crate::error::ApiError> {
    let server_id = server_id.ok_or_else(|| {
        tracing::warn!("Refusing to act on {domain}: its row names no server");
        crate::error::err(
            axum::http::StatusCode::CONFLICT,
            "This site is not associated with a server",
        )
    })?;

    state.agents.for_server(server_id).await.map_err(|e| {
        tracing::warn!(
            "Refusing to act on {domain}: its server {server_id} is unreachable ({e}) — \
             acting through a different host would touch another machine's files"
        );
        crate::error::err(
            axum::http::StatusCode::BAD_GATEWAY,
            "The server this site lives on is unreachable",
        )
    })
}

/// Which of the caller's own sites carries this domain, if any.
///
/// ⚠ NOT a claim check, and must never be used as one. `domain_claim::
/// ensure_claimable` is the single answer to "may this domain be taken", and
/// `tests/domain-claim-pin-e2e.sh` §B2 exists because paths that answered it
/// privately drifted apart. This answers a different question — "which existing
/// site of mine is this" — for a caller that needs to attach something to a site
/// the operator names by domain. It lives here, once, so it cannot become the
/// per-path copy that pin forbids.
///
/// Owner-scoped rather than using [`SITE_CALLER_PREDICATE`]: the callers write a
/// row onto the site they get back, and an administrator resolving another
/// account's domain by name is not a capability anything needs.
pub async fn site_id_for_owned_domain(
    db: &sqlx::PgPool,
    domain: &str,
    user_id: uuid::Uuid,
) -> Option<uuid::Uuid> {
    let wanted = domain.trim().trim_end_matches('.').to_ascii_lowercase();
    if wanted.is_empty() {
        return None;
    }
    sqlx::query_as::<_, (uuid::Uuid,)>(
        "SELECT id FROM sites WHERE lower(domain) = $1 AND user_id = $2",
    )
    .bind(&wanted)
    .bind(user_id)
    .fetch_optional(db)
    .await
    .ok()
    .flatten()
    .map(|(id,)| id)
}

/// The one predicate that decides whether a caller may act on a site.
///
/// Binds `$1` = site id, `$2` = the caller's own id — the same two parameters the
/// owner-only predicate it replaces took, which is what let every call site adopt
/// it without touching its binds.
///
/// Two ways to satisfy it: own the row, or be an administrator of the machine the
/// row runs on. `is_local` is this box; `sv.user_id` is a member this same
/// administrator registered. So it stops at the hardware the reader operates and
/// does not reach a server somebody else added.
///
/// ⚠ The admin arm reads `users.role` from the DATABASE, deliberately, and not
/// `claims.role` from the token. A JWT keeps asserting whatever role it was minted
/// with until it expires, so trusting the claim would leave a demoted account
/// acting as an administrator for the rest of its session. This costs one indexed
/// lookup inside a query that was already running.
pub const SITE_CALLER_PREDICATE: &str = "s.id = $1 AND (s.user_id = $2 OR EXISTS (\
    SELECT 1 FROM users u, servers sv WHERE u.id = $2 AND u.role = 'admin' \
    AND sv.id = s.server_id AND (sv.is_local OR sv.user_id = u.id)))";

// ── Suspension: the role an account gets back ───────────────────────────────
//
// Suspending overwrites `users.role`, which is the only record of what the
// account was, so something has to hold the previous value until the account is
// un-suspended. That used to be `users.reset_token` — a column the public
// password-reset flow also writes, from an endpoint that checked no role at
// all. A suspended account could therefore erase its own stash by asking for a
// password reset, and the restore then handed back whatever the fallback said.
//
// The stash now has a column of its own, and both places that suspend an
// account go through the two functions below rather than keeping a private copy
// of the query. There were eight private copies of the last query that decided
// an authorisation question in this codebase, under three different names, and
// exactly one of them had quietly drifted (v2.72.0). Two is how that starts.

/// Suspend an account, recording the role it held, and cut its live sessions.
///
/// The write is ONE statement, deliberately. The stash and the status change
/// cannot come apart — there is no window in which `role` has been overwritten
/// but nothing remembers what it was, which is what a read-then-write pair would
/// leave open if the process died between them. Postgres evaluates every SET
/// right-hand side against the OLD row, so `prior_role = role` takes the
/// pre-update value.
///
/// `AND role <> 'suspended'` matters for the same reason: without it, suspending
/// an already-suspended account would overwrite a perfectly good stash with the
/// word `suspended`, and the account could then never be given its role back.
///
/// **Revoking the sessions is part of suspending, not a step callers remember.**
/// The JWT middleware only refuses a token whose *claim* says `suspended`, and a
/// token minted while the account was a `user` keeps saying `user` until it
/// expires two hours later. The panel had always revoked; the billing webhook
/// never had, so a billing suspension did nothing at all for up to two hours.
/// Putting it here is the only way the two paths are actually the same rules,
/// which is what the guide claims.
///
/// `extra_predicate` is appended to the WHERE clause for callers that may only
/// suspend some accounts — the billing webhook refuses to touch a privileged
/// one. It is `&'static str` so it cannot carry anything caller-supplied into
/// the SQL. Returns the number of rows changed, so a caller can tell "suspended"
/// from "declined".
pub async fn suspend_account(
    state: &crate::AppState,
    id: uuid::Uuid,
    extra_predicate: &'static str,
) -> Result<u64, crate::error::ApiError> {
    let sql = format!(
        "UPDATE users SET prior_role = role, role = 'suspended', updated_at = NOW() \
         WHERE id = $1 AND role <> 'suspended'{extra_predicate}"
    );
    let changed = sqlx::query(&sql)
        .bind(id)
        .execute(&state.db)
        .await
        .map(|r| r.rows_affected())
        .map_err(|e| crate::error::internal_error("suspend account", e))?;

    if changed > 0 {
        crate::routes::auth::revoke_all_user_sessions(state, id).await;
    }
    Ok(changed)
}

/// Which role an un-suspend may restore, given what was recorded.
///
/// Pure, and deliberately separated from the query it serves: this single
/// expression IS the authorisation decision, and everything around it is I/O.
/// Kept apart so it can be exercised exhaustively in-process — the async
/// DB-bound wrapper cannot be, and a decision no test can reach is a decision
/// that drifts. `None` means *leave the account suspended and say so*; it is
/// never a default, and there is deliberately no branch here that invents a role.
///
/// An unrecognised stash — a role since retired, or a leftover from the old
/// shared column — is treated exactly like no stash at all. It is never handed
/// back verbatim: `users.role` carries a CHECK constraint, and a rejected write
/// is a 500 on the button an administrator presses to undo their own action.
pub fn role_to_restore(stashed: Option<&str>, deny: &[&str]) -> Option<String> {
    let role = stashed?;
    if !crate::routes::users::ASSIGNABLE_ROLES.contains(&role) {
        return None;
    }
    if deny.contains(&role) {
        return None;
    }
    Some(role.to_string())
}

/// Give back the role an account held before it was suspended.
///
/// Returns the restored role, or `None` when the account was left suspended —
/// which happens in exactly two cases, both deliberate:
///
///   * **the prior role is unknown**, and
///   * **the prior role is one `deny` forbids this caller to restore.**
///
/// **It never guesses, and that is the whole design.** An earlier draft fell back
/// to the least-privileged role on an unknown stash, on the reasoning that
/// `client` is strictly weaker than `user`. That reasoning was wrong: reseller
/// management is scoped `AND role = 'user'`
/// (`routes/reseller_dashboard.rs:243`, `:317`), so an account handed `client`
/// is still listed by its reseller and can no longer be managed by them — the
/// fallback would not withhold one capability, it would strand the account in a
/// state no operator asked for. There is no ordering of these roles that makes a
/// guess safe, so the caller is told instead.
///
/// `deny` is the same idea as `suspend_account`'s predicate, in the other
/// direction. The billing webhook must not RESTORE a privileged role either: its
/// suspend arm refuses to touch an `admin`, but before this it would happily hand
/// `admin` back to an account the *panel* had suspended, which turned a webhook
/// secret into a privilege-restoration primitive.
///
/// Reads `prior_role` with a narrow `SELECT` naming the column, never
/// `SELECT *`. That is not style: `models::User` derives `FromRow` and thirteen
/// queries load it that way, including the ones that authenticate a login. Keeping
/// the new column out of that struct means a database restored from a snapshot
/// older than this migration degrades THIS call alone instead of failing
/// every login.
pub async fn unsuspend_account(
    state: &crate::AppState,
    id: uuid::Uuid,
    deny: &[&str],
) -> Result<Option<String>, crate::error::ApiError> {
    let stashed: Option<(Option<String>,)> =
        sqlx::query_as("SELECT prior_role FROM users WHERE id = $1")
            .bind(id)
            .fetch_optional(&state.db)
            .await
            .map_err(|e| crate::error::internal_error("read prior role", e))?;

    let stashed = stashed.and_then(|(r,)| r);
    let Some(role) = role_to_restore(stashed.as_deref(), deny) else {
        return Ok(None);
    };

    // Clear the stash in the same statement that consumes it. A stash left behind
    // would be applied again by a later un-suspend, silently overriding whatever
    // role an administrator had chosen in between.
    //
    // `AND role = 'suspended'` makes this idempotent and safe for a caller that
    // has not already checked — the billing webhook receives an unsuspend hook for
    // whatever state the account happens to be in, and must not rewrite the role
    // of an account that was never suspended.
    sqlx::query(
        "UPDATE users SET role = $1, prior_role = NULL, updated_at = NOW() \
         WHERE id = $2 AND role = 'suspended'",
    )
    .bind(&role)
    .bind(id)
    .execute(&state.db)
    .await
    .map_err(|e| crate::error::internal_error("unsuspend account", e))?;

    Ok(Some(role))
}

/// Hash an agent token using SHA-256. Agent tokens are high-entropy (UUIDs)
/// so SHA-256 is sufficient — no need for slow hashing (argon2/bcrypt).
pub fn hash_agent_token(token: &str) -> String {
    let hash = Sha256::digest(token.as_bytes());
    hex::encode(hash)
}

/// Build Cloudflare API headers from credentials.
///
/// If `email` is provided, uses Global API Key auth (X-Auth-Email + X-Auth-Key).
/// Otherwise, uses Bearer token auth.
pub fn cf_headers(token: &str, email: Option<&str>) -> reqwest::header::HeaderMap {
    // The stored Cloudflare token / global API key is encrypted at rest (see
    // dns::create_zone). Decrypt here — the single choke point every CF caller funnels
    // through — so no read site can forget to. The legacy fallback returns any
    // pre-encryption plaintext value (and a freshly-entered token during validation)
    // unchanged, so the round trip is transparent.
    let token = crate::services::secrets_crypto::decrypt_credential_from_env(token);
    let token = token.as_str();
    let mut headers = reqwest::header::HeaderMap::new();
    if let Some(em) = email {
        if let (Ok(e_val), Ok(k_val)) = (em.parse(), token.parse()) {
            headers.insert("X-Auth-Email", e_val);
            headers.insert("X-Auth-Key", k_val);
        }
    } else if let Ok(bearer) = format!("Bearer {token}").parse() {
        headers.insert("Authorization", bearer);
    }
    headers
}

#[cfg(test)]
mod suspend_restore_tests {
    use super::role_to_restore;

    /// The sentence the whole change exists to make true: what comes back is what
    /// was recorded, for every role that can be recorded — never something else.
    #[test]
    fn every_recorded_role_comes_back_as_itself() {
        for role in crate::routes::users::ASSIGNABLE_ROLES {
            assert_eq!(
                role_to_restore(Some(role), &[]).as_deref(),
                Some(role),
                "un-suspending must return the role that was recorded, not a substitute"
            );
        }
    }

    /// ⚠ THE ARM THIS MODULE EXISTS FOR. An un-suspend must never invent a role.
    /// The first draft of this change fell back to the least-privileged role on an
    /// unknown stash; that was wrong, because there is no total ordering of these
    /// roles — reseller management is scoped `AND role = 'user'`, so handing an
    /// account `client` strands it outside its own reseller's reach rather than
    /// merely withholding something. Any future `unwrap_or` here reintroduces a
    /// silent role change on the one path nobody watches.
    #[test]
    fn an_unknown_prior_role_is_never_guessed() {
        assert_eq!(role_to_restore(None, &[]), None, "no stash must not mean a default role");
        assert_eq!(
            role_to_restore(Some(""), &[]),
            None,
            "an empty stash must not mean a default role"
        );
    }

    /// The old shared column held a 64-character SHA-256 digest. If one is still
    /// sitting in a stash, it must be refused, not written into a
    /// CHECK-constrained column where it becomes a 500 on the Unsuspend button.
    ///
    /// ⚠ Named around the digest rather than around the column, deliberately.
    /// `tests/suspend-restore-pin-e2e.sh` §B asserts that the password-reset
    /// column is named ONLY by the reset flow and the model, and a pin greps raw
    /// source — so an identifier here spelling that column would have to be
    /// excused in the arm, weakening the one census that would catch the stash
    /// moving back. Keep the name clear of it.
    #[test]
    fn a_leftover_password_digest_is_refused_not_restored() {
        // Synthetic, but the right SHAPE — 64 hex characters, which is what
        // `auth::hash_token` produces and what would actually be sitting there.
        let token = "deadbeefdeadbeefdeadbeefdeadbeefdeadbeefdeadbeefdeadbeefdeadbeef";
        assert_eq!(token.len(), 64, "the fixture must be the length a real digest is");
        assert_eq!(role_to_restore(Some(token), &[]), None);
        assert_eq!(
            role_to_restore(Some("suspended"), &[]),
            None,
            "'suspended' is a status, never a role to restore"
        );
    }

    /// The billing webhook may not hand back a privileged role. Its suspend arm
    /// already refuses to touch one; without this the same secret could RESTORE
    /// `admin` to an account the panel had deliberately suspended.
    #[test]
    fn a_denied_role_is_not_restored_and_the_account_stays_suspended() {
        let deny = ["admin", "reseller"];
        assert_eq!(role_to_restore(Some("admin"), &deny), None);
        assert_eq!(role_to_restore(Some("reseller"), &deny), None);
    }

    /// The control that keeps the test above from passing vacuously: the same
    /// deny-list must still let the ordinary roles through, or billing
    /// un-suspension would be broken for everybody rather than guarded.
    #[test]
    fn the_deny_list_still_lets_ordinary_roles_through() {
        let deny = ["admin", "reseller"];
        assert_eq!(role_to_restore(Some("user"), &deny).as_deref(), Some("user"));
        assert_eq!(role_to_restore(Some("client"), &deny).as_deref(), Some("client"));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_hash_agent_token_deterministic() {
        let hash1 = hash_agent_token("test-token-123");
        let hash2 = hash_agent_token("test-token-123");
        assert_eq!(hash1, hash2);
    }

    #[test]
    fn test_hash_agent_token_different_inputs() {
        let hash1 = hash_agent_token("token-a");
        let hash2 = hash_agent_token("token-b");
        assert_ne!(hash1, hash2);
    }

    #[test]
    fn test_hash_agent_token_length() {
        let hash = hash_agent_token("any-token");
        assert_eq!(hash.len(), 64); // SHA-256 = 32 bytes = 64 hex chars
    }

    #[test]
    fn test_hash_agent_token_hex_format() {
        let hash = hash_agent_token("test");
        assert!(hash.chars().all(|c| c.is_ascii_hexdigit()));
    }

    #[test]
    fn test_hash_agent_token_known_value() {
        // SHA-256("hello") = 2cf24dba5fb0a30e26e83b2ac5b9e29e1b161e5c1fa7425e73043362938b9824
        let hash = hash_agent_token("hello");
        assert_eq!(hash, "2cf24dba5fb0a30e26e83b2ac5b9e29e1b161e5c1fa7425e73043362938b9824");
    }

    #[test]
    fn test_hash_empty_token() {
        let hash = hash_agent_token("");
        assert_eq!(hash.len(), 64);
    }

    // ---- SSRF validator (validate_url_not_internal) ----
    //
    // These arms exist to make ONE mutation impossible to reintroduce silently: the
    // hand-rolled "strip scheme, split on '/' then ':'" host extraction, which for
    // `http://example.com:x@169.254.169.254/` tested `example.com` (public → passed)
    // while every HTTP client connects to the userinfo host `169.254.169.254`.
    //
    // No network is touched by the cases below: every rejection here is decided from
    // the URL structure alone (a literal-IP host or a credentialed authority), before
    // the resolver. A future rewrite that returns to substring parsing would read the
    // wrong host and let these through — so each is RED under exactly that mutation.

    use super::url_authority;

    /// ⭐ THE ARM THIS MODULE EXISTS FOR, and it is deterministic OFFLINE in both
    /// directions. `url_authority` names the host a client would dial; the retired
    /// hand-rolled parser named the userinfo label instead. Every label here is a
    /// PUBLIC numeric literal and every true host an INTERNAL numeric literal, so
    /// `getaddrinfo` never runs — a DNS-free runner gives the same verdict, which the
    /// earlier `example.com` version did not (it went green under the mutation whenever
    /// the runner had no DNS). Restoring `split('/').next().split(':').next()` returns
    /// the left literal and turns every assert below red.
    #[test]
    fn the_validated_host_is_the_host_a_client_would_connect_to() {
        // Host AND port come from the real authority, never a substring + hardcoded :80.
        // No resolver runs, so this is deterministic on an air-gapped runner and RED
        // against the retired parser, which returned ("[2001", 80) / ("example.com", 80).
        assert_eq!(
            url_authority("http://[2001:db8::1]:8443/").unwrap(),
            ("[2001:db8::1]".to_string(), 8443),
            "a bracketed IPv6 literal and its real port, not the string '2001' and port 80"
        );
        assert_eq!(
            url_authority("http://example.com:8443/path").unwrap(),
            ("example.com".to_string(), 8443),
            "the real port survives; the old parser hardcoded 80"
        );
        assert_eq!(
            url_authority("https://198.51.100.7:9000/admin").unwrap(),
            ("198.51.100.7".to_string(), 9000),
            "host and port both from the authority"
        );
        // The userinfo construct — the exact host/validator disagreement — is REFUSED,
        // not host-extracted. A substring parser reads the public left label and returns
        // Ok, so each of these is red under the mutation.
        assert!(
            url_authority("http://192.0.2.1:x@127.0.0.1/").is_err(),
            "a credentialed authority must be refused, not resolved to its userinfo label"
        );
        assert!(url_authority("https://198.51.100.7:tok@10.0.0.5:8080/admin").is_err());
    }

    async fn v(u: &str) -> Result<(), String> {
        super::validate_url_not_internal(u).await
    }

    #[tokio::test]
    async fn userinfo_spoofed_internal_host_is_rejected() {
        // Same numeric-literal shape so no DNS is needed even under the mutation: the
        // public label on the left, the internal host on the right, and the metadata
        // address among them. RED against the substring parser on any runner.
        for u in [
            "http://192.0.2.1:x@127.0.0.1/",
            "http://192.0.2.1:80@169.254.169.254/latest/meta-data/",
            "https://198.51.100.7:tok@10.0.0.5:8080/admin",
        ] {
            assert!(
                v(u).await.is_err(),
                "userinfo-spoofed URL must be rejected, true host is internal: {u}"
            );
        }
    }

    #[tokio::test]
    async fn alternate_ip_encodings_and_v6_embeddings_of_internal_are_rejected() {
        // `url` normalises the IPv4 encodings; `v6_is_internal` extracts the embedded v4
        // from the transition prefixes. The last two are the gap the parser fix alone
        // left open (NAT64 well-known + 6to4, both embedding 127.0.0.1).
        for u in [
            "http://2130706433/",               // decimal 127.0.0.1
            "http://0x7f000001/",               // hex
            "http://0177.0.0.1/",               // octal
            "http://[::1]/",                    // v6 loopback
            "http://[::ffff:169.254.169.254]/", // v4-mapped metadata
            "http://[64:ff9b::7f00:1]/",        // NAT64 well-known, 127.0.0.1
            "http://[2002:7f00:1::]/",          // 6to4, 127.0.0.1
        ] {
            assert!(v(u).await.is_err(), "internal literal host must be rejected: {u}");
        }
    }

    #[tokio::test]
    async fn credentials_in_a_public_authority_are_still_refused() {
        assert!(
            v("https://user:tok@example.com/hook").await.is_err(),
            "a URL with credentials must be refused regardless of the host"
        );
    }

    #[tokio::test]
    async fn a_bare_scheme_or_non_http_scheme_is_refused() {
        assert!(v("").await.is_err());
        assert!(v("ftp://example.com/").await.is_err());
        assert!(v("file:///etc/passwd").await.is_err());
        assert!(v("not a url").await.is_err());
    }
}

/// True if an IPv4 address is loopback / private / link-local / CGNAT / unspecified
/// / broadcast — i.e. one an SSRF guard must reject.
fn v4_is_internal(v4: std::net::Ipv4Addr) -> bool {
    let o = v4.octets();
    v4.is_loopback()
        || v4.is_private()
        || v4.is_link_local()
        || v4.is_unspecified()
        || v4.is_broadcast()
        || o[0] == 169 // link-local/metadata block (broad; matches the original guard)
        || (o[0] == 100 && (o[1] & 0xC0) == 0x40) // CGNAT 100.64.0.0/10
}

/// True if an IPv6 address is loopback / unspecified / ULA (fc00::/7) /
/// link-local (fe80::/10), OR embeds an internal IPv4 via a transition mechanism.
///
/// Four embeddings reach the same internal v4 through a v6 literal, and only one of
/// them (`::ffff:a.b.c.d`) is normalized by `Ipv6Addr::to_ipv4_mapped()`. The other
/// three had no rule, so `http://[64:ff9b::7f00:1]/` (NAT64, embedding 127.0.0.1)
/// passed even the rewritten validator until this was added:
///   * `::ffff:a.b.c.d`  IPv4-mapped   — low 32 bits (handled by `to_ipv4_mapped`)
///   * `64:ff9b::a.b.c.d` NAT64 WKP     — low 32 bits
///   * `::a.b.c.d`        IPv4-compat   — low 32 bits (deprecated, still routed by some stacks)
///   * `2002:a.b.c.d::/16` 6to4         — bits [16..48]
/// In every case the embedded v4 is classified by `v4_is_internal`, so one change to
/// the v4 ranges covers all forms at once.
fn v6_is_internal(v6: std::net::Ipv6Addr) -> bool {
    if v6.is_loopback() || v6.is_unspecified() {
        return true;
    }
    let seg = v6.segments();
    let low_v4 = std::net::Ipv4Addr::new(
        (seg[6] >> 8) as u8,
        (seg[6] & 0xff) as u8,
        (seg[7] >> 8) as u8,
        (seg[7] & 0xff) as u8,
    );
    // IPv4-mapped (::ffff:0:0/96), NAT64 well-known (64:ff9b::/96), and IPv4-compatible
    // (::/96) all carry the embedded v4 in the low 32 bits.
    let is_mapped = seg[0] == 0 && seg[1] == 0 && seg[2] == 0 && seg[3] == 0 && seg[4] == 0 && seg[5] == 0xffff;
    let is_nat64 = seg[0] == 0x0064 && seg[1] == 0xff9b && seg[2] == 0 && seg[3] == 0 && seg[4] == 0 && seg[5] == 0;
    let is_v4compat = seg[0] == 0 && seg[1] == 0 && seg[2] == 0 && seg[3] == 0 && seg[4] == 0 && seg[5] == 0;
    if (is_mapped || is_nat64 || is_v4compat) && v4_is_internal(low_v4) {
        return true;
    }
    // 6to4 (2002::/16) carries the embedded v4 in bits [16..48].
    if seg[0] == 0x2002 {
        let v4 = std::net::Ipv4Addr::new(
            (seg[1] >> 8) as u8,
            (seg[1] & 0xff) as u8,
            (seg[2] >> 8) as u8,
            (seg[2] & 0xff) as u8,
        );
        if v4_is_internal(v4) {
            return true;
        }
    }
    (seg[0] & 0xfe00) == 0xfc00 // ULA fc00::/7
        || (seg[0] & 0xffc0) == 0xfe80 // link-local fe80::/10
}

/// True if an IP is one an SSRF guard must reject (loopback / private / link-local /
/// ULA / CGNAT / IPv4-mapped-internal / unspecified).
fn ip_is_internal(ip: std::net::IpAddr) -> bool {
    match ip {
        std::net::IpAddr::V4(v4) => v4_is_internal(v4),
        std::net::IpAddr::V6(v6) => v6_is_internal(v6),
    }
}

/// True if `entry` — a plain IP or a CIDR block like `10.0.0.0/8` — covers `client`.
/// Families must match; a v4 entry never covers a v6 client. Anything unparseable
/// covers nothing, so a typo denies rather than admits (see `valid_panel_ip_entry`,
/// which rejects such entries at write time so an operator cannot lock themselves out).
fn ip_entry_covers(entry: &str, client: std::net::IpAddr) -> bool {
    let entry = entry.trim();
    let Some((net, bits)) = entry.split_once('/') else {
        return entry
            .parse::<std::net::IpAddr>()
            .map(|ip| ip == client)
            .unwrap_or(false);
    };
    let (Ok(net), Ok(bits)) = (net.trim().parse::<std::net::IpAddr>(), bits.trim().parse::<u32>())
    else {
        return false;
    };
    match (net, client) {
        (std::net::IpAddr::V4(n), std::net::IpAddr::V4(c)) => {
            if bits > 32 {
                return false;
            }
            // A /0 must not shift by 32 (UB-adjacent, and `checked_shl` returns None).
            let mask = if bits == 0 { 0 } else { u32::MAX << (32 - bits) };
            u32::from(n) & mask == u32::from(c) & mask
        }
        (std::net::IpAddr::V6(n), std::net::IpAddr::V6(c)) => {
            if bits > 128 {
                return false;
            }
            let mask = if bits == 0 { 0 } else { u128::MAX << (128 - bits) };
            u128::from(n) & mask == u128::from(c) & mask
        }
        _ => false,
    }
}

/// True if `client` is covered by any entry of a comma-separated panel IP allowlist.
/// The caller decides what an EMPTY list means (today: no restriction at all).
pub fn panel_ip_allowed(list: &str, client: &str) -> bool {
    let Ok(client) = client.trim().parse::<std::net::IpAddr>() else {
        // No usable client address (e.g. no `x-real-ip` from the proxy) and a
        // non-empty allowlist: deny. An allowlist that cannot identify the caller
        // must fail CLOSED.
        return false;
    };
    list.split(',')
        .map(str::trim)
        .filter(|e| !e.is_empty())
        .any(|e| ip_entry_covers(e, client))
}

/// True if `entry` is a form `panel_ip_allowed` can actually match. Used to reject a
/// malformed allowlist at write time — otherwise a typo silently locks every admin out
/// of the panel, and the only way back in is editing the database by hand.
pub fn valid_panel_ip_entry(entry: &str) -> bool {
    let entry = entry.trim();
    match entry.split_once('/') {
        None => entry.parse::<std::net::IpAddr>().is_ok(),
        Some((net, bits)) => match (net.trim().parse::<std::net::IpAddr>(), bits.trim().parse::<u32>()) {
            (Ok(std::net::IpAddr::V4(_)), Ok(b)) => b <= 32,
            (Ok(std::net::IpAddr::V6(_)), Ok(b)) => b <= 128,
            _ => false,
        },
    }
}

/// True if a redirect target `host` is internal: a literal internal IP, `localhost`,
/// OR a hostname that resolves (blocking) to any internal IP. For use inside a reqwest
/// redirect-policy closure, which is synchronous and cannot `.await` — so this resolves
/// with the blocking std resolver. That is acceptable here: it only runs when a monitored
/// target actually returns a 3xx (rare), inside the background uptime task. Fails CLOSED
/// (treats an unresolvable target as internal) so a redirect is never followed blindly.
pub fn host_resolves_internal_blocking(host: &str, port: u16) -> bool {
    let h = host.trim_matches(|c| c == '[' || c == ']');
    if h.eq_ignore_ascii_case("localhost") {
        return true;
    }
    if let Ok(ip) = h.parse::<std::net::IpAddr>() {
        return ip_is_internal(ip);
    }
    use std::net::ToSocketAddrs;
    match (h, port).to_socket_addrs() {
        Ok(addrs) => addrs.into_iter().any(|a| ip_is_internal(a.ip())),
        Err(_) => true, // cannot resolve → do not follow
    }
}

/// SSRF protection: reject a URL whose host is (or resolves to) an internal address.
///
/// Parsed with `url::Url` rather than by hand. The previous "strip the scheme, take up
/// to the first `/`, then up to the first `:`" logic extracted the WRONG host for a
/// userinfo authority: for `http://example.com:x@169.254.169.254/` it tested
/// `example.com` (public, so it passed) while every HTTP client here connects to the
/// real host, `169.254.169.254`. A real parser also normalises alternate IPv4 encodings
/// (`http://2130706433/`, `0x7f000001`, octal) back to dotted-decimal, so a literal-IP
/// host is authoritative and never depends on the resolver agreeing.
///
/// Rejects: any credentials in the authority (the userinfo-spoof vector), and a host
/// that is or resolves to loopback / private (RFC 1918) / link-local & metadata
/// (169.254/16) / CGNAT (100.64/10) / ULA / IPv4-mapped-internal / unspecified /
/// broadcast — the set `ip_is_internal` covers.
pub async fn validate_url_not_internal(url: &str) -> Result<(), String> {
    // `url_authority` decides WHAT host to check — the one a client would dial, not the
    // userinfo label the old string-surgery mistook for it. Everything below only
    // decides whether that host is internal, so the two concerns can be pinned apart.
    let (host, port) = url_authority(url)?;

    // A literal IP host (in any encoding — `url` has already normalised it) is
    // authoritative; check it directly and never resolve.
    let bare = host.trim_matches(|c| c == '[' || c == ']');
    if let Ok(ip) = bare.parse::<std::net::IpAddr>() {
        if ip_is_internal(ip) {
            return Err("URL points to a private/internal address".to_string());
        }
        return Ok(());
    }

    // Obvious internal names, before paying for a DNS lookup.
    let lower = host.to_ascii_lowercase();
    if lower == "localhost" || lower.ends_with(".localhost") {
        return Err("URL points to a local address".to_string());
    }

    // Resolve and reject if ANY resolved address is internal (split-horizon / rebind).
    match tokio::net::lookup_host((host.as_str(), port)).await {
        Ok(addrs) => {
            let mut resolved_any = false;
            for addr in addrs {
                resolved_any = true;
                if ip_is_internal(addr.ip()) {
                    return Err("URL resolves to a private/internal address".to_string());
                }
            }
            if !resolved_any {
                return Err("URL hostname could not be resolved".to_string());
            }
        }
        Err(_) => {
            return Err("URL hostname could not be resolved".to_string());
        }
    }

    Ok(())
}

/// SSRF guard for a BARE host — the tcp/ping monitor lane stores a host, not a URL, and
/// `check_tcp`/`check_ping` dial it directly. Without this a non-suspended account could
/// point a tcp monitor at `10.0.0.1:3306` or `127.0.0.1:22` and read connect/refused/
/// filtered back as the check status: an internal port scanner the HTTP lane already
/// forbids. Rejects the same address set as [`validate_url_not_internal`], so the two
/// lanes agree on what "internal" means. `port` is used only for the resolver call.
pub async fn validate_host_not_internal(host: &str, port: u16) -> Result<(), String> {
    let host = host.trim();
    if host.is_empty() {
        return Err("Host is required".to_string());
    }
    let bare = host.trim_matches(|c| c == '[' || c == ']');
    if let Ok(ip) = bare.parse::<std::net::IpAddr>() {
        if ip_is_internal(ip) {
            return Err("Host points to a private/internal address".to_string());
        }
        return Ok(());
    }
    let lower = host.to_ascii_lowercase();
    if lower == "localhost" || lower.ends_with(".localhost") {
        return Err("Host points to a local address".to_string());
    }
    match tokio::net::lookup_host((bare, port)).await {
        Ok(addrs) => {
            let mut resolved_any = false;
            for addr in addrs {
                resolved_any = true;
                if ip_is_internal(addr.ip()) {
                    return Err("Host resolves to a private/internal address".to_string());
                }
            }
            if !resolved_any {
                return Err("Host could not be resolved".to_string());
            }
        }
        Err(_) => {
            return Err("Host could not be resolved".to_string());
        }
    }
    Ok(())
}

/// Parse a URL and return the (host, port) a client would actually connect to.
///
/// Pure and synchronous, so it can be pinned by equality without a resolver: for
/// `http://192.0.2.1:x@127.0.0.1/` this returns `("127.0.0.1", 80)`, the host the HTTP
/// client dials — where the retired hand-rolled parser returned the userinfo label
/// `192.0.2.1` and the hardcoded port 80. `host_str()` keeps the brackets on an IPv6
/// literal (`"[2001:db8::1]"`); the caller strips them before parsing an `IpAddr`.
///
/// Rejects: a parse failure, a non-http(s) scheme, and any credentials in the authority
/// — the userinfo construct is the only thing that let the client and a validator
/// disagree about the host, and nothing this guard protects legitimately carries one.
pub fn url_authority(url: &str) -> Result<(String, u16), String> {
    let url = url.trim();
    if url.is_empty() {
        return Err("URL is required".to_string());
    }

    let parsed = url::Url::parse(url).map_err(|_| "URL is not valid".to_string())?;

    if !matches!(parsed.scheme(), "http" | "https") {
        return Err("URL must use http or https".to_string());
    }

    if !parsed.username().is_empty() || parsed.password().is_some() {
        return Err("URL must not contain credentials".to_string());
    }

    let host = parsed
        .host_str()
        .ok_or_else(|| "URL has no hostname".to_string())?
        .to_string();
    let port = parsed.port_or_known_default().unwrap_or(80);
    Ok((host, port))
}

/// Detect the server's public IPv4 address.
///
/// Tries the ipify.org API first (5s timeout), falls back to local UDP socket detection.
pub async fn detect_public_ip() -> String {
    match reqwest::Client::new()
        .get("https://api.ipify.org")
        .timeout(std::time::Duration::from_secs(5))
        .send()
        .await
    {
        Ok(resp) => {
            let ip = resp.text().await.unwrap_or_default().trim().to_string();
            if ip.is_empty() { String::new() } else { ip }
        }
        Err(_) => {
            use std::net::UdpSocket;
            UdpSocket::bind("0.0.0.0:0")
                .and_then(|s| { s.connect("8.8.8.8:53")?; s.local_addr() })
                .map(|a| a.ip().to_string())
                .unwrap_or_default()
        }
    }
}

/// Cache for [`detect_public_ip_cached`]: the resolved address and when it was read.
static PUBLIC_IP_CACHE: std::sync::Mutex<Option<(String, std::time::Instant)>> =
    std::sync::Mutex::new(None);

/// How long a detected public IP stays fresh. A box's public address changes
/// approximately never; five minutes is short enough that a real change is picked
/// up quickly and long enough that an interactive preflight is effectively free.
const PUBLIC_IP_TTL: std::time::Duration = std::time::Duration::from_secs(300);

/// [`detect_public_ip`] with a short-lived cache.
///
/// The uncached version makes an outbound HTTPS request to api.ipify.org on EVERY
/// call. That is fine for a once-per-provision check, but the DNS preflight runs
/// as the user types a domain — uncached, a single create-site form would issue
/// dozens of external requests and get rate-limited.
pub async fn detect_public_ip_cached() -> String {
    if let Ok(guard) = PUBLIC_IP_CACHE.lock() {
        if let Some((ip, at)) = guard.as_ref() {
            if at.elapsed() < PUBLIC_IP_TTL && !ip.is_empty() {
                return ip.clone();
            }
        }
    }

    let ip = detect_public_ip().await;

    // Never cache a failure — a transient network blip would otherwise pin an
    // empty address for the whole TTL and silently disable every DNS check.
    if !ip.is_empty() {
        if let Ok(mut guard) = PUBLIC_IP_CACHE.lock() {
            *guard = Some((ip.clone(), std::time::Instant::now()));
        }
    }
    ip
}

/// The public address to PUBLISH IN DNS for a resource that lives on `server_id`.
///
/// [`detect_public_ip`] answers "what is *this process's* public address", which is
/// the panel's. That is the right answer only when the resource in question is on
/// the panel's own box, and every auto-DNS path used it unconditionally: creating a
/// site on a fleet member published an A record pointing at the PANEL, so the site
/// was unreachable at the name it was just given. The delete path was worse in the
/// quieter direction — it removes the record only when `content == server_ip`, so a
/// member's record never matched and survived the site's deletion as a dangling A
/// record aimed at a host that no longer serves it, which is a stale-DNS and
/// subdomain-takeover surface rather than a cosmetic leftover.
///
/// `servers.ip_address` is the authoritative answer and was already being kept
/// current — `agent_checkin` refreshes it on every check-in — but nothing consulted
/// it when publishing. It was read only to draw dashboards.
///
/// Returns `None` rather than falling back to the panel's address when the host
/// cannot be resolved. Publishing DNS is exactly the operation where substituting a
/// different machine's address produces a confidently wrong, externally-visible
/// result, so the callers skip the record and say so instead — the same
/// refuse-rather-than-substitute rule [`agent_for_site_server`] applies to agents.
pub async fn public_ip_for_server(
    db: &sqlx::PgPool,
    server_id: Option<uuid::Uuid>,
) -> Option<String> {
    let Some(server_id) = server_id else {
        tracing::warn!("Auto-DNS: no server recorded for this resource — refusing to publish a record rather than guessing a host");
        return None;
    };

    let row: Option<(Option<String>, bool)> =
        sqlx::query_as("SELECT ip_address, is_local FROM servers WHERE id = $1")
            .bind(server_id)
            .fetch_optional(db)
            .await
            .ok()
            .flatten();

    match row {
        // The local box: detect it outbound exactly as before. The `servers` row for
        // the local agent often carries a LAN address (it is whatever the agent saw
        // of itself), so the stored column is the wrong source here even though it
        // is the right one for every other member.
        Some((_, true)) => {
            let ip = detect_public_ip_cached().await;
            if ip.is_empty() { None } else { Some(ip) }
        }
        Some((Some(ip), false)) if !ip.trim().is_empty() => Some(ip.trim().to_string()),
        Some((_, false)) => {
            tracing::warn!(
                "Auto-DNS: server {server_id} has no recorded ip_address — refusing to publish \
                 a record. The panel's own address would point this name at the wrong machine."
            );
            None
        }
        None => {
            tracing::warn!("Auto-DNS: server {server_id} no longer exists — refusing to publish a record");
            None
        }
    }
}

/// Parse the certificate expiry string the agent returns for SSL
/// provision / renew / DNS-01 responses.
///
/// The agent builds this with `x509_parser`'s `not_after.to_datetime().to_string()`
/// (agent `services/ssl.rs::get_cert_expiry`). That is a `time::OffsetDateTime`
/// `Display`, which renders as `"<date> <time> <offset>"` — e.g.
/// `2026-10-23 09:41:07.0 +00:00:00`. It never emits a literal `UTC`.
///
/// Every panel-side call site used to parse it with `"%Y-%m-%d %H:%M:%S%.f UTC"`,
/// which cannot match that shape, so the parse always failed and `sites.ssl_expiry`
/// was left NULL forever. That silently starved three features that read it: the
/// dashboard SSL countdown, `alert_engine::check_ssl_expiry` (whose query is
/// `WHERE ssl_enabled = TRUE AND ssl_expiry IS NOT NULL`, so it matched no rows at
/// all), and the ARI renewal bookkeeping in `auto_healer`.
///
/// Tolerant by design — panel and agent are separately-versioned binaries and a
/// fleet can run a mix, so accept the legacy `UTC` spelling and RFC 3339 too rather
/// than trade one brittle format for another.
/// Rewrite a trailing `+HH:MM:00` offset to `+HH:MM` so chrono can consume it.
/// Returns `None` when the string doesn't end in a zero-seconds offset, leaving the
/// caller to try its other formats.
fn strip_zero_offset_seconds(s: &str) -> Option<String> {
    // "+00:00:00" — 9 bytes, all ASCII, so byte indexing is safe.
    let tail = s.as_bytes();
    if tail.len() < 9 {
        return None;
    }
    let t = &tail[tail.len() - 9..];
    let shaped = matches!(t[0], b'+' | b'-')
        && t[1].is_ascii_digit()
        && t[2].is_ascii_digit()
        && t[3] == b':'
        && t[4].is_ascii_digit()
        && t[5].is_ascii_digit()
        && t[6] == b':'
        && t[7].is_ascii_digit()
        && t[8].is_ascii_digit();
    // Only whole-minute offsets: `:00` seconds carry no information to lose.
    if shaped && t[7] == b'0' && t[8] == b'0' {
        Some(s[..s.len() - 3].to_string())
    } else {
        None
    }
}

pub fn parse_agent_cert_expiry(raw: &str) -> Option<chrono::DateTime<chrono::Utc>> {
    let s = raw.trim();
    if s.is_empty() {
        return None;
    }

    // The current agent format. chrono's `%z` family stops after `+HH:MM`, so the
    // trailing `:SS` the `time` crate always prints would be left unconsumed and the
    // whole parse rejected. Drop that group first — but only when it is `:00`, so a
    // genuinely sub-minute offset falls through instead of being silently shifted.
    let normalised = strip_zero_offset_seconds(s);
    if let Ok(dt) = chrono::DateTime::parse_from_str(
        normalised.as_deref().unwrap_or(s),
        "%Y-%m-%d %H:%M:%S%.f %:z",
    ) {
        return Some(dt.with_timezone(&chrono::Utc));
    }

    // RFC 3339 / ISO 8601, in case the agent ever serialises a chrono type instead.
    if let Ok(dt) = chrono::DateTime::parse_from_rfc3339(s) {
        return Some(dt.with_timezone(&chrono::Utc));
    }

    // Legacy spelling kept so a newer panel never regresses an older agent.
    if let Ok(dt) = chrono::NaiveDateTime::parse_from_str(s, "%Y-%m-%d %H:%M:%S%.f UTC") {
        return Some(dt.and_utc());
    }

    None
}

#[cfg(test)]
mod cert_expiry_tests {
    use super::parse_agent_cert_expiry;
    use chrono::{Datelike, Timelike};

    /// THE regression test. This exact string is what
    /// `time::OffsetDateTime`'s `Display` produces, which is what the agent
    /// sends. If this ever fails, `ssl_expiry` is silently NULL again and the
    /// SSL countdown + expiry alerts go dark with no error anywhere.
    #[test]
    fn parses_the_format_the_agent_actually_sends() {
        let dt = parse_agent_cert_expiry("2026-10-23 09:41:07.0 +00:00:00")
            .expect("agent's time::OffsetDateTime Display form must parse");
        assert_eq!(dt.year(), 2026);
        assert_eq!(dt.month(), 10);
        assert_eq!(dt.day(), 23);
        assert_eq!(dt.hour(), 9);
        assert_eq!(dt.minute(), 41);
        assert_eq!(dt.second(), 7);
    }

    #[test]
    fn parses_without_fractional_seconds() {
        assert!(parse_agent_cert_expiry("2026-10-23 09:41:07 +00:00:00").is_some());
    }

    #[test]
    fn parses_a_non_utc_offset_and_normalises_to_utc() {
        let dt = parse_agent_cert_expiry("2026-10-23 11:41:07.0 +02:00:00")
            .expect("offset form must parse");
        assert_eq!(dt.hour(), 9, "must be converted to UTC, not read as wall time");
    }

    #[test]
    fn still_parses_the_legacy_utc_spelling() {
        assert!(parse_agent_cert_expiry("2026-10-23 09:41:07.0 UTC").is_some());
    }

    #[test]
    fn parses_rfc3339() {
        assert!(parse_agent_cert_expiry("2026-10-23T09:41:07Z").is_some());
    }

    #[test]
    fn rejects_junk_instead_of_defaulting() {
        assert!(parse_agent_cert_expiry("").is_none());
        assert!(parse_agent_cert_expiry("   ").is_none());
        assert!(parse_agent_cert_expiry("not a date").is_none());
        assert!(parse_agent_cert_expiry("2026-10-23").is_none());
    }
}

#[cfg(test)]
mod ssrf_tests {
    use super::{ip_is_internal, v4_is_internal, v6_is_internal};
    use std::net::{Ipv4Addr, Ipv6Addr};

    #[test]
    fn v4_internal_ranges_blocked() {
        for s in ["127.0.0.1", "10.1.2.3", "192.168.1.1", "172.16.0.1", "169.254.169.254", "0.0.0.0", "100.64.0.1"] {
            assert!(v4_is_internal(s.parse::<Ipv4Addr>().unwrap()), "{s} should be internal");
        }
    }

    #[test]
    fn v4_public_allowed() {
        for s in ["8.8.8.8", "1.1.1.1", "93.184.216.34", "100.63.255.255", "100.128.0.1"] {
            assert!(!v4_is_internal(s.parse::<Ipv4Addr>().unwrap()), "{s} should be public");
        }
    }

    #[test]
    fn v6_mapped_loopback_and_metadata_blocked() {
        // The exact gap: IPv4-mapped IPv6 whose is_loopback() is false.
        assert!(v6_is_internal("::ffff:127.0.0.1".parse::<Ipv6Addr>().unwrap()));
        assert!(v6_is_internal("::ffff:169.254.169.254".parse::<Ipv6Addr>().unwrap()));
        assert!(v6_is_internal("::ffff:10.0.0.1".parse::<Ipv6Addr>().unwrap()));
    }

    #[test]
    fn v6_ula_and_link_local_blocked() {
        assert!(v6_is_internal("fc00::1".parse::<Ipv6Addr>().unwrap()));
        assert!(v6_is_internal("fd00:ec2::254".parse::<Ipv6Addr>().unwrap()));
        assert!(v6_is_internal("fe80::1".parse::<Ipv6Addr>().unwrap()));
        assert!(v6_is_internal("::1".parse::<Ipv6Addr>().unwrap()));
        assert!(v6_is_internal("::".parse::<Ipv6Addr>().unwrap()));
    }

    #[test]
    fn v6_public_allowed() {
        assert!(!v6_is_internal("2606:4700:4700::1111".parse::<Ipv6Addr>().unwrap()));
        assert!(!ip_is_internal("2001:4860:4860::8888".parse::<std::net::IpAddr>().unwrap()));
    }
}

#[cfg(test)]
mod panel_ip_allowlist_tests {
    use super::{panel_ip_allowed, valid_panel_ip_entry};

    #[test]
    fn exact_addresses_match_either_family() {
        assert!(panel_ip_allowed("203.0.113.4", "203.0.113.4"));
        assert!(!panel_ip_allowed("203.0.113.4", "203.0.113.5"));
        assert!(panel_ip_allowed("2001:db8::1", "2001:db8::1"));
        // Written differently, same address — string equality (the pre-v2.46.0
        // behaviour) would reject this one.
        assert!(panel_ip_allowed("2001:0db8::0001", "2001:db8::1"));
    }

    #[test]
    fn cidr_ranges_cover_their_members() {
        assert!(panel_ip_allowed("10.0.0.0/8", "10.11.12.13"));
        assert!(!panel_ip_allowed("10.0.0.0/8", "11.0.0.1"));
        assert!(panel_ip_allowed("192.168.1.0/24", "192.168.1.255"));
        assert!(!panel_ip_allowed("192.168.1.0/24", "192.168.2.1"));
        assert!(panel_ip_allowed("2001:db8::/32", "2001:db8:dead:beef::1"));
        assert!(!panel_ip_allowed("2001:db8::/32", "2001:db9::1"));
    }

    #[test]
    fn prefix_boundaries_do_not_overflow() {
        // /0 covers everything of its family; a 32-bit shift would be UB-adjacent.
        assert!(panel_ip_allowed("0.0.0.0/0", "8.8.8.8"));
        assert!(panel_ip_allowed("::/0", "2001:db8::1"));
        // /32 and /128 are single hosts.
        assert!(panel_ip_allowed("8.8.8.8/32", "8.8.8.8"));
        assert!(!panel_ip_allowed("8.8.8.8/32", "8.8.8.9"));
        // Out-of-range prefixes match nothing rather than panicking.
        assert!(!panel_ip_allowed("8.8.8.8/33", "8.8.8.8"));
        assert!(!panel_ip_allowed("2001:db8::/129", "2001:db8::1"));
    }

    #[test]
    fn families_do_not_cross() {
        assert!(!panel_ip_allowed("0.0.0.0/0", "2001:db8::1"));
        assert!(!panel_ip_allowed("::/0", "8.8.8.8"));
    }

    #[test]
    fn lists_are_split_and_trimmed_and_any_entry_admits() {
        assert!(panel_ip_allowed(" 203.0.113.4 , 10.0.0.0/8 ", "10.1.1.1"));
        assert!(panel_ip_allowed("203.0.113.4,10.0.0.0/8", "203.0.113.4"));
        assert!(!panel_ip_allowed("203.0.113.4,10.0.0.0/8", "8.8.8.8"));
        // Empty entries are ignored, not treated as wildcards.
        assert!(!panel_ip_allowed(",,", "8.8.8.8"));
    }

    #[test]
    fn an_unusable_client_address_is_denied_not_admitted() {
        // No X-Real-IP from the proxy: an allowlist that cannot identify the
        // caller must fail CLOSED.
        assert!(!panel_ip_allowed("10.0.0.0/8", ""));
        assert!(!panel_ip_allowed("10.0.0.0/8", "not-an-ip"));
        assert!(!panel_ip_allowed("0.0.0.0/0", ""));
    }

    #[test]
    fn garbage_entries_admit_nobody() {
        assert!(!panel_ip_allowed("not-an-ip", "8.8.8.8"));
        assert!(!panel_ip_allowed("10.0.0.0/abc", "10.0.0.1"));
    }

    #[test]
    fn validation_accepts_what_the_matcher_understands() {
        for good in ["203.0.113.4", "10.0.0.0/8", "2001:db8::1", "2001:db8::/32", "0.0.0.0/0"] {
            assert!(valid_panel_ip_entry(good), "{good} should validate");
        }
        for bad in ["", "not-an-ip", "10.0.0.0/33", "2001:db8::/129", "10.0.0.0/", "10.0.0.0/-1"] {
            assert!(!valid_panel_ip_entry(bad), "{bad} should be rejected");
        }
    }
}

// ── Provisioning-log access control ─────────────────────────────────────
//
// `provision_logs` is one process-wide map shared by nine unrelated features:
// service installs, mail installs, system updates, site provisioning, backups
// and restores, migration imports, container deploys, git deploys and rollbacks.
// It is a single flat keyspace — nothing namespaces a site id apart from a
// deploy id — so any endpoint that looks a caller-supplied uuid up in it can
// reach every other feature's stream unless it proves ownership itself.
//
// Five endpoints did that five different ways, and the two weakest were open:
// the service-install stream looked the id up with no ownership test at all,
// and the git-deploy stream consulted the owner table but *fell through* when
// the id was absent — which was the common case, because only three of the
// sixteen sites that created a log ever recorded an owner for it. Together
// those two meant any signed-in account could read any other tenant's site
// provisioning stream, and that stream deliberately carries a generated CMS
// admin password in cleartext for the couple of minutes the install takes
// (`routes::sites`, the "credentials" step). The comment there justified
// emitting it on the grounds that the stream was owner-scoped. That was true
// of the endpoint its author was looking at and false of the siblings.
//
// So ownership stops being each caller's responsibility. A log cannot be
// created without an owner, and cannot be read except by that owner.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::Instant;

/// The shared provisioning-log map: id -> (history, live channel, created-at).
pub type ProvisionLogs = Arc<
    Mutex<
        HashMap<
            uuid::Uuid,
            (
                Vec<crate::routes::sites::ProvisionStep>,
                tokio::sync::broadcast::Sender<crate::routes::sites::ProvisionStep>,
                Instant,
            ),
        >,
    >,
>;

/// Owner table for the map above: id -> the user whose log it is.
pub type ProvisionOwners = Arc<Mutex<HashMap<uuid::Uuid, uuid::Uuid>>>;

/// Create a provisioning log and record who owns it.
///
/// Deliberately returns nothing. Every feature emits by locking the map and
/// taking `get_mut(&id)`, which is what lets the terminal `remove` actually
/// close the stream: the map holds the last sender, so dropping the entry ends
/// every receiver. Handing a `Sender` clone back would let a caller outlive the
/// entry and hold the channel open, and the SSE stream would hang instead of
/// finishing.
///
/// Both maps are taken under one lock scope, logs first. The cleanup task in
/// `main` drops owner rows whose log has been evicted (`owners.retain(|id, _|
/// map.contains_key(id))`) and takes the same two locks in the same order, so
/// it can neither interleave with a registration and strand a live log without
/// an owner — which the reader below would then refuse to serve to the very
/// user watching it — nor deadlock against one.
///
/// `capacity` is the broadcast channel depth; callers that emit fine-grained
/// output (apt line-by-line) need far more than callers that emit a dozen steps.
pub fn register_provision_log(
    logs: &ProvisionLogs,
    owners: &ProvisionOwners,
    id: uuid::Uuid,
    owner: uuid::Uuid,
    capacity: usize,
) {
    let (tx, _) = tokio::sync::broadcast::channel(capacity);
    let mut logs = logs.lock().unwrap_or_else(|e| e.into_inner());
    let mut owners = owners.lock().unwrap_or_else(|e| e.into_inner());
    logs.insert(id, (Vec::new(), tx, Instant::now()));
    owners.insert(id, owner);
}

/// Drop a provisioning log and its owner together.
///
/// For the callers that discard a log outright rather than letting their
/// terminal step retire it — deleting the migration a log belongs to, say.
/// Removing only the log would leave the owner row behind until the next sweep.
pub fn forget_provision_log(logs: &ProvisionLogs, owners: &ProvisionOwners, id: uuid::Uuid) {
    let mut logs = logs.lock().unwrap_or_else(|e| e.into_inner());
    let mut owners = owners.lock().unwrap_or_else(|e| e.into_inner());
    logs.remove(&id);
    owners.remove(&id);
}

/// Resolve a provisioning log for a caller, or refuse.
///
/// Returns the history so far plus a receiver for everything still to come.
///
/// An id with no owner recorded is refused. That is the whole point: a future
/// feature that adds a log without going through `register_provision_log` gets
/// a stream nobody can read, which is a bug its author will notice on the first
/// run — the previous arrangement gave them a stream *everybody* could read,
/// which nobody would notice at all.
///
/// "Not yours" and "no such log" deliberately return the same 404. Separating
/// them would turn the endpoint into an oracle for which uuids are live jobs.
pub fn open_provision_log(
    logs: &ProvisionLogs,
    owners: &ProvisionOwners,
    id: uuid::Uuid,
    caller: uuid::Uuid,
    missing: &str,
) -> Result<
    (
        Vec<crate::routes::sites::ProvisionStep>,
        tokio::sync::broadcast::Receiver<crate::routes::sites::ProvisionStep>,
    ),
    crate::error::ApiError,
> {
    let logs = logs.lock().unwrap_or_else(|e| e.into_inner());
    let owners = owners.lock().unwrap_or_else(|e| e.into_inner());

    let owned = owners.get(&id).copied() == Some(caller);
    match logs.get(&id) {
        Some((history, tx, _)) if owned => Ok((history.clone(), tx.subscribe())),
        _ => Err(crate::error::err(
            axum::http::StatusCode::NOT_FOUND,
            missing,
        )),
    }
}

/// Refuse, at the mint, a stream whose transport can only ever reach the panel's
/// own agent — when the caller has a different server selected.
///
/// The terminal and log-stream tickets are signed with the SELECTED server's
/// agent token, because both handlers take `ServerScope`. The browser then dials
/// `window.location.host`, and nginx pins that path to the LOCAL agent socket, so
/// the receiving agent verifies a ticket signed with somebody else's key and
/// answers 401. The result was a promised capability that failed for every fleet
/// member, and failed dishonestly: the terminal reported "Connection lost" and the
/// log viewer retried every three seconds for ever, both describing a network
/// problem that did not exist.
///
/// The knowledge to say so was already here. `ServerScope` resolves the id and
/// both handlers bound it and threw it away. Comparing it against the local
/// server answers before a socket is opened.
///
/// This deliberately does NOT proxy the stream. A backend WebSocket proxy would
/// deliver a member-signed ticket to the member's own agent — which would honour
/// it, including its `domain` query parameter, which the ticket does not bind and
/// which an empty value turns into a root login shell. Today two independent
/// things confine that to the panel host. Removing both, before the domain is
/// bound into the signed ticket, would widen it to every host in the fleet.
pub async fn require_local_agent_scope(
    state: &crate::AppState,
    scoped_server_id: uuid::Uuid,
    feature: &str,
) -> Result<(), crate::error::ApiError> {
    let local_id = state.agents.local_server_id().await;

    // No local row registered yet: `ServerScope` would already have rejected the
    // request, so there is nothing left to disagree with.
    if local_id == Some(scoped_server_id) || local_id.is_none() {
        return Ok(());
    }

    let name: Option<(String,)> = sqlx::query_as("SELECT name FROM servers WHERE id = $1")
        .bind(scoped_server_id)
        .fetch_optional(&state.db)
        .await
        .ok()
        .flatten();

    let which = name
        .map(|r| r.0)
        .unwrap_or_else(|| scoped_server_id.to_string());

    Err(crate::error::err(
        axum::http::StatusCode::NOT_IMPLEMENTED,
        &format!(
            "{feature} runs on the panel host only. \"{which}\" is a fleet member, \
             and the panel cannot yet stream to one. Switch back to the panel \
             server to use it."
        ),
    ))
}

#[cfg(test)]
mod provision_log_tests {
    use super::*;

    fn maps() -> (ProvisionLogs, ProvisionOwners) {
        (
            Arc::new(Mutex::new(HashMap::new())),
            Arc::new(Mutex::new(HashMap::new())),
        )
    }

    #[test]
    fn owner_can_read_its_own_log() {
        let (logs, owners) = maps();
        let id = uuid::Uuid::new_v4();
        let owner = uuid::Uuid::new_v4();
        register_provision_log(&logs, &owners, id, owner, 8);
        assert!(open_provision_log(&logs, &owners, id, owner, "gone").is_ok());
    }

    #[test]
    fn a_stranger_cannot_read_someone_elses_log() {
        let (logs, owners) = maps();
        let id = uuid::Uuid::new_v4();
        let owner = uuid::Uuid::new_v4();
        register_provision_log(&logs, &owners, id, owner, 8);
        let stranger = uuid::Uuid::new_v4();
        assert!(open_provision_log(&logs, &owners, id, stranger, "gone").is_err());
    }

    // The regression this module exists for: a log present in the map with no
    // owner recorded beside it. The old git-deploy stream fell through such a
    // gap to the log itself; thirteen of the sixteen creation sites left one.
    #[test]
    fn an_ownerless_log_is_refused_not_served() {
        let (logs, owners) = maps();
        let id = uuid::Uuid::new_v4();
        let (tx, _) = tokio::sync::broadcast::channel(8);
        logs.lock()
            .unwrap()
            .insert(id, (Vec::new(), tx, Instant::now()));

        assert!(
            open_provision_log(&logs, &owners, id, uuid::Uuid::new_v4(), "gone").is_err(),
            "a log with no recorded owner must not be readable"
        );
    }

    #[test]
    fn registration_records_the_owner() {
        let (logs, owners) = maps();
        let id = uuid::Uuid::new_v4();
        let owner = uuid::Uuid::new_v4();
        register_provision_log(&logs, &owners, id, owner, 8);
        assert_eq!(owners.lock().unwrap().get(&id).copied(), Some(owner));
        assert!(logs.lock().unwrap().contains_key(&id));
    }

    // Absent and forbidden must be indistinguishable, or the endpoint tells a
    // stranger which uuids are live jobs.
    #[test]
    fn missing_and_forbidden_are_indistinguishable() {
        let (logs, owners) = maps();
        let live = uuid::Uuid::new_v4();
        register_provision_log(&logs, &owners, live, uuid::Uuid::new_v4(), 8);

        let stranger = uuid::Uuid::new_v4();
        let forbidden = open_provision_log(&logs, &owners, live, stranger, "gone").unwrap_err();
        let absent =
            open_provision_log(&logs, &owners, uuid::Uuid::new_v4(), stranger, "gone").unwrap_err();

        // Status *and* body — a differing sentence is as good an oracle as a
        // differing code.
        assert_eq!(forbidden.0, absent.0);
        assert_eq!(*forbidden.1, *absent.1);
    }

    // Emitting the way every feature actually emits — lock the map, `get_mut`,
    // push and send — must reach a reader that has already attached. This is
    // the real path; a test against a sender handed back by `register` would
    // pin a contract no caller uses.
    #[test]
    fn a_step_emitted_through_the_map_reaches_an_attached_reader() {
        let (logs, owners) = maps();
        let id = uuid::Uuid::new_v4();
        let owner = uuid::Uuid::new_v4();
        register_provision_log(&logs, &owners, id, owner, 8);

        let (_history, mut rx) = open_provision_log(&logs, &owners, id, owner, "gone").unwrap();

        {
            let mut map = logs.lock().unwrap();
            let (history, tx, _) = map.get_mut(&id).unwrap();
            let ev = crate::routes::sites::ProvisionStep {
                step: "s".into(),
                label: "l".into(),
                status: "done".into(),
                message: None,
            };
            history.push(ev.clone());
            tx.send(ev).unwrap();
        }

        assert_eq!(rx.try_recv().unwrap().step, "s");
    }

    // Removing the entry must end the stream. That is what the terminal
    // `remove` in every feature relies on, and it only holds while the map owns
    // the last sender.
    #[test]
    fn removing_the_entry_closes_the_reader() {
        let (logs, owners) = maps();
        let id = uuid::Uuid::new_v4();
        let owner = uuid::Uuid::new_v4();
        register_provision_log(&logs, &owners, id, owner, 8);

        let (_history, mut rx) = open_provision_log(&logs, &owners, id, owner, "gone").unwrap();
        logs.lock().unwrap().remove(&id);

        assert!(
            matches!(
                rx.try_recv(),
                Err(tokio::sync::broadcast::error::TryRecvError::Closed)
            ),
            "dropping the map entry must close the receiver, or the SSE stream hangs"
        );
    }
}
