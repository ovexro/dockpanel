-- Batch 3: Build args, monorepo support
ALTER TABLE git_deploys ADD COLUMN build_args JSONB NOT NULL DEFAULT '{}';
ALTER TABLE git_deploys ADD COLUMN build_context VARCHAR(500) NOT NULL DEFAULT '.';
