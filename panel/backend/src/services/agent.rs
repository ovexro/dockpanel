use http_body_util::{BodyExt, Full};
use hyper::body::Bytes;
use hyper::client::conn::http1;
use hyper::Request;
use hyper_util::rt::TokioIo;
use std::collections::HashMap;
use std::fmt;
use std::sync::atomic::{AtomicU32, AtomicU64, Ordering};
use std::sync::Arc;
use std::time::Duration;
use tokio::net::UnixStream;
use tokio::sync::{RwLock, Semaphore};
use uuid::Uuid;

/// Circuit breaker threshold: after this many consecutive failures,
/// requests fast-fail without attempting a connection.
const CIRCUIT_BREAKER_THRESHOLD: u32 = 5;

/// Seconds to wait before retrying after circuit opens.
const CIRCUIT_BREAKER_RESET_SECS: u64 = 30;

/// Maximum concurrent agent connections for quick requests (prevents FD exhaustion).
const MAX_CONCURRENT_CONNECTIONS: usize = 20;

/// Maximum concurrent long-running agent operations (docker builds, etc.).
const MAX_LONG_CONNECTIONS: usize = 5;

/// Maximum response size from agent (50MB).
const MAX_RESPONSE_SIZE: usize = 50 * 1024 * 1024;

#[derive(Debug)]
pub enum AgentError {
    Connection(String),
    Request(String),
    Response(String),
    Status(u16, String),
    Parse(String),
    CircuitOpen(String),
    NotFound(String),
}

impl fmt::Display for AgentError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Connection(e) => write!(f, "agent connection failed: {e}"),
            Self::Request(e) => write!(f, "agent request failed: {e}"),
            Self::Response(e) => write!(f, "agent response error: {e}"),
            Self::Status(code, msg) => write!(f, "agent returned {code}: {msg}"),
            Self::Parse(e) => write!(f, "agent response parse error: {e}"),
            Self::CircuitOpen(e) => write!(f, "agent circuit breaker open: {e}"),
            Self::NotFound(e) => write!(f, "agent not found: {e}"),
        }
    }
}

/// Shared circuit breaker state for agent connections.
#[derive(Clone)]
struct CircuitBreaker {
    semaphore: Arc<Semaphore>,
    long_semaphore: Arc<Semaphore>,
    consecutive_failures: Arc<AtomicU32>,
    last_failure_time: Arc<AtomicU64>,
}

impl CircuitBreaker {
    fn new() -> Self {
        Self {
            semaphore: Arc::new(Semaphore::new(MAX_CONCURRENT_CONNECTIONS)),
            long_semaphore: Arc::new(Semaphore::new(MAX_LONG_CONNECTIONS)),
            consecutive_failures: Arc::new(AtomicU32::new(0)),
            last_failure_time: Arc::new(AtomicU64::new(0)),
        }
    }

    fn check(&self) -> Result<(), AgentError> {
        let failures = self.consecutive_failures.load(Ordering::Relaxed);
        if failures >= CIRCUIT_BREAKER_THRESHOLD {
            let last_fail = self.last_failure_time.load(Ordering::Relaxed);
            let now = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default()
                .as_secs();

            if now - last_fail < CIRCUIT_BREAKER_RESET_SECS {
                return Err(AgentError::CircuitOpen(format!(
                    "agent unreachable ({failures} consecutive failures), retry in {}s",
                    CIRCUIT_BREAKER_RESET_SECS - (now - last_fail)
                )));
            }
            tracing::info!("agent circuit breaker half-open, allowing probe request");
        }
        Ok(())
    }

    fn record_success(&self) {
        self.consecutive_failures.store(0, Ordering::Relaxed);
    }

    fn record_failure(&self) {
        self.consecutive_failures.fetch_add(1, Ordering::Relaxed);
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs();
        self.last_failure_time.store(now, Ordering::Relaxed);
    }
}

// ---------------------------------------------------------------------------
// AgentClient — talks to agent via Unix domain socket (local server)
// ---------------------------------------------------------------------------

#[derive(Clone)]
pub struct AgentClient {
    socket_path: String,
    token: String,
    cb: CircuitBreaker,
}

impl AgentClient {
    pub fn new(socket_path: String, token: String) -> Self {
        Self {
            socket_path,
            token,
            cb: CircuitBreaker::new(),
        }
    }

    pub fn token(&self) -> &str {
        &self.token
    }

    async fn request(
        &self,
        method: &str,
        path: &str,
        body: Option<serde_json::Value>,
    ) -> Result<serde_json::Value, AgentError> {
        self.cb.check()?;
        let _permit = self.cb.semaphore.acquire().await.map_err(|e| {
            AgentError::Connection(format!("connection semaphore closed: {e}"))
        })?;

        let result = tokio::time::timeout(
            Duration::from_secs(60),
            self.request_inner(method, path, body),
        )
        .await
        .map_err(|_| AgentError::Request("agent request timed out after 60s".into()))?;

        match &result {
            Ok(_) => self.cb.record_success(),
            Err(AgentError::Connection(_)) => self.cb.record_failure(),
            _ => {}
        }

        result
    }

    async fn request_inner(
        &self,
        method: &str,
        path: &str,
        body: Option<serde_json::Value>,
    ) -> Result<serde_json::Value, AgentError> {
        let stream = UnixStream::connect(&self.socket_path)
            .await
            .map_err(|e| AgentError::Connection(e.to_string()))?;

        let io = TokioIo::new(stream);

        let (mut sender, conn) = http1::handshake(io)
            .await
            .map_err(|e| AgentError::Connection(e.to_string()))?;

        tokio::spawn(async move {
            if let Err(e) = conn.await {
                tracing::warn!("agent connection error: {e}");
            }
        });

        let body_bytes = match &body {
            Some(v) => Full::new(Bytes::from(
                serde_json::to_vec(v)
                    .map_err(|e| AgentError::Request(format!("JSON serialize error: {e}")))?,
            )),
            None => Full::new(Bytes::new()),
        };

        let mut builder = Request::builder()
            .method(method)
            .uri(format!("http://localhost{path}"))
            .header("authorization", format!("Bearer {}", self.token));

        if body.is_some() {
            builder = builder.header("content-type", "application/json");
        }

        let req = builder
            .body(body_bytes)
            .map_err(|e| AgentError::Request(e.to_string()))?;

        let resp = sender
            .send_request(req)
            .await
            .map_err(|e| AgentError::Request(e.to_string()))?;

        let status = resp.status();
        let collected = resp
            .into_body()
            .collect()
            .await
            .map_err(|e| AgentError::Response(e.to_string()))?;

        let bytes = collected.to_bytes();

        if bytes.len() > MAX_RESPONSE_SIZE {
            return Err(AgentError::Response(format!(
                "agent response too large: {} bytes (limit: {}MB)",
                bytes.len(),
                MAX_RESPONSE_SIZE / (1024 * 1024)
            )));
        }

        if !status.is_success() {
            let msg = String::from_utf8_lossy(&bytes).to_string();
            return Err(AgentError::Status(status.as_u16(), msg));
        }

        serde_json::from_slice(&bytes).map_err(|e| AgentError::Parse(e.to_string()))
    }

    pub async fn get(&self, path: &str) -> Result<serde_json::Value, AgentError> {
        self.request("GET", path, None).await
    }

    pub async fn put(
        &self,
        path: &str,
        body: serde_json::Value,
    ) -> Result<serde_json::Value, AgentError> {
        self.request("PUT", path, Some(body)).await
    }

    pub async fn delete(&self, path: &str) -> Result<serde_json::Value, AgentError> {
        self.request("DELETE", path, None).await
    }

    pub async fn post(
        &self,
        path: &str,
        body: Option<serde_json::Value>,
    ) -> Result<serde_json::Value, AgentError> {
        self.request("POST", path, body).await
    }

    /// GET that returns raw bytes instead of JSON. Used for file downloads.
    pub async fn get_bytes(&self, path: &str) -> Result<(Vec<u8>, Option<String>), AgentError> {
        self.cb.check()?;
        let _permit = self.cb.semaphore.acquire().await.map_err(|e| {
            AgentError::Connection(format!("connection semaphore closed: {e}"))
        })?;

        let result = tokio::time::timeout(
            Duration::from_secs(120),
            self.request_bytes_inner(path),
        )
        .await
        .map_err(|_| AgentError::Request("agent request timed out after 120s".into()))?;

        match &result {
            Ok(_) => self.cb.record_success(),
            Err(AgentError::Connection(_)) => self.cb.record_failure(),
            _ => {}
        }

        result
    }

    async fn request_bytes_inner(
        &self,
        path: &str,
    ) -> Result<(Vec<u8>, Option<String>), AgentError> {
        let stream = UnixStream::connect(&self.socket_path)
            .await
            .map_err(|e| AgentError::Connection(e.to_string()))?;

        let io = TokioIo::new(stream);

        let (mut sender, conn) = http1::handshake(io)
            .await
            .map_err(|e| AgentError::Connection(e.to_string()))?;

        tokio::spawn(async move {
            if let Err(e) = conn.await {
                tracing::warn!("agent connection error: {e}");
            }
        });

        let req = Request::builder()
            .method("GET")
            .uri(format!("http://localhost{path}"))
            .header("authorization", format!("Bearer {}", self.token))
            .body(Full::new(Bytes::new()))
            .map_err(|e| AgentError::Request(e.to_string()))?;

        let resp = sender
            .send_request(req)
            .await
            .map_err(|e| AgentError::Request(e.to_string()))?;

        let status = resp.status();

        let content_disposition = resp
            .headers()
            .get("content-disposition")
            .and_then(|v| v.to_str().ok())
            .map(|s| s.to_string());

        let collected = resp
            .into_body()
            .collect()
            .await
            .map_err(|e| AgentError::Response(e.to_string()))?;

        let bytes = collected.to_bytes();

        if !status.is_success() {
            let msg = String::from_utf8_lossy(&bytes).to_string();
            return Err(AgentError::Status(status.as_u16(), msg));
        }

        Ok((bytes.to_vec(), content_disposition))
    }

    /// POST with a custom timeout (seconds). Use for long-running operations like docker build.
    /// Uses a separate semaphore (5 permits) so long ops don't starve quick requests.
    pub async fn post_long(
        &self,
        path: &str,
        body: Option<serde_json::Value>,
        timeout_secs: u64,
    ) -> Result<serde_json::Value, AgentError> {
        self.cb.check()?;
        let _permit = self.cb.long_semaphore.acquire().await.map_err(|e| {
            AgentError::Connection(format!("long operation semaphore closed: {e}"))
        })?;

        let result = tokio::time::timeout(
            Duration::from_secs(timeout_secs),
            self.request_inner("POST", path, body),
        )
        .await
        .map_err(|_| AgentError::Request(format!("agent request timed out after {timeout_secs}s")))?;

        match &result {
            Ok(_) => self.cb.record_success(),
            Err(AgentError::Connection(_)) => self.cb.record_failure(),
            _ => {}
        }

        result
    }
}

// ---------------------------------------------------------------------------
// RemoteAgentClient — talks to agent via HTTP/HTTPS (remote servers)
// ---------------------------------------------------------------------------

#[derive(Clone)]
pub struct RemoteAgentClient {
    base_url: String,
    token: String,
    http: reqwest::Client,
    cb: CircuitBreaker,
}

impl RemoteAgentClient {
    pub fn new(base_url: String, token: String) -> Self {
        let http = reqwest::Client::builder()
            .timeout(Duration::from_secs(60))
            .danger_accept_invalid_certs(true) // Agent uses self-signed certs
            .pool_max_idle_per_host(5)
            .build()
            .unwrap_or_default();

        Self {
            base_url: base_url.trim_end_matches('/').to_string(),
            token,
            http,
            cb: CircuitBreaker::new(),
        }
    }

    async fn request(
        &self,
        method: reqwest::Method,
        path: &str,
        body: Option<serde_json::Value>,
    ) -> Result<serde_json::Value, AgentError> {
        self.cb.check()?;
        let _permit = self.cb.semaphore.acquire().await.map_err(|e| {
            AgentError::Connection(format!("connection semaphore closed: {e}"))
        })?;

        let url = format!("{}{}", self.base_url, path);
        let mut req = self.http.request(method, &url)
            .header("authorization", format!("Bearer {}", self.token));

        if let Some(b) = body {
            req = req.json(&b);
        }

        let result = req.send().await;

        match result {
            Ok(resp) => {
                self.cb.record_success();
                let status = resp.status();
                let bytes = resp.bytes().await
                    .map_err(|e| AgentError::Response(e.to_string()))?;

                if bytes.len() > MAX_RESPONSE_SIZE {
                    return Err(AgentError::Response(format!(
                        "agent response too large: {} bytes",
                        bytes.len()
                    )));
                }

                if !status.is_success() {
                    let msg = String::from_utf8_lossy(&bytes).to_string();
                    return Err(AgentError::Status(status.as_u16(), msg));
                }

                serde_json::from_slice(&bytes).map_err(|e| AgentError::Parse(e.to_string()))
            }
            Err(e) => {
                self.cb.record_failure();
                Err(AgentError::Connection(e.to_string()))
            }
        }
    }

    pub async fn get(&self, path: &str) -> Result<serde_json::Value, AgentError> {
        self.request(reqwest::Method::GET, path, None).await
    }

    pub async fn post(
        &self,
        path: &str,
        body: Option<serde_json::Value>,
    ) -> Result<serde_json::Value, AgentError> {
        self.request(reqwest::Method::POST, path, body).await
    }

    pub async fn put(
        &self,
        path: &str,
        body: serde_json::Value,
    ) -> Result<serde_json::Value, AgentError> {
        self.request(reqwest::Method::PUT, path, Some(body)).await
    }

    pub async fn delete(&self, path: &str) -> Result<serde_json::Value, AgentError> {
        self.request(reqwest::Method::DELETE, path, None).await
    }

    pub async fn get_bytes(&self, path: &str) -> Result<(Vec<u8>, Option<String>), AgentError> {
        self.cb.check()?;
        let _permit = self.cb.semaphore.acquire().await.map_err(|e| {
            AgentError::Connection(format!("connection semaphore closed: {e}"))
        })?;

        let url = format!("{}{}", self.base_url, path);
        let result = self.http.get(&url)
            .header("authorization", format!("Bearer {}", self.token))
            .timeout(Duration::from_secs(120))
            .send()
            .await;

        match result {
            Ok(resp) => {
                self.cb.record_success();
                let status = resp.status();
                let content_disposition = resp
                    .headers()
                    .get("content-disposition")
                    .and_then(|v| v.to_str().ok())
                    .map(|s| s.to_string());

                let bytes = resp.bytes().await
                    .map_err(|e| AgentError::Response(e.to_string()))?;

                if !status.is_success() {
                    let msg = String::from_utf8_lossy(&bytes).to_string();
                    return Err(AgentError::Status(status.as_u16(), msg));
                }

                Ok((bytes.to_vec(), content_disposition))
            }
            Err(e) => {
                self.cb.record_failure();
                Err(AgentError::Connection(e.to_string()))
            }
        }
    }

    pub async fn post_long(
        &self,
        path: &str,
        body: Option<serde_json::Value>,
        timeout_secs: u64,
    ) -> Result<serde_json::Value, AgentError> {
        self.cb.check()?;
        let _permit = self.cb.semaphore.acquire().await.map_err(|e| {
            AgentError::Connection(format!("connection semaphore closed: {e}"))
        })?;

        let url = format!("{}{}", self.base_url, path);
        let mut req = self.http.post(&url)
            .header("authorization", format!("Bearer {}", self.token))
            .timeout(Duration::from_secs(timeout_secs));

        if let Some(b) = body {
            req = req.json(&b);
        }

        let result = req.send().await;

        match result {
            Ok(resp) => {
                self.cb.record_success();
                let status = resp.status();
                let bytes = resp.bytes().await
                    .map_err(|e| AgentError::Response(e.to_string()))?;

                if !status.is_success() {
                    let msg = String::from_utf8_lossy(&bytes).to_string();
                    return Err(AgentError::Status(status.as_u16(), msg));
                }

                serde_json::from_slice(&bytes).map_err(|e| AgentError::Parse(e.to_string()))
            }
            Err(e) => {
                self.cb.record_failure();
                Err(AgentError::Connection(e.to_string()))
            }
        }
    }
}

// ---------------------------------------------------------------------------
// AgentHandle — unified interface for local or remote agent
// ---------------------------------------------------------------------------

/// A handle to an agent that provides the same API regardless of transport.
#[derive(Clone)]
pub enum AgentHandle {
    Local(AgentClient),
    Remote(RemoteAgentClient),
}

impl AgentHandle {
    pub async fn get(&self, path: &str) -> Result<serde_json::Value, AgentError> {
        match self {
            Self::Local(c) => c.get(path).await,
            Self::Remote(c) => c.get(path).await,
        }
    }

    pub async fn post(
        &self,
        path: &str,
        body: Option<serde_json::Value>,
    ) -> Result<serde_json::Value, AgentError> {
        match self {
            Self::Local(c) => c.post(path, body).await,
            Self::Remote(c) => c.post(path, body).await,
        }
    }

    pub async fn put(
        &self,
        path: &str,
        body: serde_json::Value,
    ) -> Result<serde_json::Value, AgentError> {
        match self {
            Self::Local(c) => c.put(path, body).await,
            Self::Remote(c) => c.put(path, body).await,
        }
    }

    pub async fn delete(&self, path: &str) -> Result<serde_json::Value, AgentError> {
        match self {
            Self::Local(c) => c.delete(path).await,
            Self::Remote(c) => c.delete(path).await,
        }
    }

    pub async fn get_bytes(&self, path: &str) -> Result<(Vec<u8>, Option<String>), AgentError> {
        match self {
            Self::Local(c) => c.get_bytes(path).await,
            Self::Remote(c) => c.get_bytes(path).await,
        }
    }

    pub async fn post_long(
        &self,
        path: &str,
        body: Option<serde_json::Value>,
        timeout_secs: u64,
    ) -> Result<serde_json::Value, AgentError> {
        match self {
            Self::Local(c) => c.post_long(path, body, timeout_secs).await,
            Self::Remote(c) => c.post_long(path, body, timeout_secs).await,
        }
    }

    pub fn token(&self) -> &str {
        match self {
            Self::Local(c) => c.token(),
            Self::Remote(c) => &c.token,
        }
    }
}

// ---------------------------------------------------------------------------
// AgentRegistry — manages local + remote agents, dispatches by server_id
// ---------------------------------------------------------------------------

#[derive(Clone)]
pub struct AgentRegistry {
    /// The local server's agent (Unix socket).
    local: AgentClient,
    /// The local server's UUID in the DB.
    local_server_id: Arc<RwLock<Option<Uuid>>>,
    /// Cached remote agent clients keyed by server_id.
    remote_cache: Arc<RwLock<HashMap<Uuid, RemoteAgentClient>>>,
    /// Database pool for looking up server details.
    db: sqlx::PgPool,
}

impl AgentRegistry {
    pub fn new(local: AgentClient, db: sqlx::PgPool) -> Self {
        Self {
            local,
            local_server_id: Arc::new(RwLock::new(None)),
            remote_cache: Arc::new(RwLock::new(HashMap::new())),
            db,
        }
    }

    /// Set the local server ID (called once on startup after ensure_local_server).
    pub async fn set_local_server_id(&self, id: Uuid) {
        *self.local_server_id.write().await = Some(id);
    }

    /// Get the local server ID.
    pub async fn local_server_id(&self) -> Option<Uuid> {
        *self.local_server_id.read().await
    }

    /// Get the local agent directly (for background services that always use local).
    pub fn local(&self) -> &AgentClient {
        &self.local
    }

    /// Get an AgentHandle for the given server_id.
    /// Returns Local handle if server_id matches the local server, otherwise Remote.
    pub async fn for_server(&self, server_id: Uuid) -> Result<AgentHandle, AgentError> {
        // Check if this is the local server
        if let Some(local_id) = *self.local_server_id.read().await {
            if server_id == local_id {
                return Ok(AgentHandle::Local(self.local.clone()));
            }
        }

        // Check remote cache
        {
            let cache = self.remote_cache.read().await;
            if let Some(client) = cache.get(&server_id) {
                return Ok(AgentHandle::Remote(client.clone()));
            }
        }

        // Fetch from DB and cache
        let row: Option<(String, String)> = sqlx::query_as(
            "SELECT agent_url, agent_token FROM servers WHERE id = $1 AND status != 'pending'",
        )
        .bind(server_id)
        .fetch_optional(&self.db)
        .await
        .map_err(|e| AgentError::Connection(format!("DB lookup failed: {e}")))?;

        match row {
            Some((url, token)) if !url.is_empty() => {
                let client = RemoteAgentClient::new(url, token);
                self.remote_cache.write().await.insert(server_id, client.clone());
                Ok(AgentHandle::Remote(client))
            }
            Some(_) => Err(AgentError::NotFound(
                "Server has no agent_url configured".into(),
            )),
            None => Err(AgentError::NotFound(
                "Server not found or still pending".into(),
            )),
        }
    }

    /// Get an AgentHandle, defaulting to local if server_id is None.
    pub async fn for_server_or_local(&self, server_id: Option<Uuid>) -> Result<AgentHandle, AgentError> {
        match server_id {
            Some(id) => self.for_server(id).await,
            None => Ok(AgentHandle::Local(self.local.clone())),
        }
    }

    /// Invalidate cached remote client (e.g. after server update/delete).
    pub async fn invalidate(&self, server_id: Uuid) {
        self.remote_cache.write().await.remove(&server_id);
    }

    /// List all online server IDs (for background services that need to iterate).
    pub async fn online_server_ids(&self) -> Vec<(Uuid, bool)> {
        let rows: Vec<(Uuid, bool)> = sqlx::query_as(
            "SELECT id, is_local FROM servers WHERE status = 'online' ORDER BY is_local DESC",
        )
        .fetch_all(&self.db)
        .await
        .unwrap_or_default();
        rows
    }
}

/// Ensure the local server row exists in the DB. Returns the local server UUID.
pub async fn ensure_local_server(db: &sqlx::PgPool, agent_token: &str) -> Uuid {
    // Check if a local server already exists
    let existing: Option<(Uuid,)> =
        sqlx::query_as("SELECT id FROM servers WHERE is_local = true LIMIT 1")
            .fetch_optional(db)
            .await
            .unwrap_or(None);

    if let Some((id,)) = existing {
        // Update token if changed
        let _ = sqlx::query("UPDATE servers SET agent_token = $1, status = 'online' WHERE id = $2")
            .bind(agent_token)
            .bind(id)
            .execute(db)
            .await;
        return id;
    }

    // Find first admin user to assign as owner
    let admin: Option<(Uuid,)> =
        sqlx::query_as("SELECT id FROM users WHERE role = 'admin' ORDER BY created_at ASC LIMIT 1")
            .fetch_optional(db)
            .await
            .unwrap_or(None);

    let user_id = match admin {
        Some((uid,)) => uid,
        None => {
            // No users yet — create a placeholder that will be updated on first setup
            tracing::info!("No users yet, deferring local server registration to first login");
            return Uuid::nil();
        }
    };

    let row: (Uuid,) = sqlx::query_as(
        "INSERT INTO servers (id, user_id, name, agent_token, status, is_local) \
         VALUES (gen_random_uuid(), $1, 'This Server', $2, 'online', true) \
         RETURNING id",
    )
    .bind(user_id)
    .bind(agent_token)
    .fetch_one(db)
    .await
    .expect("Failed to create local server row");

    tracing::info!("Registered local server: {}", row.0);
    row.0
}
