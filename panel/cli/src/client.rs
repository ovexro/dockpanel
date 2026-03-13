use std::path::Path;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::UnixStream;

const SOCKET_PATH: &str = "/var/run/dockpanel/agent.sock";
const TOKEN_PATH: &str = "/etc/dockpanel/agent.token";

pub fn load_token() -> Result<String, String> {
    std::fs::read_to_string(TOKEN_PATH)
        .map(|t| t.trim().to_string())
        .map_err(|e| format!("Cannot read agent token at {TOKEN_PATH}: {e}\nAre you running as root?"))
}

async fn agent_request(
    method: &str,
    path: &str,
    body: Option<&serde_json::Value>,
    token: &str,
) -> Result<serde_json::Value, String> {
    if !Path::new(SOCKET_PATH).exists() {
        return Err(format!(
            "Agent socket not found at {SOCKET_PATH}\nIs dockpanel-agent running? Check: systemctl status dockpanel-agent"
        ));
    }

    let mut stream = UnixStream::connect(SOCKET_PATH)
        .await
        .map_err(|e| format!("Cannot connect to agent: {e}"))?;

    let body_bytes = body.map(|b| serde_json::to_vec(b).unwrap_or_default());

    let mut request = format!(
        "{method} {path} HTTP/1.1\r\nHost: localhost\r\nAuthorization: Bearer {token}\r\nConnection: close\r\n"
    );

    if let Some(ref bytes) = body_bytes {
        request.push_str(&format!(
            "Content-Type: application/json\r\nContent-Length: {}\r\n",
            bytes.len()
        ));
    }

    request.push_str("\r\n");

    stream
        .write_all(request.as_bytes())
        .await
        .map_err(|e| format!("Failed to send request: {e}"))?;

    if let Some(bytes) = &body_bytes {
        stream
            .write_all(bytes)
            .await
            .map_err(|e| format!("Failed to send body: {e}"))?;
    }

    let mut buf = Vec::with_capacity(8192);
    let mut tmp = [0u8; 4096];
    loop {
        match stream.read(&mut tmp).await {
            Ok(0) => break,
            Ok(n) => buf.extend_from_slice(&tmp[..n]),
            Err(e) => return Err(format!("Failed to read response: {e}")),
        }
    }

    let separator = b"\r\n\r\n";
    let sep_pos = buf
        .windows(4)
        .position(|w| w == separator)
        .ok_or_else(|| "Invalid HTTP response: no header/body separator".to_string())?;

    let headers = String::from_utf8_lossy(&buf[..sep_pos]);
    let body_raw = &buf[sep_pos + 4..];

    let first_line = headers.lines().next().unwrap_or("");
    if !first_line.contains("200") && !first_line.contains("201") {
        let is_chunked = headers.to_lowercase().contains("transfer-encoding: chunked");
        let err_bytes = if is_chunked {
            decode_chunked(body_raw)
        } else {
            body_raw.to_vec()
        };
        if let Ok(err_json) = serde_json::from_slice::<serde_json::Value>(&err_bytes) {
            if let Some(msg) = err_json["error"].as_str() {
                return Err(msg.to_string());
            }
            if let Some(msg) = err_json["message"].as_str() {
                return Err(msg.to_string());
            }
        }
        return Err(format!("Agent returned: {first_line}"));
    }

    let is_chunked = headers.to_lowercase().contains("transfer-encoding: chunked");

    let json_bytes = if is_chunked {
        decode_chunked(body_raw)
    } else {
        body_raw.to_vec()
    };

    if json_bytes.is_empty() {
        return Ok(serde_json::json!({"success": true}));
    }

    serde_json::from_slice(&json_bytes).map_err(|e| {
        let preview = String::from_utf8_lossy(&json_bytes[..json_bytes.len().min(200)]);
        format!("Invalid JSON from agent: {e}\nBody: {preview}")
    })
}

pub async fn agent_get(path: &str, token: &str) -> Result<serde_json::Value, String> {
    agent_request("GET", path, None, token).await
}

pub async fn agent_post(
    path: &str,
    body: &serde_json::Value,
    token: &str,
) -> Result<serde_json::Value, String> {
    agent_request("POST", path, Some(body), token).await
}

pub async fn agent_post_empty(path: &str, token: &str) -> Result<serde_json::Value, String> {
    agent_request("POST", path, None, token).await
}

pub async fn agent_put(
    path: &str,
    body: &serde_json::Value,
    token: &str,
) -> Result<serde_json::Value, String> {
    agent_request("PUT", path, Some(body), token).await
}

pub async fn agent_delete(path: &str, token: &str) -> Result<serde_json::Value, String> {
    agent_request("DELETE", path, None, token).await
}

fn decode_chunked(data: &[u8]) -> Vec<u8> {
    let mut result = Vec::new();
    let mut pos = 0;

    loop {
        let line_end = match data[pos..]
            .windows(2)
            .position(|w| w == b"\r\n")
        {
            Some(p) => pos + p,
            None => break,
        };

        let size_str = String::from_utf8_lossy(&data[pos..line_end]);
        let size = match usize::from_str_radix(size_str.trim(), 16) {
            Ok(s) => s,
            Err(_) => break,
        };

        if size == 0 {
            break;
        }

        let chunk_start = line_end + 2;
        let chunk_end = (chunk_start + size).min(data.len());
        result.extend_from_slice(&data[chunk_start..chunk_end]);

        pos = chunk_end + 2;
        if pos >= data.len() {
            break;
        }
    }

    result
}
