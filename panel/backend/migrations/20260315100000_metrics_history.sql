-- Historical metrics for dashboard charts
CREATE TABLE IF NOT EXISTS metrics_history (
    id BIGSERIAL PRIMARY KEY,
    cpu_pct REAL NOT NULL,
    mem_pct REAL NOT NULL,
    disk_pct REAL NOT NULL,
    created_at TIMESTAMPTZ NOT NULL DEFAULT NOW()
);

CREATE INDEX idx_metrics_history_created_at ON metrics_history(created_at DESC);

-- Retention: keep 7 days max (cleaned by background service)
