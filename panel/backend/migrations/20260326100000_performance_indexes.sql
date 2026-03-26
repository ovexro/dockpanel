-- Performance: composite indexes for frequently queried ORDER BY patterns
CREATE INDEX IF NOT EXISTS idx_git_deploys_user_created ON git_deploys (user_id, created_at DESC);
CREATE INDEX IF NOT EXISTS idx_monitors_user_created ON monitors (user_id, created_at DESC);
CREATE INDEX IF NOT EXISTS idx_secret_vaults_user_created ON secret_vaults (user_id, created_at DESC);
CREATE INDEX IF NOT EXISTS idx_mail_accounts_domain ON mail_accounts (domain_id, email);
CREATE INDEX IF NOT EXISTS idx_mail_aliases_domain ON mail_aliases (domain_id, source_email);
