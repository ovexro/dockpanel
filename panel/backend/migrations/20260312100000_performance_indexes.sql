-- Performance indexes

-- sites.status — filtered on dashboard, used in status updates
CREATE INDEX IF NOT EXISTS idx_sites_status ON sites(status);

-- activity_logs.user_id — foreign key, filtered in user-scoped queries
CREATE INDEX IF NOT EXISTS idx_activity_logs_user_id ON activity_logs(user_id) WHERE user_id IS NOT NULL;

-- backups(site_id, created_at) — composite for paginated backup listing
DROP INDEX IF EXISTS idx_backups_site_id;
CREATE INDEX IF NOT EXISTS idx_backups_site_created ON backups(site_id, created_at DESC);

-- sites(user_id, created_at) — composite for paginated site listing
DROP INDEX IF EXISTS idx_sites_user_id;
CREATE INDEX IF NOT EXISTS idx_sites_user_created ON sites(user_id, created_at DESC);
