-- Gap #66: Token blacklist persistence (survives restarts)
CREATE TABLE IF NOT EXISTS token_blacklist (
    jti TEXT PRIMARY KEY,
    expires_at TIMESTAMPTZ NOT NULL
);

CREATE INDEX IF NOT EXISTS idx_token_blacklist_expires ON token_blacklist (expires_at);
