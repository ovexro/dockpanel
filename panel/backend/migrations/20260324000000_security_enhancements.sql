-- Security Enhancements Migration (13 features, post-incident hardening)
-- Date: 2026-03-24

-- ═══════════════════════════════════════════════════════════════════════
-- Feature 7: Immutable Security Audit Log
-- Cannot be UPDATEd or DELETEd (enforced by trigger)
-- ═══════════════════════════════════════════════════════════════════════
CREATE TABLE IF NOT EXISTS security_audit_log (
    id UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    event_type VARCHAR(100) NOT NULL,  -- login, login_failed, register, site.create, terminal.command, terminal.blocked, lockdown, panic, etc.
    actor_email VARCHAR(255),
    actor_ip VARCHAR(45),
    target_type VARCHAR(50),           -- user, site, terminal, system
    target_name VARCHAR(255),
    details TEXT,
    geo_country VARCHAR(100),
    geo_city VARCHAR(100),
    geo_isp VARCHAR(255),
    severity VARCHAR(20) DEFAULT 'info',  -- info, warning, critical
    created_at TIMESTAMPTZ NOT NULL DEFAULT NOW()
);

CREATE INDEX IF NOT EXISTS idx_security_audit_log_created ON security_audit_log(created_at DESC);
CREATE INDEX IF NOT EXISTS idx_security_audit_log_event ON security_audit_log(event_type);
CREATE INDEX IF NOT EXISTS idx_security_audit_log_actor ON security_audit_log(actor_email);
CREATE INDEX IF NOT EXISTS idx_security_audit_log_severity ON security_audit_log(severity);

-- Trigger: prevent UPDATE and DELETE on security_audit_log
CREATE OR REPLACE FUNCTION prevent_audit_modification()
RETURNS TRIGGER AS $$
BEGIN
    RAISE EXCEPTION 'Security audit log is immutable: % operations are not allowed', TG_OP;
    RETURN NULL;
END;
$$ LANGUAGE plpgsql;

DROP TRIGGER IF EXISTS trg_immutable_audit_log ON security_audit_log;
CREATE TRIGGER trg_immutable_audit_log
    BEFORE UPDATE OR DELETE ON security_audit_log
    FOR EACH ROW
    EXECUTE FUNCTION prevent_audit_modification();

-- ═══════════════════════════════════════════════════════════════════════
-- Feature 5: Terminal Session Recordings metadata
-- Actual recordings stored as files in /var/lib/dockpanel/recordings/
-- ═══════════════════════════════════════════════════════════════════════
CREATE TABLE IF NOT EXISTS terminal_recordings (
    id UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    user_email VARCHAR(255) NOT NULL,
    domain VARCHAR(255) NOT NULL,       -- site domain or 'server'
    filename VARCHAR(255) NOT NULL,     -- recording file path
    size_bytes BIGINT DEFAULT 0,
    duration_secs INTEGER DEFAULT 0,
    started_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    ended_at TIMESTAMPTZ,
    created_at TIMESTAMPTZ NOT NULL DEFAULT NOW()
);

CREATE INDEX IF NOT EXISTS idx_terminal_recordings_user ON terminal_recordings(user_email);
CREATE INDEX IF NOT EXISTS idx_terminal_recordings_created ON terminal_recordings(created_at DESC);

-- ═══════════════════════════════════════════════════════════════════════
-- Feature 8: Registration Approval Mode
-- ═══════════════════════════════════════════════════════════════════════
ALTER TABLE users ADD COLUMN IF NOT EXISTS approved BOOLEAN DEFAULT TRUE;
ALTER TABLE users ADD COLUMN IF NOT EXISTS approved_at TIMESTAMPTZ;
ALTER TABLE users ADD COLUMN IF NOT EXISTS approved_by UUID;

-- ═══════════════════════════════════════════════════════════════════════
-- Feature 9 + 11: Lockdown State
-- Tracks whether system is in lockdown (auto or manual panic)
-- ═══════════════════════════════════════════════════════════════════════
CREATE TABLE IF NOT EXISTS lockdown_state (
    id INTEGER PRIMARY KEY DEFAULT 1 CHECK (id = 1),  -- singleton row
    active BOOLEAN NOT NULL DEFAULT FALSE,
    triggered_by VARCHAR(50),   -- 'auto' or 'panic' or 'admin'
    triggered_at TIMESTAMPTZ,
    reason TEXT,
    terminals_disabled BOOLEAN DEFAULT TRUE,
    registration_disabled BOOLEAN DEFAULT TRUE,
    non_admin_blocked BOOLEAN DEFAULT TRUE,
    unlocked_at TIMESTAMPTZ,
    unlocked_by VARCHAR(255)
);

INSERT INTO lockdown_state (id, active) VALUES (1, FALSE) ON CONFLICT DO NOTHING;

-- ═══════════════════════════════════════════════════════════════════════
-- Feature 12: Canary Files tracking
-- ═══════════════════════════════════════════════════════════════════════
CREATE TABLE IF NOT EXISTS canary_files (
    id UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    file_path VARCHAR(500) NOT NULL UNIQUE,
    description VARCHAR(255),
    last_triggered_at TIMESTAMPTZ,
    trigger_count INTEGER DEFAULT 0,
    created_at TIMESTAMPTZ NOT NULL DEFAULT NOW()
);

-- ═══════════════════════════════════════════════════════════════════════
-- Feature 13: Backup Integrity Chain
-- ═══════════════════════════════════════════════════════════════════════
ALTER TABLE backups ADD COLUMN IF NOT EXISTS sha256_hash VARCHAR(64);
ALTER TABLE backups ADD COLUMN IF NOT EXISTS previous_hash VARCHAR(64);
ALTER TABLE backups ADD COLUMN IF NOT EXISTS chain_valid BOOLEAN DEFAULT TRUE;

-- ═══════════════════════════════════════════════════════════════════════
-- Feature 1: Settings for new security features
-- ═══════════════════════════════════════════════════════════════════════
INSERT INTO settings (key, value) VALUES
    ('security_geo_alert_enabled', 'true'),
    ('security_approval_required', 'false'),
    ('security_lockdown_threshold', '5'),       -- suspicious events
    ('security_lockdown_window_minutes', '10'),  -- within this many minutes
    ('security_site_rate_limit', '3'),           -- max sites per hour
    ('security_session_recording', 'true'),
    ('security_canary_enabled', 'true'),
    ('security_db_backup_enabled', 'true'),
    ('security_db_backup_retention_days', '7'),
    ('security_backup_chain_enabled', 'true')
ON CONFLICT (key) DO NOTHING;

-- ═══════════════════════════════════════════════════════════════════════
-- Feature 4: Suspicious event counter for auto-lockdown
-- ═══════════════════════════════════════════════════════════════════════
CREATE TABLE IF NOT EXISTS suspicious_events (
    id UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    event_type VARCHAR(100) NOT NULL,
    actor_email VARCHAR(255),
    actor_ip VARCHAR(45),
    details TEXT,
    created_at TIMESTAMPTZ NOT NULL DEFAULT NOW()
);

CREATE INDEX IF NOT EXISTS idx_suspicious_events_created ON suspicious_events(created_at DESC);
