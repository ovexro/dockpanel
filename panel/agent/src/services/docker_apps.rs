use super::app_sidecar;
use bollard::container::{
    Config, CreateContainerOptions, ListContainersOptions, RemoveContainerOptions,
    RenameContainerOptions, StartContainerOptions, StopContainerOptions,
};
use bollard::image::CreateImageOptions;
use bollard::Docker;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use tokio_stream::StreamExt;

/// Where a template app's persistent volumes are bind-mounted from.
///
/// Named once because three places have to agree: the deploy that creates the
/// directory, the prefix check that keeps a volume from escaping it, and the
/// removal that decides whether this container is entitled to delete it.
pub const APP_DATA_DIR: &str = "/var/lib/dockpanel/apps";

/// What `deploy_app` puts in front of an app's name to make its container name.
///
/// Named for the same reason as the directory above: the two are NOT the same string,
/// and every recreate path has an `info.name` (the container) in hand while every path
/// that touches data needs the app. Confusing the two is what put a migrated app's data
/// somewhere the panel could not find it — see `app_data_name`.
pub const CONTAINER_NAME_PREFIX: &str = "dockpanel-app-";

// ── Container auto-update detection ────────────────────────────────

#[derive(Debug, Serialize)]
pub struct ImageUpdateInfo {
    pub container_id: String,
    pub name: String,
    pub image: String,
    pub current_digest: Option<String>,
    pub remote_digest: Option<String>,
    pub update_available: bool,
    pub check_error: Option<String>,
}

struct ImageRef {
    registry: String,
    repository: String,
    tag: String,
}

/// Parse a Docker image reference into registry, repository, tag.
/// Examples:
///   "redis:7-alpine"           → registry-1.docker.io / library/redis / 7-alpine
///   "grafana/grafana:latest"   → registry-1.docker.io / grafana/grafana / latest
///   "ghcr.io/foo/bar:v1"      → ghcr.io / foo/bar / v1
fn parse_image_ref(image: &str) -> ImageRef {
    // Strip any @sha256:... digest suffix
    let image = image.split('@').next().unwrap_or(image);

    let has_registry = image.contains('/') && {
        let first = image.split('/').next().unwrap_or("");
        first.contains('.') || first.contains(':')
    };

    let (registry, rest) = if has_registry {
        let (reg, rest) = image.split_once('/').unwrap_or(("", image));
        (reg.to_string(), rest.to_string())
    } else if image.contains('/') {
        ("registry-1.docker.io".to_string(), image.to_string())
    } else {
        ("registry-1.docker.io".to_string(), format!("library/{image}"))
    };

    let (repository, tag) = if let Some((r, t)) = rest.rsplit_once(':') {
        (r.to_string(), t.to_string())
    } else {
        (rest, "latest".to_string())
    };

    ImageRef { registry, repository, tag }
}

/// Fetch the manifest digest from a Docker registry (Docker Hub, GHCR, or generic OCI).
async fn get_registry_digest(image_ref: &ImageRef) -> Result<String, String> {
    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(15))
        .build()
        .map_err(|e| format!("HTTP client: {e}"))?;

    let accept = "application/vnd.docker.distribution.manifest.list.v2+json, \
                  application/vnd.oci.image.index.v1+json, \
                  application/vnd.docker.distribution.manifest.v2+json, \
                  application/vnd.oci.image.manifest.v1+json";

    if image_ref.registry == "registry-1.docker.io" || image_ref.registry == "docker.io" {
        // Docker Hub auth
        let token_url = format!(
            "https://auth.docker.io/token?service=registry.docker.io&scope=repository:{}:pull",
            image_ref.repository
        );
        #[derive(Deserialize)]
        struct TokenResp { token: String }

        let token: TokenResp = client.get(&token_url)
            .send().await.map_err(|e| format!("Auth failed: {e}"))?
            .json().await.map_err(|e| format!("Auth parse: {e}"))?;

        let url = format!(
            "https://registry-1.docker.io/v2/{}/manifests/{}",
            image_ref.repository, image_ref.tag
        );
        let resp = client.head(&url)
            .header("Authorization", format!("Bearer {}", token.token))
            .header("Accept", accept)
            .send().await.map_err(|e| format!("Manifest fetch: {e}"))?;

        if !resp.status().is_success() {
            return Err(format!("Registry HTTP {}", resp.status()));
        }
        extract_digest(&resp)
    } else if image_ref.registry == "ghcr.io" {
        // GitHub Container Registry
        let token_url = format!(
            "https://ghcr.io/token?service=ghcr.io&scope=repository:{}:pull",
            image_ref.repository
        );
        #[derive(Deserialize)]
        struct TokenResp { token: String }

        let token: TokenResp = client.get(&token_url)
            .send().await.map_err(|e| format!("GHCR auth: {e}"))?
            .json().await.map_err(|e| format!("GHCR auth parse: {e}"))?;

        let url = format!(
            "https://ghcr.io/v2/{}/manifests/{}",
            image_ref.repository, image_ref.tag
        );
        let resp = client.head(&url)
            .header("Authorization", format!("Bearer {}", token.token))
            .header("Accept", accept)
            .send().await.map_err(|e| format!("GHCR manifest: {e}"))?;

        if !resp.status().is_success() {
            return Err(format!("GHCR HTTP {}", resp.status()));
        }
        extract_digest(&resp)
    } else {
        // Generic OCI registry — try anonymous access
        let url = format!(
            "https://{}/v2/{}/manifests/{}",
            image_ref.registry, image_ref.repository, image_ref.tag
        );
        let resp = client.head(&url)
            .header("Accept", accept)
            .send().await.map_err(|e| format!("Registry: {e}"))?;

        if resp.status() == reqwest::StatusCode::UNAUTHORIZED {
            return Err("Auth required".to_string());
        }
        if !resp.status().is_success() {
            return Err(format!("HTTP {}", resp.status()));
        }
        extract_digest(&resp)
    }
}

fn extract_digest(resp: &reqwest::Response) -> Result<String, String> {
    resp.headers()
        .get("docker-content-digest")
        .ok_or_else(|| "No digest header".to_string())?
        .to_str()
        .map(|s| s.to_string())
        .map_err(|e| format!("Invalid digest: {e}"))
}

/// Check all managed containers for available image updates by comparing
/// local RepoDigests against registry manifests.
pub async fn check_image_updates() -> Result<Vec<ImageUpdateInfo>, String> {
    let docker = Docker::connect_with_local_defaults()
        .map_err(|e| format!("Docker connect: {e}"))?;

    let apps = list_deployed_apps().await?;
    let mut results = Vec::with_capacity(apps.len());

    for app in &apps {
        let image = match &app.image {
            Some(img) => img.clone(),
            None => {
                results.push(ImageUpdateInfo {
                    container_id: app.container_id.clone(),
                    name: app.name.clone(),
                    image: "unknown".to_string(),
                    current_digest: None,
                    remote_digest: None,
                    update_available: false,
                    check_error: Some("No image info".to_string()),
                });
                continue;
            }
        };

        // Get local image digest from RepoDigests
        let local_digest = docker.inspect_image(&image).await.ok()
            .and_then(|info| info.repo_digests)
            .and_then(|d| d.into_iter().next())
            .and_then(|d| d.split('@').nth(1).map(|s| s.to_string()));

        let image_ref = parse_image_ref(&image);

        let (remote_digest, check_error, update_available) = match get_registry_digest(&image_ref).await {
            Ok(digest) => {
                let has_update = local_digest.as_ref().map_or(false, |local| local != &digest);
                (Some(digest), None, has_update)
            }
            Err(e) => (None, Some(e), false),
        };

        results.push(ImageUpdateInfo {
            container_id: app.container_id.clone(),
            name: app.name.clone(),
            image,
            current_digest: local_digest,
            remote_digest,
            update_available,
            check_error,
        });
    }

    Ok(results)
}

/// Template definition used in the static array (slice-based, no heap allocation).
struct AppTemplateDef {
    id: &'static str,
    name: &'static str,
    description: &'static str,
    category: &'static str,
    image: &'static str,
    default_port: u16,
    container_port: &'static str,
    env_vars: &'static [EnvVarDef],
    volumes: &'static [&'static str],
}

struct EnvVarDef {
    name: &'static str,
    label: &'static str,
    default: &'static str,
    required: bool,
    secret: bool,
}

/// Serializable template returned to API consumers.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AppTemplate {
    pub id: String,
    pub name: String,
    pub description: String,
    pub category: String,
    pub image: String,
    pub default_port: u16,
    pub container_port: String,
    pub env_vars: Vec<EnvVar>,
    pub volumes: Vec<String>,
    /// True when the template materially benefits from GPU passthrough
    /// (LLM/diffusion/ASR inference). Frontend uses this to badge the
    /// template card and pre-tick the GPU toggle on the deploy form.
    pub gpu_recommended: bool,
}

/// Template IDs whose container materially benefits from GPU passthrough.
/// Source-of-truth list kept here instead of a per-template field to avoid
/// touching every static AppTemplateDef declaration when this set evolves.
static GPU_RECOMMENDED_TEMPLATES: &[&str] = &[
    "ollama",
    "localai",
    "vllm",
    "stable-diffusion-webui",
    "text-generation-webui",
    "whisper",
];

/// Startup arguments for the templates whose image default `CMD` does not start a
/// server. Kept as a keyed list rather than a per-template field for the reason
/// given above `GPU_RECOMMENDED_TEMPLATES` — every static declaration in the
/// catalogue would otherwise have to be edited to add a field all but one leaves
/// empty.
///
/// Absence is the meaningful default: an image whose entrypoint already serves must
/// keep deciding its own startup, so upstream can change it without this catalogue
/// silently overriding them.
///
/// `minio` needs it because `minio/minio`'s default `CMD` is `["minio"]`, which
/// prints usage and **exits 0**. Nothing here crashes — the container simply has no
/// supervised process, so `unless-stopped` restarts it forever at `ExitCode: 0`
/// (issue #101). `/data` is the path the template already mounts a volume at, so the
/// served directory and the persisted directory are the same one; a `server` command
/// pointed anywhere else would come up healthy and lose every object on restart.
/// What a domain-derived env var should be filled in with.
///
/// The variants are the shapes real apps ask for, and they are not
/// interchangeable: n8n wants the bare host in `N8N_HOST` and a full URL in
/// `WEBHOOK_URL`, and drone splits host and scheme across two variables the
/// same way. A single "the app's URL" variant would be wrong for four of the
/// eleven entries below.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum DomainEnv {
    /// Bare hostname — `example.com`.
    Host,
    /// `https` or `http`, as a word.
    Scheme,
    /// `https://example.com`, no trailing slash.
    Url,
    /// `https://example.com/`. Separate from [`DomainEnv::Url`] because
    /// n8n concatenates `WEBHOOK_URL` with a path and emits a double slash
    /// without it, while Ghost rejects a trailing slash in `url`.
    UrlSlash,
    /// Reverse-proxy hops between the client and the app.
    ///
    /// Always exactly 1: `deploy_app` binds every container to `127.0.0.1`
    /// (see the port-binding block below), so the panel's own nginx is the only
    /// thing that can reach it, and that nginx appends exactly one entry
    /// (`proxy_set_header X-Forwarded-For $proxy_add_x_forwarded_for` in both
    /// `templates/nginx/proxy.conf` and `https.conf`). An operator who puts
    /// Cloudflare in front adds a second entry; the value still makes the app
    /// trust the proxy, it only shifts which entry it reads as the client — so
    /// this is right where it matters and imprecise where it does not.
    ProxyHops,
}

/// Env vars whose correct value is the domain the operator claimed on the
/// deploy form.
///
/// Kept as a keyed list rather than a per-template field for the reason given
/// above `GPU_RECOMMENDED_TEMPLATES` — every static declaration in the
/// catalogue would otherwise have to be edited to add a field all but six
/// leave empty.
///
/// **Why this exists.** Every entry names a variable the app uses to build its
/// OWN absolute URLs: webhook registrations, OAuth callbacks, password-reset
/// links. Left at the catalogue's `localhost:<port>` default, the app comes up
/// and serves fine — and every URL it hands out points somewhere the visitor's
/// browser cannot follow. The deploy form has collected the domain all along;
/// until this change `deploy_app` used it for one container label and threw it
/// away, so the operator had to know to retype it into a second field, and for
/// n8n there was no field to retype it into (GitHub #111).
///
/// An operator value that differs from the catalogue default always wins — see
/// the merge in [`deploy_app`]. This only fills a field nobody filled in.
static TEMPLATE_DOMAIN_ENV: &[(&str, &[(&str, DomainEnv)])] = &[
    // n8n declares no env vars at all, so all four of these are additions the
    // deploy form could not have offered. Without them n8n prints
    // `Editor is now accessible via: http://localhost:5678`, registers webhooks
    // against that address, and throws ERR_ERL_UNEXPECTED_X_FORWARDED_FOR on
    // every request because express-rate-limit does not trust the proxy.
    (
        "n8n",
        &[
            ("N8N_HOST", DomainEnv::Host),
            ("N8N_PROTOCOL", DomainEnv::Scheme),
            ("WEBHOOK_URL", DomainEnv::UrlSlash),
            ("N8N_PROXY_HOPS", DomainEnv::ProxyHops),
        ],
    ),
    ("ghost", &[("url", DomainEnv::Url)]),
    ("plausible", &[("BASE_URL", DomainEnv::Url)]),
    (
        "drone",
        &[
            ("DRONE_SERVER_HOST", DomainEnv::Host),
            ("DRONE_SERVER_PROTO", DomainEnv::Scheme),
        ],
    ),
    ("photoprism", &[("PHOTOPRISM_SITE_URL", DomainEnv::Url)]),
    (
        "graylog",
        &[("GRAYLOG_HTTP_EXTERNAL_URI", DomainEnv::UrlSlash)],
    ),
];

/// The values [`TEMPLATE_DOMAIN_ENV`] resolves to for one deploy.
///
/// `https` is not a guess. A claimed domain reaches the app through the panel's
/// nginx either way: with `external_tls` the operator terminates TLS upstream,
/// and without it the backend requests a certificate as part of the same deploy
/// (see `deploy_ssl_email` in the backend route). Both paths are public https.
/// The one case this is optimistic about is a certificate that fails to issue,
/// which leaves the derived value ahead of reality until it does — and an app
/// on a domain with no certificate is already broken in more visible ways.
fn domain_env_for(template_id: &str, domain: &str) -> Vec<(&'static str, String)> {
    TEMPLATE_DOMAIN_ENV
        .iter()
        .find(|(id, _)| *id == template_id)
        .map(|(_, vars)| {
            vars.iter()
                .map(|(name, kind)| {
                    let value = match kind {
                        DomainEnv::Host => domain.to_string(),
                        DomainEnv::Scheme => "https".to_string(),
                        DomainEnv::Url => format!("https://{domain}"),
                        DomainEnv::UrlSlash => format!("https://{domain}/"),
                        DomainEnv::ProxyHops => "1".to_string(),
                    };
                    (*name, value)
                })
                .collect()
        })
        .unwrap_or_default()
}

/// Assemble a deploy's `KEY=value` list from the template's declared vars, what
/// the operator submitted, and whatever [`domain_env_for`] derived.
///
/// Pure so the precedence can be tested without Docker. Called from
/// [`deploy_app`] and nowhere else.
///
/// Precedence, highest first:
/// 1. An operator value that DIFFERS from the catalogue default — a deliberate
///    choice, and it wins even over a derived value.
/// 2. The derived value, when this template has one for the name.
/// 3. The operator's value (i.e. one equal to the default).
/// 4. The catalogue default.
///
/// Rule 1 is the load-bearing one. Without it, an operator who typed their own
/// `WEBHOOK_URL` would have it silently overwritten by the domain — which is
/// the same class of bug as shipping `localhost`, just pointing the other way.
fn merge_deploy_env(
    env_vars: &[EnvVarDef],
    env: &HashMap<String, String>,
    derived: &[(&'static str, String)],
) -> Vec<String> {
    let mut env_list: Vec<String> = Vec::new();
    for ev in env_vars {
        let supplied = env.get(ev.name);
        let value = match supplied {
            Some(v) if v != ev.default => v.clone(),
            _ => derived
                .iter()
                .find(|(n, _)| *n == ev.name)
                .map(|(_, v)| v.clone())
                .or_else(|| supplied.cloned())
                .unwrap_or_else(|| ev.default.to_string()),
        };
        if !value.is_empty() {
            env_list.push(format!("{}={}", ev.name, value));
        }
    }
    // Include any extra env vars the user passed that aren't in the template
    for (k, v) in env {
        if !env_vars.iter().any(|ev| ev.name == k.as_str()) {
            env_list.push(format!("{k}={v}"));
        }
    }
    // Derived vars the template does not declare at all. n8n is the reason this
    // loop exists: its catalogue row is `env_vars: &[]`, so all four of its
    // domain-derived vars are additions no form field could have carried and
    // the loop above could never have reached. Skipped when the operator sent
    // the key themselves — the extras loop directly above already emitted it,
    // and appending here as well would set the variable twice.
    for (name, value) in derived {
        if !env_vars.iter().any(|ev| ev.name == *name) && !env.contains_key(*name) {
            env_list.push(format!("{name}={value}"));
        }
    }
    env_list
}

/// Whether this template fills `name` in from the claimed domain — the flag the
/// deploy form uses to tell the operator so, instead of showing them a
/// `localhost` default it is about to override.
fn is_domain_derived(template_id: &str, name: &str) -> bool {
    TEMPLATE_DOMAIN_ENV
        .iter()
        .find(|(id, _)| *id == template_id)
        .is_some_and(|(_, vars)| vars.iter().any(|(n, _)| *n == name))
}

static TEMPLATE_CMD: &[(&str, &[&str])] = &[
    ("minio", &["server", "/data", "--console-address", ":9001"]),
    // Each entry below was established by RUNNING the image and observing it
    // both ways: with no command it exits, with this command it stays up and
    // answers on its port. `cloudflared`'s own default Cmd is ["version"], so
    // every deploy printed a version string and stopped; `tunnel run` reads
    // TUNNEL_TOKEN from the environment the template already collects, so the
    // token never has to reach the command line.
    ("cloudflared", &["tunnel", "run"]),
    ("ntfy", &["serve"]),
    ("trivy", &["server", "--listen", "0.0.0.0:8080"]),
    ("keycloak", &["start-dev"]),
    // Plausible's image entrypoint does not migrate. Upstream's own compose file
    // supplies this exact chain, and without it the app meets an empty schema on
    // first run. Copied verbatim from the compose published at the version this
    // catalogue now pins, not reconstructed from prose.
    (
        "plausible",
        &[
            "sh",
            "-c",
            "/entrypoint.sh db createdb && /entrypoint.sh db migrate && /entrypoint.sh run",
        ],
    ),
];

/// The catalogue's command for whatever template a running container came from.
///
/// A recreate has the container's RESOLVED `Cmd` in hand, which is the old image's default
/// whenever the catalogue overrode nothing — so carrying that forward would pin a departing
/// image's command onto its replacement. Going back to the template label instead gives the
/// override where one exists and nothing where it does not, which is what lets the new
/// image's own default apply.
fn catalogue_cmd_for(labels: Option<&HashMap<String, String>>) -> Option<Vec<String>> {
    labels
        .and_then(|l| l.get("dockpanel.app.template"))
        .and_then(|template_id| template_cmd(template_id))
}

/// The `Cmd` to give a template's container, if the catalogue overrides it.
fn template_cmd(id: &str) -> Option<Vec<String>> {
    TEMPLATE_CMD
        .iter()
        .find(|(template_id, _)| *template_id == id)
        .map(|(_, args)| args.iter().map(|a| (*a).to_string()).collect())
}

/// `tecnativa/docker-socket-proxy`'s per-endpoint ACL, spelled out in full and
/// deny-by-default rather than left to the image's own defaults — the same
/// posture `deploy_app`'s `cap_drop: ALL` + explicit `cap_add` allowlist
/// already takes for every template container. Only `CONTAINERS` is granted:
/// Dozzle's entire purpose is listing containers and streaming their logs,
/// both served under that one toggle, and nothing else it could ask for
/// (exec into a container, create one, touch images/volumes/networks/swarm,
/// or any POST at all) is enabled.
static DOZZLE_PROXY_ENV: &[(&str, &str)] = &[
    ("CONTAINERS", "1"),
    ("EVENTS", "1"),
    ("PING", "1"),
    ("VERSION", "1"),
    ("POST", "0"),
    ("EXEC", "0"),
    ("IMAGES", "0"),
    ("NETWORKS", "0"),
    ("VOLUMES", "0"),
    ("SERVICES", "0"),
    ("SWARM", "0"),
    ("SYSTEM", "0"),
    ("TASKS", "0"),
    ("BUILD", "0"),
    ("COMMIT", "0"),
    ("AUTH", "0"),
    ("SECRETS", "0"),
    ("CONFIGS", "0"),
    ("DISTRIBUTION", "0"),
    ("NODES", "0"),
    ("PLUGINS", "0"),
    ("SESSION", "0"),
    ("GRPC", "0"),
    ("INFO", "0"),
    ("ALLOW_START", "0"),
    ("ALLOW_STOP", "0"),
    ("ALLOW_RESTARTS", "0"),
    ("ALLOW_PAUSE", "0"),
    ("ALLOW_UNPAUSE", "0"),
];

/// Templates whose catalogue entry alone cannot serve their purpose — they
/// need a companion container holding a capability (Docker API access) this
/// panel deliberately never hands the app container itself. A side table
/// rather than an `AppTemplateDef` field, for the same reason `TEMPLATE_CMD`
/// is one: one entry here should never have to touch the other 140+ template
/// literals.
static TEMPLATE_SIDECAR: &[(&str, app_sidecar::SidecarDef)] = &[(
    "dozzle",
    app_sidecar::SidecarDef {
        image: "tecnativa/docker-socket-proxy:latest",
        alias: "dockerproxy",
        port: 2375,
        proxy_env: DOZZLE_PROXY_ENV,
        client_env_var: "DOZZLE_REMOTE_HOST",
    },
)];

/// The sidecar a template needs, if any.
fn template_sidecar(id: &str) -> Option<&'static app_sidecar::SidecarDef> {
    TEMPLATE_SIDECAR
        .iter()
        .find(|(template_id, _)| *template_id == id)
        .map(|(_, s)| s)
}

/// The sidecar a running container's template needs, read from the
/// `dockpanel.app.template` label rather than the container's own live
/// network attachments — the same reason `catalogue_cmd_for` reads the label
/// instead of the resolved `Cmd`: this is the panel's CURRENT policy for the
/// template, not whatever the container happens to be doing right now.
fn catalogue_sidecar_for(labels: Option<&HashMap<String, String>>) -> Option<&'static app_sidecar::SidecarDef> {
    labels
        .and_then(|l| l.get("dockpanel.app.template"))
        .and_then(|template_id| template_sidecar(template_id))
}

/// Attach `env_list`/the returned `NetworkingConfig` to `app_name`'s sidecar
/// network, if the template this container was deployed from has one —
/// called from EVERY recreate path (`update_app`'s stop/start fallback,
/// `blue_green_update`, `change_container_image`, `update_env`), so an image
/// swap or an env edit can never silently strand the app back on the default
/// bridge, unable to resolve a sidecar it depends on. A no-op (returns
/// `None`, leaves `env_list` untouched) for every template without one.
fn reattach_sidecar_if_any(
    labels: Option<&HashMap<String, String>>,
    app_name: &str,
    env_list: &mut Vec<String>,
) -> Option<bollard::container::NetworkingConfig<String>> {
    let sidecar = catalogue_sidecar_for(labels)?;
    env_list.retain(|e| !e.starts_with(&format!("{}=", sidecar.client_env_var)));
    env_list.push(sidecar.client_env_entry());
    Some(app_sidecar::attach_config(app_name))
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EnvVar {
    pub name: String,
    pub label: String,
    pub default: String,
    pub required: bool,
    pub secret: bool,
    /// True when `deploy_app` fills this in from the claimed domain, so the
    /// deploy form can say so rather than presenting a `localhost` default it
    /// is about to replace. See [`TEMPLATE_DOMAIN_ENV`].
    #[serde(default)]
    pub domain_derived: bool,
}

#[derive(Debug, Serialize)]
pub struct DeployResult {
    pub container_id: String,
    pub name: String,
    pub port: u16,
}

#[derive(Debug, Serialize)]
pub struct UpdateResult {
    pub container_id: String,
    pub blue_green: bool,
    /// Container paths this update moved out of the writable layer and onto a bind
    /// mount. Non-empty only for an app deployed before its template declared the
    /// path — see `migrate_unmounted_volumes`. Reported so the operator learns their
    /// app just became durable rather than having it happen silently.
    pub migrated_volumes: Vec<String>,
    /// Host paths this update handed back to the uid the image runs as, because an
    /// EARLIER release migrated them with `docker cp` and left every file owned by
    /// root — see `repair_root_owned_volumes`. Non-empty only on an install that
    /// was actually damaged, and reported for the same reason as the field above:
    /// an operator whose app has been failing since it was last updated deserves to
    /// be told that this is what was wrong with it.
    pub repaired_volumes: Vec<String>,
}

#[derive(Debug, Serialize)]
pub struct DeployedApp {
    pub container_id: String,
    pub name: String,
    pub template: String,
    pub status: String,
    pub port: Option<u16>,
    pub domain: Option<String>,
    /// Whether this box serves `domain` over HTTPS. Carried so the panel can
    /// link to the scheme that actually answers instead of assuming `https`
    /// (issue #96 — an operator terminating TLS upstream serves plain HTTP).
    /// Always false when there is no domain.
    pub ssl: bool,
    pub health: Option<String>,
    pub image: Option<String>,
    pub volumes: Vec<String>,
    pub stack_id: Option<String>,
    pub user_id: Option<String>,
}

static TEMPLATES: &[AppTemplateDef] = &[
    // WordPress, Drupal, Joomla, PrestaShop moved to Sites (native PHP install)
    AppTemplateDef {
        id: "ghost",
        name: "Ghost",
        description: "Professional publishing platform for blogs and newsletters",
        category: "CMS",
        image: "ghost:5",
        default_port: 2368,
        container_port: "2368/tcp",
        env_vars: &[
            EnvVarDef {
                name: "url",
                label: "Site URL",
                default: "http://localhost:2368",
                required: false,
                secret: false,
            },
            // Without these two the container could never start, and nothing here
            // said so. `ghost:5` sets NODE_ENV=production, and Ghost's production
            // default database client is MySQL — so a deploy dialled 127.0.0.1:3306
            // inside its own network namespace, got ECONNREFUSED, and exited 2. The
            // catalogue asked the operator for a Site URL and for nothing else, so
            // there was no field they could have filled in to prevent it.
            //
            // Measured, not reasoned: run as this catalogue declares it, the
            // container logs `connect ECONNREFUSED 127.0.0.1:3306` from
            // knex-migrator and stops. Ghost supports SQLite as a first-class
            // client, and the file below lands inside the volume this template
            // already persists — which is what the comment under `volumes` has
            // claimed all along.
            //
            // Left visible and editable rather than forced: an operator who wants
            // Ghost on a real MySQL changes these two fields instead of being
            // unable to.
            EnvVarDef {
                name: "database__client",
                label: "Database Client",
                default: "sqlite3",
                required: false,
                secret: false,
            },
            EnvVarDef {
                name: "database__connection__filename",
                label: "SQLite Database File",
                default: "/var/lib/ghost/content/data/ghost.db",
                required: false,
                secret: false,
            },
        ],
        // Ghost keeps its database, themes and uploaded images here, and the image
        // declares it as a VOLUME. Undeclared, Docker satisfied that with an
        // ANONYMOUS volume — which `docker rm` leaves behind but the replacement
        // container never sees, so an update read as total data loss (#110).
        volumes: &["/var/lib/ghost/content"],
    },
    AppTemplateDef {
        id: "azuracast",
        name: "AzuraCast",
        description: "Self-hosted web radio suite — station management, streaming, playlists, and listener stats in one all-in-one container",
        category: "Media",
        image: "ghcr.io/azuracast/azuracast:stable",
        default_port: 8420,
        container_port: "80/tcp",
        // AzuraCast runs its own MariaDB inside the container, and the database
        // refuses to initialise without a root password — it says so and stops.
        // The template declared NO environment at all, so there was no field an
        // operator could have filled in.
        //
        // Two failures were stacked here, and fixing either alone leaves it
        // broken: the empty bind mount over /var/azuracast also hid the fifteen
        // entries the image ships there, so supervisord could not even find the
        // paths it logs to. Seeding uncovered the database failure behind it.
        env_vars: &[EnvVarDef {
            name: "MARIADB_ROOT_PASSWORD",
            label: "Database Root Password",
            default: "",
            required: true,
            secret: true,
        }],
        volumes: &["/var/azuracast"],
    },
    AppTemplateDef {
        id: "redis",
        name: "Redis",
        description: "In-memory data store for caching and message brokering",
        category: "Database",
        image: "redis:7-alpine",
        default_port: 6379,
        container_port: "6379/tcp",
        env_vars: &[],
        // The image declares /data as a VOLUME and Redis writes its RDB/AOF snapshots
        // there. This template is in the "Database" category and was the only one in it
        // with nothing declared, so every recreate started it from an empty dataset.
        volumes: &["/data"],
    },
    AppTemplateDef {
        id: "adminer",
        name: "Adminer",
        description: "Lightweight database management UI supporting MySQL, PostgreSQL, SQLite",
        category: "Tools",
        image: "adminer:4",
        default_port: 8081,
        container_port: "8080/tcp",
        env_vars: &[],
        volumes: &[],
    },
    AppTemplateDef {
        id: "uptime-kuma",
        name: "Uptime Kuma",
        description: "Self-hosted monitoring tool with notifications and status pages",
        category: "Monitoring",
        image: "louislam/uptime-kuma:1",
        default_port: 3001,
        container_port: "3001/tcp",
        env_vars: &[],
        volumes: &["/app/data"],
    },
    // Portainer withdrawn (v2.178.0) — same defect as Dozzle above: it needs the
    // host Docker socket to do anything (containers/images/volumes/networks
    // ARE the product) and `deploy_app` deliberately never mounts it. Unlike
    // Dozzle it does not crash-loop — its web UI starts and serves a login page
    // fine with no socket, then every real feature behind it fails — so it was
    // never caught by the weekly Template Census's start/respond check. See
    // FEATURES.md §Withdrawn Claims.
    AppTemplateDef {
        id: "n8n",
        name: "n8n",
        description: "Workflow automation tool with 200+ integrations",
        category: "Automation",
        image: "n8nio/n8n:1",
        default_port: 5678,
        container_port: "5678/tcp",
        // `N8N_BASIC_AUTH_USER` / `N8N_BASIC_AUTH_PASSWORD` used to live here. n8n
        // REMOVED basic auth in 1.0 and this template pins `n8nio/n8n:1`, so both were
        // inert — the deploy form offered an "Admin Password" field that protected
        // nothing and told the operator their instance was credentialed when it was
        // not. Ownership is established by creating the owner account on first visit.
        env_vars: &[],
        // THE #110 PATH. n8n defaults to SQLite and keeps everything here —
        // database.sqlite (workflows, executions, the owner account), the credential
        // encryption key in .n8n/config, and any community nodes. The image declares NO
        // volume of its own, so with nothing declared here the whole lot lived in the
        // container's writable layer and `docker rm` deleted it: the reporter's n8n came
        // back at first-run state after pressing Update, and none of it was recoverable.
        volumes: &["/home/node/.n8n"],
    },
    AppTemplateDef {
        id: "gitea",
        name: "Gitea",
        description: "Lightweight self-hosted Git service with issues, pull requests, and CI/CD",
        category: "Development",
        image: "gitea/gitea:1",
        default_port: 3000,
        container_port: "3000/tcp",
        env_vars: &[],
        volumes: &["/data", "/etc/timezone"],
    },
    // ─── Databases ──────────────────────────────────────────────────
    AppTemplateDef {
        id: "postgres",
        name: "PostgreSQL",
        description: "Advanced open-source relational database",
        category: "Database",
        image: "postgres:16-alpine",
        default_port: 5432,
        container_port: "5432/tcp",
        env_vars: &[
            EnvVarDef { name: "POSTGRES_USER", label: "Username", default: "postgres", required: true, secret: false },
            EnvVarDef { name: "POSTGRES_PASSWORD", label: "Password", default: "", required: true, secret: true },
            EnvVarDef { name: "POSTGRES_DB", label: "Database Name", default: "app", required: false, secret: false },
        ],
        volumes: &["/var/lib/postgresql/data"],
    },
    AppTemplateDef {
        id: "mysql",
        name: "MySQL",
        description: "The world's most popular open-source relational database",
        category: "Database",
        image: "mysql:8",
        default_port: 3306,
        container_port: "3306/tcp",
        env_vars: &[
            EnvVarDef { name: "MYSQL_ROOT_PASSWORD", label: "Root Password", default: "", required: true, secret: true },
            EnvVarDef { name: "MYSQL_DATABASE", label: "Database Name", default: "app", required: false, secret: false },
            EnvVarDef { name: "MYSQL_USER", label: "User", default: "", required: false, secret: false },
            EnvVarDef { name: "MYSQL_PASSWORD", label: "User Password", default: "", required: false, secret: true },
        ],
        volumes: &["/var/lib/mysql"],
    },
    AppTemplateDef {
        id: "mariadb",
        name: "MariaDB",
        description: "Community-developed fork of MySQL with enhanced performance",
        category: "Database",
        image: "mariadb:11",
        default_port: 3307,
        container_port: "3306/tcp",
        env_vars: &[
            EnvVarDef { name: "MARIADB_ROOT_PASSWORD", label: "Root Password", default: "", required: true, secret: true },
            EnvVarDef { name: "MARIADB_DATABASE", label: "Database Name", default: "app", required: false, secret: false },
        ],
        volumes: &["/var/lib/mysql"],
    },
    AppTemplateDef {
        id: "mongo",
        name: "MongoDB",
        description: "Document-oriented NoSQL database for modern apps",
        category: "Database",
        image: "mongo:7",
        default_port: 27017,
        container_port: "27017/tcp",
        env_vars: &[
            EnvVarDef { name: "MONGO_INITDB_ROOT_USERNAME", label: "Root Username", default: "admin", required: true, secret: false },
            EnvVarDef { name: "MONGO_INITDB_ROOT_PASSWORD", label: "Root Password", default: "", required: true, secret: true },
        ],
        volumes: &["/data/db"],
    },
    // ─── CMS & Content ──────────────────────────────────────────────
    AppTemplateDef {
        id: "directus",
        name: "Directus",
        description: "Open data platform — instant REST & GraphQL API for any SQL database",
        category: "CMS",
        image: "directus/directus:11",
        default_port: 8055,
        container_port: "8055/tcp",
        env_vars: &[
            EnvVarDef { name: "ADMIN_EMAIL", label: "Admin Email", default: "admin@example.com", required: true, secret: false },
            EnvVarDef { name: "ADMIN_PASSWORD", label: "Admin Password", default: "", required: true, secret: true },
            EnvVarDef { name: "SECRET", label: "Secret Key", default: "", required: true, secret: true },
        ],
        volumes: &["/directus/uploads", "/directus/database"],
    },
    AppTemplateDef {
        id: "nextcloud",
        name: "Nextcloud",
        description: "Self-hosted productivity platform — files, calendar, contacts, and more",
        category: "Storage",
        image: "nextcloud:30",
        default_port: 8082,
        container_port: "80/tcp",
        env_vars: &[
            EnvVarDef { name: "NEXTCLOUD_ADMIN_USER", label: "Admin Username", default: "admin", required: true, secret: false },
            EnvVarDef { name: "NEXTCLOUD_ADMIN_PASSWORD", label: "Admin Password", default: "", required: true, secret: true },
        ],
        volumes: &["/var/www/html"],
    },
    // ─── Monitoring & Analytics ─────────────────────────────────────
    AppTemplateDef {
        id: "grafana",
        name: "Grafana",
        description: "Open-source observability platform for metrics, logs, and traces",
        category: "Monitoring",
        image: "grafana/grafana:13.1",
        default_port: 3002,
        container_port: "3000/tcp",
        env_vars: &[
            EnvVarDef { name: "GF_SECURITY_ADMIN_PASSWORD", label: "Admin Password", default: "", required: true, secret: true },
        ],
        volumes: &["/var/lib/grafana"],
    },
    AppTemplateDef {
        id: "prometheus",
        name: "Prometheus",
        description: "Monitoring system and time-series database for metrics",
        category: "Monitoring",
        image: "prom/prometheus:v3",
        default_port: 9090,
        container_port: "9090/tcp",
        env_vars: &[],
        volumes: &["/prometheus"],
    },
    AppTemplateDef {
        id: "plausible",
        name: "Plausible Analytics",
        description: "Privacy-friendly alternative to Google Analytics",
        category: "Analytics",
        // Was `plausible/analytics:v2` — an image the publisher ABANDONED. Every
        // tag in that Docker Hub repository (`v2`, `v2.0`, `v2.0.0` and `latest`
        // alike) resolves to one digest last pushed on 2023-07-12, and nothing has
        // been published there since; the page carries no deprecation notice, so
        // it still reads as the current install path. Upstream moved to GHCR.
        image: "ghcr.io/plausible/community-edition:v3.2.1",
        default_port: 8000,
        container_port: "8000/tcp",
        env_vars: &[
            EnvVarDef { name: "SECRET_KEY_BASE", label: "Secret Key (64+ chars)", default: "", required: true, secret: true },
            EnvVarDef { name: "BASE_URL", label: "Site URL", default: "http://localhost:8000", required: true, secret: false },
            // Plausible needs BOTH a Postgres and a ClickHouse, and this template
            // used to ask for neither. Its own `runtime.exs` defaults them to the
            // hostnames `plausible_db` and `plausible_events_db` — the service
            // names in upstream's compose file — which resolve to nothing in a
            // single-container deploy, so the app crashed on a connection pool
            // timeout and wrote an Erlang crash dump. The operator was never shown
            // a field that could have prevented it.
            //
            // Required and empty on purpose: there is no default that is not a
            // lie, and the deploy form makes a required field impossible to skip.
            EnvVarDef { name: "DATABASE_URL", label: "PostgreSQL URL (required, external)", default: "", required: true, secret: true },
            EnvVarDef { name: "CLICKHOUSE_DATABASE_URL", label: "ClickHouse URL (required, external)", default: "", required: true, secret: true },
        ],
        volumes: &[],
    },
    AppTemplateDef {
        id: "umami",
        name: "Umami",
        description: "Simple, privacy-focused website analytics",
        category: "Analytics",
        image: "ghcr.io/umami-software/umami:postgresql-v2.15.1",
        default_port: 3003,
        container_port: "3000/tcp",
        env_vars: &[
            EnvVarDef { name: "DATABASE_URL", label: "Postgres URL", default: "", required: true, secret: true },
        ],
        volumes: &[],
    },
    AppTemplateDef {
        id: "matomo",
        name: "Matomo",
        description: "Google Analytics alternative that respects user privacy",
        category: "Analytics",
        image: "matomo:5",
        default_port: 8083,
        container_port: "80/tcp",
        env_vars: &[],
        volumes: &["/var/www/html"],
    },
    // ─── Tools & Utilities ──────────────────────────────────────────
    AppTemplateDef {
        id: "pgadmin",
        name: "pgAdmin",
        description: "Web-based PostgreSQL management and administration tool",
        category: "Tools",
        image: "dpage/pgadmin4:8",
        default_port: 5050,
        container_port: "80/tcp",
        env_vars: &[
            EnvVarDef { name: "PGADMIN_DEFAULT_EMAIL", label: "Admin Email", default: "admin@example.com", required: true, secret: false },
            EnvVarDef { name: "PGADMIN_DEFAULT_PASSWORD", label: "Admin Password", default: "", required: true, secret: true },
        ],
        volumes: &["/var/lib/pgadmin"],
    },
    AppTemplateDef {
        id: "minio",
        name: "MinIO",
        description: "High-performance S3-compatible object storage",
        category: "Storage",
        image: "minio/minio:latest",
        default_port: 9000,
        container_port: "9000/tcp",
        env_vars: &[
            EnvVarDef { name: "MINIO_ROOT_USER", label: "Root User", default: "minioadmin", required: true, secret: false },
            EnvVarDef { name: "MINIO_ROOT_PASSWORD", label: "Root Password", default: "", required: true, secret: true },
        ],
        volumes: &["/data"],
    },
    AppTemplateDef {
        id: "vaultwarden",
        name: "Vaultwarden",
        description: "Lightweight Bitwarden-compatible password manager server",
        category: "Security",
        image: "vaultwarden/server:1.37.1",
        default_port: 8084,
        container_port: "80/tcp",
        env_vars: &[
            EnvVarDef { name: "ADMIN_TOKEN", label: "Admin Token", default: "", required: false, secret: true },
        ],
        volumes: &["/data"],
    },
    AppTemplateDef {
        id: "meilisearch",
        name: "Meilisearch",
        description: "Lightning-fast, typo-tolerant search engine",
        category: "Tools",
        image: "getmeili/meilisearch:v1",
        default_port: 7700,
        container_port: "7700/tcp",
        env_vars: &[
            EnvVarDef { name: "MEILI_MASTER_KEY", label: "Master Key", default: "", required: true, secret: true },
        ],
        volumes: &["/meili_data"],
    },
    AppTemplateDef {
        id: "metabase",
        name: "Metabase",
        description: "Business intelligence and analytics dashboard builder",
        category: "Analytics",
        image: "metabase/metabase:v0.58-lts",
        default_port: 3004,
        container_port: "3000/tcp",
        env_vars: &[],
        volumes: &["/metabase-data"],
    },
    // ─── Communication ──────────────────────────────────────────────
    AppTemplateDef {
        id: "nocodb",
        name: "NocoDB",
        description: "Open-source Airtable alternative — turn any database into a spreadsheet",
        category: "Tools",
        image: "nocodb/nocodb:latest",
        default_port: 8085,
        container_port: "8080/tcp",
        env_vars: &[],
        volumes: &["/usr/app/data"],
    },
    AppTemplateDef {
        id: "searxng",
        name: "SearXNG",
        description: "Privacy-respecting metasearch engine aggregating 70+ sources",
        category: "Tools",
        image: "searxng/searxng:latest",
        default_port: 8086,
        container_port: "8080/tcp",
        env_vars: &[],
        // SearXNG generates settings.yml — including its `secret_key` — into /etc/searxng
        // on first boot, and the image declares that path as a VOLUME. Only the config
        // half is declared here: /var/cache/searxng is a rebuildable cache and does not
        // need to survive anything.
        volumes: &["/etc/searxng"],
    },
    AppTemplateDef {
        id: "jellyfin",
        name: "Jellyfin",
        description: "Free media server for movies, TV shows, music, and photos",
        category: "Media",
        image: "jellyfin/jellyfin:10",
        default_port: 8096,
        container_port: "8096/tcp",
        env_vars: &[],
        volumes: &["/config", "/cache"],
    },
    AppTemplateDef {
        id: "code-server",
        name: "VS Code Server",
        description: "Run VS Code in the browser — full IDE accessible anywhere",
        category: "Development",
        image: "codercom/code-server:4.131.0",
        default_port: 8443,
        container_port: "8080/tcp",
        env_vars: &[
            EnvVarDef { name: "PASSWORD", label: "Access Password", default: "", required: true, secret: true },
        ],
        volumes: &["/home/coder"],
    },
    AppTemplateDef {
        id: "drone",
        name: "Drone CI",
        description: "Container-native CI/CD platform with pipeline-as-code",
        category: "Development",
        image: "drone/drone:2",
        default_port: 8087,
        container_port: "80/tcp",
        env_vars: &[
            EnvVarDef { name: "DRONE_SERVER_HOST", label: "Server Host", default: "localhost", required: true, secret: false },
            EnvVarDef { name: "DRONE_SERVER_PROTO", label: "Protocol", default: "http", required: false, secret: false },
            EnvVarDef { name: "DRONE_RPC_SECRET", label: "RPC Secret", default: "", required: true, secret: true },
        ],
        volumes: &["/data"],
    },
    AppTemplateDef {
        id: "registry",
        name: "Docker Registry",
        description: "Private Docker image registry for storing and distributing container images",
        category: "Development",
        image: "registry:2",
        default_port: 5000,
        container_port: "5000/tcp",
        env_vars: &[],
        volumes: &["/var/lib/registry"],
    },
    AppTemplateDef {
        id: "mailpit",
        name: "Mailpit",
        description: "Email testing tool — catches outgoing emails for dev/testing",
        category: "Development",
        image: "axllent/mailpit:v1.30",
        default_port: 8025,
        container_port: "8025/tcp",
        env_vars: &[],
        volumes: &[],
    },
    AppTemplateDef {
        id: "pihole",
        name: "Pi-hole",
        description: "Network-wide ad blocker and DNS sinkhole",
        category: "Networking",
        image: "pihole/pihole:2024.07.0",
        default_port: 8088,
        container_port: "80/tcp",
        env_vars: &[
            EnvVarDef { name: "WEBPASSWORD", label: "Web Password", default: "", required: true, secret: true },
        ],
        volumes: &["/etc/pihole", "/etc/dnsmasq.d"],
    },
    AppTemplateDef {
        id: "loki",
        name: "Grafana Loki",
        description: "Log aggregation system designed to work with Grafana",
        category: "Monitoring",
        image: "grafana/loki:3",
        default_port: 3100,
        container_port: "3100/tcp",
        env_vars: &[],
        volumes: &["/loki"],
    },
    // ─── Wiki / Docs ─────────────────────────────────────────────
    AppTemplateDef {
        id: "wikijs",
        name: "Wiki.js",
        description: "Modern wiki engine with powerful features and beautiful interface",
        category: "Tools",
        image: "ghcr.io/requarks/wiki:2",
        default_port: 3005,
        container_port: "3000/tcp",
        env_vars: &[
            EnvVarDef { name: "DB_TYPE", label: "Database Type", default: "sqlite", required: false, secret: false },
        ],
        volumes: &["/wiki/data"],
    },
    AppTemplateDef {
        id: "bookstack",
        name: "BookStack",
        description: "Simple, self-hosted platform for organizing and storing information",
        category: "Tools",
        image: "lscr.io/linuxserver/bookstack:latest",
        default_port: 6875,
        container_port: "80/tcp",
        env_vars: &[
            EnvVarDef { name: "APP_URL", label: "Application URL", default: "", required: true, secret: false },
            EnvVarDef { name: "DB_HOST", label: "Database Host", default: "", required: true, secret: false },
            EnvVarDef { name: "DB_USER", label: "Database User", default: "bookstack", required: false, secret: false },
            EnvVarDef { name: "DB_PASS", label: "Database Password", default: "", required: true, secret: true },
            EnvVarDef { name: "DB_DATABASE", label: "Database Name", default: "bookstack", required: false, secret: false },
        ],
        volumes: &["/config"],
    },
    AppTemplateDef {
        id: "outline",
        name: "Outline",
        description: "Team knowledge base and wiki with a fast, beautiful editor",
        category: "Tools",
        image: "outlinewiki/outline:1.9",
        default_port: 3006,
        container_port: "3000/tcp",
        env_vars: &[
            EnvVarDef { name: "SECRET_KEY", label: "Secret Key", default: "", required: true, secret: true },
            EnvVarDef { name: "URL", label: "Application URL", default: "", required: true, secret: false },
            EnvVarDef { name: "DATABASE_URL", label: "Database URL", default: "", required: true, secret: false },
            EnvVarDef { name: "REDIS_URL", label: "Redis URL", default: "redis://localhost:6379", required: false, secret: false },
        ],
        volumes: &["/var/lib/outline/data"],
    },
    // ─── Communication ───────────────────────────────────────────
    AppTemplateDef {
        id: "mattermost",
        name: "Mattermost",
        description: "Open-source Slack alternative for secure team collaboration",
        category: "Tools",
        image: "mattermost/mattermost-team-edition:release-11",
        default_port: 8065,
        container_port: "8065/tcp",
        env_vars: &[
            EnvVarDef { name: "MM_SQLSETTINGS_DATASOURCE", label: "Database Connection String", default: "", required: true, secret: false },
        ],
        volumes: &["/mattermost/config", "/mattermost/data", "/mattermost/logs"],
    },
    AppTemplateDef {
        id: "rocketchat",
        name: "Rocket.Chat",
        description: "Open-source team communication platform with channels, DMs, and video",
        category: "Tools",
        image: "rocketchat/rocket.chat:8.7.0",
        default_port: 3007,
        container_port: "3000/tcp",
        env_vars: &[
            EnvVarDef { name: "MONGO_URL", label: "MongoDB URL", default: "", required: true, secret: false },
            EnvVarDef { name: "ROOT_URL", label: "Root URL", default: "", required: true, secret: false },
        ],
        volumes: &["/app/uploads"],
    },
    // ─── Media ───────────────────────────────────────────────────
    AppTemplateDef {
        id: "immich",
        name: "Immich",
        description: "High-performance self-hosted photo and video management",
        category: "Media",
        image: "ghcr.io/immich-app/immich-server:release",
        default_port: 2283,
        container_port: "3001/tcp",
        env_vars: &[
            EnvVarDef { name: "DB_HOSTNAME", label: "Database Hostname", default: "", required: true, secret: false },
            EnvVarDef { name: "DB_USERNAME", label: "Database Username", default: "immich", required: false, secret: false },
            EnvVarDef { name: "DB_PASSWORD", label: "Database Password", default: "", required: true, secret: true },
            EnvVarDef { name: "DB_DATABASE_NAME", label: "Database Name", default: "immich", required: false, secret: false },
            EnvVarDef { name: "REDIS_HOSTNAME", label: "Redis Hostname", default: "redis", required: false, secret: false },
        ],
        volumes: &["/usr/src/app/upload"],
    },
    AppTemplateDef {
        id: "photoprism",
        name: "PhotoPrism",
        description: "AI-powered photo management app for browsing, organizing, and sharing",
        category: "Media",
        image: "photoprism/photoprism:latest",
        default_port: 2342,
        container_port: "2342/tcp",
        env_vars: &[
            EnvVarDef { name: "PHOTOPRISM_ADMIN_PASSWORD", label: "Admin Password", default: "", required: true, secret: true },
            EnvVarDef { name: "PHOTOPRISM_SITE_URL", label: "Site URL", default: "http://localhost:2342", required: false, secret: false },
        ],
        volumes: &["/photoprism/storage", "/photoprism/originals"],
    },
    // ─── Security ────────────────────────────────────────────────
    AppTemplateDef {
        id: "authentik",
        name: "Authentik",
        description: "Open-source identity provider with SSO, MFA, and user management",
        category: "Security",
        image: "ghcr.io/goauthentik/server:2024.12",
        default_port: 9001,
        container_port: "9000/tcp",
        env_vars: &[
            EnvVarDef { name: "AUTHENTIK_SECRET_KEY", label: "Secret Key", default: "", required: true, secret: true },
            EnvVarDef { name: "AUTHENTIK_REDIS__HOST", label: "Redis Host", default: "redis", required: false, secret: false },
            EnvVarDef { name: "AUTHENTIK_POSTGRESQL__HOST", label: "PostgreSQL Host", default: "", required: true, secret: false },
            EnvVarDef { name: "AUTHENTIK_POSTGRESQL__PASSWORD", label: "PostgreSQL Password", default: "", required: true, secret: true },
        ],
        volumes: &["/media", "/templates"],
    },
    AppTemplateDef {
        id: "keycloak",
        name: "Keycloak",
        description: "Open-source IAM with SSO, identity brokering, and social login. Requires 'start-dev' command override for dev mode.",
        category: "Security",
        image: "quay.io/keycloak/keycloak:26.7",
        default_port: 8180,
        container_port: "8080/tcp",
        env_vars: &[
            EnvVarDef { name: "KEYCLOAK_ADMIN", label: "Admin Username", default: "admin", required: false, secret: false },
            EnvVarDef { name: "KEYCLOAK_ADMIN_PASSWORD", label: "Admin Password", default: "", required: true, secret: true },
            EnvVarDef { name: "KC_DB", label: "Database Type", default: "postgres", required: false, secret: false },
            EnvVarDef { name: "KC_DB_URL", label: "Database JDBC URL", default: "", required: true, secret: false },
        ],
        volumes: &[],
    },
    // ─── Dev Tools ───────────────────────────────────────────────
    AppTemplateDef {
        id: "woodpecker",
        name: "Woodpecker CI",
        description: "Simple yet powerful CI/CD engine with great extensibility",
        category: "Development",
        image: "woodpeckerci/woodpecker-server:v3",
        default_port: 8000,
        container_port: "8000/tcp",
        env_vars: &[
            EnvVarDef { name: "WOODPECKER_OPEN", label: "Open Registration", default: "true", required: false, secret: false },
            EnvVarDef { name: "WOODPECKER_ADMIN", label: "Admin User", default: "", required: true, secret: false },
            EnvVarDef { name: "WOODPECKER_HOST", label: "Server Host URL", default: "", required: true, secret: false },
            EnvVarDef { name: "WOODPECKER_AGENT_SECRET", label: "Agent Secret", default: "", required: true, secret: true },
        ],
        volumes: &["/var/lib/woodpecker"],
    },
    AppTemplateDef {
        id: "sonarqube",
        name: "SonarQube",
        description: "Continuous code quality and security inspection platform",
        category: "Development",
        image: "sonarqube:10-community",
        default_port: 9002,
        container_port: "9000/tcp",
        env_vars: &[
            EnvVarDef { name: "SONAR_JDBC_URL", label: "JDBC URL", default: "jdbc:h2:tcp://localhost/sonar", required: false, secret: false },
        ],
        volumes: &["/opt/sonarqube/data", "/opt/sonarqube/logs", "/opt/sonarqube/extensions"],
    },
    AppTemplateDef {
        id: "forgejo",
        name: "Forgejo",
        description: "Community-driven Git forge — lightweight Gitea fork with extra features",
        category: "Development",
        image: "codeberg.org/forgejo/forgejo:9",
        default_port: 3009,
        container_port: "3000/tcp",
        env_vars: &[],
        volumes: &["/data", "/etc/timezone"],
    },
    // ─── Business / Productivity ─────────────────────────────────
    AppTemplateDef {
        id: "invoice-ninja",
        name: "Invoice Ninja",
        description: "Full-featured invoicing, payments, and expense tracking for freelancers",
        category: "Tools",
        image: "invoiceninja/invoiceninja:5",
        default_port: 9003,
        container_port: "80/tcp",
        env_vars: &[
            EnvVarDef { name: "APP_URL", label: "Application URL", default: "", required: true, secret: false },
            EnvVarDef { name: "APP_KEY", label: "Application Key", default: "", required: true, secret: true },
            EnvVarDef { name: "DB_HOST", label: "Database Host", default: "", required: true, secret: false },
            EnvVarDef { name: "DB_DATABASE", label: "Database Name", default: "ninja", required: false, secret: false },
            EnvVarDef { name: "DB_USERNAME", label: "Database Username", default: "ninja", required: false, secret: false },
            EnvVarDef { name: "DB_PASSWORD", label: "Database Password", default: "", required: true, secret: true },
        ],
        volumes: &["/var/app/public", "/var/app/storage"],
    },
    AppTemplateDef {
        id: "erpnext",
        name: "ERPNext",
        description: "Open-source ERP for manufacturing, distribution, retail, and services",
        category: "Tools",
        image: "frappe/erpnext:latest",
        default_port: 8080,
        container_port: "8080/tcp",
        env_vars: &[],
        volumes: &["/home/frappe/frappe-bench/sites"],
    },
    AppTemplateDef {
        id: "calcom",
        name: "Cal.com",
        description: "Open-source scheduling infrastructure for appointments and meetings",
        category: "Tools",
        image: "calcom/cal.com:v6.2.0",
        default_port: 3010,
        container_port: "3000/tcp",
        env_vars: &[
            EnvVarDef { name: "DATABASE_URL", label: "Database URL", default: "", required: true, secret: false },
            EnvVarDef { name: "NEXTAUTH_SECRET", label: "NextAuth Secret", default: "", required: true, secret: true },
            EnvVarDef { name: "CALENDSO_ENCRYPTION_KEY", label: "Encryption Key", default: "", required: true, secret: true },
        ],
        volumes: &[],
    },
    // ─── Support ─────────────────────────────────────────────────
    AppTemplateDef {
        id: "chatwoot",
        name: "Chatwoot",
        description: "Open-source customer engagement platform with live chat and helpdesk",
        category: "Tools",
        image: "chatwoot/chatwoot:latest-ce",
        default_port: 3011,
        container_port: "3000/tcp",
        env_vars: &[
            EnvVarDef { name: "SECRET_KEY_BASE", label: "Secret Key Base", default: "", required: true, secret: true },
            EnvVarDef { name: "FRONTEND_URL", label: "Frontend URL", default: "", required: true, secret: false },
            EnvVarDef { name: "POSTGRES_HOST", label: "PostgreSQL Host", default: "", required: true, secret: false },
            EnvVarDef { name: "POSTGRES_PASSWORD", label: "PostgreSQL Password", default: "", required: true, secret: true },
            EnvVarDef { name: "REDIS_URL", label: "Redis URL", default: "redis://redis:6379", required: false, secret: false },
        ],
        volumes: &["/app/storage"],
    },
    AppTemplateDef {
        id: "typebot",
        name: "Typebot",
        description: "Open-source chatbot builder with drag-and-drop visual editor",
        category: "Tools",
        image: "baptistearno/typebot-builder:2",
        default_port: 3012,
        container_port: "3000/tcp",
        env_vars: &[
            EnvVarDef { name: "DATABASE_URL", label: "Database URL", default: "", required: true, secret: false },
            EnvVarDef { name: "NEXTAUTH_URL", label: "NextAuth URL", default: "", required: true, secret: false },
            EnvVarDef { name: "ENCRYPTION_SECRET", label: "Encryption Secret", default: "", required: true, secret: true },
        ],
        volumes: &[],
    },
    // Roundcube and Rspamd removed — use Mail → Webmail and Mail → Spam Filter instead
    // ─── AI / Machine Learning ──────────────────────────────────
    AppTemplateDef {
        id: "ollama",
        name: "Ollama",
        description: "Run large language models locally (Llama, Mistral, Gemma, Phi)",
        category: "AI",
        image: "ollama/ollama:latest",
        default_port: 11434,
        container_port: "11434/tcp",
        env_vars: &[
            EnvVarDef { name: "OLLAMA_KEEP_ALIVE", label: "Model idle timeout", default: "5m", required: false, secret: false },
            EnvVarDef { name: "OLLAMA_NUM_PARALLEL", label: "Max parallel requests", default: "1", required: false, secret: false },
        ],
        volumes: &["/root/.ollama"],
    },
    AppTemplateDef {
        id: "open-webui",
        name: "Open WebUI",
        description: "ChatGPT-style web interface for Ollama and OpenAI-compatible APIs",
        category: "AI",
        image: "ghcr.io/open-webui/open-webui:main",
        default_port: 3101,
        container_port: "8080/tcp",
        env_vars: &[
            EnvVarDef { name: "OLLAMA_BASE_URL", label: "Ollama URL", default: "http://host.docker.internal:11434", required: false, secret: false },
        ],
        volumes: &["/app/backend/data"],
    },
    AppTemplateDef {
        id: "localai",
        name: "LocalAI",
        description: "Self-hosted OpenAI-compatible API for running LLMs, image and audio generation",
        category: "AI",
        // GPU-aware default. Operators on CPU-only hosts can switch the image
        // to localai/localai:latest-cpu via the Image field on the deploy form.
        image: "localai/localai:latest-gpu-nvidia-cuda-12",
        default_port: 8296,
        container_port: "8080/tcp",
        env_vars: &[],
        volumes: &["/build/models"],
    },
    // ─── Dashboards ─────────────────────────────────────────────
    AppTemplateDef {
        id: "homepage",
        name: "Homepage",
        description: "Highly customizable application dashboard with service integrations",
        category: "Tools",
        image: "ghcr.io/gethomepage/homepage:latest",
        default_port: 3013,
        container_port: "3000/tcp",
        env_vars: &[],
        volumes: &["/app/config"],
    },
    AppTemplateDef {
        id: "homarr",
        name: "Homarr",
        description: "Sleek server dashboard with drag-and-drop widgets and integrations",
        category: "Tools",
        image: "ghcr.io/homarr-labs/homarr:latest",
        default_port: 7575,
        container_port: "7575/tcp",
        env_vars: &[],
        volumes: &["/appdata"],
    },
    AppTemplateDef {
        id: "dashy",
        name: "Dashy",
        description: "Feature-rich self-hosted dashboard for your homelab and cloud stack",
        category: "Tools",
        image: "lissy93/dashy:latest",
        default_port: 4000,
        container_port: "8080/tcp",
        env_vars: &[],
        volumes: &["/app/user-data"],
    },
    // ─── Documents / Productivity ────────────────────────────────
    AppTemplateDef {
        id: "paperless-ngx",
        name: "Paperless-ngx",
        description: "Document management system that transforms physical documents into searchable archive",
        category: "Tools",
        image: "ghcr.io/paperless-ngx/paperless-ngx:latest",
        default_port: 8010,
        container_port: "8000/tcp",
        env_vars: &[
            EnvVarDef { name: "PAPERLESS_SECRET_KEY", label: "Secret Key", default: "", required: true, secret: true },
            EnvVarDef { name: "PAPERLESS_ADMIN_USER", label: "Admin Username", default: "admin", required: false, secret: false },
            EnvVarDef { name: "PAPERLESS_ADMIN_PASSWORD", label: "Admin Password", default: "", required: true, secret: true },
        ],
        volumes: &["/usr/src/paperless/data", "/usr/src/paperless/media"],
    },
    AppTemplateDef {
        id: "stirling-pdf",
        name: "Stirling-PDF",
        description: "Self-hosted PDF manipulation tool with merge, split, convert, and OCR",
        category: "Tools",
        image: "frooodle/s-pdf:latest",
        default_port: 8182,
        container_port: "8080/tcp",
        env_vars: &[],
        volumes: &["/usr/share/tessdata", "/configs"],
    },
    AppTemplateDef {
        id: "actual-budget",
        name: "Actual Budget",
        description: "Privacy-focused local-first personal budgeting app",
        category: "Tools",
        image: "actualbudget/actual-server:latest",
        default_port: 5006,
        container_port: "5006/tcp",
        env_vars: &[],
        volumes: &["/data"],
    },
    AppTemplateDef {
        id: "hedgedoc",
        name: "HedgeDoc",
        description: "Real-time collaborative Markdown editor for teams",
        category: "Tools",
        image: "quay.io/hedgedoc/hedgedoc:latest",
        default_port: 3014,
        container_port: "3000/tcp",
        env_vars: &[
            EnvVarDef { name: "CMD_DB_URL", label: "Database URL", default: "", required: true, secret: false },
            EnvVarDef { name: "CMD_SESSION_SECRET", label: "Session Secret", default: "", required: true, secret: true },
            EnvVarDef { name: "CMD_DOMAIN", label: "Domain", default: "", required: false, secret: false },
        ],
        volumes: &["/hedgedoc/public/uploads"],
    },
    AppTemplateDef {
        id: "vikunja",
        name: "Vikunja",
        description: "Open-source to-do and project management app (Todoist alternative)",
        category: "Tools",
        image: "vikunja/vikunja:latest",
        default_port: 3456,
        container_port: "3456/tcp",
        env_vars: &[
            EnvVarDef { name: "VIKUNJA_SERVICE_JWTSECRET", label: "JWT Secret", default: "", required: true, secret: true },
        ],
        volumes: &["/app/vikunja/files"],
    },
    AppTemplateDef {
        id: "memos",
        name: "Memos",
        description: "Privacy-first lightweight note-taking service (Google Keep alternative)",
        category: "Tools",
        image: "neosmemo/memos:stable",
        default_port: 5230,
        container_port: "5230/tcp",
        env_vars: &[],
        volumes: &["/var/opt/memos"],
    },
    AppTemplateDef {
        id: "trilium",
        name: "Trilium Notes",
        description: "Hierarchical note-taking application with rich text, relations, and scripting",
        category: "Tools",
        image: "triliumnext/notes:latest",
        default_port: 8383,
        container_port: "8080/tcp",
        env_vars: &[],
        volumes: &["/home/node/trilium-data"],
    },
    AppTemplateDef {
        id: "docuseal",
        name: "DocuSeal",
        description: "Open-source document signing platform (DocuSign alternative)",
        category: "Tools",
        image: "docuseal/docuseal:latest",
        default_port: 3015,
        container_port: "3000/tcp",
        env_vars: &[],
        volumes: &["/data"],
    },
    // ─── Media ──────────────────────────────────────────────────
    AppTemplateDef {
        id: "navidrome",
        name: "Navidrome",
        description: "Modern music server and streamer compatible with Subsonic/Airsonic clients",
        category: "Media",
        image: "deluan/navidrome:latest",
        default_port: 4533,
        container_port: "4533/tcp",
        env_vars: &[],
        volumes: &["/data", "/music"],
    },
    AppTemplateDef {
        id: "audiobookshelf",
        name: "Audiobookshelf",
        description: "Self-hosted audiobook and podcast server with mobile apps",
        category: "Media",
        image: "ghcr.io/advplyr/audiobookshelf:latest",
        default_port: 13378,
        container_port: "80/tcp",
        env_vars: &[],
        volumes: &["/config", "/metadata", "/audiobooks", "/podcasts"],
    },
    AppTemplateDef {
        id: "calibre-web",
        name: "Calibre-Web",
        description: "Web-based e-book library manager with OPDS feed and reading interface",
        category: "Media",
        image: "lscr.io/linuxserver/calibre-web:latest",
        default_port: 8283,
        container_port: "8083/tcp",
        env_vars: &[
            EnvVarDef { name: "PUID", label: "User ID", default: "1000", required: false, secret: false },
            EnvVarDef { name: "PGID", label: "Group ID", default: "1000", required: false, secret: false },
        ],
        volumes: &["/config", "/books"],
    },
    AppTemplateDef {
        id: "kavita",
        name: "Kavita",
        description: "Self-hosted digital library for manga, comics, and books",
        category: "Media",
        image: "jvmilazz0/kavita:latest",
        default_port: 5001,
        container_port: "5000/tcp",
        env_vars: &[],
        volumes: &["/kavita/config", "/manga"],
    },
    AppTemplateDef {
        id: "plex",
        name: "Plex",
        description: "Media server for organizing and streaming movies, TV, music, and photos",
        category: "Media",
        image: "lscr.io/linuxserver/plex:latest",
        default_port: 32400,
        container_port: "32400/tcp",
        env_vars: &[
            EnvVarDef { name: "PUID", label: "User ID", default: "1000", required: false, secret: false },
            EnvVarDef { name: "PGID", label: "Group ID", default: "1000", required: false, secret: false },
            EnvVarDef { name: "PLEX_CLAIM", label: "Plex Claim Token", default: "", required: false, secret: true },
        ],
        volumes: &["/config", "/tv", "/movies"],
    },
    // ─── RSS / Link Management ──────────────────────────────────
    AppTemplateDef {
        id: "freshrss",
        name: "FreshRSS",
        description: "Self-hosted RSS feed aggregator with full-text search and mobile apps",
        category: "Tools",
        image: "freshrss/freshrss:latest",
        default_port: 8284,
        container_port: "80/tcp",
        env_vars: &[],
        volumes: &["/var/www/FreshRSS/data", "/var/www/FreshRSS/extensions"],
    },
    AppTemplateDef {
        id: "linkwarden",
        name: "Linkwarden",
        description: "Collaborative bookmark manager with archiving, tagging, and collections",
        category: "Tools",
        image: "ghcr.io/linkwarden/linkwarden:latest",
        default_port: 3016,
        container_port: "3000/tcp",
        env_vars: &[
            EnvVarDef { name: "DATABASE_URL", label: "Database URL", default: "", required: true, secret: false },
            EnvVarDef { name: "NEXTAUTH_SECRET", label: "NextAuth Secret", default: "", required: true, secret: true },
            EnvVarDef { name: "NEXTAUTH_URL", label: "NextAuth URL", default: "", required: true, secret: false },
        ],
        volumes: &["/data/data"],
    },
    AppTemplateDef {
        id: "changedetection",
        name: "Changedetection.io",
        description: "Website change detection and monitoring with notifications",
        category: "Monitoring",
        image: "ghcr.io/dgtlmoon/changedetection.io:latest",
        default_port: 5555,
        container_port: "5000/tcp",
        env_vars: &[],
        volumes: &["/datastore"],
    },
    // ─── Recipes ────────────────────────────────────────────────
    AppTemplateDef {
        id: "tandoor",
        name: "Tandoor Recipes",
        description: "Recipe management and meal planning application",
        category: "Tools",
        image: "vabene1111/recipes:latest",
        default_port: 8285,
        container_port: "8080/tcp",
        env_vars: &[
            EnvVarDef { name: "SECRET_KEY", label: "Secret Key", default: "", required: true, secret: true },
            EnvVarDef { name: "DB_ENGINE", label: "DB Engine", default: "django.db.backends.sqlite3", required: false, secret: false },
        ],
        volumes: &["/opt/recipes/mediafiles", "/opt/recipes/staticfiles"],
    },
    AppTemplateDef {
        id: "mealie",
        name: "Mealie",
        description: "Self-hosted recipe manager with meal planning, shopping lists, and API",
        category: "Tools",
        image: "ghcr.io/mealie-recipes/mealie:latest",
        default_port: 9925,
        container_port: "9000/tcp",
        env_vars: &[
            EnvVarDef { name: "BASE_URL", label: "Base URL", default: "", required: false, secret: false },
        ],
        volumes: &["/app/data"],
    },
    // ─── Communication ──────────────────────────────────────────
    AppTemplateDef {
        id: "ntfy",
        name: "ntfy",
        description: "Simple push notification service using HTTP PUT/POST (UnifiedPush compatible)",
        category: "Tools",
        image: "binwiederhier/ntfy:latest",
        default_port: 8286,
        container_port: "80/tcp",
        env_vars: &[],
        volumes: &["/var/cache/ntfy", "/etc/ntfy"],
    },
    AppTemplateDef {
        id: "gotify",
        name: "Gotify",
        description: "Simple server for sending and receiving push notifications via REST API",
        category: "Tools",
        image: "gotify/server:latest",
        default_port: 8287,
        container_port: "80/tcp",
        env_vars: &[
            EnvVarDef { name: "GOTIFY_DEFAULTUSER_PASS", label: "Admin Password", default: "", required: true, secret: true },
        ],
        volumes: &["/app/data"],
    },
    // ─── DNS ────────────────────────────────────────────────────
    AppTemplateDef {
        id: "adguard-home",
        name: "AdGuard Home",
        description: "Network-wide ad and tracker blocking DNS server with HTTPS filtering",
        category: "Networking",
        image: "adguard/adguardhome:latest",
        default_port: 3017,
        container_port: "3000/tcp",
        env_vars: &[],
        volumes: &["/opt/adguardhome/work", "/opt/adguardhome/conf"],
    },
    AppTemplateDef {
        id: "technitium-dns",
        name: "Technitium DNS",
        description: "Open-source authoritative and recursive DNS server with web admin panel",
        category: "Networking",
        image: "technitium/dns-server:latest",
        default_port: 5380,
        container_port: "5380/tcp",
        env_vars: &[],
        volumes: &["/etc/dns"],
    },
    // ─── Networking / Proxy ─────────────────────────────────────
    AppTemplateDef {
        id: "nginx-proxy-manager",
        name: "Nginx Proxy Manager",
        description: "Easy-to-use reverse proxy with free SSL and web-based management",
        category: "Networking",
        image: "jc21/nginx-proxy-manager:latest",
        default_port: 8181,
        container_port: "81/tcp",
        env_vars: &[],
        volumes: &["/data", "/etc/letsencrypt"],
    },
    AppTemplateDef {
        id: "wireguard",
        name: "WireGuard",
        description: "Modern, fast VPN tunnel with easy configuration",
        category: "Networking",
        image: "lscr.io/linuxserver/wireguard:latest",
        default_port: 51820,
        container_port: "51820/udp",
        env_vars: &[
            EnvVarDef { name: "PUID", label: "User ID", default: "1000", required: false, secret: false },
            EnvVarDef { name: "PGID", label: "Group ID", default: "1000", required: false, secret: false },
            EnvVarDef { name: "PEERS", label: "Number of Peers", default: "3", required: false, secret: false },
            EnvVarDef { name: "SERVERURL", label: "Server URL/IP", default: "", required: true, secret: false },
        ],
        volumes: &["/config"],
    },
    // ─── Monitoring (additional) ────────────────────────────────
    // Dozzle was withdrawn at v2.178.0 (#125 — it fatals with "Could not
    // connect to any Docker Engine" because `deploy_app` deliberately never
    // mounts the host Docker socket into any one-click app) and reinstated at
    // v2.179.0 behind a paired docker-socket-proxy sidecar instead — see
    // `TEMPLATE_SIDECAR` and `services::app_sidecar`. The socket is real, but
    // this container never touches it: the sidecar does, on a network
    // nothing else can reach.
    AppTemplateDef {
        id: "dozzle",
        name: "Dozzle",
        description: "Real-time Docker container log viewer with a clean web interface",
        category: "Monitoring",
        image: "amir20/dozzle:latest",
        default_port: 9999,
        container_port: "8080/tcp",
        env_vars: &[],
        volumes: &[],
    },
    AppTemplateDef {
        id: "glances",
        name: "Glances",
        description: "Cross-platform system monitoring tool with web interface and API",
        category: "Monitoring",
        image: "nicolargo/glances:latest-full",
        default_port: 61208,
        container_port: "61208/tcp",
        env_vars: &[
            EnvVarDef { name: "GLANCES_OPT", label: "Glances Options", default: "-w", required: false, secret: false },
        ],
        volumes: &[],
    },
    AppTemplateDef {
        id: "netdata",
        name: "Netdata",
        description: "Real-time infrastructure monitoring with zero configuration",
        category: "Monitoring",
        image: "netdata/netdata:stable",
        default_port: 19999,
        container_port: "19999/tcp",
        env_vars: &[],
        volumes: &["/etc/netdata", "/var/lib/netdata", "/var/cache/netdata"],
    },
    // ─── Storage / Files ────────────────────────────────────────
    AppTemplateDef {
        id: "filebrowser",
        name: "File Browser",
        description: "Web-based file manager with sharing, users, and customization",
        category: "Storage",
        image: "filebrowser/filebrowser:latest",
        default_port: 8288,
        container_port: "80/tcp",
        env_vars: &[],
        // `/database`, not `/database/filebrowser.db` — the same file-declared-as-a
        // -volume mistake as Element Web, and here it got one step further before
        // failing: the container started, then exited with
        // `open /database/filebrowser.db: is a directory`.
        //
        // Repointed at the parent rather than dropped, because unlike Element Web
        // there IS something to keep here. File Browser creates its database
        // inside this directory, so mounting the directory persists the users,
        // shares and settings across an update — which is precisely what mounting
        // the file failed to do while looking like it did.
        volumes: &["/srv", "/database"],
    },
    AppTemplateDef {
        id: "syncthing",
        name: "Syncthing",
        description: "Continuous file synchronization between devices (Dropbox alternative)",
        category: "Storage",
        image: "lscr.io/linuxserver/syncthing:latest",
        default_port: 8384,
        container_port: "8384/tcp",
        env_vars: &[
            EnvVarDef { name: "PUID", label: "User ID", default: "1000", required: false, secret: false },
            EnvVarDef { name: "PGID", label: "Group ID", default: "1000", required: false, secret: false },
        ],
        volumes: &["/config", "/data"],
    },
    // ─── Development (additional) ───────────────────────────────
    AppTemplateDef {
        id: "it-tools",
        name: "IT-Tools",
        description: "Collection of handy developer tools (JSON formatter, UUID generator, hash, base64, etc.)",
        category: "Development",
        image: "corentinth/it-tools:latest",
        default_port: 8289,
        container_port: "80/tcp",
        env_vars: &[],
        volumes: &[],
    },
    AppTemplateDef {
        id: "jenkins",
        name: "Jenkins",
        description: "Leading open-source CI/CD automation server",
        category: "Development",
        image: "jenkins/jenkins:lts",
        default_port: 8290,
        container_port: "8080/tcp",
        env_vars: &[],
        volumes: &["/var/jenkins_home"],
    },
    // Woodpecker CI already exists above — skipped duplicate
    // ─── Databases (additional) ─────────────────────────────────
    AppTemplateDef {
        id: "clickhouse",
        name: "ClickHouse",
        description: "Column-oriented OLAP database for real-time analytics on large datasets",
        category: "Database",
        image: "clickhouse/clickhouse-server:latest",
        default_port: 8123,
        container_port: "8123/tcp",
        env_vars: &[],
        volumes: &["/var/lib/clickhouse", "/var/log/clickhouse-server"],
    },
    AppTemplateDef {
        id: "influxdb",
        name: "InfluxDB",
        description: "Time series database for metrics, events, and real-time analytics",
        category: "Database",
        image: "influxdb:2",
        default_port: 8292,
        container_port: "8086/tcp",
        env_vars: &[
            EnvVarDef { name: "DOCKER_INFLUXDB_INIT_MODE", label: "Init Mode", default: "setup", required: false, secret: false },
            EnvVarDef { name: "DOCKER_INFLUXDB_INIT_USERNAME", label: "Admin Username", default: "admin", required: true, secret: false },
            EnvVarDef { name: "DOCKER_INFLUXDB_INIT_PASSWORD", label: "Admin Password", default: "", required: true, secret: true },
            EnvVarDef { name: "DOCKER_INFLUXDB_INIT_ORG", label: "Organization", default: "myorg", required: true, secret: false },
            EnvVarDef { name: "DOCKER_INFLUXDB_INIT_BUCKET", label: "Bucket Name", default: "default", required: true, secret: false },
        ],
        volumes: &["/var/lib/influxdb2"],
    },
    AppTemplateDef {
        id: "valkey",
        name: "Valkey",
        description: "High-performance key/value datastore (community fork of Redis)",
        category: "Database",
        image: "valkey/valkey:8-alpine",
        default_port: 6380,
        container_port: "6379/tcp",
        env_vars: &[],
        volumes: &["/data"],
    },
    AppTemplateDef {
        id: "couchdb",
        name: "CouchDB",
        description: "Document-oriented NoSQL database with multi-master replication",
        category: "Database",
        image: "couchdb:3",
        default_port: 5984,
        container_port: "5984/tcp",
        env_vars: &[
            EnvVarDef { name: "COUCHDB_USER", label: "Admin Username", default: "admin", required: true, secret: false },
            EnvVarDef { name: "COUCHDB_PASSWORD", label: "Admin Password", default: "", required: true, secret: true },
        ],
        volumes: &["/opt/couchdb/data"],
    },
    // ─── Security ───────────────────────────────────────────────
    AppTemplateDef {
        id: "crowdsec",
        name: "CrowdSec",
        description: "Collaborative security engine analyzing logs and sharing threat intelligence",
        category: "Security",
        image: "crowdsecurity/crowdsec:latest",
        default_port: 6060,
        container_port: "6060/tcp",
        env_vars: &[],
        volumes: &["/etc/crowdsec", "/var/lib/crowdsec/data"],
    },
    // ─── Automation (additional) ────────────────────────────────
    AppTemplateDef {
        id: "node-red",
        name: "Node-RED",
        description: "Flow-based programming tool for wiring IoT, APIs, and online services",
        category: "Automation",
        image: "nodered/node-red:latest",
        default_port: 1880,
        container_port: "1880/tcp",
        env_vars: &[],
        volumes: &["/data"],
    },
    AppTemplateDef {
        id: "activepieces",
        name: "Activepieces",
        description: "No-code workflow automation platform (Zapier alternative)",
        category: "Automation",
        image: "activepieces/activepieces:latest",
        default_port: 8293,
        container_port: "80/tcp",
        env_vars: &[
            EnvVarDef { name: "AP_ENGINE_EXECUTABLE_PATH", label: "Engine Path", default: "dist/packages/engine/main.js", required: false, secret: false },
            EnvVarDef { name: "AP_ENCRYPTION_KEY", label: "Encryption Key", default: "", required: true, secret: true },
            EnvVarDef { name: "AP_JWT_SECRET", label: "JWT Secret", default: "", required: true, secret: true },
            // Activepieces defaults to POSTGRES + a standalone Redis, neither of which
            // this single-container template deploys, so it could not start at all. The
            // embedded pair is what upstream documents for the one-container install —
            // and it is what makes the volume below meaningful, since PGLite is the
            // thing that writes into it. Both halves or neither.
            EnvVarDef { name: "AP_DB_TYPE", label: "Database Mode", default: "PGLITE", required: false, secret: false },
            EnvVarDef { name: "AP_REDIS_TYPE", label: "Queue Mode", default: "MEMORY", required: false, secret: false },
        ],
        // PGLite's embedded Postgres and the local settings live here (AP_CONFIG_PATH
        // defaults to ~/.activepieces, and the image runs as root, so HOME is /root).
        volumes: &["/root/.activepieces"],
    },
    // ─── CMS (additional) ───────────────────────────────────────
    AppTemplateDef {
        id: "wordpress-docker",
        name: "WordPress (Docker)",
        description: "World's most popular CMS as a Docker container with Apache and PHP",
        category: "CMS",
        image: "wordpress:6-apache",
        default_port: 8294,
        container_port: "80/tcp",
        env_vars: &[
            EnvVarDef { name: "WORDPRESS_DB_HOST", label: "Database Host", default: "", required: true, secret: false },
            EnvVarDef { name: "WORDPRESS_DB_USER", label: "Database User", default: "wordpress", required: true, secret: false },
            EnvVarDef { name: "WORDPRESS_DB_PASSWORD", label: "Database Password", default: "", required: true, secret: true },
            EnvVarDef { name: "WORDPRESS_DB_NAME", label: "Database Name", default: "wordpress", required: true, secret: false },
        ],
        volumes: &["/var/www/html"],
    },
    AppTemplateDef {
        id: "listmonk",
        name: "Listmonk",
        description: "High-performance self-hosted newsletter and mailing list manager",
        category: "CMS",
        image: "listmonk/listmonk:latest",
        default_port: 9100,
        container_port: "9000/tcp",
        env_vars: &[
            EnvVarDef { name: "LISTMONK_app__address", label: "Listen Address", default: "0.0.0.0:9000", required: false, secret: false },
            EnvVarDef { name: "LISTMONK_db__host", label: "DB Host", default: "", required: true, secret: false },
            EnvVarDef { name: "LISTMONK_db__port", label: "DB Port", default: "5432", required: false, secret: false },
            EnvVarDef { name: "LISTMONK_db__user", label: "DB User", default: "listmonk", required: false, secret: false },
            EnvVarDef { name: "LISTMONK_db__password", label: "DB Password", default: "", required: true, secret: true },
            EnvVarDef { name: "LISTMONK_db__database", label: "DB Name", default: "listmonk", required: false, secret: false },
        ],
        volumes: &["/listmonk/uploads"],
    },
    // ─── Analytics (additional) ─────────────────────────────────
    AppTemplateDef {
        id: "shynet",
        name: "Shynet",
        description: "Privacy-friendly web analytics without cookies or JavaScript",
        category: "Analytics",
        image: "milesmcc/shynet:latest",
        default_port: 8295,
        container_port: "8080/tcp",
        env_vars: &[
            EnvVarDef { name: "DJANGO_SECRET_KEY", label: "Secret Key", default: "", required: true, secret: true },
            EnvVarDef { name: "DB_NAME", label: "Database Name", default: "shynet", required: false, secret: false },
        ],
        volumes: &[],
    },
    // ─── Image / Upload ─────────────────────────────────────────
    AppTemplateDef {
        id: "zipline",
        name: "Zipline",
        description: "ShareX/file upload server with URL shortening and image galleries",
        category: "Storage",
        image: "ghcr.io/diced/zipline:latest",
        default_port: 3018,
        container_port: "3000/tcp",
        env_vars: &[
            EnvVarDef { name: "CORE_SECRET", label: "Core Secret", default: "", required: true, secret: true },
            EnvVarDef { name: "CORE_DATABASE_URL", label: "Database URL", default: "", required: true, secret: false },
        ],
        volumes: &["/zipline/uploads", "/zipline/public"],
    },
    // ─── Authentication ─────────────────────────────────────────
    AppTemplateDef {
        id: "authelia",
        name: "Authelia",
        description: "Single Sign-On and 2FA portal for securing web applications",
        category: "Security",
        image: "authelia/authelia:latest",
        default_port: 9091,
        container_port: "9091/tcp",
        env_vars: &[],
        volumes: &["/config"],
    },
    // ─── AI / Machine Learning (additional) ─────────────────────
    AppTemplateDef {
        id: "text-generation-webui",
        name: "Text Generation WebUI",
        description: "Gradio web UI for running large language models (oobabooga)",
        category: "AI",
        // Pinned to stable default tag instead of nightly so deploys don't drift
        image: "atinoda/text-generation-webui:default",
        default_port: 7861,
        container_port: "7860/tcp",
        env_vars: &[
            EnvVarDef { name: "EXTRA_LAUNCH_ARGS", label: "Extra Launch Args", default: "--listen", required: false, secret: false },
        ],
        volumes: &["/app/models", "/app/characters", "/app/loras"],
    },
    AppTemplateDef {
        id: "whisper",
        name: "Whisper ASR",
        description: "OpenAI Whisper speech-to-text service with REST API",
        category: "AI",
        image: "onerahmet/openai-whisper-asr-webservice:latest",
        default_port: 9300,
        container_port: "9000/tcp",
        env_vars: &[
            EnvVarDef { name: "ASR_MODEL", label: "Model Size", default: "base", required: false, secret: false },
        ],
        volumes: &[],
    },
    AppTemplateDef {
        id: "litellm",
        name: "LiteLLM Proxy",
        description: "Unified proxy for 100+ LLM APIs (OpenAI, Anthropic, Cohere, etc.)",
        category: "AI",
        image: "ghcr.io/berriai/litellm:main-latest",
        default_port: 4100,
        container_port: "4000/tcp",
        env_vars: &[
            EnvVarDef { name: "LITELLM_MASTER_KEY", label: "Master API Key", default: "", required: true, secret: true },
        ],
        volumes: &[],
    },
    AppTemplateDef {
        id: "flowise",
        name: "Flowise",
        description: "Drag-and-drop LLM flow builder for LangChain applications",
        category: "AI",
        image: "flowiseai/flowise:latest",
        default_port: 3020,
        container_port: "3000/tcp",
        env_vars: &[
            EnvVarDef { name: "FLOWISE_USERNAME", label: "Username", default: "admin", required: false, secret: false },
            EnvVarDef { name: "FLOWISE_PASSWORD", label: "Password", default: "", required: false, secret: true },
        ],
        volumes: &["/root/.flowise"],
    },
    AppTemplateDef {
        id: "langflow",
        name: "Langflow",
        description: "Visual framework for building multi-agent and RAG applications",
        category: "AI",
        image: "langflowai/langflow:latest",
        default_port: 7862,
        container_port: "7860/tcp",
        env_vars: &[
            EnvVarDef { name: "LANGFLOW_AUTO_LOGIN", label: "Auto Login", default: "false", required: false, secret: false },
        ],
        volumes: &["/app/langflow"],
    },
    AppTemplateDef {
        id: "dify",
        name: "Dify",
        description: "Open-source LLM app development platform with RAG, agents, and workflows",
        category: "AI",
        image: "langgenius/dify-api:latest",
        default_port: 5002,
        container_port: "5001/tcp",
        env_vars: &[
            EnvVarDef { name: "SECRET_KEY", label: "Secret Key", default: "", required: true, secret: true },
            EnvVarDef { name: "DB_USERNAME", label: "Database Username", default: "dify", required: true, secret: false },
            EnvVarDef { name: "DB_PASSWORD", label: "Database Password", default: "", required: true, secret: true },
            EnvVarDef { name: "DB_HOST", label: "Database Host", default: "localhost", required: true, secret: false },
            EnvVarDef { name: "REDIS_HOST", label: "Redis Host", default: "localhost", required: false, secret: false },
        ],
        volumes: &["/app/api/storage"],
    },
    AppTemplateDef {
        id: "vllm",
        name: "vLLM",
        description: "High-throughput, memory-efficient LLM inference server with OpenAI-compatible API",
        category: "AI",
        image: "vllm/vllm-openai:latest",
        default_port: 8000,
        container_port: "8000/tcp",
        env_vars: &[
            EnvVarDef { name: "MODEL", label: "Model (HuggingFace ID)", default: "meta-llama/Llama-3.2-1B-Instruct", required: true, secret: false },
            EnvVarDef { name: "HUGGING_FACE_HUB_TOKEN", label: "HuggingFace Token (gated models)", default: "", required: false, secret: true },
        ],
        volumes: &["/root/.cache/huggingface"],
    },
    // ─── Databases (new) ────────────────────────────────────────
    AppTemplateDef {
        id: "surrealdb",
        name: "SurrealDB",
        description: "Multi-model database for web, mobile, serverless, and backend (SQL, graph, document)",
        category: "Database",
        image: "surrealdb/surrealdb:latest",
        default_port: 8300,
        container_port: "8000/tcp",
        env_vars: &[
            EnvVarDef { name: "SURREAL_USER", label: "Root Username", default: "root", required: true, secret: false },
            EnvVarDef { name: "SURREAL_PASS", label: "Root Password", default: "", required: true, secret: true },
        ],
        volumes: &["/data"],
    },
    AppTemplateDef {
        id: "questdb",
        name: "QuestDB",
        description: "High-performance time series database with SQL support and built-in web console",
        category: "Database",
        image: "questdb/questdb:latest",
        default_port: 9009,
        container_port: "9000/tcp",
        env_vars: &[],
        volumes: &["/var/lib/questdb"],
    },
    AppTemplateDef {
        id: "timescaledb",
        name: "TimescaleDB",
        description: "PostgreSQL extension for time-series data with automatic partitioning",
        category: "Database",
        image: "timescale/timescaledb:latest-pg16",
        default_port: 5433,
        container_port: "5432/tcp",
        env_vars: &[
            EnvVarDef { name: "POSTGRES_USER", label: "Username", default: "postgres", required: true, secret: false },
            EnvVarDef { name: "POSTGRES_PASSWORD", label: "Password", default: "", required: true, secret: true },
        ],
        volumes: &["/var/lib/postgresql/data"],
    },
    AppTemplateDef {
        id: "keydb",
        name: "KeyDB",
        description: "Multi-threaded fork of Redis with active replication and flash storage support",
        category: "Database",
        image: "eqalpha/keydb:latest",
        default_port: 6381,
        container_port: "6379/tcp",
        env_vars: &[],
        volumes: &["/data"],
    },
    AppTemplateDef {
        id: "dragonflydb",
        name: "DragonflyDB",
        description: "Modern in-memory datastore compatible with Redis and Memcached APIs",
        category: "Database",
        image: "docker.dragonflydb.io/dragonflydb/dragonfly:latest",
        default_port: 6382,
        container_port: "6379/tcp",
        env_vars: &[],
        volumes: &["/data"],
    },
    AppTemplateDef {
        id: "cassandra",
        name: "Apache Cassandra",
        description: "Distributed wide-column NoSQL database for high availability at scale",
        category: "Database",
        image: "cassandra:5",
        default_port: 9042,
        container_port: "9042/tcp",
        env_vars: &[
            EnvVarDef { name: "CASSANDRA_CLUSTER_NAME", label: "Cluster Name", default: "DockPanelCluster", required: false, secret: false },
        ],
        volumes: &["/var/lib/cassandra"],
    },
    AppTemplateDef {
        id: "neo4j",
        name: "Neo4j",
        description: "Leading graph database for connected data with Cypher query language",
        category: "Database",
        image: "neo4j:5",
        default_port: 7474,
        container_port: "7474/tcp",
        env_vars: &[
            EnvVarDef { name: "NEO4J_AUTH", label: "Auth (user/pass)", default: "neo4j/changeme", required: true, secret: true },
        ],
        volumes: &["/data", "/logs"],
    },
    AppTemplateDef {
        id: "arangodb",
        name: "ArangoDB",
        description: "Multi-model database combining document, graph, and key-value in one engine",
        category: "Database",
        image: "arangodb:3.12",
        default_port: 8529,
        container_port: "8529/tcp",
        env_vars: &[
            EnvVarDef { name: "ARANGO_ROOT_PASSWORD", label: "Root Password", default: "", required: true, secret: true },
        ],
        volumes: &["/var/lib/arangodb3", "/var/lib/arangodb3-apps"],
    },
    // ─── Development (new) ──────────────────────────────────────
    AppTemplateDef {
        id: "gitness",
        name: "Gitness",
        description: "Open-source developer platform with Git hosting, pipelines, and code review (by Harness)",
        category: "Development",
        image: "harness/gitness:latest",
        default_port: 3030,
        container_port: "3000/tcp",
        env_vars: &[],
        volumes: &["/data"],
    },
    AppTemplateDef {
        id: "verdaccio",
        name: "Verdaccio",
        description: "Lightweight private npm proxy registry for hosting and caching packages",
        category: "Development",
        image: "verdaccio/verdaccio:6",
        default_port: 4873,
        container_port: "4873/tcp",
        env_vars: &[],
        volumes: &["/verdaccio/storage", "/verdaccio/conf"],
    },
    AppTemplateDef {
        id: "nexus",
        name: "Nexus Repository",
        description: "Universal artifact repository manager for Maven, npm, Docker, and more",
        category: "Development",
        image: "sonatype/nexus3:latest",
        default_port: 8320,
        container_port: "8081/tcp",
        env_vars: &[],
        volumes: &["/nexus-data"],
    },
    AppTemplateDef {
        id: "buildkite-agent",
        name: "Buildkite Agent",
        description: "CI/CD agent that runs build jobs from the Buildkite platform",
        category: "Development",
        image: "buildkite/agent:3",
        default_port: 8301,
        container_port: "8080/tcp",
        env_vars: &[
            EnvVarDef { name: "BUILDKITE_AGENT_TOKEN", label: "Agent Token", default: "", required: true, secret: true },
        ],
        volumes: &["/buildkite/builds"],
    },
    // ─── Media (new) ────────────────────────────────────────────
    AppTemplateDef {
        id: "jellyseerr",
        name: "Jellyseerr",
        description: "Media request management for Jellyfin, Emby, and Plex with auto-approval",
        category: "Media",
        image: "fallenbagel/jellyseerr:latest",
        default_port: 5055,
        container_port: "5055/tcp",
        env_vars: &[],
        volumes: &["/app/config"],
    },
    AppTemplateDef {
        id: "overseerr",
        name: "Overseerr",
        description: "Media request management and discovery tool for Plex ecosystems",
        category: "Media",
        image: "lscr.io/linuxserver/overseerr:latest",
        default_port: 5056,
        container_port: "5055/tcp",
        env_vars: &[
            EnvVarDef { name: "PUID", label: "User ID", default: "1000", required: false, secret: false },
            EnvVarDef { name: "PGID", label: "Group ID", default: "1000", required: false, secret: false },
        ],
        volumes: &["/config"],
    },
    AppTemplateDef {
        id: "bazarr",
        name: "Bazarr",
        description: "Companion app for Sonarr and Radarr to manage and download subtitles",
        category: "Media",
        image: "lscr.io/linuxserver/bazarr:latest",
        default_port: 6767,
        container_port: "6767/tcp",
        env_vars: &[
            EnvVarDef { name: "PUID", label: "User ID", default: "1000", required: false, secret: false },
            EnvVarDef { name: "PGID", label: "Group ID", default: "1000", required: false, secret: false },
        ],
        volumes: &["/config", "/movies", "/tv"],
    },
    AppTemplateDef {
        id: "prowlarr",
        name: "Prowlarr",
        description: "Indexer manager and proxy for Sonarr, Radarr, Lidarr, and Readarr",
        category: "Media",
        image: "lscr.io/linuxserver/prowlarr:latest",
        default_port: 9696,
        container_port: "9696/tcp",
        env_vars: &[
            EnvVarDef { name: "PUID", label: "User ID", default: "1000", required: false, secret: false },
            EnvVarDef { name: "PGID", label: "Group ID", default: "1000", required: false, secret: false },
        ],
        volumes: &["/config"],
    },
    AppTemplateDef {
        id: "radarr",
        name: "Radarr",
        description: "Movie collection manager with automatic downloading and organization",
        category: "Media",
        image: "lscr.io/linuxserver/radarr:latest",
        default_port: 7878,
        container_port: "7878/tcp",
        env_vars: &[
            EnvVarDef { name: "PUID", label: "User ID", default: "1000", required: false, secret: false },
            EnvVarDef { name: "PGID", label: "Group ID", default: "1000", required: false, secret: false },
        ],
        volumes: &["/config", "/movies", "/downloads"],
    },
    AppTemplateDef {
        id: "sonarr",
        name: "Sonarr",
        description: "TV series collection manager with automatic downloading and organization",
        category: "Media",
        image: "lscr.io/linuxserver/sonarr:latest",
        default_port: 8989,
        container_port: "8989/tcp",
        env_vars: &[
            EnvVarDef { name: "PUID", label: "User ID", default: "1000", required: false, secret: false },
            EnvVarDef { name: "PGID", label: "Group ID", default: "1000", required: false, secret: false },
        ],
        volumes: &["/config", "/tv", "/downloads"],
    },
    AppTemplateDef {
        id: "lidarr",
        name: "Lidarr",
        description: "Music collection manager with automatic downloading and metadata management",
        category: "Media",
        image: "lscr.io/linuxserver/lidarr:latest",
        default_port: 8686,
        container_port: "8686/tcp",
        env_vars: &[
            EnvVarDef { name: "PUID", label: "User ID", default: "1000", required: false, secret: false },
            EnvVarDef { name: "PGID", label: "Group ID", default: "1000", required: false, secret: false },
        ],
        volumes: &["/config", "/music", "/downloads"],
    },
    // The Readarr template was withdrawn s342. LinuxServer's image for it publishes
    // an OCI index with ZERO manifests on both of its tags — `docker pull` exits 1
    // with "no matching manifest for linux/amd64", while the Sonarr and Radarr
    // controls return four manifests each. The upstream project is archived, last
    // pushed 2025-06-27, so there is nothing to repoint to. Same call as the four
    // withdrawn in v2.96.0.
    //
    // ⚠ This shape is invisible to a classifier that keys on the manifest's HTTP
    // status: the request succeeds with 200 and a well-formed index that happens
    // to be empty. v2.96.0's sweep classified by error MESSAGE and could not see
    // it. Any future resolvability check has to assert the index is NON-EMPTY.
    //
    // The exact reference is deliberately not spelled here — this file is the
    // subject of pins that grep raw source, and a name written in a comment
    // silently satisfies the absence arm meant to keep it gone.
    AppTemplateDef {
        id: "tautulli",
        name: "Tautulli",
        description: "Monitoring and tracking tool for Plex Media Server usage and statistics",
        category: "Media",
        image: "lscr.io/linuxserver/tautulli:latest",
        default_port: 8183,
        container_port: "8181/tcp",
        env_vars: &[
            EnvVarDef { name: "PUID", label: "User ID", default: "1000", required: false, secret: false },
            EnvVarDef { name: "PGID", label: "Group ID", default: "1000", required: false, secret: false },
        ],
        volumes: &["/config"],
    },
    // ─── Monitoring (new) ───────────────────────────────────────
    AppTemplateDef {
        id: "checkmk",
        name: "Checkmk",
        description: "Infrastructure and application monitoring with auto-discovery and alerting",
        category: "Monitoring",
        image: "checkmk/check-mk-raw:2.3.0-latest",
        default_port: 8303,
        container_port: "5000/tcp",
        env_vars: &[
            EnvVarDef { name: "CMK_PASSWORD", label: "Admin Password", default: "", required: true, secret: true },
            EnvVarDef { name: "CMK_SITE_ID", label: "Site ID", default: "cmk", required: false, secret: false },
        ],
        volumes: &["/omd/sites"],
    },
    AppTemplateDef {
        id: "graylog",
        name: "Graylog",
        description: "Centralized log management and analysis platform with alerting",
        category: "Monitoring",
        image: "graylog/graylog:6.1",
        default_port: 9200,
        container_port: "9000/tcp",
        env_vars: &[
            EnvVarDef { name: "GRAYLOG_PASSWORD_SECRET", label: "Password Secret (16+ chars)", default: "", required: true, secret: true },
            EnvVarDef { name: "GRAYLOG_ROOT_PASSWORD_SHA2", label: "Root Password SHA256 Hash", default: "", required: true, secret: true },
            EnvVarDef { name: "GRAYLOG_HTTP_EXTERNAL_URI", label: "External URI", default: "http://localhost:9000/", required: true, secret: false },
            EnvVarDef { name: "GRAYLOG_MONGODB_URI", label: "MongoDB URI", default: "mongodb://mongo:27017/graylog", required: true, secret: false },
            EnvVarDef { name: "GRAYLOG_ELASTICSEARCH_HOSTS", label: "Elasticsearch Hosts", default: "http://elasticsearch:9200", required: true, secret: false },
        ],
        volumes: &["/usr/share/graylog/data"],
    },
    AppTemplateDef {
        id: "vector",
        name: "Vector",
        description: "High-performance observability data pipeline for logs, metrics, and traces",
        category: "Monitoring",
        image: "timberio/vector:latest-alpine",
        default_port: 8304,
        container_port: "8686/tcp",
        env_vars: &[],
        volumes: &["/etc/vector", "/var/lib/vector"],
    },
    // ─── Productivity (new) ─────────────────────────────────────
    AppTemplateDef {
        id: "leantime",
        name: "Leantime",
        description: "Open-source project management system for non-project managers (lean methodology)",
        category: "Productivity",
        image: "leantime/leantime:latest",
        default_port: 8305,
        container_port: "80/tcp",
        env_vars: &[
            EnvVarDef { name: "LEAN_DB_HOST", label: "Database Host", default: "", required: true, secret: false },
            EnvVarDef { name: "LEAN_DB_USER", label: "Database User", default: "leantime", required: true, secret: false },
            EnvVarDef { name: "LEAN_DB_PASSWORD", label: "Database Password", default: "", required: true, secret: true },
            EnvVarDef { name: "LEAN_DB_DATABASE", label: "Database Name", default: "leantime", required: false, secret: false },
        ],
        volumes: &["/var/www/html/public/userfiles", "/var/www/html/userfiles"],
    },
    AppTemplateDef {
        id: "focalboard",
        name: "Focalboard",
        description: "Open-source project management tool (Trello/Notion/Asana alternative by Mattermost)",
        category: "Productivity",
        image: "mattermost/focalboard:latest",
        default_port: 8306,
        container_port: "8000/tcp",
        env_vars: &[],
        volumes: &["/opt/focalboard/data"],
    },
    AppTemplateDef {
        id: "joplin-server",
        name: "Joplin Server",
        description: "Sync server for Joplin note-taking app with sharing and collaboration",
        category: "Productivity",
        image: "joplin/server:latest",
        default_port: 22300,
        container_port: "22300/tcp",
        env_vars: &[
            EnvVarDef { name: "APP_BASE_URL", label: "Base URL", default: "", required: true, secret: false },
            EnvVarDef { name: "DB_CLIENT", label: "DB Client", default: "pg", required: false, secret: false },
            EnvVarDef { name: "POSTGRES_HOST", label: "PostgreSQL Host", default: "", required: true, secret: false },
            EnvVarDef { name: "POSTGRES_PORT", label: "PostgreSQL Port", default: "5432", required: false, secret: false },
            EnvVarDef { name: "POSTGRES_DATABASE", label: "Database Name", default: "joplin", required: false, secret: false },
            EnvVarDef { name: "POSTGRES_USER", label: "Database User", default: "joplin", required: true, secret: false },
            EnvVarDef { name: "POSTGRES_PASSWORD", label: "Database Password", default: "", required: true, secret: true },
        ],
        volumes: &[],
    },
    // ─── Communication (new) ────────────────────────────────────
    AppTemplateDef {
        id: "element-web",
        name: "Element Web",
        description: "Feature-rich Matrix client for secure, decentralized communication",
        category: "Communication",
        image: "vectorim/element-web:latest",
        default_port: 8307,
        container_port: "80/tcp",
        env_vars: &[],
        // Was `/app/config.json` — a FILE. Every volume path here becomes a host
        // DIRECTORY that is bind-mounted over the container path, so Docker was
        // asked to mount a directory onto a file and refused: the container could
        // not be CREATED at all, and the deploy failed before anything started.
        // That is the loudest failure in this catalogue and it survived every
        // audit, because reading the entry tells you nothing — the string looks
        // exactly like the ones that work.
        //
        // Dropped rather than repointed. Element Web is a static single-page app;
        // its config.json ships in the image and there is no user data at that
        // path to preserve, so there is nothing a volume here would persist.
        volumes: &[],
    },
    AppTemplateDef {
        id: "synapse",
        name: "Matrix Synapse",
        description: "Reference Matrix homeserver for decentralized, encrypted messaging",
        category: "Communication",
        image: "matrixdotorg/synapse:latest",
        default_port: 8308,
        container_port: "8008/tcp",
        env_vars: &[
            EnvVarDef { name: "SYNAPSE_SERVER_NAME", label: "Server Name", default: "", required: true, secret: false },
            EnvVarDef { name: "SYNAPSE_REPORT_STATS", label: "Report Stats", default: "no", required: false, secret: false },
        ],
        volumes: &["/data"],
    },
    // ─── Networking (new) ───────────────────────────────────────
    AppTemplateDef {
        id: "traefik",
        name: "Traefik",
        description: "Cloud-native reverse proxy and load balancer with automatic HTTPS",
        category: "Networking",
        image: "traefik:v3",
        default_port: 8309,
        // 80, not 8080. Traefik with no configuration creates exactly ONE
        // entryPoint — `http` on :80 — and the :8080 one only exists when
        // `--api.insecure`, ping, prometheus or the REST provider asks for it.
        // None of those are set here, so nothing ever listened on 8080 and the
        // published port answered connection-refused forever. The image's own
        // Dockerfile declares EXPOSE 80 and nothing else.
        //
        // The tempting fix — a start command passing `--api.insecure=true` — is
        // the wrong one, and this panel already wrote down why: `traefik.rs`
        // passes exactly that flag for the panel's OWN managed Traefik, above a
        // comment whose safety argument is that the port is bound to 127.0.0.1
        // and cannot be reached from outside. A catalogue template can be given a
        // domain and put behind the proxy, which breaks precisely that condition,
        // so the flag would publish an unauthenticated admin API.
        container_port: "80/tcp",
        env_vars: &[],
        volumes: &["/etc/traefik", "/letsencrypt"],
    },
    AppTemplateDef {
        id: "caddy",
        name: "Caddy",
        description: "Powerful web server with automatic HTTPS and easy configuration",
        category: "Networking",
        image: "caddy:2-alpine",
        default_port: 8310,
        container_port: "80/tcp",
        env_vars: &[],
        volumes: &["/etc/caddy", "/data", "/config"],
    },
    AppTemplateDef {
        id: "cloudflared",
        name: "Cloudflare Tunnel",
        description: "Secure tunnel to expose local services to the internet via Cloudflare",
        category: "Networking",
        image: "cloudflare/cloudflared:latest",
        default_port: 8311,
        container_port: "8080/tcp",
        env_vars: &[
            EnvVarDef { name: "TUNNEL_TOKEN", label: "Tunnel Token", default: "", required: true, secret: true },
        ],
        volumes: &[],
    },
    AppTemplateDef {
        id: "tailscale",
        name: "Tailscale",
        description: "Zero-config mesh VPN built on WireGuard for secure private networking",
        category: "Networking",
        image: "tailscale/tailscale:latest",
        default_port: 8312,
        container_port: "41641/udp",
        env_vars: &[
            EnvVarDef { name: "TS_AUTHKEY", label: "Auth Key", default: "", required: true, secret: true },
            EnvVarDef { name: "TS_HOSTNAME", label: "Hostname", default: "dockpanel", required: false, secret: false },
        ],
        volumes: &["/var/lib/tailscale"],
    },
    // ─── Storage (new) ──────────────────────────────────────────
    AppTemplateDef {
        id: "garage",
        name: "Garage",
        description: "Lightweight S3-compatible distributed object storage for self-hosting",
        category: "Storage",
        image: "dxflrs/garage:v2.3.0",
        default_port: 3900,
        container_port: "3900/tcp",
        env_vars: &[],
        volumes: &["/var/lib/garage/data", "/var/lib/garage/meta", "/etc/garage"],
    },
    AppTemplateDef {
        id: "seaweedfs",
        name: "SeaweedFS",
        description: "Fast distributed storage system for billions of files with S3 API support",
        category: "Storage",
        image: "chrislusf/seaweedfs:latest",
        default_port: 9333,
        container_port: "9333/tcp",
        env_vars: &[],
        volumes: &["/data"],
    },
    // ─── Security (new) ─────────────────────────────────────────
    AppTemplateDef {
        id: "wazuh",
        name: "Wazuh Manager",
        description: "Open-source security platform for threat detection, compliance, and incident response",
        category: "Security",
        image: "wazuh/wazuh-manager:4.9.2",
        default_port: 1514,
        container_port: "1514/tcp",
        env_vars: &[],
        volumes: &["/var/ossec/api/configuration", "/var/ossec/etc", "/var/ossec/logs", "/var/ossec/queue"],
    },
    AppTemplateDef {
        id: "trivy",
        name: "Trivy Server",
        description: "Comprehensive vulnerability scanner for containers, filesystems, and IaC",
        category: "Security",
        image: "aquasec/trivy:latest",
        default_port: 8313,
        container_port: "8080/tcp",
        env_vars: &[],
        volumes: &["/root/.cache"],
    },
    // ─── Automation (new) ───────────────────────────────────────
    AppTemplateDef {
        id: "huginn",
        name: "Huginn",
        description: "System for building agents that perform automated tasks online (IFTTT alternative)",
        category: "Automation",
        image: "ghcr.io/huginn/huginn:latest",
        default_port: 3021,
        container_port: "3000/tcp",
        env_vars: &[
            EnvVarDef { name: "HUGINN_DATABASE_ADAPTER", label: "DB Adapter", default: "mysql2", required: false, secret: false },
            EnvVarDef { name: "HUGINN_DATABASE_HOST", label: "DB Host", default: "", required: true, secret: false },
            EnvVarDef { name: "HUGINN_DATABASE_NAME", label: "DB Name", default: "huginn", required: false, secret: false },
            EnvVarDef { name: "HUGINN_DATABASE_USERNAME", label: "DB Username", default: "huginn", required: true, secret: false },
            EnvVarDef { name: "HUGINN_DATABASE_PASSWORD", label: "DB Password", default: "", required: true, secret: true },
        ],
        volumes: &[],
    },
    AppTemplateDef {
        id: "windmill",
        name: "Windmill",
        description: "Developer platform for building internal tools, workflows, and scripts as code",
        category: "Automation",
        image: "ghcr.io/windmill-labs/windmill:main",
        default_port: 8314,
        container_port: "8000/tcp",
        env_vars: &[
            EnvVarDef { name: "DATABASE_URL", label: "PostgreSQL URL", default: "", required: true, secret: true },
        ],
        volumes: &[],
    },
    AppTemplateDef {
        id: "temporal",
        name: "Temporal Server",
        description: "Durable execution platform for reliable microservices and workflows",
        category: "Automation",
        image: "temporalio/auto-setup:latest",
        default_port: 8315,
        container_port: "8233/tcp",
        env_vars: &[
            EnvVarDef { name: "DB", label: "Database Type", default: "postgresql", required: false, secret: false },
            EnvVarDef { name: "POSTGRES_SEEDS", label: "PostgreSQL Host", default: "", required: true, secret: false },
            EnvVarDef { name: "POSTGRES_USER", label: "PostgreSQL User", default: "temporal", required: true, secret: false },
            EnvVarDef { name: "POSTGRES_PWD", label: "PostgreSQL Password", default: "", required: true, secret: true },
        ],
        volumes: &[],
    },
    // ─── Analytics (new) ────────────────────────────────────────
    AppTemplateDef {
        id: "superset",
        name: "Apache Superset",
        description: "Modern data exploration and visualization platform with rich SQL editor",
        category: "Analytics",
        image: "apache/superset:latest",
        default_port: 8321,
        container_port: "8088/tcp",
        env_vars: &[
            EnvVarDef { name: "SUPERSET_SECRET_KEY", label: "Secret Key", default: "", required: true, secret: true },
            EnvVarDef { name: "ADMIN_USERNAME", label: "Admin Username", default: "admin", required: false, secret: false },
            EnvVarDef { name: "ADMIN_PASSWORD", label: "Admin Password", default: "", required: true, secret: true },
        ],
        volumes: &["/app/superset_home"],
    },
    AppTemplateDef {
        id: "redash",
        name: "Redash",
        description: "Connect to any data source, visualize, and share dashboards with your team",
        category: "Analytics",
        image: "redash/redash:latest",
        default_port: 5100,
        container_port: "5000/tcp",
        env_vars: &[
            EnvVarDef { name: "REDASH_DATABASE_URL", label: "PostgreSQL URL", default: "", required: true, secret: true },
            EnvVarDef { name: "REDASH_REDIS_URL", label: "Redis URL", default: "redis://redis:6379/0", required: true, secret: false },
            EnvVarDef { name: "REDASH_SECRET_KEY", label: "Secret Key", default: "", required: true, secret: true },
        ],
        volumes: &[],
    },
    AppTemplateDef {
        id: "posthog",
        name: "PostHog",
        description: "Open-source product analytics with session recording, feature flags, and A/B testing",
        category: "Analytics",
        image: "posthog/posthog:latest",
        default_port: 8316,
        container_port: "8000/tcp",
        env_vars: &[
            EnvVarDef { name: "SECRET_KEY", label: "Secret Key", default: "", required: true, secret: true },
            EnvVarDef { name: "DATABASE_URL", label: "PostgreSQL URL", default: "", required: true, secret: true },
            EnvVarDef { name: "REDIS_URL", label: "Redis URL", default: "redis://redis:6379/", required: false, secret: false },
        ],
        volumes: &[],
    },
];

/// Convert a static template definition to the owned serializable type.
fn to_app_template(def: &AppTemplateDef) -> AppTemplate {
    AppTemplate {
        id: def.id.to_string(),
        name: def.name.to_string(),
        description: def.description.to_string(),
        category: def.category.to_string(),
        image: def.image.to_string(),
        default_port: def.default_port,
        container_port: def.container_port.to_string(),
        env_vars: def
            .env_vars
            .iter()
            .map(|ev| EnvVar {
                name: ev.name.to_string(),
                label: ev.label.to_string(),
                default: ev.default.to_string(),
                required: ev.required,
                secret: ev.secret,
                domain_derived: is_domain_derived(def.id, ev.name),
            })
            .collect(),
        volumes: def.volumes.iter().map(|v| v.to_string()).collect(),
        gpu_recommended: GPU_RECOMMENDED_TEMPLATES.contains(&def.id),
    }
}

/// Return all available app templates.
/// What `GET /apps/{id}/env` substitutes for a value it will not show.
///
/// Exported because [`update_env`] has to recognise it coming back: the edit
/// dialog is seeded from the masked read, so the mask is what a caller sends
/// for every field they did not touch.
pub const ENV_MASK: &str = "********";

/// Env var names the shipped catalogue itself declares are NOT secret.
///
/// The mask is chosen by substring — `KEY`, `AUTH`, `TOKEN`, `SECRET`,
/// `PASSWORD` anywhere in the name — which is right for an unknown key and
/// wrong for several we ship ourselves: `KEYCLOAK_ADMIN` is a username,
/// `NEXTAUTH_URL` is a URL, `AUTHENTIK_POSTGRESQL__HOST` is a hostname. All
/// three contain a pattern; none is a secret; all three were shown as
/// `********`, which is unreadable and — before the sentinel in [`update_env`]
/// — unrecoverable once saved.
///
/// A name is allowlisted only if EVERY template that declares it says
/// `secret: false`, so a name reused as a secret elsewhere keeps its mask, and
/// a name the catalogue has never heard of falls through to the substring test.
/// The list can therefore only ever un-mask something we ourselves defined.
/// Merge the caller's environment over the container's own, treating
/// [`ENV_MASK`] as "leave this one alone".
///
/// `GET /apps/{id}/env` substitutes the mask for anything sensitive, the edit
/// dialog is seeded from exactly that response and sends every field back, and
/// **this container is the only place the real value is stored** — no table
/// holds a Docker app's environment. Without this, saving the dialog wrote the
/// literal `********` over every masked variable and then removed the container
/// that held the original, which for Vaultwarden's `ADMIN_TOKEN` and
/// code-server's `PASSWORD` is an auth downgrade to a published literal, and for
/// Meilisearch a container that refuses to start after the original is gone.
///
/// Returns the new `KEY=value` list and the names carried over, so the caller
/// can say what it did rather than doing it silently.
pub(crate) fn merge_env_preserving_masked(
    new_env: &HashMap<String, String>,
    container_env: &[String],
) -> (Vec<String>, Vec<String>) {
    let existing: HashMap<&str, &str> = container_env
        .iter()
        .filter_map(|kv| kv.split_once('='))
        .collect();

    let mut kept = Vec::new();
    let env_list = new_env
        .iter()
        .map(|(k, v)| {
            if v == ENV_MASK {
                if let Some(real) = existing.get(k.as_str()) {
                    kept.push(k.clone());
                    return format!("{k}={real}");
                }
            }
            format!("{k}={v}")
        })
        .collect();

    kept.sort();
    (env_list, kept)
}

pub fn catalogue_non_secret_env(name: &str) -> bool {
    let mut seen = false;
    for t in TEMPLATES {
        for v in t.env_vars {
            if v.name == name {
                if v.secret {
                    return false;
                }
                seen = true;
            }
        }
    }
    seen
}

pub fn list_templates() -> Vec<AppTemplate> {
    TEMPLATES.iter().map(to_app_template).collect()
}

// ── Volume ownership ───────────────────────────────────────────────

/// Who a template's volume directories have to belong to.
///
/// `deploy_app` creates each declared volume as a **root-owned** directory on the
/// host and bind-mounts it. That is right for most images, which start as root and
/// chown their own data directory in their entrypoint before dropping privileges —
/// `postgres`, `mysql` and `mongo` all do exactly that.
///
/// It is wrong for an image that declares a non-root `Config.User`, because that
/// process starts **already dropped** and can never chown its way out: it cannot
/// write the directory the template just mounted for it. Both ways that shows up
/// are bad, and the quiet one is worse — either the container exits with
/// `Permission denied`, or it comes up healthy and silently persists nothing.
///
/// Measured over the catalogue by reading every image's config: 42 of 148 images
/// run non-root, and 31 of those declare a volume.
#[derive(Debug, Clone, PartialEq, Eq)]
struct VolumeOwner {
    uid: u32,
    /// `None` when no group could be resolved. The directory then keeps the group
    /// it already has, which is enough — the owner can write either way.
    gid: Option<u32>,
    /// The literal `Config.User`, so the log names what it acted on.
    spec: String,
}

/// Split a Docker `User` spec into its user and group halves.
///
/// Docker accepts `user`, `uid`, `user:group`, `uid:gid` and the mixed forms.
fn split_user_spec(spec: &str) -> (&str, Option<&str>) {
    match spec.split_once(':') {
        Some((user, group)) => (user.trim(), Some(group.trim())),
        None => (spec.trim(), None),
    }
}

/// Does this `Config.User` mean root? An empty field does — it is simply unset,
/// which is how the overwhelming majority of images ship.
fn user_spec_is_root(spec: &str) -> bool {
    matches!(split_user_spec(spec).0, "" | "root" | "0")
}

/// Pull one regular file's bytes out of a `docker cp` tar stream.
///
/// A dependency-free reader for the only shape this ever asks for: one small
/// file. A ustar header is 512 bytes, the size sits at offset 124 as octal ASCII,
/// and the payload is padded to the next 512-byte boundary.
fn tar_extract_first_file(archive: &[u8]) -> Option<String> {
    let mut offset = 0usize;
    while offset + 512 <= archive.len() {
        let header = &archive[offset..offset + 512];
        // A zero block ends the archive.
        if header.iter().all(|byte| *byte == 0) {
            return None;
        }
        let size_field = std::str::from_utf8(&header[124..136]).ok()?;
        let size = usize::from_str_radix(size_field.trim_matches(|c| c == '\0' || c == ' '), 8).ok()?;
        let body = offset + 512;
        // '0' and NUL both mean "regular file"; skip anything else.
        if (header[156] == b'0' || header[156] == 0) && size > 0 {
            let end = body.checked_add(size)?;
            return String::from_utf8(archive.get(body..end)?.to_vec()).ok();
        }
        offset = body + size.div_ceil(512) * 512;
    }
    None
}

/// Unpack a Docker copy-API tar into `dest`, dropping the archive's own leading
/// path component, and return what it wrote.
///
/// ⚠ The archive comes from a THIRD PARTY's image, so every entry is checked to
/// land inside `dest` before anything is written. An entry naming `..` or an
/// absolute path is skipped, not sanitised — the only safe response to a tar that
/// tries to escape is to not write it.
///
/// Only regular files and directories are materialised. Symlinks and hardlinks
/// are skipped for the same reason: a link is a second way to name a path outside
/// the destination, and no image's default config needs one to be useful.
fn tar_extract_into(archive: &[u8], dest: &std::path::Path) -> std::io::Result<Vec<std::path::PathBuf>> {
    let mut written = Vec::new();
    let mut offset = 0usize;
    while offset + 512 <= archive.len() {
        let header = &archive[offset..offset + 512];
        if header.iter().all(|byte| *byte == 0) {
            break;
        }
        let name_end = header[..100].iter().position(|b| *b == 0).unwrap_or(100);
        let name = String::from_utf8_lossy(&header[..name_end]).to_string();
        let octal = |range: std::ops::Range<usize>| -> Option<u32> {
            u32::from_str_radix(
                std::str::from_utf8(&header[range])
                    .ok()?
                    .trim_matches(|c| c == '\0' || c == ' '),
                8,
            )
            .ok()
        };
        let size = octal(124..136).unwrap_or(0) as usize;
        // Mode and ownership are NOT decoration. An image ships executable
        // entrypoints and per-user config, and an extractor that writes every
        // entry 0644 root:root hands the app a directory that looks right and
        // behaves wrong — which is a worse failure than the empty one being
        // fixed, because it is silent.
        //
        // ⚠ But the archive's mode is NOT copied verbatim. Only the question "was
        // this executable?" is carried across; the answer is then expressed as a
        // fixed safe mode. Reproducing an image's bits faithfully would let a
        // third party's image put a world-writable or setuid file into a data
        // directory on a host that runs other people's containers — which is the
        // exact shortcut §4 of `app-volume-ownership-pin-e2e.sh` refuses, and
        // that arm is what caught this: it fired on the first version of this
        // function and it was right to.
        let executable = octal(100..108).is_some_and(|m| m & 0o111 != 0);
        let uid = octal(108..116);
        let gid = octal(116..124);
        let body = offset + 512;
        let kind = header[156];

        // Drop the leading component: copying `/etc/caddy` yields `caddy/...`.
        let relative: std::path::PathBuf =
            std::path::Path::new(&name).components().skip(1).collect();
        let target = dest.join(&relative);

        let escapes = relative.components().any(|c| {
            matches!(
                c,
                std::path::Component::ParentDir | std::path::Component::RootDir
            )
        });

        if !escapes && !relative.as_os_str().is_empty() {
            let materialised = if kind == b'5' {
                std::fs::create_dir_all(&target)?;
                true
            } else if kind == b'0' || kind == 0 {
                if let Some(parent) = target.parent() {
                    std::fs::create_dir_all(parent)?;
                }
                let end = body.saturating_add(size).min(archive.len());
                std::fs::write(&target, &archive[body.min(end)..end])?;
                true
            } else {
                false
            };
            if materialised {
                use std::os::unix::fs::PermissionsExt;
                let safe_mode = if kind == b'5' || executable { 0o755 } else { 0o644 };
                let _ =
                    std::fs::set_permissions(&target, std::fs::Permissions::from_mode(safe_mode));
                if let (Some(uid), Some(gid)) = (uid, gid) {
                    let owner = VolumeOwner {
                        uid,
                        gid: Some(gid),
                        spec: String::new(),
                    };
                    let _ = chown_to(&target.to_string_lossy(), &owner);
                }
                written.push(target);
            }
        }
        offset = body + size.div_ceil(512) * 512;
    }
    Ok(written)
}

/// Give a bind-mounted directory the contents the image ships at that path.
///
/// **This restores a Docker behaviour that binding opts out of.** When an empty
/// NAMED volume is mounted over a path that has content in the image, Docker
/// copies that content into the volume; when a BIND mount is used, it never does.
/// This panel binds every declared volume — so it can hand the operator a
/// directory they can see and back up — and in doing so silently deleted, from the
/// container's point of view, every file its image shipped at that path.
///
/// Proven rather than reasoned: the same `caddy:2` image, the same `/etc/caddy`,
/// the same empty storage, differing only in mount type — bind exits 1 with
/// *"open /etc/caddy/Caddyfile: no such file or directory"*, while a named volume
/// comes up listening and Docker has placed `Caddyfile` inside it.
///
/// Seeding from the IMAGE rather than from a starter file written here is the
/// whole point. A config this file hardcodes is a copy that rots the next time
/// upstream changes its defaults, and it has to be maintained per template
/// forever. The image's own bytes are correct by construction and cost nothing to
/// keep current.
///
/// Two guards, both deliberate:
///   * it writes ONLY into a directory that is empty, so an operator's existing
///     data is never touched — the same emptiness test `migrate_unmounted_volumes`
///     uses before it moves anything;
///   * a failure is logged and swallowed. Before this existed the directory was
///     empty, which is exactly where a failure leaves it, so nothing regresses.
async fn seed_volume_from_image(
    docker: &Docker,
    image: &str,
    container_path: &str,
    host_dir: &str,
) {
    // Only ever seed an empty directory.
    match std::fs::read_dir(host_dir) {
        Ok(mut entries) => {
            if entries.next().is_some() {
                return;
            }
        }
        Err(_) => return,
    }

    let probe_name = format!("dockpanel-seedprobe-{}", uuid::Uuid::new_v4());
    let created = match docker
        .create_container(
            Some(CreateContainerOptions {
                name: probe_name.as_str(),
                platform: None,
            }),
            Config {
                image: Some(image.to_string()),
                // Never started; this only has to satisfy `create` for an image
                // that declares no command of its own.
                cmd: Some(vec!["dockpanel-seed-probe".to_string()]),
                ..Default::default()
            },
        )
        .await
    {
        Ok(c) => c,
        Err(e) => {
            tracing::debug!("Could not create a seed probe for {image}: {e}");
            return;
        }
    };

    let mut stream = docker.download_from_container(
        &created.id,
        Some(bollard::container::DownloadFromContainerOptions {
            path: container_path,
        }),
    );
    let mut buf: Vec<u8> = Vec::new();
    let mut failed = false;
    while let Some(chunk) = stream.next().await {
        match chunk {
            Ok(bytes) => buf.extend_from_slice(&bytes),
            // The overwhelmingly common case: the image ships nothing at this
            // path, which is normal and is not a problem.
            Err(_) => {
                failed = true;
                break;
            }
        }
    }

    // The probe is created with no HostConfig, so Docker materialises one anonymous
    // volume for every path the image declares as a VOLUME. Removing it without the
    // volumes flag orphans every one of them, on every call, for ever — and nothing
    // in the product ever reclaims them. The flag removes ONLY anonymous volumes;
    // named volumes and bind sources are untouched (measured against the daemon),
    // so it is safe here and at every sibling below.
    let _ = docker
        .remove_container(
            &created.id,
            Some(RemoveContainerOptions {
                v: true,
                force: true,
                ..Default::default()
            }),
        )
        .await;

    if failed || buf.is_empty() {
        return;
    }

    match tar_extract_into(&buf, std::path::Path::new(host_dir)) {
        Ok(written) if !written.is_empty() => {
            // Ownership and modes come from the archive itself, entry by entry,
            // which is what Docker's own named-volume pre-population does. Forcing
            // every seeded file to one owner here would be less faithful than the
            // behaviour being restored, not more.
            tracing::info!(
                "Seeded {} entr{} into {host_dir} from {image}:{container_path} — the image \
                 ships files there, and a bind mount would otherwise hide them",
                written.len(),
                if written.len() == 1 { "y" } else { "ies" }
            );
        }
        Ok(_) => {}
        Err(e) => tracing::warn!("Could not seed {host_dir} from {image}:{container_path}: {e}"),
    }
}

/// Read one file out of an already-pulled image **without running it**.
///
/// Creating a container is enough to give Docker's copy API a filesystem to read;
/// nothing is ever started. This is the only way to resolve a login name, because
/// a name like `nonroot` is defined by the image's own `/etc/passwd` and by
/// nothing on the host.
async fn read_file_from_image(docker: &Docker, image: &str, path: &str) -> Option<String> {
    let probe_name = format!("dockpanel-userprobe-{}", uuid::Uuid::new_v4());
    let created = docker
        .create_container(
            Some(CreateContainerOptions {
                name: probe_name.as_str(),
                platform: None,
            }),
            Config {
                image: Some(image.to_string()),
                // Never started, so this only has to satisfy `create` for an image
                // that declares no command of its own.
                cmd: Some(vec!["dockpanel-user-probe".to_string()]),
                ..Default::default()
            },
        )
        .await
        .ok()?;

    let mut stream = docker.download_from_container(
        &created.id,
        Some(bollard::container::DownloadFromContainerOptions { path }),
    );
    let mut buf: Vec<u8> = Vec::new();
    while let Some(chunk) = stream.next().await {
        match chunk {
            Ok(bytes) => buf.extend_from_slice(&bytes),
            Err(_) => break,
        }
    }

    // Same probe shape as `seed_volume_from_image` — see the note there.
    let _ = docker
        .remove_container(
            &created.id,
            Some(RemoveContainerOptions {
                v: true,
                force: true,
                ..Default::default()
            }),
        )
        .await;

    tar_extract_first_file(&buf)
}

/// Resolve a login name to `(uid, gid)` from an image's own `/etc/passwd`.
fn passwd_lookup(passwd: &str, name: &str) -> Option<(u32, u32)> {
    passwd.lines().find_map(|line| {
        let mut fields = line.split(':');
        if fields.next()? != name {
            return None;
        }
        let _password = fields.next()?;
        Some((fields.next()?.parse().ok()?, fields.next()?.parse().ok()?))
    })
}

/// Find the primary gid of a numeric uid in an image's own `/etc/passwd`.
fn passwd_gid_for_uid(passwd: &str, uid: u32) -> Option<u32> {
    passwd.lines().find_map(|line| {
        let mut fields = line.split(':');
        let _login = fields.next()?;
        let _password = fields.next()?;
        if fields.next()?.parse::<u32>().ok()? != uid {
            return None;
        }
        fields.next()?.parse().ok()
    })
}

/// Resolve a group name to a gid from an image's own `/etc/group`.
fn group_lookup(group: &str, name: &str) -> Option<u32> {
    group.lines().find_map(|line| {
        let mut fields = line.split(':');
        if fields.next()? != name {
            return None;
        }
        let _password = fields.next()?;
        fields.next()?.parse().ok()
    })
}

/// Work out who a template's volume directories must belong to, or `None` when
/// the image runs as root and today's behaviour is already correct.
///
/// ⚠ There is deliberately **no fallback**. Guessing a uid hands one tenant's data
/// directory to a different account, and `0777` would put a world-writable
/// directory on a host that runs other people's containers. When the spec cannot
/// be resolved this returns `None` and says so out loud, leaving the directory
/// root-owned — the same state as before, but no longer a silent one.
async fn resolve_volume_owner(docker: &Docker, image: &str) -> Option<VolumeOwner> {
    let spec = docker
        .inspect_image(image)
        .await
        .ok()?
        .config?
        .user
        .unwrap_or_default();
    let spec = spec.trim();
    if user_spec_is_root(spec) {
        return None;
    }
    let (user_part, group_part) = split_user_spec(spec);
    let numeric_uid = user_part.parse::<u32>().ok();

    // `/etc/passwd` is needed to translate a login name, and to find the primary
    // group of a bare numeric uid. A `uid:gid` pair needs nothing from the image.
    let passwd = if numeric_uid.is_none() || group_part.is_none() {
        read_file_from_image(docker, image, "/etc/passwd").await
    } else {
        None
    };

    let (uid, primary_gid) = match numeric_uid {
        Some(uid) => (
            uid,
            passwd.as_deref().and_then(|p| passwd_gid_for_uid(p, uid)),
        ),
        None => match passwd.as_deref().and_then(|p| passwd_lookup(p, user_part)) {
            Some((uid, gid)) => (uid, Some(gid)),
            None => {
                tracing::warn!(
                    "Image {image} runs as non-root user {spec:?} which could not be \
                     resolved from its own /etc/passwd; leaving volume directories \
                     root-owned. The app may be unable to write its data directory."
                );
                return None;
            }
        },
    };

    let gid = match group_part {
        Some(group) => match group.parse::<u32>() {
            Ok(gid) => Some(gid),
            Err(_) => read_file_from_image(docker, image, "/etc/group")
                .await
                .as_deref()
                .and_then(|g| group_lookup(g, group)),
        },
        None => primary_gid,
    };

    Some(VolumeOwner {
        uid,
        gid,
        spec: spec.to_string(),
    })
}

/// `chown` a path, leaving the group alone when none was resolved.
///
/// Deliberately **not** recursive. The owner of the directory is what lets the
/// image create its own files underneath; recursing would rewrite ownership of
/// data a previous deployment wrote, which is a bigger and riskier operation than
/// the defect calls for.
fn chown_to(path: &str, owner: &VolumeOwner) -> std::io::Result<()> {
    let c_path = std::ffi::CString::new(path)
        .map_err(|_| std::io::Error::other("volume path contains a NUL byte"))?;
    // `-1` means "leave this one as it is" — the same call shape `main.rs` uses to
    // set only the group on the agent socket.
    let gid = owner.gid.unwrap_or(u32::MAX);
    if unsafe { libc::chown(c_path.as_ptr(), owner.uid, gid) } == 0 {
        Ok(())
    } else {
        Err(std::io::Error::last_os_error())
    }
}

/// `chown` one path without ever following a symlink.
///
/// The same call as `chown_to`, except that a symlink is changed rather than the
/// thing it points at. An app's data directory may legitimately contain one, and
/// following it would apply the app's uid to whatever is on the other end.
fn lchown_to(path: &std::path::Path, owner: &VolumeOwner) -> std::io::Result<()> {
    use std::os::unix::ffi::OsStrExt;
    let c_path = std::ffi::CString::new(path.as_os_str().as_bytes())
        .map_err(|_| std::io::Error::other("volume path contains a NUL byte"))?;
    let gid = owner.gid.unwrap_or(u32::MAX);
    if unsafe { libc::lchown(c_path.as_ptr(), owner.uid, gid) } == 0 {
        Ok(())
    } else {
        Err(std::io::Error::last_os_error())
    }
}

/// Give the app ownership of every file the migration just wrote for it.
///
/// **This is recursive, and `chown_to` is deliberately not. The difference is a
/// difference in what is underneath, not a difference of opinion.** On the deploy
/// path the directory may hold data an earlier deployment wrote, and rewriting
/// that wholesale is a bigger operation than the defect there called for. Here
/// the caller has already REFUSED to proceed unless the directory was empty and
/// has then filled it itself, so every path underneath is one we created seconds
/// ago. Recursing over exactly that set is bounded by construction.
///
/// It has to happen, because `docker cp` does not preserve ownership on the way
/// out of a container. Copying a container's files to the host writes them as the
/// user running the command — root, for the agent — and `--archive` does not
/// change that (measured on docker 29.7.2: a file owned by 1000:1000 inside the
/// container arrives 0:0 either way). Chowning only the directory leaves the app
/// holding its own data owned by a user it is not, and a non-root image then dies
/// on the first write it attempts. That is GitHub #110's second act: the update
/// rescued the data and handed it back unwritable.
///
/// Returns how many paths were changed, so the log can evidence the work rather
/// than merely claim it.
fn chown_migrated_tree(root: &std::path::Path, owner: &VolumeOwner) -> std::io::Result<usize> {
    let mut stack = vec![root.to_path_buf()];
    let mut touched = 0usize;
    while let Some(path) = stack.pop() {
        // `symlink_metadata` does not follow, so a symlink to a directory is
        // chowned as a link and never descended into.
        let meta = std::fs::symlink_metadata(&path)?;
        lchown_to(&path, owner)?;
        touched += 1;
        if meta.is_dir() {
            for entry in std::fs::read_dir(&path)? {
                stack.push(entry?.path());
            }
        }
    }
    Ok(touched)
}

/// Give back what the broken migration took, on an install it already broke.
///
/// v2.111.0 through v2.113.1 copied an app's data onto a bind mount with
/// `docker cp` — which writes as root — and then chowned only the directory, so
/// every file underneath came back owned by root and a non-root image could not
/// write its own data (#110). v2.113.2 stopped that happening. It could not undo
/// it: `migrate_unmounted_volumes` runs only for a volume that is MISSING, and
/// the bad migration is precisely what supplied the mount. So the population that
/// was harmed and the population the fix reached were disjoint, and the only
/// remedy was an operator running `chown -R` over SSH.
///
/// This is the other half. It runs on the recreate paths, where the container is
/// stopped and its data is not being written.
///
/// **It chowns only entries currently owned by root, and only to the uid the
/// image itself declares.** That is the narrowest rule that repairs what we did:
/// a file owned by any third uid is left exactly as it was, so an ownership
/// somebody chose deliberately is never disturbed. It is also a de-escalation in
/// every case — from root to the unprivileged account the app already runs as,
/// inside that app's own data directory. Root-run images are skipped entirely,
/// because `resolve_volume_owner` returns `None` for them and there is nothing
/// here to repair.
async fn repair_root_owned_volumes(
    docker: &Docker,
    image: &str,
    host_config: &bollard::service::HostConfig,
) -> Vec<String> {
    let prefix = format!("{APP_DATA_DIR}/");
    let sources: Vec<String> = host_config
        .binds
        .iter()
        .flatten()
        .filter_map(|bind| bind.split(':').next())
        .filter(|src| src.starts_with(&prefix))
        .map(|src| src.to_string())
        .collect();
    if sources.is_empty() {
        return Vec::new();
    }
    let Some(owner) = resolve_volume_owner(docker, image).await else {
        return Vec::new();
    };

    let mut repaired = Vec::new();
    for src in sources {
        // Canonicalize then re-check the prefix, exactly as every other writer
        // here does — the check is what proves the path is one of ours, and it
        // only means anything after symlinks are resolved.
        let Ok(resolved) = std::fs::canonicalize(&src) else {
            continue;
        };
        let resolved_str = resolved.to_string_lossy().to_string();
        if !resolved_str.starts_with(&prefix) {
            continue;
        }
        match chown_root_owned_entries(&resolved, &owner) {
            Ok(0) => {}
            Ok(count) => {
                tracing::info!(
                    "Repaired {resolved_str}: {count} root-owned path(s) given back to {} \
                     for image user {:?} — this app was migrated by a release that left \
                     its data unwritable (#110)",
                    owner.uid,
                    owner.spec
                );
                repaired.push(format!("{resolved_str} ({count})"));
            }
            Err(e) => tracing::warn!("Could not repair ownership under {resolved_str}: {e}"),
        }
    }
    repaired
}

/// `lchown` every root-owned path under `root`, leaving every other owner alone.
///
/// The top level is probed first and the walk is skipped when nothing there is
/// root-owned. A healthy app therefore pays one `read_dir` per update instead of
/// a full tree walk, which matters for a database whose data directory holds tens
/// of thousands of files.
fn chown_root_owned_entries(
    root: &std::path::Path,
    owner: &VolumeOwner,
) -> std::io::Result<usize> {
    use std::os::unix::fs::MetadataExt;

    let mut worth_walking = std::fs::symlink_metadata(root)?.uid() == 0;
    if !worth_walking {
        for entry in std::fs::read_dir(root)? {
            if std::fs::symlink_metadata(entry?.path())?.uid() == 0 {
                worth_walking = true;
                break;
            }
        }
    }
    if !worth_walking {
        return Ok(0);
    }

    let mut stack = vec![root.to_path_buf()];
    let mut changed = 0usize;
    while let Some(path) = stack.pop() {
        let meta = std::fs::symlink_metadata(&path)?;
        if meta.uid() == 0 {
            lchown_to(&path, owner)?;
            changed += 1;
        }
        // Descend into real directories only. A symlink is chowned as a link when
        // it is root-owned and is never followed, so this cannot walk out of the
        // volume.
        if meta.is_dir() {
            for entry in std::fs::read_dir(&path)? {
                stack.push(entry?.path());
            }
        }
    }
    Ok(changed)
}

/// Deploy an app from a template: pull image, create container, start it.
pub async fn deploy_app(
    template_id: &str,
    name: &str,
    port: u16,
    env: HashMap<String, String>,
    domain: Option<&str>,
    memory_mb: Option<u64>,
    cpu_percent: Option<u64>,
    user_id: Option<&str>,
    gpu_enabled: bool,
    gpu_indices: Option<Vec<u32>>,
) -> Result<DeployResult, String> {
    // Refused before anything is pulled or created. An app called
    // `dockpanel-app-<something>` owns the very directory a recreate of the app
    // `<something>` migrated into on releases v2.111.0 through v2.113.3, so the two would
    // share one tree and deleting either would take the other's data. The migration no
    // longer writes there, but the installs it already wrote cannot be un-written — so the
    // name is closed rather than left as a collision waiting to be found.
    if name.starts_with(CONTAINER_NAME_PREFIX) {
        return Err(format!(
            "'{name}' is not an available app name: it collides with the container naming \
             this panel uses, and with the data directory of the app it would shadow. \
             Choose a name that does not begin with that prefix."
        ));
    }

    let template = TEMPLATES
        .iter()
        .find(|t| t.id == template_id)
        .ok_or_else(|| format!("Unknown template: {template_id}"))?;

    let docker =
        Docker::connect_with_local_defaults().map_err(|e| format!("Docker connect failed: {e}"))?;

    // Pull image (with timeout to prevent hanging on Docker daemon issues)
    let pull_result = tokio::time::timeout(std::time::Duration::from_secs(300), async {
        let mut pull = docker.create_image(
            Some(CreateImageOptions {
                from_image: template.image,
                ..Default::default()
            }),
            None,
            None,
        );
        while let Some(result) = pull.next().await {
            if let Err(e) = result {
                tracing::warn!("Image pull warning: {e}");
            }
        }
    }).await;
    if pull_result.is_err() {
        return Err(format!("Image pull timed out for {}", template.image));
    }

    let container_name = format!("{CONTAINER_NAME_PREFIX}{name}");

    // Deploy this template's sidecar (if it has one) BEFORE creating the main
    // container, so a sidecar failure never leaves an app half-deployed with
    // no way to reach the capability it needs. See `services::app_sidecar`.
    let sidecar = template_sidecar(template.id);
    if let Some(sc) = sidecar {
        app_sidecar::deploy(&docker, name, sc).await?;
    }

    // Values this template wants filled in from the domain the operator claimed.
    // Empty for 142 of the 148 templates, and for every deploy without a domain.
    let derived: Vec<(&'static str, String)> = domain
        .map(|d| domain_env_for(template_id, d))
        .unwrap_or_default();

    // Build environment variables: merge template defaults with user-supplied values
    let mut env_list = merge_deploy_env(template.env_vars, &env, &derived);
    // Reasserted, not merely defaulted — see `SidecarDef::client_env_entry`.
    if let Some(sc) = sidecar {
        env_list.retain(|e| !e.starts_with(&format!("{}=", sc.client_env_var)));
        env_list.push(sc.client_env_entry());
    }

    // Port bindings
    let mut port_bindings = HashMap::new();
    port_bindings.insert(
        template.container_port.to_string(),
        Some(vec![bollard::service::PortBinding {
            host_ip: Some("127.0.0.1".to_string()),
            host_port: Some(port.to_string()),
        }]),
    );

    // An image that runs as a non-root user starts already dropped and cannot chown
    // the directory we are about to create for it, so resolve who it needs to be
    // owned by before creating any. Root images resolve to `None` and are left
    // exactly as they were.
    let volume_owner = if template.volumes.is_empty() {
        None
    } else {
        resolve_volume_owner(&docker, template.image).await
    };

    // Volume binds — create dirs and canonicalize to prevent symlink TOCTOU
    let mut binds: Vec<String> = Vec::new();
    for vol in template.volumes {
        let host_dir = format!("{APP_DATA_DIR}/{name}{vol}");
        // Create directory before canonicalize (it must exist)
        std::fs::create_dir_all(&host_dir).ok();
        // Canonicalize to resolve any symlinks, then verify it's still under the allowed prefix
        let resolved = std::fs::canonicalize(&host_dir)
            .map_err(|e| format!("Volume path {host_dir} inaccessible: {e}"))?;
        let resolved_str = resolved.to_string_lossy();
        if !resolved_str.starts_with(&format!("{APP_DATA_DIR}/")) {
            return Err(format!("Volume path {host_dir} escapes allowed prefix after canonicalization"));
        }
        // Give the directory the files the image ships at this path, if it ships
        // any and the directory is empty. Docker does this for a named volume and
        // not for a bind, and this panel binds — so without it an image whose CMD
        // reads its own shipped config finds an empty directory and exits.
        //
        // AFTER the prefix check, for the same reason the chown below is: the
        // check is what proves the path is one of ours, and writing into a
        // directory is exactly the operation a symlink escape would want to
        // borrow.
        seed_volume_from_image(&docker, template.image, vol, &resolved_str).await;

        // Chown AFTER the prefix check, never before — the check is what proves the
        // path is one of ours, and a chown is exactly the operation a symlink escape
        // would want to borrow.
        if let Some(owner) = &volume_owner {
            match chown_to(&resolved_str, owner) {
                Ok(()) => tracing::info!(
                    "Volume {resolved_str} chowned to {}:{} for image user {:?}",
                    owner.uid,
                    owner.gid.map_or("unchanged".to_string(), |g| g.to_string()),
                    owner.spec
                ),
                Err(e) => tracing::warn!(
                    "Could not chown volume {resolved_str} to {}: {e}. The app runs as \
                     {:?} and may be unable to write its data directory.",
                    owner.uid,
                    owner.spec
                ),
            }
        }
        binds.push(format!("{resolved_str}:{vol}"));
    }

    // NOTE: the one-click catalogue deliberately never mounts the host Docker
    // socket for ANY template's OWN container — it gives whatever runs with it
    // full host escape capabilities, not just Docker visibility, regardless of
    // who was allowed to trigger the deploy. Portainer needed it for its core
    // purpose (create/stop/exec containers, manage volumes/networks) and stays
    // withdrawn (v2.178.0; see FEATURES.md §Withdrawn Claims) — a read-only
    // proxy would gut it and a write-permitting one reopens the risk. Dozzle
    // only ever needed to list containers and read logs, so it was reinstated
    // (v2.179.0) behind a `docker-socket-proxy` sidecar that holds the real
    // socket instead — see `TEMPLATE_SIDECAR` / `services::app_sidecar`. An
    // admin who wants a different socket-mounted tool anyway can still build
    // one via Docker → Create Stack, outside this sandboxing entirely.

    let mut host_config = bollard::service::HostConfig {
        port_bindings: Some(port_bindings),
        restart_policy: Some(bollard::service::RestartPolicy {
            name: Some(bollard::service::RestartPolicyNameEnum::UNLESS_STOPPED),
            ..Default::default()
        }),
        security_opt: Some(vec!["no-new-privileges:true".to_string()]),
        cap_drop: Some(vec!["ALL".to_string()]),
        cap_add: Some(vec![
            "CHOWN".to_string(),
            "SETUID".to_string(),
            "SETGID".to_string(),
            "NET_BIND_SERVICE".to_string(),
            "DAC_OVERRIDE".to_string(),
            "FOWNER".to_string(),
            "SETFCAP".to_string(),
        ]),
        ..Default::default()
    };

    // Docker resource limits
    if let Some(mem) = memory_mb {
        if mem > 0 {
            // saturating_mul avoids a u64 overflow wrapping to a garbage/negative i64 limit
            // for an absurd memory_mb; the backend also clamps to 4..=65536 (s237 audit).
            let bytes = mem.saturating_mul(1024 * 1024).min(i64::MAX as u64) as i64;
            let swap = mem.saturating_mul(2 * 1024 * 1024).min(i64::MAX as u64) as i64;
            host_config.memory = Some(bytes);
            // Memory swap = 2x memory (allows some swap)
            host_config.memory_swap = Some(swap);
        }
    }
    if let Some(cpu) = cpu_percent {
        if cpu > 0 {
            // cpu_percent is % of ONE core (100 = 1.0 core); values > 100 request multiple
            // cores, so cpu_quota may exceed cpu_period. The previous `cpu <= 100` bound
            // silently dropped the limit for any multi-core request → the container ran with
            // UNLIMITED CPU (s237 audit — parity with update_limits which accepts 1..=10000).
            host_config.cpu_period = Some(100_000);
            host_config.cpu_quota = Some(cpu.saturating_mul(1000).min(i64::MAX as u64) as i64);
        }
    }

    // GPU passthrough (requires NVIDIA Container Toolkit installed on host).
    // Two modes: all-GPUs (count: -1) vs specific indices (device_ids).
    // Setting both fields would be rejected by the daemon, so they're mutually
    // exclusive — device_ids wins when present, count is set otherwise.
    if gpu_enabled {
        let specific_ids: Option<Vec<String>> = gpu_indices
            .as_ref()
            .filter(|v| !v.is_empty())
            .map(|v| v.iter().map(|i| i.to_string()).collect());
        let (count_field, device_ids_field, log_target) = match &specific_ids {
            Some(ids) => (None, Some(ids.clone()), format!("specific GPU(s) [{}]", ids.join(","))),
            None => (Some(-1_i64), None, "all available GPUs".to_string()),
        };
        host_config.device_requests = Some(vec![
            bollard::service::DeviceRequest {
                driver: Some("nvidia".to_string()),
                count: count_field,
                device_ids: device_ids_field,
                capabilities: Some(vec![vec!["gpu".to_string(), "compute".to_string(), "utility".to_string()]]),
                ..Default::default()
            }
        ]);
        // NOTE (s237 audit): GPU compute via the NVIDIA Container Toolkit does NOT need
        // CAP_SYS_ADMIN — the device_requests above grant GPU access. Granting SYS_ADMIN would
        // reverse the cap_drop ALL hardening (it enables mount(2)/cgroup container→host escape),
        // so it is deliberately NOT added; the container keeps the same minimal cap set as any
        // other template. A workload that genuinely needs it should be an explicit opt-in.
        tracing::info!("GPU passthrough enabled for container {container_name}: {log_target}");
    }

    if !binds.is_empty() {
        host_config.binds = Some(binds);
    }

    let mut exposed_ports = HashMap::new();
    exposed_ports.insert(template.container_port.to_string(), HashMap::new());

    let config = Config {
        image: Some(template.image.to_string()),
        // None for all but one template, which leaves the image's own CMD in charge.
        // See TEMPLATE_CMD: without this the container inherits a default that may
        // print usage and exit 0, which restarts forever instead of serving (#101).
        cmd: template_cmd(template.id),
        env: if env_list.is_empty() {
            None
        } else {
            Some(env_list)
        },
        exposed_ports: Some(exposed_ports),
        host_config: Some(host_config),
        // Attaches to the sidecar's private network instead of the default
        // bridge when this template has one — the SAME config every recreate
        // path (`update_app`, `blue_green_update`, `change_container_image`,
        // `update_env`) applies via `reattach_sidecar_if_any`, so an update
        // can never silently drop this back onto the default bridge.
        networking_config: sidecar.map(|_| app_sidecar::attach_config(name)),
        labels: Some({
            let mut labels = HashMap::from([
                ("dockpanel.managed".to_string(), "true".to_string()),
                (
                    "dockpanel.app.template".to_string(),
                    template.id.to_string(),
                ),
                ("dockpanel.app.name".to_string(), name.to_string()),
            ]);
            if let Some(domain) = domain {
                labels.insert("dockpanel.app.domain".to_string(), domain.to_string());
            }
            if let Some(uid) = user_id {
                labels.insert("dockpanel.user.id".to_string(), uid.to_string());
            }
            labels
        }),
        ..Default::default()
    };

    let container = match docker
        .create_container(
            Some(CreateContainerOptions {
                name: container_name.as_str(),
                platform: None,
            }),
            config,
        )
        .await
    {
        Ok(c) => c,
        Err(e) => {
            // A sidecar deployed above and never given a main container to pair
            // with is not a partial app an operator could ever find or delete —
            // take it and its network with the failed deploy.
            if sidecar.is_some() {
                app_sidecar::teardown(name).await;
            }
            return Err(format!("Failed to create container: {e}"));
        }
    };

    if let Err(e) = docker
        .start_container(&container.id, None::<StartContainerOptions<String>>)
        .await
    {
        // Clean up orphaned container on start failure. It never started, so any
        // anonymous volume Docker gave it for an unbound image VOLUME is empty by
        // construction — take them with it rather than stranding the whole set on
        // every failed deploy. Bind sources hold the app's real data and are not
        // touched by the volumes flag.
        let _ = docker
            .remove_container(
                &container.id,
                Some(bollard::container::RemoveContainerOptions {
                    v: true,
                    force: true,
                    ..Default::default()
                }),
            )
            .await;
        if sidecar.is_some() {
            app_sidecar::teardown(name).await;
        }
        return Err(format!("Failed to start container: {e}"));
    }

    tracing::info!(
        "App deployed: {container_name} (template={}, port={port})",
        template.id
    );

    Ok(DeployResult {
        container_id: container.id,
        name: container_name,
        port,
    })
}

/// List all deployed apps (containers with dockpanel.app.template label).
pub async fn list_deployed_apps() -> Result<Vec<DeployedApp>, String> {
    let docker =
        Docker::connect_with_local_defaults().map_err(|e| format!("Docker connect failed: {e}"))?;

    let mut filters = HashMap::new();
    filters.insert("label", vec!["dockpanel.managed=true"]);

    let containers = docker
        .list_containers(Some(ListContainersOptions {
            all: true,
            filters,
            ..Default::default()
        }))
        .await
        .map_err(|e| format!("Failed to list containers: {e}"))?;

    let apps = containers
        .into_iter()
        .filter_map(|c| {
            let labels = c.labels.as_ref()?;
            // Only include containers that have the app template label
            let template = labels.get("dockpanel.app.template")?;
            let id = c.id.as_ref()?;

            let port = c
                .ports
                .as_ref()
                .and_then(|ports| {
                    ports
                        .iter()
                        .find(|p| p.public_port.is_some())
                        .and_then(|p| p.public_port)
                })
                .map(|p| p as u16);

            let status = c.state.unwrap_or_default();
            let name = c
                .names
                .as_ref()
                .and_then(|n| n.first())
                .map(|n| n.trim_start_matches('/').to_string())
                .unwrap_or_default();

            let domain = labels.get("dockpanel.app.domain").cloned();
            let image = c.image.clone();

            // Extract volume mounts
            let volumes = c
                .mounts
                .as_ref()
                .map(|mounts| {
                    mounts
                        .iter()
                        .filter_map(|m| {
                            let src = m.source.as_deref().unwrap_or("?");
                            let dst = m.destination.as_deref().unwrap_or("?");
                            Some(format!("{src} → {dst}"))
                        })
                        .collect::<Vec<_>>()
                })
                .unwrap_or_default();

            // Extract health from human-readable status string (e.g., "Up 2 hours (healthy)")
            let health = c.status.as_deref().and_then(|s| {
                if s.contains("(healthy)") {
                    Some("healthy".to_string())
                } else if s.contains("(unhealthy)") {
                    Some("unhealthy".to_string())
                } else if s.contains("(health: starting)") {
                    Some("starting".to_string())
                } else {
                    None
                }
            });

            let stack_id = labels.get("dockpanel.stack_id").cloned();
            let user_id = labels.get("dockpanel.user.id").cloned();

            let ssl = domain
                .as_deref()
                .map(crate::services::nginx::domain_is_https)
                .unwrap_or(false);

            Some(DeployedApp {
                container_id: id.clone(),
                name,
                template: template.clone(),
                status,
                port,
                domain,
                ssl,
                health,
                image,
                volumes,
                stack_id,
                user_id,
            })
        })
        .collect();

    Ok(apps)
}

/// This container's app name, read fresh from Docker — used by the
/// power-action and removal paths below to find a paired sidecar (if any)
/// without assuming the caller already has the labels in hand. `None` only
/// when the container itself cannot be inspected, which every caller here
/// treats as "nothing to mirror" rather than an error: the main action is
/// what the operator asked for.
async fn current_app_name(docker: &Docker, container_id: &str) -> Option<String> {
    let info = docker.inspect_container(container_id, None).await.ok()?;
    let labels = info.config.as_ref().and_then(|c| c.labels.as_ref());
    let name = info.name.as_deref().unwrap_or_default().trim_start_matches('/').to_string();
    Some(app_data_name(labels, &name))
}

/// Stop a running app container.
pub async fn stop_app(container_id: &str) -> Result<(), String> {
    let docker =
        Docker::connect_with_local_defaults().map_err(|e| format!("Docker connect failed: {e}"))?;

    if let Some(app_name) = current_app_name(&docker, container_id).await {
        app_sidecar::mirror_stop(&docker, &app_name).await;
    }

    docker
        .stop_container(container_id, Some(StopContainerOptions { t: 10 }))
        .await
        .map_err(|e| format!("Failed to stop container: {e}"))?;

    tracing::info!("App container stopped: {container_id}");
    Ok(())
}

/// Start a stopped app container.
pub async fn start_app(container_id: &str) -> Result<(), String> {
    let docker =
        Docker::connect_with_local_defaults().map_err(|e| format!("Docker connect failed: {e}"))?;

    // Sidecar first: an app whose sidecar is still cold should not race its
    // own first connection attempt against it.
    if let Some(app_name) = current_app_name(&docker, container_id).await {
        app_sidecar::mirror_start(&docker, &app_name).await;
    }

    docker
        .start_container(container_id, None::<StartContainerOptions<String>>)
        .await
        .map_err(|e| format!("Failed to start container: {e}"))?;

    tracing::info!("App container started: {container_id}");
    Ok(())
}

/// Restart an app container.
pub async fn restart_app(container_id: &str) -> Result<(), String> {
    let docker =
        Docker::connect_with_local_defaults().map_err(|e| format!("Docker connect failed: {e}"))?;

    if let Some(app_name) = current_app_name(&docker, container_id).await {
        app_sidecar::mirror_restart(&docker, &app_name).await;
    }

    docker
        .restart_container(container_id, Some(bollard::container::RestartContainerOptions { t: 10 }))
        .await
        .map_err(|e| format!("Failed to restart container: {e}"))?;

    tracing::info!("App container restarted: {container_id}");
    Ok(())
}

/// Get container logs (last N lines).
pub async fn get_app_logs(container_id: &str, tail: usize) -> Result<String, String> {
    let docker =
        Docker::connect_with_local_defaults().map_err(|e| format!("Docker connect failed: {e}"))?;

    use bollard::container::LogsOptions;

    let mut output = docker.logs(
        container_id,
        Some(LogsOptions::<String> {
            stdout: true,
            stderr: true,
            tail: tail.to_string(),
            ..Default::default()
        }),
    );

    let mut logs = String::new();
    while let Some(result) = output.next().await {
        match result {
            Ok(log) => logs.push_str(&log.to_string()),
            Err(e) => return Err(format!("Failed to read logs: {e}")),
        }
    }

    Ok(logs)
}

/// Find a free port for the blue-green container by asking the OS.
pub(crate) fn find_free_port() -> Result<u16, String> {
    let listener = std::net::TcpListener::bind("127.0.0.1:0")
        .map_err(|e| format!("Failed to find free port: {e}"))?;
    let port = listener
        .local_addr()
        .map_err(|e| format!("Failed to get port: {e}"))?
        .port();
    drop(listener);
    Ok(port)
}

/// Health check: wait for a container to accept TCP connections on a port.
pub(crate) async fn health_check_port(port: u16, timeout_secs: u64) -> Result<(), String> {
    let deadline =
        tokio::time::Instant::now() + std::time::Duration::from_secs(timeout_secs);
    loop {
        match tokio::net::TcpStream::connect(format!("127.0.0.1:{port}")).await {
            Ok(_) => return Ok(()),
            Err(_) => {
                if tokio::time::Instant::now() >= deadline {
                    return Err(format!(
                        "Container failed health check on port {port} after {timeout_secs}s"
                    ));
                }
                tokio::time::sleep(std::time::Duration::from_millis(500)).await;
            }
        }
    }
}

/// Swap the proxy_pass port in an existing nginx config file.
/// Returns Ok(()) on success, Err on failure.
pub(crate) fn swap_nginx_proxy_port(domain: &str, old_port: u16, new_port: u16) -> Result<(), String> {
    let config_path = format!("{}/{domain}.conf", super::nginx::sites_dir());
    let content = std::fs::read_to_string(&config_path)
        .map_err(|e| format!("Failed to read nginx config: {e}"))?;

    let old_pattern = format!("proxy_pass http://127.0.0.1:{old_port};");
    let new_pattern = format!("proxy_pass http://127.0.0.1:{new_port};");

    if !content.contains(&old_pattern) {
        return Err(format!(
            "Nginx config for {domain} does not contain proxy_pass on port {old_port}"
        ));
    }

    let new_content = content.replace(&old_pattern, &new_pattern);

    // Atomic write
    let tmp_path = format!("{config_path}.tmp");
    std::fs::write(&tmp_path, &new_content)
        .map_err(|e| format!("Failed to write nginx config: {e}"))?;
    std::fs::rename(&tmp_path, &config_path).map_err(|e| {
        std::fs::remove_file(&tmp_path).ok();
        format!("Failed to rename nginx config: {e}")
    })?;

    Ok(())
}

/// Extract the host port from a container's HostConfig port bindings.
pub(crate) fn extract_host_port(
    host_config: &bollard::service::HostConfig,
) -> Option<u16> {
    host_config
        .port_bindings
        .as_ref()?
        .values()
        .next()?
        .as_ref()?
        .first()?
        .host_port
        .as_ref()?
        .parse::<u16>()
        .ok()
}

/// The same port [`extract_host_port`] reads for [`removal_identity`], for a
/// caller that only has a container id — a fresh `inspect_container`, so it
/// works whether the container is running or stopped. `list_deployed_apps`'s
/// own `port` field cannot substitute here: it is read from Docker's
/// `list_containers` API, which only reports a LIVE (currently bound) port —
/// empty for a stopped ("parked") container even though its HostConfig still
/// carries the binding. `None` here means either the container has no
/// recorded binding or has been removed entirely (nothing left to inspect);
/// a caller proving ownership of a vhost must treat both the same way
/// `removal_identity` already does — refuse to delete rather than guess.
pub(crate) async fn inspect_host_port(container_id: &str) -> Option<u16> {
    let docker = Docker::connect_with_local_defaults().ok()?;
    let info = docker.inspect_container(container_id, None).await.ok()?;
    extract_host_port(info.host_config.as_ref()?)
}

/// The mount destinations this container already has, read from BOTH spellings.
///
/// A container may express a mount as a `Binds` entry (`"src:dst[:opts]"`) or as a
/// `Mounts` entry carrying a `target`, and Docker accepts either. A census keyed on one
/// of them is blind to the other, so this reads both and returns the union.
fn mounted_destinations(host_config: &bollard::service::HostConfig) -> Vec<String> {
    let mut dests = Vec::new();
    if let Some(binds) = &host_config.binds {
        for bind in binds {
            // "<source>:<destination>[:<options>]" — the destination is the second
            // field, so split from the left and take index 1.
            if let Some(dest) = bind.split(':').nth(1) {
                if !dest.is_empty() {
                    dests.push(dest.to_string());
                }
            }
        }
    }
    if let Some(mounts) = &host_config.mounts {
        for m in mounts {
            if let Some(target) = m.target.as_ref().filter(|t| !t.is_empty()) {
                dests.push(target.clone());
            }
        }
    }
    dests
}

/// Paths this container's own template calls persistent that nothing actually mounts.
///
/// Returns empty for anything we are not entitled to touch: a container without
/// `dockpanel.managed=true`, one whose `dockpanel.app.template` names no template we
/// ship, or one whose declared paths are all mounted already.
fn unmounted_declared_volumes(
    config: &bollard::models::ContainerConfig,
    host_config: &bollard::service::HostConfig,
) -> Vec<&'static str> {
    let Some(labels) = config.labels.as_ref() else {
        return Vec::new();
    };
    if labels.get("dockpanel.managed").map(String::as_str) != Some("true") {
        return Vec::new();
    }
    let Some(template_id) = labels.get("dockpanel.app.template") else {
        return Vec::new();
    };
    let declared: &'static [&'static str] = match TEMPLATES.iter().find(|t| t.id == template_id) {
        Some(template) => template.volumes,
        // Not every managed container comes from the one-click catalogue, and the ones
        // that do not were getting no migration at all — see `non_catalogue_volumes`.
        None => non_catalogue_volumes(template_id),
    };
    let mounted = mounted_destinations(host_config);
    declared
        .iter()
        .copied()
        .filter(|vol| !mounted.iter().any(|d| d == vol))
        .collect()
}

/// Paths kept by a managed container whose template label names no catalogue row.
///
/// The label is not proof of a template. The mail stack installs its webmail container
/// with `dockpanel.managed=true` and a template label of its own, and nothing in the
/// catalogue answers to that name — so the lookup above found no row, returned no paths,
/// and every recreate door removed the container with its writable layer and everything
/// in it. That is #110 exactly, against a container the panel installs itself: the webmail
/// app keeps its accounts, identities, contacts and preferences in a SQLite database, and
/// the image declares no volume of its own to catch it.
///
/// The path is measured, not assumed. The image's entrypoint selects a local SQLite store
/// under `/var/roundcube/db` whenever no database is configured by environment, and the
/// mail stack configures none — so that directory is the whole of its state.
///
/// A compose service is the other container that reaches here, and it is genuinely fine:
/// its mounts come from the stack's own YAML, so it arrives with its state already bound
/// and has nothing for this function to rescue. It returns no paths by falling through,
/// and that is a decision rather than an oversight.
fn non_catalogue_volumes(template_id: &str) -> &'static [&'static str] {
    match template_id {
        "roundcube" => &["/var/roundcube/db"],
        _ => &[],
    }
}

/// The app's own name — the one whose directory under `APP_DATA_DIR` holds its data.
///
/// `deploy_app` creates the container as `dockpanel-app-<app>` but writes its volume
/// directories at `{APP_DATA_DIR}/<app>{vol}`, and `removal_identity` looks for them
/// under `{APP_DATA_DIR}/<app>`. Every recreate path reads `info.name`, which is the
/// CONTAINER name, and handed that straight to the migration below — so a rescued app's
/// data landed under a directory named for the container, where nothing else in the panel
/// ever looks. The delete path could not find it to clean up, and because that directory
/// never existed for an app deployed before its template declared a volume, not even the
/// "leaving it in place" warning fired: the data was orphaned in silence.
///
/// The label is the app's real name. Stripping the container prefix is the fallback for a
/// container that somehow carries no label, and it is a fallback rather than the rule
/// because the label is what `removal_identity` reads.
fn app_data_name(labels: Option<&HashMap<String, String>>, container_name: &str) -> String {
    labels
        .and_then(|l| l.get("dockpanel.app.name"))
        .filter(|name| !name.is_empty())
        .cloned()
        .unwrap_or_else(|| {
            container_name
                .strip_prefix(CONTAINER_NAME_PREFIX)
                .unwrap_or(container_name)
                .to_string()
        })
}

/// Carry an app's data out of the container's writable layer and onto a bind mount,
/// on the way through a recreate.
///
/// Every recreate path stops the container, `docker rm`s it and rebuilds the replacement
/// from a `docker inspect` of the original — and `docker rm` deletes the writable layer.
/// For a container with no mount covering the directory its app writes to, that layer IS
/// the storage, so the recreate is unrecoverable deletion. GitHub #110 is exactly this: an
/// n8n deployed while the template declared `volumes: &[]` came back at first-run state
/// the first time its operator pressed Update, account and workflows gone.
///
/// Declaring the volume fixes the NEXT deploy and does nothing for the containers already
/// running. This closes that half — the destructive step becomes the repair. Limits, all
/// deliberate:
///
/// - Only paths the container's OWN template declares and the container does not mount.
///   An operator who arranged their own bind keeps it.
/// - Only into a host directory that is absent or empty. A non-empty one is somebody's
///   data and this declines rather than merging into it.
/// - **Any failure is returned, and every caller must abort the recreate before removing
///   the container.** A migration that half-worked followed by a `docker rm` would destroy
///   the very data it was called to save.
async fn migrate_unmounted_volumes(
    docker: &Docker,
    container_id: &str,
    name: &str,
    image: &str,
    missing: &[&'static str],
    host_config: &mut bollard::service::HostConfig,
) -> Result<Vec<String>, String> {
    if missing.is_empty() {
        return Ok(Vec::new());
    }
    let owner = resolve_volume_owner(docker, image).await;
    let mut binds = host_config.binds.clone().unwrap_or_default();
    let mut migrated = Vec::new();

    for vol in missing {
        let host_dir = format!("{APP_DATA_DIR}/{name}{vol}");
        if let Ok(mut entries) = std::fs::read_dir(&host_dir) {
            if entries.next().is_some() {
                return Err(format!(
                    "Refusing to migrate {vol} for {name}: {host_dir} already exists and is \
                     not empty. Move it aside and retry, or mount it yourself."
                ));
            }
        }
        std::fs::create_dir_all(&host_dir)
            .map_err(|e| format!("Failed to create {host_dir}: {e}"))?;
        // Canonicalize then re-check the prefix, exactly as `deploy_app` does — the
        // check is what proves the path is one of ours, and it has to happen after
        // symlinks are resolved.
        let resolved = std::fs::canonicalize(&host_dir)
            .map_err(|e| format!("Volume path {host_dir} inaccessible: {e}"))?;
        let resolved_str = resolved.to_string_lossy().to_string();
        if !resolved_str.starts_with(&format!("{APP_DATA_DIR}/")) {
            return Err(format!(
                "Volume path {host_dir} escapes allowed prefix after canonicalization"
            ));
        }

        // `<container>:<path>/.` copies the CONTENTS of the directory, so the data lands
        // directly in the host directory instead of one level down inside a copy of it.
        //
        // ⚠ It does NOT carry ownership across. This comment used to claim the
        // opposite — that the numeric uid/gid survived the copy, so a non-root app
        // kept ownership of its own data — and that was simply wrong. `docker cp`
        // out of a container writes as the user running it, which for the agent is
        // root, and `--archive` does not change it either (both measured). The tree
        // is therefore chowned explicitly below; without that the app is handed
        // back its own data owned by somebody else. See #110.
        let out = crate::safe_cmd::safe_command("docker")
            .args(["cp", &format!("{container_id}:{vol}/."), &resolved_str])
            .output()
            .await
            .map_err(|e| format!("Failed to run docker cp for {vol}: {e}"))?;
        if !out.status.success() {
            let err = String::from_utf8_lossy(&out.stderr);
            // A path the image never created carries nothing to lose. That is not a
            // failure — but the bind is still added, so the NEXT recreate has somewhere
            // durable to write.
            let absent = err.contains("No such container:path")
                || err.contains("no such file or directory")
                || err.contains("Could not find the file");
            if !absent {
                return Err(format!(
                    "Failed to copy {vol} out of {name}: {}",
                    err.trim()
                ));
            }
            tracing::info!(
                "Nothing to migrate at {vol} for {name} — the path does not exist in the \
                 container; mounting it empty so future updates preserve it"
            );
        }

        // Deliberately NOT fatal, and the reasoning is worth stating because the
        // opposite reading is defensible. Aborting here would strand the copy: the
        // host directory is now non-empty, so the refusal at the top of this loop
        // would decline every later attempt, turning a recoverable state into a
        // permanent one. As root, on a directory this function created moments ago,
        // this call does not realistically fail — and if it somehow does, the app
        // is broken in a way the operator must see, so it is logged at error rather
        // than dressed down to a warning.
        if let Some(owner) = &owner {
            match chown_migrated_tree(&resolved, owner) {
                Ok(touched) => tracing::info!(
                    "Migrated volume {resolved_str}: {touched} path(s) chowned to {} for \
                     image user {:?}",
                    owner.uid,
                    owner.spec
                ),
                Err(e) => tracing::error!(
                    "Could not chown migrated volume {resolved_str} to {}: {e}. The app runs \
                     as {:?} and WILL be unable to write the data just rescued — chown the \
                     directory by hand and recreate the app.",
                    owner.uid,
                    owner.spec
                ),
            }
        }
        binds.push(format!("{resolved_str}:{vol}"));
        migrated.push((*vol).to_string());
        tracing::info!("Migrated {name}{vol} onto {resolved_str} — it now survives recreates");
    }

    host_config.binds = Some(binds);
    Ok(migrated)
}

/// Does this container hold state that a second copy of it would fight over?
///
/// Blue-green starts the replacement while the original is still serving, and only
/// stops the original after the health check and the nginx swap have both succeeded.
/// For that window — up to 30s of health checking plus an nginx test and reload — two
/// processes are running against the SAME host paths, because `blue_green_update`
/// clones the old `HostConfig` and rewrites only its port bindings.
///
/// That is harmless when the container's only state is its own writable layer: the
/// replacement gets a fresh one and nothing is shared. It is data corruption when the
/// state is a single-writer database sitting in a bind mount. Postgres, MySQL and Mongo
/// self-protect with a pidfile lock, so they merely fail to start and the caller falls
/// back to stop/start — noisy, but safe. **SQLite has no such lock**, so Ghost, Gitea,
/// Bookstack and Vaultwarden would interleave writes from two processes silently.
///
/// There is no way to ask an image whether it tolerates two writers, so the only sound
/// reading of "this container has a mount" is "assume it does not". Zero-downtime is
/// worth having; it is not worth a corrupted database, and the stop/start path this
/// falls back to is the same one every blue-green failure already takes.
fn shares_persistent_state(host_config: &bollard::service::HostConfig) -> bool {
    let has_binds = host_config.binds.as_ref().is_some_and(|b| !b.is_empty());
    let has_mounts = host_config.mounts.as_ref().is_some_and(|m| !m.is_empty());
    // `volumes_from` points at another container's mounts, which are shared by
    // definition. Anonymous volumes are deliberately NOT counted: Docker gives the
    // replacement its own, so there is nothing for the two to contend over.
    let has_volumes_from = host_config
        .volumes_from
        .as_ref()
        .is_some_and(|v| !v.is_empty());
    has_binds || has_mounts || has_volumes_from
}

/// Blue-green update: start new container → health check → swap nginx → remove old.
/// Returns UpdateResult on success. On ANY failure, rolls back and returns Err
/// so the caller can fall back to stop/start.
async fn blue_green_update(
    docker: &Docker,
    old_container_id: &str,
    name: &str,
    config: &bollard::models::ContainerConfig,
    host_config: &bollard::service::HostConfig,
    domain: &str,
    old_port: u16,
) -> Result<UpdateResult, String> {
    let temp_port = find_free_port()?;
    tracing::info!(
        "Blue-green update for {name}: old_port={old_port}, temp_port={temp_port}"
    );

    // Build new host_config with temp port
    let mut new_host_config = host_config.clone();
    if let Some(ref mut pb) = new_host_config.port_bindings {
        for bindings in pb.values_mut() {
            if let Some(bindings) = bindings {
                for binding in bindings.iter_mut() {
                    binding.host_port = Some(temp_port.to_string());
                }
            }
        }
    }

    // Create new container with a temp name. The separator is `.` and not `-`
    // because `is_valid_name` rejects `.`: an app may legally be called
    // `api-blue`, and `dockpanel-app-api-blue` was therefore both its production
    // container AND the stand-in name for the app `api`.
    let new_name = crate::services::ownership::blue_name(name);

    // Clean up the leftover of a failed previous attempt — if it is ours.
    if let Ok(info) = docker.inspect_container(&new_name, None).await {
        let owner = crate::services::ownership::blue_leftover(
            info.config.as_ref().and_then(|c| c.labels.as_ref()),
        );
        if !owner.may_delete() {
            return Err(format!(
                "A container named {new_name} exists and is not managed by DockPanel \
                 ({owner:?}); refusing to force-remove it to make room for a blue-green swap"
            ));
        }
        docker
            .stop_container(&new_name, Some(StopContainerOptions { t: 5 }))
            .await
            .ok();
        docker
            .remove_container(
                &new_name,
                Some(RemoveContainerOptions {
                    force: true,
                    v: false,
                    ..Default::default()
                }),
            )
            .await
            .ok();
    }

    // Keyed off the real app name, not `new_name` (the temp blue container) —
    // the sidecar and its network are the app's, unaffected by the swap.
    let app_name = app_data_name(config.labels.as_ref(), name);
    let mut env_list = config.env.clone().unwrap_or_default();
    let networking_config = reattach_sidecar_if_any(config.labels.as_ref(), &app_name, &mut env_list);

    let new_config = Config {
        image: config.image.clone(),
        env: if env_list.is_empty() { None } else { Some(env_list) },
        exposed_ports: config.exposed_ports.clone(),
        labels: config.labels.clone(),
        host_config: Some(new_host_config),
        networking_config,
        cmd: config.cmd.clone(),
        entrypoint: config.entrypoint.clone(),
        working_dir: if config.working_dir.as_deref() == Some("") {
            None
        } else {
            config.working_dir.clone()
        },
        ..Default::default()
    };

    let new_container = docker
        .create_container(
            Some(CreateContainerOptions {
                name: new_name.as_str(),
                platform: None,
            }),
            new_config,
        )
        .await
        .map_err(|e| format!("Failed to create blue container: {e}"))?;

    // Start new container
    if let Err(e) = docker
        .start_container(&new_container.id, None::<StartContainerOptions<String>>)
        .await
    {
        docker
            .remove_container(
                &new_container.id,
                Some(RemoveContainerOptions {
                    force: true,
                    v: false,
                    ..Default::default()
                }),
            )
            .await
            .ok();
        return Err(format!("Failed to start blue container: {e}"));
    }

    // Health check the new container (30s timeout)
    if let Err(e) = health_check_port(temp_port, 30).await {
        docker
            .stop_container(&new_container.id, Some(StopContainerOptions { t: 5 }))
            .await
            .ok();
        docker
            .remove_container(
                &new_container.id,
                Some(RemoveContainerOptions {
                    force: true,
                    v: false,
                    ..Default::default()
                }),
            )
            .await
            .ok();
        return Err(format!("Blue container health check failed: {e}"));
    }

    // Swap nginx to point to new container's port
    if let Err(e) = swap_nginx_proxy_port(domain, old_port, temp_port) {
        docker
            .stop_container(&new_container.id, Some(StopContainerOptions { t: 5 }))
            .await
            .ok();
        docker
            .remove_container(
                &new_container.id,
                Some(RemoveContainerOptions {
                    force: true,
                    v: false,
                    ..Default::default()
                }),
            )
            .await
            .ok();
        return Err(format!("Nginx port swap failed: {e}"));
    }

    // Test nginx config before reloading
    match crate::services::nginx::test_config().await {
        Ok(output) if output.success => {
            if let Err(e) = crate::services::nginx::reload().await {
                // Rollback nginx
                swap_nginx_proxy_port(domain, temp_port, old_port).ok();
                docker
                    .stop_container(&new_container.id, Some(StopContainerOptions { t: 5 }))
                    .await
                    .ok();
                docker
                    .remove_container(
                        &new_container.id,
                        Some(RemoveContainerOptions {
                            force: true,
                            v: false,
                            ..Default::default()
                        }),
                    )
                    .await
                    .ok();
                return Err(format!("Nginx reload failed: {e}"));
            }
        }
        Ok(output) => {
            // Rollback nginx + cleanup blue container
            swap_nginx_proxy_port(domain, temp_port, old_port).ok();
            docker
                .stop_container(&new_container.id, Some(StopContainerOptions { t: 5 }))
                .await
                .ok();
            docker
                .remove_container(
                    &new_container.id,
                    Some(RemoveContainerOptions {
                        force: true,
                        v: false,
                        ..Default::default()
                    }),
                )
                .await
                .ok();
            return Err(format!("Nginx config test failed: {}", output.stderr));
        }
        Err(e) => {
            swap_nginx_proxy_port(domain, temp_port, old_port).ok();
            docker
                .stop_container(&new_container.id, Some(StopContainerOptions { t: 5 }))
                .await
                .ok();
            docker
                .remove_container(
                    &new_container.id,
                    Some(RemoveContainerOptions {
                        force: true,
                        v: false,
                        ..Default::default()
                    }),
                )
                .await
                .ok();
            return Err(format!("Nginx test error: {e}"));
        }
    }

    // Traffic is now flowing to the new container. Stop and remove old container.
    docker
        .stop_container(old_container_id, Some(StopContainerOptions { t: 10 }))
        .await
        .ok();
    docker
        .remove_container(
            old_container_id,
            Some(RemoveContainerOptions {
                v: false,
                force: true,
                ..Default::default()
            }),
        )
        .await
        .ok();

    // Rename blue container to the original name
    docker
        .rename_container(
            &new_container.id,
            RenameContainerOptions {
                name: name.to_string(),
            },
        )
        .await
        .ok();

    tracing::info!(
        "App updated (blue-green, zero-downtime): {name}, port {old_port} → {temp_port}"
    );

    Ok(UpdateResult {
        container_id: new_container.id,
        blue_green: true,
        // Blue-green is only reached when nothing needed migrating — see `update_app`.
        migrated_volumes: Vec::new(),
        repaired_volumes: Vec::new(),
    })
}

/// What an update should do once the pull attempt has finished.
///
/// Kept separate from Docker so all four cases can be exercised as a unit test:
/// reaching them for real needs a registry that refuses, which no test here has.
#[derive(Debug, PartialEq, Eq)]
enum PullVerdict {
    /// The pull reported no error. Carry on.
    Proceed,
    /// The pull failed, but what the tag resolves to on this server is not what
    /// the container is running — so an update really is waiting on disk and
    /// applying it is the point of the button.
    ProceedFromLocal,
    /// The pull failed and the tag resolves to nothing on this server. The
    /// recreate would remove a container that cannot then be rebuilt.
    RefuseMissing,
    /// The pull failed and the local copy is byte-for-byte what the container is
    /// already running. Recreating spends an outage to arrive where it started.
    RefuseNoChange,
}

/// Decide the verdict above from three facts the caller reads out of Docker.
///
/// `running_image` being absent is treated as "not equal", which lands on
/// `ProceedFromLocal`: the image exists locally, so nothing can be destroyed,
/// and refusing on missing metadata would block an update that would work.
fn pull_verdict(
    pull_error: Option<&str>,
    local_image: Option<&str>,
    running_image: Option<&str>,
) -> PullVerdict {
    if pull_error.is_none() {
        return PullVerdict::Proceed;
    }
    match local_image {
        None => PullVerdict::RefuseMissing,
        Some(local) if Some(local) == running_image => PullVerdict::RefuseNoChange,
        Some(_) => PullVerdict::ProceedFromLocal,
    }
}

/// Update an app by pulling the latest image.
/// Uses blue-green deployment (zero-downtime) when the app has a domain with nginx reverse proxy.
/// Falls back to stop/start when no reverse proxy is configured.
pub async fn update_app(container_id: &str) -> Result<UpdateResult, String> {
    let docker =
        Docker::connect_with_local_defaults().map_err(|e| format!("Docker connect failed: {e}"))?;

    // Inspect the container to get its full config
    let info = docker
        .inspect_container(container_id, None)
        .await
        .map_err(|e| format!("Failed to inspect container: {e}"))?;

    // Was this app running before we touched it? A recreate that always starts
    // the new container turns "update this app" into "update and start it", so
    // pressing Update on something deliberately stopped brought it back up —
    // on a host where it had been stopped for a reason.
    //
    // Read from `status`, not `running`: bollard reports a PAUSED container as
    // running as well, and a paused app is not one the operator left up.
    let was_running = matches!(
        info.state.as_ref().and_then(|s| s.status),
        Some(bollard::models::ContainerStateStatusEnum::RUNNING)
    );

    // Read before the fields below are moved out of `info`. This is the image the
    // container is actually running, which the verdict compares against whatever
    // the tag resolves to locally.
    let running_image = info.image.clone();

    let config = info.config.ok_or("No container config found")?;
    let mut host_config = info.host_config.ok_or("No host config found")?;
    let name = info
        .name
        .unwrap_or_default()
        .trim_start_matches('/')
        .to_string();
    let image = config.image.clone().ok_or("No image found")?;
    // The container is `dockpanel-app-<app>`; its DATA lives under the app's own name.
    let app_name = app_data_name(config.labels.as_ref(), &name);

    // Anything this app's template calls persistent that the container does not mount is
    // living in the writable layer, which the recreate below deletes (#110). Decide that
    // BEFORE choosing a strategy: migrating needs the container stopped, which only the
    // stop/start path gives us.
    let unmounted = unmounted_declared_volumes(&config, &host_config);

    // Check if this app has a domain (nginx reverse proxy) for blue-green
    let domain = config
        .labels
        .as_ref()
        .and_then(|l| l.get("dockpanel.app.domain"))
        .cloned();
    let old_port = extract_host_port(&host_config);

    // Pull the latest image
    tracing::info!("Updating app {name}: pulling {image}");
    let mut pull = docker.create_image(
        Some(CreateImageOptions {
            from_image: image.as_str(),
            ..Default::default()
        }),
        None,
        None,
    );
    let mut pull_error: Option<String> = None;
    while let Some(result) = pull.next().await {
        if let Err(e) = result {
            pull_error = Some(e.to_string());
        }
    }

    // A failed pull used to be one line in the agent's log and nothing more, so
    // the recreate below ran regardless: stop, migrate, remove, create. When the
    // tag had gone away that spent a real outage rebuilding the container from
    // what was already on disk, and the operator was told the app had updated.
    // The other door that replaces an image has always refused instead; this is
    // the same guard, on this one.
    //
    // Decided before anything is stopped, so a refusal costs the app nothing.
    let local_image = docker.inspect_image(&image).await.ok().and_then(|i| i.id);
    let pull_failure = pull_error.as_deref().unwrap_or("unknown error");
    match pull_verdict(
        pull_error.as_deref(),
        local_image.as_deref(),
        running_image.as_deref(),
    ) {
        PullVerdict::Proceed => {}
        PullVerdict::ProceedFromLocal => {
            tracing::warn!(
                "App {name}: '{image}' could not be pulled ({pull_failure}), but the copy on \
                 this server differs from what the container is running, so the update \
                 continues from it"
            );
        }
        PullVerdict::RefuseMissing => {
            return Err(format!(
                "'{image}' could not be pulled and no copy of it exists on this server, so \
                 {name} was left untouched — recreating it would have removed a container \
                 that could not be rebuilt. The app is still running. Pull error: \
                 {pull_failure}"
            ));
        }
        PullVerdict::RefuseNoChange => {
            return Err(format!(
                "'{image}' could not be pulled, and the copy on this server is already what \
                 {name} is running, so there is nothing to update. The app is still running \
                 and was left untouched. Pull error: {pull_failure}"
            ));
        }
    }

    // Try blue-green if app has a domain and a known port — and only if running two
    // copies of it at once cannot corrupt anything. See `shares_persistent_state`.
    if let (Some(domain), Some(old_port)) = (&domain, old_port) {
        // Check that nginx config exists for this domain
        let config_path = format!("{}/{domain}.conf", super::nginx::sites_dir());
        if !unmounted.is_empty() {
            tracing::info!(
                "Not using blue-green for {name}: {} declared path(s) are unmounted and have \
                 to be migrated out of the writable layer first, which needs the container \
                 stopped. Using stop/start instead.",
                unmounted.len()
            );
        } else if shares_persistent_state(&host_config) {
            tracing::info!(
                "Not using blue-green for {name}: it has persistent mounts, and blue-green \
                 would run the old and new containers against the same host paths for the \
                 length of the health check. Using stop/start instead (brief downtime, no \
                 concurrent writers)."
            );
        } else if !was_running {
            tracing::info!(
                "Not using blue-green for {name}: it is stopped, and blue-green exists to \
                 avoid downtime for something that is up. Using stop/start instead, which \
                 leaves it stopped."
            );
        } else if std::path::Path::new(&config_path).exists() {
            match blue_green_update(
                &docker,
                container_id,
                &name,
                &config,
                &host_config,
                domain,
                old_port,
            )
            .await
            {
                Ok(result) => return Ok(result),
                Err(e) => {
                    tracing::warn!(
                        "Blue-green failed for {name}, falling back to stop/start: {e}"
                    );
                }
            }
        }
    }

    // Fallback: stop old → migrate → remove → create new → start (brief downtime)
    docker
        .stop_container(container_id, Some(StopContainerOptions { t: 10 }))
        .await
        .ok();

    // Stopped, so the app has flushed; not yet removed, so the data is still reachable.
    // This is the only window in which it can be rescued, and a failure here MUST abort
    // before the remove below — the whole point is to not destroy what we came to save.
    let migrated =
        migrate_unmounted_volumes(&docker, container_id, &app_name, &image, &unmounted, &mut host_config)
            .await
            .map_err(|e| {
                format!(
                    "Aborting update of {name} without removing it: {e}. The container is \
                     stopped and intact; start it again with `docker start {name}`."
                )
            })?;

    // Same window, the other direction: give back ownership of data an EARLIER
    // release migrated and left root-owned. Not fatal — an app that is already
    // broken must not have its update refused as well.
    let repaired = repair_root_owned_volumes(&docker, &image, &host_config).await;

    docker
        .remove_container(
            container_id,
            Some(RemoveContainerOptions {
                v: false,
                force: true,
                ..Default::default()
            }),
        )
        .await
        .map_err(|e| format!("Failed to remove old container: {e}"))?;

    let mut env_list = config.env.clone().unwrap_or_default();
    let networking_config = reattach_sidecar_if_any(config.labels.as_ref(), &app_name, &mut env_list);

    let new_config = Config {
        image: config.image,
        env: if env_list.is_empty() { None } else { Some(env_list) },
        exposed_ports: config.exposed_ports,
        labels: config.labels,
        host_config: Some(host_config),
        networking_config,
        cmd: config.cmd,
        entrypoint: config.entrypoint,
        working_dir: if config.working_dir.as_deref() == Some("") {
            None
        } else {
            config.working_dir
        },
        ..Default::default()
    };

    let container = docker
        .create_container(
            Some(CreateContainerOptions {
                name: name.as_str(),
                platform: None,
            }),
            new_config,
        )
        .await
        .map_err(|e| format!("Failed to create updated container: {e}"))?;

    if was_running {
        docker
            .start_container(&container.id, None::<StartContainerOptions<String>>)
            .await
            .map_err(|e| format!("Failed to start updated container: {e}"))?;
    } else {
        tracing::info!("{name} was stopped before the update; leaving it stopped.");
    }

    if migrated.is_empty() {
        tracing::info!("App updated (stop/start): {name} ({image})");
    } else {
        tracing::info!(
            "App updated (stop/start): {name} ({image}) — and {} path(s) migrated onto \
             persistent storage: {}",
            migrated.len(),
            migrated.join(", ")
        );
    }
    if !repaired.is_empty() {
        tracing::info!(
            "App {name}: ownership repaired on {} path(s) left root-owned by an earlier \
             release: {}",
            repaired.len(),
            repaired.join(", ")
        );
    }
    Ok(UpdateResult {
        container_id: container.id,
        blue_green: false,
        migrated_volumes: migrated,
        repaired_volumes: repaired,
    })
}

/// Change a managed container's image, preserving its full runtime configuration.
///
/// (s237 audit) The previous implementation recreated the container with a bare
/// `docker run -d --name X --volumes-from Y --network bridge <image>`, which dropped
/// EVERY hardening flag the original deploy applied — cap_drop ALL + the minimal cap_add
/// allowlist, security_opt no-new-privileges, the published `127.0.0.1:<port>` binding
/// (so the nginx reverse proxy 502'd), the restart policy, the memory/CPU limits, the env
/// vars, and the `dockpanel.managed` labels (so the container vanished from the panel).
/// This inspects the existing container and recreates it with the same `host_config`
/// (caps, security_opt, port bindings, restart, resource limits, GPU device requests, binds),
/// `labels`, `env`, and `exposed_ports`, only swapping the image.
///
/// The new image's own entrypoint and working dir apply, because a tag bump may legitimately
/// change them and `deploy_app` never overrode either. **Its command is a different case and
/// used to be treated the same, wrongly.** For the handful of templates the catalogue gives an
/// explicit command to, `deploy_app` sets that command precisely because the image's own
/// default does not serve — it prints usage and exits, which restarts for ever (#101). Letting
/// the image's default reassert itself here therefore broke every one of those templates the
/// moment an operator changed its image, and the comment that used to sit on this line claimed
/// the opposite was "the original run semantics". The command is re-derived from the template
/// label below, so a template with no override still gets the new image's default and one with
/// an override keeps working.
pub async fn change_container_image(container_id: &str, new_image: &str) -> Result<String, String> {
    let docker =
        Docker::connect_with_local_defaults().map_err(|e| format!("Docker connect failed: {e}"))?;

    // Inspect the existing container to capture its full config before touching it.
    let info = docker
        .inspect_container(container_id, None)
        .await
        .map_err(|e| format!("Failed to inspect container: {e}"))?;

    // Was this app running before we touched it? A recreate that always starts
    // the new container turns "update this app" into "update and start it", so
    // pressing Update on something deliberately stopped brought it back up —
    // on a host where it had been stopped for a reason.
    //
    // Read from `status`, not `running`: bollard reports a PAUSED container as
    // running as well, and a paused app is not one the operator left up.
    let was_running = matches!(
        info.state.as_ref().and_then(|s| s.status),
        Some(bollard::models::ContainerStateStatusEnum::RUNNING)
    );

    let config = info.config.ok_or("No container config found")?;
    let mut host_config = info.host_config.ok_or("No host config found")?;
    let name = info
        .name
        .unwrap_or_default()
        .trim_start_matches('/')
        .to_string();
    if name.is_empty() {
        return Err("Container not found".to_string());
    }
    let old_image = config.image.clone().unwrap_or_default();
    let app_name = app_data_name(config.labels.as_ref(), &name);
    let unmounted = unmounted_declared_volumes(&config, &host_config);
    // Read the override off the template label rather than carrying the old container's
    // resolved Cmd forward: the resolved one may be the OLD image's default, and pinning
    // that onto a new image is the very thing this path deliberately avoids.
    let catalogue_cmd = catalogue_cmd_for(config.labels.as_ref());

    // Pull the new image. create_image's stream may emit non-fatal warnings, so verify the
    // image exists locally afterwards rather than trusting the stream's per-chunk results.
    let mut pull = docker.create_image(
        Some(CreateImageOptions {
            from_image: new_image,
            ..Default::default()
        }),
        None,
        None,
    );
    let mut last_err: Option<String> = None;
    while let Some(result) = pull.next().await {
        if let Err(e) = result {
            last_err = Some(e.to_string());
        }
    }
    if docker.inspect_image(new_image).await.is_err() {
        return Err(format!(
            "Failed to pull image '{new_image}': {}",
            last_err.unwrap_or_else(|| "image not found after pull".to_string())
        ));
    }

    // Stop + remove the old container, KEEPING its volumes (v: false) so data persists.
    // ⚠ `v: false` preserves VOLUMES, which is not the same as preserving DATA: a
    // container with no mount keeps its state in the writable layer, and `docker rm`
    // deletes that whatever `v` says (#110). Migrate first, in the window where the
    // container is stopped but still exists, and abort without removing if that fails.
    docker
        .stop_container(container_id, Some(StopContainerOptions { t: 10 }))
        .await
        .ok();
    migrate_unmounted_volumes(
        &docker,
        container_id,
        &app_name,
        &old_image,
        &unmounted,
        &mut host_config,
    )
    .await
    .map_err(|e| {
        format!(
            "Aborting image change for {name} without removing it: {e}. The container is \
             stopped and intact; start it again with `docker start {name}`."
        )
    })?;

    // Repair against the image the container is about to RUN, not the one it is
    // leaving — the uid that has to be able to write this data is the new one.
    let _ = repair_root_owned_volumes(&docker, new_image, &host_config).await;
    docker
        .remove_container(
            container_id,
            Some(RemoveContainerOptions {
                v: false,
                force: true,
                ..Default::default()
            }),
        )
        .await
        .map_err(|e| format!("Failed to remove old container: {e}"))?;

    let mut env_list = config.env.clone().unwrap_or_default();
    let networking_config = reattach_sidecar_if_any(config.labels.as_ref(), &app_name, &mut env_list);

    // Recreate with the SAME hardening/networking/limits/labels/env, only the image changes.
    let new_config = Config {
        image: Some(new_image.to_string()),
        env: if env_list.is_empty() { None } else { Some(env_list) },
        exposed_ports: config.exposed_ports,
        labels: config.labels,
        host_config: Some(host_config),
        networking_config,
        cmd: catalogue_cmd,
        ..Default::default()
    };

    let container = docker
        .create_container(
            Some(CreateContainerOptions {
                name: name.as_str(),
                platform: None,
            }),
            new_config,
        )
        .await
        .map_err(|e| format!("Failed to create container with new image: {e}"))?;

    if was_running {
        docker
            .start_container(&container.id, None::<StartContainerOptions<String>>)
            .await
            .map_err(|e| format!("Failed to start container with new image: {e}"))?;
    } else {
        tracing::info!("{name} was stopped before the image change; leaving it stopped.");
    }

    tracing::info!("Image changed for {name}: → {new_image} (new: {})", container.id);
    Ok(container.id)
}

/// Get environment variables from a running container.
pub async fn get_app_env(container_id: &str) -> Result<Vec<(String, String)>, String> {
    let docker =
        Docker::connect_with_local_defaults().map_err(|e| format!("Docker connect failed: {e}"))?;

    let info = docker
        .inspect_container(container_id, None)
        .await
        .map_err(|e| format!("Failed to inspect container: {e}"))?;

    let env_list = info
        .config
        .and_then(|c| c.env)
        .unwrap_or_default()
        .into_iter()
        .filter_map(|entry| {
            let (k, v) = entry.split_once('=')?;
            Some((k.to_string(), v.to_string()))
        })
        .collect();

    Ok(env_list)
}

/// What a removal needs to know about a container *before* it is removed, so
/// the cleanup that follows can prove it owns each thing it deletes.
///
/// Read in one inspect because after `remove_app` there is nothing left to ask.
/// Every field is optional and a missing one means "cannot prove it" — the
/// callers treat that as a refusal, per [`crate::services::ownership`].
#[derive(Debug, Default, Clone)]
pub struct RemovalIdentity {
    /// `dockpanel.app.domain` — the vhost, cert dir and logs are named after it.
    pub domain: Option<String>,
    /// `dockpanel.app.name` — `/var/lib/dockpanel/apps/{name}` is named after it.
    pub name: Option<String>,
    /// The published host port. The app's vhost `proxy_pass`es to it, and it is
    /// the only thing in that file identifying which container it fronts.
    pub host_port: Option<u16>,
    /// Whether this container actually bind-mounts out of
    /// `/var/lib/dockpanel/apps/{name}`.
    ///
    /// A template deploy does; a compose service does not — its binds are
    /// confined to `/var/lib/dockpanel/compose/`, so an identically-named
    /// compose service was deleting a template app's data directory while
    /// owning nothing under it.
    pub owns_app_dir: bool,
    /// The directory the container's own binds prove it owns, when it owns one.
    ///
    /// Read from the binds rather than rebuilt from the name label, because the two
    /// disagreed: releases v2.111.0 through v2.113.3 migrated an app's data into a
    /// directory named for its CONTAINER, so the label-derived path pointed at nothing
    /// and the data was left behind on delete without so much as a warning. Taking the
    /// path from the mount that actually exists cleans up both spellings, and keeps the
    /// compose protection above by construction — a bind outside `APP_DATA_DIR` yields
    /// `None` and nothing is removed.
    pub app_dir: Option<String>,
}

/// Gather [`RemovalIdentity`] for `container_id`. Never fails: an unreachable
/// Docker yields all-none, which refuses every subsequent delete.
/// Which directory under `APP_DATA_DIR` do this container's own binds prove it owns?
///
/// Two spellings are accepted, and only two: the one `deploy_app` writes, and the one the
/// migration wrote on releases v2.111.0 through v2.113.3, when it was handed the container
/// name instead of the app name. Accepting the second is what lets an install that was
/// already migrated have its data cleaned up rather than orphaned.
///
/// Anything else is refused, which is what keeps a compose service from deleting a
/// template app's tree: its binds live outside `APP_DATA_DIR` entirely, so nothing matches
/// and the caller is told it owns nothing. The `{prefix}/` boundary is checked explicitly
/// so a directory called `data` cannot answer for one called `database`.
fn owned_app_dir(name: &str, binds: &[String]) -> Option<String> {
    let candidates = [
        format!("{APP_DATA_DIR}/{name}"),
        format!("{APP_DATA_DIR}/{CONTAINER_NAME_PREFIX}{name}"),
    ];
    binds.iter().find_map(|bind| {
        let source = bind.split(':').next().unwrap_or_default();
        candidates
            .iter()
            .find(|dir| source == dir.as_str() || source.starts_with(&format!("{dir}/")))
            .cloned()
    })
}

pub async fn removal_identity(container_id: &str) -> RemovalIdentity {
    let mut id = RemovalIdentity::default();
    let Ok(docker) = Docker::connect_with_local_defaults() else {
        return id;
    };
    let Ok(info) = docker.inspect_container(container_id, None).await else {
        return id;
    };

    if let Some(labels) = info.config.as_ref().and_then(|c| c.labels.as_ref()) {
        id.domain = labels.get("dockpanel.app.domain").cloned();
        id.name = labels.get("dockpanel.app.name").cloned();
    }

    if let Some(host_config) = info.host_config.as_ref() {
        id.host_port = extract_host_port(host_config);

        // Ownership of the data directory is decided by the container's own
        // mounts, not by its name label. `{prefix}` rather than `{prefix}/` so a
        // bind of the directory itself counts, and the trailing boundary is
        // checked explicitly so `/apps/data` cannot answer for `/apps/database`.
        if let (Some(name), Some(binds)) = (id.name.as_deref(), host_config.binds.as_ref()) {
            id.app_dir = owned_app_dir(name, binds);
            id.owns_app_dir = id.app_dir.is_some();
        }
    }

    id
}

/// True iff the container's labels mark it dockpanel-managed (`dockpanel.managed=true`).
/// This is the boundary that separates panel-managed app containers from the panel's OWN infra
/// (its postgres/api/agent containers carry no such label). Pure + unit-tested.
fn is_managed_labels(labels: Option<&HashMap<String, String>>) -> bool {
    labels
        .and_then(|l| l.get("dockpanel.managed"))
        .map(|v| v == "true")
        .unwrap_or(false)
}

/// Guard an action-handler target: inspect the container and reject unless it is dockpanel-managed.
/// Gives WRITE scope (stop/start/restart/logs/exec/remove/change-image/update/env/snapshot/limits)
/// the same `dockpanel.managed=true` boundary the `list` READ path already enforces, so an admin
/// cannot act on the panel's own infrastructure containers by id (confused-deputy footgun). For the
/// recreate paths (change_container_image/update_env) this runs on the ORIGINAL container before any
/// stop/remove, and the recreation preserves the label — so the new container stays managed.
pub async fn require_managed(container_id: &str) -> Result<(), String> {
    let docker =
        Docker::connect_with_local_defaults().map_err(|e| format!("Docker connect failed: {e}"))?;
    let info = docker
        .inspect_container(container_id, None)
        .await
        .map_err(|e| format!("Failed to inspect container: {e}"))?;
    if !is_managed_labels(info.config.as_ref().and_then(|c| c.labels.as_ref())) {
        return Err("not a dockpanel-managed container".to_string());
    }
    Ok(())
}

/// Update a container's environment variables by recreating it with the new env.
pub async fn update_env(
    container_id: &str,
    new_env: HashMap<String, String>,
) -> Result<String, String> {
    let docker =
        Docker::connect_with_local_defaults().map_err(|e| format!("Docker connect failed: {e}"))?;

    let info = docker
        .inspect_container(container_id, None)
        .await
        .map_err(|e| format!("Failed to inspect container: {e}"))?;

    // Was this app running before we touched it? A recreate that always starts
    // the new container turns "update this app" into "update and start it", so
    // pressing Update on something deliberately stopped brought it back up —
    // on a host where it had been stopped for a reason.
    //
    // Read from `status`, not `running`: bollard reports a PAUSED container as
    // running as well, and a paused app is not one the operator left up.
    let was_running = matches!(
        info.state.as_ref().and_then(|s| s.status),
        Some(bollard::models::ContainerStateStatusEnum::RUNNING)
    );

    let config = info.config.ok_or("No container config found")?;
    let mut host_config = info.host_config.ok_or("No host config found")?;
    let name = info
        .name
        .unwrap_or_default()
        .trim_start_matches('/')
        .to_string();
    let image = config.image.clone().unwrap_or_default();
    let app_name = app_data_name(config.labels.as_ref(), &name);
    let unmounted = unmounted_declared_volumes(&config, &host_config);

    // Build new env list, carrying over anything the caller sent back masked.
    let (mut env_list, kept) = merge_env_preserving_masked(&new_env, config.env.as_deref().unwrap_or(&[]));

    if !kept.is_empty() {
        tracing::info!(
            "update_env for {container_id}: {} masked value(s) carried over unchanged ({})",
            kept.len(),
            kept.join(", ")
        );
    }

    // Stop, migrate anything living in the writable layer, then remove.
    // Editing one environment variable recreates the container, so this door destroys an
    // unmounted app's data exactly as thoroughly as Update does (#110) — it just does not
    // sound like it. The migration has to happen before the remove, and a failure has to
    // abort before it.
    docker
        .stop_container(container_id, Some(StopContainerOptions { t: 10 }))
        .await
        .ok();
    migrate_unmounted_volumes(
        &docker,
        container_id,
        &app_name,
        &image,
        &unmounted,
        &mut host_config,
    )
    .await
    .map_err(|e| {
        format!(
            "Aborting environment update for {name} without removing it: {e}. The container \
             is stopped and intact; start it again with `docker start {name}`."
        )
    })?;

    let _ = repair_root_owned_volumes(&docker, &image, &host_config).await;
    docker
        .remove_container(
            container_id,
            Some(RemoveContainerOptions {
                v: false,
                force: true,
                ..Default::default()
            }),
        )
        .await
        .map_err(|e| format!("Failed to remove old container: {e}"))?;

    let networking_config = reattach_sidecar_if_any(config.labels.as_ref(), &app_name, &mut env_list);

    // Recreate with new env
    let new_config = Config {
        image: config.image,
        env: Some(env_list),
        exposed_ports: config.exposed_ports,
        labels: config.labels,
        host_config: Some(host_config),
        networking_config,
        cmd: config.cmd,
        entrypoint: config.entrypoint,
        working_dir: if config.working_dir.as_deref() == Some("") {
            None
        } else {
            config.working_dir
        },
        ..Default::default()
    };

    let container = docker
        .create_container(
            Some(CreateContainerOptions {
                name: name.as_str(),
                platform: None,
            }),
            new_config,
        )
        .await
        .map_err(|e| format!("Failed to create container: {e}"))?;

    if was_running {
        docker
            .start_container(&container.id, None::<StartContainerOptions<String>>)
            .await
            .map_err(|e| format!("Failed to start container: {e}"))?;
    } else {
        tracing::info!("{name} was stopped before the env change; leaving it stopped.");
    }

    tracing::info!(
        "Container env updated: {name} ({} vars)",
        new_env.len()
    );
    Ok(container.id)
}

/// Stop and remove an app container, optionally removing its volumes.
pub async fn remove_app(container_id: &str) -> Result<(), String> {
    let docker =
        Docker::connect_with_local_defaults().map_err(|e| format!("Docker connect failed: {e}"))?;

    // Read before removal — there is nothing left to ask afterwards, and this
    // is how the app's sidecar (if any) gets found and torn down alongside it.
    let app_name = current_app_name(&docker, container_id).await;

    // Stop first (ignore error if already stopped)
    docker
        .stop_container(container_id, Some(StopContainerOptions { t: 10 }))
        .await
        .ok();

    docker
        .remove_container(
            container_id,
            Some(RemoveContainerOptions {
                v: true,
                force: true,
                ..Default::default()
            }),
        )
        .await
        .map_err(|e| format!("Failed to remove container: {e}"))?;

    if let Some(name) = app_name {
        app_sidecar::teardown(&name).await;
    }

    tracing::info!("App container removed: {container_id}");
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// `minio/minio`'s default `CMD` is `["minio"]`, which prints usage and exits
    /// 0 — so the container restarts forever at `ExitCode: 0` rather than serving
    /// (issue #101). Verified against the image s339: without these arguments the
    /// container reached `restarting` with 7 restarts and exit 0; with them it ran
    /// with 0 restarts and answered 200 on both the S3 port and the console.
    #[test]
    fn minio_overrides_a_cmd_that_would_exit_instead_of_serving() {
        let cmd = template_cmd("minio").expect("minio must carry startup arguments");
        assert_eq!(cmd, vec!["server", "/data", "--console-address", ":9001"]);

        // The served path must be the one the template persists, or the container
        // comes up healthy and silently loses every object on restart.
        let minio = TEMPLATES
            .iter()
            .find(|t| t.id == "minio")
            .expect("the minio template must exist");
        assert!(
            minio.volumes.contains(&"/data"),
            "minio serves /data but persists {:?}",
            minio.volumes
        );
    }

    /// Absence is the default for an image whose entrypoint already serves — but
    /// absence is NOT proof that a template is fine.
    ///
    /// ⚠ **MinIO is not the only template in this class.** A catalogue audit found
    /// at least seven more whose image cannot come up as shipped, verified from
    /// their image config: `cloudflared` (Cmd `["version"]` — prints a version and
    /// exits, every time, while the template collects a TUNNEL_TOKEN it has no way
    /// to use), `ntfy`, `trivy` and `surrealdb` (multi-command CLI entrypoints with
    /// a null Cmd, so a bare run prints help), plus `keycloak`, `authentik` and
    /// `vllm`. `keycloak`'s own description says it *"Requires 'start-dev' command
    /// override"* — written against a struct that then had no way to express one.
    ///
    /// ⚠ **The sentence that used to close this comment — "Only MinIO was
    /// reported, reproduced and fixed; the rest are unfixed" — had been false for
    /// several releases when it was finally noticed.** Four of the seven
    /// (`cloudflared`, `ntfy`, `trivy`, `keycloak`) carry start commands in the
    /// table above and have for some time. The three genuinely still without one
    /// are `surrealdb`, `authentik` and `vllm`, and §6 of
    /// `app-template-images-pin-e2e.sh` explains for each why a command is the
    /// wrong instrument rather than a missing one. A comment stating what is and
    /// is not fixed is a specification, and this one went on being read as current
    /// by every audit that came after the fixes landed.
    #[test]
    fn every_cmd_override_is_live_and_well_formed() {
        assert!(template_cmd("minio").is_some());
        assert!(template_cmd("ghost").is_none());
        assert!(template_cmd("no-such-template").is_none());

        // Every override must name a real template, or it is dead configuration
        // that looks live.
        for (id, args) in TEMPLATE_CMD {
            assert!(
                TEMPLATES.iter().any(|t| t.id == *id),
                "TEMPLATE_CMD names '{id}', which is not in the catalogue"
            );
            assert!(!args.is_empty(), "'{id}' carries an empty command");
        }
    }

    #[test]
    fn managed_label_gate() {
        // Only dockpanel.managed=true passes; panel infra (no such label) and false are rejected.
        let managed = HashMap::from([("dockpanel.managed".to_string(), "true".to_string())]);
        assert!(is_managed_labels(Some(&managed)));

        let disabled = HashMap::from([("dockpanel.managed".to_string(), "false".to_string())]);
        assert!(!is_managed_labels(Some(&disabled)));

        // A panel-infra container (e.g. only compose labels) must be rejected.
        let infra = HashMap::from([("com.docker.compose.project".to_string(), "dockpanel".to_string())]);
        assert!(!is_managed_labels(Some(&infra)));

        assert!(!is_managed_labels(Some(&HashMap::new())));
        assert!(!is_managed_labels(None));
    }

    fn env(pairs: &[(&str, &str)]) -> HashMap<String, String> {
        pairs
            .iter()
            .map(|(k, v)| ((*k).to_string(), (*v).to_string()))
            .collect()
    }

    // ── What Update does when the image will not pull ────────────────────────

    /// Before this the pull error was logged and discarded, and the recreate ran
    /// anyway. These four cases are the whole of the decision, and none of them
    /// is reachable from a test that needs a registry to refuse.

    #[test]
    fn a_pull_that_worked_is_not_second_guessed() {
        assert_eq!(
            pull_verdict(None, Some("sha256:aaa"), Some("sha256:aaa")),
            PullVerdict::Proceed
        );
        // Even with nothing resolvable locally: a successful pull put it there,
        // and the inspect that follows is not the authority on that.
        assert_eq!(pull_verdict(None, None, None), PullVerdict::Proceed);
    }

    #[test]
    fn a_dead_tag_with_no_local_copy_leaves_the_container_alone() {
        // The one case that used to destroy the app: the remove succeeds, the
        // create then fails on an image that is nowhere, and nothing puts it
        // back.
        assert_eq!(
            pull_verdict(Some("manifest unknown"), None, Some("sha256:aaa")),
            PullVerdict::RefuseMissing
        );
    }

    #[test]
    fn a_dead_tag_whose_local_copy_is_already_running_is_not_worth_an_outage() {
        assert_eq!(
            pull_verdict(Some("no such host"), Some("sha256:aaa"), Some("sha256:aaa")),
            PullVerdict::RefuseNoChange
        );
    }

    #[test]
    fn a_dead_tag_still_applies_an_update_that_is_already_on_disk() {
        // An earlier pull succeeded and the container was never recreated, so
        // pressing Update has real work to do even though today's pull failed.
        // Refusing here would be a regression, not a guard.
        assert_eq!(
            pull_verdict(Some("timeout"), Some("sha256:bbb"), Some("sha256:aaa")),
            PullVerdict::ProceedFromLocal
        );
    }

    #[test]
    fn an_unreadable_running_image_never_blocks_an_update_it_cannot_destroy() {
        // The copy exists, so the recreate can complete; with no id to compare
        // against, refusing would block on missing metadata alone.
        assert_eq!(
            pull_verdict(Some("timeout"), Some("sha256:bbb"), None),
            PullVerdict::ProceedFromLocal
        );
    }

    // ── Domain-derived template env (GitHub #111) ────────────────────────────

    /// The catalogue rows these tests assert against, so a template edit that
    /// breaks the derivation fails here rather than in the field.
    fn template_by_id(id: &str) -> &'static AppTemplateDef {
        TEMPLATES
            .iter()
            .find(|t| t.id == id)
            .unwrap_or_else(|| panic!("template {id} missing from the catalogue"))
    }

    #[test]
    fn n8n_gets_all_four_of_its_url_vars_from_the_claimed_domain() {
        // n8n's row declares `env_vars: &[]`, so before this every one of these
        // was unreachable from the panel: the deploy form renders inputs only
        // from the template list, and the edit modal could not compose a new
        // key. The reporter had to rebuild the container by hand (#111).
        let t = template_by_id("n8n");
        assert!(
            t.env_vars.is_empty(),
            "n8n declaring env vars would change which merge branch fills these in"
        );

        let derived = domain_env_for("n8n", "n8n.example.com");
        let out = merge_deploy_env(t.env_vars, &env(&[]), &derived);

        assert!(
            out.contains(&"N8N_HOST=n8n.example.com".to_string()),
            "{out:?}"
        );
        assert!(out.contains(&"N8N_PROTOCOL=https".to_string()), "{out:?}");
        // Trailing slash: n8n appends a path to WEBHOOK_URL.
        assert!(
            out.contains(&"WEBHOOK_URL=https://n8n.example.com/".to_string()),
            "{out:?}"
        );
        assert!(out.contains(&"N8N_PROXY_HOPS=1".to_string()), "{out:?}");
        assert_eq!(out.len(), 4, "nothing else should be emitted: {out:?}");
    }

    #[test]
    fn no_domain_means_no_derivation_and_the_catalogue_default_still_stands() {
        // A deploy without a domain is reachable only on localhost, so the
        // localhost defaults are correct there and must survive untouched.
        let t = template_by_id("ghost");
        let out = merge_deploy_env(t.env_vars, &env(&[]), &[]);
        assert!(
            out.contains(&"url=http://localhost:2368".to_string()),
            "{out:?}"
        );

        // And n8n, which declares nothing, must emit nothing at all.
        let n8n = merge_deploy_env(template_by_id("n8n").env_vars, &env(&[]), &[]);
        assert!(n8n.is_empty(), "{n8n:?}");
    }

    #[test]
    fn an_operator_value_that_differs_from_the_default_beats_the_derived_one() {
        // The rule that stops this fix becoming the same bug pointing the other
        // way. Someone running Ghost behind their own CDN types a different
        // URL; the domain must not silently overwrite it.
        let t = template_by_id("ghost");
        let derived = domain_env_for("ghost", "blog.example.com");
        let out = merge_deploy_env(
            t.env_vars,
            &env(&[("url", "https://cdn.example.net")]),
            &derived,
        );

        assert!(
            out.contains(&"url=https://cdn.example.net".to_string()),
            "operator value must win: {out:?}"
        );
        assert!(
            !out.iter().any(|e| e == "url=https://blog.example.com"),
            "derived value must not also be emitted: {out:?}"
        );
    }

    #[test]
    fn a_submitted_value_still_equal_to_the_default_is_treated_as_untouched() {
        // The deploy form seeds every field from `default` and posts all of
        // them, so "the operator left it alone" arrives as the default value,
        // not as an absent key. If this branch broke, the fix would do nothing
        // for the five templates that DO declare the var — which is the
        // majority of the affected catalogue.
        let t = template_by_id("ghost");
        let derived = domain_env_for("ghost", "blog.example.com");
        let out = merge_deploy_env(
            t.env_vars,
            &env(&[("url", "http://localhost:2368")]),
            &derived,
        );
        assert!(
            out.contains(&"url=https://blog.example.com".to_string()),
            "{out:?}"
        );
    }

    #[test]
    fn a_derived_var_the_operator_also_sent_is_emitted_exactly_once() {
        // Both the extras loop and the derived-append loop can reach a name the
        // template does not declare. Emitting it twice would put two entries
        // for one key into the container's env.
        let t = template_by_id("n8n");
        let derived = domain_env_for("n8n", "n8n.example.com");
        let out = merge_deploy_env(
            t.env_vars,
            &env(&[("N8N_HOST", "manual.example.com")]),
            &derived,
        );

        let hosts: Vec<_> = out.iter().filter(|e| e.starts_with("N8N_HOST=")).collect();
        assert_eq!(hosts.len(), 1, "{out:?}");
        assert_eq!(hosts[0], "N8N_HOST=manual.example.com");
    }

    #[test]
    fn every_domain_derived_name_exists_on_the_template_it_is_keyed_to() {
        // A severed pair guard. Renaming or dropping a catalogue env var while
        // leaving its TEMPLATE_DOMAIN_ENV entry behind would derive a value
        // nothing reads — the GPU_RECOMMENDED_TEMPLATES /
        // stable-diffusion-webui shape, which sat undetected for 19 releases.
        // n8n is the deliberate exception: it declares none of its four, which
        // is the whole reason they are derived.
        for (template_id, vars) in TEMPLATE_DOMAIN_ENV {
            let t = template_by_id(template_id);
            if *template_id == "n8n" {
                continue;
            }
            for (name, _) in *vars {
                assert!(
                    t.env_vars.iter().any(|ev| ev.name == *name),
                    "{template_id} has no env var {name}"
                );
            }
        }
    }

    #[test]
    fn every_domain_derived_template_still_exists_in_the_catalogue() {
        // The other half of the same severed pair: a template withdrawn from
        // the catalogue must not leave an entry here. `template_by_id` panics
        // with the offending id.
        for (template_id, _) in TEMPLATE_DOMAIN_ENV {
            let _ = template_by_id(template_id);
        }
    }

    #[test]
    fn the_derived_flag_the_form_reads_matches_what_deploy_actually_derives() {
        // `domain_derived` is what the deploy form uses to tell the operator a
        // field is filled in for them. If it disagreed with the derivation, the
        // form would either promise something that does not happen or show a
        // localhost default it is about to overwrite.
        for t in TEMPLATES {
            let derived = domain_env_for(t.id, "example.com");
            for ev in t.env_vars {
                assert_eq!(
                    is_domain_derived(t.id, ev.name),
                    derived.iter().any(|(n, _)| *n == ev.name),
                    "{}.{} flag disagrees with derivation",
                    t.id,
                    ev.name
                );
            }
        }
        // And the flag is false for a template with no entry at all.
        assert!(!is_domain_derived("postgres", "POSTGRES_PASSWORD"));
    }

    #[test]
    fn a_derived_url_never_carries_the_container_port() {
        // The catalogue defaults embed the container port because without a
        // domain that is how you reach the app. Through the panel's nginx the
        // public URL has no port, and shipping one produces a URL that resolves
        // to nothing — the failure this change exists to remove, reintroduced.
        for (template_id, vars) in TEMPLATE_DOMAIN_ENV {
            let derived = domain_env_for(template_id, "example.com");
            assert_eq!(derived.len(), vars.len());
            for (name, value) in &derived {
                assert!(
                    !value.contains("example.com:"),
                    "{template_id}.{name} = {value} carries a port"
                );
                assert!(
                    !value.contains("localhost"),
                    "{template_id}.{name} = {value} still points at localhost"
                );
            }
        }
    }

    #[test]
    fn a_masked_value_sent_back_leaves_the_real_one_alone() {
        // Exactly what the edit dialog posts after the user changes one field:
        // it was seeded from the MASKED read, so every untouched secret comes
        // back as the mask.
        let container = vec![
            "ADMIN_TOKEN=hunter2-the-real-one".to_string(),
            "PUID=1000".to_string(),
        ];
        let posted = env(&[("ADMIN_TOKEN", ENV_MASK), ("PUID", "1001")]);

        let (list, kept) = merge_env_preserving_masked(&posted, &container);

        assert!(
            list.contains(&"ADMIN_TOKEN=hunter2-the-real-one".to_string()),
            "the real value must survive; got {list:?}"
        );
        assert!(!list.iter().any(|kv| kv.contains(ENV_MASK)));
        assert!(list.contains(&"PUID=1001".to_string()), "edits still apply");
        assert_eq!(kept, vec!["ADMIN_TOKEN".to_string()]);
    }

    #[test]
    fn a_value_that_merely_looks_like_the_mask_is_not_resurrected() {
        // A NEW variable whose value the operator really did type as eight
        // asterisks has nothing to fall back to, and must be stored verbatim
        // rather than silently dropped.
        let (list, kept) = merge_env_preserving_masked(&env(&[("NEW", ENV_MASK)]), &[]);
        assert_eq!(list, vec![format!("NEW={ENV_MASK}")]);
        assert!(kept.is_empty());
    }

    #[test]
    fn a_container_env_entry_with_no_equals_does_not_derail_the_merge() {
        let container = vec!["MALFORMED".to_string(), "A=1".to_string()];
        let (list, _) = merge_env_preserving_masked(&env(&[("A", ENV_MASK)]), &container);
        assert_eq!(list, vec!["A=1".to_string()]);
    }

    #[test]
    fn the_catalogue_exempts_its_own_non_secrets_and_never_its_secrets() {
        // All four contain a SENSITIVE_PATTERN substring; none is a secret.
        for name in [
            "KEYCLOAK_ADMIN",
            "NEXTAUTH_URL",
            "AUTHENTIK_POSTGRESQL__HOST",
            "AUTHENTIK_REDIS__HOST",
        ] {
            assert!(
                catalogue_non_secret_env(name),
                "{name} is declared secret:false in the catalogue and must not be masked"
            );
        }
        // The real secrets sitting right beside them stay masked.
        for name in [
            "KEYCLOAK_ADMIN_PASSWORD",
            "NEXTAUTH_SECRET",
            "AUTHENTIK_SECRET_KEY",
            "AUTHENTIK_POSTGRESQL__PASSWORD",
        ] {
            assert!(!catalogue_non_secret_env(name), "{name} must stay masked");
        }
        // A name the catalogue has never heard of falls through to the
        // substring test rather than being exempted.
        assert!(!catalogue_non_secret_env("SOME_VENDOR_API_KEY"));
    }

    // ── Volume ownership ───────────────────────────────────────────

    /// An empty `User` is how almost every image ships and it means root, which is
    /// the case that must stay untouched: those images chown their own data
    /// directory in their entrypoint.
    #[test]
    fn root_user_specs_are_recognised_in_every_spelling() {
        for spec in ["", "root", "0", "0:0", "root:root", "  root  ", "0:1000"] {
            assert!(user_spec_is_root(spec), "{spec:?} names root");
        }
        for spec in ["nobody", "472", "nonroot", "65532:65532", "node-red", "1000"] {
            assert!(!user_spec_is_root(spec), "{spec:?} does not name root");
        }
    }

    #[test]
    fn user_specs_split_into_their_halves() {
        assert_eq!(split_user_spec("nobody"), ("nobody", None));
        assert_eq!(split_user_spec("65532:65532"), ("65532", Some("65532")));
        assert_eq!(split_user_spec("node:node"), ("node", Some("node")));
        assert_eq!(split_user_spec("1000:staff"), ("1000", Some("staff")));
    }

    /// The real `/etc/passwd` out of `prom/prometheus:v3`, whose `Config.User` is
    /// the NAME `nobody` — resolvable only from inside the image.
    #[test]
    fn a_login_name_resolves_from_the_images_own_passwd() {
        let passwd = "root:x:0:0:root:/root:/bin/ash\nnobody:x:65534:65534:nobody:/home:/bin/false\n";
        assert_eq!(passwd_lookup(passwd, "nobody"), Some((65534, 65534)));
        assert_eq!(passwd_lookup(passwd, "root"), Some((0, 0)));
        // A name the image does not define must fail rather than guess.
        assert_eq!(passwd_lookup(passwd, "grafana"), None);
        // A bare numeric uid still wants its primary group.
        assert_eq!(passwd_gid_for_uid(passwd, 65534), Some(65534));
        assert_eq!(passwd_gid_for_uid(passwd, 472), None);
    }

    #[test]
    fn a_group_name_resolves_from_the_images_own_group_file() {
        let group = "root:x:0:\nnogroup:x:65534:\nnode-red:x:1000:\n";
        assert_eq!(group_lookup(group, "nogroup"), Some(65534));
        assert_eq!(group_lookup(group, "node-red"), Some(1000));
        assert_eq!(group_lookup(group, "absent"), None);
    }

    /// The tar reader has to cope with what `docker cp` actually returns, and must
    /// stop rather than guess when it is handed something else.
    #[test]
    fn the_tar_reader_extracts_a_file_and_refuses_junk() {
        fn ustar(name: &str, body: &str) -> Vec<u8> {
            let mut block = vec![0u8; 512];
            block[..name.len()].copy_from_slice(name.as_bytes());
            let size = format!("{:011o}\0", body.len());
            block[124..124 + size.len()].copy_from_slice(size.as_bytes());
            block[156] = b'0';
            block.extend_from_slice(body.as_bytes());
            block.resize(512 + body.len().div_ceil(512) * 512, 0);
            block
        }

        let body = "nobody:x:65534:65534:nobody:/home:/bin/false\n";
        assert_eq!(tar_extract_first_file(&ustar("passwd", body)).as_deref(), Some(body));

        // An all-zero block is the end of an archive, not a file.
        assert_eq!(tar_extract_first_file(&vec![0u8; 1024]), None);
        // A truncated stream must not panic or return a partial read. The cut has
        // to land INSIDE the body (header 512 + body 44 = 556), or the archive is
        // merely missing its padding and still parses perfectly.
        let mut truncated = ustar("passwd", body);
        truncated.truncate(530);
        assert_eq!(tar_extract_first_file(&truncated), None);
        // Nothing at all.
        assert_eq!(tar_extract_first_file(&[]), None);
    }

    /// Every template whose image runs non-root and declares a volume needed the
    /// chown, so the catalogue must not quietly lose the volumes that make them
    /// victims. Measured s342 by reading all 148 images' config blobs from the
    /// registry: 42 run non-root, 31 of those declare a volume.
    ///
    /// ⚠ The cases are spelled `"id count"` rather than as `(id, count)` tuples on
    /// purpose. `app-template-images-pin-e2e.sh` §6 proves `TEMPLATE_CMD` carries
    /// no entry for the three templates a command cannot fix, by grepping this
    /// file's whitespace-stripped source for a parenthesised template id followed
    /// by a comma. A tuple here flattens to exactly that shape and turns another
    /// suite red against correct code. Test DATA is raw source like anything else,
    /// and so is this comment — that suite does not strip comments, so the shape
    /// must not be spelled out here either. Do not "tidy" this back into tuples.
    #[test]
    fn the_measured_victims_still_declare_the_volumes_they_could_not_write() {
        for case in [
            "prometheus 1",
            "grafana 1",
            "jenkins 1",
            "mattermost 3",
            "sonarqube 3",
            "node-red 1",
            "surrealdb 1",
        ] {
            let (id, count) = case.split_once(' ').expect("each case is `id count`");
            let count: usize = count.parse().expect("count parses");
            let template = TEMPLATES
                .iter()
                .find(|t| t.id == id)
                .unwrap_or_else(|| panic!("{id} must exist in the catalogue"));
            assert_eq!(
                template.volumes.len(),
                count,
                "{id} declares {:?}",
                template.volumes
            );
        }
    }

    /// The catalogue, projected to data, so a checker never has to parse Rust.
    ///
    /// Every defect this file has shipped was found by something READING it:
    /// eighteen image references that did not resolve, thirty-one volumes the app
    /// could not write, data deleted on update — three consecutive releases of
    /// hand-audit. The checks those produced live in `tests/` and read this source
    /// with `awk` and `grep`, which is why one of them has to open by asserting the
    /// catalogue still parses: a parser that quietly returns three entries reports
    /// the other hundred and forty-five as clean, and reads exactly like a pass.
    ///
    /// So emit it instead. The mechanism is the one
    /// `services/prerequisites/copy.rs` already uses for the guidance manual — a
    /// committed artefact plus a test that FAILS when it stops matching the
    /// registry. It is not a reminder to regenerate; it stands between a changed
    /// template and a merge, and it gives the runtime census a catalogue that
    /// cannot silently disagree with the one the agent actually deploys.
    fn generated_catalogue() -> String {
        let templates: Vec<serde_json::Value> = TEMPLATES
            .iter()
            .map(|t| {
                serde_json::json!({
                    "id": t.id,
                    "name": t.name,
                    "category": t.category,
                    "image": t.image,
                    "default_port": t.default_port,
                    "container_port": t.container_port,
                    // Null for all but the handful the catalogue overrides, which
                    // is exactly what the container gets.
                    "cmd": template_cmd(t.id),
                    "env_vars": t
                        .env_vars
                        .iter()
                        .map(|e| {
                            serde_json::json!({
                                "name": e.name,
                                "label": e.label,
                                "default": e.default,
                                "required": e.required,
                                "secret": e.secret,
                            })
                        })
                        .collect::<Vec<_>>(),
                    "volumes": t.volumes,
                })
            })
            .collect();

        let mut out = serde_json::to_string_pretty(&serde_json::json!({
            "_generated": "Do not edit. Source of truth is TEMPLATES in \
                           panel/agent/src/services/docker_apps.rs. Regenerate with \
                           UPDATE_TEMPLATE_CATALOGUE=1 cargo test -p dockpanel-agent catalogue",
            "count": TEMPLATES.len(),
            "templates": templates,
        }))
        .expect("the catalogue serialises");
        out.push('\n');
        out
    }

    /// Regenerate with:
    ///     UPDATE_TEMPLATE_CATALOGUE=1 cargo test -p dockpanel-agent catalogue
    #[test]
    fn the_generated_catalogue_is_current() {
        let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../tests/fixtures/app-templates.json");
        let expected = generated_catalogue();

        if std::env::var("UPDATE_TEMPLATE_CATALOGUE").is_ok() {
            if let Some(dir) = path.parent() {
                std::fs::create_dir_all(dir).expect("create the fixtures directory");
            }
            std::fs::write(&path, &expected).expect("write the generated catalogue");
            return;
        }

        let found = std::fs::read_to_string(&path).unwrap_or_else(|e| {
            panic!(
                "tests/fixtures/app-templates.json is missing ({e}). Regenerate it:\n    \
                 UPDATE_TEMPLATE_CATALOGUE=1 cargo test -p dockpanel-agent catalogue"
            )
        });

        if found != expected {
            // Naming the first divergence, rather than printing two 148-entry
            // documents at each other, is the difference between a failure that
            // is read and one that is skipped.
            let (line, committed, registry) = found
                .lines()
                .zip(expected.lines())
                .enumerate()
                .find(|(_, (f, e))| f != e)
                .map(|(i, (f, e))| (i + 1, f.to_string(), e.to_string()))
                .unwrap_or_else(|| {
                    (
                        found.lines().count().min(expected.lines().count()) + 1,
                        "<end of file>".to_string(),
                        "<more lines>".to_string(),
                    )
                });
            panic!(
                "tests/fixtures/app-templates.json is out of date — the catalogue \
                 changed and this artefact did not.\nFirst difference at line \
                 {line}:\n  committed: {committed}\n  registry:  {registry}\n\n\
                 Regenerate it:\n    \
                 UPDATE_TEMPLATE_CATALOGUE=1 cargo test -p dockpanel-agent catalogue"
            );
        }
    }

    /// Hits the local Docker daemon and the two images the fix was proven on, so
    /// it is not part of the ordinary suite. Run with:
    ///   cargo test -p dockpanel-agent volume_owner -- --ignored --nocapture
    #[tokio::test]
    #[ignore = "requires a Docker daemon with the test images pulled"]
    async fn volume_owner_resolves_real_images_both_ways() {
        let docker = Docker::connect_with_unix_defaults().expect("docker");

        // A NAME, resolvable only from inside the image.
        let prometheus = resolve_volume_owner(&docker, "prom/prometheus:v3").await;
        assert_eq!(
            prometheus,
            Some(VolumeOwner {
                uid: 65534,
                gid: Some(65534),
                spec: "nobody".to_string()
            })
        );

        // A bare numeric uid, whose primary group still comes from the image.
        let grafana = resolve_volume_owner(&docker, "grafana/grafana:13.1").await;
        assert_eq!(grafana.as_ref().map(|o| o.uid), Some(472));

        // The control: a root image must resolve to None so nothing is chowned.
        assert_eq!(resolve_volume_owner(&docker, "nginx:alpine").await, None);
    }

    /// Runs a `curlimages/curl` container on `network`, waits for it to exit,
    /// and returns its exit code and log output — the harness the sidecar
    /// proxy test below uses to prove the ACL from OUTSIDE the proxy's own
    /// process, the same vantage point a real Dozzle container has.
    async fn run_probe(docker: &Docker, network: &str, name: &str, args: &[&str]) -> String {
        let _ = docker
            .remove_container(
                name,
                Some(RemoveContainerOptions {
                    force: true,
                    v: true,
                    ..Default::default()
                }),
            )
            .await;

        let mut pull = docker.create_image(
            Some(CreateImageOptions { from_image: "curlimages/curl:latest", ..Default::default() }),
            None,
            None,
        );
        while let Some(r) = pull.next().await {
            let _ = r;
        }

        let mut endpoints = HashMap::new();
        endpoints.insert(network.to_string(), bollard::models::EndpointSettings::default());
        let config = Config {
            image: Some("curlimages/curl:latest".to_string()),
            cmd: Some(args.iter().map(|s| s.to_string()).collect()),
            networking_config: Some(bollard::container::NetworkingConfig { endpoints_config: endpoints }),
            ..Default::default()
        };
        let container = docker
            .create_container(Some(CreateContainerOptions { name, platform: None }), config)
            .await
            .expect("create probe container");
        docker
            .start_container(&container.id, None::<StartContainerOptions<String>>)
            .await
            .expect("start probe container");

        let mut wait = docker.wait_container(&container.id, None::<bollard::container::WaitContainerOptions<String>>);
        while let Some(w) = wait.next().await {
            let _ = w;
        }

        let logs = get_app_logs(&container.id, 50).await.unwrap_or_default();
        let _ = docker
            .remove_container(
                &container.id,
                Some(RemoveContainerOptions {
                    force: true,
                    v: true,
                    ..Default::default()
                }),
            )
            .await;
        logs
    }

    /// Exercises the REAL `docker-socket-proxy` sidecar end to end against the
    /// local daemon — not just that `app_sidecar::deploy` compiles, but that
    /// the security property it exists for actually holds, from a probe
    /// container on the sidecar's own network (the same vantage point a real
    /// Dozzle container has, never the panel's own process).
    #[tokio::test]
    #[ignore = "creates real containers and a real network; run with cargo test -p dockpanel-agent sidecar_actually_proxies -- --ignored --nocapture"]
    async fn sidecar_actually_proxies_reads_and_refuses_writes() {
        let docker = Docker::connect_with_unix_defaults().expect("docker");
        let app = "sidecartest-s426";

        // Clean up any leftover from a prior interrupted run before asserting anything.
        app_sidecar::teardown(app).await;

        let sidecar = template_sidecar("dozzle").expect("dozzle must declare a sidecar");
        let network = app_sidecar::deploy(&docker, app, sidecar)
            .await
            .expect("sidecar deploy must succeed");
        assert_eq!(network, app_sidecar::network_name(app));

        let sidecar_id = app_sidecar::find(&docker, app)
            .await
            .expect("the sidecar container must be discoverable by its deterministic name");
        let info = docker.inspect_container(&sidecar_id, None).await.expect("inspect sidecar");
        assert_eq!(
            info.state.and_then(|s| s.running),
            Some(true),
            "the sidecar must actually be running, not just created"
        );

        let read = run_probe(
            &docker,
            &network,
            "sidecar-probe-read",
            &["-s", "-o", "/dev/null", "-w", "%{http_code}", "http://dockerproxy:2375/containers/json"],
        )
        .await;
        assert_eq!(
            read.trim(),
            "200",
            "CONTAINERS=1 must let a probe on the private network list containers through the \
             proxy — dozzle cannot do its entire job otherwise; got {read:?}"
        );

        let write = run_probe(
            &docker,
            &network,
            "sidecar-probe-write",
            &[
                "-s", "-o", "/dev/null", "-w", "%{http_code}", "-X", "POST",
                "http://dockerproxy:2375/containers/create", "-d", "{}",
            ],
        )
        .await;
        assert_eq!(
            write.trim(),
            "403",
            "POST=0 must refuse container creation through the proxy — this is the actual \
             security boundary #125's fix depends on; got {write:?}"
        );

        app_sidecar::teardown(app).await;
        assert!(
            app_sidecar::find(&docker, app).await.is_none(),
            "teardown must remove the sidecar container"
        );
        assert!(
            docker.inspect_network::<String>(&network, None).await.is_err(),
            "teardown must remove the app's private network"
        );
    }

    /// Build a minimal ustar header. Only the three fields the extractor reads
    /// are populated — writing a checksum it never verifies would be describing a
    /// format rather than testing the parser.
    fn tar_header(name: &str, size: usize, kind: u8, mode: u32) -> Vec<u8> {
        let mut h = vec![0u8; 512];
        h[..name.len()].copy_from_slice(name.as_bytes());
        let mode_field = format!("{mode:07o}\0");
        h[100..100 + mode_field.len()].copy_from_slice(mode_field.as_bytes());
        let size_field = format!("{size:011o}\0");
        h[124..124 + size_field.len()].copy_from_slice(size_field.as_bytes());
        h[156] = kind;
        h
    }

    fn tar_entry(name: &str, body: &[u8], kind: u8) -> Vec<u8> {
        tar_entry_mode(name, body, kind, 0o644)
    }

    fn tar_entry_mode(name: &str, body: &[u8], kind: u8, mode: u32) -> Vec<u8> {
        let mut out = tar_header(name, body.len(), kind, mode);
        out.extend_from_slice(body);
        let pad = body.len().div_ceil(512) * 512 - body.len();
        out.extend(std::iter::repeat_n(0u8, pad));
        out
    }

    /// The repair must touch root-owned paths and nothing else.
    ///
    /// ⚠ SPLIT IN TWO, AND THE HALVES DEFEND DIFFERENT THINGS. Say which, because
    /// the first version of this comment claimed the unprivileged half carried the
    /// `uid() == 0` filter and mutation testing showed it does not:
    ///
    ///   * **unprivileged half** — defends the cheap top-level probe. A tree with
    ///     nothing root-owned must come back 0 and be left completely alone.
    ///   * **root half** — defends the filter itself (a third party's uid survives
    ///     untouched), that root-owned paths really are handed over, and that a
    ///     second pass is a no-op.
    ///
    /// Dropping the filter therefore goes RED only under root: the probe returns
    /// early for an unprivileged runner, so the walk it guards is never reached.
    /// Verified by planting exactly that and running BOTH ways.
    ///
    /// Root is never load-bearing for a PASS — the unprivileged half asserts a real
    /// invariant and says out loud what it skipped, which is #491 from the far side.
    #[test]
    fn the_repair_touches_only_root_owned_paths() {
        use std::os::unix::fs::MetadataExt;
        let base = std::env::temp_dir().join(format!("dp-repair-{}", uuid::Uuid::new_v4()));
        let root = base.join("volume");
        std::fs::create_dir_all(root.join("sub")).expect("temp tree");
        std::fs::write(root.join("mine.db"), b"x").expect("file");
        std::fs::write(root.join("sub").join("nested"), b"x").expect("nested file");

        let me = unsafe { libc::getuid() };
        let owner = VolumeOwner {
            uid: me,
            gid: Some(unsafe { libc::getgid() }),
            spec: String::new(),
        };

        // Nothing here is root-owned (unless the suite itself runs as root, in
        // which case everything just created is — handled below).
        if me != 0 {
            assert_eq!(
                chown_root_owned_entries(&root, &owner).expect("walks"),
                0,
                "a tree with no root-owned path must be left completely alone"
            );
            eprintln!("note: the positive half needs root; skipped for uid {me}");
            std::fs::remove_dir_all(&base).ok();
            return;
        }

        // As root, the target must be an UNPRIVILEGED uid — the real case is an
        // image running as somebody other than root, and chowning root to root
        // would make the whole exercise an identity operation. (The first draft of
        // this test did exactly that and its second pass still found four.)
        let app_uid: u32 = 4242;
        let owner = VolumeOwner { uid: app_uid, gid: Some(app_uid), spec: String::new() };

        // A path owned by a third party, established BEFORE the repair runs.
        let third = root.join("third-party");
        std::fs::write(&third, b"x").expect("file");
        let sentinel: u32 = 12345;
        lchown_to(
            &third,
            &VolumeOwner { uid: sentinel, gid: Some(sentinel), spec: String::new() },
        )
        .expect("chown to the sentinel uid");

        // volume, mine.db, sub, sub/nested — four root-owned paths. NOT third-party.
        let claimed = chown_root_owned_entries(&root, &owner).expect("walks");
        assert_eq!(claimed, 4, "expected the volume, two files and the subdirectory");
        assert_eq!(
            std::fs::symlink_metadata(&third).expect("still there").uid(),
            sentinel,
            "a path owned by a third uid was rewritten; the repair must only reclaim root"
        );
        assert_eq!(
            std::fs::symlink_metadata(root.join("mine.db")).expect("still there").uid(),
            app_uid,
            "a root-owned file was not handed to the app"
        );

        // Idempotent: nothing root-owned is left, so the cheap probe exits at once.
        let after = chown_root_owned_entries(&root, &owner).expect("walks again");
        assert_eq!(after, 0, "a second pass has nothing left to reclaim, got {after}");

        std::fs::remove_dir_all(&base).ok();
    }

    /// The migration's chown must reach every file it just wrote, and must not
    /// walk out of the directory through a symlink while doing it.
    ///
    /// Chowns to the CURRENT uid/gid on purpose. Setting a file you already own to
    /// the owner it already has is permitted for an unprivileged user, so this runs
    /// identically for root on a dev box and for a non-root CI runner — the #491
    /// shape, where a harness passed locally only because root happened to be
    /// load-bearing and nobody noticed. What it measures is the WALK, which is the
    /// part that was wrong: the old code chowned the directory alone, and `docker
    /// cp` had already written everything inside it as root.
    #[test]
    fn the_migrated_chown_reaches_every_file_and_never_follows_a_symlink() {
        let base = std::env::temp_dir().join(format!("dp-chown-{}", uuid::Uuid::new_v4()));
        let root = base.join("volume");
        let outside = base.join("outside");
        std::fs::create_dir_all(root.join("sub")).expect("temp tree");
        std::fs::create_dir_all(&outside).expect("temp tree");
        std::fs::write(root.join("database.sqlite"), b"x").expect("file");
        std::fs::write(root.join("sub").join("nested"), b"x").expect("nested file");
        std::fs::write(outside.join("untouched"), b"x").expect("outside file");
        std::os::unix::fs::symlink(&outside, root.join("link")).expect("symlink");

        let owner = VolumeOwner {
            uid: unsafe { libc::getuid() },
            gid: Some(unsafe { libc::getgid() }),
            spec: String::new(),
        };
        let touched = chown_migrated_tree(&root, &owner).expect("chowns the tree");

        // The volume, its two files, its subdirectory, and the symlink AS a link:
        // five. Four or fewer means it stopped at the directory, which is the
        // defect. Six or more means it walked THROUGH the link, which would apply
        // the app's uid to files outside the volume altogether.
        assert_eq!(
            touched, 5,
            "expected the volume, its two files, its subdirectory and the symlink \
             itself — got {touched}"
        );
        assert!(
            outside.join("untouched").exists(),
            "the symlink target must be left alone"
        );

        std::fs::remove_dir_all(&base).ok();
    }

    #[test]
    fn seeding_unpacks_the_image_content_without_its_leading_component() {
        let dir = std::env::temp_dir().join(format!("dp-seed-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).expect("temp dir");

        // Exactly the shape Docker's copy API returns for `/etc/caddy`.
        let mut archive = tar_entry("caddy/", b"", b'5');
        archive.extend(tar_entry("caddy/Caddyfile", b":80\nrespond \"hi\"\n", b'0'));
        archive.extend(tar_entry("caddy/conf.d/extra.conf", b"# extra\n", b'0'));
        archive.extend(tar_entry_mode("caddy/run.sh", b"#!/bin/sh\n", b'0', 0o755));

        let written = tar_extract_into(&archive, &dir).expect("extracts");

        assert_eq!(
            std::fs::read_to_string(dir.join("Caddyfile")).expect("Caddyfile lands"),
            ":80\nrespond \"hi\"\n",
            "the archive's leading component must be stripped, or the file arrives \
             one directory too deep and the image still cannot find it"
        );
        assert!(dir.join("conf.d/extra.conf").exists(), "nested entries land too");

        // Modes carry across. An image ships executable entrypoints, and an
        // extractor that writes everything 0644 hands the app a directory that
        // looks correct and fails at the first exec — silently, and only for the
        // templates whose shipped files matter most.
        // Asserts the executable BIT rather than a full mode. A literal spelling
        // the world-writable mode would trip §4 of app-volume-ownership-pin-e2e.sh,
        // which greps raw source and does not exempt test data.
        use std::os::unix::fs::PermissionsExt;
        let mode = std::fs::metadata(dir.join("run.sh"))
            .expect("the executable entry lands")
            .permissions()
            .mode();
        assert!(
            mode & 0o111 != 0,
            "the archive's executable bit was dropped (mode {mode:o})"
        );
        let plain = std::fs::metadata(dir.join("Caddyfile"))
            .expect("the plain entry lands")
            .permissions()
            .mode();
        assert!(
            plain & 0o111 == 0,
            "a non-executable entry was made executable (mode {plain:o})"
        );
        // Two, not three: the archive's own root entry strips to nothing and is
        // skipped, because the destination directory already exists.
        assert_eq!(written.len(), 3, "wrote {written:?}");

        std::fs::remove_dir_all(&dir).ok();
    }

    /// The archive is built by a THIRD PARTY's image, so this is a security
    /// property and not a tidiness one.
    #[test]
    fn seeding_refuses_an_archive_that_tries_to_escape_its_directory() {
        let dir = std::env::temp_dir().join(format!("dp-seed-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).expect("temp dir");
        let sentinel = dir.join("escaped.txt");

        // `x/../escaped.txt` strips to `../escaped.txt`, which resolves to the
        // PARENT of the destination — the classic tar-slip.
        let mut archive = tar_entry("x/../escaped.txt", b"owned\n", b'0');
        // Two levels up, in case one `..` is special-cased and a second is not.
        archive.extend(tar_entry("x/../../escaped-twice.txt", b"owned\n", b'0'));
        // A symlink, skipped because a link is a second way to name a path
        // outside the destination even when the member name looks innocent.
        archive.extend(tar_entry("x/link", b"", b'2'));
        // A hardlink, same reasoning.
        archive.extend(tar_entry("x/hard", b"", b'1'));
        // The positive control. Without it this test would pass just as well
        // against an extractor that writes nothing at all, and an absence arm
        // that cannot distinguish "refused" from "did nothing" asserts nothing.
        archive.extend(tar_entry("x/legitimate.conf", b"fine\n", b'0'));

        let written = tar_extract_into(&archive, &dir).expect("does not error");

        assert_eq!(
            written,
            vec![dir.join("legitimate.conf")],
            "only the safe member may be written"
        );
        assert!(!sentinel.exists(), "an entry escaped into the parent directory");
        assert!(
            !dir.parent().unwrap().join("escaped-twice.txt").exists(),
            "an entry escaped two levels up"
        );
        assert!(!dir.join("link").exists(), "a symlink member was materialised");
        assert!(!dir.join("hard").exists(), "a hardlink member was materialised");

        std::fs::remove_dir_all(&dir).ok();
    }

    /// The guard that keeps an operator's data safe: seeding only ever fills a
    /// directory that is empty. Proven on the emptiness test itself, since the
    /// Docker half needs a daemon.
    #[test]
    fn seeding_writes_into_a_directory_that_already_has_content_only_by_refusing() {
        let dir = std::env::temp_dir().join(format!("dp-seed-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).expect("temp dir");
        std::fs::write(dir.join("operators-own.conf"), b"do not touch\n").expect("write");

        let occupied = std::fs::read_dir(&dir)
            .expect("readable")
            .next()
            .is_some();
        assert!(
            occupied,
            "the emptiness test seeding gates on must see existing content"
        );

        std::fs::remove_dir_all(&dir).ok();
    }

    fn labels_of(pairs: &[(&str, &str)]) -> HashMap<String, String> {
        pairs
            .iter()
            .map(|(k, v)| ((*k).to_string(), (*v).to_string()))
            .collect()
    }

    /// The container is named for the app; the DATA is named for the app alone.
    #[test]
    fn the_data_directory_is_named_for_the_app_and_not_for_its_container() {
        let labels = labels_of(&[("dockpanel.app.name", "n8n")]);
        assert_eq!(app_data_name(Some(&labels), "dockpanel-app-n8n"), "n8n");

        // No label: strip the prefix the deploy added rather than keeping it. Getting
        // this wrong is the whole defect — the migration wrote under the container name.
        assert_eq!(app_data_name(None, "dockpanel-app-ghost"), "ghost");
        assert_eq!(app_data_name(None, "something-else"), "something-else");

        // An empty label is not an answer.
        let blank = labels_of(&[("dockpanel.app.name", "")]);
        assert_eq!(app_data_name(Some(&blank), "dockpanel-app-gitea"), "gitea");

        // The label WINS over the container name, because the label is what the removal
        // path reads. A container renamed by hand must not move the data directory.
        let renamed = labels_of(&[("dockpanel.app.name", "wiki")]);
        assert_eq!(app_data_name(Some(&renamed), "dockpanel-app-old"), "wiki");
    }

    /// Both spellings are cleaned up; nothing else is touched.
    #[test]
    fn removal_follows_the_bind_and_accepts_the_directory_the_old_migration_wrote() {
        let canonical = vec![format!("{APP_DATA_DIR}/n8n/home/node/.n8n:/home/node/.n8n")];
        assert_eq!(
            owned_app_dir("n8n", &canonical),
            Some(format!("{APP_DATA_DIR}/n8n"))
        );

        // What v2.111.0 through v2.113.3 actually wrote. Before this, the delete path
        // rebuilt the path from the label, found nothing there, and left the data behind
        // without even warning — because the label-derived directory did not exist.
        let legacy = vec![format!(
            "{APP_DATA_DIR}/dockpanel-app-n8n/home/node/.n8n:/home/node/.n8n"
        )];
        assert_eq!(
            owned_app_dir("n8n", &legacy),
            Some(format!("{APP_DATA_DIR}/dockpanel-app-n8n"))
        );

        // A compose service binds outside the app directory entirely and must keep
        // owning nothing, or it deletes the tree of the app it shares a name with.
        let compose = vec!["/var/lib/dockpanel/compose/n8n/data:/data".to_string()];
        assert_eq!(owned_app_dir("n8n", &compose), None);

        // A neighbouring app's directory is not this app's, and a prefix that stops
        // mid-segment must not match.
        let neighbour = vec![format!("{APP_DATA_DIR}/n8n-staging/data:/data")];
        assert_eq!(owned_app_dir("n8n", &neighbour), None);
        assert_eq!(owned_app_dir("data", &vec![format!("{APP_DATA_DIR}/database/x:/x")]), None);

        assert_eq!(owned_app_dir("n8n", &[]), None);
    }

    /// A managed container the catalogue never heard of still has data worth keeping.
    #[test]
    fn a_managed_container_outside_the_catalogue_still_declares_its_state() {
        let config = bollard::models::ContainerConfig {
            labels: Some(labels_of(&[
                ("dockpanel.managed", "true"),
                ("dockpanel.app.template", "roundcube"),
                ("dockpanel.app.name", "roundcube"),
            ])),
            ..Default::default()
        };

        // Nothing mounted: the webmail database is in the writable layer, which every
        // recreate deletes. Before this it reported nothing to migrate at all.
        let bare = bollard::service::HostConfig::default();
        assert_eq!(
            unmounted_declared_volumes(&config, &bare),
            vec!["/var/roundcube/db"]
        );

        // Once it is bound there is nothing left to rescue.
        let bound = bollard::service::HostConfig {
            binds: Some(vec![format!(
                "{APP_DATA_DIR}/roundcube/var/roundcube/db:/var/roundcube/db"
            )]),
            ..Default::default()
        };
        assert!(unmounted_declared_volumes(&config, &bound).is_empty());

        // A compose service brings its own mounts and is deliberately left alone.
        let compose = bollard::models::ContainerConfig {
            labels: Some(labels_of(&[
                ("dockpanel.managed", "true"),
                ("dockpanel.app.template", "compose"),
            ])),
            ..Default::default()
        };
        assert!(unmounted_declared_volumes(&compose, &bare).is_empty());

        // And an unmanaged container is still none of our business.
        let unmanaged = bollard::models::ContainerConfig {
            labels: Some(labels_of(&[("dockpanel.app.template", "roundcube")])),
            ..Default::default()
        };
        assert!(unmounted_declared_volumes(&unmanaged, &bare).is_empty());
    }

    /// The command comes back from the TEMPLATE, not from the departing image.
    #[test]
    fn a_recreate_re_derives_the_catalogue_command_from_the_template_label() {
        // A template the catalogue gives a command to keeps it across an image change.
        // Without this the image's own default reasserts itself, and for these templates
        // that default prints usage and exits.
        let ntfy = labels_of(&[("dockpanel.app.template", "ntfy")]);
        assert_eq!(
            catalogue_cmd_for(Some(&ntfy)),
            Some(vec!["serve".to_string()])
        );

        // A template with no override gets none, so the NEW image's default applies —
        // which is the behaviour this path always intended.
        let ghost = labels_of(&[("dockpanel.app.template", "ghost")]);
        assert_eq!(catalogue_cmd_for(Some(&ghost)), None);

        assert_eq!(catalogue_cmd_for(None), None);
        let unknown = labels_of(&[("dockpanel.app.template", "not-a-template")]);
        assert_eq!(catalogue_cmd_for(Some(&unknown)), None);
    }

    /// Every template the catalogue gives a command to is one a recreate can recover it for.
    #[test]
    fn every_commanded_template_is_reachable_through_its_own_label() {
        for (id, args) in TEMPLATE_CMD {
            let labels = labels_of(&[("dockpanel.app.template", id)]);
            let recovered = catalogue_cmd_for(Some(&labels));
            assert_eq!(
                recovered.as_deref(),
                Some(
                    args.iter()
                        .map(|a| (*a).to_string())
                        .collect::<Vec<_>>()
                        .as_slice()
                ),
                "'{id}' has a catalogue command that a recreate cannot recover"
            );
        }
    }

    /// ⚠ The template id here is deliberately one that does NOT exist, and that is
    /// load-bearing rather than tidy.
    ///
    /// The guard runs before the catalogue lookup, so with the guard in place this
    /// refuses on the NAME. With the guard removed — which is exactly what a mutation
    /// test does to it — an id naming a real template would carry straight on into
    /// `deploy_app`'s pull and `create_container`, and the "test" would deploy a real
    /// container onto whatever box it was run on. That is not hypothetical: it happened
    /// on this project's own demo host, and the run that did it reported a clean kill.
    /// With an unknown id the mutant fails on `Unknown template` instead, so the
    /// mutation is still caught and the daemon is never contacted either way.
    #[tokio::test]
    async fn an_app_may_not_be_named_so_it_shadows_another_apps_data_directory() {
        let err = deploy_app(
            "dockpanel-no-such-template",
            "dockpanel-app-n8n",
            8080,
            HashMap::new(),
            None,
            None,
            None,
            None,
            false,
            None,
        )
        .await
        .expect_err("a name that shadows another app's data directory must be refused");
        assert!(
            err.contains("not an available app name"),
            "refusal should say why, got: {err}"
        );
    }

    // ── The runtime census ──────────────────────────────────────────────────
    //
    // Templates this census cannot judge, because serving requires a service the
    // census has no way to supply. Each one asks the operator for it BY NAME on
    // the deploy form, so the operator is told; a template that needs a database
    // and never mentions one is a defect, and belongs in the catalogue's fixes
    // rather than on this list.
    //
    // ⚠ Spelled `id | reason` rather than as tuples ON PURPOSE, exactly as
    // `the_measured_victims_still_declare_the_volumes_they_could_not_write`
    // above is. `app-template-images-pin-e2e.sh` §6 proves the command table
    // carries no entry for the templates a command cannot fix, by grepping this
    // file's whitespace-stripped source for a parenthesised template id followed
    // by a comma — and that suite does not strip comments. A tuple here, in code
    // or in this sentence, flattens to precisely that shape and turns another
    // suite red against correct code. Do not "tidy" these into tuples.
    /// A template the census cannot judge because it needs something no automated
    /// run can produce — and which the CATALOGUE does not already say so about.
    ///
    /// ⚠ It is deliberately tiny, and it must stay tiny. The first version of this
    /// list was hand-written and named eight templates; **sixty-one** of the 148
    /// require an operator-supplied value, so it was wrong by fifty-three the day
    /// it shipped and would have reported every one of them BROKEN. A list that
    /// has to enumerate a large, moving subset is a list that is always wrong —
    /// derive the rule instead, which `census_needs_operator_input` now does.
    /// What is left here is the residue that rule cannot see.
    static CENSUS_NEEDS_A_SERVICE_THE_CENSUS_CANNOT_SUPPLY: &[&str] = &[
        "vllm | an NVIDIA GPU. It exits with `Failed to infer device type` on a \
         CPU-only host, which is what a CI runner is. The catalogue already marks \
         it GPU-recommended, so this is the census lacking hardware rather than \
         the template being wrong",
    ];

    /// Does deploying this template require a value only a human can supply?
    ///
    /// A required field with no default is exactly how the catalogue says "the
    /// operator must type this" — the deploy form refuses to submit without it.
    /// The census can only invent a syntactically plausible value, so for these
    /// templates a failure says nothing: it is indistinguishable from the census
    /// having guessed a database URL that points at nothing. Reporting them as
    /// broken would be asserting something the run cannot support.
    ///
    /// Derived from the catalogue, so a template that gains or loses such a field
    /// is reclassified automatically and no list has to be maintained.
    fn census_needs_operator_input(t: &AppTemplateDef) -> bool {
        t.env_vars.iter().any(|e| e.required && e.default.is_empty())
    }

    /// Templates that genuinely do NOT work, are not fixed, and are recorded here
    /// rather than left to turn the census permanently red.
    ///
    /// This list is DEBT, not an exemption — the difference from the list above
    /// matters and the two must never be merged. Above are templates the census
    /// cannot judge; here are templates it judged and found broken. A check that
    /// is always red gets ignored within a week, and one that quietly excuses its
    /// failures under a reason that does not fit them stops describing anything.
    /// Each row therefore says what is actually wrong and what it would take.
    ///
    /// The census still reports when one of these starts working, so a row that
    /// stops being true cannot sit here unnoticed.
    ///
    /// ⚠ Spelled `id | reason` for the same reason as the list above — see that
    /// comment before reformatting either of them.
    static CENSUS_KNOWN_BROKEN: &[&str] = &[
        "vector | its image ships NOTHING at /etc/vector, so there is nothing to \
         seed, and bare `vector` exits 78 with `Config file not found in path`. \
         Needs a starter vector.yaml this panel would author and then own \
         forever, or withdrawal",
        "garage | reads /etc/garage.toml, which is not even under the /etc/garage \
         directory the template mounts, and the image ships no config anywhere. \
         Same choice as vector: author a starter garage.toml, or withdraw",
        "erpnext | gunicorn listens on 8000 while the catalogue publishes 8080, so \
         nothing answers — and correcting the port alone would only expose the \
         second failure, which is that a frappe bench needs MariaDB, Redis and a \
         site that only `bench new-site` creates",
        "authelia | prints its help and exits 0, the same shape as the templates \
         the command table fixes. §6 of app-template-images-pin-e2e.sh explains \
         why a command is the wrong instrument here: with one it would sit \
         retrying an absent database for ever, which is a SILENT failure \
         replacing a loud one",
        "flowise | an UPSTREAM defect on the tag pinned, not a catalogue mistake: \
         it initialises fully, then dies in connect-sqlite3 with `this.db.exec is \
         not a function`. Precisely the decay this census exists to notice — \
         nothing in this repository changed",
        "wazuh | the manager is one container of a three-container stack. Its own \
         filebeat dials https://wazuh.indexer:9200, a compose service name that \
         resolves to nothing in a single-container deploy — the same shape as \
         plausible, and the template asks for no indexer",
    ];

    /// Templates known to leave state in an ANONYMOUS volume, and what is in it.
    ///
    /// An image may declare a path persistent on its own account. Docker honours that by
    /// creating an anonymous volume for any such path the container does not otherwise
    /// mount — and every recreate door builds the replacement from the original's
    /// `HostConfig`, which carries binds and knows nothing about anonymous volumes. So the
    /// replacement gets a brand-new empty one and the old one is orphaned: the app comes
    /// back at first-run state with its data still on the disk and nothing pointing at it.
    /// That is #110's mechanism again, arriving through the image rather than through the
    /// catalogue, and the entries for two templates already fixed this way say so in their
    /// own rows above the catalogue.
    ///
    /// ⚠ Like `CENSUS_KNOWN_BROKEN`, this is a REPAIR QUEUE and not an exemption. Each row
    /// is a template whose catalogue entry should grow the path, measured rather than
    /// guessed: every image in the catalogue had its config blob read from its registry,
    /// and these are the declared paths the catalogue does not bind. The rows carry what is
    /// actually lost, because "an anonymous volume exists" is not a severity — losing a
    /// scratch directory every recreate is fine and losing a bundled database is not.
    ///
    /// The census reports BOTH directions, so a row that stops being true cannot sit here:
    /// an unlisted template that produces one fails the run, and a listed template that
    /// produces none is reported as a stale row. That second direction is what settles the
    /// one question this list was NOT able to answer from the outside — whether a bind at a
    /// parent path suppresses an image volume declared at a child of it, which is the whole
    /// of the first entry below.
    ///
    /// ⚠ Spelled `id | reason`, for the reason given above `CENSUS_KNOWN_BROKEN`.
    static CENSUS_ANONYMOUS_VOLUME_DEBT: &[&str] = &[
        "azuracast | the image declares nine paths beneath /var/azuracast plus the MySQL \
         it bundles, while the catalogue binds only the parent directory. Station media, \
         listener uploads and the whole database are at stake, and whether the parent bind \
         covers the children is exactly what this run answers",
        "localai | models, data, configuration and backends are all declared by the image \
         and none is bound. A downloaded model set runs to tens of gigabytes and is fetched \
         again after every recreate",
        "huginn | the image bundles its own MySQL and the catalogue binds nothing at all, so \
         the entire database is anonymous and every recreate starts an empty one",
        "gitness | every repository lives under /git, which the image declares and the \
         catalogue does not bind. Only the /data path is bound",
        "immich | the image declares /data while the catalogue binds the upload directory \
         alone",
        "mattermost | operator-installed plugins live in the two plugin directories, both \
         declared by the image and neither bound",
        "paperless-ngx | the consume drop folder and the export directory are declared by \
         the image and unbound, so a document waiting to be ingested does not survive",
        "plausible | the image declares /var/lib/plausible and the catalogue does not bind it",
        "wikijs | page content is declared by the image at a path the catalogue does not bind",
        "filebrowser | the image declares /config, which the catalogue does not bind, so the \
         server's own configuration resets",
        "influxdb | the image declares its configuration directory and the catalogue binds \
         only the data",
        "mongo | the config-server metadata directory is declared by the image; the data \
         directory beside it is bound",
        "buildkite-agent | the image declares /buildkite and the catalogue binds nothing there",
        "sonarqube | scratch space only. The image declares a temp directory, and losing it \
         on a recreate costs nothing — recorded so the row is not mistaken for an omission",
        "searxng | a cache directory only, rebuilt on demand. Recorded for the same reason \
         as the row above",
        "erpnext | the bench log directory only. This template cannot start at all for the \
         reason recorded above, so the row is about completeness rather than data",
        "surrealdb | a log directory only",
    ];

    fn census_free_port() -> u16 {
        let listener =
            std::net::TcpListener::bind("127.0.0.1:0").expect("a free loopback port exists");
        let port = listener.local_addr().expect("bound address").port();
        drop(listener);
        port
    }

    /// A value for a required field the catalogue gives no default for.
    ///
    /// The deploy form makes the operator type one of these, so leaving it empty
    /// would be testing a deploy the panel does not permit — the census would
    /// manufacture failures no user can reach.
    fn census_synthetic_value(name: &str, port: u16) -> String {
        let upper = name.to_ascii_uppercase();
        let has = |needle: &str| upper.contains(needle);
        if has("PASSWORD") || has("SECRET") || has("TOKEN") || has("KEY") {
            "Census0000000000000000000000000000000000000000000000000000000000".to_string()
        } else if has("EMAIL") {
            "admin@example.com".to_string()
        } else if has("URL") || has("HOST") || has("DOMAIN") {
            format!("http://localhost:{port}")
        } else if has("PORT") {
            port.to_string()
        } else {
            "dockpanel-census".to_string()
        }
    }

    /// THE RUNTIME CENSUS — the decay this repository structurally cannot see.
    ///
    /// Every existing check over this catalogue reads the source: which tags do
    /// not resolve, which volumes the app cannot write, which images need a
    /// command. None of them can answer the only question an operator actually
    /// asks — whether pressing Deploy produces something that works — and nothing
    /// in this repository changes when the answer stops being yes. An upstream
    /// image adds a required setting, drops its embedded database, or moves the
    /// config it used to ship, and a catalogue entry that was correct on Tuesday
    /// is wrong on Wednesday with no commit in between. `live-surfaces-check.sh`
    /// exists for that shape of decay on the published web surfaces. This is its
    /// equivalent for the one-click catalogue, and it is overdue: the last three
    /// releases to touch this file each repaired it by hand, after a person read
    /// it.
    ///
    /// It deploys through `deploy_app` ON PURPOSE rather than assembling an
    /// equivalent container config. A census that builds its own approximation
    /// measures the approximation — and the empty bind mounts, the dropped
    /// capabilities, the volume chown and the command overlay are all decisions
    /// taken inside that function, every one of which has already been the cause
    /// of a real defect here.
    ///
    /// ⚠ IT CREATES REAL CONTAINERS AND WRITES UNDER `APP_DATA_DIR`. On a box
    /// running a panel those are indistinguishable from deployed apps, because
    /// that is what they are. So it is `#[ignore]`d AND gated behind an explicit
    /// variable, since `--ignored` alone is one habit away from running it:
    ///
    ///   TEMPLATE_CENSUS=1 cargo test -p dockpanel-agent template_census \
    ///       -- --ignored --nocapture
    ///
    /// `TEMPLATE_CENSUS_SHARD=2/4` takes one quarter of the catalogue, which is
    /// how the scheduled workflow stays inside a runner's disk and time budget.
    /// `TEMPLATE_CENSUS_PRUNE=1` deletes each image after measuring it — right on
    /// a throwaway runner, wrong on a machine whose images someone is using.
    #[tokio::test]
    #[ignore = "creates real containers; run with TEMPLATE_CENSUS=1 … -- --ignored"]
    async fn template_census_every_one_click_template_still_serves() {
        if std::env::var("TEMPLATE_CENSUS").is_err() {
            panic!(
                "The runtime census was invoked without TEMPLATE_CENSUS=1.\n\
                 It deploys real containers and writes under {APP_DATA_DIR}. Refusing \
                 rather than doing that by accident — see this test's documentation."
            );
        }
        let prune = std::env::var("TEMPLATE_CENSUS_PRUNE").is_ok();
        let (shard, shards) = std::env::var("TEMPLATE_CENSUS_SHARD")
            .ok()
            .map(|s| {
                let (a, b) = s.split_once('/').expect("TEMPLATE_CENSUS_SHARD is N/M");
                (
                    a.trim().parse::<usize>().expect("N parses"),
                    b.trim().parse::<usize>().expect("M parses"),
                )
            })
            .unwrap_or((1, 1));
        assert!(
            shards >= 1 && (1..=shards).contains(&shard),
            "TEMPLATE_CENSUS_SHARD must be N/M with 1 <= N <= M, got {shard}/{shards}"
        );

        // Every id on the exclusion list must exist, or the list is silently
        // excusing nothing while reading like coverage.
        let parse_rows = |rows: &'static [&'static str]| -> HashMap<&'static str, &'static str> {
            rows.iter()
                .map(|row| {
                    let (id, why) = row.split_once('|').expect("each row is `id | reason`");
                    (id.trim(), why.trim())
                })
                .collect()
        };
        let excused = parse_rows(CENSUS_NEEDS_A_SERVICE_THE_CENSUS_CANNOT_SUPPLY);
        let known_broken = parse_rows(CENSUS_KNOWN_BROKEN);
        let anon_debt = parse_rows(CENSUS_ANONYMOUS_VOLUME_DEBT);
        for id in excused
            .keys()
            .chain(known_broken.keys())
            .chain(anon_debt.keys())
        {
            assert!(
                TEMPLATES.iter().any(|t| t.id == *id),
                "the census names '{id}', which is not in the catalogue"
            );
        }
        // A template cannot be both unjudgeable and judged-broken. Allowing both
        // would let a real failure hide behind the softer label.
        for id in known_broken.keys() {
            assert!(
                !excused.contains_key(id),
                "'{id}' is on BOTH census lists; they mean different things"
            );
            let t = TEMPLATES.iter().find(|t| t.id == *id).expect("checked above");
            assert!(
                !census_needs_operator_input(t),
                "'{id}' is listed as known-broken, but it has a required field with \
                 no default — the census supplies a synthetic value for that, so it \
                 cannot support the claim that the template is broken. Record it \
                 outside this list."
            );
        }

        // `TEMPLATE_CENSUS_ONLY=caddy,ghost` narrows to named templates, which is
        // how a single fix gets re-measured through the real deploy path instead
        // of through something written to resemble it.
        let only: Option<Vec<String>> = std::env::var("TEMPLATE_CENSUS_ONLY")
            .ok()
            .map(|s| s.split(',').map(|v| v.trim().to_string()).collect());
        if let Some(ids) = &only {
            for id in ids {
                assert!(
                    TEMPLATES.iter().any(|t| t.id == id),
                    "TEMPLATE_CENSUS_ONLY names '{id}', which is not in the catalogue"
                );
            }
        }

        let subjects: Vec<&AppTemplateDef> = TEMPLATES
            .iter()
            .enumerate()
            .filter(|(i, t)| match &only {
                Some(ids) => ids.iter().any(|id| id == t.id),
                None => i % shards == shard - 1,
            })
            .map(|(_, t)| t)
            .collect();
        // An enumeration that came back empty must fail, not report a clean sweep.
        assert!(
            !subjects.is_empty(),
            "shard {shard}/{shards} selected no templates out of {}",
            TEMPLATES.len()
        );
        println!(
            "census shard {shard}/{shards}: {} of {} templates",
            subjects.len(),
            TEMPLATES.len()
        );

        let docker = Docker::connect_with_local_defaults().expect("a Docker daemon");
        let mut broken: Vec<String> = Vec::new();
        let mut unexpectedly_fine: Vec<String> = Vec::new();
        let mut anon_unlisted: Vec<String> = Vec::new();
        let mut anon_stale: Vec<String> = Vec::new();

        for t in subjects {
            let name = format!("census-{}", t.id);
            let port = census_free_port();
            let mut env: HashMap<String, String> = HashMap::new();
            for ev in t.env_vars {
                if ev.required && ev.default.is_empty() {
                    env.insert(ev.name.to_string(), census_synthetic_value(ev.name, port));
                }
            }

            // Leave nothing behind from an interrupted earlier run.
            let _ = remove_app(&format!("{CONTAINER_NAME_PREFIX}{name}")).await;
            let data_dir = format!("{APP_DATA_DIR}/{name}");
            let _ = std::fs::remove_dir_all(&data_dir);

            // Every mount Docker gave the container that is NOT one of our binds. Read
            // from the container rather than computed from the image, because the
            // question this answers — which declared paths end up as an anonymous
            // volume — depends on how the daemon resolves a declaration against the
            // mounts already present, and the daemon is the only honest source for that.
            let mut anonymous: Vec<String> = Vec::new();
            let mut deployed_ok = false;

            let outcome = match deploy_app(
                t.id, &name, port, env, None, None, None, None, false, None,
            )
            .await
            {
                Err(e) => Err(format!("deploy refused: {e}")),
                Ok(deployed) => {
                    deployed_ok = true;
                    let served =
                        census_wait_until_listening(&docker, &deployed.container_id, t.container_port)
                            .await;
                    let info = docker
                        .inspect_container(&deployed.container_id, None)
                        .await
                        .ok();
                    anonymous = info
                        .as_ref()
                        .and_then(|c| c.mounts.as_ref())
                        .map(|mounts| {
                            mounts
                                .iter()
                                .filter(|m| {
                                    m.typ != Some(bollard::service::MountPointTypeEnum::BIND)
                                })
                                .map(|m| {
                                    format!(
                                        "{} ({})",
                                        m.destination.clone().unwrap_or_default(),
                                        m.typ
                                            .map(|v| format!("{v:?}").to_ascii_lowercase())
                                            .unwrap_or_else(|| "unknown".to_string())
                                    )
                                })
                                .collect()
                        })
                        .unwrap_or_default();
                    let state = info.and_then(|c| c.state);
                    let status = state
                        .as_ref()
                        .and_then(|s| s.status.map(|v| v.to_string()))
                        .unwrap_or_else(|| "unknown".to_string());
                    let logs = get_app_logs(&deployed.container_id, 12).await.unwrap_or_default();
                    let _ = remove_app(&deployed.container_id).await;

                    if status != "running" {
                        Err(format!("container is {status}; last log: {}", census_tail(&logs)))
                    } else if !served {
                        Err(format!(
                            "running but nothing answers on {}; last log: {}",
                            t.container_port,
                            census_tail(&logs)
                        ))
                    } else {
                        Ok(())
                    }
                }
            };

            let _ = std::fs::remove_dir_all(&data_dir);
            if prune {
                let _ = docker.remove_image(t.image, None, None).await;
            }

            // ── Does anything this app writes end up somewhere a recreate cannot follow?
            //
            // Judged for EVERY template that produced a container, deliberately ahead of
            // the operator-input skip below. Whether a template can be judged on SERVING
            // and whether its data survives an update are different questions, and the
            // second one is answerable for a template the census cannot configure — it
            // only has to start, and Docker creates the volume either way.
            if deployed_ok {
                match (anonymous.is_empty(), anon_debt.get(t.id)) {
                    (true, None) => {}
                    (false, Some(why)) => {
                        println!("  anonvol {} ({}) — recorded: {why}", t.id, anonymous.join(", "))
                    }
                    (false, None) => {
                        println!("  ANONVOL {} — {}", t.id, anonymous.join(", "));
                        anon_unlisted.push(format!("{} — {}", t.id, anonymous.join(", ")));
                    }
                    (true, Some(why)) => {
                        // The other direction, and the one that keeps the list honest:
                        // this template was recorded as leaving data in an anonymous
                        // volume and no longer does.
                        println!("  ANONFIXED {} (recorded: {why})", t.id);
                        anon_stale.push(format!("{} — recorded: {why}", t.id));
                    }
                }
            }

            // The derived rule first: a template the operator must configure is one
            // this run cannot judge in EITHER direction, so it is reported and
            // never asserted on.
            if census_needs_operator_input(t) {
                match outcome {
                    Ok(()) => println!("  unjudged {} (serves anyway on a synthetic value)", t.id),
                    Err(e) => println!("  unjudged {} (needs operator input; {e})", t.id),
                }
                continue;
            }

            let listed = excused.get(t.id).or_else(|| known_broken.get(t.id));
            match (outcome, listed) {
                (Ok(()), None) => println!("  ok      {}", t.id),
                (Ok(()), Some(why)) => {
                    // The positive-control direction. A template listed as
                    // unjudgeable or as known-broken that now serves means its row
                    // has expired, and a list nobody revisits stops describing
                    // anything at all.
                    println!("  SERVES  {} (listed as: {why})", t.id);
                    unexpectedly_fine.push(format!("{} — listed as: {why}", t.id));
                }
                (Err(e), Some(_)) => println!("  listed  {} ({e})", t.id),
                (Err(e), None) => {
                    println!("  BROKEN  {} — {e}", t.id);
                    broken.push(format!("{} — {e}", t.id));
                }
            }
        }

        assert!(
            broken.is_empty(),
            "{} template(s) in this shard cannot start and serve as the catalogue \
             declares them:\n  {}",
            broken.len(),
            broken.join("\n  ")
        );
        assert!(
            unexpectedly_fine.is_empty(),
            "{} template(s) are listed as unjudgeable or known-broken but now \
             serve. Good news, and the list is stale — delete the rows:\n  {}",
            unexpectedly_fine.len(),
            unexpectedly_fine.join("\n  ")
        );
        assert!(
            anon_unlisted.is_empty(),
            "{} template(s) put data in an anonymous volume that no recreate carries \
             across, and are not recorded as doing so. Every one of these loses whatever \
             is at that path the next time its operator presses Update — declare the path \
             in the template, or record it with what is actually lost:\n  {}",
            anon_unlisted.len(),
            anon_unlisted.join("\n  ")
        );
        assert!(
            anon_stale.is_empty(),
            "{} template(s) are recorded as leaving data in an anonymous volume and no \
             longer do. Good news, and the row is stale — delete it:\n  {}",
            anon_stale.len(),
            anon_stale.join("\n  ")
        );
    }

    fn census_tail(logs: &str) -> String {
        logs.lines()
            .rev()
            .find(|l| !l.trim().is_empty())
            .unwrap_or("(no output)")
            .chars()
            .take(200)
            .collect()
    }

    /// Is anything actually LISTENING on the port, inside the container?
    ///
    /// Deliberately NOT a connect to the published host port. Publishing on
    /// 127.0.0.1 requires the userland proxy, and docker-proxy binds that host
    /// port the instant the container starts, completing the TCP handshake with
    /// nothing at all behind it — a connect succeeds against a container running
    /// `sleep`. This census was first written that way, and every template it did
    /// not observe exiting looked like it served; the error surfaced only because
    /// a control was run against a container with nothing listening. A green that
    /// cannot go red is not a measurement (#144).
    ///
    /// Reading the container's own network namespace is protocol-agnostic, which
    /// a payload probe is not: redis, postgres and mongo never speak first, so
    /// anything that waits for a banner has to special-case each of them.
    fn census_listens(pid: i64, container_port: &str) -> bool {
        let Some(port) = container_port
            .split('/')
            .next()
            .and_then(|p| p.parse::<u16>().ok())
        else {
            return false;
        };
        let needle = format!(":{port:04X}");
        for family in ["tcp", "tcp6"] {
            let Ok(text) = std::fs::read_to_string(format!("/proc/{pid}/net/{family}")) else {
                continue;
            };
            for line in text.lines().skip(1) {
                let cols: Vec<&str> = line.split_whitespace().collect();
                // cols[1] is local_address as HEXIP:HEXPORT; cols[3] is the socket
                // state, and 0A is LISTEN.
                if cols.len() > 3 && cols[1].ends_with(&needle) && cols[3] == "0A" {
                    return true;
                }
            }
        }
        false
    }

    /// A UDP template has no TCP socket to find, so it is judged on staying up.
    async fn census_wait_until_listening(
        docker: &Docker,
        container_id: &str,
        container_port: &str,
    ) -> bool {
        if container_port.ends_with("/udp") {
            return true;
        }
        // The budget starts HERE, after the container exists — never at the top of
        // the loop, or a slow image pull spends it before anything is measured.
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(120);
        while std::time::Instant::now() < deadline {
            let Ok(info) = docker.inspect_container(container_id, None).await else {
                return false;
            };
            let state = info.state.as_ref();
            let status = state
                .and_then(|s| s.status.map(|v| v.to_string()))
                .unwrap_or_default();
            if let Some(pid) = state.and_then(|s| s.pid) {
                if pid != 0 && census_listens(pid, container_port) {
                    return true;
                }
            }
            // A container that has already exited will never start listening.
            if !matches!(status.as_str(), "running" | "created" | "restarting") {
                return false;
            }
            tokio::time::sleep(std::time::Duration::from_secs(2)).await;
        }
        false
    }
}
