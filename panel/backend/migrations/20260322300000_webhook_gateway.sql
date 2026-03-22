-- Phase 4: Webhook Gateway — receive, inspect, route, and forward webhooks

-- Inbound webhook endpoints (each gets a unique URL)
CREATE TABLE IF NOT EXISTS webhook_endpoints (
    id UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    user_id UUID NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    name VARCHAR(100) NOT NULL,
    description VARCHAR(500),
    -- The endpoint token (unique slug in the URL: /api/webhooks/gateway/{token})
    token VARCHAR(64) NOT NULL UNIQUE,
    -- Optional HMAC secret for signature verification of incoming requests
    verify_secret VARCHAR(200),
    -- Verify mode: none, hmac_sha256, hmac_sha1
    verify_mode VARCHAR(20) NOT NULL DEFAULT 'none',
    -- Header name containing the signature (e.g., X-Hub-Signature-256)
    verify_header VARCHAR(100),
    -- Status
    enabled BOOLEAN NOT NULL DEFAULT TRUE,
    -- Stats
    total_received INTEGER NOT NULL DEFAULT 0,
    last_received_at TIMESTAMPTZ,
    created_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT NOW()
);
CREATE INDEX IF NOT EXISTS idx_webhook_endpoints_user ON webhook_endpoints(user_id);
CREATE INDEX IF NOT EXISTS idx_webhook_endpoints_token ON webhook_endpoints(token);

-- Inbound webhook deliveries (log of received requests)
CREATE TABLE IF NOT EXISTS webhook_deliveries (
    id UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    endpoint_id UUID NOT NULL REFERENCES webhook_endpoints(id) ON DELETE CASCADE,
    -- Request details
    method VARCHAR(10) NOT NULL DEFAULT 'POST',
    headers JSONB NOT NULL DEFAULT '{}',
    body TEXT,
    query_string VARCHAR(2000),
    source_ip VARCHAR(45),
    -- Verification
    signature_valid BOOLEAN,
    -- Forwarding status
    forwarded BOOLEAN NOT NULL DEFAULT FALSE,
    forward_status INTEGER,
    forward_response TEXT,
    forward_duration_ms INTEGER,
    -- Timestamps
    received_at TIMESTAMPTZ NOT NULL DEFAULT NOW()
);
CREATE INDEX IF NOT EXISTS idx_webhook_deliveries_endpoint ON webhook_deliveries(endpoint_id, received_at DESC);

-- Webhook routes (forward incoming webhooks to destinations)
CREATE TABLE IF NOT EXISTS webhook_routes (
    id UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    endpoint_id UUID NOT NULL REFERENCES webhook_endpoints(id) ON DELETE CASCADE,
    name VARCHAR(100) NOT NULL,
    -- Destination URL
    destination_url TEXT NOT NULL,
    -- Optional filter: only forward if body contains this JSON path/value
    filter_path VARCHAR(200),
    filter_value VARCHAR(500),
    -- Transform: header overrides (JSONB: {"Authorization": "Bearer xxx"})
    extra_headers JSONB NOT NULL DEFAULT '{}',
    -- Retry config
    retry_count INTEGER NOT NULL DEFAULT 3,
    retry_delay_secs INTEGER NOT NULL DEFAULT 5,
    -- Status
    enabled BOOLEAN NOT NULL DEFAULT TRUE,
    -- Stats
    total_forwarded INTEGER NOT NULL DEFAULT 0,
    last_forwarded_at TIMESTAMPTZ,
    last_status INTEGER,
    created_at TIMESTAMPTZ NOT NULL DEFAULT NOW()
);
CREATE INDEX IF NOT EXISTS idx_webhook_routes_endpoint ON webhook_routes(endpoint_id);
