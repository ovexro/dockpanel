-- Migration Wizard: track import jobs from cPanel/Plesk/HestiaCP

CREATE TABLE IF NOT EXISTS migrations (
    id UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    user_id UUID NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    server_id UUID REFERENCES servers(id) ON DELETE SET NULL,
    source VARCHAR(20) NOT NULL,         -- 'cpanel', 'plesk', 'hestiacp'
    status VARCHAR(20) NOT NULL DEFAULT 'analyzing',  -- analyzing, analyzed, importing, done, failed
    backup_path TEXT NOT NULL,
    inventory JSONB,
    selected_items JSONB,
    result JSONB,
    created_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT NOW()
);

CREATE INDEX IF NOT EXISTS idx_migrations_user ON migrations(user_id);
