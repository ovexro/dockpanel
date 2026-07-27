//! The agent's own systemd unit — one source, reconciled at startup.
//!
//! # Why this module exists
//!
//! `panel/agent/dockpanel-agent.service` is the unit. `setup.sh` and `update.sh`
//! deploy it by copying that exact file, and both derive their pre-`mkdir` list
//! from its `ReadWritePaths=` line rather than mirroring it (s269). A third
//! installer did not: `install-agent.sh` hand-wrote its own unit into a
//! heredoc, and that copy — the one every fleet member in existence is running —
//! set systemd's four headline protection switches — new-privileges, system,
//! home and private-tmp — to off, declared no writable path list whatsoever, and
//! omitted eight further hardening directives the real unit sets, under a
//! comment reading "Create systemd service (matching local agent hardening)".
//! It matched nothing. Every remote box the panel manages ran the agent with no
//! sandbox from s253 to s271.
//!
//! The disabled values are described here rather than quoted, deliberately:
//! `tests/sandbox-paths-pin-e2e.sh` greps the installer sources for them, and a
//! check that matches its own explanation cannot fail.
//!
//! # Why fixing install-agent.sh is not enough (lesson #97a)
//!
//! `install-agent.sh` runs exactly once, at install time, and is served *by the
//! panel* out of the frontend root. Changing it changes what a **new** fleet
//! member gets and nothing else, and `agent-self-update.sh` swaps only the
//! binary — it has never touched the unit. So a corrected installer would have
//! reached no box that already exists, which is precisely the shape that kept
//! the Roundcube fix from arriving for seven sessions.
//!
//! The delivery mechanism that *does* already reach every box is the binary
//! itself, on a six-hourly self-update. So the unit rides the binary:
//! [`AGENT_UNIT`] is `include_str!`'d from the same file the installers copy —
//! the discipline `agent-self-update.sh` states in its own header, "compiled
//! into dockpanel-agent so it cannot drift from the binary that invokes it" —
//! and [`heal_agent_unit`] reconciles the on-disk unit against it at startup.
//!
//! # Why this cannot brick the fleet
//!
//! Writing a hardened unit over an unhardened one is the one change here that
//! could take a whole fleet offline at its next restart, so three things guard
//! it, in order:
//!
//! 1. **The directories are created first.** An UNPREFIXED `ReadWritePaths=`
//!    entry that does not exist fails the namespace mount and the unit does not
//!    start at all (`/etc/apt` did exactly this to every RPM-family box, s264).
//!    `/etc/nginx` is unprefixed and an agent-only box has no nginx, so the
//!    list is derived and created before the unit is swapped — never mirrored.
//! 2. **PID1 is asked whether it accepted the file.** After `daemon-reload`,
//!    `systemctl show -p LoadError` reports what systemd actually thinks of the
//!    unit rather than what we hope; a non-empty `LoadError` restores the
//!    previous bytes verbatim and reloads again.
//! 3. **Nothing is restarted here.** The new sandbox applies at the next start,
//!    which the self-update performs anyway. An agent that restarted itself out
//!    of its own cgroup would be unable to observe its own failure.

use crate::safe_cmd::safe_command;
use std::path::Path;

/// The canonical unit, compiled in from the file the installers deploy.
///
/// **The single source of this unit's contents.** Nothing else may spell it
/// out — `install-agent.sh` doing so for eighteen releases is the whole reason
/// this module exists.
pub const AGENT_UNIT: &str = include_str!("../../dockpanel-agent.service");

/// Where systemd reads the agent's unit from on every install path.
const UNIT_PATH: &str = "/etc/systemd/system/dockpanel-agent.service";

/// Directories the unit's `ReadWritePaths=` names, with any `-` prefix stripped.
///
/// Mirrors `agent_rwp_dirs()` in `setup.sh`/`update.sh`, against the same source
/// text, so the three installers cannot disagree about which directories a
/// working sandbox needs.
pub fn read_write_paths(unit: &str) -> Vec<&str> {
    unit.lines()
        .filter_map(|l| l.strip_prefix("ReadWritePaths="))
        .flat_map(|v| v.split_whitespace())
        .map(|p| p.strip_prefix('-').unwrap_or(p))
        .filter(|p| p.starts_with('/'))
        .collect()
}

/// Bring the on-disk unit up to the compiled-in one.
///
/// Runs at agent startup, so an ordinary binary update is all it takes — which
/// is the point: this is the only path that reaches a box installed by an older
/// `install-agent.sh`. No-op when the bytes already match, which is every panel
/// box (`setup.sh` copies the same file) and every fleet member from the second
/// boot onward.
pub async fn heal_agent_unit() {
    // Only ever reconcile a unit that is already there. Writing one where none
    // exists would mean installing the agent as a service on a box that chose
    // to run it some other way.
    if !Path::new(UNIT_PATH).exists() {
        return;
    }
    let Ok(current) = std::fs::read_to_string(UNIT_PATH) else {
        return;
    };
    if current == AGENT_UNIT {
        return;
    }

    // Guard 1 — the namespace mount. Must happen BEFORE the unit is swapped:
    // an unprefixed entry that is missing at start makes the agent unstartable,
    // and by then there is no agent left to fix it.
    for dir in read_write_paths(AGENT_UNIT) {
        if !Path::new(dir).exists() {
            if let Err(e) = std::fs::create_dir_all(dir) {
                tracing::warn!("agent unit heal: could not create {dir}: {e} — not swapping the unit");
                return;
            }
        }
    }

    if let Err(e) = std::fs::write(UNIT_PATH, AGENT_UNIT) {
        tracing::warn!("agent unit heal: could not write {UNIT_PATH}: {e}");
        return;
    }
    let _ = safe_command("systemctl").arg("daemon-reload").output().await;

    // Guard 2 — ask PID1 what it made of the file, rather than assuming the
    // write succeeded means the unit is loadable.
    let load_error = safe_command("systemctl")
        .args(["show", "dockpanel-agent", "-p", "LoadError", "--value"])
        .output()
        .await
        .ok()
        .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string())
        .unwrap_or_default();

    if !load_error.is_empty() {
        tracing::error!(
            "agent unit heal: systemd rejected the new unit ({load_error}) — restoring the previous one"
        );
        let _ = std::fs::write(UNIT_PATH, &current);
        let _ = safe_command("systemctl").arg("daemon-reload").output().await;
        return;
    }

    // Guard 3 — deliberately no restart. See the module header.
    tracing::info!(
        "Healed {UNIT_PATH} to the compiled-in unit (sandbox + ReadWritePaths); \
         it takes effect at the next agent restart"
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The hardening the fleet spent eighteen releases without. Spelled here as
    /// separate needles rather than as a copy of the unit, so this test pins the
    /// PROPERTY and not the file's current formatting.
    #[test]
    fn compiled_unit_is_the_hardened_one() {
        for directive in [
            "NoNewPrivileges=yes",
            "ProtectSystem=strict",
            "ProtectHome=yes",
            "PrivateTmp=yes",
            "ProtectKernelTunables=yes",
            "ProtectControlGroups=yes",
            "RestrictSUIDSGID=yes",
            "LockPersonality=yes",
            "RestrictNamespaces=~user",
        ] {
            assert!(
                AGENT_UNIT.contains(directive),
                "the compiled-in unit lost {directive} — install-agent.sh's unit \
                 disabled exactly these, and this module exists to stop that shipping again"
            );
        }
        assert!(
            !AGENT_UNIT.contains("ProtectSystem=no"),
            "the compiled-in unit turns the sandbox off"
        );
    }

    /// A fleet member keeps its token and central URL in agent.env; a panel box
    /// has no such file. The `-` prefix is what lets one unit serve both, and
    /// dropping it would make the panel box refuse to start.
    #[test]
    fn env_file_is_optional_not_required() {
        assert!(
            AGENT_UNIT.contains("EnvironmentFile=-/etc/dockpanel/agent.env"),
            "a fleet member reads AGENT_TOKEN/DOCKPANEL_CENTRAL_URL from agent.env, \
             and the `-` prefix is what keeps the panel box (which has no such file) starting"
        );
    }

    #[test]
    fn read_write_paths_strips_the_optional_prefix() {
        let paths = read_write_paths(AGENT_UNIT);
        assert!(paths.contains(&"/etc/nginx"), "unprefixed entries must be listed");
        assert!(
            paths.contains(&"/etc/apt"),
            "a `-`-prefixed entry still needs its directory created; the prefix \
             only means systemd tolerates it missing at start"
        );
        assert!(
            !paths.iter().any(|p| p.starts_with('-')),
            "the `-` prefix must be stripped before these are handed to mkdir"
        );
    }
}
