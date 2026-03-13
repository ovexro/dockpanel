-- Git deployment configuration per site
CREATE TABLE deploy_configs (
    id UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    site_id UUID NOT NULL REFERENCES sites(id) ON DELETE CASCADE,
    repo_url TEXT NOT NULL,
    branch VARCHAR(100) NOT NULL DEFAULT 'main',
    deploy_script TEXT NOT NULL DEFAULT '',
    auto_deploy BOOLEAN NOT NULL DEFAULT FALSE,
    webhook_secret VARCHAR(64) NOT NULL,
    deploy_key_public TEXT,
    deploy_key_path TEXT,
    last_deploy TIMESTAMPTZ,
    last_status VARCHAR(20),
    created_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    UNIQUE(site_id)
);

-- Deployment history / logs
CREATE TABLE deploy_logs (
    id UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    site_id UUID NOT NULL REFERENCES sites(id) ON DELETE CASCADE,
    commit_hash VARCHAR(40),
    status VARCHAR(20) NOT NULL,
    output TEXT,
    triggered_by VARCHAR(50) NOT NULL DEFAULT 'manual',
    duration_ms INTEGER,
    created_at TIMESTAMPTZ NOT NULL DEFAULT NOW()
);

CREATE INDEX idx_deploy_logs_site ON deploy_logs(site_id, created_at DESC);
