-- Telemetry: diagnostic event collection and reporting
CREATE TABLE telemetry_events (
    id UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    event_type VARCHAR(20) NOT NULL DEFAULT 'error',        -- panic, error, warning, info
    category VARCHAR(40) NOT NULL DEFAULT 'general',        -- agent, api, database, ssl, docker, mail, security, nginx, backup, general
    message TEXT NOT NULL,
    context JSONB DEFAULT '{}',                             -- structured metadata (route, status_code, etc.)
    system_info JSONB DEFAULT '{}',                         -- OS, arch, version snapshot at event time
    sent_at TIMESTAMPTZ,                                    -- NULL = not yet sent to remote
    created_at TIMESTAMPTZ NOT NULL DEFAULT NOW()
);

CREATE INDEX idx_telemetry_events_created ON telemetry_events (created_at DESC);
CREATE INDEX idx_telemetry_events_unsent ON telemetry_events (sent_at) WHERE sent_at IS NULL;
CREATE INDEX idx_telemetry_events_category ON telemetry_events (category, event_type);
