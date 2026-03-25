use lettre::{
    message::{header::ContentType, Mailbox},
    transport::smtp::{
        authentication::Credentials,
        client::{Tls, TlsParameters},
    },
    AsyncSmtpTransport, AsyncTransport, Message, Tokio1Executor,
};
use sqlx::PgPool;

/// Read SMTP settings from DB and send an email.
/// Returns Ok(()) if sent, Err(message) if SMTP not configured or send failed.
pub async fn send_email(
    pool: &PgPool,
    to: &str,
    subject: &str,
    body_html: &str,
) -> Result<(), String> {
    // Read SMTP settings from DB
    let rows: Vec<(String, String)> =
        sqlx::query_as("SELECT key, value FROM settings WHERE key LIKE 'smtp_%'")
            .fetch_all(pool)
            .await
            .map_err(|e| format!("DB error: {e}"))?;

    let get = |key: &str| -> Option<String> {
        rows.iter()
            .find(|(k, _)| k == key)
            .map(|(_, v)| v.clone())
            .filter(|v| !v.is_empty())
    };

    let host = get("smtp_host").ok_or("SMTP not configured")?;
    let port: u16 = get("smtp_port")
        .unwrap_or_else(|| "587".to_string())
        .parse()
        .unwrap_or(587);
    let username = get("smtp_username").ok_or("SMTP username not configured")?;
    let password_raw = get("smtp_password").ok_or("SMTP password not configured")?;
    // Decrypt the password (handles legacy plaintext values gracefully)
    let password = crate::services::secrets_crypto::decrypt_credential_from_env(&password_raw);
    let from_email = get("smtp_from").ok_or("SMTP from address not configured")?;
    let from_name = get("smtp_from_name").unwrap_or_else(|| "DockPanel".to_string());
    let encryption = get("smtp_encryption").unwrap_or_else(|| "starttls".to_string());

    let from: Mailbox = format!("{from_name} <{from_email}>")
        .parse()
        .map_err(|e| format!("Invalid from address: {e}"))?;

    let to_mailbox: Mailbox = to
        .parse()
        .map_err(|e| format!("Invalid to address: {e}"))?;

    let email = Message::builder()
        .from(from)
        .to(to_mailbox)
        .subject(subject)
        .header(ContentType::TEXT_HTML)
        .body(body_html.to_string())
        .map_err(|e| format!("Failed to build email: {e}"))?;

    let creds = Credentials::new(username, password);

    let transport = match encryption.as_str() {
        "ssl" | "tls" => {
            let tls = TlsParameters::new(host.clone())
                .map_err(|e| format!("TLS error: {e}"))?;
            AsyncSmtpTransport::<Tokio1Executor>::relay(&host)
                .map_err(|e| format!("SMTP relay error: {e}"))?
                .port(port)
                .tls(Tls::Wrapper(tls))
                .credentials(creds)
                .build()
        }
        _ => {
            // STARTTLS (default)
            AsyncSmtpTransport::<Tokio1Executor>::starttls_relay(&host)
                .map_err(|e| format!("SMTP relay error: {e}"))?
                .port(port)
                .credentials(creds)
                .build()
        }
    };

    transport
        .send(email)
        .await
        .map_err(|e| format!("Failed to send email: {e}"))?;

    Ok(())
}

/// Send email verification link.
pub async fn send_verification_email(
    pool: &PgPool,
    to: &str,
    token: &str,
    base_url: &str,
) -> Result<(), String> {
    let link = format!("{base_url}/verify-email?token={token}");
    let body = format!(
        r#"<div style="font-family: sans-serif; max-width: 600px; margin: 0 auto;">
            <h2 style="color: #4f46e5;">Verify your email</h2>
            <p>Click the button below to verify your DockPanel account:</p>
            <p style="margin: 24px 0;">
                <a href="{link}" style="background-color: #4f46e5; color: white; padding: 12px 24px; text-decoration: none; border-radius: 8px; font-weight: 600;">
                    Verify Email
                </a>
            </p>
            <p style="color: #6b7280; font-size: 14px;">Or copy this link: {link}</p>
            <p style="color: #9ca3af; font-size: 12px;">This link expires in 24 hours. If you didn't create an account, ignore this email.</p>
        </div>"#
    );

    send_email(pool, to, "Verify your DockPanel account", &body).await
}

/// Send password reset link.
pub async fn send_reset_email(
    pool: &PgPool,
    to: &str,
    token: &str,
    base_url: &str,
) -> Result<(), String> {
    let link = format!("{base_url}/reset-password?token={token}");
    let body = format!(
        r#"<div style="font-family: sans-serif; max-width: 600px; margin: 0 auto;">
            <h2 style="color: #4f46e5;">Reset your password</h2>
            <p>Click the button below to reset your DockPanel password:</p>
            <p style="margin: 24px 0;">
                <a href="{link}" style="background-color: #4f46e5; color: white; padding: 12px 24px; text-decoration: none; border-radius: 8px; font-weight: 600;">
                    Reset Password
                </a>
            </p>
            <p style="color: #6b7280; font-size: 14px;">Or copy this link: {link}</p>
            <p style="color: #9ca3af; font-size: 12px;">This link expires in 1 hour. If you didn't request this, ignore this email.</p>
        </div>"#
    );

    send_email(pool, to, "Reset your DockPanel password", &body).await
}
