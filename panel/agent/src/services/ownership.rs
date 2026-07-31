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
/// `create_app_service` writes `WorkingDirectory=/var/www/{domain}/public` and
/// `Description=DockPanel App: {domain}`; either identifies the owner, and a
/// hand-edited description should not be able to make the unit undeletable, so
/// both are accepted.
pub fn systemd_unit(path: &str, domain: &str) -> Owner {
    owner_by_lines(
        path,
        &[
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
}
