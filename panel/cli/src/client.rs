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

pub async fn agent_get(path: &str, token: &str) -> Result<serde_json::Value, String> {
    if !Path::new(SOCKET_PATH).exists() {
        return Err(format!(
            "Agent socket not found at {SOCKET_PATH}\nIs dockpanel-agent running? Check: systemctl status dockpanel-agent"
        ));
    }

    let mut stream = UnixStream::connect(SOCKET_PATH)
        .await
        .map_err(|e| format!("Cannot connect to agent: {e}"))?;

    let request = format!(
        "GET {path} HTTP/1.1\r\nHost: localhost\r\nAuthorization: Bearer {token}\r\nConnection: close\r\n\r\n"
    );
    stream
        .write_all(request.as_bytes())
        .await
        .map_err(|e| format!("Failed to send request: {e}"))?;

    // Read response (Connection: close means server will close when done)
    let mut buf = Vec::with_capacity(8192);
    let mut tmp = [0u8; 4096];
    loop {
        match stream.read(&mut tmp).await {
            Ok(0) => break,
            Ok(n) => buf.extend_from_slice(&tmp[..n]),
            Err(e) => return Err(format!("Failed to read response: {e}")),
        }
    }

    // Find \r\n\r\n separator between headers and body
    let separator = b"\r\n\r\n";
    let sep_pos = buf
        .windows(4)
        .position(|w| w == separator)
        .ok_or_else(|| "Invalid HTTP response: no header/body separator".to_string())?;

    let headers = String::from_utf8_lossy(&buf[..sep_pos]);
    let body = &buf[sep_pos + 4..];

    // Check status line
    let first_line = headers.lines().next().unwrap_or("");
    if !first_line.contains("200") {
        return Err(format!("Agent returned: {first_line}"));
    }

    // Check for chunked encoding
    let is_chunked = headers.to_lowercase().contains("transfer-encoding: chunked");

    let json_bytes = if is_chunked {
        decode_chunked(body)
    } else {
        body.to_vec()
    };

    serde_json::from_slice(&json_bytes).map_err(|e| {
        let preview = String::from_utf8_lossy(&json_bytes[..json_bytes.len().min(200)]);
        format!("Invalid JSON from agent: {e}\nBody: {preview}")
    })
}

fn decode_chunked(data: &[u8]) -> Vec<u8> {
    let mut result = Vec::new();
    let mut pos = 0;

    loop {
        // Find end of chunk size line
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

        // Skip chunk data + trailing \r\n
        pos = chunk_end + 2;
        if pos >= data.len() {
            break;
        }
    }

    result
}
