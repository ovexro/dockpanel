-- Git Push-to-Deploy: connect a Git repo → build Docker image → deploy container
CREATE TABLE git_deploys (
    id UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    user_id UUID NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    name VARCHAR(100) NOT NULL,
    repo_url TEXT NOT NULL,
    branch VARCHAR(100) NOT NULL DEFAULT 'main',
    dockerfile VARCHAR(500) NOT NULL DEFAULT 'Dockerfile',
    container_port INT NOT NULL DEFAULT 3000,
    host_port INT NOT NULL,
    domain VARCHAR(255),
    env_vars JSONB NOT NULL DEFAULT '{}',
    auto_deploy BOOLEAN NOT NULL DEFAULT FALSE,
    webhook_secret VARCHAR(64) NOT NULL,
    deploy_key_public TEXT,
    deploy_key_path TEXT,
    container_id VARCHAR(64),
    image_tag VARCHAR(255),
    status VARCHAR(20) NOT NULL DEFAULT 'pending',
    memory_mb INT,
    cpu_percent INT,
    last_deploy TIMESTAMPTZ,
    last_commit VARCHAR(40),
    created_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    UNIQUE(name),
    UNIQUE(host_port)
);

CREATE INDEX idx_git_deploys_user ON git_deploys(user_id);

-- Deploy history for rollback
CREATE TABLE git_deploy_history (
    id UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    git_deploy_id UUID NOT NULL REFERENCES git_deploys(id) ON DELETE CASCADE,
    commit_hash VARCHAR(40) NOT NULL,
    commit_message TEXT,
    image_tag VARCHAR(255) NOT NULL,
    status VARCHAR(20) NOT NULL,
    output TEXT,
    triggered_by VARCHAR(50) NOT NULL DEFAULT 'manual',
    duration_ms INTEGER,
    created_at TIMESTAMPTZ NOT NULL DEFAULT NOW()
);

CREATE INDEX idx_git_deploy_history ON git_deploy_history(git_deploy_id, created_at DESC);
