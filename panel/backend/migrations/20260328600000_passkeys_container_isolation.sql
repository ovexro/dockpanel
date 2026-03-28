-- Passkey/WebAuthn credentials
CREATE TABLE passkeys (
    id UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    user_id UUID NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    credential_id TEXT NOT NULL UNIQUE,
    public_key_cbor BYTEA NOT NULL,
    sign_count BIGINT NOT NULL DEFAULT 0,
    name VARCHAR(255) NOT NULL DEFAULT 'My Passkey',
    transports TEXT,
    aaguid TEXT,
    created_at TIMESTAMPTZ NOT NULL DEFAULT NOW()
);
CREATE INDEX idx_passkeys_user_id ON passkeys(user_id);
CREATE INDEX idx_passkeys_credential_id ON passkeys(credential_id);

-- Per-user container isolation policies
CREATE TABLE container_policies (
    id UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    user_id UUID NOT NULL REFERENCES users(id) ON DELETE CASCADE UNIQUE,
    max_containers INT NOT NULL DEFAULT 10,
    max_memory_mb BIGINT NOT NULL DEFAULT 4096,
    max_cpu_percent INT NOT NULL DEFAULT 400,
    network_isolation BOOLEAN NOT NULL DEFAULT false,
    allowed_images TEXT,
    created_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT NOW()
);
CREATE INDEX idx_container_policies_user_id ON container_policies(user_id);
