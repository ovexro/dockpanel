-- GAP 58: One-time scheduled deploy for git deploys
ALTER TABLE git_deploys ADD COLUMN IF NOT EXISTS scheduled_deploy_at TIMESTAMPTZ;
