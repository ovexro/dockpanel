-- Reseller / Multi-Tenant Account System

-- 0. Update role CHECK constraint to allow 'reseller'
ALTER TABLE users DROP CONSTRAINT IF EXISTS chk_users_role;
ALTER TABLE users ADD CONSTRAINT chk_users_role
    CHECK (role IN ('admin', 'reseller', 'user'));

-- 1. Add reseller_id to users (NULL = direct user, non-NULL = belongs to reseller)
ALTER TABLE users ADD COLUMN IF NOT EXISTS reseller_id UUID REFERENCES users(id) ON DELETE SET NULL;
CREATE INDEX IF NOT EXISTS idx_users_reseller ON users(reseller_id) WHERE reseller_id IS NOT NULL;

-- 2. Reseller profiles (one per reseller user)
CREATE TABLE IF NOT EXISTS reseller_profiles (
    id UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    user_id UUID NOT NULL UNIQUE REFERENCES users(id) ON DELETE CASCADE,
    -- Branding
    panel_name VARCHAR(100),
    -- Quotas (NULL = unlimited)
    max_users INTEGER DEFAULT 10,
    max_sites INTEGER DEFAULT 50,
    max_databases INTEGER DEFAULT 50,
    max_disk_mb BIGINT,
    max_email_accounts INTEGER,
    -- Denormalized usage counters (updated by app code)
    used_users INTEGER NOT NULL DEFAULT 0,
    used_sites INTEGER NOT NULL DEFAULT 0,
    used_databases INTEGER NOT NULL DEFAULT 0,
    created_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT NOW()
);

-- 3. Server allocation: which servers a reseller can deploy to
CREATE TABLE IF NOT EXISTS reseller_servers (
    id UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    reseller_id UUID NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    server_id UUID NOT NULL REFERENCES servers(id) ON DELETE CASCADE,
    created_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    UNIQUE(reseller_id, server_id)
);

CREATE INDEX IF NOT EXISTS idx_reseller_servers_reseller ON reseller_servers(reseller_id);
CREATE INDEX IF NOT EXISTS idx_reseller_servers_server ON reseller_servers(server_id);
