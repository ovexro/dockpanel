//! A delete must prove it owns what it deletes.
//!
//! v2.52.0 fixed the *write* side of this mistake: a vhost writer that replaced
//! a file and, when the whole-server `nginx -t` failed, deleted it — so a
//! neighbour's broken config cost an innocent site its own. The read side was
//! left standing, and it is the same shape one step later: a *removal* that
//! names its target by a key it never checked belongs to the thing being
//! removed.
//!
//! Two ways the key lies.
//!
//! **It is derived from an identity nobody re-verified.** A Docker app's domain
//! lives in the `dockpanel.app.domain` label. Removing the app deletes
//! `/etc/nginx/sites-enabled/{domain}.conf` — but by then that vhost may belong
//! to a site, because on any box installed before v2.52.0 nothing stopped a site
//! from claiming a domain an app already held.
//!
//! **It is not injective.** `domain.replace('.', "-")` maps `a.b.com` and
//! `a-b.com` onto one name, and `-` is legal inside a domain label, so both are
//! separately claimable and separately deletable by different owners. That is
//! the bug `traefik::route_key` was written to fix — and the fix is only half
//! applied while `remove_route_config` still deletes the pre-fix name, which is
//! the *current* name of a different live domain.
//!
//! The remedy in both cases is the same and needs no rename, no migration and no
//! new database column: **every one of these resources already records its own
//! owner**, because the panel wrote it there. A systemd unit carries the
//! `WorkingDirectory` of the site it runs. A Fail2Ban jail carries the `logpath`
//! it watches. A Traefik route carries its `Host()` rule. An app's vhost carries
//! the `proxy_pass` port of the container behind it. So do not trust the
//! filename — open the file and ask who it says it belongs to.
//!
//! [`Owner::Unknown`] deliberately does **not** permit a delete. Leaving a stale
//! config behind is an untidy box; deleting a live one is an outage nobody can
//! attribute. The asymmetry is the whole point, so every caller logs the refusal
//! loudly enough that the leftover is findable.
//!
//! # Containers
//!
//! Everything above reads a file. That left the largest resource the agent
//! destroys — a running container — with no primitive at all, and the same two
//! ways for a key to lie apply to it word for word.
//!
//! A git deploy named `X` is the container `dockpanel-git-X`. A *preview* of
//! config `C` on branch `B` was the container `dockpanel-git-{C}-pr-{slug(B)}`,
//! which is the same string a deploy named `{C}-pr-{slug(B)}` produces — and
//! `is_valid_name` permits that name. The branch half is chosen by whoever can
//! push to `C`'s repo, and the webhook that acts on it has no auth extractor.
//! `find_container` matched on the name alone, so a pushed branch could adopt a
//! stranger's production container, blue-green it out of existence and repoint
//! **that stranger's own domain** at the pushed build. The ownership guard
//! shipped in v2.53.0 authorised the vhost write, correctly: the victim's
//! container really was the one behind the victim's vhost.
//!
//! So the two spaces are now disjoint (a preview is scoped `pr.`, and `.` is a
//! character [`crate::routes::is_valid_name`] rejects, so no deploy can spell
//! one), and [`git_container`] is the read-side proof: a container states what
//! it is in the labels the agent wrote when it created it. Same remedy as
//! above — do not trust the name, open the thing and ask.

/// Who a resource on disk says it belongs to.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Owner {
    /// The file names this domain. Deleting it is correct.
    Ours,
    /// The file names something else. Deleting it destroys another owner's
    /// configuration.
    Theirs,
    /// The file could not be read, or carries no marker to judge by. Treated as
    /// `Theirs` by [`Owner::may_delete`] — see the module note on fail-closed.
    Unknown,
}

impl Owner {
    /// The only question a caller should ask. `Unknown` is not a maybe.
    pub fn may_delete(self) -> bool {
        matches!(self, Owner::Ours)
    }
}

/// The label key a git container carries to say which of the two disjoint name
/// spaces it was created in.
pub const GIT_KIND_LABEL: &str = "dockpanel.git.kind";

/// The suffix a blue-green swap parks its stand-in container under.
///
/// It was `-blue`, which is not a space anybody owns: `is_valid_name` accepts
/// `api-blue`, so `dockpanel-app-api-blue` is *simultaneously* the stand-in for
/// the app `api` and the production container of the app `api-blue` — and the
/// leftover-cleanup that runs before every swap force-removed whatever it
/// found there. The separator is now `.`, which `is_valid_name` rejects, so no
/// deployment can be standing in this space to begin with.
pub const BLUE_SUFFIX: &str = ".blue";

/// Compose the blue-green stand-in name for `container_name`.
pub fn blue_name(container_name: &str) -> String {
    format!("{container_name}{BLUE_SUFFIX}")
}

/// Which name space a git request addresses.
///
/// The variants are not interchangeable spellings of one thing: they select the
/// container name, the image repository, the on-disk checkout directory *and*
/// the ownership rule, and those four must never disagree.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GitScope {
    /// A configured deployment — a row in `git_deploys`, whose `name` is unique
    /// per server.
    Deploy,
    /// A branch preview. Scoped `pr.` so it cannot spell a deploy's name.
    Preview,
    /// A preview created before the scopes were separated, addressed by its old
    /// unscoped name. Cleanup only — nothing may ever *create* in this space.
    PreviewLegacy,
}

impl GitScope {
    /// Parse the wire value. Unknown values are [`GitScope::Deploy`] so that a
    /// panel older than this agent keeps working unchanged.
    pub fn from_wire(s: &str) -> Self {
        match s {
            "preview" => GitScope::Preview,
            "preview_legacy" => GitScope::PreviewLegacy,
            _ => GitScope::Deploy,
        }
    }

    /// The [`GIT_KIND_LABEL`] value written on containers created in this space.
    pub fn label(self) -> &'static str {
        match self {
            GitScope::Deploy => "deploy",
            GitScope::Preview | GitScope::PreviewLegacy => "preview",
        }
    }

    /// Turn the panel's `name` into the name this space actually uses.
    ///
    /// `.` is deliberate: [`crate::routes::is_valid_name`] rejects it, so no
    /// deploy name can ever produce a string in the preview space. A prefix made
    /// of characters a deploy name *can* contain would only move the collision.
    pub fn scoped(self, name: &str) -> String {
        match self {
            GitScope::Deploy | GitScope::PreviewLegacy => name.to_string(),
            GitScope::Preview => format!("pr.{name}"),
        }
    }
}

/// Is the container carrying `labels` the one a request in `scope` may act on?
///
/// `expected_port` is what the CALLER's own record says this thing publishes,
/// and `actual_port` is what the container publishes. Docker will not let two
/// containers bind one host port, so when the labels cannot answer, the port
/// can — the same argument [`app_vhost`] already makes for a vhost.
///
/// Containers created before the scopes were separated carry no
/// [`GIT_KIND_LABEL`] at all, and the answer to that has to differ by space,
/// because the three spaces have entirely different legacy populations:
///
/// * [`GitScope::Deploy`] — an unlabelled container at `dockpanel-git-{name}`
///   is this deployment's own, created by an older agent: deploy names are
///   unique per server, and the one thing that could previously stand there
///   under someone else's ownership — a preview — no longer writes into this
///   space at all. Refusing would break the update path of every app on every
///   existing install. `Ours`, and the next deploy relabels it.
/// * [`GitScope::Preview`] — nothing legacy can be here; the space did not
///   exist. An unlabelled container at `dockpanel-git-pr.{name}` was not put
///   there by this agent. `Unknown`, which may not delete.
/// * [`GitScope::PreviewLegacy`] — the old SHARED space, reached only to clean
///   up, and the one place where "unlabelled" is genuinely ambiguous: an old
///   preview and a deployment that merely shares its name look identical.
///   Answering `Ours` here would have preserved, inside the fix, the exact
///   unattended cross-owner delete the fix exists to close — every container
///   predating this build is unlabelled, including the victim's. So the port
///   must agree, and if the caller does not know it, or it does not match, the
///   answer is `Unknown` and the stale preview is left standing.
pub fn git_container(
    labels: Option<&std::collections::HashMap<String, String>>,
    scope: GitScope,
    expected_port: Option<u16>,
    actual_port: Option<u16>,
) -> Owner {
    let kind = labels.and_then(|l| l.get(GIT_KIND_LABEL)).map(String::as_str);
    match (scope, kind) {
        (_, Some(k)) if k == scope.label() => Owner::Ours,
        (_, Some(_)) => Owner::Theirs,
        (GitScope::Deploy, None) => Owner::Ours,
        (GitScope::Preview, None) => Owner::Unknown,
        (GitScope::PreviewLegacy, None) => match (expected_port, actual_port) {
            (Some(a), Some(b)) if a == b => Owner::Ours,
            _ => Owner::Unknown,
        },
    }
}

/// May the container found at a [`blue_name`] be force-removed as the leftover
/// of an interrupted swap?
///
/// The load-bearing half of that question is answered by [`BLUE_SUFFIX`]: no
/// panel resource can be named into that space, so nothing legitimate is
/// standing there. This is the second half — Docker itself will accept a `.` in
/// a name typed by hand, and an operator's own container is not ours to
/// destroy. `Unknown` for anything that does not say it is managed.
pub fn blue_leftover(labels: Option<&std::collections::HashMap<String, String>>) -> Owner {
    match labels
        .and_then(|l| l.get("dockpanel.managed"))
        .map(String::as_str)
    {
        Some("true") => Owner::Ours,
        _ => Owner::Unknown,
    }
}

/// Read `path` and decide ownership by whether any line, trimmed, equals one of
/// `markers`.
///
/// Whole-line equality rather than `contains`, because every marker here embeds
/// a domain and `contains` would let `example.com` answer for
/// `example.community` — the same unanchored-substring defect that let one
/// site's WordPress auto-update cron strip another's.
fn owner_by_lines(path: &str, markers: &[String]) -> Owner {
    let Ok(content) = std::fs::read_to_string(path) else {
        return Owner::Unknown;
    };
    if content
        .lines()
        .any(|line| markers.iter().any(|m| line.trim() == m))
    {
        Owner::Ours
    } else {
        Owner::Theirs
    }
}

/// Does the systemd unit at `path` run `domain`'s app process?
///
/// `create_app_service` writes a `WorkingDirectory` under `/var/www/{domain}` and
/// `Description=DockPanel App: {domain}`; either identifies the owner, and a
/// hand-edited description should not be able to make the unit undeletable, so
/// both are accepted.
///
/// ⚠ BOTH working-directory forms are listed, and the `/public` one may not be
/// dropped. Units written before v2.123.0 hardcoded `{domain}/public`; v2.123.0
/// starts the process at the site root instead (the crash-loop fix in
/// `app_process`). Recognising only the new form would hand every unit already on
/// disk to the `Theirs` branch — `owned_service_name` refuses to stop, restart or
/// delete one of those — so an upgrade would strand exactly the sites the release
/// exists to repair. The `Description` marker happens to cover the same case, but
/// this function's whole reason for accepting two markers is that either one alone
/// can be edited away.
pub fn systemd_unit(path: &str, domain: &str) -> Owner {
    owner_by_lines(
        path,
        &[
            format!("WorkingDirectory=/var/www/{domain}"),
            format!("WorkingDirectory=/var/www/{domain}/public"),
            format!("Description=DockPanel App: {domain}"),
        ],
    )
}

/// Does the Fail2Ban jail at `path` watch `domain`'s access log?
///
/// The jail's `logpath` is the one line that says what it is actually
/// protecting, which is exactly what the colliding `nginx-{mangled}.conf`
/// filename does not.
pub fn fail2ban_jail(path: &str, domain: &str) -> Owner {
    owner_by_lines(
        path,
        &[format!("logpath = /var/log/nginx/{domain}.access.log")],
    )
}

/// Does the Traefik dynamic route file at `path` route `domain`?
///
/// Both the plain and the TLS router emit the same `rule:` line, so one marker
/// covers both shapes `write_route_config` produces.
pub fn traefik_route(path: &str, domain: &str) -> Owner {
    owner_by_lines(path, &[format!("rule: \"Host(`{domain}`)\"")])
}

/// Is the nginx vhost at `path` the one serving the container published on
/// `host_port`?
///
/// An app's vhost is rendered from the same template a site's is, so the file
/// carries nothing that says "an app wrote this". What it does carry is the
/// `proxy_pass` line naming the container's own published port, and Docker will
/// not let two containers publish the same host port — so the port is the
/// app's identity in the only place the vhost records it.
///
/// `None` for the port means the container's binding could not be read; that is
/// `Unknown`, and the caller must not delete.
pub fn app_vhost(path: &str, host_port: Option<u16>) -> Owner {
    let Some(port) = host_port else {
        return Owner::Unknown;
    };
    let Ok(content) = std::fs::read_to_string(path) else {
        return Owner::Unknown;
    };
    let marker = format!("proxy_pass http://127.0.0.1:{port};");
    if content.lines().any(|line| line.trim() == marker) {
        Owner::Ours
    } else {
        Owner::Theirs
    }
}

/// Is the certificate directory `/etc/dockpanel/ssl/{domain}` still referenced
/// by a vhost other than `domain`'s own?
///
/// A DNS-01 wildcard is provisioned once under the zone apex and every site in
/// the zone points its `ssl_certificate` at that one directory. Deleting it
/// because *one* of those sites went away leaves the others pointing at a
/// missing file — which does not fail now, because nginx is already serving from
/// memory, but fails the next `nginx -t` anyone triggers and leaves nginx down
/// at the next full restart, for every site on the box.
///
/// Answered by reading the vhosts rather than the database because the agent has
/// no database, and because the vhosts are what nginx will actually parse.
pub fn cert_dir_in_use_elsewhere(domain: &str) -> bool {
    let needle = format!("/etc/dockpanel/ssl/{domain}/");
    let own = format!("{domain}.conf");
    let Ok(entries) = std::fs::read_dir("/etc/nginx/sites-enabled") else {
        // Cannot tell — assume shared. Refusing to delete a cert directory is
        // recoverable; deleting one another vhost still points at is not.
        return true;
    };
    for entry in entries.flatten() {
        let name = entry.file_name();
        let name = name.to_string_lossy();
        if name == own.as_str() {
            continue;
        }
        if let Ok(content) = std::fs::read_to_string(entry.path()) {
            if content.contains(&needle) {
                return true;
            }
        }
    }
    false
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A file under the process temp dir, named uniquely so parallel test
    /// threads cannot collide. Matches the idiom in `services::files`.
    struct Tmp(std::path::PathBuf);

    impl Drop for Tmp {
        fn drop(&mut self) {
            std::fs::remove_file(&self.0).ok();
        }
    }

    fn tmpfile(body: &str) -> (Tmp, String) {
        let path = std::env::temp_dir().join(format!("dp-own-{}", uuid::Uuid::new_v4()));
        std::fs::write(&path, body).unwrap();
        let s = path.to_string_lossy().into_owned();
        (Tmp(path), s)
    }

    #[test]
    fn a_missing_file_is_unknown_and_unknown_may_not_delete() {
        assert_eq!(systemd_unit("/nonexistent/unit.service", "a.com"), Owner::Unknown);
        assert!(!Owner::Unknown.may_delete());
        assert!(!Owner::Theirs.may_delete());
        assert!(Owner::Ours.may_delete());
    }

    #[test]
    fn the_systemd_collision_that_motivated_this_is_refused() {
        // `a.b.com` and `a-b.com` both mangle to `dockpanel-app-a-b-com`, so the
        // owner of one used to be able to delete the unit of the other.
        let (_d, p) = tmpfile(
            "[Unit]\nDescription=DockPanel App: a.b.com\n\n[Service]\n\
             WorkingDirectory=/var/www/a.b.com/public\nExecStart=node server.js\n",
        );
        assert_eq!(systemd_unit(&p, "a.b.com"), Owner::Ours);
        assert_eq!(systemd_unit(&p, "a-b.com"), Owner::Theirs);
    }

    /// v2.123.0 moved a managed process off `{domain}/public` and onto the site
    /// root. Ownership is keyed on that very line, so BOTH spellings have to be
    /// recognised — and the one that matters on an upgrade is the OLD one: every
    /// unit already on disk carries it, and `owned_service_name` refuses to stop,
    /// restart or delete anything this function calls `Theirs`. Dropping the
    /// superseded marker would strand exactly the sites the release repairs.
    ///
    /// The description is deliberately omitted from both fixtures. With it
    /// present either marker alone satisfies the check, and the arm would pass
    /// without ever exercising the WorkingDirectory line it exists to pin.
    #[test]
    fn both_working_directory_forms_are_ours_and_neither_leaks_to_a_collision() {
        let (_d1, new_form) =
            tmpfile("[Service]\nWorkingDirectory=/var/www/a.b.com\nExecStart=node server.js\n");
        assert_eq!(systemd_unit(&new_form, "a.b.com"), Owner::Ours);
        assert_eq!(systemd_unit(&new_form, "a-b.com"), Owner::Theirs);

        let (_d2, old_form) = tmpfile(
            "[Service]\nWorkingDirectory=/var/www/a.b.com/public\nExecStart=node server.js\n",
        );
        assert_eq!(systemd_unit(&old_form, "a.b.com"), Owner::Ours);
        assert_eq!(systemd_unit(&old_form, "a-b.com"), Owner::Theirs);

        // The site-root marker is a strict PREFIX of the public one, so an
        // unanchored comparison would let a unit for `a.b.com` answer for a
        // sibling whose name merely starts the same way. `owner_by_lines` compares
        // whole trimmed lines; this asserts it stays that way.
        assert_eq!(systemd_unit(&new_form, "a.b.co"), Owner::Theirs);
    }

    #[test]
    fn the_fail2ban_collision_is_refused_on_the_same_pair() {
        let (_d, p) = tmpfile(
            "[nginx-a-b-com]\nenabled = true\nfilter = nginx-http-auth\n\
             logpath = /var/log/nginx/a.b.com.access.log\nmaxretry = 10\n",
        );
        assert_eq!(fail2ban_jail(&p, "a.b.com"), Owner::Ours);
        assert_eq!(fail2ban_jail(&p, "a-b.com"), Owner::Theirs);
    }

    #[test]
    fn the_traefik_legacy_name_names_a_different_live_domain() {
        // `legacy_route_config_path("a-b.com")` is `a-b-com.yml`, which is
        // `route_key("a.b.com")` — the file below. Deleting it on behalf of
        // `a-b.com` is what this refuses.
        let (_d, p) = tmpfile(concat!(
            "http:\n",
            "  routers:\n",
            "    a-b-com:\n",
            "      rule: \"Host(`a.b.com`)\"\n",
            "      entryPoints:\n",
            "        - web\n",
        ));
        assert_eq!(traefik_route(&p, "a.b.com"), Owner::Ours);
        assert_eq!(traefik_route(&p, "a-b.com"), Owner::Theirs);
    }

    #[test]
    fn a_vhost_proxying_to_another_port_is_not_this_apps() {
        let (_d, p) = tmpfile(concat!(
            "server {\n",
            "  server_name app.example.com;\n",
            "  location / {\n",
            "    proxy_pass http://127.0.0.1:8080;\n",
            "  }\n",
            "}\n",
        ));
        assert_eq!(app_vhost(&p, Some(8080)), Owner::Ours);
        assert_eq!(app_vhost(&p, Some(9090)), Owner::Theirs);
        // A container whose port binding could not be read decides nothing.
        assert_eq!(app_vhost(&p, None), Owner::Unknown);
    }

    #[test]
    fn markers_are_whole_line_so_a_prefix_domain_cannot_answer_for_a_longer_one() {
        // The unanchored-substring class: `example.com` must not match
        // `example.community`, which is a separately registerable domain.
        let (_d, p) = tmpfile(
            "[Unit]\nDescription=DockPanel App: example.community\n\n[Service]\n\
             WorkingDirectory=/var/www/example.community/public\n",
        );
        assert_eq!(systemd_unit(&p, "example.community"), Owner::Ours);
        assert_eq!(systemd_unit(&p, "example.com"), Owner::Theirs);
    }

    #[test]
    fn a_file_with_no_marker_at_all_is_theirs_not_ours() {
        let (_d, p) = tmpfile("[Unit]\nDescription=Something an operator wrote\n");
        assert_eq!(systemd_unit(&p, "a.com"), Owner::Theirs);
    }

    fn labelled(pairs: &[(&str, &str)]) -> std::collections::HashMap<String, String> {
        pairs
            .iter()
            .map(|(k, v)| ((*k).to_string(), (*v).to_string()))
            .collect()
    }

    #[test]
    fn the_preview_scope_cannot_spell_any_legal_deploy_name() {
        // The whole separation rests on this: `is_valid_name` rejects `.`, so a
        // deploy can never be asked for under a name the preview space uses.
        let scoped = GitScope::Preview.scoped("myapp-pr-feature-x");
        assert!(scoped.contains('.'));
        assert!(!crate::routes::is_valid_name(&scoped));
        // And the collision that motivated this: a deploy literally named
        // `{config}-pr-{branch}` no longer shares a string with that preview.
        assert_ne!(scoped, GitScope::Deploy.scoped("myapp-pr-feature-x"));
    }

    #[test]
    fn a_preview_may_not_adopt_a_container_that_says_it_is_a_deploy() {
        let deploy = labelled(&[(GIT_KIND_LABEL, "deploy")]);
        assert_eq!(
            git_container(Some(&deploy), GitScope::Preview, None, None),
            Owner::Theirs
        );
        // …and the legacy cleanup path refuses it by the same label even when
        // the caller's port happens to agree, which is the only thing standing
        // between an expiring preview and a production container sharing its
        // old name.
        assert_eq!(
            git_container(Some(&deploy), GitScope::PreviewLegacy, Some(8001), Some(8001)),
            Owner::Theirs
        );
    }

    #[test]
    fn an_unlabelled_container_is_read_differently_in_each_space() {
        // Deploy: an older agent's container. Refusing it would break every
        // existing install's update path.
        assert_eq!(git_container(None, GitScope::Deploy, None, None), Owner::Ours);
        // Preview: the space is new, so nothing legitimate is unlabelled here.
        assert_eq!(git_container(None, GitScope::Preview, None, None), Owner::Unknown);
        assert!(!git_container(None, GitScope::Preview, None, None).may_delete());
    }

    #[test]
    fn the_legacy_space_needs_the_port_because_every_old_container_is_unlabelled() {
        // THE TRAP THIS TEST EXISTS FOR: the compatibility path is reached with
        // a name that a production deployment can legally hold, and on an
        // install upgrading to this build EVERY container is unlabelled —
        // including the victim's. Answering `Ours` on the label alone would
        // have carried the original unattended delete straight through the fix.
        assert_eq!(
            git_container(None, GitScope::PreviewLegacy, None, None),
            Owner::Unknown
        );
        assert!(!git_container(None, GitScope::PreviewLegacy, None, None).may_delete());
        // A caller that cannot read the container's port decides nothing.
        assert_eq!(
            git_container(None, GitScope::PreviewLegacy, Some(8001), None),
            Owner::Unknown
        );
        // A port that disagrees is positive evidence of the wrong container.
        assert_eq!(
            git_container(None, GitScope::PreviewLegacy, Some(8001), Some(7003)),
            Owner::Unknown
        );
        // Agreement is the proof: Docker will not bind one host port twice.
        assert_eq!(
            git_container(None, GitScope::PreviewLegacy, Some(8001), Some(8001)),
            Owner::Ours
        );
    }

    #[test]
    fn each_space_accepts_its_own_label() {
        let d = labelled(&[(GIT_KIND_LABEL, "deploy")]);
        let p = labelled(&[(GIT_KIND_LABEL, "preview")]);
        assert_eq!(git_container(Some(&d), GitScope::Deploy, None, None), Owner::Ours);
        assert_eq!(git_container(Some(&p), GitScope::Preview, None, None), Owner::Ours);
        assert_eq!(git_container(Some(&p), GitScope::Deploy, None, None), Owner::Theirs);
        // A labelled preview in the legacy space is unambiguous without a port.
        assert_eq!(
            git_container(Some(&p), GitScope::PreviewLegacy, None, None),
            Owner::Ours
        );
    }

    #[test]
    fn the_blue_stand_in_space_cannot_hold_a_real_deployment() {
        // The defect: `is_valid_name` accepts `api-blue`, so the app whose
        // container is `dockpanel-app-api-blue` WAS the stand-in name for the
        // app `api`, and was force-removed every time `api` was updated.
        assert_eq!(blue_name("dockpanel-app-api"), "dockpanel-app-api.blue");
        assert_ne!(blue_name("dockpanel-app-api"), "dockpanel-app-api-blue");
        // Nothing the panel will name can land in that space.
        assert!(!crate::routes::is_valid_name("api.blue"));
        assert!(crate::routes::is_valid_name("api-blue"));
    }

    #[test]
    fn an_operators_own_container_is_not_ours_to_force_remove() {
        // Docker accepts a hand-typed `.` even though the panel does not.
        assert_eq!(blue_leftover(None), Owner::Unknown);
        assert!(!blue_leftover(None).may_delete());
        let handmade = labelled(&[("com.example.stack", "mine")]);
        assert!(!blue_leftover(Some(&handmade)).may_delete());
        let ours = labelled(&[("dockpanel.managed", "true")]);
        assert!(blue_leftover(Some(&ours)).may_delete());
    }

    #[test]
    fn an_older_panel_that_sends_no_scope_still_means_deploy() {
        assert_eq!(GitScope::from_wire("deploy"), GitScope::Deploy);
        assert_eq!(GitScope::from_wire(""), GitScope::Deploy);
        assert_eq!(GitScope::from_wire("nonsense"), GitScope::Deploy);
        assert_eq!(GitScope::from_wire("preview"), GitScope::Preview);
        assert_eq!(GitScope::from_wire("preview_legacy"), GitScope::PreviewLegacy);
        // The legacy space addresses the OLD name, not the scoped one — that is
        // the entire reason it exists.
        assert_eq!(GitScope::PreviewLegacy.scoped("a-pr-b"), "a-pr-b");
    }
}
