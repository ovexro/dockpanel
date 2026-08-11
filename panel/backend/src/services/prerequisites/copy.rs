//! The one registry of guidance copy.
//!
//! Every sentence the guidance layer says to a user lives here, as data, in one
//! file. The checks in the sibling modules decide *which* outcome a situation is;
//! they no longer decide what it says.
//!
//! # Why this exists
//!
//! Through v2.29.0 the copy lived inside each check as Rust format strings. That
//! was honest with one vertical and stopped being honest with four: the same
//! facts were also written by hand into `docs/`, and the two drifted, silently,
//! in the direction that matters least to whoever is editing at the time. The
//! audit that opened v2.30.0 found the manual telling operators to publish
//! `default._domainkey` while the product has deployed the selector `dockpanel`
//! for its entire life — a user who followed the documentation got a DKIM record
//! that could never verify, and a panel that then correctly reported it missing.
//!
//! So the rule is not "keep them in sync". The rule is **there is one copy of
//! each fact, and the manual is generated from it** — see [`manual`]. A
//! hand-written page cannot contradict the product if it does not contain the
//! product's facts.
//!
//! # The three tiers are one system
//!
//! Brief §3.9 asks for tooltips and callouts to be the same content rendered at
//! different urgencies. That is why [`FIELDS`] (the passive tier — the line under
//! a form field, and the longer text behind its (i)) sits in this file beside
//! [`PREREQS`] (the reactive and blocking tiers). A field's quiet explanation and
//! the loud callout it escalates into are two renderings of one concern, and
//! [`Field::escalates_to`] names the link between them.
//!
//! # Editing
//!
//! Change a sentence here and `cargo test` fails until the manual is regenerated:
//!
//! ```text
//! UPDATE_GUIDANCE_DOCS=1 cargo test -p dockpanel-api guidance
//! ```
//!
//! That failure is the mechanism. It is not a reminder to keep docs updated — it
//! is the reason they cannot be out of date.

use super::{PrereqResult, PrereqSeverity, PrereqState};

// ─────────────────────────────────────────────────────────────────────────────
// Shapes
// ─────────────────────────────────────────────────────────────────────────────

/// A value a piece of copy interpolates, with the example the manual renders it
/// with.
///
/// The example is not decoration: it is what makes the generated page readable
/// (`example.com doesn't resolve yet` rather than `{domain} doesn't resolve yet`)
/// and it is what the round-trip test substitutes to prove no placeholder was
/// left unfilled.
pub struct Var {
    pub name: &'static str,
    pub example: &'static str,
}

/// One situation a check can conclude, and what the user is told about it.
pub struct Outcome {
    /// Stable id, unique within its check. Greppable; the manual anchors on it.
    pub id: &'static str,
    pub state: PrereqState,
    pub severity: PrereqSeverity,
    /// The one-line title. May interpolate any of [`Outcome::vars`].
    pub title: &'static str,
    /// Used instead of [`Outcome::title`] when the `count` var is anything but
    /// `1`. Empty means `title` is always used.
    ///
    /// English plurals are the one piece of grammar a copy registry cannot avoid;
    /// the alternative is a near-duplicate outcome per number, which buries the
    /// distinctions that actually matter.
    pub title_plural: &'static str,
    /// The explanation, in the user's terms.
    pub detail: &'static str,
    /// Doc-facing: what the operator does about it. Empty for outcomes that need
    /// nothing done — the manual renders those as "nothing to do".
    pub fix: &'static str,
    pub vars: &'static [Var],
}

impl Outcome {
    /// True when a control gated on this outcome would be disabled. Mirrors
    /// [`PrereqResult::blocks`], for the manual and for tests.
    pub fn blocks(&self) -> bool {
        matches!(self.state, PrereqState::Unsatisfied)
            && matches!(self.severity, PrereqSeverity::Blocking)
    }
}

/// One prerequisite: everything true of it regardless of which outcome it reaches.
pub struct Prereq {
    /// The stable key the API and the frontend already use, e.g. `dns.points_here`.
    pub key: &'static str,
    /// The vertical this belongs to, as a user would name it.
    pub vertical: &'static str,
    /// The question the check answers, in one line.
    pub question: &'static str,
    /// Why an operator should care — the consequence, not the mechanism.
    pub why: &'static str,
    /// Why this check blocks, warns or merely explains. Recorded because severity
    /// here is chosen by consequence rather than by confidence, and that choice is
    /// the part most likely to be re-litigated by whoever edits next.
    pub severity_policy: &'static str,
    /// Where in the product this is evaluated and shown.
    pub surfaces: &'static [&'static str],
    pub outcomes: &'static [&'static Outcome],
}

impl Prereq {
    /// Build the runtime result for one outcome, filling its placeholders.
    ///
    /// The only way a check produces a `PrereqResult`. `expected`, `observed` and
    /// `remediation` are attached by the caller afterwards — those are measured
    /// data, not copy, and do not belong in this file.
    pub fn result(&self, outcome: &Outcome, vars: &[(&str, &str)]) -> PrereqResult {
        debug_assert!(
            self.outcomes.iter().any(|o| o.id == outcome.id),
            "outcome `{}` is not listed on `{}`, so the manual will never document it",
            outcome.id,
            self.key
        );
        debug_assert!(
            outcome.vars.iter().all(|v| vars.iter().any(|(n, _)| *n == v.name)),
            "`{}`/`{}` declares vars the caller did not supply",
            self.key,
            outcome.id
        );

        let plural = !outcome.title_plural.is_empty()
            && vars
                .iter()
                .find(|(n, _)| *n == "count")
                .is_some_and(|(_, v)| *v != "1");

        let template = if plural { outcome.title_plural } else { outcome.title };

        PrereqResult {
            key: self.key.to_string(),
            state: outcome.state,
            severity: outcome.severity,
            title: fill(template, vars),
            detail: fill(outcome.detail, vars),
            expected: None,
            observed: Vec::new(),
            remediation: None,
        }
    }
}

/// A form field's passive guidance — the quiet tier.
pub struct Field {
    /// Stable id, e.g. `sites.create.domain`. The frontend keys off it.
    pub id: &'static str,
    /// The surface it appears on, as a user would name it.
    pub surface: &'static str,
    /// The field's label, spelled exactly as the screen spells it. A registry
    /// that calls a field something the UI doesn't is the same drift in
    /// miniature.
    pub label: &'static str,
    /// The line under the field. Must stay short.
    ///
    /// Empty means this entry exists for its (i) alone — the standing line is
    /// computed from the form's own state (which runtime is selected, which
    /// domain is open) and belongs in the component. Static copy that pretends
    /// to be dynamic is worse than no entry.
    pub help: &'static str,
    /// The longer explanation behind the (i). Empty means the field has no
    /// tooltip — most don't, and inventing one for every field is how a help
    /// system becomes noise.
    pub more: &'static str,
    /// The prerequisite key this field's guidance escalates into when the quiet
    /// tier stops being enough. Empty when nothing checks this field.
    ///
    /// This is the link that makes the three tiers one system rather than three
    /// that happen to sit near each other.
    pub escalates_to: &'static str,
}

/// Replace every `{name}` with its value.
///
/// Deliberately not a templating engine: a registry of product copy that needs
/// conditionals or loops has stopped being a registry.
/// Single-pass, and that matters — a sequence of `str::replace` calls re-reads
/// its own output, so a value containing `{something}` would be substituted
/// again by a later variable. Several of these values are named by the operator
/// (a backup policy, a container), and copy that rewrites what the user typed is
/// a small lie of exactly the kind this file exists to stop.
///
/// An unknown placeholder is left visible rather than dropped: silence is how a
/// missing value survives review, and a stray `{name}` on screen fails the tests
/// that guard every call site.
fn fill(template: &str, vars: &[(&str, &str)]) -> String {
    let mut out = String::with_capacity(template.len());
    let mut rest = template;

    while let Some(start) = rest.find('{') {
        out.push_str(&rest[..start]);
        let after = &rest[start + 1..];

        match after.find('}') {
            Some(end) => {
                let name = &after[..end];
                match vars.iter().find(|(n, _)| *n == name) {
                    Some((_, value)) => out.push_str(value),
                    None => {
                        out.push('{');
                        out.push_str(name);
                        out.push('}');
                    }
                }
                rest = &after[end + 1..];
            }
            None => {
                // Unbalanced brace: emit the remainder verbatim rather than
                // looping forever on it.
                out.push('{');
                out.push_str(after);
                rest = "";
            }
        }
    }

    out.push_str(rest);
    out
}

// ─────────────────────────────────────────────────────────────────────────────
// DNS
// ─────────────────────────────────────────────────────────────────────────────

pub static DNS_ENTER_A_DOMAIN: Outcome = Outcome {
    id: "enter_a_domain",
    state: PrereqState::Unknown,
    severity: PrereqSeverity::Info,
    title: "Enter a domain",
    title_plural: "",
    detail: "Once you enter a domain, DockPanel checks whether it already points here.",
    fix: "",
    vars: &[],
};

pub static DNS_NO_PUBLIC_IP: Outcome = Outcome {
    id: "no_public_ip",
    state: PrereqState::Unknown,
    severity: PrereqSeverity::Info,
    title: "Couldn't determine this server's public address",
    title_plural: "",
    detail: "DockPanel could not detect its own public IP, so it can't check whether the domain \
             points here. SSL issuance will still be attempted.",
    fix: "Nothing, usually — the check is unavailable, not failing. If issuance then fails, \
          confirm the server can reach the internet.",
    vars: &[],
};

pub static DNS_NOT_RESOLVING: Outcome = Outcome {
    id: "not_resolving",
    state: PrereqState::Unsatisfied,
    severity: PrereqSeverity::Blocking,
    title: "{domain} doesn't resolve yet",
    title_plural: "",
    detail: "No DNS record was found for {domain}. Create the record below at whoever manages \
             this domain's DNS, then check again. New records usually appear within a few \
             minutes.",
    fix: "Create the A (or AAAA) record DockPanel shows, at whoever manages the domain's DNS. \
          The gate opens by itself once the record goes live.",
    vars: &[Var { name: "domain", example: "example.com" }],
};

pub static DNS_POINTS_HERE: Outcome = Outcome {
    id: "points_here",
    state: PrereqState::Satisfied,
    severity: PrereqSeverity::Info,
    title: "{domain} points here",
    title_plural: "",
    detail: "{domain} resolves to this server ({server_ip}).",
    fix: "",
    vars: &[
        Var { name: "domain", example: "example.com" },
        Var { name: "server_ip", example: "203.0.113.10" },
    ],
};

pub static DNS_POINTS_ELSEWHERE: Outcome = Outcome {
    id: "points_elsewhere",
    state: PrereqState::Unsatisfied,
    severity: PrereqSeverity::Warning,
    title: "{domain} points somewhere else",
    title_plural: "",
    detail: "{domain} resolves to {resolved} rather than this server ({server_ip}).\n\n\
             If you use Cloudflare's proxy (the orange cloud), this is expected and certificate \
             issuance normally still works — Cloudflare passes the validation request through to \
             this server. You can continue.\n\n\
             Otherwise the domain is pointed at a different host. Update the record below and \
             check again.",
    fix: "If the domain is behind a proxy such as Cloudflare, nothing — issuance works through \
          it. Otherwise point the record at this server.",
    vars: &[
        Var { name: "domain", example: "example.com" },
        Var { name: "resolved", example: "198.51.100.7" },
        Var { name: "server_ip", example: "203.0.113.10" },
    ],
};

pub static DNS_POINTS_ELSEWHERE_SUBDOMAIN: Outcome = Outcome {
    id: "points_elsewhere_subdomain",
    state: PrereqState::Unsatisfied,
    severity: PrereqSeverity::Warning,
    title: "{domain} points somewhere else",
    title_plural: "",
    detail: "{domain} resolves to {resolved} rather than this server ({server_ip}).\n\n\
             If you use Cloudflare's proxy (the orange cloud), this is expected and certificate \
             issuance normally still works — Cloudflare passes the validation request through to \
             this server. You can continue.\n\n\
             Otherwise the domain is pointed at a different host. Update the record below and \
             check again — note this is the record for {domain}, not for {apex}.",
    fix: "Same as above, with one trap: edit the record for the subdomain itself. Pointing the \
          apex somewhere new does not move a subdomain that has its own record.",
    vars: &[
        Var { name: "domain", example: "www.example.com" },
        Var { name: "resolved", example: "198.51.100.7" },
        Var { name: "server_ip", example: "203.0.113.10" },
        Var { name: "apex", example: "example.com" },
    ],
};

pub static DNS_POINTS_HERE_CHECK: Prereq = Prereq {
    key: "dns.points_here",
    vertical: "DNS",
    question: "Does this domain point at this server yet?",
    why: "An HTTP-01 certificate order sends the certificate authority to your domain to fetch a \
          file from this server. If the domain resolves nowhere, the CA has nowhere to connect \
          and the order fails — after the panel has already told you the site was created.",
    severity_policy: "Blocking when the domain resolves to nothing at all, because issuance \
                      cannot possibly succeed. Only a warning when it resolves somewhere else: a \
                      Cloudflare-proxied domain resolves to Cloudflare and is indistinguishable \
                      from a misconfigured one by address alone, and issuance through the proxy \
                      demonstrably works. Refusing it would block a setup we have evidence is fine.",
    surfaces: &[
        "The create-site form, as you type the domain (never blocks creation)",
        "The SSL card on a site, where it gates the Let's Encrypt button",
        "`POST /api/sites/{id}/ssl`, which refuses with 412 for the same reason and in the same words",
    ],
    outcomes: &[
        &DNS_ENTER_A_DOMAIN,
        &DNS_NO_PUBLIC_IP,
        &DNS_NOT_RESOLVING,
        &DNS_POINTS_HERE,
        &DNS_POINTS_ELSEWHERE,
        &DNS_POINTS_ELSEWHERE_SUBDOMAIN,
    ],
};

// ─────────────────────────────────────────────────────────────────────────────
// Docker apps
// ─────────────────────────────────────────────────────────────────────────────

pub static APPS_ENV_ALL_PRESENT: Outcome = Outcome {
    id: "all_present",
    state: PrereqState::Satisfied,
    severity: PrereqSeverity::Info,
    title: "Required settings are filled in",
    title_plural: "",
    detail: "Every setting this app needs has a value.",
    fix: "",
    vars: &[],
};

pub static APPS_ENV_MISSING_SECRETS: Outcome = Outcome {
    id: "missing_secrets",
    state: PrereqState::Unsatisfied,
    severity: PrereqSeverity::Blocking,
    title: "{first_label} needs a value",
    title_plural: "{count} required settings have no value",
    detail: "{template_id} needs a value for {names}. These are passwords or keys with no safe \
             default — the container is started without them entirely, which usually means it \
             refuses to start or comes up with no protection at all.\n\n\
             Use Generate to have DockPanel pick a strong value.",
    fix: "Press Generate beside the field. DockPanel picks a strong value so there is nothing to \
          invent and nothing to remember.",
    vars: &[
        Var { name: "first_label", example: "POSTGRES_PASSWORD" },
        Var { name: "count", example: "1" },
        Var { name: "template_id", example: "postgres" },
        Var { name: "names", example: "POSTGRES_PASSWORD (POSTGRES_PASSWORD)" },
    ],
};

pub static APPS_ENV_MISSING_SETTINGS: Outcome = Outcome {
    id: "missing_settings",
    state: PrereqState::Unsatisfied,
    severity: PrereqSeverity::Blocking,
    title: "{first_label} needs a value",
    title_plural: "{count} required settings have no value",
    detail: "{template_id} needs a value for {names}. Without it the setting is not passed to \
             the container at all, so the app starts misconfigured — usually failing minutes \
             later, after the image has finished downloading.",
    fix: "Fill in the marked fields before deploying.",
    vars: &[
        Var { name: "first_label", example: "MEILI_MASTER_KEY" },
        Var { name: "count", example: "2" },
        Var { name: "template_id", example: "meilisearch" },
        Var { name: "names", example: "MEILI_MASTER_KEY (MEILI_MASTER_KEY), Environment (MEILI_ENV)" },
    ],
};

pub static APPS_ENV_CHECK: Prereq = Prereq {
    key: "apps.required_env",
    vertical: "Docker apps",
    question: "Does every setting this app requires have a value?",
    why: "104 of the shipped templates declare at least one setting that is required and has no \
          default — database passwords, signing keys, and so on. The agent resolves each setting \
          as \"the value you gave, otherwise the default\", and **drops it entirely when the \
          result is empty**. The variable is not passed as blank; it is not passed at all. \
          Postgres refuses to initialise, Grafana quietly keeps its built-in password, and you \
          find out from container logs several minutes after the image pull finished.",
    severity_policy: "Blocking. The template's own author declared the setting required, and the \
                      fix is to type into a field that is already on screen.",
    surfaces: &[
        "The Docker app deploy dialog, above the Deploy button",
        "`POST /api/apps/deploy`, which refuses with 412 by calling this same check",
    ],
    outcomes: &[&APPS_ENV_ALL_PRESENT, &APPS_ENV_MISSING_SECRETS, &APPS_ENV_MISSING_SETTINGS],
};

pub static APPS_PORT_UNSET: Outcome = Outcome {
    id: "unset",
    state: PrereqState::Unsatisfied,
    severity: PrereqSeverity::Blocking,
    title: "Choose a port",
    title_plural: "",
    detail: "Pick the host port this app should be reachable on.",
    fix: "Enter a port.",
    vars: &[],
};

pub static APPS_PORT_HELD_BY_KNOWN: Outcome = Outcome {
    id: "held_by_known",
    state: PrereqState::Unsatisfied,
    severity: PrereqSeverity::Blocking,
    title: "Port {port} is already in use",
    title_plural: "",
    detail: "Port {port} is taken by {holder}. Two things cannot publish on the same host port, \
             so this deploy would fail once the image finished downloading.",
    fix: "Use the free port DockPanel suggests, or stop whatever holds this one.",
    vars: &[
        Var { name: "port", example: "5432" },
        Var { name: "holder", example: "the app `pg-old`" },
    ],
};

pub static APPS_PORT_HELD_BY_UNKNOWN: Outcome = Outcome {
    id: "held_by_unknown",
    state: PrereqState::Unsatisfied,
    severity: PrereqSeverity::Blocking,
    title: "Port {port} is already in use",
    title_plural: "",
    detail: "Something on this server is already listening on port {port}. It isn't managed by \
             DockPanel, so it may be another service you installed yourself. Pick a different \
             port, or stop whatever is using this one.",
    fix: "Pick a different port. `ss -lntp | grep :{port}` on the server names the process.",
    vars: &[Var { name: "port", example: "5432" }],
};

pub static APPS_PORT_FREE: Outcome = Outcome {
    id: "free",
    state: PrereqState::Satisfied,
    severity: PrereqSeverity::Info,
    title: "Port {port} is free",
    title_plural: "",
    detail: "Nothing is listening on port {port}.",
    fix: "",
    vars: &[Var { name: "port", example: "5432" }],
};

pub static APPS_PORT_UNPROBEABLE: Outcome = Outcome {
    id: "unprobeable",
    state: PrereqState::Unknown,
    severity: PrereqSeverity::Info,
    title: "Couldn't check the port",
    title_plural: "",
    detail: "DockPanel couldn't ask this server whether port {port} is free, so it will be \
             attempted as-is.",
    fix: "Nothing. This happens on a fleet member running an agent older than the port-check \
          route; a check that cannot run must never turn into a refusal.",
    vars: &[Var { name: "port", example: "5432" }],
};

pub static APPS_PORT_CHECK: Prereq = Prereq {
    key: "apps.port_available",
    vertical: "Docker apps",
    question: "Is the host port this app wants actually free?",
    why: "Two containers cannot publish on the same host port. The sites surface has refused \
          colliding ports for years; apps never got the equivalent, so a collision surfaced as a \
          Docker error after the image pull rather than before it.",
    severity_policy: "Blocking when something is known to hold the port, because the deploy will \
                      certainly fail. Unknown — never a refusal — when the server could not be \
                      asked.",
    surfaces: &[
        "The Docker app deploy dialog, above the Deploy button",
        "`POST /api/apps/deploy`, which refuses with 412 by calling this same check",
    ],
    outcomes: &[
        &APPS_PORT_UNSET,
        &APPS_PORT_HELD_BY_KNOWN,
        &APPS_PORT_HELD_BY_UNKNOWN,
        &APPS_PORT_FREE,
        &APPS_PORT_UNPROBEABLE,
    ],
};

pub static APPS_NAME_UNSET: Outcome = Outcome {
    id: "unset",
    state: PrereqState::Unknown,
    severity: PrereqSeverity::Info,
    title: "Name this app",
    title_plural: "",
    detail: "Give the app a name to continue.",
    fix: "Enter a name.",
    vars: &[],
};

pub static APPS_NAME_TAKEN: Outcome = Outcome {
    id: "taken",
    state: PrereqState::Unsatisfied,
    severity: PrereqSeverity::Blocking,
    title: "An app called {name} already exists",
    title_plural: "",
    detail: "This server already runs an app named {name}. Container names have to be unique, so \
             this deploy would be refused. Pick a different name.",
    fix: "Use the name DockPanel suggests, or choose your own.",
    vars: &[Var { name: "name", example: "postgres" }],
};

pub static APPS_NAME_AVAILABLE: Outcome = Outcome {
    id: "available",
    state: PrereqState::Satisfied,
    severity: PrereqSeverity::Info,
    title: "Name is available",
    title_plural: "",
    detail: "No other app is called {name}.",
    fix: "",
    vars: &[Var { name: "name", example: "postgres" }],
};

pub static APPS_NAME_CHECK: Prereq = Prereq {
    key: "apps.name_available",
    vertical: "Docker apps",
    question: "Is this app name already taken on this server?",
    why: "Container names are unique per host, and the deploy form pre-fills the name with the \
          template id — so deploying a second Postgres collides by default.",
    severity_policy: "Blocking. Docker will refuse the name outright.",
    surfaces: &[
        "The Docker app deploy dialog, above the Deploy button",
        "`POST /api/apps/deploy`, which refuses with 412 by calling this same check",
    ],
    outcomes: &[&APPS_NAME_UNSET, &APPS_NAME_TAKEN, &APPS_NAME_AVAILABLE],
};

pub static APPS_MEM_NOT_CHECKED: Outcome = Outcome {
    id: "not_checked",
    state: PrereqState::Unknown,
    severity: PrereqSeverity::Info,
    title: "Memory not checked",
    title_plural: "",
    detail: "No memory limit was set for this app, or this server's memory couldn't be read.",
    fix: "",
    vars: &[],
};

pub static APPS_MEM_OVER_TOTAL: Outcome = Outcome {
    id: "over_total",
    state: PrereqState::Unsatisfied,
    severity: PrereqSeverity::Warning,
    title: "That's more memory than this server has",
    title_plural: "",
    detail: "This app is limited to {want} MB, but the server only has {total} MB in total \
             ({free} MB currently free). The limit will be accepted, but the container can never \
             reach it — and the server will start swapping first.",
    fix: "Lower the limit, or resize the server. Nothing stops you deploying as-is.",
    vars: &[
        Var { name: "want", example: "8192" },
        Var { name: "total", example: "2048" },
        Var { name: "free", example: "1100" },
    ],
};

pub static APPS_MEM_OVER_FREE: Outcome = Outcome {
    id: "over_free",
    state: PrereqState::Unsatisfied,
    severity: PrereqSeverity::Warning,
    title: "Less free memory than this app is allowed",
    title_plural: "",
    detail: "This app is allowed {want} MB but only {free} MB of the server's {total} MB is free \
             right now. It will still start; if it actually uses its full limit, the kernel will \
             have to reclaim memory from something else.",
    fix: "Usually nothing — over-committing memory is normal. Worth a second look if the box is \
          already swapping.",
    vars: &[
        Var { name: "want", example: "1024" },
        Var { name: "free", example: "700" },
        Var { name: "total", example: "2048" },
    ],
};

pub static APPS_MEM_FITS: Outcome = Outcome {
    id: "fits",
    state: PrereqState::Satisfied,
    severity: PrereqSeverity::Info,
    title: "Enough memory",
    title_plural: "",
    detail: "{free} MB free, {want} MB requested.",
    fix: "",
    vars: &[
        Var { name: "free", example: "1100" },
        Var { name: "want", example: "512" },
    ],
};

pub static APPS_MEM_CHECK: Prereq = Prereq {
    key: "apps.resource_headroom",
    vertical: "Docker apps",
    question: "Does this server have the memory this app is being given?",
    why: "A container whose limit exceeds what the box can supply does not fail at deploy time. \
          It fails later, under load, as an OOM kill — which is the worst possible moment to \
          learn the number was never achievable.",
    severity_policy: "Warning, never blocking. Over-committing memory is ordinary practice, the \
                      kernel will page, and the operator may know what is about to be freed. \
                      What is not acceptable is finding out from an OOM kill.",
    surfaces: &["The Docker app deploy dialog, above the Deploy button"],
    outcomes: &[&APPS_MEM_NOT_CHECKED, &APPS_MEM_OVER_TOTAL, &APPS_MEM_OVER_FREE, &APPS_MEM_FITS],
};

// ─────────────────────────────────────────────────────────────────────────────
// Mail
// ─────────────────────────────────────────────────────────────────────────────

pub static MAIL_DKIM_READY: Outcome = Outcome {
    id: "ready",
    state: PrereqState::Satisfied,
    severity: PrereqSeverity::Info,
    title: "DKIM signing key is ready",
    title_plural: "",
    detail: "Outgoing mail for {domain} is signed.",
    fix: "",
    vars: &[Var { name: "domain", example: "example.com" }],
};

pub static MAIL_DKIM_MISSING: Outcome = Outcome {
    id: "missing",
    state: PrereqState::Unsatisfied,
    severity: PrereqSeverity::Warning,
    title: "No DKIM signing key",
    title_plural: "",
    detail: "DKIM key generation failed when {domain} was added, so outgoing mail isn't signed \
             and there is no DKIM record to publish. Check that the mail stack is installed, \
             then remove and re-add the domain to retry.",
    fix: "Confirm the mail server is installed, then remove and re-add the domain.",
    vars: &[Var { name: "domain", example: "example.com" }],
};

pub static MAIL_DKIM_CHECK: Prereq = Prereq {
    key: "mail.dkim_key",
    vertical: "Mail",
    question: "Does this domain have a DKIM signing key?",
    why: "Without a key there is nothing to sign outgoing mail with and no DKIM record to \
          publish. Adding a domain stores it even when key generation fails, so this state is \
          reachable and would otherwise be invisible.",
    severity_policy: "Warning. Mail still flows; it is simply unsigned, and no control in the \
                      panel is gated on it.",
    surfaces: &["The Mail domain's DNS tab", "`GET /api/mail/domains/{id}/preflight`"],
    outcomes: &[&MAIL_DKIM_READY, &MAIL_DKIM_MISSING],
};

pub static MAIL_DNS_NO_PUBLIC_IP: Outcome = Outcome {
    id: "no_public_ip",
    state: PrereqState::Unknown,
    severity: PrereqSeverity::Info,
    title: "Couldn't determine this server's public address",
    title_plural: "",
    detail: "DockPanel could not determine the public address of the server this domain is on, \
             so it can't tell you which records the domain needs.",
    fix: "Nothing to do at the registrar. If the domain is on this server, confirm it can reach \
          the internet; if it is on another server in the fleet, confirm that server's agent is \
          online — its address is recorded when the agent checks in.",
    vars: &[],
};

pub static MAIL_DNS_UNREADABLE: Outcome = Outcome {
    id: "unreadable",
    state: PrereqState::Unknown,
    severity: PrereqSeverity::Info,
    title: "Couldn't check DNS",
    title_plural: "",
    detail: "DockPanel couldn't run DNS lookups on this server, so it can't verify these \
             records. Create them if you haven't already.",
    fix: "Create the records shown. They are correct whether or not the lookup ran.",
    vars: &[],
};

pub static MAIL_DNS_ALL_PUBLISHED: Outcome = Outcome {
    id: "all_published",
    state: PrereqState::Satisfied,
    severity: PrereqSeverity::Info,
    title: "{domain}'s mail records are published",
    title_plural: "",
    detail: "MX, SPF, DKIM and DMARC all resolve and point at this server. Mail from this domain \
             can be authenticated.",
    fix: "",
    vars: &[Var { name: "domain", example: "example.com" }],
};

pub static MAIL_DNS_MISSING_ALL: Outcome = Outcome {
    id: "missing_all",
    state: PrereqState::Unsatisfied,
    severity: PrereqSeverity::Warning,
    title: "{domain} has no mail DNS records yet",
    title_plural: "",
    detail: "{count} of {total} records for {domain} are missing or point somewhere else: \
             {missing}.\n\n\
             Until they are correct, receiving servers can't verify that this server may send \
             mail for {domain} — messages are likely to be filed as spam or rejected. Create the \
             records below at whoever manages this domain's DNS; DockPanel re-checks \
             automatically.",
    fix: "Create every record shown. Each card says whether that record is published or missing, \
          so you never have to re-do the ones already correct.",
    vars: &[
        Var { name: "domain", example: "example.com" },
        Var { name: "count", example: "5" },
        Var { name: "total", example: "5" },
        Var { name: "missing", example: "A example.com, MX example.com, TXT example.com" },
    ],
};

pub static MAIL_DNS_MISSING_SOME: Outcome = Outcome {
    id: "missing_some",
    state: PrereqState::Unsatisfied,
    severity: PrereqSeverity::Warning,
    title: "{count} mail record still missing for {domain}",
    title_plural: "{count} mail records still missing for {domain}",
    detail: "{count} of {total} records for {domain} are missing or point somewhere else: \
             {missing}.\n\n\
             Until they are correct, receiving servers can't verify that this server may send \
             mail for {domain} — messages are likely to be filed as spam or rejected. Create the \
             records below at whoever manages this domain's DNS; DockPanel re-checks \
             automatically.",
    fix: "Create the records marked missing. The published ones need no attention.",
    vars: &[
        Var { name: "domain", example: "example.com" },
        Var { name: "count", example: "2" },
        Var { name: "total", example: "5" },
        Var { name: "missing", example: "TXT dockpanel._domainkey.example.com, TXT _dmarc.example.com" },
    ],
};

pub static MAIL_DNS_CHECK: Prereq = Prereq {
    key: "mail.dns_published",
    vertical: "Mail",
    question: "Are this domain's mail records published, and do they point at this server?",
    why: "Mail leaves the server whether or not these records exist — it just doesn't arrive. \
          Receiving servers use SPF, DKIM and DMARC to decide whether to believe a message came \
          from your domain, and a domain missing them lands in spam folders with nothing anywhere \
          saying so.\n\nThis check verifies **correctness, not existence**, which is the whole \
          point of it: through v2.29.0 the older DNS check passed on any MX record at all \
          (including a competitor's), any string beginning `v=spf1` (including one that forbids \
          this server), and any DKIM record containing `p=` (including a stale key from a \
          previous install). Run against `gmail.com` — a domain whose mail goes to Google and \
          whose SPF does not authorise us at all — the old logic reported MX, SPF and DMARC all \
          passing.",
    severity_policy: "Warning, never blocking, and this follows the rule rather than ducking it: \
                      nothing in the panel is gated on these records. There is no control to \
                      disable, so there is nothing to block.",
    surfaces: &[
        "The Mail domain's DNS tab",
        "`GET /api/mail/domains/{id}/preflight`",
        "`GET /api/mail/domains/{id}/dns-check`, the long-standing Check DNS button, which now \
         delegates here",
    ],
    outcomes: &[
        &MAIL_DNS_NO_PUBLIC_IP,
        &MAIL_DNS_UNREADABLE,
        &MAIL_DNS_ALL_PUBLISHED,
        &MAIL_DNS_MISSING_ALL,
        &MAIL_DNS_MISSING_SOME,
    ],
};

// ─────────────────────────────────────────────────────────────────────────────
// Backups
// ─────────────────────────────────────────────────────────────────────────────

pub static BACKUPS_DEST_UPLOADS: Outcome = Outcome {
    id: "uploads_offsite",
    state: PrereqState::Satisfied,
    severity: PrereqSeverity::Info,
    title: "Backups are copied off this server",
    title_plural: "",
    detail: "'{policy}' uploads to {destination}.",
    fix: "",
    vars: &[
        Var { name: "policy", example: "Nightly" },
        Var { name: "destination", example: "wasabi-eu" },
    ],
};

pub static BACKUPS_DEST_DELETED: Outcome = Outcome {
    id: "destination_deleted",
    state: PrereqState::Unsatisfied,
    severity: PrereqSeverity::Warning,
    title: "This policy's backup destination no longer exists",
    title_plural: "",
    detail: "'{policy}' is set to upload to a destination that has since been deleted, so its \
             backups are staying on this server. Pick a destination again, or create a new one.",
    fix: "Choose a destination on the policy, or re-create the one that was removed.",
    vars: &[Var { name: "policy", example: "Nightly" }],
};

pub static BACKUPS_DEST_LOCAL_WITH_OPTIONS: Outcome = Outcome {
    id: "local_only_with_destinations",
    state: PrereqState::Unsatisfied,
    severity: PrereqSeverity::Warning,
    title: "Backups never leave this server",
    title_plural: "",
    detail: "'{policy}' writes its backups to this server's own disk and nowhere else. You \
             already have {available} destination(s) configured — choosing one means these \
             backups survive losing this machine.",
    fix: "Edit the policy and pick one of the destinations you have already configured.",
    vars: &[
        Var { name: "policy", example: "Nightly" },
        Var { name: "available", example: "2" },
    ],
};

pub static BACKUPS_DEST_LOCAL_NO_OPTIONS: Outcome = Outcome {
    id: "local_only_no_destinations",
    state: PrereqState::Unsatisfied,
    severity: PrereqSeverity::Warning,
    title: "Backups never leave this server",
    title_plural: "",
    detail: "'{policy}' writes its backups to this server's own disk and nowhere else. That \
             protects you from a bad deploy or a dropped table, but not from losing the server: \
             disk failure, a deleted instance or a compromised host take the backups with the \
             data.\n\n\
             Add an S3 or SFTP destination and select it on this policy.",
    fix: "Add an S3 or SFTP destination, then select it on the policy.",
    vars: &[Var { name: "policy", example: "Nightly" }],
};

pub static BACKUPS_DEST_CHECK: Prereq = Prereq {
    key: "backups.destination_configured",
    vertical: "Backups",
    question: "Does this backup policy send its backups anywhere off this server?",
    why: "A policy with no destination writes its archives to a directory on the same disk it is \
          insuring. That protects against a bad deploy or a dropped table, and against nothing \
          else: disk failure, a deleted instance, ransomware or a closed provider account take \
          the backups with the data.",
    severity_policy: "Warning, not blocking — and this one is worth defending. Local-only \
                      backups are a legitimate choice for a box whose real protection is provider \
                      snapshots, and refusing to save the policy would be the panel overriding a \
                      decision that is not its to make. What is not legitimate is making that \
                      choice by accident and never being told.",
    surfaces: &["Backup Manager → Overview", "`GET /api/backup-orchestrator/preflight`"],
    outcomes: &[
        &BACKUPS_DEST_UPLOADS,
        &BACKUPS_DEST_DELETED,
        &BACKUPS_DEST_LOCAL_WITH_OPTIONS,
        &BACKUPS_DEST_LOCAL_NO_OPTIONS,
    ],
};

pub static BACKUPS_POLICY_NOTHING: Outcome = Outcome {
    id: "nothing_to_back_up",
    state: PrereqState::Unknown,
    severity: PrereqSeverity::Info,
    title: "Nothing to back up yet",
    title_plural: "",
    detail: "Once you add a site or a database, DockPanel can back it up on a schedule.",
    fix: "",
    vars: &[],
};

pub static BACKUPS_POLICY_RUNNING: Outcome = Outcome {
    id: "running",
    state: PrereqState::Satisfied,
    severity: PrereqSeverity::Info,
    title: "Scheduled backups are running",
    title_plural: "",
    detail: "{count} enabled backup policy/policies.",
    fix: "",
    vars: &[Var { name: "count", example: "1" }],
};

pub static BACKUPS_POLICY_NONE: Outcome = Outcome {
    id: "none_enabled",
    state: PrereqState::Unsatisfied,
    severity: PrereqSeverity::Warning,
    title: "Nothing is being backed up on a schedule",
    title_plural: "",
    detail: "This server hosts {sites} site(s) and has no enabled backup policy, so nothing is \
             backed up automatically. Manual backups still work, but they only exist when \
             somebody remembers.",
    fix: "Create a backup policy. \"Protect Everything\" configures sites, databases and volumes \
          in one step.",
    vars: &[Var { name: "sites", example: "3" }],
};

pub static BACKUPS_POLICY_CHECK: Prereq = Prereq {
    key: "backups.policy_configured",
    vertical: "Backups",
    question: "Is anything being backed up on a schedule at all?",
    why: "Prior to, and distinct from, the destination question — a panel with no enabled policy \
          has nothing to send anywhere. Manual backups still work, but they only exist when \
          somebody remembers.",
    severity_policy: "Warning when the server actually hosts something. A panel with no sites is \
                      not neglecting anything, so that reports as unknown rather than as a fault.",
    surfaces: &["Backup Manager → Overview", "`GET /api/backup-orchestrator/preflight`"],
    outcomes: &[&BACKUPS_POLICY_NOTHING, &BACKUPS_POLICY_RUNNING, &BACKUPS_POLICY_NONE],
};

/// Every prerequisite, in the order the manual presents them.
pub static PREREQS: &[&Prereq] = &[
    &DNS_POINTS_HERE_CHECK,
    &APPS_ENV_CHECK,
    &APPS_PORT_CHECK,
    &APPS_NAME_CHECK,
    &APPS_MEM_CHECK,
    &MAIL_DKIM_CHECK,
    &MAIL_DNS_CHECK,
    &BACKUPS_DEST_CHECK,
    &BACKUPS_POLICY_CHECK,
];

// ─────────────────────────────────────────────────────────────────────────────
// The passive tier — form-field help and the text behind an (i)
// ─────────────────────────────────────────────────────────────────────────────

/// Every field whose passive guidance is owned by this registry.
///
/// Not every field in the product: a registry that must grow by one entry per
/// input is a registry nobody maintains. These are the fields where getting it
/// wrong costs the user a failed deploy, a lost credential or an afternoon —
/// which is the same test that decides whether something deserves a check.
pub static FIELDS: &[Field] = &[
    Field {
        id: "setup.admin_password",
        surface: "Create your admin account",
        label: "Password",
        // The very first field a new operator ever types into. The 8-character
        // rule was previously enforced only by the browser's native minLength,
        // which rejects the form before any of our own text can explain why.
        help: "At least 8 characters. This signs you in to the panel itself — not to any site \
               you host with it.",
        more: "This is the panel's first account and a full administrator: it can reach every \
               site, database and container on this server. There is no password-reset email \
               configured yet, so store it somewhere you trust before continuing. You can add \
               two-factor authentication straight afterwards from Settings.",
        escalates_to: "",
    },
    Field {
        id: "sites.create.domain",
        surface: "Create a site",
        label: "Domain",
        help: "Your site's public domain name (e.g. example.com). It needs to point at this \
               server before HTTPS can be issued — DockPanel checks that for you below.",
        more: "You can create the site before the domain resolves; only the certificate needs \
               DNS. If you are moving a live site, create it here first, confirm it serves \
               correctly, and change the DNS record last — that way the switchover has no gap.",
        escalates_to: "dns.points_here",
    },
    Field {
        id: "sites.create.runtime",
        surface: "Create a site",
        label: "Runtime",
        // Tooltip-only: the line under this select already describes whichever
        // runtime is currently chosen, which is more useful than anything static.
        help: "",
        more: "Static serves files as they are. PHP runs them through PHP-FPM. Node and Python \
               get a managed systemd process with nginx in front. Reverse Proxy is the one to \
               pick when something else already listens on a port — a Docker app, or a service \
               you run yourself — and DockPanel then owns only the front end and the certificate.",
        escalates_to: "",
    },
    Field {
        id: "sites.create.admin_password",
        surface: "Create a site",
        label: "Admin Password",
        help: "Leave blank and DockPanel generates one, then stores it in this site's Secrets \
               vault — it is not shown again anywhere else.",
        more: "Open the site, then Secrets, to read it back. Earlier versions generated this \
               password, used it and discarded it, which left you unable to log in to the site \
               you had just created.",
        escalates_to: "",
    },
    Field {
        id: "sites.create.proxy_port",
        surface: "Create a site",
        label: "Proxy Port",
        help: "The local port your application listens on.",
        more: "The port as seen from the server itself, not a public one. nginx connects to it \
               on 127.0.0.1; nothing needs to be opened in the firewall.",
        escalates_to: "",
    },
    Field {
        id: "apps.deploy.name",
        surface: "Deploy a Docker app",
        label: "Container Name",
        help: "Names the container on this server. Must be unique — DockPanel suggests a free \
               one if it isn't.",
        more: "The container is created as `dockpanel-app-<name>`, so this name is what you will \
               see in `docker ps` minus that prefix.",
        escalates_to: "apps.name_available",
    },
    Field {
        id: "apps.deploy.port",
        surface: "Deploy a Docker app",
        label: "Host Port",
        help: "The port on this server the app will answer on. DockPanel checks it is free \
               before the image is pulled.",
        more: "Published on 127.0.0.1 by default. To expose the app publicly, put a Reverse \
               Proxy site in front of it — that gets you a certificate and a domain instead of a \
               port number.",
        escalates_to: "apps.port_available",
    },
    Field {
        id: "apps.deploy.memory",
        surface: "Deploy a Docker app",
        label: "Memory (MB)",
        help: "Hard ceiling for this container. Leave blank for no limit.",
        more: "A limit above what the server has is accepted but unreachable — the box swaps \
               first. DockPanel warns rather than refuses, because over-committing is ordinary \
               and you may know what is about to be freed.",
        escalates_to: "apps.resource_headroom",
    },
    Field {
        id: "backups.policy.destination",
        surface: "Backup policy",
        label: "Destination",
        help: "Where a copy is uploaded after each backup. Leave empty and backups stay on this \
               server's own disk.",
        more: "A backup on the disk it protects covers a bad deploy or a dropped table, and \
               nothing worse. Destinations are S3-compatible storage or any SFTP server. \
               SFTP destinations that authenticate by PASSWORD need sshpass on the server the \
               backup runs from: fresh installs and servers added through the agent installer \
               get it automatically, but a panel upgraded in place does not, because update.sh \
               upgrades binaries and installs no packages. When it is missing, Test Connection \
               now says so in those words and Settings can install it onto that server for \
               you — you are no longer asked to run a package manager by hand. \
               Key-authenticated SFTP and every S3 destination are unaffected.",
        escalates_to: "backups.destination_configured",
    },
    Field {
        id: "mail.domain.records",
        surface: "Mail domain → DNS",
        label: "Required DNS Records",
        // Tooltip-only: the standing line names the domain being viewed.
        help: "",
        more: "These are the records DockPanel publishes for you when it manages the zone, and \
               the ones to create by hand when it doesn't. Verify DNS checks that each points at \
               this server rather than merely that something exists — a domain whose MX belongs \
               to another provider is reported as such, not as a pass.",
        escalates_to: "mail.dns_published",
    },
];

// ─────────────────────────────────────────────────────────────────────────────
// Export
// ─────────────────────────────────────────────────────────────────────────────

/// The header every generated artefact carries.
const GENERATED_BANNER: &str =
    "DO NOT EDIT — generated from panel/backend/src/services/prerequisites/copy.rs";

/// Render the registry as the documentation page.
///
/// This is the mechanism behind "the manual can never contradict the product":
/// the page is not written, it is emitted, from the same statics the running
/// product renders. A test asserts the committed file matches.
pub fn manual() -> String {
    let mut out = String::new();

    out.push_str(&format!("<!-- {GENERATED_BANNER} -->\n\n"));
    out.push_str("# What DockPanel checks before you commit\n\n");
    out.push_str(
        "Some things have to be true before a feature can work — a domain has to resolve, a port \
         has to be free, a backup has to have somewhere to go. DockPanel checks these for you and \
         says so on the screen where it matters, rather than letting the action fail and leaving \
         you to work out why.\n\n\
         This page is generated from the same source the product renders its messages from, so \
         the wording below is the wording you will see.\n\n",
    );

    out.push_str("## How to read the three tiers\n\n");
    out.push_str(
        "| Tier | What it looks like | What it means |\n\
         |---|---|---|\n\
         | Passive | A line under a field, or text behind an (i) | Context. Nothing is wrong. |\n\
         | Warning | An amber callout | It probably won't work, or it will leave a mess — but \
         the choice is yours and the control stays usable. |\n\
         | Blocking | A red callout, and the button is disabled | It *will* fail. The gate opens \
         by itself once the condition is met. |\n\n\
         A check DockPanel could not run never blocks anything and never accuses you of \
         anything — it says so and gets out of the way.\n\n",
    );

    let mut current_vertical = "";
    for prereq in PREREQS {
        if prereq.vertical != current_vertical {
            out.push_str(&format!("## {}\n\n", prereq.vertical));
            current_vertical = prereq.vertical;
        }

        out.push_str(&format!("### {}\n\n", prereq.question));
        out.push_str(&format!("`{}`\n\n", prereq.key));
        out.push_str(&format!("{}\n\n", prereq.why));
        out.push_str(&format!("**When it blocks, and when it only warns.** {}\n\n", prereq.severity_policy));

        out.push_str("**Where you see it**\n\n");
        for surface in prereq.surfaces {
            out.push_str(&format!("- {surface}\n"));
        }
        out.push('\n');

        out.push_str("| | What DockPanel says | What to do |\n|---|---|---|\n");
        for outcome in prereq.outcomes {
            let vars: Vec<(&str, &str)> =
                outcome.vars.iter().map(|v| (v.name, v.example)).collect();
            // Derived from the same predicate the product gates on, never
            // re-stated: a manual that decides for itself what counts as
            // blocking is exactly the drift this page exists to end.
            let tier = if outcome.blocks() {
                "**Blocks**"
            } else {
                match (outcome.state, outcome.severity) {
                    (PrereqState::Satisfied, _) => "OK",
                    (PrereqState::Unknown, _) => "Not checked",
                    (_, PrereqSeverity::Warning) => "Warns",
                    _ => "Explains",
                }
            };
            let title = fill(outcome.title, &vars);
            let detail = fill(outcome.detail, &vars).replace('\n', " ");
            let fix = if outcome.fix.is_empty() { "Nothing." } else { outcome.fix };
            out.push_str(&format!(
                "| {tier} | **{}** — {} | {} |\n",
                escape_table(&title),
                escape_table(&detail),
                escape_table(fix)
            ));
        }
        out.push('\n');
    }

    out.push_str("## Field help\n\n");
    out.push_str(
        "The quiet tier. These are the explanations under the fields themselves; where a field \
         has a check behind it, the same concern reappears as a callout when it stops being \
         hypothetical.\n\n",
    );

    let mut current_surface = "";
    for field in FIELDS {
        if field.surface != current_surface {
            out.push_str(&format!("### {}\n\n", field.surface));
            current_surface = field.surface;
        }
        if field.help.is_empty() {
            out.push_str(&format!("**{}**\n\n", field.label));
        } else {
            out.push_str(&format!("**{}** — {}\n\n", field.label, field.help));
        }
        if !field.more.is_empty() {
            out.push_str(&format!("{}\n\n", field.more));
        }
        if !field.escalates_to.is_empty() {
            out.push_str(&format!("Checked by `{}`.\n\n", field.escalates_to));
        }
    }

    out
}

/// Render the passive tier as a TypeScript module for the frontend.
///
/// Generated and committed rather than served, for two reasons: a form's helper
/// text must not depend on a network call that can fail, and a string the
/// frontend renders should be greppable in the frontend.
pub fn frontend_module() -> String {
    let mut out = String::new();
    out.push_str(&format!("// {GENERATED_BANNER}\n"));
    out.push_str("//\n// The passive tier of the guidance layer: the line under a field, and the\n");
    out.push_str("// longer text behind its (i). The reactive and blocking tiers arrive at\n");
    out.push_str("// runtime as PrereqResults from the same registry.\n\n");

    out.push_str("export interface FieldGuidance {\n");
    out.push_str("  /** Surface this field belongs to, as a user would name it. */\n");
    out.push_str("  surface: string;\n");
    out.push_str("  label: string;\n");
    out.push_str("  /** The line under the field. Always shown. */\n");
    out.push_str("  help: string;\n");
    out.push_str("  /** The longer explanation behind the (i). Empty means no tooltip. */\n");
    out.push_str("  more: string;\n");
    out.push_str("  /** Prerequisite key this field's guidance escalates into, if any. */\n");
    out.push_str("  escalatesTo: string;\n");
    out.push_str("}\n\n");

    out.push_str("export const FIELD_GUIDANCE = {\n");
    for field in FIELDS {
        out.push_str(&format!("  {:?}: {{\n", field.id));
        out.push_str(&format!("    surface: {:?},\n", field.surface));
        out.push_str(&format!("    label: {:?},\n", field.label));
        out.push_str(&format!("    help: {:?},\n", field.help));
        out.push_str(&format!("    more: {:?},\n", field.more));
        out.push_str(&format!("    escalatesTo: {:?},\n", field.escalates_to));
        out.push_str("  },\n");
    }
    out.push_str("} as const satisfies Record<string, FieldGuidance>;\n\n");
    out.push_str("export type FieldGuidanceId = keyof typeof FIELD_GUIDANCE;\n");

    out
}

/// Pipes and newlines end a markdown table cell early.
fn escape_table(s: &str) -> String {
    s.replace('|', "\\|")
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashSet;

    /// A value that happens to contain a placeholder must survive intact.
    ///
    /// Policy and container names are typed by the operator. Under a
    /// replace-in-sequence fill, a policy called `{available}` would come back
    /// rendered as a number — the product quietly correcting what someone
    /// deliberately typed.
    #[test]
    fn a_value_containing_a_placeholder_is_not_substituted_again() {
        let r = BACKUPS_DEST_CHECK.result(
            &BACKUPS_DEST_LOCAL_WITH_OPTIONS,
            &[("policy", "{available}"), ("available", "2")],
        );
        assert!(
            r.detail.contains("'{available}' writes its backups"),
            "the operator's own text was rewritten: {}",
            r.detail
        );
        assert!(r.detail.contains("already have 2 destination"));
    }

    /// An unbalanced brace must not hang the renderer.
    #[test]
    fn an_unterminated_placeholder_is_emitted_verbatim() {
        assert_eq!(fill("a {b", &[("b", "x")]), "a {b");
        assert_eq!(fill("{unknown} tail", &[]), "{unknown} tail");
    }

    #[test]
    fn every_prereq_key_is_unique_and_namespaced_by_its_vertical() {
        let mut seen = HashSet::new();
        for p in PREREQS {
            assert!(seen.insert(p.key), "duplicate prerequisite key `{}`", p.key);
            assert!(
                p.key.contains('.'),
                "`{}` must be namespaced, e.g. `dns.points_here`",
                p.key
            );
        }
    }

    #[test]
    fn outcome_ids_are_unique_within_their_check() {
        for p in PREREQS {
            let mut seen = HashSet::new();
            for o in p.outcomes {
                assert!(seen.insert(o.id), "`{}` has two outcomes called `{}`", p.key, o.id);
            }
        }
    }

    /// Every placeholder must be declared, or the user is shown a literal
    /// `{domain}` — the exact class of defect this registry exists to make
    /// impossible.
    #[test]
    fn rendering_an_outcome_with_its_declared_vars_leaves_no_placeholder() {
        for p in PREREQS {
            for o in p.outcomes {
                let vars: Vec<(&str, &str)> = o.vars.iter().map(|v| (v.name, v.example)).collect();
                for (label, template) in [
                    ("title", o.title),
                    ("title_plural", o.title_plural),
                    ("detail", o.detail),
                ] {
                    let rendered = fill(template, &vars);
                    assert!(
                        !rendered.contains('{'),
                        "`{}`/`{}` {label} has an undeclared placeholder: {rendered}",
                        p.key,
                        o.id
                    );
                }
            }
        }
    }

    /// A declared var that no template uses is dead weight that will outlive the
    /// sentence it was written for.
    #[test]
    fn every_declared_var_is_actually_used() {
        for p in PREREQS {
            for o in p.outcomes {
                for v in o.vars {
                    let needle = format!("{{{}}}", v.name);
                    assert!(
                        o.title.contains(&needle)
                            || o.title_plural.contains(&needle)
                            || o.detail.contains(&needle),
                        "`{}`/`{}` declares `{}` but never uses it",
                        p.key,
                        o.id,
                        v.name
                    );
                }
            }
        }
    }

    /// A plural title is selected by the `count` var; declaring one without it
    /// means the plural form can never be reached.
    #[test]
    fn a_plural_title_requires_a_count_var() {
        for p in PREREQS {
            for o in p.outcomes {
                if !o.title_plural.is_empty() {
                    assert!(
                        o.vars.iter().any(|v| v.name == "count"),
                        "`{}`/`{}` has a plural title but no `count` var to select it",
                        p.key,
                        o.id
                    );
                }
            }
        }
    }

    #[test]
    fn the_plural_title_is_used_only_when_count_is_not_one() {
        let one = APPS_ENV_CHECK.result(
            &APPS_ENV_MISSING_SECRETS,
            &[
                ("first_label", "POSTGRES_PASSWORD"),
                ("count", "1"),
                ("template_id", "postgres"),
                ("names", "POSTGRES_PASSWORD"),
            ],
        );
        assert_eq!(one.title, "POSTGRES_PASSWORD needs a value");

        let many = APPS_ENV_CHECK.result(
            &APPS_ENV_MISSING_SECRETS,
            &[
                ("first_label", "POSTGRES_PASSWORD"),
                ("count", "3"),
                ("template_id", "postgres"),
                ("names", "a, b, c"),
            ],
        );
        assert_eq!(many.title, "3 required settings have no value");
    }

    /// The runtime result must carry the check's key, not the outcome's id —
    /// the frontend and every test selector key off the former.
    #[test]
    fn a_rendered_result_carries_the_check_key_and_its_outcome_tier() {
        let r = DNS_POINTS_HERE_CHECK.result(&DNS_NOT_RESOLVING, &[("domain", "example.com")]);
        assert_eq!(r.key, "dns.points_here");
        assert_eq!(r.title, "example.com doesn't resolve yet");
        assert!(r.blocks());

        let ok = DNS_POINTS_HERE_CHECK
            .result(&DNS_POINTS_HERE, &[("domain", "example.com"), ("server_ip", "1.2.3.4")]);
        assert!(!ok.blocks());
        assert_eq!(ok.detail, "example.com resolves to this server (1.2.3.4).");
    }

    /// `Outcome::blocks` and `PrereqResult::blocks` must agree, or the manual
    /// documents a gate the product does not apply.
    #[test]
    fn the_manuals_notion_of_blocking_matches_the_runtimes() {
        for p in PREREQS {
            for o in p.outcomes {
                let vars: Vec<(&str, &str)> = o.vars.iter().map(|v| (v.name, v.example)).collect();
                assert_eq!(
                    o.blocks(),
                    p.result(o, &vars).blocks(),
                    "`{}`/`{}` is documented differently from how it behaves",
                    p.key,
                    o.id
                );
            }
        }
    }

    /// Every field that claims to escalate must name a check that exists.
    #[test]
    fn field_escalations_resolve_to_a_real_prerequisite() {
        for f in FIELDS {
            if f.escalates_to.is_empty() {
                continue;
            }
            assert!(
                PREREQS.iter().any(|p| p.key == f.escalates_to),
                "field `{}` escalates to `{}`, which is not a prerequisite",
                f.id,
                f.escalates_to
            );
        }
    }

    /// An entry that says nothing is a row in a registry that looks maintained
    /// and isn't.
    #[test]
    fn every_field_entry_says_something() {
        for f in FIELDS {
            assert!(
                !f.help.is_empty() || !f.more.is_empty(),
                "field `{}` has neither a help line nor a tooltip",
                f.id
            );
        }
    }

    #[test]
    fn field_ids_are_unique() {
        let mut seen = HashSet::new();
        for f in FIELDS {
            assert!(seen.insert(f.id), "duplicate field id `{}`", f.id);
        }
    }

    /// The generated TypeScript must be valid to paste into the frontend: the
    /// `{:?}` escaping is what makes an em-dash-and-quotes sentence safe, and a
    /// stray raw newline would break the module.
    #[test]
    fn the_generated_frontend_module_quotes_every_string() {
        let ts = frontend_module();
        assert!(ts.contains("export const FIELD_GUIDANCE"));
        for f in FIELDS {
            assert!(ts.contains(&format!("{:?}: {{", f.id)), "missing field {}", f.id);
        }
        // Every line inside the object literal is either structural or a quoted
        // property; a raw newline inside a string would produce an unterminated one.
        for line in ts.lines() {
            let quotes = line.chars().filter(|c| *c == '"').count();
            assert_eq!(quotes % 2, 0, "unbalanced quotes in generated line: {line}");
        }
    }

    #[test]
    fn the_manual_documents_every_check_and_every_outcome() {
        let md = manual();
        for p in PREREQS {
            assert!(md.contains(p.key), "the manual omits `{}`", p.key);
            for o in p.outcomes {
                let vars: Vec<(&str, &str)> = o.vars.iter().map(|v| (v.name, v.example)).collect();
                let title = escape_table(&fill(o.title, &vars));
                assert!(
                    md.contains(&title),
                    "the manual omits `{}`/`{}` ({title})",
                    p.key,
                    o.id
                );
            }
        }
    }

    /// The repository root, derived from this crate rather than the working
    /// directory, so the test behaves the same from an editor and from CI.
    fn repo_root() -> std::path::PathBuf {
        std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../..")
            .canonicalize()
            .expect("repo root")
    }

    /// Assert a generated artefact matches what the registry emits — or rewrite
    /// it when explicitly asked to.
    ///
    /// This is the mechanism the whole exercise exists for. It is not a reminder
    /// to update the docs: it is a failing test that stands between a changed
    /// sentence and a merge, so the manual physically cannot describe a version
    /// of the product that no longer exists.
    fn assert_generated(relative: &str, expected: String) {
        let path = repo_root().join(relative);
        let update = std::env::var("UPDATE_GUIDANCE_DOCS").is_ok();

        if update {
            if let Some(dir) = path.parent() {
                std::fs::create_dir_all(dir).expect("create parent dir");
            }
            std::fs::write(&path, &expected).expect("write generated file");
            return;
        }

        let found = std::fs::read_to_string(&path).unwrap_or_else(|e| {
            panic!(
                "{relative} is missing ({e}). Regenerate it:\n    \
                 UPDATE_GUIDANCE_DOCS=1 cargo test -p dockpanel-api guidance"
            )
        });

        if found != expected {
            let (line, f, e) = found
                .lines()
                .zip(expected.lines())
                .enumerate()
                .find(|(_, (f, e))| f != e)
                .map(|(i, (f, e))| (i + 1, f.to_string(), e.to_string()))
                .unwrap_or_else(|| {
                    (
                        found.lines().count().min(expected.lines().count()) + 1,
                        "<end of file>".into(),
                        "<more lines>".into(),
                    )
                });
            panic!(
                "{relative} is out of date — the copy registry changed and this file did not.\n\
                 First difference at line {line}:\n  committed: {f}\n  registry:  {e}\n\n\
                 Regenerate it:\n    UPDATE_GUIDANCE_DOCS=1 cargo test -p dockpanel-api guidance"
            );
        }
    }

    #[test]
    fn guidance_manual_is_current() {
        assert_generated("docs/guides/prerequisites.md", manual());
    }

    #[test]
    fn guidance_frontend_module_is_current() {
        assert_generated(
            "panel/frontend/src/content/guidance.generated.ts",
            frontend_module(),
        );
    }

    /// Every field entry must actually be rendered somewhere.
    ///
    /// The failure this prevents is the one the whole session was about: copy
    /// that documents a screen which does not show it. A registry entry nobody
    /// renders still lands in the manual, so the manual describes a helper line
    /// the user will look for and not find.
    ///
    /// Deliberately a cross-tree assertion — the backend owns the copy and the
    /// frontend renders it, so nothing inside either tree alone can catch this.
    #[test]
    fn every_field_entry_is_rendered_by_the_frontend() {
        let src = repo_root().join("panel/frontend/src");
        let mut rendered = String::new();
        let mut stack = vec![src.clone()];
        while let Some(dir) = stack.pop() {
            for entry in std::fs::read_dir(&dir).expect("read frontend src") {
                let path = entry.expect("dir entry").path();
                if path.is_dir() {
                    stack.push(path);
                } else if path.extension().is_some_and(|e| e == "tsx") {
                    rendered.push_str(&std::fs::read_to_string(&path).unwrap_or_default());
                }
            }
        }

        for f in FIELDS {
            let needle = format!("\"{}\"", f.id);
            assert!(
                rendered.contains(&needle),
                "field `{}` is in the registry (and therefore in the manual) but no .tsx renders \
                 it — either wire it with <FieldHelp id={needle}/> or remove the entry",
                f.id
            );
        }
    }

    /// The generated page has to be reachable, or it is a file nobody reads.
    #[test]
    fn the_manual_is_listed_in_the_docs_summary() {
        let summary = std::fs::read_to_string(repo_root().join("docs/SUMMARY.md"))
            .expect("docs/SUMMARY.md");
        assert!(
            summary.contains("guides/prerequisites.md"),
            "docs/SUMMARY.md must link the generated page, or mdbook will not render it"
        );
    }

    /// A table cell containing a raw pipe silently truncates the row.
    #[test]
    fn generated_table_rows_have_a_stable_column_count() {
        for line in manual().lines() {
            if line.starts_with("| ") && !line.starts_with("|---") {
                assert_eq!(
                    line.matches('|').count() - line.matches("\\|").count(),
                    4,
                    "table row has the wrong number of cells: {line}"
                );
            }
        }
    }
}
