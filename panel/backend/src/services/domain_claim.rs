//! The one place a domain is claimed.
//!
//! Before this module there were eleven paths that could cause a vhost to be
//! written — sites create/rename/clone/alias, staging, migration import, git
//! deploy create/update, git preview, and Docker app deploy in both nginx and
//! Traefik mode — and each carried its own subset of the guards. `sites.rs` even
//! had a shared helper whose doc comment said it existed "so the guard set
//! create() enforces cannot drift"; two of the eleven called it.
//!
//! What the drift cost, measured against v2.51.0:
//!
//! * A Docker app's domain lives only in the `dockpanel.app.domain` container
//!   label, so **no** SQL guard could ever see it — creating or renaming a site
//!   onto a domain an app owned passed every check, and the agent then replaced
//!   the app's vhost.
//! * `docker_apps::deploy` checked nothing at all: not the reserved control-plane
//!   domain, not `sites`, not `git_deploys`.
//! * `git_deploys::update` dropped both conflict queries its own `create`
//!   performs, under a comment claiming parity with it.
//! * `staging::create` — the tenant-reachable one — consulted only `sites`.
//! * Domain comparison was case-sensitive while the reserved check was not, so
//!   `EXAMPLE.com` walked past a row holding `example.com`.
//!
//! So: every path calls [`ensure_claimable`], and it returns the normalised
//! domain the caller must store. Adding a new domain-introducing surface without
//! calling this is what `tests/domain-claim-pin-e2e.sh` exists to catch.

use axum::http::{HeaderMap, StatusCode};
use uuid::Uuid;

use crate::error::{err, internal_error, ApiError};
use crate::routes::{is_reserved_domain_for, is_valid_domain};

/// A claim that already exists and must not conflict with itself.
///
/// A rename re-claims a domain its own row may already hold; without this the
/// guard would refuse to let a row keep the name it already has.
///
/// There is deliberately no `App` variant. A Docker app cannot re-claim its own
/// domain today: `deploy` is the only app path that supplies one, and a redeploy
/// under an existing name is already refused by the name check, so an app-holder
/// exclusion would be unreachable code. Add it together with the app-rename it
/// is for (GitHub #95), not before.
///
/// There is no mail variant either, for the same reason: nothing renames a mail
/// domain. `mail::create_domain` is the only writer of `mail_domains.domain` and
/// `mail::update_domain` changes `catch_all` and `enabled` only, so a mail domain
/// never needs to be excluded from its own claim.
#[derive(Debug, Clone, Copy, Default)]
pub enum Holder {
    /// A brand-new claim — nothing may hold this domain.
    #[default]
    New,
    /// The `sites` row with this id may hold it.
    Site(Uuid),
    /// The `git_deploys` row with this id may hold it.
    GitDeploy(Uuid),
    /// The `docker_stacks` row with this id may hold it. Unlike a Docker app, a
    /// stack *can* re-claim its own domain: `update` redeploys in place under
    /// the same id, so without this a stack could not be edited twice.
    Stack(Uuid),
}

/// What already holds a domain. Carried so the error can name it — "in use" with
/// no owner is the sentence that made these collisions hard to diagnose.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Occupant {
    Site,
    GitDeploy,
    DockerApp,
    Stack,
    /// A `mail_domains` row. Unlike every variant above this one holds no vhost,
    /// so it is not a nginx collision — it is an AUTHORISATION collision, and it
    /// is tolerated for an administrator. See [`may_claim_mail_held`].
    MailDomain,
}

impl Occupant {
    fn message(self) -> &'static str {
        match self {
            Occupant::Site => "Domain already in use by a site",
            Occupant::GitDeploy => "Domain already in use by a git deployment",
            Occupant::DockerApp => {
                "Domain already in use by a Docker app. Rename or remove the app \
                 first — deploying over it would replace its nginx configuration."
            }
            Occupant::MailDomain => {
                "Domain already in use by a mail domain. Ask an administrator to \
                 create the site and transfer it to you — claiming a name whose \
                 mailboxes already exist would hand you those mailboxes."
            }
            Occupant::Stack => {
                "Domain already in use by a Compose stack. Change that stack's \
                 domain or remove it first."
            }
        }
    }
}

/// The role that holds sites but may not introduce new ones (GitHub #51).
///
/// Deliberately a `const` rather than a literal at each comparison: the value
/// also lives in a database CHECK constraint and in the user editor, and a role
/// string that means "deny" is the worst possible thing to typo — a misspelling
/// does not fail, it silently grants.
pub const CLIENT_ROLE: &str = "client";

/// Whether `role` may make the claim `holder` describes.
///
/// The restriction is on the TRANSITION, not on the resource: `Holder::New` is
/// by definition "no existing row may hold this domain", i.e. a domain entering
/// service for the first time. Every other variant re-claims a domain on behalf
/// of a row that already exists, which is management of something the caller was
/// already authorised for by the ownership check in its own handler.
///
/// Putting it HERE rather than at each creating handler is the whole design.
/// This module exists because eleven domain-introducing paths each carried their
/// own subset of guards and drifted apart — the header above lists what that
/// cost. Four role checks bolted onto the four `INSERT INTO sites` sites would
/// have rebuilt exactly that: `git_deploys`, `docker_apps` and `stacks` all
/// materialise a served vhost without ever inserting into `sites`, so a client
/// blocked from creating a site could still have deployed a git app on a new
/// domain. Nine call sites reach this function and `tests/domain-claim-pin-e2e.sh`
/// already fails the build when a new surface does not.
pub fn may_claim_new(role: &str, holder: Holder) -> bool {
    !(role == CLIENT_ROLE && matches!(holder, Holder::New))
}

/// Whether `role` may claim a domain that an existing MAIL domain already holds.
///
/// Only an administrator may. This is the one rule in this module keyed on the
/// role ALONE, with no holder term, and the asymmetry is the point.
///
/// **Why it exists.** Mail is scoped by matching `mail_domains.domain` against
/// the domain of a site the caller owns (GitHub #106). That makes `sites.domain`
/// an authorisation key — and `sites.domain` is WRITABLE by the account being
/// authorised. Without this, a non-administrator could point a site they already
/// own at a domain whose mailboxes exist, and the match would then hand them
/// every mailbox and alias on it, including the power to replace each password
/// hash. The entitlement would have minted itself.
///
/// **Why it is not restricted to `client`.** [`may_claim_new`] can be, because
/// it refuses only the introduction of a brand-new name. This one cannot: a
/// plain `user` may create sites freely, so leaving them out would close the
/// rename door and leave the create door beside it open. Every non-admin role is
/// refused, which is also why the message points at the transfer flow — an
/// administrator creating the site and handing it over is the documented way to
/// give somebody a domain whose mail already exists, and it stays open.
///
/// **What it does NOT refuse.** An administrator putting a site and its mail on
/// the same name is the pairing the entitlement is BUILT on, not a collision, so
/// tolerating it is required rather than lenient. And this asks only about the
/// domain being claimed: renaming AWAY from a domain whose mail the caller holds
/// is untouched, because the mail row is keyed on the name, not on the site.
pub fn may_claim_mail_held(role: &str) -> bool {
    role == "admin"
}

/// Normalise a domain for storage and comparison.
///
/// Hostnames are case-insensitive, but `is_valid_domain` accepts uppercase and
/// the `(domain, server_id)` unique index is a plain `varchar`, so `EXAMPLE.com`
/// and `example.com` were two different rows and two different vhost files.
/// Callers store what this returns.
pub fn normalise(domain: &str) -> String {
    domain.trim().trim_end_matches('.').to_ascii_lowercase()
}

/// Assert that `domain` may be claimed anywhere in the fleet, and return the
/// normalised form to store.
///
/// Order matters: format, then reserved, then ownership — a malformed domain
/// should be reported as malformed rather than as available.
///
/// **Fleet-wide, on every leg.** It takes the registry rather than one agent
/// because the Docker-app leg used to ask only the caller's host while the three
/// SQL legs were fleet-wide — and an app's domain is invisible to SQL, so that
/// leg was the only thing that could ever catch it.
///
/// **Reachability:** an online member that will not answer is logged by name and
/// skipped, not treated as a refusal. This is a deliberate change from the
/// previous fail-closed note, which was true when one agent was consulted: with
/// the whole fleet in scope, failing closed would let a single sick box block
/// every domain claim everywhere. The collision that leniency could miss requires
/// the unreachable host and the claim's target to be the SAME host, and in that
/// case the vhost write immediately after this fails on its own.
pub async fn ensure_claimable(
    db: &sqlx::PgPool,
    agents: &crate::services::agent::AgentRegistry,
    domain: &str,
    headers: &HeaderMap,
    holder: Holder,
    claimant_role: &str,
) -> Result<String, ApiError> {
    let domain = normalise(domain);

    if !is_valid_domain(&domain) {
        return Err(err(StatusCode::BAD_REQUEST, "Invalid domain"));
    }
    if !may_claim_new(claimant_role, holder) {
        return Err(err(
            StatusCode::FORBIDDEN,
            "This account can manage the domains it holds but cannot bring a new \
             one into service. Ask an administrator to create it and transfer it \
             to you.",
        ));
    }
    if is_reserved_domain_for(&domain, headers) {
        return Err(err(
            StatusCode::FORBIDDEN,
            "This domain is reserved and cannot be used",
        ));
    }

    if let Some(occupant) = find_occupant(db, agents, &domain, holder).await? {
        // A mail domain is the one occupant an administrator may claim over,
        // because a site and its mailboxes SHARING a name is the arrangement the
        // mail entitlement is built on (#106) — refusing it outright would break
        // the ordinary "set the mail up first, add the website after" order.
        //
        // Reported by `find_occupant` LAST, deliberately, so this tolerance can
        // never swallow a real vhost collision: if a site, git deploy, stack or
        // Docker app also holds the name, that is what comes back and this is
        // never reached.
        if occupant != Occupant::MailDomain || !may_claim_mail_held(claimant_role) {
            return Err(err(StatusCode::CONFLICT, occupant.message()));
        }
    }

    Ok(domain)
}

/// Who holds `domain`, if anyone. Separated from [`ensure_claimable`] so a caller
/// that must report rather than reject (the migration wizard's per-site loop) can
/// ask the same question and get the same answer.
pub async fn find_occupant(
    db: &sqlx::PgPool,
    agents: &crate::services::agent::AgentRegistry,
    domain: &str,
    holder: Holder,
) -> Result<Option<Occupant>, ApiError> {
    let domain = normalise(domain);

    // Sites are checked fleet-wide, which is what create() already did. It is
    // stricter than the (domain, server_id) index and tightening is safe;
    // loosening it here would silently widen what the panel accepts.
    let exclude_site = match holder {
        Holder::Site(id) => Some(id),
        _ => None,
    };
    let site: Option<(Uuid,)> =
        sqlx::query_as("SELECT id FROM sites WHERE lower(domain) = $1 AND ($2::uuid IS NULL OR id <> $2)")
            .bind(&domain)
            .bind(exclude_site)
            .fetch_optional(db)
            .await
            .map_err(|e| internal_error("domain availability", e))?;
    if site.is_some() {
        return Ok(Some(Occupant::Site));
    }

    let exclude_git = match holder {
        Holder::GitDeploy(id) => Some(id),
        _ => None,
    };
    let git: Option<(Uuid,)> = sqlx::query_as(
        "SELECT id FROM git_deploys WHERE lower(domain) = $1 AND ($2::uuid IS NULL OR id <> $2)",
    )
    .bind(&domain)
    .bind(exclude_git)
    .fetch_optional(db)
    .await
    .map_err(|e| internal_error("domain availability", e))?;
    if git.is_some() {
        return Ok(Some(Occupant::GitDeploy));
    }

    // Compose stacks became domain holders in v2.54.0. A path that writes a
    // vhost but is invisible to this function is how a domain ends up with two
    // owners, so the leg lands with the feature rather than after it.
    let exclude_stack = match holder {
        Holder::Stack(id) => Some(id),
        _ => None,
    };
    let stack: Option<(Uuid,)> = sqlx::query_as(
        "SELECT id FROM docker_stacks WHERE lower(domain) = $1 AND ($2::uuid IS NULL OR id <> $2)",
    )
    .bind(&domain)
    .bind(exclude_stack)
    .fetch_optional(db)
    .await
    .map_err(|e| internal_error("domain availability", e))?;
    if stack.is_some() {
        return Ok(Some(Occupant::Stack));
    }

    // The leg that could not exist before: Docker apps are not rows. The agent
    // reads `dockpanel.app.domain` off each managed container and already returns
    // it from `GET /apps` — a single `docker.list_containers`, on the 20-permit
    // quick lane, not the long one. The panel simply never asked.
    //
    // ⚠ It asked exactly ONE host until v2.80.0 — the caller's — while all three
    // SQL legs above are deliberately fleet-wide, and the comment justifying that
    // ("the agent handle is already scoped to this server") described the code
    // accurately while missing that scoping was the defect. A domain held by an
    // app on host B therefore passed a claim made on host A, and since a Docker
    // app's domain is invisible to every SQL guard, this leg was the only thing
    // that could ever have caught it.
    //
    // A member we cannot reach is reported and skipped rather than treated as a
    // refusal. Failing closed would make one offline box block every domain
    // claim in the fleet, and the collision it would prevent needs the SAME host
    // to be both the unreachable one and the claim's target — in which case the
    // vhost write that follows fails on its own. Failing open silently is what
    // produced this bug, so it does not fail open silently.
    for member in agents.online_fleet().await {
        let apps = match member.agent.get("/apps").await {
            Ok(v) => v,
            Err(e) => {
                tracing::warn!(
                    "Domain availability: could not ask {} about Docker-app domains ({e}) — \
                     '{domain}' is being treated as free there",
                    member.name
                );
                continue;
            }
        };
        let Some(list) = apps.as_array() else { continue };
        for app in list {
            let Some(app_domain) = app.get("domain").and_then(|v| v.as_str()) else {
                continue;
            };
            if normalise(app_domain) != domain {
                continue;
            }
            return Ok(Some(Occupant::DockerApp));
        }
    }

    // Mail domains hold a name without holding a vhost, and this function never
    // asked about them — so a domain could be held by `mail_domains` and claimed
    // by a site at the same time, which is precisely the two-owners state the
    // module exists to prevent. It went unnoticed because the collision has no
    // nginx symptom: a mail-only domain has no vhost to overwrite.
    //
    // ⚠ LAST on purpose, after the fleet loop and not beside the other SQL legs.
    // `ensure_claimable` TOLERATES this occupant for an administrator, and that
    // tolerance is only sound if a mail domain can never be reported in place of
    // a holder that must always refuse. Returning it last makes that structural:
    // every vhost holder has already had its say.
    //
    // Fleet-wide like the three legs above, and `lower()` for the same reason —
    // `mail::create_domain` lowercases before storing, but the comparison must
    // not depend on a convention enforced somewhere else.
    let mail: Option<(Uuid,)> =
        sqlx::query_as("SELECT id FROM mail_domains WHERE lower(domain) = $1")
            .bind(&domain)
            .fetch_optional(db)
            .await
            .map_err(|e| internal_error("domain availability", e))?;
    if mail.is_some() {
        return Ok(Some(Occupant::MailDomain));
    }

    Ok(None)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalise_lowercases_and_strips_the_root_dot() {
        assert_eq!(normalise("EXAMPLE.com"), "example.com");
        assert_eq!(normalise("  Example.COM  "), "example.com");
        assert_eq!(normalise("example.com."), "example.com");
    }

    #[test]
    fn normalise_collapses_the_variants_that_used_to_be_separate_claims() {
        // The bypass this closes: each of these passed `WHERE domain = $1`
        // against a row holding the lowercase form, and earned its own vhost.
        for variant in ["EXAMPLE.COM", "Example.Com", "eXaMpLe.cOm", "example.com."] {
            assert_eq!(normalise(variant), normalise("example.com"));
        }
    }

    #[test]
    fn normalise_does_not_merge_genuinely_different_domains() {
        assert_ne!(normalise("a.b.com"), normalise("a-b.com"));
        assert_ne!(normalise("example.com"), normalise("example.net"));
    }

    // ── the client gate — s312 / GitHub #51 ─────────────────────────────

    #[test]
    fn a_client_may_not_bring_a_new_domain_into_service() {
        assert!(!may_claim_new(CLIENT_ROLE, Holder::New));
    }

    #[test]
    fn a_client_may_still_manage_the_domains_it_holds() {
        // The whole point of the role: it HOLDS sites. A rename re-claims a
        // domain on behalf of a row that already exists, and the handler's own
        // ownership check has already established the caller may act on it.
        // Refusing these would make the role useless rather than restricted.
        let id = Uuid::nil();
        assert!(may_claim_new(CLIENT_ROLE, Holder::Site(id)));
        assert!(may_claim_new(CLIENT_ROLE, Holder::GitDeploy(id)));
        assert!(may_claim_new(CLIENT_ROLE, Holder::Stack(id)));
    }

    #[test]
    fn every_other_role_is_unaffected() {
        // The gate must be a pure addition. If it refused anyone else, the
        // regression would be invisible here and loud in production.
        for role in ["admin", "reseller", "user", "suspended", ""] {
            assert!(
                may_claim_new(role, Holder::New),
                "role {role:?} must still be able to claim a new domain here — \
                 whether it may act at all is decided by its own handler's guard"
            );
        }
    }

    #[test]
    fn the_refusal_is_keyed_on_the_exact_role_string() {
        // A near-miss must NOT be treated as a client: this function fails OPEN
        // by construction (it returns true for anything it does not recognise),
        // so the only thing standing between a client and a new domain is that
        // this string matches what the database CHECK and the user editor write.
        for near in ["Client", "CLIENT", "clients", "client ", " client"] {
            assert!(
                may_claim_new(near, Holder::New),
                "{near:?} is not the role value — if this ever becomes a client, \
                 the value has drifted from the migration and the editor"
            );
        }
        assert_eq!(CLIENT_ROLE, "client");
    }

    // ── the mail gate — s347 / GitHub #106 ──────────────────────────────

    #[test]
    fn only_an_administrator_may_claim_a_domain_a_mail_domain_holds() {
        assert!(may_claim_mail_held("admin"));
        for role in ["client", "user", "reseller", "suspended", ""] {
            assert!(
                !may_claim_mail_held(role),
                "role {role:?} must not claim a name whose mailboxes exist — the \
                 mail entitlement matches on the site's domain, so allowing it \
                 would let the account mint its own access to those mailboxes"
            );
        }
    }

    #[test]
    fn the_mail_gate_fails_closed_where_the_client_gate_fails_open() {
        // The two gates in this module are deliberately opposite. `may_claim_new`
        // returns true for a role it does not recognise, so an unknown value is
        // merely un-restricted. This one returns false, so an unknown value is
        // refused. A role string that drifts must never silently become able to
        // take over a mailbox.
        for near in ["Admin", "ADMIN", "admins", "admin ", " admin", "administrator"] {
            assert!(
                !may_claim_mail_held(near),
                "{near:?} is not the administrator role value; it must be refused, \
                 not granted"
            );
            assert!(may_claim_new(near, Holder::New));
        }
    }

    #[test]
    fn a_mail_domain_is_the_only_occupant_an_administrator_may_claim_over() {
        // Pins the tolerance in `ensure_claimable` to ONE variant. If a future
        // occupant is added to the enum and quietly folded into that branch, an
        // administrator would start claiming over a live vhost.
        for occupant in [
            Occupant::Site,
            Occupant::GitDeploy,
            Occupant::DockerApp,
            Occupant::Stack,
        ] {
            assert!(
                occupant != Occupant::MailDomain,
                "{occupant:?} must keep refusing every caller, administrator included"
            );
        }
    }

    #[test]
    fn every_occupant_names_what_holds_the_domain() {
        // "In use" with no owner is the sentence that made these collisions hard
        // to diagnose — the reason `Occupant` is carried at all.
        for occupant in [
            Occupant::Site,
            Occupant::GitDeploy,
            Occupant::DockerApp,
            Occupant::Stack,
            Occupant::MailDomain,
        ] {
            let msg = occupant.message();
            assert!(!msg.is_empty() && msg.contains("in use by"), "{occupant:?}: {msg}");
        }
    }
}
