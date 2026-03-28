-- Zero-downtime PHP deploys via atomic symlink swap
ALTER TABLE deploy_configs ADD COLUMN atomic_deploy BOOLEAN NOT NULL DEFAULT FALSE;
ALTER TABLE deploy_configs ADD COLUMN keep_releases INTEGER NOT NULL DEFAULT 5;
