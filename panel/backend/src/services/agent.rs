use http_body_util::{BodyExt, Full};
use hyper::body::Bytes;
use hyper::client::conn::http1;
use hyper::Request;
use hyper_util::rt::TokioIo;
use std::fmt;
use std::sync::atomic::{AtomicU32, AtomicU64, Ordering};
use std::sync::Arc;
use std::time::Duration;
use tokio::net::UnixStream;
use tokio::sync::Semaphore;

/// Circuit breaker threshold: after this many consecutive failures,
/// requests fast-fail without attempting a connection.
const CIRCUIT_BREAKER_THRESHOLD: u32 = 5;

/// Seconds to wait before retrying after circuit opens.
const CIRCUIT_BREAKER_RESET_SECS: u64 = 30;

/// Maximum concurrent agent connections (prevents FD exhaustion).
const MAX_CONCURRENT_CONNECTIONS: usize = 20;

#[derive(Debug)]
pub enum AgentError {
    Connection(String),
    Request(String),
    Response(String),
    Status(u16, String),
    Parse(String),
    CircuitOpen(String),
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
        }
    }
}

#[derive(Clone)]
pub struct AgentClient {
    socket_path: String,
    token: String,
    /// Limits concurrent connections to prevent FD exhaustion.
    semaphore: Arc<Semaphore>,
    /// Number of consecutive connection failures (circuit breaker).
    consecutive_failures: Arc<AtomicU32>,
    /// Epoch seconds of last connection failure (for circuit reset).
    last_failure_time: Arc<AtomicU64>,
}

impl AgentClient {
    pub fn new(socket_path: String, token: String) -> Self {
        Self {
            socket_path,
            token,
            semaphore: Arc::new(Semaphore::new(MAX_CONCURRENT_CONNECTIONS)),
            consecutive_failures: Arc::new(AtomicU32::new(0)),
            last_failure_time: Arc::new(AtomicU64::new(0)),
        }
    }

    /// Check if the circuit breaker allows a request through.
    fn check_circuit_breaker(&self) -> Result<(), AgentError> {
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
            // Reset period elapsed — allow one attempt (half-open)
            tracing::info!("agent circuit breaker half-open, allowing probe request");
        }
        Ok(())
    }

    /// Record a successful response — reset circuit breaker.
    fn record_success(&self) {
        self.consecutive_failures.store(0, Ordering::Relaxed);
    }

    /// Record a connection failure — increment circuit breaker counter.
    fn record_failure(&self) {
        self.consecutive_failures.fetch_add(1, Ordering::Relaxed);
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs();
        self.last_failure_time.store(now, Ordering::Relaxed);
    }

    async fn request(
        &self,
        method: &str,
        path: &str,
        body: Option<serde_json::Value>,
    ) -> Result<serde_json::Value, AgentError> {
        // Circuit breaker: fast-fail if agent is known to be down
        self.check_circuit_breaker()?;

        // Connection semaphore: limit concurrent FDs
        let _permit = self.semaphore.acquire().await.map_err(|e| {
            AgentError::Connection(format!("connection semaphore closed: {e}"))
        })?;

        let result = tokio::time::timeout(
            Duration::from_secs(60),
            self.request_inner(method, path, body),
        )
        .await
        .map_err(|_| AgentError::Request("agent request timed out after 60s".into()))?;

        match &result {
            Ok(_) => self.record_success(),
            Err(AgentError::Connection(_)) => self.record_failure(),
            _ => {} // Status/Parse errors mean agent is reachable
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

        // Guard against oversized agent responses (50MB limit)
        const MAX_RESPONSE_SIZE: usize = 50 * 1024 * 1024;
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

    pub fn token(&self) -> &str {
        &self.token
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
}
