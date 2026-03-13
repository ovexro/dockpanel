-- Alert rules: user-configurable thresholds per server
CREATE TABLE alert_rules (
    id UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    user_id UUID NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    server_id UUID REFERENCES servers(id) ON DELETE CASCADE,
    cpu_threshold INTEGER NOT NULL DEFAULT 90,
    cpu_duration INTEGER NOT NULL DEFAULT 5,
    memory_threshold INTEGER NOT NULL DEFAULT 90,
    memory_duration INTEGER NOT NULL DEFAULT 5,
    disk_threshold INTEGER NOT NULL DEFAULT 85,
    alert_cpu BOOLEAN NOT NULL DEFAULT TRUE,
    alert_memory BOOLEAN NOT NULL DEFAULT TRUE,
    alert_disk BOOLEAN NOT NULL DEFAULT TRUE,
    alert_offline BOOLEAN NOT NULL DEFAULT TRUE,
    alert_backup_failure BOOLEAN NOT NULL DEFAULT TRUE,
    alert_ssl_expiry BOOLEAN NOT NULL DEFAULT TRUE,
    alert_service_health BOOLEAN NOT NULL DEFAULT TRUE,
    ssl_warning_days VARCHAR(50) NOT NULL DEFAULT '30,14,7,3,1',
    notify_email BOOLEAN NOT NULL DEFAULT TRUE,
    notify_slack_url VARCHAR(500),
    notify_discord_url VARCHAR(500),
    cooldown_minutes INTEGER NOT NULL DEFAULT 60,
    created_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    UNIQUE(user_id, server_id)
);

CREATE INDEX idx_alert_rules_user ON alert_rules(user_id);

-- Alert history
CREATE TABLE alerts (
    id UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    user_id UUID NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    server_id UUID REFERENCES servers(id) ON DELETE SET NULL,
    site_id UUID REFERENCES sites(id) ON DELETE SET NULL,
    alert_type VARCHAR(30) NOT NULL,
    severity VARCHAR(10) NOT NULL DEFAULT 'warning',
    title VARCHAR(200) NOT NULL,
    message TEXT NOT NULL,
    status VARCHAR(20) NOT NULL DEFAULT 'firing',
    notified_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    resolved_at TIMESTAMPTZ,
    acknowledged_at TIMESTAMPTZ,
    created_at TIMESTAMPTZ NOT NULL DEFAULT NOW()
);

CREATE INDEX idx_alerts_user_status ON alerts(user_id, status, created_at DESC);
CREATE INDEX idx_alerts_server ON alerts(server_id, alert_type, created_at DESC);

-- Alert state machine: tracks current state per alert source
CREATE TABLE alert_state (
    id UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    server_id UUID REFERENCES servers(id) ON DELETE CASCADE,
    site_id UUID REFERENCES sites(id) ON DELETE CASCADE,
    alert_type VARCHAR(30) NOT NULL,
    state_key VARCHAR(100) NOT NULL DEFAULT '',
    current_state VARCHAR(20) NOT NULL DEFAULT 'ok',
    fired_at TIMESTAMPTZ,
    last_notified_at TIMESTAMPTZ,
    consecutive_count INTEGER NOT NULL DEFAULT 0,
    metadata JSONB
);

CREATE UNIQUE INDEX idx_alert_state_server ON alert_state(server_id, alert_type, state_key) WHERE server_id IS NOT NULL;
CREATE UNIQUE INDEX idx_alert_state_site ON alert_state(site_id, alert_type, state_key) WHERE site_id IS NOT NULL;

-- Add disk usage columns to servers
ALTER TABLE servers ADD COLUMN IF NOT EXISTS disk_used_gb INTEGER;
ALTER TABLE servers ADD COLUMN IF NOT EXISTS disk_usage_pct REAL;
ALTER TABLE servers ADD COLUMN IF NOT EXISTS mem_usage_pct REAL;
