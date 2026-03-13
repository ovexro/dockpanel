-- Backup destinations (admin-managed remote storage targets)
CREATE TABLE backup_destinations (
    id UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    name VARCHAR(100) NOT NULL,
    dtype VARCHAR(20) NOT NULL CHECK (dtype IN ('s3', 'sftp')),
    config JSONB NOT NULL DEFAULT '{}',
    created_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT NOW()
);

-- Backup schedules (per-site, references a destination)
CREATE TABLE backup_schedules (
    id UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    site_id UUID NOT NULL REFERENCES sites(id) ON DELETE CASCADE,
    destination_id UUID REFERENCES backup_destinations(id) ON DELETE SET NULL,
    schedule VARCHAR(100) NOT NULL,
    retention_count INTEGER NOT NULL DEFAULT 7,
    enabled BOOLEAN NOT NULL DEFAULT TRUE,
    last_run TIMESTAMPTZ,
    last_status VARCHAR(20),
    created_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    UNIQUE(site_id)
);

CREATE INDEX idx_backup_schedules_enabled ON backup_schedules(enabled) WHERE enabled = true;
