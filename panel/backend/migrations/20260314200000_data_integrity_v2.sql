-- Data integrity improvements: CHECK constraints, uniqueness, indexes

-- CHECK constraints on status/type fields
ALTER TABLE sites ADD CONSTRAINT chk_sites_status
    CHECK (status IN ('creating', 'active', 'error', 'suspended'));
ALTER TABLE sites ADD CONSTRAINT chk_sites_runtime
    CHECK (runtime IN ('static', 'php', 'proxy'));
ALTER TABLE servers ADD CONSTRAINT chk_servers_status
    CHECK (status IN ('pending', 'online', 'offline'));
ALTER TABLE users ADD CONSTRAINT chk_users_role
    CHECK (role IN ('admin', 'user'));
ALTER TABLE databases ADD CONSTRAINT chk_databases_engine
    CHECK (engine IN ('postgres', 'mysql', 'mariadb'));
ALTER TABLE alerts ADD CONSTRAINT chk_alerts_severity
    CHECK (severity IN ('critical', 'warning', 'info'));
ALTER TABLE alerts ADD CONSTRAINT chk_alerts_status
    CHECK (status IN ('firing', 'acknowledged', 'resolved'));

-- CHECK constraints on alert thresholds
ALTER TABLE alert_rules ADD CONSTRAINT chk_cpu_threshold
    CHECK (cpu_threshold BETWEEN 1 AND 100);
ALTER TABLE alert_rules ADD CONSTRAINT chk_memory_threshold
    CHECK (memory_threshold BETWEEN 1 AND 100);
ALTER TABLE alert_rules ADD CONSTRAINT chk_disk_threshold
    CHECK (disk_threshold BETWEEN 1 AND 100);
ALTER TABLE alert_rules ADD CONSTRAINT chk_cooldown_minutes
    CHECK (cooldown_minutes >= 1);

-- Fix alert_rules UNIQUE constraint for NULL server_id
-- Drop the existing broken constraint (NULL != NULL in PostgreSQL)
ALTER TABLE alert_rules DROP CONSTRAINT IF EXISTS alert_rules_user_id_server_id_key;
-- Create partial unique indexes that handle NULL correctly
CREATE UNIQUE INDEX IF NOT EXISTS idx_alert_rules_user_global
    ON alert_rules(user_id) WHERE server_id IS NULL;
CREATE UNIQUE INDEX IF NOT EXISTS idx_alert_rules_user_server
    ON alert_rules(user_id, server_id) WHERE server_id IS NOT NULL;

-- Uniqueness: server names per user
CREATE UNIQUE INDEX IF NOT EXISTS idx_servers_user_name
    ON servers(user_id, name);

-- Uniqueness: team names per owner
CREATE UNIQUE INDEX IF NOT EXISTS idx_teams_owner_name
    ON teams(owner_id, name);

-- Uniqueness: team invites (prevent duplicate invites)
CREATE UNIQUE INDEX IF NOT EXISTS idx_team_invites_team_email
    ON team_invites(team_id, email);

-- Missing index on alert_state for type+state queries
CREATE INDEX IF NOT EXISTS idx_alert_state_type_state
    ON alert_state(alert_type, current_state);

-- Port uniqueness index (if not already present)
CREATE UNIQUE INDEX IF NOT EXISTS idx_databases_port_unique
    ON databases(port) WHERE port IS NOT NULL;

-- Email length constraint (RFC 5321: max 254 chars)
ALTER TABLE users ADD CONSTRAINT chk_users_email_length
    CHECK (length(email) <= 254);
