-- Phase 3: Secrets Manager — encrypted secret storage with versioning

-- Secret vaults (project/site scoping)
CREATE TABLE IF NOT EXISTS secret_vaults (
    id UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    user_id UUID NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    name VARCHAR(100) NOT NULL,
    description VARCHAR(500),
    -- Scoping: null = global, set = site-specific
    site_id UUID REFERENCES sites(id) ON DELETE CASCADE,
    server_id UUID REFERENCES servers(id) ON DELETE SET NULL,
    created_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT NOW()
);
CREATE INDEX IF NOT EXISTS idx_secret_vaults_user ON secret_vaults(user_id);
CREATE INDEX IF NOT EXISTS idx_secret_vaults_site ON secret_vaults(site_id);

-- Individual secrets (encrypted key-value pairs)
CREATE TABLE IF NOT EXISTS secrets (
    id UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    vault_id UUID NOT NULL REFERENCES secret_vaults(id) ON DELETE CASCADE,
    key VARCHAR(200) NOT NULL,
    -- Encrypted value (AES-256-GCM, base64-encoded ciphertext)
    encrypted_value TEXT NOT NULL,
    -- Metadata
    description VARCHAR(500),
    -- Type hint for UI
    secret_type VARCHAR(20) NOT NULL DEFAULT 'env', -- env, api_key, password, certificate, custom
    -- Auto-inject into .env on deploy
    auto_inject BOOLEAN NOT NULL DEFAULT FALSE,
    -- Version tracking
    version INTEGER NOT NULL DEFAULT 1,
    updated_by VARCHAR(255),
    created_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    UNIQUE(vault_id, key)
);
CREATE INDEX IF NOT EXISTS idx_secrets_vault ON secrets(vault_id);

-- Secret version history (audit trail)
CREATE TABLE IF NOT EXISTS secret_versions (
    id UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    secret_id UUID NOT NULL REFERENCES secrets(id) ON DELETE CASCADE,
    version INTEGER NOT NULL,
    -- Encrypted old value
    encrypted_value TEXT NOT NULL,
    changed_by VARCHAR(255),
    change_type VARCHAR(20) NOT NULL DEFAULT 'update', -- create, update, rotate
    created_at TIMESTAMPTZ NOT NULL DEFAULT NOW()
);
CREATE INDEX IF NOT EXISTS idx_secret_versions_secret ON secret_versions(secret_id, version DESC);
