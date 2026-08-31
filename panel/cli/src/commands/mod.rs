pub mod apps;
pub mod backup;
pub mod db;
pub mod iac;
pub mod logs;
pub mod php;
pub mod security;
pub mod sites;
pub mod ssl;
pub mod status;

/// Resolve an operator-chosen password from `--password`, `--password-stdin`,
/// or (when neither is given) an interactive masked prompt read from the
/// controlling terminal.
///
/// A bare `--password VALUE` lands in shell history and stays visible to any
/// local user via `ps`/`/proc/<pid>/cmdline` for the life of the process —
/// this exists so that door is opt-in rather than the only one.
pub fn resolve_password(
    password: Option<String>,
    password_stdin: bool,
    prompt: &str,
) -> Result<String, String> {
    if let Some(p) = password {
        return Ok(p);
    }
    if password_stdin {
        let mut line = String::new();
        std::io::stdin()
            .read_line(&mut line)
            .map_err(|e| format!("Failed to read password from stdin: {e}"))?;
        let line = line.trim_end_matches(['\n', '\r']).to_string();
        if line.is_empty() {
            return Err("Password read from stdin was empty".to_string());
        }
        return Ok(line);
    }
    let p = rpassword::prompt_password(prompt)
        .map_err(|e| format!("Failed to read password: {e}"))?;
    if p.is_empty() {
        return Err("Password must not be empty".to_string());
    }
    Ok(p)
}
