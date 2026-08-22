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

/// Classify a failed remote send, and only count the ones that mean "offline".
///
/// ⚠ A timeout is not an unreachable agent, and conflating the two is worse now
/// than it was. Every remote failure used to become `Connection`, which trips the
/// circuit breaker and reaches the operator as "agent unreachable". A busy agent
/// answering perfectly well would be reported as down — and moving remote long
/// operations onto the small pool makes waiting, and therefore timing out, far
/// more likely than it was.
///
/// The local twins already draw this line: an elapsed clock is a `Request`, which
/// the breaker does not count. This is that rule, applied to the client that
/// needed it most.
fn remote_send_error(cb: &CircuitBreaker, e: reqwest::Error) -> AgentError {
    if e.is_timeout() {
        AgentError::Request(e.to_string())
    } else {
        cb.record_failure();
        AgentError::Connection(e.to_string())
    }
}

/// Take a permit within the caller's budget, and report what is left of it.
///
/// The budget a caller names is the time it has to spend before something above
/// it stops waiting — the doc on `DB_RESTORE_TIMEOUT_SECS` sets 270s precisely
/// so the 300s `TimeoutLayer` never fires first, because a timeout the panel
/// owns is the only kind it can explain. Queuing outside that budget defeats it:
/// the permit wait was unbounded, so the work started late and ran its full
/// allowance on top, and the server cut the request off mid-flight with a
/// bodyless 504 — the exact answer that doc exists to prevent.
///
/// So the wait is inside the budget, and a request that never got a permit says
/// THAT, rather than borrowing the sentence for work that ran and overran.
/// Nothing that reached the agent is described differently than before.
async fn permit_within<'a>(
    sem: &'a tokio::sync::Semaphore,
    budget: Duration,
    pool: &str,
) -> Result<(tokio::sync::SemaphorePermit<'a>, Duration), AgentError> {
    let start = std::time::Instant::now();
    let permit = tokio::time::timeout(budget, sem.acquire())
        .await
        .map_err(|_| {
            AgentError::Request(format!(
                "timed out after {}s waiting for a free {pool} slot; the operation never started",
                budget.as_secs()
            ))
        })?
        .map_err(|e| AgentError::Connection(format!("{pool} semaphore closed: {e}")))?;
    Ok((permit, budget.saturating_sub(start.elapsed())))
}

/// Maximum response size from agent (50MB).
const MAX_RESPONSE_SIZE: usize = 50 * 1024 * 1024;

/// How long a database restore/import may occupy a request before the panel stops waiting.
///
/// Deliberately BELOW the server's own ceiling rather than matched to the agent's work
/// budget, and the gap is the whole point. The agent gives a restore 600s
/// (`agent/src/services/database_backup.rs`), but `main.rs` wraps every route in a 300s
/// `TimeoutLayer` and the generated nginx vhost sets `proxy_read_timeout 300s` — so a
/// handler that waited 600s would be cut off by its own server at 300 and answer a
/// bodyless 504 that no client can explain. Waiting slightly less means the timeout is
/// OURS, which is what lets the handler say something true about it: a timeout here does
/// not mean the import failed, it means we stopped watching. `agent_error` cannot make
/// that distinction — `AgentError::Request` falls to the arm that mints an incident id —
/// so the callers of this constant handle that variant themselves.
///
/// ⚠ A panel behind Cloudflare's proxy is cut at 100s regardless of any of this; the
/// same caveat is recorded at `routes/migration.rs` for the migration importer.
pub const DB_RESTORE_TIMEOUT_SECS: u64 = 270;

/// Effectively-disabled total timeout for streamed remote requests.
///
/// reqwest only offers a *total* per-request timeout, and the client default
/// (60s) is far shorter than a legitimate streamed `apt-get upgrade`. There is
/// no per-request "no timeout", so it is pushed out of reach and the real bound
/// becomes the idle watchdog in [`drive_idle_bounded`] — which is the correct
/// thing to measure for a stream anyway.
const STREAM_TOTAL_TIMEOUT_SECS: u64 = 24 * 60 * 60;

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

/// Drives a streaming-NDJSON future under an **idle** deadline rather than a
/// total-runtime one.
///
/// `ticks` is bumped by the caller's wrapped line callback. The watchdog arms a
/// fresh `idle_timeout_secs` window each time round the loop and only gives up
/// if the counter did not move across a whole window — so an operation that is
/// slow but still talking runs to completion, while one that has genuinely
/// wedged still fails in bounded time.
///
/// The callback must do the bumping because this layer never sees the lines:
/// `request_ndjson_inner` parses them and hands them straight to `on_line`.
async fn drive_idle_bounded<Fut>(
    idle_timeout_secs: u64,
    ticks: Arc<AtomicU64>,
    fut: Fut,
) -> Result<(), AgentError>
where
    Fut: std::future::Future<Output = Result<(), AgentError>>,
{
    tokio::pin!(fut);
    let idle = Duration::from_secs(idle_timeout_secs);

    loop {
        let before = ticks.load(Ordering::Relaxed);
        tokio::select! {
            // Biased so a future that is ready in the same tick as the timer
            // reports its own result instead of being called idle.
            biased;
            result = &mut fut => return result,
            _ = tokio::time::sleep(idle) => {
                if ticks.load(Ordering::Relaxed) == before {
                    return Err(AgentError::Request(format!(
                        "agent stream produced no output for {idle_timeout_secs}s"
                    )));
                }
            }
        }
    }
}

// ---------------------------------------------------------------------------
// AgentClient — talks to agent via Unix domain socket (local server)
// ---------------------------------------------------------------------------

#[derive(Clone)]
pub struct AgentClient {
    socket_path: String,
    token: Arc<tokio::sync::RwLock<String>>,
    cb: CircuitBreaker,
}

impl AgentClient {
    pub fn new(socket_path: String, token: String) -> Self {
        Self {
            socket_path,
            token: Arc::new(tokio::sync::RwLock::new(token)),
            cb: CircuitBreaker::new(),
        }
    }

    pub async fn current_token(&self) -> String {
        self.token.read().await.clone()
    }

    /// Update the agent token after rotation.
    pub async fn update_token(&self, new_token: String) {
        let mut t = self.token.write().await;
        *t = new_token;
    }

    async fn request(
        &self,
        method: &str,
        path: &str,
        body: Option<serde_json::Value>,
    ) -> Result<serde_json::Value, AgentError> {
        self.cb.check()?;
        let (_permit, remaining) =
            permit_within(&self.cb.semaphore, Duration::from_secs(60), "connection").await?;

        let result = tokio::time::timeout(
            remaining,
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

        let token = self.token.read().await;
        let mut builder = Request::builder()
            .method(method)
            .uri(format!("http://localhost{path}"))
            .header("authorization", format!("Bearer {token}"));

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

        // Stream-limited body collection: stops reading as soon as the limit
        // is exceeded instead of buffering the entire response first.
        let limited = http_body_util::Limited::new(resp.into_body(), MAX_RESPONSE_SIZE);
        let collected = limited
            .collect()
            .await
            .map_err(|e| {
                let msg = e.to_string();
                if msg.contains("length limit exceeded") {
                    AgentError::Response(format!(
                        "agent response too large (limit: {}MB)",
                        MAX_RESPONSE_SIZE / (1024 * 1024)
                    ))
                } else {
                    AgentError::Response(msg)
                }
            })?;

        let bytes = collected.to_bytes();

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

    /// PUT with an explicit timeout, for the same reason `post_long` exists: some PUTs
    /// do real work. Editing an app's environment recreates its container and may copy
    /// the app's data onto a bind mount first, which does not fit the default budget.
    pub async fn put_long(
        &self,
        path: &str,
        body: serde_json::Value,
        timeout_secs: u64,
    ) -> Result<serde_json::Value, AgentError> {
        self.cb.check()?;
        let (_permit, remaining) = permit_within(
            &self.cb.long_semaphore,
            Duration::from_secs(timeout_secs),
            "long operation",
        )
        .await?;

        let result = tokio::time::timeout(
            remaining,
            self.request_inner("PUT", path, Some(body)),
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
        let (_permit, remaining) =
            permit_within(&self.cb.semaphore, Duration::from_secs(120), "connection").await?;

        let result = tokio::time::timeout(
            remaining,
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

        let token = self.token.read().await;
        let req = Request::builder()
            .method("GET")
            .uri(format!("http://localhost{path}"))
            .header("authorization", format!("Bearer {token}"))
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
        let (_permit, remaining) = permit_within(
            &self.cb.long_semaphore,
            Duration::from_secs(timeout_secs),
            "long operation",
        )
        .await?;

        let result = tokio::time::timeout(
            remaining,
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

    /// POST with streaming NDJSON response. Reads the agent response body frame-by-frame,
    /// parses newline-delimited JSON, and calls `on_line` for each parsed JSON value.
    /// Used for long-running operations that stream output (e.g. apt updates).
    ///
    /// `idle_timeout_secs` bounds **silence, not total runtime** — the clock is
    /// rearmed every time a line arrives. This was a flat cap on the whole
    /// operation, which cannot be set correctly for a stream: `apt-get upgrade`
    /// finishes in 40s on this box and can legitimately run for the better part
    /// of an hour on a slow link with a full release of packages, so the old
    /// 300s ceiling simply failed the long ones while apt carried on
    /// underneath and the operator was told the update had failed. How long the
    /// work takes is not evidence of anything; going quiet is.
    pub async fn post_long_ndjson<F>(
        &self,
        path: &str,
        body: Option<serde_json::Value>,
        idle_timeout_secs: u64,
        on_line: F,
    ) -> Result<(), AgentError>
    where
        F: Fn(serde_json::Value) + Send + 'static,
    {
        self.cb.check()?;
        // A stream has no total budget by design — the idle clock bounds SILENCE,
        // not runtime — so there is no allowance to subtract a queue wait from. The
        // wait is bounded by that same idle figure instead: a stream that cannot
        // even be started inside its own silence budget is stalled, and saying so
        // beats waiting for ever behind work that may not finish.
        let (_permit, _) = permit_within(
            &self.cb.long_semaphore,
            Duration::from_secs(idle_timeout_secs),
            "long operation",
        )
        .await?;

        let ticks = Arc::new(AtomicU64::new(0));
        let probe = {
            let ticks = ticks.clone();
            move |json: serde_json::Value| {
                ticks.fetch_add(1, Ordering::Relaxed);
                on_line(json);
            }
        };

        let result = drive_idle_bounded(
            idle_timeout_secs,
            ticks,
            self.request_ndjson_inner(path, body, probe),
        )
        .await;

        match &result {
            Ok(_) => self.cb.record_success(),
            Err(AgentError::Connection(_)) => self.cb.record_failure(),
            _ => {}
        }

        result
    }

    async fn request_ndjson_inner<F>(
        &self,
        path: &str,
        body: Option<serde_json::Value>,
        on_line: F,
    ) -> Result<(), AgentError>
    where
        F: Fn(serde_json::Value),
    {
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

        let token = self.token.read().await;
        let mut builder = Request::builder()
            .method("POST")
            .uri(format!("http://localhost{path}"))
            .header("authorization", format!("Bearer {token}"));

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
        if !status.is_success() {
            let collected = resp.into_body().collect()
                .await
                .map_err(|e| AgentError::Response(e.to_string()))?;
            let msg = String::from_utf8_lossy(&collected.to_bytes()).to_string();
            return Err(AgentError::Status(status.as_u16(), msg));
        }

        // Read body frame-by-frame, parse NDJSON lines
        let mut body = resp.into_body();
        let mut buf = String::new();
        while let Some(frame) = body.frame().await {
            match frame {
                Ok(frame) => {
                    if let Some(data) = frame.data_ref() {
                        buf.push_str(&String::from_utf8_lossy(data));
                        // Process all complete lines in the buffer
                        while let Some(pos) = buf.find('\n') {
                            let line = &buf[..pos];
                            if !line.is_empty() {
                                if let Ok(json) = serde_json::from_str::<serde_json::Value>(line) {
                                    on_line(json);
                                }
                            }
                            buf = buf[pos + 1..].to_string();
                        }
                    }
                }
                Err(e) => {
                    return Err(AgentError::Response(e.to_string()));
                }
            }
        }

        // Process any remaining data in buffer
        if !buf.trim().is_empty() {
            if let Ok(json) = serde_json::from_str::<serde_json::Value>(buf.trim()) {
                on_line(json);
            }
        }

        Ok(())
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

/// Rustls verifier that trusts exactly one self-signed cert, identified by the
/// SHA-256 hex fingerprint of its DER encoding. Replaces `danger_accept_invalid_certs`
/// once the panel has captured an agent's fingerprint on first checkin (TOFU).
#[derive(Debug)]
struct PinnedFingerprintVerifier {
    expected: String,
    provider: Arc<rustls::crypto::CryptoProvider>,
}

impl PinnedFingerprintVerifier {
    fn new(expected_hex: String) -> Self {
        Self {
            expected: expected_hex.to_ascii_lowercase(),
            provider: Arc::new(rustls::crypto::aws_lc_rs::default_provider()),
        }
    }
}

impl rustls::client::danger::ServerCertVerifier for PinnedFingerprintVerifier {
    fn verify_server_cert(
        &self,
        end_entity: &rustls::pki_types::CertificateDer<'_>,
        _intermediates: &[rustls::pki_types::CertificateDer<'_>],
        _server_name: &rustls::pki_types::ServerName<'_>,
        _ocsp_response: &[u8],
        _now: rustls::pki_types::UnixTime,
    ) -> Result<rustls::client::danger::ServerCertVerified, rustls::Error> {
        use sha2::{Digest, Sha256};
        use subtle::ConstantTimeEq;
        let actual = hex::encode(Sha256::digest(end_entity.as_ref()));
        if actual.as_bytes().ct_eq(self.expected.as_bytes()).into() {
            Ok(rustls::client::danger::ServerCertVerified::assertion())
        } else {
            Err(rustls::Error::General(format!(
                "agent cert fingerprint mismatch: expected {}, got {}",
                self.expected, actual
            )))
        }
    }

    fn verify_tls12_signature(
        &self,
        message: &[u8],
        cert: &rustls::pki_types::CertificateDer<'_>,
        dss: &rustls::DigitallySignedStruct,
    ) -> Result<rustls::client::danger::HandshakeSignatureValid, rustls::Error> {
        rustls::crypto::verify_tls12_signature(
            message,
            cert,
            dss,
            &self.provider.signature_verification_algorithms,
        )
    }

    fn verify_tls13_signature(
        &self,
        message: &[u8],
        cert: &rustls::pki_types::CertificateDer<'_>,
        dss: &rustls::DigitallySignedStruct,
    ) -> Result<rustls::client::danger::HandshakeSignatureValid, rustls::Error> {
        rustls::crypto::verify_tls13_signature(
            message,
            cert,
            dss,
            &self.provider.signature_verification_algorithms,
        )
    }

    fn supported_verify_schemes(&self) -> Vec<rustls::SignatureScheme> {
        self.provider
            .signature_verification_algorithms
            .supported_schemes()
    }
}

impl RemoteAgentClient {
    pub fn new(base_url: String, token: String) -> Self {
        Self::new_with_pin(base_url, token, None)
    }

    /// Build a client that optionally pins the agent's TLS cert by SHA-256
    /// fingerprint. When `fingerprint` is `Some`, TLS verification rejects any
    /// cert whose DER SHA-256 doesn't match — a stronger guarantee than CA-based
    /// trust for self-signed agent certs. When `None`, falls back to the legacy
    /// `AGENT_TLS_VERIFY=insecure` env flag so old agents (without fingerprint
    /// reporting) still work while their first checkin captures a pin.
    pub fn new_with_pin(
        base_url: String,
        token: String,
        fingerprint: Option<String>,
    ) -> Self {
        let http = if let Some(fp) = fingerprint {
            let verifier = Arc::new(PinnedFingerprintVerifier::new(fp));
            let tls_config = rustls::ClientConfig::builder()
                .dangerous()
                .with_custom_certificate_verifier(verifier)
                .with_no_client_auth();
            reqwest::Client::builder()
                .timeout(Duration::from_secs(60))
                .use_preconfigured_tls(tls_config)
                .pool_max_idle_per_host(5)
                .build()
                .unwrap_or_default()
        } else {
            let accept_invalid = std::env::var("AGENT_TLS_VERIFY")
                .map(|v| v == "insecure")
                .unwrap_or(false);
            if !accept_invalid {
                tracing::warn!(
                    "Remote agent {base_url} has no pinned fingerprint — TLS will require a valid CA cert. If the agent is self-signed (default), wait for first checkin to capture the pin, or set AGENT_TLS_VERIFY=insecure temporarily."
                );
            }
            reqwest::Client::builder()
                .timeout(Duration::from_secs(60))
                .danger_accept_invalid_certs(accept_invalid)
                .pool_max_idle_per_host(5)
                .build()
                .unwrap_or_default()
        };

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
        // 60s to match this client's own default request timeout, so the wait
        // cannot outlast the call it is waiting to make.
        let (_permit, _) =
            permit_within(&self.cb.semaphore, Duration::from_secs(60), "connection").await?;

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
            Err(e) => Err(remote_send_error(&self.cb, e)),
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

    /// PUT with an explicit timeout — the remote twin of [`AgentClient::put_long`].
    pub async fn put_long(
        &self,
        path: &str,
        body: serde_json::Value,
        timeout_secs: u64,
    ) -> Result<serde_json::Value, AgentError> {
        self.cb.check()?;
        // The LONG pool, matching the local twin this function is named after. The
        // third of three remote long-operation entry points that took the quick one.
        let (_permit, remaining) = permit_within(
            &self.cb.long_semaphore,
            Duration::from_secs(timeout_secs),
            "long operation",
        )
        .await?;

        let url = format!("{}{}", self.base_url, path);
        let result = self
            .http
            .put(&url)
            .header("authorization", format!("Bearer {}", self.token))
            .timeout(remaining)
            .json(&body)
            .send()
            .await;

        match result {
            Ok(resp) => {
                self.cb.record_success();
                let status = resp.status();
                let bytes = resp
                    .bytes()
                    .await
                    .map_err(|e| AgentError::Response(e.to_string()))?;

                if !status.is_success() {
                    let msg = String::from_utf8_lossy(&bytes).to_string();
                    return Err(AgentError::Status(status.as_u16(), msg));
                }

                serde_json::from_slice(&bytes).map_err(|e| AgentError::Parse(e.to_string()))
            }
            Err(e) => Err(remote_send_error(&self.cb, e)),
        }
    }

    pub async fn delete(&self, path: &str) -> Result<serde_json::Value, AgentError> {
        self.request(reqwest::Method::DELETE, path, None).await
    }

    pub async fn get_bytes(&self, path: &str) -> Result<(Vec<u8>, Option<String>), AgentError> {
        self.cb.check()?;
        let (_permit, remaining) =
            permit_within(&self.cb.semaphore, Duration::from_secs(120), "connection").await?;

        let url = format!("{}{}", self.base_url, path);
        let result = self.http.get(&url)
            .header("authorization", format!("Bearer {}", self.token))
            .timeout(remaining)
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
            Err(e) => Err(remote_send_error(&self.cb, e)),
        }
    }

    pub async fn post_long(
        &self,
        path: &str,
        body: Option<serde_json::Value>,
        timeout_secs: u64,
    ) -> Result<serde_json::Value, AgentError> {
        self.cb.check()?;
        // The LONG pool, as the local client has always used here. Taking the quick
        // pool meant `MAX_LONG_CONNECTIONS` bounded nothing on a fleet — every
        // remote build, import and restore competed with ordinary requests for the
        // same 20 permits, so the limit that exists to stop long work starving the
        // panel applied only to the machine the panel runs on.
        let (_permit, remaining) = permit_within(
            &self.cb.long_semaphore,
            Duration::from_secs(timeout_secs),
            "long operation",
        )
        .await?;

        let url = format!("{}{}", self.base_url, path);
        let mut req = self.http.post(&url)
            .header("authorization", format!("Bearer {}", self.token))
            .timeout(remaining);

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
            Err(e) => Err(remote_send_error(&self.cb, e)),
        }
    }

    /// POST with streaming NDJSON response (remote). Reads response chunks and
    /// calls `on_line` for each parsed JSON line. Used for streamed operations.
    ///
    /// As with the local client, `idle_timeout_secs` bounds silence rather than
    /// total runtime — see [`AgentClient::post_long_ndjson`].
    pub async fn post_long_ndjson<F>(
        &self,
        path: &str,
        body: Option<serde_json::Value>,
        idle_timeout_secs: u64,
        on_line: F,
    ) -> Result<(), AgentError>
    where
        F: Fn(serde_json::Value) + Send + 'static,
    {
        self.cb.check()?;
        // The LONG pool, as the local twin uses. See the note there on why the idle
        // figure bounds the wait.
        let (_permit, _) = permit_within(
            &self.cb.long_semaphore,
            Duration::from_secs(idle_timeout_secs),
            "long operation",
        )
        .await?;

        let ticks = Arc::new(AtomicU64::new(0));
        let probe = {
            let ticks = ticks.clone();
            move |json: serde_json::Value| {
                ticks.fetch_add(1, Ordering::Relaxed);
                on_line(json);
            }
        };

        let url = format!("{}{}", self.base_url, path);
        let mut req = self.http.post(&url)
            .header("authorization", format!("Bearer {}", self.token))
            .timeout(Duration::from_secs(STREAM_TOTAL_TIMEOUT_SECS));

        if let Some(b) = body {
            req = req.json(&b);
        }

        let streamed = async move {
            let resp = req.send().await
                .map_err(|e| AgentError::Connection(e.to_string()))?;

            let status = resp.status();
            if !status.is_success() {
                let bytes = resp.bytes().await
                    .map_err(|e| AgentError::Response(e.to_string()))?;
                let msg = String::from_utf8_lossy(&bytes).to_string();
                return Err(AgentError::Status(status.as_u16(), msg));
            }

            // Stream response chunks
            let mut buf = String::new();
            let mut stream = resp.bytes_stream();
            use futures::StreamExt;
            while let Some(chunk) = stream.next().await {
                let chunk = chunk.map_err(|e| AgentError::Response(e.to_string()))?;
                buf.push_str(&String::from_utf8_lossy(&chunk));
                while let Some(pos) = buf.find('\n') {
                    let line = &buf[..pos];
                    if !line.is_empty() {
                        if let Ok(json) = serde_json::from_str::<serde_json::Value>(line) {
                            probe(json);
                        }
                    }
                    buf = buf[pos + 1..].to_string();
                }
            }
            if !buf.trim().is_empty() {
                if let Ok(json) = serde_json::from_str::<serde_json::Value>(buf.trim()) {
                    probe(json);
                }
            }
            Ok(())
        };

        let result = drive_idle_bounded(idle_timeout_secs, ticks, streamed).await;

        // Success is now recorded only once the stream has actually completed.
        // The old code called record_success() the moment the response headers
        // arrived, so a connection that died halfway through the body still
        // counted as a healthy round-trip to the circuit breaker.
        match &result {
            Ok(_) => self.cb.record_success(),
            Err(AgentError::Connection(_)) => self.cb.record_failure(),
            _ => {}
        }

        result
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

    pub async fn put_long(
        &self,
        path: &str,
        body: serde_json::Value,
        timeout_secs: u64,
    ) -> Result<serde_json::Value, AgentError> {
        match self {
            Self::Local(c) => c.put_long(path, body, timeout_secs).await,
            Self::Remote(c) => c.put_long(path, body, timeout_secs).await,
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

    /// `idle_timeout_secs` bounds silence, not total runtime — see
    /// [`AgentClient::post_long_ndjson`].
    pub async fn post_long_ndjson<F>(
        &self,
        path: &str,
        body: Option<serde_json::Value>,
        idle_timeout_secs: u64,
        on_line: F,
    ) -> Result<(), AgentError>
    where
        F: Fn(serde_json::Value) + Send + 'static,
    {
        match self {
            Self::Local(c) => c.post_long_ndjson(path, body, idle_timeout_secs, on_line).await,
            Self::Remote(c) => c.post_long_ndjson(path, body, idle_timeout_secs, on_line).await,
        }
    }

    pub async fn token(&self) -> String {
        match self {
            Self::Local(c) => c.current_token().await,
            Self::Remote(c) => c.token.clone(),
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
    /// Key `servers.agent_token` is encrypted under. Held here because
    /// `for_server` is the single site that opens a stored dial-out token, and
    /// a struct field is a dependency the compiler checks — an env read inside
    /// the method would be a second source of truth for that key, silently
    /// returning ciphertext-as-token if it were ever unset.
    jwt_secret: String,
}

impl AgentRegistry {
    pub fn new(local: AgentClient, db: sqlx::PgPool, jwt_secret: String) -> Self {
        Self {
            local,
            local_server_id: Arc::new(RwLock::new(None)),
            remote_cache: Arc::new(RwLock::new(HashMap::new())),
            db,
            jwt_secret,
        }
    }

    /// The key stored credentials are encrypted under.
    ///
    /// Exposed so the UNATTENDED renewal loops can open a Cloudflare API token
    /// for a DNS-01 renewal. They hold an `AgentRegistry` and no `AppState`, and
    /// this struct is already the single place that key lives for background
    /// work — re-reading it from the environment there would be the second
    /// source of truth the field comment above exists to prevent.
    pub fn jwt_secret(&self) -> &str {
        &self.jwt_secret
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

        // Fetch from DB and cache. `is_local` is read too, and `agent_url` is
        // read as nullable, because the local row has NEITHER a URL nor a
        // fingerprint: `ensure_local_server` inserts it without one, since
        // nothing ever dials the local agent over TCP.
        let row: Option<(Option<String>, String, Option<String>, bool)> = sqlx::query_as(
            "SELECT agent_url, agent_token, cert_fingerprint, is_local FROM servers WHERE id = $1 AND status != 'pending'",
        )
        .bind(server_id)
        .fetch_optional(&self.db)
        .await
        .map_err(|e| AgentError::Connection(format!("DB lookup failed: {e}")))?;

        match row {
            // The local server, reached without the short-circuit above. That
            // happens whenever `local_server_id` is still None — which is a real
            // state, not a theoretical one: `ensure_local_server` returns
            // `Uuid::nil()` when no admin exists yet, so nothing sets the cached
            // id until someone completes setup or logs in.
            //
            // Without this arm the fallthrough asks the local row for an
            // `agent_url` it does not have and returns NotFound, so every
            // background service that resolves its OWN server would refuse to
            // act. Threading the fleet onto `for_server` is what makes that
            // reachable, and a single-server install is the commonest
            // deployment there is — so this arm is load-bearing for them, not
            // an edge case.
            Some((_, _, _, true)) => Ok(AgentHandle::Local(self.local.clone())),
            Some((Some(url), token, fingerprint, false)) if !url.is_empty() => {
                // The ONLY consumer of the stored dial-out token. `servers.agent_token`
                // is encrypted at rest by every writer (`routes::servers::create_server`,
                // `rotate_token`, and `ensure_local_server`); decrypt here — the single
                // choke point every remote agent call funnels through — so no read site
                // can forget to. `decrypt_credential_or_legacy` returns a pre-encryption
                // plaintext token unchanged, so an existing fleet keeps dialling without
                // a migration and without a re-enrolment.
                let token = crate::services::secrets_crypto::decrypt_credential_or_legacy(
                    &token,
                    &self.jwt_secret,
                );
                let client = RemoteAgentClient::new_with_pin(url, token, fingerprint);
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

    /// Every online server, resolved to its owner, its name and its own agent.
    ///
    /// This is the subject list for a background service whose subject is a
    /// MACHINE rather than a row — a scanner, or a health check that asks a host
    /// what it is running. A service driven by a table gets its host from the
    /// row and should use `for_server` directly instead.
    ///
    /// **A server whose agent will not resolve is SKIPPED, never substituted.**
    /// The registry also offers an `Option`-taking convenience resolver that
    /// answers a missing id with the local agent; reaching for it here is how a
    /// member's readings end up stamped with a member's id and taken from the
    /// panel, which is the whole class of defect this returns a fleet to avoid.
    pub async fn online_fleet(&self) -> Vec<FleetMember> {
        let rows: Vec<(Uuid, Uuid, String, bool)> = sqlx::query_as(
            "SELECT id, user_id, name, is_local FROM servers WHERE status = 'online' \
             ORDER BY is_local DESC",
        )
        .fetch_all(&self.db)
        .await
        .unwrap_or_default();

        let mut fleet = Vec::with_capacity(rows.len());
        for (id, user_id, name, is_local) in rows {
            match self.for_server(id).await {
                Ok(agent) => fleet.push(FleetMember { id, user_id, name, is_local, agent }),
                Err(e) => tracing::debug!("Fleet iteration: skipping {name} ({id}) — no agent: {e}"),
            }
        }
        fleet
    }
}

/// One online server, together with the identity every row written about it
/// must carry and the agent that can actually answer for it.
pub struct FleetMember {
    pub id: Uuid,
    pub user_id: Uuid,
    pub name: String,
    /// True for the box the API itself runs on. Load-bearing for anything that
    /// writes a row the check-in handler would otherwise have written: a member
    /// reports its own scalars by phoning home, and this host has nobody to
    /// phone. See `metrics_collector`.
    pub is_local: bool,
    pub agent: AgentHandle,
}

/// Ensure the local server row exists in the DB. Returns the local server UUID.
///
/// `jwt_secret` is threaded in rather than read from the environment because it
/// is the key `servers.agent_token` is encrypted under, and both callers already
/// hold it. An env read here would be an untestable second source of truth for
/// the one value that decides whether the column can be opened again.
pub async fn ensure_local_server(db: &sqlx::PgPool, agent_token: &str, jwt_secret: &str) -> Uuid {
    // Encrypted at rest like every other stored credential. The local row's
    // plaintext is never dialled — `AgentPool::for_server` short-circuits
    // `is_local` to the Unix-socket handle before it looks at the token — so
    // this row is write-only in practice; it is encrypted anyway so that "no
    // plaintext credential lives in this column" is true of every row, which is
    // what a sweep, a dump and the published promise can each rely on.
    let agent_token_enc = match crate::services::secrets_crypto::encrypt_credential(
        agent_token,
        jwt_secret,
    ) {
        Ok(enc) => enc,
        Err(e) => {
            tracing::error!(error = %e, "could not encrypt the local agent token; storing it unencrypted");
            agent_token.to_string()
        }
    };

    // Check if a local server already exists
    let existing: Option<(Uuid,)> =
        sqlx::query_as("SELECT id FROM servers WHERE is_local = true LIMIT 1")
            .fetch_optional(db)
            .await
            .unwrap_or(None);

    if let Some((id,)) = existing {
        // Update token + hash if changed
        let token_hash = crate::helpers::hash_agent_token(agent_token);
        let _ = sqlx::query("UPDATE servers SET agent_token = $1, agent_token_hash = $2, status = 'online' WHERE id = $3")
            .bind(&agent_token_enc)
            .bind(&token_hash)
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

    let token_hash = crate::helpers::hash_agent_token(agent_token);
    let row: (Uuid,) = sqlx::query_as(
        "INSERT INTO servers (id, user_id, name, agent_token, agent_token_hash, status, is_local) \
         VALUES (gen_random_uuid(), $1, 'This Server', $2, $3, 'online', true) \
         RETURNING id",
    )
    .bind(user_id)
    .bind(&agent_token_enc)
    .bind(&token_hash)
    .fetch_one(db)
    .await
    .expect("Failed to create local server row");

    tracing::info!("Registered local server: {}", row.0);
    row.0
}
