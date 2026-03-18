-- Batch 5: Deploy protection
ALTER TABLE git_deploys ADD COLUMN IF NOT EXISTS deploy_protected BOOLEAN NOT NULL DEFAULT FALSE;
