-- Point-in-time recovery configuration per database
CREATE TABLE db_pitr_config (
    id UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    database_id UUID NOT NULL REFERENCES databases(id) ON DELETE CASCADE UNIQUE,
    pitr_enabled BOOLEAN NOT NULL DEFAULT false,
    retention_hours INT NOT NULL DEFAULT 24,
    last_backup_at TIMESTAMPTZ,
    last_wal_at TIMESTAMPTZ,
    backup_size_bytes BIGINT NOT NULL DEFAULT 0,
    created_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT NOW()
);
CREATE INDEX idx_db_pitr_config_database_id ON db_pitr_config(database_id);

-- GPU configuration per container deployment
ALTER TABLE container_sleep_config ADD COLUMN IF NOT EXISTS gpu_enabled BOOLEAN NOT NULL DEFAULT false;
