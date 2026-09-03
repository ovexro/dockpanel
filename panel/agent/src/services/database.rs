use crate::safe_cmd::{safe_command, DockerEnvFile};
use bollard::container::{
    Config, CreateContainerOptions, ListContainersOptions, RemoveContainerOptions,
    StartContainerOptions, StopContainerOptions,
};
use bollard::Docker;
use std::collections::HashMap;

const DB_NETWORK: &str = "dockpanel-db";

#[derive(serde::Serialize)]
pub struct DbContainer {
    pub container_id: String,
    pub name: String,
    pub port: u16,
    pub engine: String,
    pub status: String,
}

/// Create a database container (MySQL or PostgreSQL).
pub async fn create_database(
    name: &str,
    engine: &str,
    password: &str,
    port: u16,
) -> Result<DbContainer, String> {
    let docker =
        Docker::connect_with_local_defaults().map_err(|e| format!("Docker connect failed: {e}"))?;

    // Ensure the shared DB bridge exists AND has inter-container communication
    // disabled (H2: block cross-tenant lateral movement between DB containers).
    ensure_network(&docker).await?;

    // M1: for postgres the container's bootstrap superuser (`postgres`) gets a
    // random, immediately-discarded password. The tenant NEVER connects as the
    // superuser — a separate NON-superuser owner role (named {name}) is provisioned
    // after start. Unused for MariaDB (which is already DB-scoped/non-root).
    let admin_password = uuid::Uuid::new_v4().to_string().replace('-', "");

    let (image, env, container_port) = match engine {
        "mysql" | "mariadb" => (
            "mariadb:11",
            vec![
                format!("MYSQL_DATABASE={name}"),
                format!("MYSQL_USER={name}"),
                format!("MYSQL_PASSWORD={password}"),
                "MYSQL_RANDOM_ROOT_PASSWORD=yes".to_string(),
            ],
            "3306/tcp",
        ),
        _ => (
            "postgres:16-alpine",
            vec![
                format!("POSTGRES_DB={name}"),
                // NOTE: POSTGRES_USER is deliberately NOT set to {name} anymore — that
                // bootstrapped the tenant as the cluster SUPERUSER (M1: COPY..TO PROGRAM
                // in-container RCE foothold). The image default superuser `postgres` is
                // kept with this random, discarded password; the real tenant role is
                // created NON-superuser by provision_postgres_tenant_role() below.
                format!("POSTGRES_PASSWORD={admin_password}"),
            ],
            "5432/tcp",
        ),
    };

    // Pull image if needed
    use bollard::image::CreateImageOptions;
    use tokio_stream::StreamExt;
    let mut pull = docker.create_image(
        Some(CreateImageOptions {
            from_image: image,
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

    let container_name = format!("dockpanel-db-{name}");

    let mut port_bindings = HashMap::new();
    port_bindings.insert(
        container_port.to_string(),
        Some(vec![bollard::service::PortBinding {
            host_ip: Some("127.0.0.1".to_string()),
            host_port: Some(port.to_string()),
        }]),
    );

    let host_config = bollard::service::HostConfig {
        port_bindings: Some(port_bindings),
        network_mode: Some(DB_NETWORK.to_string()),
        restart_policy: Some(bollard::service::RestartPolicy {
            name: Some(bollard::service::RestartPolicyNameEnum::UNLESS_STOPPED),
            ..Default::default()
        }),
        memory: Some(256 * 1024 * 1024), // 256MB
        // Cap CPU at 2 cores (like app containers, which set cpu_period/cpu_quota) so a
        // heavy query/dump/restore on one tenant's DB container cannot starve co-tenant
        // containers on the shared host.
        cpu_period: Some(100_000),
        cpu_quota: Some(200_000),
        ..Default::default()
    };

    let mut exposed_ports = HashMap::new();
    exposed_ports.insert(container_port.to_string(), HashMap::new());

    let config = Config {
        image: Some(image.to_string()),
        env: Some(env.clone()),
        exposed_ports: Some(exposed_ports),
        host_config: Some(host_config),
        labels: Some(HashMap::from([
            ("dockpanel.managed".to_string(), "true".to_string()),
            ("dockpanel.db.name".to_string(), name.to_string()),
            ("dockpanel.db.engine".to_string(), engine.to_string()),
        ])),
        ..Default::default()
    };

    let container = docker
        .create_container(
            Some(CreateContainerOptions {
                name: container_name.as_str(),
                platform: None,
            }),
            config,
        )
        .await
        .map_err(|e| format!("Failed to create container: {e}"))?;

    if let Err(e) = docker
        .start_container(&container.id, None::<StartContainerOptions<String>>)
        .await
    {
        // This engine declares its data directory as an image VOLUME and the panel
        // binds nothing here, so the container carries an anonymous volume. It never
        // started, so that volume is empty — and the sibling teardown a few lines
        // below (role provisioning failed) already takes it. Match it.
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
        return Err(format!("Failed to start container: {e}"));
    }

    // M1: postgres tenant now runs as a NON-superuser owner. Provision that role now
    // that the container is up. If it fails the container has no usable tenant login,
    // so tear it down and surface the error (backend create() then deletes its row +
    // releases the reseller slot). MariaDB is already DB-scoped/non-root — skip.
    if !matches!(engine, "mysql" | "mariadb") {
        if let Err(e) =
            provision_postgres_tenant_role(&container_name, name, &admin_password, password).await
        {
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
            return Err(e);
        }
    }

    tracing::info!("Database container created: {container_name} ({engine}, port {port})");

    Ok(DbContainer {
        container_id: container.id,
        name: container_name,
        port,
        engine: engine.to_string(),
        status: "running".to_string(),
    })
}

/// Remove a database container.
pub async fn remove_database(container_id: &str) -> Result<(), String> {
    let docker =
        Docker::connect_with_local_defaults().map_err(|e| format!("Docker connect failed: {e}"))?;

    // Stop first
    docker
        .stop_container(container_id, Some(StopContainerOptions { t: 10 }))
        .await
        .ok(); // Ignore if already stopped

    docker
        .remove_container(
            container_id,
            Some(RemoveContainerOptions {
                v: true, // remove volumes
                force: true,
                ..Default::default()
            }),
        )
        .await
        .map_err(|e| format!("Failed to remove container: {e}"))?;

    tracing::info!("Database container removed: {container_id}");
    Ok(())
}

/// List all DockPanel-managed database containers.
pub async fn list_databases() -> Result<Vec<DbContainer>, String> {
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

    let dbs = containers
        .into_iter()
        .filter_map(|c| {
            let labels = c.labels.as_ref()?;
            let _db_name = labels.get("dockpanel.db.name")?;
            let engine = labels.get("dockpanel.db.engine")?;
            let id = c.id.as_ref()?;

            let port = c
                .ports
                .as_ref()
                .and_then(|ports| ports.first())
                .and_then(|p| p.public_port)
                .unwrap_or(0) as u16;

            let status = c.state.unwrap_or_default();
            let name = c
                .names
                .as_ref()
                .and_then(|n| n.first())
                .map(|n| n.trim_start_matches('/').to_string())
                .unwrap_or_default();

            Some(DbContainer {
                container_id: id.clone(),
                name,
                port,
                engine: engine.clone(),
                status,
            })
        })
        .collect();

    Ok(dbs)
}

/// Result of a SQL query execution.
#[derive(serde::Serialize)]
pub struct QueryResult {
    pub columns: Vec<String>,
    pub rows: Vec<Vec<String>>,
    pub row_count: usize,
    pub execution_time_ms: u64,
    pub truncated: bool,
}

const MAX_ROWS: usize = 1000;
const QUERY_TIMEOUT_SECS: u64 = 15;
const MAX_OUTPUT_BYTES: usize = 5 * 1024 * 1024;

/// Execute a SQL query inside a database container via docker exec.
pub async fn execute_query(
    container: &str,
    engine: &str,
    user: &str,
    password: &str,
    database: &str,
    sql: &str,
) -> Result<QueryResult, String> {
    let start = std::time::Instant::now();

    // Build the docker-exec command. kill_on_drop ensures a timed-out (dropped) future
    // actually terminates the docker exec child rather than leaking an orphaned process.
    //
    // The credential goes in via `--env-file`, NOT `-e KEY=value`: a `-e`
    // argument is literal `docker` argv, world-readable via `ps`/`/proc/<pid>/
    // cmdline` for the life of the child (no `hidepid=` assumed). See
    // `DockerEnvFile` — `dockpanel-fanout` s445 completeness critic.
    let env_file = match engine {
        "mysql" | "mariadb" => DockerEnvFile::new(&[("MYSQL_PWD", password)]),
        _ => DockerEnvFile::new(&[("PGPASSWORD", password)]),
    }
    .map_err(|e| format!("Failed to prepare credentials: {e}"))?;
    let mut cmd = safe_command("docker");
    match engine {
        "mysql" | "mariadb" => {
            cmd.arg("exec")
                .arg("--env-file").arg(env_file.path())
                .arg(container)
                .arg("mariadb").arg("-u").arg(user).arg(database)
                .arg("-e").arg(sql).arg("--batch").arg("--column-names");
        }
        _ => {
            cmd.arg("exec")
                .arg("--env-file").arg(env_file.path())
                .arg(container)
                .arg("psql").arg("-U").arg(user).arg("-d").arg(database)
                .arg("-c").arg(sql).arg("--csv");
        }
    }
    cmd.stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .kill_on_drop(true);

    // Stream stdout with a HARD cap (MAX_OUTPUT_BYTES) instead of buffering the whole
    // result set with .output(). A tenant could otherwise stream hundreds of MB into the
    // agent's memory and OOM the shared agent (MemoryMax). stderr is drained concurrently
    // (bounded) to avoid a pipe-full deadlock.
    let run = async {
        use tokio::io::AsyncReadExt;
        let mut child = cmd
            .spawn()
            .map_err(|e| format!("Failed to execute docker exec: {e}"))?;
        let mut child_stdout = child.stdout.take().ok_or("Failed to capture stdout")?;
        let mut child_stderr = child.stderr.take().ok_or("Failed to capture stderr")?;

        let stdout_fut = async {
            let mut buf: Vec<u8> = Vec::with_capacity(64 * 1024);
            let mut chunk = vec![0u8; 64 * 1024];
            let mut overflow = false;
            loop {
                let n = match child_stdout.read(&mut chunk).await {
                    Ok(0) => break,
                    Ok(n) => n,
                    Err(e) => return Err(format!("read error: {e}")),
                };
                if buf.len() + n > MAX_OUTPUT_BYTES {
                    overflow = true;
                    break;
                }
                buf.extend_from_slice(&chunk[..n]);
            }
            Ok::<(Vec<u8>, bool), String>((buf, overflow))
        };
        let stderr_fut = async {
            let mut buf = Vec::new();
            let _ = (&mut child_stderr).take(64 * 1024).read_to_end(&mut buf).await;
            buf
        };
        let (out_res, err_buf) = tokio::join!(stdout_fut, stderr_fut);
        let (out_buf, overflow) = out_res?;
        if overflow {
            let _ = child.start_kill();
            return Err(format!(
                "Query output too large (max {} MB)",
                MAX_OUTPUT_BYTES / (1024 * 1024)
            ));
        }
        let status = child
            .wait()
            .await
            .map_err(|e| format!("docker exec wait error: {e}"))?;
        Ok::<_, String>((status, out_buf, err_buf))
    };

    let (status, out_bytes, err_bytes) = match tokio::time::timeout(
        std::time::Duration::from_secs(QUERY_TIMEOUT_SECS),
        run,
    )
    .await
    {
        Ok(Ok(v)) => v,
        Ok(Err(e)) => return Err(e),
        Err(_) => return Err(format!("Query timed out ({QUERY_TIMEOUT_SECS}s limit)")),
    };

    let elapsed = start.elapsed().as_millis() as u64;

    if !status.success() {
        let stderr = String::from_utf8_lossy(&err_bytes);
        let stdout = String::from_utf8_lossy(&out_bytes);
        let msg = if stderr.trim().is_empty() {
            stdout.trim().to_string()
        } else {
            stderr.trim().to_string()
        };
        return Err(msg);
    }

    let stdout = String::from_utf8_lossy(&out_bytes);

    let (columns, mut rows) = match engine {
        "mysql" | "mariadb" => parse_tsv(&stdout),
        _ => parse_csv(&stdout),
    };

    let truncated = rows.len() > MAX_ROWS;
    if truncated {
        rows.truncate(MAX_ROWS);
    }
    let row_count = rows.len();

    Ok(QueryResult {
        columns,
        rows,
        row_count,
        execution_time_ms: elapsed,
        truncated,
    })
}

/// Parse tab-separated output (MariaDB --batch mode).
fn parse_tsv(output: &str) -> (Vec<String>, Vec<Vec<String>>) {
    let mut lines = output.lines();
    let columns: Vec<String> = match lines.next() {
        Some(header) if !header.is_empty() => header.split('\t').map(|s| s.to_string()).collect(),
        _ => return (vec![], vec![]),
    };
    let rows: Vec<Vec<String>> = lines
        .filter(|line| !line.is_empty())
        .map(|line| line.split('\t').map(|s| s.to_string()).collect())
        .collect();
    (columns, rows)
}

/// Parse CSV output (PostgreSQL --csv mode). Handles quoted fields with embedded
/// commas, newlines, and escaped double-quotes per RFC 4180.
fn parse_csv(output: &str) -> (Vec<String>, Vec<Vec<String>>) {
    let mut records: Vec<Vec<String>> = Vec::new();
    let mut field = String::new();
    let mut record: Vec<String> = Vec::new();
    let mut in_quotes = false;
    let mut chars = output.chars().peekable();

    while let Some(c) = chars.next() {
        if in_quotes {
            if c == '"' {
                if chars.peek() == Some(&'"') {
                    chars.next();
                    field.push('"');
                } else {
                    in_quotes = false;
                }
            } else {
                field.push(c);
            }
        } else {
            match c {
                '"' => in_quotes = true,
                ',' => {
                    record.push(std::mem::take(&mut field));
                }
                '\n' => {
                    record.push(std::mem::take(&mut field));
                    if !record.is_empty() {
                        records.push(std::mem::take(&mut record));
                    }
                }
                '\r' => {} // skip CR
                _ => field.push(c),
            }
        }
    }

    // Last field/record
    if !field.is_empty() || !record.is_empty() {
        record.push(field);
        if !record.iter().all(String::is_empty) || record.len() > 1 {
            records.push(record);
        }
    }

    if records.is_empty() {
        return (vec![], vec![]);
    }

    // Check if the output is a PostgreSQL command tag (INSERT/UPDATE/DELETE/etc.)
    // rather than actual CSV data — these have no commas and a single "column"
    if records.len() == 1 && records[0].len() == 1 {
        let tag = &records[0][0];
        if tag.starts_with("INSERT")
            || tag.starts_with("UPDATE")
            || tag.starts_with("DELETE")
            || tag.starts_with("CREATE")
            || tag.starts_with("ALTER")
            || tag.starts_with("DROP")
            || tag.starts_with("TRUNCATE")
            || tag.starts_with("GRANT")
            || tag.starts_with("REVOKE")
        {
            return (vec![], vec![]);
        }
    }

    let columns = records.remove(0);
    (columns, records)
}

/// Escape a value for use inside a single-quoted SQL string literal, MariaDB rules.
///
/// The two engines need DIFFERENT escaping, which is why this is not one shared
/// helper: MariaDB treats `\` as an escape character inside string literals, so a
/// literal backslash must be doubled. Doing the same to PostgreSQL would STORE two
/// backslashes, because it runs with `standard_conforming_strings = on` (the default
/// since 9.1) and `\` there is an ordinary character. Backslashes are doubled first
/// so the `''` produced by the quote pass is never re-escaped.
fn mysql_string_escape(s: &str) -> String {
    s.replace('\\', "\\\\").replace('\'', "''")
}

/// Escape a value for use inside a single-quoted SQL string literal, PostgreSQL rules.
/// See [`mysql_string_escape`] for why the backslash is deliberately left alone.
fn pg_string_escape(s: &str) -> String {
    s.replace('\'', "''")
}

/// Reset the password for a database user inside a running container.
///
/// Both engines authenticate AS THE TENANT and let the server change its own
/// account, so `old_password` is the thing that decides whether the reset is
/// allowed to happen at all.
///
/// ⚠ The MariaDB arm used to connect as the container's root account with no
/// credential, on the stated premise that root can authenticate over the unix
/// socket inside the container. That premise was never true of a container THIS
/// FILE creates: `create_database` asks the image for a random root password,
/// which gives `root@localhost` a random `mysql_native_password` and does NOT
/// enable the `unix_socket` plugin. Every MariaDB/MySQL password reset therefore
/// died on `ERROR 1045 … Access denied for user 'root'@'localhost'` and surfaced
/// to the operator as a 500. Measured on `mariadb:11` with exactly the env
/// `create_database` sets. `SET PASSWORD` needs no privilege beyond being the
/// account, and this is the same connection shape `execute_query` already uses in
/// production.
///
/// The exact flag and env spellings are deliberately NOT written here:
/// `db-credential-auth-pin-e2e.sh` §B greps this tree for them, and a pin that
/// matches the comment narrating it is no pin at all.
pub async fn reset_password(
    container: &str,
    engine: &str,
    user: &str,
    old_password: &str,
    new_password: &str,
) -> Result<(), String> {
    let output = match engine {
        "mysql" | "mariadb" => {
            // `SET PASSWORD` with no FOR clause targets CURRENT_USER(), which is the
            // account we authenticated as — the same `'{user}'@'%'` row the old
            // ALTER USER named. Verified: a socket connection reports `user@localhost`
            // for USER() while CURRENT_USER() resolves to `user@%`.
            let sql = format!(
                "SET PASSWORD = PASSWORD('{}');",
                mysql_string_escape(new_password),
            );
            let env_file = match DockerEnvFile::new(&[("MYSQL_PWD", old_password)]) {
                Ok(f) => f,
                Err(e) => return Err(format!("Failed to prepare credentials: {e}")),
            };
            tokio::time::timeout(
                std::time::Duration::from_secs(QUERY_TIMEOUT_SECS),
                safe_command("docker")
                    .arg("exec")
                    .arg("--env-file")
                    .arg(env_file.path())
                    .arg(container)
                    .arg("mariadb")
                    .arg("-u")
                    .arg(user)
                    .arg("-e")
                    .arg(&sql)
                    .output(),
            )
            .await
        }
        _ => {
            // PostgreSQL: connect as the tenant and ALTER its own role.
            //
            // ⚠ Honest scope limit, measured rather than assumed: the official
            // `postgres:16-alpine` image ships a pg_hba.conf whose `local`, `127.0.0.1/32`
            // and `::1/128` lines are all `trust` — only non-loopback hosts reach the
            // `scram-sha-256` line. Since this runs INSIDE the container, PGPASSWORD is
            // accepted but never checked. So `old_password` is real authority on the
            // MariaDB arm and decorative on this one. It is deliberately NOT "fixed" by
            // forcing a non-loopback connection: the tenant boundary for this route is
            // the panel's ownership check in `databases::reset_password`, and making the
            // agent enforce it here would turn a working feature into one that breaks
            // permanently the moment a tenant changes their own password through the SQL
            // console (the panel has no recovery path — the bootstrap superuser password
            // is random and discarded by `create_database`).
            let sql = format!(
                "ALTER USER \"{}\" WITH PASSWORD '{}';",
                user.replace('"', "\"\""),
                pg_string_escape(new_password),
            );
            let env_file = match DockerEnvFile::new(&[("PGPASSWORD", old_password)]) {
                Ok(f) => f,
                Err(e) => return Err(format!("Failed to prepare credentials: {e}")),
            };
            tokio::time::timeout(
                std::time::Duration::from_secs(QUERY_TIMEOUT_SECS),
                safe_command("docker")
                    .arg("exec")
                    .arg("--env-file")
                    .arg(env_file.path())
                    .arg(container)
                    .arg("psql")
                    .arg("-U")
                    .arg(user)
                    .arg("-d")
                    .arg(user)
                    .arg("-c")
                    .arg(&sql)
                    .output(),
            )
            .await
        }
    };

    let output = match output {
        Ok(Ok(o)) => o,
        Ok(Err(e)) => return Err(format!("Failed to execute docker exec: {e}")),
        Err(_) => return Err(format!("Password reset timed out ({QUERY_TIMEOUT_SECS}s limit)")),
    };

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        let stderr = stderr.trim();
        // Name the one failure an operator cannot otherwise diagnose. The reset now
        // authenticates as the tenant, so a rejected login means the panel's stored
        // credential is no longer the database's real one — which a tenant can cause
        // themselves by running SET PASSWORD/ALTER USER in the SQL console. The generic
        // "Access denied" gives no hint that the panel, not the request, is the stale party.
        if stderr.contains("Access denied") || stderr.contains("password authentication failed") {
            return Err(format!(
                "Password reset failed: the database rejected the current password stored by \
                 the panel, so the two are out of sync (this happens if the password was \
                 changed outside the panel, e.g. via the SQL console). Original error: {stderr}"
            ));
        }
        return Err(format!("Password reset failed: {stderr}"));
    }

    tracing::info!("Database password reset for user '{user}' in container '{container}'");
    Ok(())
}

/// After a fresh postgres container starts, provision the NON-superuser tenant owner
/// role (M1). Runs as the bootstrap superuser `postgres` over the in-container socket;
/// waits for readiness first (initdb + POSTGRES_DB creation take a moment).
async fn provision_postgres_tenant_role(
    container: &str,
    db_name: &str,
    admin_password: &str,
    tenant_password: &str,
) -> Result<(), String> {
    // One env-file for the whole function: `admin_password` doesn't change
    // across the readiness loop or the provisioning call below.
    let env_file = DockerEnvFile::new(&[("PGPASSWORD", admin_password)])
        .map_err(|e| format!("Failed to prepare credentials: {e}"))?;

    // Wait until postgres accepts connections to the tenant DB (mirrors backup_verify).
    let mut ready = false;
    for _ in 0..30 {
        tokio::time::sleep(std::time::Duration::from_secs(1)).await;
        let ok = safe_command("docker")
            .arg("exec")
            .arg("--env-file")
            .arg(env_file.path())
            .arg(container)
            .arg("psql")
            .arg("-U")
            .arg("postgres")
            .arg("-d")
            .arg(db_name)
            .arg("-c")
            .arg("SELECT 1")
            .output()
            .await
            .map(|o| o.status.success())
            .unwrap_or(false);
        if ok {
            ready = true;
            break;
        }
    }
    if !ready {
        return Err("PostgreSQL did not become ready within 30s".to_string());
    }

    let sql = tenant_role_sql(db_name, tenant_password);
    let output = safe_command("docker")
        .arg("exec")
        .arg("--env-file")
        .arg(env_file.path())
        .arg(container)
        .arg("psql")
        .arg("-U")
        .arg("postgres")
        .arg("-d")
        .arg(db_name)
        .arg("-v")
        .arg("ON_ERROR_STOP=1")
        .arg("-c")
        .arg(&sql[0])
        .arg("-c")
        .arg(&sql[1])
        .arg("-c")
        .arg(&sql[2])
        .output()
        .await
        .map_err(|e| format!("Failed to provision tenant role: {e}"))?;
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(format!("Failed to provision tenant role: {}", stderr.trim()));
    }
    Ok(())
}

/// The three statements that hand a fresh postgres DB to a NON-superuser tenant owner
/// named `db_name`: create the role (NOSUPERUSER so it cannot COPY..TO PROGRAM /
/// pg_read_server_files — the M1 RCE foothold), give it the database, and give it the
/// `public` schema (PG15+ no longer grants PUBLIC CREATE, so DB ownership alone would
/// not let the tenant create tables). Run as the admin superuser, connected to the DB.
fn tenant_role_sql(db_name: &str, password: &str) -> [String; 3] {
    // Escape single quotes in the password string literal. Identifiers are validated to
    // [A-Za-z0-9_] upstream, so the quoted role/db name needs no escaping.
    let pw = password.replace('\'', "''");
    [
        format!(
            "CREATE ROLE \"{db_name}\" LOGIN PASSWORD '{pw}' NOSUPERUSER NOCREATEDB NOCREATEROLE;"
        ),
        format!("ALTER DATABASE \"{db_name}\" OWNER TO \"{db_name}\";"),
        format!("ALTER SCHEMA public OWNER TO \"{db_name}\";"),
    ]
}

/// Ensure the shared `dockpanel-db` bridge exists AND has inter-container communication
/// disabled. No consumer connects to a managed DB over this network (every consumer uses
/// the 127.0.0.1-published host port), so ICC can be off — which blocks a compromised
/// tenant DB container from reaching sibling tenants' DB containers (H2 lateral movement).
///
/// s242 set `enable_icc=false` only when the network was FIRST created, so installs whose
/// `dockpanel-db` predates that keep ICC on (and even NEW DBs there join the ICC-on net) —
/// the gap that made the s242 mitigation partial. This reconciles a legacy ICC-on network
/// to ICC=false (one-time, idempotent) since ICC is a create-time, in-place-immutable option.
async fn ensure_network(docker: &Docker) -> Result<(), String> {
    match docker.inspect_network::<String>(DB_NETWORK, None).await {
        Ok(net) => {
            let icc_off = net
                .options
                .as_ref()
                .and_then(|o| o.get("com.docker.network.bridge.enable_icc"))
                .map(|v| v == "false")
                .unwrap_or(false);
            if icc_off {
                return Ok(());
            }
            // Recreating the network requires disconnecting attached DB containers first.
            let attached: Vec<String> = net
                .containers
                .as_ref()
                .map(|c| c.keys().cloned().collect())
                .unwrap_or_default();
            reconcile_network_icc(docker, &attached).await
        }
        Err(_) => create_db_network(docker).await,
    }
}

/// Create the shared DB bridge with inter-container communication disabled.
async fn create_db_network(docker: &Docker) -> Result<(), String> {
    create_db_network_at(docker, DB_NETWORK).await
}

async fn create_db_network_at(docker: &Docker, network: &str) -> Result<(), String> {
    use bollard::network::CreateNetworkOptions;
    docker
        .create_network(CreateNetworkOptions {
            name: network,
            driver: "bridge",
            options: HashMap::from([("com.docker.network.bridge.enable_icc", "false")]),
            ..Default::default()
        })
        .await
        .map_err(|e| format!("Failed to create network: {e}"))?;
    tracing::info!("Created Docker network: {network} (enable_icc=false)");
    Ok(())
}

/// Recreate `dockpanel-db` with ICC disabled, reconnecting every attached DB container.
/// One-time reconcile for pre-s242 installs whose network still has ICC enabled.
async fn reconcile_network_icc(docker: &Docker, attached: &[String]) -> Result<(), String> {
    reconcile_network_icc_at(docker, DB_NETWORK, attached).await
}

async fn reconcile_network_icc_at(
    docker: &Docker,
    network: &str,
    attached: &[String],
) -> Result<(), String> {
    use bollard::network::{ConnectNetworkOptions, DisconnectNetworkOptions};
    // Disconnect so the network can be removed. Published 127.0.0.1 ports are
    // re-established from the container's HostConfig on reconnect (lab-verified).
    for cid in attached {
        let _ = docker
            .disconnect_network(
                network,
                DisconnectNetworkOptions {
                    container: cid.as_str(),
                    force: true,
                },
            )
            .await;
    }
    docker
        .remove_network(network)
        .await
        .map_err(|e| format!("Failed to remove legacy DB network for ICC reconcile: {e}"))?;
    create_db_network_at(docker, network).await?;

    // Best-effort, not abort-on-first-failure: the network was already torn
    // down and recreated above, so a container this loop never REACHES has
    // ZERO network attachment — its published port stops routing, a silent
    // live outage for whichever tenant owns it, who may have nothing to do
    // with the request that triggered this reconcile. The old `?` here
    // meant the first Docker-daemon hiccup left every LATER container in
    // `attached` orphaned with no record of which ones. Known residual
    // limitation, not fixed here: once this function returns (however many
    // reconnects failed), `ensure_network`'s `icc_off` check short-circuits
    // true on every future call — a container that failed to reconnect has
    // no automatic retry and needs the manual command below.
    let mut failed: Vec<String> = Vec::new();
    for cid in attached {
        if let Err(e) = docker
            .connect_network(
                network,
                ConnectNetworkOptions {
                    container: cid.as_str(),
                    endpoint_config: Default::default(),
                },
            )
            .await
        {
            tracing::error!("Failed to reconnect {cid} to hardened DB network: {e}");
            failed.push(cid.clone());
        }
    }

    if !failed.is_empty() {
        return Err(format!(
            "Reconciled {network} to enable_icc=false, but {} of {} container(s) failed to \
             reconnect and are now offline (no network attached): {}. Reconnect manually: \
             `docker network connect {network} <container>`.",
            failed.len(),
            attached.len(),
            failed.join(", "),
        ));
    }

    tracing::info!(
        "Reconciled {network} to enable_icc=false ({} container(s) reconnected)",
        attached.len()
    );
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tenant_role_sql_is_non_superuser() {
        let sql = tenant_role_sql("blog_db", "secret");
        assert!(sql[0].contains("NOSUPERUSER"));
        assert!(sql[0].contains("NOCREATEDB"));
        assert!(sql[0].contains("NOCREATEROLE"));
        // Must NOT grant superuser — the space-prefixed token excludes NOSUPERUSER.
        assert!(!sql[0].contains(" SUPERUSER"));
        assert!(sql[0].contains("CREATE ROLE \"blog_db\""));
        assert!(sql[1].contains("ALTER DATABASE \"blog_db\" OWNER TO \"blog_db\""));
        assert!(sql[2].contains("ALTER SCHEMA public OWNER TO \"blog_db\""));
    }

    #[test]
    fn tenant_role_sql_escapes_password_quote() {
        let sql = tenant_role_sql("app", "a'b");
        assert!(sql[0].contains("PASSWORD 'a''b'"));
    }

    /// s455: `reconcile_network_icc_at` used to abort on the FIRST reconnect
    /// failure, silently orphaning every container later in `attached` — its
    /// network was already torn down, so a container the loop never reached
    /// had ZERO attachment. Proven against a REAL, fully disposable Docker
    /// network (never the real `dockpanel-db`, so this cannot touch
    /// production networking on this box): two real containers straddle a
    /// third id that cannot possibly connect, forcing a real mid-loop
    /// failure. Both real containers must still end up connected, and the
    /// error must name exactly the bad id.
    #[tokio::test]
    #[ignore = "creates real containers and a real (disposable) network; run with cargo test -p dockpanel-agent reconcile_network_icc -- --ignored --nocapture"]
    async fn reconcile_network_icc_reconnects_every_container_despite_one_bad_id() {
        use bollard::container::{Config, CreateContainerOptions, RemoveContainerOptions};
        use bollard::network::{ConnectNetworkOptions, CreateNetworkOptions};

        let docker = Docker::connect_with_local_defaults().expect("docker connect");
        let tag = uuid::Uuid::new_v4();
        let network = format!("dp-test-reconcile-net-{tag}");
        let name_a = format!("dp-test-reconcile-a-{tag}");
        let name_b = format!("dp-test-reconcile-b-{tag}");

        for name in [&name_a, &name_b] {
            docker
                .create_container(
                    Some(CreateContainerOptions { name: name.as_str(), platform: None }),
                    Config {
                        image: Some("alpine:latest".to_string()),
                        // Never started; connect/disconnect only needs the
                        // container to exist, not to be running.
                        cmd: Some(vec!["dockpanel-test-probe".to_string()]),
                        ..Default::default()
                    },
                )
                .await
                .expect("create test container");
        }

        // `reconcile_network_icc_at` mirrors the real `ensure_network` flow,
        // which only ever calls it when the network ALREADY exists (just
        // with the wrong ICC setting) — set that up for real here too.
        docker
            .create_network(CreateNetworkOptions {
                name: network.as_str(),
                driver: "bridge",
                ..Default::default()
            })
            .await
            .expect("create disposable test network");
        for name in [&name_a, &name_b] {
            docker
                .connect_network(
                    &network,
                    ConnectNetworkOptions { container: name.as_str(), endpoint_config: Default::default() },
                )
                .await
                .expect("attach test container to disposable network");
        }

        // Cannot possibly exist — forces connect_network to fail for exactly
        // this one entry, in the MIDDLE of the batch, so an abort-on-first-
        // failure loop would silently orphan name_b.
        let fake_id = format!("dp-test-nonexistent-{tag}");
        let attached = vec![name_a.clone(), fake_id.clone(), name_b.clone()];

        let result = reconcile_network_icc_at(&docker, &network, &attached).await;

        let mut orphaned: Vec<String> = Vec::new();
        for name in [&name_a, &name_b] {
            let connected = docker
                .inspect_container(name, None)
                .await
                .ok()
                .and_then(|c| c.network_settings)
                .and_then(|ns| ns.networks)
                .map(|nets| nets.contains_key(network.as_str()))
                .unwrap_or(false);
            if !connected {
                orphaned.push(name.clone());
            }
        }

        // Cleanup runs regardless of what the assertions below find — this
        // must never leave a disposable test container/network OR its
        // anonymous volume behind, so `v: true` is explicit here.
        for name in [&name_a, &name_b] {
            let _ = docker
                .remove_container(
                    name,
                    Some(RemoveContainerOptions {
                        v: true,
                        force: true,
                        ..Default::default()
                    }),
                )
                .await;
        }
        let _ = docker.remove_network(&network).await;

        assert!(result.is_err(), "expected an aggregated error naming the bad id");
        let msg = result.unwrap_err();
        assert!(msg.contains(&fake_id), "error must name the failed container: {msg}");
        assert!(!msg.contains(&name_a), "must not blame a container that actually succeeded: {msg}");
        assert!(!msg.contains(&name_b), "must not blame a container that actually succeeded: {msg}");
        assert!(
            orphaned.is_empty(),
            "these real containers — on either side of the bad id in the same batch — were \
             never reconnected: {orphaned:?} (an abort-on-first-failure loop is back)"
        );
    }
}
