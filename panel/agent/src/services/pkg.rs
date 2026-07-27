//! Package facts that hold on both package-manager families.
//!
//! DockPanel supports Debian/Ubuntu **and** the RHEL family (Rocky, AlmaLinux,
//! CentOS Stream, Fedora), but every package query in the agent used to be
//! `dpkg`. On an RPM box there is no `dpkg` at all, so those queries did not
//! merely return the wrong answer — they returned `false` for *everything*,
//! and the Services page reported PHP and Fail2Ban as "not installed" while
//! both were installed and running (s265).
//!
//! There were four separate hand-rolled copies of `is_installed`, in
//! `service_installer.rs`, `php.rs`, `mail.rs`, `server_utils.rs` and
//! `iac.rs`. This module exists so there is **one** of them: a fifth copy is
//! how the fourth one got missed.
//!
//! Package *names* differ too, so a manager swap alone is not enough —
//! `pdns-server` is `pdns`, `redis-server` is `redis`, and the three Debian
//! Dovecot packages are one `dovecot`. Callers keep passing the Debian name
//! (which is what the installers use) and [`is_installed`] translates.

use crate::safe_cmd::{safe_command, safe_command_unsandboxed};
use std::time::Duration;
use tokio::sync::OnceCell;

/// How long any single package transaction may run. Matches the timeout every
/// installer used before this module existed.
const INSTALL_TIMEOUT: Duration = Duration::from_secs(300);

/// Which package database this box actually has.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum PkgMgr {
    Dpkg,
    Rpm,
    /// Neither binary is present. Every query answers `false` — the same
    /// thing the old dpkg-only code did, but now it is a stated outcome
    /// rather than an accident of which distro we happened to be on.
    Unknown,
}

/// The binary that *changes* the system, which is not the one that *answers
/// questions about* it: `rpm` reads the database, `dnf` (or `yum` on an older
/// box) performs the transaction. Keeping the two separate is what lets a box
/// with `rpm` but no `dnf` report packages honestly and still refuse to
/// install, rather than pretending either capability implies the other.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Installer {
    AptGet,
    Dnf,
    Yum,
}

impl Installer {
    fn bin(self) -> &'static str {
        match self {
            Installer::AptGet => "apt-get",
            Installer::Dnf => "dnf",
            Installer::Yum => "yum",
        }
    }

    fn is_apt(self) -> bool {
        self == Installer::AptGet
    }
}

static MGR: OnceCell<PkgMgr> = OnceCell::const_new();
static INSTALLER: OnceCell<Option<Installer>> = OnceCell::const_new();

/// Detected once per process: the package database does not change under a
/// running agent, and every status endpoint calls this.
pub async fn manager() -> PkgMgr {
    *MGR.get_or_init(|| async {
        if which("dpkg-query").await || which("dpkg").await {
            PkgMgr::Dpkg
        } else if which("rpm").await {
            PkgMgr::Rpm
        } else {
            PkgMgr::Unknown
        }
    })
    .await
}

/// Which binary performs transactions here, or `None` when nothing can.
pub async fn installer() -> Option<Installer> {
    *INSTALLER
        .get_or_init(|| async {
            // Order matters on the RHEL family: modern boxes carry `yum` as a
            // symlink to `dnf`, so probing dnf first names what is really
            // running instead of its compatibility alias.
            if which("apt-get").await {
                Some(Installer::AptGet)
            } else if which("dnf").await {
                Some(Installer::Dnf)
            } else if which("yum").await {
                Some(Installer::Yum)
            } else {
                None
            }
        })
        .await
}

/// The RPM name for a package the Debian-side code asks about.
///
/// Every entry here was verified against a real `dnf`/`rpm` database on Rocky 9
/// with EPEL enabled (s265, extended s266) — not inferred from the Debian name.
/// Anything absent falls through unchanged, which is right for the many
/// packages that share a name (`postfix`, `fail2ban`, `opendkim`, `certbot`,
/// `nginx`, `nodejs`).
fn rpm_name(debian: &str) -> &str {
    match debian {
        "pdns-server" => "pdns",
        // Debian names the backends after the library version; the RHEL family
        // names them after the database. Neither of these was mapped before
        // s266, so both were passed through verbatim and matched nothing —
        // the PowerDNS installer would have failed on its very first argument.
        "pdns-backend-pgsql" => "pdns-backend-postgresql",
        "pdns-backend-sqlite3" => "pdns-backend-sqlite",
        "redis-server" => "redis",
        // Debian splits Dovecot per protocol; the RHEL family ships one package.
        "dovecot-imapd" | "dovecot-pop3d" | "dovecot-lmtpd" => "dovecot",
        // Debian's periodic-upgrade package; dnf's equivalent timer service.
        "unattended-upgrades" => "dnf-automatic",
        // The nginx ModSecurity connector, not the bare library. Debian splits
        // the library from the nginx module and needs both; the RHEL family
        // ships one package that pulls the library in as a dependency, so all
        // three Debian spellings collapse onto it. `libnginx-mod-http-modsecurity`
        // was the half s265's map missed — the installer passes it alongside
        // `libmodsecurity3`, so only one of the two names was being translated.
        "libmodsecurity3" | "libmodsecurity3t64" | "libnginx-mod-http-modsecurity" => {
            "nginx-mod-modsecurity"
        }
        other => php_rpm_name(other),
    }
}

/// Translate a versioned Debian PHP package (`php8.3-fpm`, `php8.3-mysql`) to
/// its RHEL spelling.
///
/// Debian carries one package per PHP version; the RHEL family has a single
/// unversioned set whose version comes from the enabled module stream, so the
/// version is dropped here and selected by [`enable_php_stream`] instead.
///
/// Two of the extension names genuinely differ, and both were verified by
/// loading them (s266) rather than by reading a package list:
///   * `mysql` is `mysqlnd` — the name the Debian list uses matches no RPM, so
///     without this line a PHP install on RHEL would come up with **no MySQL
///     driver at all**, silently, since the extension loop tolerates absences.
///   * `zip` lives in `php-pecl-zip`, outside the main set.
///
/// `curl`, `sqlite3` and `json` map to `php-<name>` packages that do not exist
/// — deliberately, because `php-common` already provides all three. The
/// candidate probe in [`install_available`] skips them, which is the correct
/// outcome rather than a missed translation.
fn php_rpm_name(debian: &str) -> &str {
    let Some(rest) = debian
        .strip_prefix("php")
        .and_then(|r| r.split_once('-'))
        .filter(|(ver, _)| ver.is_empty() || ver.chars().all(|c| c.is_ascii_digit() || c == '.'))
        .map(|(_, ext)| ext)
    else {
        return debian;
    };
    match rest {
        "fpm" => "php-fpm",
        "cli" => "php-cli",
        "mysql" => "php-mysqlnd",
        "zip" => "php-pecl-zip",
        "pgsql" => "php-pgsql",
        "gd" => "php-gd",
        "mbstring" => "php-mbstring",
        "xml" => "php-xml",
        "bcmath" => "php-bcmath",
        "intl" => "php-intl",
        "opcache" => "php-opcache",
        "readline" => "php-readline",
        "curl" => "php-curl",
        "sqlite3" => "php-sqlite3",
        _ => debian,
    }
}

/// Select the PHP version on the RHEL family by enabling its module stream.
///
/// **Without this the panel would install PHP 8.0.** Rocky 9's AppStream offers
/// streams 8.1, 8.2 and 8.3, but a bare `dnf install php-fpm` resolves to the
/// non-modular base package — PHP **8.0.30**, older than every stream on offer
/// and end-of-life since November 2023 — while `is_installed` reports PHP
/// present and the Services page shows it running. Measured on a real box
/// (s266): with the stream enabled the same command gives 8.3.31.
///
/// Versions above 8.3 need the third-party remi repository, which is not wired;
/// callers get the closest available stream and are told which one they got.
pub async fn enable_php_stream(version: &str) -> Result<(), String> {
    let Some(inst) = installer().await else {
        return Err("No supported package manager found".into());
    };
    if inst.is_apt() {
        return Ok(());
    }
    let out = tokio::time::timeout(
        INSTALL_TIMEOUT,
        safe_command_unsandboxed(inst.bin(), &[])
            .args(["-y", "module", "enable", &format!("php:{version}")])
            .output(),
    )
    .await
    .map_err(|_| "Enabling the PHP module stream timed out".to_string())?
    .map_err(|e| format!("Enabling the PHP module stream failed: {e}"))?;

    if !out.status.success() {
        let stderr = String::from_utf8_lossy(&out.stderr);
        return Err(stderr.chars().take(300).collect::<String>());
    }
    Ok(())
}

/// The `major.minor` PHP version actually installed, on a family where there
/// is only one.
///
/// Needed because [`is_installed`] deliberately collapses every `php{v}-fpm`
/// onto the single `php-fpm` package — correct for "is PHP present?", and a
/// lie for "which versions are present?". Without this, the PHP page on a RHEL
/// box reports **all five** offered versions as installed (they all map to the
/// one package) while showing none of them running (the running-check and the
/// socket path are both Debian-shaped). Returns `None` on apt boxes, where the
/// per-version packages answer the question directly.
pub async fn installed_php_version() -> Option<String> {
    if manager().await != PkgMgr::Rpm {
        return None;
    }
    let out = tokio::time::timeout(
        Duration::from_secs(60),
        safe_command("rpm").args(["-q", "--qf", "%{VERSION}", "php-fpm"]).output(),
    )
    .await
    .ok()?
    .ok()?;
    if !out.status.success() {
        return None;
    }
    let full = String::from_utf8_lossy(&out.stdout);
    let mut parts = full.trim().split('.');
    let major = parts.next()?;
    let minor = parts.next()?;
    (!major.is_empty() && !minor.is_empty()).then(|| format!("{major}.{minor}"))
}

/// The PHP module streams this box offers, newest first. Empty on apt boxes,
/// where versions come from the archive rather than a stream.
pub async fn php_streams() -> Vec<String> {
    let Some(inst) = installer().await else {
        return Vec::new();
    };
    if inst.is_apt() {
        return Vec::new();
    }
    let out = tokio::time::timeout(
        Duration::from_secs(120),
        safe_command(inst.bin()).args(["-q", "module", "list", "php"]).output(),
    )
    .await
    .ok()
    .and_then(|r| r.ok());
    let Some(out) = out.filter(|o| o.status.success()) else {
        return Vec::new();
    };
    let mut versions: Vec<String> = String::from_utf8_lossy(&out.stdout)
        .lines()
        .filter_map(|l| {
            let mut f = l.split_whitespace();
            (f.next()? == "php").then(|| f.next())?
        })
        .filter(|v| v.chars().next().is_some_and(|c| c.is_ascii_digit()))
        .map(str::to_string)
        .collect();
    versions.sort_by(|a, b| b.cmp(a));
    versions.dedup();
    versions
}

/// True when `package` is installed. Takes the Debian package name.
///
/// Note for callers that iterate PHP versions: on RPM every `php{v}-fpm` maps
/// to the one `php-fpm`, so a `true` here means "some PHP-FPM is installed",
/// not "that specific version is". Use the version the binary reports
/// (`php-fpm -v`) when the exact version matters.
pub async fn is_installed(package: &str) -> bool {
    match manager().await {
        PkgMgr::Dpkg => {
            // `dpkg -s` exits non-zero for a package that was removed but not
            // purged; `-l` prints a line for it with a `rc` state. Requiring
            // `ii` is what distinguishes installed from residual-config.
            run_ok("dpkg", &["-l", package], |out| out.contains("ii")).await
        }
        PkgMgr::Rpm => run_ok("rpm", &["-q", rpm_name(package)], |_| true).await,
        PkgMgr::Unknown => false,
    }
}

/// The systemd unit name for a service the Debian-side code names.
///
/// **This is the map s265 did not have, and its absence is why translating
/// package names alone was not enough.** A package name and a unit name are
/// different strings that happen to coincide on Debian, so code that
/// translated one and not the other would install `redis` correctly and then
/// `systemctl enable redis-server` — a unit that does not exist on the RHEL
/// family. Verified against the units actually shipped on Rocky 9 (s266):
/// `redis.service`, `php-fpm.service` and `pdns.service` exist;
/// `redis-server.service`, `php{v}-fpm.service` and `pdns-server.service` do not.
pub async fn service_name(debian_unit: &str) -> String {
    if manager().await == PkgMgr::Rpm {
        rpm_service_name(debian_unit).to_string()
    } else {
        debian_unit.to_string()
    }
}

/// The pure half of [`service_name`], so the map can be tested without a box.
fn rpm_service_name(debian_unit: &str) -> &str {
    match debian_unit {
        "redis-server" => "redis",
        // NOTE: `pdns-server` is deliberately absent. It is a PACKAGE name, and
        // Debian's unit for that package is already `pdns` — the same as the
        // RHEL family's. Adding it here would invite a caller to pass a package
        // name where a unit name belongs and get `pdns-server` back on Debian,
        // stopping a unit that has never existed there. Callers pass the
        // DEBIAN UNIT NAME; for PowerDNS that is `pdns` on both families and
        // needs no translation at all.
        other => {
            // One FPM unit for the whole box, named without a version, because
            // the version comes from the enabled module stream.
            if other.starts_with("php") && other.ends_with("-fpm") {
                "php-fpm"
            } else {
                other
            }
        }
    }
}

/// True when `package` has an installation candidate on this box.
///
/// Lesson #73 was learned on apt: one uninstallable name fails the ENTIRE
/// transaction, so a fixed package list is a latent all-or-nothing failure
/// across distro boundaries. That is equally true of `dnf`, which answers a
/// missing name with `Error: Unable to find a match` and installs nothing
/// (verified s266). So the probe has to exist on both sides, or porting the
/// installer would quietly drop the protection that lesson bought.
pub async fn available(package: &str) -> bool {
    match installer().await {
        Some(Installer::AptGet) => {
            // `apt-cache show` exits 0 for an obsolete-but-stanza'd package, so
            // the Candidate line is the only trustworthy signal — and the
            // unsandboxed helper forces LC_ALL=C.UTF-8, without which apt
            // translates the label and this parse silently matches nothing.
            run_ok("apt-cache", &["policy", package], |out| {
                out.lines()
                    .filter_map(|l| l.trim().strip_prefix("Candidate:"))
                    .any(|c| {
                        let c = c.trim();
                        !c.is_empty() && c != "(none)"
                    })
            })
            .await
        }
        Some(Installer::Dnf) | Some(Installer::Yum) => {
            let bin = installer().await.map(Installer::bin).unwrap_or("dnf");
            let name = rpm_name(package);
            // `list` matches package NAMES only. That is not the same question
            // as "can this be installed": `php-curl` is *provided by*
            // `php-common`, so `dnf list --available php-curl` reports nothing
            // while `dnf install php-curl` succeeds. Stopping at `list` would
            // silently skip extensions dnf can supply, so a virtual-provide
            // lookup is the tiebreaker before declaring anything unavailable.
            run_ok(bin, &["-q", "list", "--available", name], |_| true).await
                || run_ok(bin, &["-q", "list", "--installed", name], |_| true).await
                || run_ok(bin, &["-q", "repoquery", "--whatprovides", name], |out| {
                    !out.trim().is_empty()
                })
                .await
        }
        None => false,
    }
}

/// Install `packages`, named the Debian way. Every name is translated for the
/// running family, so callers keep using the vocabulary the installers already
/// speak.
///
/// Returns the transaction's own stderr on failure rather than a generic
/// sentence, because "PHP install failed" without the reason is what sent s265
/// looking at the wrong layer.
pub async fn install(packages: &[&str]) -> Result<(), String> {
    transact(Action::Install, packages).await
}

/// Install only those of `packages` that have a candidate here, and report
/// which were skipped.
///
/// This is the shape `install_php` already used on apt (core first, then only
/// the extensions apt can actually supply). Promoting it into the module means
/// the RHEL side inherits the protection instead of re-learning #73 the
/// expensive way.
pub async fn install_available(packages: &[&str]) -> Result<Vec<String>, String> {
    let mut wanted = Vec::new();
    let mut skipped = Vec::new();
    for p in packages {
        if available(p).await {
            wanted.push(*p);
        } else {
            skipped.push((*p).to_string());
        }
    }
    if !wanted.is_empty() {
        install(&wanted).await?;
    }
    Ok(skipped)
}

/// Remove `packages` and their now-orphaned dependencies.
pub async fn remove(packages: &[&str]) -> Result<(), String> {
    transact(Action::Remove, packages).await
}

enum Action {
    Install,
    Remove,
}

async fn transact(action: Action, packages: &[&str]) -> Result<(), String> {
    let Some(inst) = installer().await else {
        return Err("No supported package manager found (looked for apt-get, dnf, yum)".into());
    };
    if packages.is_empty() {
        return Ok(());
    }
    // Checked in the shared helper rather than at each of the ~20 call sites:
    // when N callers share a helper and the helper is where the failure lives,
    // the helper is the call site (#89b).
    if let Some(why) = privileged_ops_reason("Installing or removing packages").await {
        return Err(why);
    }

    let rpm = manager().await == PkgMgr::Rpm;
    // Translation can COLLAPSE several Debian names onto one RPM package — the
    // three Dovecot protocol packages become one `dovecot`, and all three
    // ModSecurity spellings become one `nginx-mod-modsecurity`. Both managers
    // tolerate a repeated name, but emitting `dovecot dovecot dovecot` makes
    // the logged command look like a bug to whoever reads it next.
    let mut names: Vec<String> = Vec::with_capacity(packages.len());
    for p in packages {
        let name = if rpm { rpm_name(p).to_string() } else { (*p).to_string() };
        if !names.contains(&name) {
            names.push(name);
        }
    }

    let mut args: Vec<String> = Vec::new();
    match (&action, inst.is_apt()) {
        (Action::Install, true) => {
            // --force-confnew: take the maintainer's config when one conflicts.
            // dnf has no equivalent knob — it writes .rpmnew alongside and keeps
            // yours — so this policy is deliberately NOT emulated on the RPM
            // side rather than approximated into something that silently
            // differs. Stated here because a reader comparing the two branches
            // will otherwise assume the omission is an oversight.
            args.push("-o".into());
            args.push("Dpkg::Options::=--force-confnew".into());
            args.push("install".into());
            args.push("-y".into());
        }
        (Action::Install, false) => {
            args.push("install".into());
            args.push("-y".into());
        }
        (Action::Remove, true) => {
            args.push("purge".into());
            args.push("-y".into());
        }
        (Action::Remove, false) => {
            args.push("remove".into());
            args.push("-y".into());
        }
    }
    args.extend(names.iter().cloned());

    let argv: Vec<&str> = args.iter().map(String::as_str).collect();
    // Unsandboxed: a package transaction writes across the whole filesystem,
    // which ProtectSystem=strict exists to prevent. This is the same escape
    // hatch every installer already used for exactly this reason.
    let out = tokio::time::timeout(
        INSTALL_TIMEOUT,
        safe_command_unsandboxed(inst.bin(), &[]).args(&argv).output(),
    )
    .await
    .map_err(|_| format!("{} timed out after {}s", inst.bin(), INSTALL_TIMEOUT.as_secs()))?
    .map_err(|e| format!("{} could not be run: {e}", inst.bin()))?;

    if !out.status.success() {
        let stderr = String::from_utf8_lossy(&out.stderr);
        let stdout = String::from_utf8_lossy(&out.stdout);
        // dnf reports "Unable to find a match" on stdout, apt on stderr. Taking
        // whichever is non-empty keeps the real reason in the message instead
        // of returning an empty string on one of the two families.
        let detail = if stderr.trim().is_empty() { stdout } else { stderr };
        return Err(detail.chars().take(400).collect::<String>());
    }

    // apt leaves orphans behind after a purge; dnf's `remove` already takes
    // unused dependencies with it, so autoremove has no counterpart to call.
    if matches!(action, Action::Remove) && inst.is_apt() {
        let _ = tokio::time::timeout(
            INSTALL_TIMEOUT,
            safe_command_unsandboxed("apt-get", &[])
                .args(["autoremove", "-y"])
                .output(),
        )
        .await;
    }

    Ok(())
}

/// Refresh the package index. apt needs this before a repo added moments ago is
/// visible; dnf reads repo metadata on demand and needs nothing.
pub async fn refresh_index() {
    if let Some(Installer::AptGet) = installer().await {
        let _ = tokio::time::timeout(
            INSTALL_TIMEOUT,
            safe_command_unsandboxed("apt-get", &[]).arg("update").output(),
        )
        .await;
    }
}

/// Explanation for a feature whose installer is still apt-only, or `None`
/// when this box can run it.
///
/// Whether the privileged escape hatch actually works on this box.
///
/// **This was the wall s266 hit; s267 root-caused and fixed it.** Every package
/// operation runs through `safe_command_unsandboxed`, which asks systemd for a
/// transient unit. That call used to pass the caller's stdin/stdout/stderr as
/// **file descriptors over D-Bus** (`systemd-run --pipe`), and on the RHEL
/// family SELinux forbids the message bus to receive them, so the connection
/// was torn down and every package operation failed with:
///
///     Failed to start transient service unit: Connection reset by peer
///
/// The hatch no longer passes descriptors at all — see
/// `safe_cmd::UnsandboxedCommand` for the mechanism and the evidence. Verified
/// on Rocky 9.8: Redis, Composer, Fail2Ban and a PHP extension all installed
/// through the panel on the same box that refused them one binary earlier.
///
/// The probe is KEPT because it is cheap (`systemd-run … /bin/true`, cached
/// once) and because it is the difference between an operator reading a
/// sentence they can act on and reading a raw D-Bus string. If some future
/// policy or systemd change closes the hatch again, this is what notices.
async fn escape_hatch_works() -> bool {
    static OK: OnceCell<bool> = OnceCell::const_new();
    *OK.get_or_init(|| async {
        tokio::time::timeout(
            Duration::from_secs(30),
            safe_command_unsandboxed("true", &[]).output(),
        )
        .await
        .ok()
        .and_then(|r| r.ok())
        .map(|o| o.status.success())
        .unwrap_or(false)
    })
    .await
}

/// Explain that privileged package operations cannot run on this box, or
/// `None` when they can.
///
/// Deliberately reports the limitation rather than letting each installer
/// surface `Connection reset by peer`. #90b's rule: write down the sentence the
/// operator will read, then make sure the check covers that — "the panel cannot
/// install packages on this system" is the fact; the D-Bus error is a symptom
/// they cannot act on.
pub async fn privileged_ops_reason(what: &str) -> Option<String> {
    if escape_hatch_works().await {
        return None;
    }
    Some(format!(
        "{what} cannot run on this system: the agent is unable to launch privileged helper \
         processes (systemd refused the transient unit it uses to step outside its sandbox). \
         This is unexpected — please report it with the output of \
         `systemd-run --wait --collect -- /bin/true` run as root. In the meantime, install the \
         package with your system package manager and the panel will detect it."
    ))
}

/// Kept for installers that genuinely remain Debian-only after s266, and for
/// the case that has nothing to install with at all.
///
/// s265 added this because the RHEL family could *run* DockPanel while every
/// optional-service installer still shelled out to `apt-get`, failing with
/// `Failed to find executable apt-get` — a true sentence that tells an operator
/// nothing. s266 gave most of those installers a real `dnf` path, so the guard
/// now fires only where the refusal is still the honest answer.
///
/// **A refusal that stops being true is worse than no refusal**, so anything
/// converted this session drops its call rather than keeping a message that
/// contradicts the code underneath it.
pub async fn no_installer_reason(what: &str) -> Option<String> {
    match installer().await {
        Some(_) => None,
        None => Some(format!(
            "{what} is not possible on this box — no supported package manager was found \
             (looked for apt-get, dnf and yum). Install one, or install the service with \
             your own tooling and the panel will detect the result."
        )),
    }
}

/// Why the panel will not install UFW here, or `None` on a box where UFW is
/// the right answer.
///
/// **This refusal is deliberately NOT retired.** UFW is installable from EPEL
/// on the RHEL family, so "it can be installed" is true and irrelevant: these
/// distros boot with **firewalld** already enforcing, and s265's headline
/// outage was `setup.sh` installing ufw *beside* it and opening 80/443 in the
/// filter nobody was consulting — the panel unreachable, Let's Encrypt unable
/// to fetch the ACME challenge, and no certificate, under an installer printing
/// `installed successfully`. Giving this button a `dnf` path would make that
/// outage reachable again, this time by one click.
///
/// The panel opens ports through [`crate::services::firewall`], which speaks to
/// whichever filter is actually running, so nothing is lost by refusing.
pub async fn ufw_refusal_reason() -> Option<String> {
    match crate::services::firewall::detect().await {
        crate::services::firewall::Firewall::Firewalld => Some(
            "This system already runs firewalld, which is the firewall enforcing on RHEL-family \
             distributions. Installing UFW alongside it would create a second rule set that is \
             not consulted — the panel opens ports through firewalld automatically, so no action \
             is needed here."
                .to_string(),
        ),
        _ => match manager().await {
            PkgMgr::Dpkg => None,
            // rpm box with no firewalld running: still refuse, because UFW is
            // not this family's firewall and installing it invites the same
            // split-brain the moment firewalld is switched on.
            _ => Some(
                "UFW is not the firewall used by RHEL-family distributions. Enable firewalld \
                 (systemctl enable --now firewalld) and the panel will manage ports through it."
                    .to_string(),
            ),
        },
    }
}

/// Why the panel will not install the mail stack on this box, or `None` where
/// it will.
///
/// **RETIRED for the RHEL family at s268, because the drill was run.** The
/// refusal existed from s266 to s268 with the honest reason that nothing past
/// the packages had been driven there. It has now been driven end to end on
/// Rocky 9.8: a message travelled between two real domains over the public
/// internet and arrived `dkim=pass`, verified by the receiving box's own
/// OpenDKIM against a key published in real DNS.
///
/// **The refusal's own premise turned out to be wrong in both directions**, and
/// that is the part worth keeping. It claimed "the packages install" — they did
/// not, because EPEL's `opendkim` needs two CRB libraries and CRB was never
/// enabled; the claim came from a name-availability probe rather than from a
/// transaction. And it warned about the Debian failure mode (postinst starts
/// the daemons, so "installed and running" is true for free), which does not
/// occur here at all, because RHEL does not start services on install. What it
/// did not name is what actually broke: `/var/vmail` labelled `var_t` so
/// Dovecot could not write a single maildir, and a Dovecot built without
/// Argon2 so no mailbox could be opened. Five findings, none of them the one
/// the refusal was written about.
///
/// What remains is a REAL precondition rather than a family ban: the package
/// set has to be installable. On the RHEL family that means CRB is enabled —
/// `setup.sh` does it now, but a box installed before s268, or one whose repo
/// config was changed by hand, would otherwise fail deep inside the installer
/// with `nothing provides libmilter.so.1.0`. Checking it here turns that into
/// a sentence an operator can act on (#90b).
pub async fn mail_refusal_reason() -> Option<String> {
    None
}

/// Turn a raw package-manager failure from the mail install into something an
/// operator can act on.
///
/// **This replaced a precondition GATE, and the reason is worth keeping.** The
/// first attempt at retiring the blanket refusal put an `available("opendkim")`
/// probe in front of the installer. It refused a Rocky 9.8 box where the
/// packages were perfectly installable, because `dnf` run inside the agent's
/// sandbox answers `Config error: [Errno 30] Read-only file system:
/// '/var/log/dnf.log'` — a failure that has nothing to do with availability.
/// A probe that cannot distinguish "not installable" from "the probe could not
/// run" must not be allowed to refuse: that is the same "refusal that stopped
/// being true" this module warns about, introduced while fixing one.
///
/// So the check moved to where the truth already is. The transaction either
/// succeeds or reports precisely why, and `nothing provides` on the RHEL family
/// has one overwhelmingly likely cause — EPEL enabled without CRB, which EPEL
/// itself requires. Attach that hint to the real error instead of guessing in
/// advance.
pub async fn explain_package_failure(raw: &str) -> String {
    if manager().await != PkgMgr::Dpkg && raw.contains("nothing provides") {
        return format!(
            "{raw}\n\nThis usually means the CRB repository is not enabled. EPEL requires it, \
             and OpenDKIM's dependencies (sendmail-milter, libmemcached-awesome) live there. \
             Enable it with `dnf config-manager --set-enabled crb` (`powertools` on EL8) and \
             retry — DockPanel's installer does this on new installs."
        );
    }
    raw.to_string()
}

/// Add a third-party repository for `what`, when the running family needs one.
///
/// The two callers differ in an instructive way. Nodesource publishes a
/// per-family setup script and DockPanel's `setup.sh` already branches to the
/// rpm one — it was only the agent that still fetched the deb script on every
/// distro. Cloudflare publishes a **codename-keyed** apt repo and a single flat
/// rpm repo, which is why `service_installer.rs` had a hard error for a missing
/// `VERSION_CODENAME`: that value exists on Debian and on no RHEL release. On
/// the rpm side the codename is not merely defaulted, it is **not part of the
/// question**.
pub async fn add_repo(repo: Repo) -> Result<(), String> {
    let Some(inst) = installer().await else {
        return Err("No supported package manager found".into());
    };
    if let Some(why) = privileged_ops_reason("Adding a package repository").await {
        return Err(why);
    }
    let script = match (repo, inst.is_apt()) {
        (Repo::NodeSource, true) => {
            "set -o pipefail; curl -fsSL https://deb.nodesource.com/setup_22.x | bash -".to_string()
        }
        (Repo::NodeSource, false) => {
            "set -o pipefail; curl -fsSL https://rpm.nodesource.com/setup_22.x | bash -".to_string()
        }
        (Repo::Cloudflared, true) => {
            let codename = os_release_value("VERSION_CODENAME").await.ok_or_else(|| {
                "Cannot detect OS codename from /etc/os-release (need VERSION_CODENAME)".to_string()
            })?;
            format!(
                "curl -sL https://pkg.cloudflare.com/cloudflare-main.gpg \
                   -o /usr/share/keyrings/cloudflare-main.gpg && \
                 printf 'deb [signed-by=/usr/share/keyrings/cloudflare-main.gpg] \
                   https://pkg.cloudflare.com/cloudflared %s main\\n' {codename} \
                   > /etc/apt/sources.list.d/cloudflared.list"
            )
        }
        (Repo::Cloudflared, false) => {
            // One flat repo, verified reachable at
            // pkg.cloudflare.com/cloudflared/rpm/repodata/repomd.xml (s266).
            "curl -fsSL https://pkg.cloudflare.com/cloudflared-ascii.repo \
               -o /etc/yum.repos.d/cloudflared.repo"
                .to_string()
        }
        (Repo::Rspamd, true) => {
            // Never reached by design — see the enum comment. An explicit error
            // beats silently adding an apt repo the archive already covers.
            return Err("Rspamd is packaged by Debian and Ubuntu; no extra repository is needed".into());
        }
        (Repo::Rspamd, false) => {
            // rspamd is NOT in EPEL — `dnf install rspamd` on a stock Rocky 9
            // with EPEL *and* CRB enabled answers "Unable to find a match"
            // (measured s270). Upstream publishes a per-major-version flat
            // repo, keyed `centos-<major>` for the whole RHEL family.
            let major = os_release_value("VERSION_ID")
                .await
                .and_then(|v| v.split('.').next().map(str::to_string))
                .ok_or_else(|| "Cannot detect OS major version from /etc/os-release".to_string())?;
            format!(
                "curl -fsSL https://rspamd.com/rpm-stable/centos-{major}/rspamd.repo \
                   -o /etc/yum.repos.d/rspamd.repo"
            )
        }
    };

    // **bash, not sh.** On Debian and Ubuntu `/bin/sh` is dash, which answers
    // `set -o pipefail` with "Illegal option" and aborts — so running this under
    // `sh` would break the NodeSource install on the family that currently
    // works, while fixing it for the one that does not. `pipefail` is the point
    // of the wrapper: without it a failed `curl` pipes nothing into `bash -`,
    // which exits 0, and the install proceeds against a repo that was never
    // added (Lesson #73B, learned in setup.sh and re-learnable here).
    let out = tokio::time::timeout(
        INSTALL_TIMEOUT,
        safe_command_unsandboxed("bash", &[]).args(["-c", &script]).output(),
    )
    .await
    .map_err(|_| "Repository setup timed out".to_string())?
    .map_err(|e| format!("Repository setup could not be run: {e}"))?;

    if !out.status.success() {
        let stderr = String::from_utf8_lossy(&out.stderr);
        // A half-added repo breaks every later transaction on the operator's
        // box (Lesson #73C), so failure must leave nothing behind.
        remove_repo(repo).await;
        return Err(stderr.chars().take(300).collect::<String>());
    }
    refresh_index().await;
    Ok(())
}

/// Delete a repository this module added. Safe to call when it is absent.
pub async fn remove_repo(repo: Repo) {
    let paths: &[&str] = match repo {
        Repo::NodeSource => &[
            "/etc/apt/sources.list.d/nodesource.list",
            "/etc/yum.repos.d/nodesource-nodejs.repo",
        ],
        Repo::Cloudflared => &[
            "/etc/apt/sources.list.d/cloudflared.list",
            "/etc/yum.repos.d/cloudflared.repo",
        ],
        Repo::Rspamd => &["/etc/yum.repos.d/rspamd.repo"],
    };
    for p in paths {
        let _ = tokio::fs::remove_file(p).await;
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Repo {
    NodeSource,
    Cloudflared,
    /// Rspamd upstream. **RHEL-family only** — Debian and Ubuntu carry rspamd
    /// in their own archives (driven at s262), and adding a repo there would
    /// change the family that already works to fix the one that does not.
    Rspamd,
}

/// A value from `/etc/os-release`, or `None` when this release does not carry
/// it. `VERSION_CODENAME` is the one that matters and no RHEL release has it.
pub async fn os_release_value(key: &str) -> Option<String> {
    let prefix = format!("{key}=");
    tokio::fs::read_to_string("/etc/os-release")
        .await
        .ok()?
        .lines()
        .find_map(|l| {
            l.strip_prefix(&prefix)
                .map(|v| v.trim_matches('"').to_string())
        })
        .filter(|v| !v.is_empty())
}

/// Run a query and apply `accept` to its stdout when it exits 0.
async fn run_ok(bin: &str, args: &[&str], accept: impl Fn(&str) -> bool) -> bool {
    tokio::time::timeout(
        Duration::from_secs(120),
        safe_command(bin).args(args).output(),
    )
    .await
    .ok()
    .and_then(|r| r.ok())
    .map(|o| o.status.success() && accept(&String::from_utf8_lossy(&o.stdout)))
    .unwrap_or(false)
}

async fn which(cmd: &str) -> bool {
    tokio::time::timeout(
        Duration::from_secs(30),
        safe_command("which").arg(cmd).output(),
    )
    .await
    .ok()
    .and_then(|r| r.ok())
    .map(|o| o.status.success())
    .unwrap_or(false)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn maps_the_names_that_actually_differ() {
        assert_eq!(rpm_name("pdns-server"), "pdns");
        assert_eq!(rpm_name("redis-server"), "redis");
        assert_eq!(rpm_name("dovecot-imapd"), "dovecot");
        assert_eq!(rpm_name("dovecot-lmtpd"), "dovecot");
        assert_eq!(rpm_name("unattended-upgrades"), "dnf-automatic");
        assert_eq!(rpm_name("libmodsecurity3t64"), "nginx-mod-modsecurity");
    }

    #[test]
    fn collapses_every_php_fpm_version_to_the_single_rpm_package() {
        for v in ["8.1", "8.2", "8.3", "8.4", "8.5"] {
            assert_eq!(rpm_name(&format!("php{v}-fpm")), "php-fpm");
        }
        assert_eq!(rpm_name("php-fpm"), "php-fpm");
    }

    #[test]
    fn leaves_shared_names_alone() {
        for p in ["postfix", "fail2ban", "opendkim", "certbot", "nginx", "nodejs"] {
            assert_eq!(rpm_name(p), p);
        }
    }

    /// Both PowerDNS backends were passed through untranslated before s266, so
    /// the installer's very first argument matched nothing on the RHEL family.
    /// Verified against a real dnf database, not inferred from the Debian name.
    #[test]
    fn maps_both_powerdns_backends() {
        assert_eq!(rpm_name("pdns-backend-pgsql"), "pdns-backend-postgresql");
        assert_eq!(rpm_name("pdns-backend-sqlite3"), "pdns-backend-sqlite");
    }

    /// The WAF installer passes two package names; only one of them was mapped.
    #[test]
    fn maps_every_spelling_of_the_nginx_modsecurity_module() {
        for p in [
            "libmodsecurity3",
            "libmodsecurity3t64",
            "libnginx-mod-http-modsecurity",
        ] {
            assert_eq!(rpm_name(p), "nginx-mod-modsecurity");
        }
    }

    /// The map that did not exist before s266. A package name and a unit name
    /// are different strings that merely coincide on Debian; translating one
    /// and not the other installs the right package and then enables a unit
    /// that is not there.
    #[test]
    fn translates_unit_names_not_just_package_names() {
        assert_eq!(rpm_service_name("redis-server"), "redis");
        for v in ["8.1", "8.2", "8.3", "8.4", "8.5"] {
            assert_eq!(rpm_service_name(&format!("php{v}-fpm")), "php-fpm");
        }
    }

    /// A unit whose name is the same on both families must survive the map
    /// unchanged — otherwise the translation is the regression.
    #[test]
    fn leaves_units_that_match_alone() {
        for u in ["nginx", "fail2ban", "postfix", "dovecot", "opendkim", "docker"] {
            assert_eq!(rpm_service_name(u), u);
        }
    }

    /// PowerDNS is the case that proves the two maps must stay separate: the
    /// PACKAGE differs across families (`pdns-server` vs `pdns`) while the UNIT
    /// is `pdns` on BOTH. A single shared translation cannot express that, and
    /// feeding the package name to the unit map would stop a unit that does not
    /// exist on Debian — a bug this very session wrote and then caught.
    #[test]
    fn powerdns_package_differs_while_its_unit_does_not() {
        assert_eq!(rpm_name("pdns-server"), "pdns");
        // The unit map must NOT know about the package name…
        assert_eq!(rpm_service_name("pdns-server"), "pdns-server");
        // …because the real Debian unit is already `pdns`, and stays `pdns`.
        assert_eq!(rpm_service_name("pdns"), "pdns");
    }

    /// A package name and its unit name diverge INDEPENDENTLY. Pinning them
    /// together is what stops a future edit from "simplifying" one map into a
    /// call to the other.
    #[test]
    fn the_two_maps_are_not_interchangeable() {
        assert_eq!(rpm_name("php8.3-fpm"), "php-fpm");
        assert_eq!(rpm_service_name("php8.3-fpm"), "php-fpm");
        // …but a package with no unit keeps its package translation only:
        assert_eq!(rpm_name("pdns-backend-sqlite3"), "pdns-backend-sqlite");
        assert_eq!(rpm_service_name("pdns-backend-sqlite3"), "pdns-backend-sqlite3");
    }

    /// `php-mysql` matches no RPM; the driver is `php-mysqlnd`. Because the
    /// extension loop tolerates absent packages by design (#73), an unmapped
    /// name is skipped *without error* — so getting this wrong ships a PHP with
    /// no MySQL driver and no complaint.
    #[test]
    fn maps_the_php_extensions_whose_names_actually_differ() {
        assert_eq!(rpm_name("php8.3-mysql"), "php-mysqlnd");
        assert_eq!(rpm_name("php8.3-zip"), "php-pecl-zip");
        assert_eq!(rpm_name("php8.3-pgsql"), "php-pgsql");
        assert_eq!(rpm_name("php8.3-opcache"), "php-opcache");
    }

    /// A package that is not PHP-shaped must not be mangled by the PHP branch.
    #[test]
    fn the_php_branch_does_not_capture_unrelated_packages() {
        for p in ["phpmyadmin", "php", "postfix-policyd", "python3-venv"] {
            assert_eq!(rpm_name(p), p, "{p} should pass through untouched");
        }
    }
}
