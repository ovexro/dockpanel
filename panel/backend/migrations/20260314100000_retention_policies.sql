-- Indexes for efficient retention cleanup queries
CREATE INDEX IF NOT EXISTS idx_activity_logs_created ON activity_logs(created_at);
CREATE INDEX IF NOT EXISTS idx_monitor_checks_checked ON monitor_checks(checked_at);
CREATE INDEX IF NOT EXISTS idx_alerts_created_status ON alerts(created_at, status);

-- Composite index for activity audit queries
CREATE INDEX IF NOT EXISTS idx_activity_logs_user_action ON activity_logs(user_id, action, created_at DESC);
