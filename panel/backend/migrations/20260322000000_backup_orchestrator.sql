-- Backup Orchestrator: database backups, volume backups, encryption, verification, policies

-- Extend backup_destinations with encryption support and more provider types
ALTER TABLE backup_destinations
    ADD COLUMN IF NOT EXISTS encryption_enabled BOOLEAN NOT NULL DEFAULT FALSE,
    ADD COLUMN IF NOT EXISTS encryption_key TEXT;

-- Drop and recreate the dtype check to allow b2 and gcs
ALTER TABLE backup_destinations DROP CONSTRAINT IF EXISTS backup_destinations_dtype_check;
ALTER TABLE backup_destinations ADD CONSTRAINT backup_destinations_dtype_check
    CHECK (dtype IN ('s3', 'sftp', 'b2', 'gcs'));

-- Backup policies: cross-resource backup orchestration
CREATE TABLE IF NOT EXISTS backup_policies (
    id UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    user_id UUID NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    server_id UUID REFERENCES servers(id) ON DELETE CASCADE,
    name VARCHAR(100) NOT NULL,
    -- What to back up
    backup_sites BOOLEAN NOT NULL DEFAULT TRUE,
    backup_databases BOOLEAN NOT NULL DEFAULT TRUE,
    backup_volumes BOOLEAN NOT NULL DEFAULT FALSE,
    -- Schedule (cron 5-field)
    schedule VARCHAR(100) NOT NULL DEFAULT '0 2 * * *',
    -- Where to send
    destination_id UUID REFERENCES backup_destinations(id) ON DELETE SET NULL,
    -- Retention
    retention_count INTEGER NOT NULL DEFAULT 7 CHECK (retention_count BETWEEN 1 AND 365),
    -- Encryption
    encrypt BOOLEAN NOT NULL DEFAULT FALSE,
    -- Verification
    verify_after_backup BOOLEAN NOT NULL DEFAULT FALSE,
    -- State
    enabled BOOLEAN NOT NULL DEFAULT TRUE,
    last_run TIMESTAMPTZ,
    last_status VARCHAR(20),
    created_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT NOW()
);
CREATE INDEX IF NOT EXISTS idx_backup_policies_user ON backup_policies(user_id);
CREATE INDEX IF NOT EXISTS idx_backup_policies_enabled ON backup_policies(enabled);

-- Database backups (separate from site file backups)
CREATE TABLE IF NOT EXISTS database_backups (
    id UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    database_id UUID NOT NULL REFERENCES databases(id) ON DELETE CASCADE,
    server_id UUID REFERENCES servers(id) ON DELETE SET NULL,
    filename TEXT NOT NULL,
    size_bytes BIGINT NOT NULL DEFAULT 0,
    db_type VARCHAR(20) NOT NULL, -- mysql, postgres, mongo
    db_name VARCHAR(100) NOT NULL,
    encrypted BOOLEAN NOT NULL DEFAULT FALSE,
    policy_id UUID REFERENCES backup_policies(id) ON DELETE SET NULL,
    destination_id UUID REFERENCES backup_destinations(id) ON DELETE SET NULL,
    uploaded BOOLEAN NOT NULL DEFAULT FALSE,
    created_at TIMESTAMPTZ NOT NULL DEFAULT NOW()
);
CREATE INDEX IF NOT EXISTS idx_database_backups_db ON database_backups(database_id);
CREATE INDEX IF NOT EXISTS idx_database_backups_created ON database_backups(created_at DESC);

-- Volume backups (Docker app volumes)
CREATE TABLE IF NOT EXISTS volume_backups (
    id UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    container_id VARCHAR(64) NOT NULL,
    container_name VARCHAR(200) NOT NULL,
    server_id UUID REFERENCES servers(id) ON DELETE SET NULL,
    volume_name VARCHAR(200) NOT NULL,
    filename TEXT NOT NULL,
    size_bytes BIGINT NOT NULL DEFAULT 0,
    encrypted BOOLEAN NOT NULL DEFAULT FALSE,
    policy_id UUID REFERENCES backup_policies(id) ON DELETE SET NULL,
    destination_id UUID REFERENCES backup_destinations(id) ON DELETE SET NULL,
    uploaded BOOLEAN NOT NULL DEFAULT FALSE,
    created_at TIMESTAMPTZ NOT NULL DEFAULT NOW()
);
CREATE INDEX IF NOT EXISTS idx_volume_backups_container ON volume_backups(container_name);
CREATE INDEX IF NOT EXISTS idx_volume_backups_created ON volume_backups(created_at DESC);

-- Backup verifications
CREATE TABLE IF NOT EXISTS backup_verifications (
    id UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    backup_type VARCHAR(20) NOT NULL, -- site, database, volume
    backup_id UUID NOT NULL,
    server_id UUID REFERENCES servers(id) ON DELETE SET NULL,
    status VARCHAR(20) NOT NULL DEFAULT 'pending', -- pending, running, passed, failed
    checks_run INTEGER NOT NULL DEFAULT 0,
    checks_passed INTEGER NOT NULL DEFAULT 0,
    details JSONB NOT NULL DEFAULT '{}',
    error_message TEXT,
    duration_ms INTEGER,
    started_at TIMESTAMPTZ,
    completed_at TIMESTAMPTZ,
    created_at TIMESTAMPTZ NOT NULL DEFAULT NOW()
);
CREATE INDEX IF NOT EXISTS idx_backup_verifications_type ON backup_verifications(backup_type, backup_id);
CREATE INDEX IF NOT EXISTS idx_backup_verifications_status ON backup_verifications(status);
CREATE INDEX IF NOT EXISTS idx_backup_verifications_created ON backup_verifications(created_at DESC);
