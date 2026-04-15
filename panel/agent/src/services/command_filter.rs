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
    // Reading other sites
    "/var/www/",
];

/// Check if a command string is safe for cron execution.
/// Rejects shell metacharacters and dangerous patterns.
pub fn is_safe_cron_command(cmd: &str) -> bool {
    if cmd.is_empty() || cmd.len() > 4096 || cmd.contains('\0') || cmd.contains('\n') || cmd.contains('\r') {
        return false;
    }

    // Reject shell metacharacters that enable chaining/substitution
    if cmd.contains('`') || cmd.contains("$(") || cmd.contains("| ") || cmd.contains("|/")
        || cmd.contains("<(") || cmd.contains("<<")
    {
        return false;
    }

    let lower = cmd.to_lowercase();
    !BLOCKED_PATTERNS.iter().any(|b| lower.contains(b))
}

/// Check if a terminal input line is safe.
/// Applies both the base blocklist and terminal-specific blocks.
pub fn is_safe_terminal_command(cmd: &str) -> bool {
    if cmd.trim().is_empty() {
        return true; // empty input is fine (just pressing Enter)
    }

    let lower = cmd.to_lowercase();

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
    // Reject known dangerous patterns
    let lower = command.to_lowercase();
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
    let lower = cmd.to_lowercase();
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
}
