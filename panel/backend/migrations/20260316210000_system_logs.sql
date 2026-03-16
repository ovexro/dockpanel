CREATE TABLE IF NOT EXISTS system_logs (
    id UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    level TEXT NOT NULL CHECK (level IN ('error', 'warning', 'info')),
    source TEXT NOT NULL,         -- 'api', 'agent', 'backup_scheduler', 'alert_engine', 'auto_healer', 'ssl', 'nginx', etc.
    message TEXT NOT NULL,
    details TEXT,                  -- stack trace, request path, error context
    created_at TIMESTAMPTZ NOT NULL DEFAULT NOW()
);

CREATE INDEX IF NOT EXISTS idx_system_logs_created ON system_logs(created_at DESC);
CREATE INDEX IF NOT EXISTS idx_system_logs_level ON system_logs(level, created_at DESC);

-- Retention: keep 30 days
-- (auto_healer cleanup will handle this)
