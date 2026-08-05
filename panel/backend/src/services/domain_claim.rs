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

use crate::error::{agent_error, err, internal_error, ApiError};
use crate::routes::{is_reserved_domain_for, is_valid_domain};
use crate::services::agent::AgentHandle;

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
}

impl Occupant {
    fn message(self) -> &'static str {
        match self {
            Occupant::Site => "Domain already in use by a site",
            Occupant::GitDeploy => "Domain already in use by a git deployment",
            Occupant::DockerApp => {
                "Domain already in use by a Docker app on this server. Rename or \
                 remove the app first — deploying over it would replace its nginx \
                 configuration."
            }
            Occupant::Stack => {
                "Domain already in use by a Compose stack on this server. Change \
                 that stack's domain or remove it first."
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

/// Normalise a domain for storage and comparison.
///
/// Hostnames are case-insensitive, but `is_valid_domain` accepts uppercase and
/// the `(domain, server_id)` unique index is a plain `varchar`, so `EXAMPLE.com`
/// and `example.com` were two different rows and two different vhost files.
/// Callers store what this returns.
pub fn normalise(domain: &str) -> String {
    domain.trim().trim_end_matches('.').to_ascii_lowercase()
}

/// Assert that `domain` may be claimed on `server_id`, and return the normalised
/// form to store.
///
/// Order matters: format, then reserved, then ownership — a malformed domain
/// should be reported as malformed rather than as available.
///
/// **Fails closed.** The Docker-app leg needs the agent, and if the agent cannot
/// answer this returns the agent's own 502 rather than allowing the claim. That
/// is not a new fragility: every one of these callers goes on to ask the same
/// agent to write the vhost, so an unreachable agent already failed the request —
/// it just used to fail it *after* taking the domain.
pub async fn ensure_claimable(
    db: &sqlx::PgPool,
    agent: &AgentHandle,
    server_id: Uuid,
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

    if let Some(occupant) = find_occupant(db, agent, server_id, &domain, holder).await? {
        return Err(err(StatusCode::CONFLICT, occupant.message()));
    }

    Ok(domain)
}

/// Who holds `domain`, if anyone. Separated from [`ensure_claimable`] so a caller
/// that must report rather than reject (the migration wizard's per-site loop) can
/// ask the same question and get the same answer.
pub async fn find_occupant(
    db: &sqlx::PgPool,
    agent: &AgentHandle,
    server_id: Uuid,
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
    let apps = agent
        .get("/apps")
        .await
        .map_err(|e| agent_error("Domain availability check", e))?;
    if let Some(list) = apps.as_array() {
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
    let _ = server_id; // the agent handle is already scoped to this server

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
}
