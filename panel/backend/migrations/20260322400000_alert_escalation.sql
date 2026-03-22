-- Gap #9: Alert escalation — add escalated_at column to alerts table
-- Tracks when an unacknowledged alert was last escalated, so we can re-notify
-- every 30 minutes until someone acknowledges.
ALTER TABLE alerts ADD COLUMN IF NOT EXISTS escalated_at TIMESTAMPTZ;

-- Index for efficient escalation queries (unacknowledged firing alerts)
CREATE INDEX IF NOT EXISTS idx_alerts_escalation
    ON alerts(status, acknowledged_at, created_at)
    WHERE status = 'firing' AND acknowledged_at IS NULL;
