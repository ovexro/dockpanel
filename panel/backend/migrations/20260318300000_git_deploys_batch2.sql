-- Batch 2: Auto-SSL, deploy hooks
ALTER TABLE git_deploys ADD COLUMN ssl_email VARCHAR(255);
ALTER TABLE git_deploys ADD COLUMN pre_build_cmd TEXT;
ALTER TABLE git_deploys ADD COLUMN post_deploy_cmd TEXT;
