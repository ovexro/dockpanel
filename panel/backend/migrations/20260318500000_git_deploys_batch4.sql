-- Batch 4: GitHub integration, previews, scheduled deploys
ALTER TABLE git_deploys ADD COLUMN github_token TEXT;
ALTER TABLE git_deploys ADD COLUMN deploy_cron VARCHAR(100);

CREATE TABLE git_previews (
    id UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    git_deploy_id UUID NOT NULL REFERENCES git_deploys(id) ON DELETE CASCADE,
    branch VARCHAR(255) NOT NULL,
    container_name VARCHAR(200) NOT NULL,
    container_id VARCHAR(64),
    host_port INT NOT NULL,
    domain VARCHAR(255),
    status VARCHAR(20) NOT NULL DEFAULT 'deploying',
    commit_hash VARCHAR(40),
    created_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    UNIQUE(git_deploy_id, branch)
);
