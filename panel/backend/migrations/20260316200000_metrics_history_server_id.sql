-- Add server_id to metrics_history for multi-server charting
ALTER TABLE metrics_history ADD COLUMN IF NOT EXISTS server_id UUID REFERENCES servers(id) ON DELETE CASCADE;

CREATE INDEX IF NOT EXISTS idx_metrics_history_server ON metrics_history(server_id, created_at DESC);
