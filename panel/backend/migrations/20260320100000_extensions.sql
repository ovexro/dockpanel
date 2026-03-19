-- Plugin/Extension API

CREATE TABLE IF NOT EXISTS extensions (
    id UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    user_id UUID NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    name VARCHAR(100) NOT NULL,
    description TEXT NOT NULL DEFAULT '',
    author VARCHAR(100) NOT NULL DEFAULT '',
    version VARCHAR(20) NOT NULL DEFAULT '1.0.0',
    webhook_url TEXT NOT NULL,
    webhook_secret TEXT NOT NULL,
    api_key_hash TEXT,
    api_key_prefix VARCHAR(16),
    enabled BOOLEAN NOT NULL DEFAULT TRUE,
    event_subscriptions TEXT NOT NULL DEFAULT '[]',
    api_scopes TEXT NOT NULL DEFAULT '[]',
    last_webhook_at TIMESTAMPTZ,
    last_webhook_status INTEGER,
    created_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT NOW()
);

CREATE INDEX IF NOT EXISTS idx_extensions_user ON extensions(user_id);
CREATE INDEX IF NOT EXISTS idx_extensions_enabled ON extensions(enabled) WHERE enabled = TRUE;
CREATE INDEX IF NOT EXISTS idx_extensions_api_key ON extensions(api_key_prefix);

CREATE TABLE IF NOT EXISTS extension_events (
    id UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    extension_id UUID NOT NULL REFERENCES extensions(id) ON DELETE CASCADE,
    event_type TEXT NOT NULL,
    payload TEXT NOT NULL,
    response_status INTEGER,
    response_body TEXT,
    duration_ms INTEGER,
    delivered_at TIMESTAMPTZ NOT NULL DEFAULT NOW()
);

CREATE INDEX IF NOT EXISTS idx_ext_events_extension ON extension_events(extension_id, delivered_at DESC);
