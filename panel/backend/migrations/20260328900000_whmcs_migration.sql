-- WHMCS billing integration
CREATE TABLE whmcs_config (
    id UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    api_url VARCHAR(512) NOT NULL,
    api_identifier VARCHAR(255) NOT NULL,
    api_secret_encrypted TEXT NOT NULL,
    auto_provision BOOLEAN NOT NULL DEFAULT true,
    auto_suspend BOOLEAN NOT NULL DEFAULT true,
    auto_terminate BOOLEAN NOT NULL DEFAULT false,
    webhook_secret VARCHAR(255),
    created_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT NOW()
);

CREATE TABLE whmcs_service_map (
    id UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    whmcs_service_id INT NOT NULL UNIQUE,
    user_id UUID REFERENCES users(id) ON DELETE SET NULL,
    site_id UUID REFERENCES sites(id) ON DELETE SET NULL,
    plan VARCHAR(100) NOT NULL DEFAULT 'basic',
    status VARCHAR(50) NOT NULL DEFAULT 'active',
    provisioned_at TIMESTAMPTZ,
    created_at TIMESTAMPTZ NOT NULL DEFAULT NOW()
);
CREATE INDEX idx_whmcs_service_map_whmcs_id ON whmcs_service_map(whmcs_service_id);

-- App migration between servers
CREATE TABLE app_migrations (
    id UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    container_id VARCHAR(255) NOT NULL,
    container_name VARCHAR(255) NOT NULL,
    source_server_id UUID NOT NULL REFERENCES servers(id),
    target_server_id UUID NOT NULL REFERENCES servers(id),
    status VARCHAR(50) NOT NULL DEFAULT 'pending',
    progress_pct INT NOT NULL DEFAULT 0,
    error_message TEXT,
    started_at TIMESTAMPTZ,
    completed_at TIMESTAMPTZ,
    created_at TIMESTAMPTZ NOT NULL DEFAULT NOW()
);
CREATE INDEX idx_app_migrations_status ON app_migrations(status);
