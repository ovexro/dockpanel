use crate::safe_cmd::safe_command;
use axum::{
    extract::State,
    http::StatusCode,
    routing::post,
    Json, Router,
};
use serde::Deserialize;
use std::collections::HashMap;

use super::AppState;
use crate::services::{deploy, git_build, ownership};

type ApiErr = (StatusCode, Json<serde_json::Value>);

fn err(status: StatusCode, msg: &str) -> ApiErr {
    (status, Json(serde_json::json!({ "error": msg })))
}

/// Allowed commands for pre-build hooks (whitelist).
const ALLOWED_PRE_BUILD: &[&str] = &[
    "npm install",
    "npm ci",
    "yarn install",
    "pnpm install",
    "composer install",
    "bundle install",
    "pip install -r requirements.txt",
    "pip3 install -r requirements.txt",
    "cargo build --release",
];

/// Validate that a repo URL uses an allowed protocol and is not malicious.
fn is_valid_repo_url(url: &str) -> bool {
    if url.starts_with('-') {
        return false;
    }
    if url.starts_with("file://") || url.starts_with("ext://") {
        return false;
    }
    url.starts_with("https://")
        || url.starts_with("http://")
        || url.starts_with("ssh://")
        || url.starts_with("git@")
}

/// Validate that a branch name is safe.
fn is_valid_branch(branch: &str) -> bool {
    !branch.starts_with('-') && !branch.contains("..")
}

/// Security floor for a domain that becomes an nginx file path
/// (`/etc/nginx/sites-enabled/{domain}.conf`) and an unescaped `server_name`
/// written as root. Allows only `[A-Za-z0-9._-]` and rejects `..`, which blocks
/// path traversal (`/`, `..`) and nginx directive injection (`;`, `{`, `}`,
/// quotes, `$`, `#`, whitespace, control chars) while still permitting benign
/// characters like `_`. Deliberately looser than full RFC validity — the backend
/// enforces is_valid_domain on new values; this only stops the dangerous set.
fn is_safe_nginx_domain(domain: &str) -> bool {
    !domain.is_empty()
        && domain.len() <= 253
        && !domain.contains("..")
        && domain
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '.' || c == '-' || c == '_')
}

/// Validate that a dockerfile path does not escape the build context.
fn is_valid_dockerfile(dockerfile: &str) -> bool {
    !dockerfile.contains("..") && !dockerfile.starts_with('/')
}

/// Validate that a build_context path does not escape the repo directory.
fn is_valid_build_context(ctx: &str) -> bool {
    !ctx.contains("..") && !ctx.starts_with('/')
}

/// Validate that an image_tag is a dockpanel-managed tag and has no path traversal.
fn is_valid_image_tag(tag: &str) -> bool {
    tag.starts_with("dockpanel-git-") && !tag.contains('/')
}

/// Every git endpoint addresses one of two disjoint name spaces, and the caller
/// says which.
///
/// A preview used to be addressed by the bare string `{config}-pr-{branch}`,
/// which is a name a *deployment* can legally have — so a pushed branch could
/// name a stranger's production container, checkout directory and image
/// repository. `GitScope::scoped` puts previews behind a `.`, which
/// `is_valid_name` rejects, so the two spaces can no longer intersect.
///
/// Absent or unrecognised means `deploy`: a panel older than this agent keeps
/// working exactly as before, and only the panel that knows about previews can
/// ask for one.
fn scope_of(raw: &str) -> ownership::GitScope {
    ownership::GitScope::from_wire(raw)
}

#[derive(Deserialize)]
struct CloneRequest {
    name: String,
    repo_url: String,
    branch: String,
    key_path: Option<String>,
    #[serde(default)]
    scope: String,
}

#[derive(Deserialize)]
struct BuildRequest {
    name: String,
    #[serde(default = "default_dockerfile")]
    dockerfile: String,
    commit_hash: String,
    #[serde(default)]
    build_args: HashMap<String, String>,
    #[serde(default = "default_context")]
    build_context: String,
    #[serde(default)]
    scope: String,
}

fn default_dockerfile() -> String {
    "Dockerfile".to_string()
}

fn default_context() -> String {
    ".".to_string()
}

#[derive(Deserialize)]
struct DeployRequest {
    name: String,
    image_tag: String,
    container_port: u16,
    host_port: u16,
    /// The container's runtime environment.
    ///
    /// Accepts both spellings deliberately. This endpoint has always read `env`
    /// (matching `/apps/deploy`), while every caller in the panel spelled it
    /// `env_vars` after the column it is loaded from — and `serde(default)` turned
    /// that mismatch into an empty map rather than an error, so git deploys came up
    /// with no environment at all and nothing anywhere said so (GH #94). The panel
    /// now sends `env`; the alias keeps a not-yet-updated panel working, since
    /// agents and panels are installed separately and update on their own schedule.
    ///
    /// Callers must send ONE of the two keys, never both: serde rejects a payload
    /// carrying a field and its alias as a duplicate field.
    #[serde(default, alias = "env_vars")]
    env: HashMap<String, String>,
    domain: Option<String>,
    memory_mb: Option<u64>,
    cpu_percent: Option<u64>,
    ssl_email: Option<String>,
    #[serde(default)]
    scope: String,
}

#[derive(Deserialize)]
struct KeygenRequest {
    name: String,
}

/// The OLD domain a rename is walking away from, plus the port that proves the
/// vhost is still this deploy's. `host_port` is not optional in spirit: without
/// it `ownership::app_vhost` cannot tell "still ours" from "re-claimed since",
/// and it fails CLOSED — so a missing port leaves the vhost in place.
#[derive(Deserialize)]
struct ReleaseDomainRequest {
    domain: String,
    #[serde(default)]
    host_port: Option<u16>,
}

#[derive(Deserialize)]
struct CleanupRequest {
    name: String,
    #[serde(default)]
    scope: String,
    /// What the PANEL believes this deployment's vhost and published port are.
    ///
    /// The agent reads both off the container, which answers nothing once the
    /// container is already gone — and that is the common case for a preview
    /// swept after a crash. The vhost and its certificate then survived with no
    /// record anywhere naming them. These are a fallback, not an override: the
    /// port still has to match what the vhost proxies to before anything is
    /// deleted.
    #[serde(default)]
    domain: Option<String>,
    #[serde(default)]
    host_port: Option<u16>,
}

#[derive(Deserialize)]
struct PruneRequest {
    name: String,
    #[serde(default = "default_keep")]
    keep: usize,
    #[serde(default)]
    scope: String,
}

fn default_keep() -> usize {
    5
}

/// POST /git/clone — Clone or pull a Git repository.
async fn clone(
    Json(body): Json<CloneRequest>,
) -> Result<Json<serde_json::Value>, ApiErr> {
    if !super::is_valid_name(&body.name) {
        return Err(err(StatusCode::BAD_REQUEST, "Invalid name"));
    }
    if body.repo_url.is_empty() {
        return Err(err(StatusCode::BAD_REQUEST, "Missing repo_url"));
    }
    if !is_valid_repo_url(&body.repo_url) {
        return Err(err(StatusCode::BAD_REQUEST, "Invalid repo_url: must use https://, http://, ssh://, or git@ protocol"));
    }
    if !is_valid_branch(&body.branch) {
        return Err(err(StatusCode::BAD_REQUEST, "Invalid branch name"));
    }

    let name = scope_of(&body.scope).scoped(&body.name);

    tracing::info!("Git clone: {} from {} ({})", name, body.repo_url, body.branch);

    let result = git_build::clone_or_pull(
        &name,
        &body.repo_url,
        &body.branch,
        body.key_path.as_deref(),
    )
    .await
    .map_err(|e| err(StatusCode::INTERNAL_SERVER_ERROR, &e))?;

    Ok(Json(serde_json::json!({
        "commit_hash": result.commit_hash,
        "commit_message": result.commit_message,
    })))
}

/// POST /git/build — Build a Docker image from the cloned repo.
async fn build(
    Json(body): Json<BuildRequest>,
) -> Result<Json<serde_json::Value>, ApiErr> {
    if !super::is_valid_name(&body.name) {
        return Err(err(StatusCode::BAD_REQUEST, "Invalid name"));
    }
    if !is_valid_dockerfile(&body.dockerfile) {
        return Err(err(StatusCode::BAD_REQUEST, "Invalid dockerfile path"));
    }
    if !is_valid_build_context(&body.build_context) {
        return Err(err(StatusCode::BAD_REQUEST, "Invalid build_context path"));
    }

    let name = scope_of(&body.scope).scoped(&body.name);

    tracing::info!("Git build: {} (commit: {})", name, body.commit_hash);

    let result = git_build::build_image(
        &name,
        &body.dockerfile,
        &body.commit_hash,
        &body.build_args,
        &body.build_context,
    )
    .await
    .map_err(|e| err(StatusCode::INTERNAL_SERVER_ERROR, &e))?;

    Ok(Json(serde_json::json!({
        "image_tag": result.image_tag,
        "output": result.output,
    })))
}

/// POST /git/deploy — Deploy a container from a locally-built image.
async fn deploy_container(
    State(state): State<AppState>,
    Json(body): Json<DeployRequest>,
) -> Result<Json<serde_json::Value>, ApiErr> {
    if !super::is_valid_name(&body.name) {
        return Err(err(StatusCode::BAD_REQUEST, "Invalid name"));
    }
    if !is_valid_image_tag(&body.image_tag) {
        return Err(err(StatusCode::BAD_REQUEST, "Invalid image_tag: must start with dockpanel-git- and not contain /"));
    }
    if body.container_port == 0 || body.host_port == 0 {
        return Err(err(StatusCode::BAD_REQUEST, "Invalid port"));
    }
    // Defense-in-depth: the domain becomes an nginx file path
    // (/etc/nginx/sites-enabled/{domain}.conf) and an unescaped `server_name`
    // written as root, so reject the injection/traversal characters here even if
    // the backend validation were bypassed (the s241 #69 threat). This is a
    // SECURITY floor (block '/', ';', braces, quotes, '$', whitespace, control),
    // NOT full RFC validity — the backend already enforces is_valid_domain on new
    // values; being stricter here would break grandfathered/preview domains that
    // merely contain a benign '_'.
    if let Some(ref domain) = body.domain {
        if !domain.is_empty() && !is_safe_nginx_domain(domain) {
            return Err(err(StatusCode::BAD_REQUEST, "Invalid domain"));
        }
    }

    let scope = scope_of(&body.scope);
    let name = scope.scoped(&body.name);

    tracing::info!(
        "Git deploy: {} (image: {}, port: {}→{})",
        name, body.image_tag, body.host_port, body.container_port
    );

    let result = git_build::deploy_or_update(
        &name,
        scope,
        &body.image_tag,
        body.container_port,
        body.host_port,
        body.env,
        body.domain.as_deref(),
        &state.templates,
        body.memory_mb,
        body.cpu_percent,
        body.ssl_email.as_deref(),
    )
    .await
    .map_err(|e| err(StatusCode::INTERNAL_SERVER_ERROR, &e))?;

    Ok(Json(serde_json::json!({
        "container_id": result.container_id,
        "blue_green": result.blue_green,
        // What was ACTUALLY created, so the caller records a name rather than
        // predicting one. An agent and a panel are installed separately and
        // update on their own schedule, and a panel newer than its agent would
        // otherwise store the scoped name for a container the agent had created
        // under the old one — a row pointing at nothing.
        "container_name": format!("dockpanel-git-{name}"),
    })))
}

/// POST /git/keygen — Generate SSH deploy key.
async fn keygen(
    Json(body): Json<KeygenRequest>,
) -> Result<Json<serde_json::Value>, ApiErr> {
    if !super::is_valid_name(&body.name) {
        return Err(err(StatusCode::BAD_REQUEST, "Invalid name"));
    }

    let (public_key, key_path) = deploy::generate_deploy_key(&body.name)
        .map_err(|e| err(StatusCode::INTERNAL_SERVER_ERROR, &e))?;

    Ok(Json(serde_json::json!({
        "public_key": public_key,
        "key_path": key_path,
    })))
}

/// POST /git/cleanup — Stop + remove container and clean up nginx/SSL/volumes.
async fn cleanup(
    Json(body): Json<CleanupRequest>,
) -> Result<Json<serde_json::Value>, ApiErr> {
    if !super::is_valid_name(&body.name) {
        return Err(err(StatusCode::BAD_REQUEST, "Invalid name"));
    }

    if let Some(ref d) = body.domain {
        if !d.is_empty() && !is_safe_nginx_domain(d) {
            return Err(err(StatusCode::BAD_REQUEST, "Invalid domain"));
        }
    }

    let scope = scope_of(&body.scope);

    git_build::cleanup_container(
        &scope.scoped(&body.name),
        scope,
        body.domain.as_deref(),
        body.host_port,
    )
    .await
        .map_err(|e| err(StatusCode::INTERNAL_SERVER_ERROR, &e))?;

    Ok(Json(serde_json::json!({ "success": true })))
}

/// POST /git/release-domain — drop a vhost + certs this deploy no longer holds.
///
/// The rename half of `/git/cleanup`. A Git Deploy that changes its domain used
/// to leave the OLD vhost proxying to the still-running container, with a
/// certificate that kept renewing, while the panel released the name for anyone
/// else to claim. Same ownership gate as the delete path, for the same reason.
async fn release_domain(
    Json(body): Json<ReleaseDomainRequest>,
) -> Result<Json<serde_json::Value>, ApiErr> {
    if body.domain.is_empty() || !is_safe_nginx_domain(&body.domain) {
        return Err(err(StatusCode::BAD_REQUEST, "Invalid domain"));
    }
    git_build::release_domain_artifacts(&body.domain, body.host_port).await;
    Ok(Json(serde_json::json!({ "success": true })))
}

/// POST /git/prune — Remove old Docker images, keeping the last N.
async fn prune(
    Json(body): Json<PruneRequest>,
) -> Result<Json<serde_json::Value>, ApiErr> {
    if !super::is_valid_name(&body.name) {
        return Err(err(StatusCode::BAD_REQUEST, "Invalid name"));
    }

    let pruned = git_build::prune_images(&scope_of(&body.scope).scoped(&body.name), body.keep)
        .await
        .map_err(|e| err(StatusCode::INTERNAL_SERVER_ERROR, &e))?;

    Ok(Json(serde_json::json!({ "pruned": pruned })))
}

#[derive(Deserialize)]
struct LifecycleRequest {
    name: String,
    #[serde(default)]
    scope: String,
}

/// POST /git/stop
async fn stop_container(Json(body): Json<LifecycleRequest>) -> Result<Json<serde_json::Value>, ApiErr> {
    if !super::is_valid_name(&body.name) { return Err(err(StatusCode::BAD_REQUEST, "Invalid name")); }
    let container_name = format!("dockpanel-git-{}", scope_of(&body.scope).scoped(&body.name));
    let docker = bollard::Docker::connect_with_local_defaults()
        .map_err(|e| err(StatusCode::INTERNAL_SERVER_ERROR, &format!("{e}")))?;
    tokio::time::timeout(
        std::time::Duration::from_secs(30),
        docker.stop_container(&container_name, Some(bollard::container::StopContainerOptions { t: 10 }))
    ).await
        .map_err(|_| err(StatusCode::GATEWAY_TIMEOUT, "docker stop timed out (30s)"))?
        .map_err(|e| err(StatusCode::INTERNAL_SERVER_ERROR, &format!("{e}")))?;
    Ok(Json(serde_json::json!({ "success": true })))
}

/// POST /git/start
async fn start_container(Json(body): Json<LifecycleRequest>) -> Result<Json<serde_json::Value>, ApiErr> {
    if !super::is_valid_name(&body.name) { return Err(err(StatusCode::BAD_REQUEST, "Invalid name")); }
    let container_name = format!("dockpanel-git-{}", scope_of(&body.scope).scoped(&body.name));
    let docker = bollard::Docker::connect_with_local_defaults()
        .map_err(|e| err(StatusCode::INTERNAL_SERVER_ERROR, &format!("{e}")))?;
    tokio::time::timeout(
        std::time::Duration::from_secs(30),
        docker.start_container(&container_name, None::<bollard::container::StartContainerOptions<String>>)
    ).await
        .map_err(|_| err(StatusCode::GATEWAY_TIMEOUT, "docker start timed out (30s)"))?
        .map_err(|e| err(StatusCode::INTERNAL_SERVER_ERROR, &format!("{e}")))?;
    Ok(Json(serde_json::json!({ "success": true })))
}

/// POST /git/restart
async fn restart_container(Json(body): Json<LifecycleRequest>) -> Result<Json<serde_json::Value>, ApiErr> {
    if !super::is_valid_name(&body.name) { return Err(err(StatusCode::BAD_REQUEST, "Invalid name")); }
    let container_name = format!("dockpanel-git-{}", scope_of(&body.scope).scoped(&body.name));
    let docker = bollard::Docker::connect_with_local_defaults()
        .map_err(|e| err(StatusCode::INTERNAL_SERVER_ERROR, &format!("{e}")))?;
    tokio::time::timeout(
        std::time::Duration::from_secs(30),
        docker.restart_container(&container_name, Some(bollard::container::RestartContainerOptions { t: 10 }))
    ).await
        .map_err(|_| err(StatusCode::GATEWAY_TIMEOUT, "docker restart timed out (30s)"))?
        .map_err(|e| err(StatusCode::INTERNAL_SERVER_ERROR, &format!("{e}")))?;
    Ok(Json(serde_json::json!({ "success": true })))
}

#[derive(Deserialize)]
struct LogsRequest {
    name: String,
    #[serde(default = "default_log_lines")]
    lines: usize,
    #[serde(default)]
    scope: String,
}
fn default_log_lines() -> usize { 200 }

/// POST /git/logs
async fn container_logs(Json(body): Json<LogsRequest>) -> Result<Json<serde_json::Value>, ApiErr> {
    if !super::is_valid_name(&body.name) { return Err(err(StatusCode::BAD_REQUEST, "Invalid name")); }
    let container_name = format!("dockpanel-git-{}", scope_of(&body.scope).scoped(&body.name));
    let docker = bollard::Docker::connect_with_local_defaults()
        .map_err(|e| err(StatusCode::INTERNAL_SERVER_ERROR, &format!("{e}")))?;

    use bollard::container::LogsOptions;
    use tokio_stream::StreamExt;
    let mut logs = docker.logs(&container_name, Some(LogsOptions::<String> {
        stdout: true, stderr: true, tail: body.lines.to_string(), ..Default::default()
    }));
    let mut output = String::new();
    while let Some(Ok(log)) = logs.next().await {
        output.push_str(&log.to_string());
    }
    Ok(Json(serde_json::json!({ "logs": output })))
}

#[derive(Deserialize)]
struct HookRequest {
    name: String,
    command: String,
    #[serde(default)]
    scope: String,
}

/// POST /git/hook — Run a command inside a git-deployed container (docker exec).
async fn run_hook(Json(body): Json<HookRequest>) -> Result<Json<serde_json::Value>, ApiErr> {
    if !super::is_valid_name(&body.name) { return Err(err(StatusCode::BAD_REQUEST, "Invalid name")); }
    if body.command.is_empty() { return Err(err(StatusCode::BAD_REQUEST, "Empty command")); }

    // Validate command does not contain shell injection characters
    if !crate::services::command_filter::is_safe_hook_command(&body.command) {
        return Err(err(StatusCode::BAD_REQUEST, "Command contains disallowed characters or patterns"));
    }

    let container_name = format!("dockpanel-git-{}", scope_of(&body.scope).scoped(&body.name));

    let output = tokio::time::timeout(
        std::time::Duration::from_secs(300),
        safe_command("docker")
            .args(["exec", &container_name, "sh", "-c", &body.command])
            .output()
    ).await
        .map_err(|_| err(StatusCode::GATEWAY_TIMEOUT, "Hook timed out (300s)"))?
        .map_err(|e| err(StatusCode::INTERNAL_SERVER_ERROR, &format!("{e}")))?;

    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    let combined = format!("{stdout}{stderr}");
    // Truncate to 50KB
    let truncated = if combined.len() > 50_000 { format!("{}...\n[truncated]", &combined[..50_000]) } else { combined };

    Ok(Json(serde_json::json!({
        "success": output.status.success(),
        "output": truncated,
    })))
}

#[derive(Deserialize)]
struct PreBuildHookRequest {
    name: String,
    command: String,
}

/// POST /git/pre-build-hook — Run a whitelisted command on the host in the git repo directory.
async fn pre_build_hook(Json(body): Json<PreBuildHookRequest>) -> Result<Json<serde_json::Value>, ApiErr> {
    if !super::is_valid_name(&body.name) { return Err(err(StatusCode::BAD_REQUEST, "Invalid name")); }
    if body.command.is_empty() { return Err(err(StatusCode::BAD_REQUEST, "Empty command")); }

    // Only allow whitelisted commands — arbitrary shell execution is not permitted.
    if !ALLOWED_PRE_BUILD.contains(&body.command.as_str()) {
        return Err(err(StatusCode::BAD_REQUEST, "Command not allowed. Permitted commands: npm install, npm ci, yarn install, pnpm install, composer install, bundle install, pip install -r requirements.txt, pip3 install -r requirements.txt, cargo build --release"));
    }

    let git_dir = format!("/var/lib/dockpanel/git/{}", body.name);
    if !std::path::Path::new(&git_dir).exists() {
        return Err(err(StatusCode::NOT_FOUND, "Git repo not found"));
    }

    let output = tokio::time::timeout(
        std::time::Duration::from_secs(300),
        safe_command("sh")
            .args(["-c", &body.command])
            .current_dir(&git_dir)
            .env("HOME", &git_dir)
            .env("NODE_ENV", "production")
            .output()
    ).await
        .map_err(|_| err(StatusCode::GATEWAY_TIMEOUT, "Hook timed out (300s)"))?
        .map_err(|e| err(StatusCode::INTERNAL_SERVER_ERROR, &format!("{e}")))?;

    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    let combined = format!("{stdout}{stderr}");
    let truncated = if combined.len() > 50_000 { format!("{}...\n[truncated]", &combined[..50_000]) } else { combined };

    Ok(Json(serde_json::json!({
        "success": output.status.success(),
        "output": truncated,
    })))
}

#[derive(Deserialize)]
struct AutoDetectRequest {
    name: String,
    #[serde(default = "default_dockerfile")]
    dockerfile: String,
    #[serde(default = "default_context")]
    build_context: String,
}

/// POST /git/auto-detect — Auto-detect language and generate Dockerfile if missing.
async fn auto_detect(Json(body): Json<AutoDetectRequest>) -> Result<Json<serde_json::Value>, ApiErr> {
    if !super::is_valid_name(&body.name) { return Err(err(StatusCode::BAD_REQUEST, "Invalid name")); }
    if !is_valid_build_context(&body.build_context) {
        return Err(err(StatusCode::BAD_REQUEST, "Invalid build_context path"));
    }

    // Check if the original Dockerfile exists before calling auto-detect
    let deploy_dir = format!("/var/lib/dockpanel/git/{}", body.name);
    let context_dir = if body.build_context == "." { deploy_dir.clone() } else { format!("{deploy_dir}/{}", body.build_context) };
    let original_exists = std::path::Path::new(&context_dir).join(&body.dockerfile).exists();

    let dockerfile = git_build::auto_generate_dockerfile(&body.name, &body.dockerfile, &body.build_context)
        .map_err(|e| err(StatusCode::UNPROCESSABLE_ENTITY, &e))?;

    // auto_generated is true only if the original didn't exist (meaning the function created one)
    let auto_generated = !original_exists;

    Ok(Json(serde_json::json!({
        "dockerfile": dockerfile,
        "auto_generated": auto_generated,
    })))
}

#[derive(Deserialize)]
struct ComposeCheckRequest {
    name: String,
    #[serde(default = "default_context")]
    build_context: String,
}

/// POST /git/compose-check — Check if repo has docker-compose.yml
async fn compose_check(Json(body): Json<ComposeCheckRequest>) -> Result<Json<serde_json::Value>, ApiErr> {
    if !super::is_valid_name(&body.name) { return Err(err(StatusCode::BAD_REQUEST, "Invalid name")); }
    if !is_valid_build_context(&body.build_context) {
        return Err(err(StatusCode::BAD_REQUEST, "Invalid build_context path"));
    }
    let deploy_dir = format!("/var/lib/dockpanel/git/{}", body.name);
    let context_dir = if body.build_context == "." { deploy_dir.clone() } else { format!("{deploy_dir}/{}", body.build_context) };

    // Check for docker-compose.yml or compose.yml
    let compose_file = ["docker-compose.yml", "docker-compose.yaml", "compose.yml", "compose.yaml"]
        .iter()
        .find(|f| std::path::Path::new(&context_dir).join(f).exists())
        .map(|f| f.to_string());

    match compose_file {
        Some(f) => {
            let content = std::fs::read_to_string(std::path::Path::new(&context_dir).join(&f))
                .map_err(|e| err(StatusCode::INTERNAL_SERVER_ERROR, &format!("{e}")))?;
            Ok(Json(serde_json::json!({ "found": true, "file": f, "content": content })))
        }
        None => Ok(Json(serde_json::json!({ "found": false }))),
    }
}

/// POST /git/nixpacks-build — Build image using nixpacks
async fn nixpacks_build_handler(
    State(_state): State<AppState>,
    Json(body): Json<serde_json::Value>,
) -> Result<Json<serde_json::Value>, (StatusCode, String)> {
    let name = body["name"].as_str().ok_or((StatusCode::BAD_REQUEST, "name required".into()))?;
    if !super::is_valid_name(name) {
        return Err((StatusCode::BAD_REQUEST, "Invalid name".into()));
    }
    let commit_hash = body["commit_hash"].as_str().unwrap_or("latest");
    let build_context = body["build_context"].as_str().unwrap_or(".");
    if build_context.contains("..") || build_context.starts_with('/') {
        return Err((StatusCode::BAD_REQUEST, "Invalid build_context path".into()));
    }
    // Both spellings, for the same reason `/git/deploy` accepts both: this endpoint
    // reads `env_vars` and its sibling reads `env`, and that disagreement is what
    // GH #94 was. Neither endpoint should care which one a given panel sends.
    let env_vars: std::collections::HashMap<String, String> = body
        .get("env_vars")
        .filter(|v| !v.is_null())
        .or_else(|| body.get("env"))
        .and_then(|v| v.as_object())
        .map(|m| m.iter().filter_map(|(k, v)| v.as_str().map(|s| (k.clone(), s.to_string()))).collect())
        .unwrap_or_default();

    match crate::services::git_build::nixpacks_build(name, commit_hash, build_context, &env_vars).await {
        Ok((image_tag, output)) => Ok(Json(serde_json::json!({
            "image_tag": image_tag,
            "output": output,
            "build_method": "nixpacks",
        }))),
        Err(e) => Err((StatusCode::INTERNAL_SERVER_ERROR, e)),
    }
}

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/git/clone", post(clone))
        .route("/git/build", post(build))
        .route("/git/deploy", post(deploy_container))
        .route("/git/keygen", post(keygen))
        .route("/git/cleanup", post(cleanup))
        .route("/git/release-domain", post(release_domain))
        .route("/git/prune", post(prune))
        .route("/git/stop", post(stop_container))
        .route("/git/start", post(start_container))
        .route("/git/restart", post(restart_container))
        .route("/git/logs", post(container_logs))
        .route("/git/hook", post(run_hook))
        .route("/git/pre-build-hook", post(pre_build_hook))
        .route("/git/auto-detect", post(auto_detect))
        .route("/git/compose-check", post(compose_check))
        .route("/git/nixpacks-build", post(nixpacks_build_handler))
}

#[cfg(test)]
mod tests {
    use super::is_safe_nginx_domain;

    #[test]
    fn safe_nginx_domain_blocks_injection_and_traversal() {
        // The s241 #69 sink threats must be rejected.
        for d in [
            "../../../etc/nginx/conf.d/pwn",              // path traversal
            "x; } server { listen 80 default_server; }", // directive injection (space, ';', braces)
            "a\"b.com",                                    // quote break-out
            "a$b.com", "a#b.com", "a b.com", "a/b",       // '$','#',space,'/'
            "",
        ] {
            assert!(!is_safe_nginx_domain(d), "{d:?} must be rejected");
        }
    }

    #[test]
    fn safe_nginx_domain_allows_real_and_grandfathered_hosts() {
        // Valid hosts AND benign grandfathered/preview values (underscore) pass.
        for d in ["app.example.com", "feature-login.app.example.com", "my_app.example.com", "a-b_c.example.com"] {
            assert!(is_safe_nginx_domain(d), "{d:?} must be allowed");
        }
    }
}
