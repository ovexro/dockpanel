//! Opening ports in the firewall this box is actually enforcing with.
//!
//! The agent only ever spoke `ufw`. On the RHEL family the running firewall is
//! **firewalld**, so every `ufw allow` was either a no-op or landed in a
//! rule set nothing was consulting — while the caller logged success anyway
//! (s265). `setup.sh` has the same distinction as `FW_MGR`; this is its
//! agent-side counterpart.

use crate::safe_cmd::safe_command;
use std::time::Duration;
use tokio::sync::OnceCell;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Firewall {
    Firewalld,
    Ufw,
    /// No firewall is running. Opening a port is vacuously fine — the port is
    /// already reachable — so callers should treat this as success, not
    /// failure, or they will warn about a problem the box does not have.
    None,
}

static FW: OnceCell<Firewall> = OnceCell::const_new();

/// Detected once per process. A firewall can in principle be installed while
/// the agent runs, but every caller here is a service installer, and the
/// alternative is re-probing on every port.
pub async fn detect() -> Firewall {
    *FW.get_or_init(|| async {
        // firewalld first: when both are present it is the one whose nftables
        // rules the kernel is actually consulting on these distros.
        if run("firewall-cmd", &["--state"]).await {
            Firewall::Firewalld
        } else if run("ufw", &["status"]).await {
            Firewall::Ufw
        } else {
            Firewall::None
        }
    })
    .await
}

/// Open `port`/tcp. Returns whether the port is now allowed.
///
/// Unlike the code this replaces, the result is **returned** rather than
/// discarded — a caller that logs "ports opened" without checking is stating
/// something it does not know.
pub async fn allow_tcp(port: &str) -> bool {
    match detect().await {
        Firewall::Firewalld => {
            let spec = format!("{port}/tcp");
            // --permanent survives reboots but does nothing until reloaded;
            // the plain call applies now but is lost on restart. Both, then.
            let permanent = run("firewall-cmd", &["--permanent", "--add-port", &spec]).await;
            let runtime = run("firewall-cmd", &["--add-port", &spec]).await;
            permanent && runtime
        }
        Firewall::Ufw => run("ufw", &["allow", &format!("{port}/tcp")]).await,
        Firewall::None => true,
    }
}

/// Apply staged firewalld rules. No-op elsewhere.
pub async fn reload() {
    if detect().await == Firewall::Firewalld {
        let _ = run("firewall-cmd", &["--reload"]).await;
    }
}

/// Open `port`/`proto` — like [`allow_tcp`] but not tcp-only, for the
/// Security page's own "add rule" feature rather than a service installer.
pub async fn add_port(port: &str, proto: &str) -> bool {
    match detect().await {
        Firewall::Firewalld => {
            let spec = format!("{port}/{proto}");
            let permanent = run("firewall-cmd", &["--permanent", "--add-port", &spec]).await;
            let runtime = run("firewall-cmd", &["--add-port", &spec]).await;
            permanent && runtime
        }
        Firewall::Ufw => run("ufw", &["allow", &format!("{port}/{proto}")]).await,
        Firewall::None => true,
    }
}

/// Close a `port`/`proto` previously opened with [`add_port`].
pub async fn remove_port(port: &str, proto: &str) -> bool {
    match detect().await {
        Firewall::Firewalld => {
            let spec = format!("{port}/{proto}");
            let permanent = run("firewall-cmd", &["--permanent", "--remove-port", &spec]).await;
            let runtime = run("firewall-cmd", &["--remove-port", &spec]).await;
            permanent && runtime
        }
        Firewall::Ufw => run("ufw", &["delete", "allow", &format!("{port}/{proto}")]).await,
        Firewall::None => true,
    }
}

/// Remove a firewalld service entry (e.g. `ssh`, `http`) from the default zone.
pub async fn remove_service(name: &str) -> bool {
    if detect().await != Firewall::Firewalld {
        return false;
    }
    let permanent = run("firewall-cmd", &["--permanent", "--remove-service", name]).await;
    let runtime = run("firewall-cmd", &["--remove-service", name]).await;
    permanent && runtime
}

/// The canonical `firewall-cmd` rich-rule spec for a port-scoped allow/deny,
/// optionally source-restricted — firewalld's equivalent of a ufw rule with
/// a `deny` action or a `from <source>` clause, neither of which a plain
/// `--add-port` can express. Family is inferred from whether `from` looks
/// like an IPv6 address (contains `:`) — the same shape `security.rs`'s own
/// source-address validator already accepts for both address families.
pub fn rich_rule_spec(port: &str, proto: &str, action: &str, from: Option<&str>) -> String {
    let verdict = if action.eq_ignore_ascii_case("deny") { "reject" } else { "accept" };
    match from {
        Some(src) => {
            let family = if src.contains(':') { "ipv6" } else { "ipv4" };
            format!(
                r#"rule family="{family}" source address="{src}" port port="{port}" protocol="{proto}" {verdict}"#
            )
        }
        None => format!(r#"rule family="ipv4" port port="{port}" protocol="{proto}" {verdict}"#),
    }
}

/// Add a rich rule built from [`rich_rule_spec`]. `--permanent` + runtime,
/// same belt-and-suspenders as every other mutation in this module.
pub async fn add_rich_rule(port: &str, proto: &str, action: &str, from: Option<&str>) -> bool {
    add_rich_rule_raw(&rich_rule_spec(port, proto, action, from)).await
}

pub async fn add_rich_rule_raw(spec: &str) -> bool {
    if detect().await != Firewall::Firewalld {
        return false;
    }
    let permanent = run("firewall-cmd", &["--permanent", "--add-rich-rule", spec]).await;
    let runtime = run("firewall-cmd", &["--add-rich-rule", spec]).await;
    permanent && runtime
}

/// Remove a rich rule by its exact spec string — pass back the line
/// `--list-rich-rules` itself reported (not a rebuilt string) when removing
/// an existing rule, since firewalld's rich-rule parser must resolve the
/// text to the same internal rule it stored.
pub async fn remove_rich_rule_raw(spec: &str) -> bool {
    if detect().await != Firewall::Firewalld {
        return false;
    }
    let permanent = run("firewall-cmd", &["--permanent", "--remove-rich-rule", spec]).await;
    let runtime = run("firewall-cmd", &["--remove-rich-rule", spec]).await;
    permanent && runtime
}

/// Open several ports and report which ones failed, so the caller can say
/// something true about the outcome.
pub async fn allow_tcp_ports(ports: &[&str]) -> Vec<String> {
    let mut failed = Vec::new();
    for p in ports {
        if !allow_tcp(p).await {
            failed.push((*p).to_string());
        }
    }
    reload().await;
    failed
}

async fn run(bin: &str, args: &[&str]) -> bool {
    tokio::time::timeout(
        Duration::from_secs(60),
        safe_command(bin).args(args).output(),
    )
    .await
    .ok()
    .and_then(|r| r.ok())
    .map(|o| o.status.success())
    .unwrap_or(false)
}
