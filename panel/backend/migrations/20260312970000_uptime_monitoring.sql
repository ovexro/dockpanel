-- Uptime monitors
CREATE TABLE monitors (
    id UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    user_id UUID NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    site_id UUID REFERENCES sites(id) ON DELETE SET NULL,
    url VARCHAR(500) NOT NULL,
    name VARCHAR(100) NOT NULL,
    check_interval INTEGER NOT NULL DEFAULT 60,
    status VARCHAR(20) NOT NULL DEFAULT 'pending',
    last_checked_at TIMESTAMPTZ,
    last_response_time INTEGER,
    last_status_code INTEGER,
    enabled BOOLEAN NOT NULL DEFAULT TRUE,
    alert_email BOOLEAN NOT NULL DEFAULT TRUE,
    alert_slack_url VARCHAR(500),
    alert_discord_url VARCHAR(500),
    created_at TIMESTAMPTZ NOT NULL DEFAULT NOW()
);

CREATE INDEX idx_monitors_user ON monitors(user_id);
CREATE INDEX idx_monitors_enabled ON monitors(enabled, status) WHERE enabled = TRUE;

-- Monitor check history (last 24h kept, older purged)
CREATE TABLE monitor_checks (
    id UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    monitor_id UUID NOT NULL REFERENCES monitors(id) ON DELETE CASCADE,
    status_code INTEGER,
    response_time INTEGER,
    error VARCHAR(500),
    checked_at TIMESTAMPTZ NOT NULL DEFAULT NOW()
);

CREATE INDEX idx_monitor_checks_monitor ON monitor_checks(monitor_id, checked_at DESC);

-- Incidents (downtime periods)
CREATE TABLE incidents (
    id UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    monitor_id UUID NOT NULL REFERENCES monitors(id) ON DELETE CASCADE,
    started_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    resolved_at TIMESTAMPTZ,
    cause VARCHAR(500),
    alerted BOOLEAN NOT NULL DEFAULT FALSE
);

CREATE INDEX idx_incidents_monitor ON incidents(monitor_id, started_at DESC);
