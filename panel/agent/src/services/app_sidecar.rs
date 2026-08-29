//! Paired-sidecar deployments.
//!
//! A one-click template can need a companion container it must never talk to
//! the outside world through directly — the motivating case is Dozzle (#125):
//! it needs live Docker API access, and `deploy_app` deliberately never
//! mounts the host socket into ANY one-click app, because that is a full
//! host-escape vector regardless of who was allowed to trigger the deploy.
//! The fix is not to loosen that policy — it is to give the app a sidecar
//! that holds the real socket and exposes only a filtered slice of its API
//! over a network nothing else can reach: the industry-standard
//! `docker-socket-proxy` pattern, which Dozzle's own docs recommend for
//! exactly this situation.
//!
//! Shape: a dedicated bridge network per app, mirroring `compose.rs`'s
//! per-stack network (`ensure_stack_network`/`remove_stack_network`) at
//! app scope instead of stack scope. The sidecar joins it under a fixed
//! alias and publishes no host port; the main container joins the SAME
//! network in place of the default bridge, so its existing host-port publish
//! keeps working unchanged (port publishing does not care which bridge-driver
//! network a container is on — `compose.rs`'s services already prove this),
//! and reaches the sidecar at `tcp://<alias>:<port>`. The sidecar is labelled
//! `dockpanel.sidecar_of=<app>`, never `dockpanel.app.template`, so
//! `list_deployed_apps`'s existing filter excludes it without any change —
//! the operator manages exactly one thing.

use bollard::Docker;
use bollard::container::{
    Config, CreateContainerOptions, NetworkingConfig, RemoveContainerOptions,
    RestartContainerOptions, StartContainerOptions, StopContainerOptions,
};
use bollard::image::CreateImageOptions;
use bollard::models::EndpointSettings;
use bollard::network::CreateNetworkOptions;
use std::collections::HashMap;
use tokio_stream::StreamExt;

/// What a template's sidecar is and how the main container finds it.
pub struct SidecarDef {
    pub image: &'static str,
    /// Network alias the main container reaches this sidecar by, on the
    /// private per-app network — safe to keep short and generic (e.g.
    /// "dockerproxy") because the network holds only this one app-pair.
    pub alias: &'static str,
    /// TCP port the sidecar listens on inside that network.
    pub port: u16,
    /// Env vars the sidecar itself needs, written explicitly and
    /// deny-by-default rather than left to the image's own defaults — see
    /// `docker_apps.rs`'s `DOZZLE_PROXY_ENV` for why every toggle is spelled
    /// out instead of omitted.
    pub proxy_env: &'static [(&'static str, &'static str)],
    /// Env var the panel injects into the MAIN container so it finds this
    /// sidecar. Never exposed as a user-editable `EnvVarDef` — pointing it
    /// anywhere else defeats the sandboxing this whole module exists for.
    pub client_env_var: &'static str,
}

impl SidecarDef {
    /// The `KEY=value` entry the main container's env gets, on every deploy
    /// AND every recreate — reasserted, not merely defaulted, so an operator
    /// editing the app's env cannot silently disconnect it by touching a row
    /// that looks like ordinary configuration but is actually wiring: there
    /// is no other value that resolves, because the alias only exists on
    /// this app's own private network.
    pub fn client_env_entry(&self) -> String {
        format!("{}=tcp://{}:{}", self.client_env_var, self.alias, self.port)
    }
}

/// The private bridge network an app's sidecar (if any) lives on. A prefix
/// distinct from `CONTAINER_NAME_PREFIX` ("dockpanel-app-"), so it can never
/// collide with an app's own container name.
pub fn network_name(app_name: &str) -> String {
    format!("dockpanel-appnet-{app_name}")
}

/// The sidecar's own container name — a namespace disjoint from
/// `dockpanel-app-*`, so no app name can ever collide with it (every app name
/// is already refused if it starts with that prefix; this one starts with a
/// different prefix entirely, not a suffix of it).
fn container_name(app_name: &str) -> String {
    format!("dockpanel-sidecar-{app_name}")
}

/// Create the app's private network if it does not already exist, deploy the
/// sidecar onto it, and return the network name for the caller to attach its
/// own container to.
///
/// On a failure after the sidecar container exists but fails to start, that
/// container is removed here (mirrors `deploy_app`'s own orphan cleanup). A
/// network created just before that failure is left in place — harmless, and
/// reused rather than recreated on a retry (see the existence check below).
pub async fn deploy(
    docker: &Docker,
    app_name: &str,
    sidecar: &SidecarDef,
) -> Result<String, String> {
    let network = network_name(app_name);

    if docker
        .inspect_network::<String>(&network, None)
        .await
        .is_err()
    {
        docker
            .create_network(CreateNetworkOptions {
                name: network.as_str(),
                driver: "bridge",
                labels: HashMap::from([("dockpanel.managed", "true")]),
                ..Default::default()
            })
            .await
            .map_err(|e| format!("Failed to create app network {network}: {e}"))?;
    }

    let pull_result = tokio::time::timeout(std::time::Duration::from_secs(300), async {
        let mut pull = docker.create_image(
            Some(CreateImageOptions {
                from_image: sidecar.image,
                ..Default::default()
            }),
            None,
            None,
        );
        while let Some(result) = pull.next().await {
            if let Err(e) = result {
                tracing::warn!("Sidecar image pull warning: {e}");
            }
        }
    })
    .await;
    if pull_result.is_err() {
        return Err(format!(
            "Sidecar image pull timed out for {}",
            sidecar.image
        ));
    }

    let mut endpoints = HashMap::new();
    endpoints.insert(
        network.clone(),
        EndpointSettings {
            aliases: Some(vec![sidecar.alias.to_string()]),
            ..Default::default()
        },
    );

    let name = container_name(app_name);
    let config = Config {
        image: Some(sidecar.image.to_string()),
        env: Some(
            sidecar
                .proxy_env
                .iter()
                .map(|(k, v)| format!("{k}={v}"))
                .collect(),
        ),
        host_config: Some(bollard::service::HostConfig {
            binds: Some(vec![
                "/var/run/docker.sock:/var/run/docker.sock:ro".to_string(),
            ]),
            restart_policy: Some(bollard::service::RestartPolicy {
                name: Some(bollard::service::RestartPolicyNameEnum::UNLESS_STOPPED),
                ..Default::default()
            }),
            security_opt: Some(vec!["no-new-privileges:true".to_string()]),
            // No cap_add: this container touches no bind-mounted app data of
            // its own, so none of the CHOWN/SETUID/DAC_OVERRIDE grants the
            // main container's hardening needs apply here — it only opens a
            // unix socket and serves HTTP over the private network above.
            cap_drop: Some(vec!["ALL".to_string()]),
            ..Default::default()
        }),
        networking_config: Some(NetworkingConfig {
            endpoints_config: endpoints,
        }),
        labels: Some(HashMap::from([
            ("dockpanel.managed".to_string(), "true".to_string()),
            ("dockpanel.sidecar_of".to_string(), app_name.to_string()),
        ])),
        ..Default::default()
    };

    let container = docker
        .create_container(
            Some(CreateContainerOptions {
                name: name.as_str(),
                platform: None,
            }),
            config,
        )
        .await
        .map_err(|e| format!("Failed to create sidecar container: {e}"))?;

    if let Err(e) = docker
        .start_container(&container.id, None::<StartContainerOptions<String>>)
        .await
    {
        let _ = docker
            .remove_container(
                &container.id,
                Some(RemoveContainerOptions {
                    v: true,
                    force: true,
                    ..Default::default()
                }),
            )
            .await;
        return Err(format!("Failed to start sidecar container: {e}"));
    }

    Ok(network)
}

/// The networking config that attaches a container to `app_name`'s sidecar
/// network under its own alias. Used by the initial deploy and by every
/// recreate path, so an image/env update can never silently drop the app
/// back onto the default bridge and strand it unable to resolve its sidecar.
pub fn attach_config(app_name: &str) -> NetworkingConfig<String> {
    let mut endpoints = HashMap::new();
    endpoints.insert(
        network_name(app_name),
        EndpointSettings {
            aliases: Some(vec![app_name.to_string()]),
            ..Default::default()
        },
    );
    NetworkingConfig {
        endpoints_config: endpoints,
    }
}

/// The app's sidecar container id, if its template has one and it still exists.
pub async fn find(docker: &Docker, app_name: &str) -> Option<String> {
    docker
        .inspect_container(&container_name(app_name), None)
        .await
        .ok()
        .and_then(|c| c.id)
}

/// Stop and remove an app's sidecar and its private network. Best-effort by
/// design, mirroring `compose::remove_stack_network`: a network Docker still
/// sees endpoints on refuses removal (the correct answer when something
/// unexpected is still attached), and an app whose template has no sidecar
/// simply has nothing here to remove.
pub async fn teardown(app_name: &str) {
    let Ok(docker) = Docker::connect_with_local_defaults() else {
        return;
    };

    let name = container_name(app_name);
    if docker.inspect_container(&name, None).await.is_ok() {
        docker
            .stop_container(&name, Some(StopContainerOptions { t: 10 }))
            .await
            .ok();
        if let Err(e) = docker
            .remove_container(
                &name,
                Some(RemoveContainerOptions {
                    v: true,
                    force: true,
                    ..Default::default()
                }),
            )
            .await
        {
            tracing::warn!("Could not remove sidecar container {name}: {e}");
        }
    }

    let network = network_name(app_name);
    match docker.inspect_network::<String>(&network, None).await {
        Ok(info) => {
            let ours = info
                .labels
                .as_ref()
                .and_then(|l| l.get("dockpanel.managed"))
                .map(|v| v == "true")
                .unwrap_or(false);
            if !ours {
                tracing::warn!("Not removing network {network}: no dockpanel.managed label");
                return;
            }
        }
        Err(_) => return,
    }
    if let Err(e) = docker.remove_network(&network).await {
        tracing::warn!("Could not remove app network {network}: {e}");
    }
}

/// Mirror a stop onto this app's sidecar, if it has one. Best-effort: the
/// operator asked to stop their app, and that already succeeded by the time
/// this runs — a sidecar that fails to stop is logged, never surfaced as the
/// action's own error.
pub async fn mirror_stop(docker: &Docker, app_name: &str) {
    if let Some(id) = find(docker, app_name).await {
        if let Err(e) = docker
            .stop_container(&id, Some(StopContainerOptions { t: 10 }))
            .await
        {
            tracing::warn!("Could not stop sidecar for app {app_name}: {e}");
        }
    }
}

/// Mirror a start onto this app's sidecar, if it has one — run BEFORE the
/// main container starts, so the sidecar is already reachable instead of
/// making the app's first connection attempt race a cold start.
pub async fn mirror_start(docker: &Docker, app_name: &str) {
    if let Some(id) = find(docker, app_name).await {
        if let Err(e) = docker
            .start_container(&id, None::<StartContainerOptions<String>>)
            .await
        {
            tracing::warn!("Could not start sidecar for app {app_name}: {e}");
        }
    }
}

/// Mirror a restart onto this app's sidecar, if it has one.
pub async fn mirror_restart(docker: &Docker, app_name: &str) {
    if let Some(id) = find(docker, app_name).await {
        if let Err(e) = docker
            .restart_container(&id, Some(RestartContainerOptions { t: 10 }))
            .await
        {
            tracing::warn!("Could not restart sidecar for app {app_name}: {e}");
        }
    }
}
