-- How many deploy-history rows a Git Deploy keeps. 0 = keep everything (the
-- behaviour before this column existed), so existing deploys are untouched.
ALTER TABLE git_deploys ADD COLUMN IF NOT EXISTS history_retention INT NOT NULL DEFAULT 0;
