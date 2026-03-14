use http_body_util::{BodyExt, Full};
use hyper::body::Bytes;
use hyper::client::conn::http1;
use hyper::Request;
use hyper_util::rt::TokioIo;
use std::fmt;
use std::time::Duration;
use tokio::net::UnixStream;

#[derive(Debug)]
pub enum AgentError {
    Connection(String),
    Request(String),
    Response(String),
    Status(u16, String),
    Parse(String),
}

impl fmt::Display for AgentError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Connection(e) => write!(f, "agent connection failed: {e}"),
            Self::Request(e) => write!(f, "agent request failed: {e}"),
            Self::Response(e) => write!(f, "agent response error: {e}"),
            Self::Status(code, msg) => write!(f, "agent returned {code}: {msg}"),
            Self::Parse(e) => write!(f, "agent response parse error: {e}"),
        }
    }
}

#[derive(Clone)]
pub struct AgentClient {
    socket_path: String,
    token: String,
}

impl AgentClient {
    pub fn new(socket_path: String, token: String) -> Self {
        Self { socket_path, token }
    }

    async fn request(
        &self,
        method: &str,
        path: &str,
        body: Option<serde_json::Value>,
    ) -> Result<serde_json::Value, AgentError> {
        tokio::time::timeout(Duration::from_secs(60), self.request_inner(method, path, body))
            .await
            .map_err(|_| AgentError::Request("agent request timed out after 60s".into()))?
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
