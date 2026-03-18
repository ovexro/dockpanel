-- Monitoring Batch 2: Ping, heartbeat, maintenance windows, custom headers, PagerDuty
ALTER TABLE monitors ADD COLUMN custom_headers JSONB;
ALTER TABLE monitors ADD COLUMN maintenance_until TIMESTAMPTZ;

CREATE TABLE maintenance_windows (
    id UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    user_id UUID NOT NULL REFERENCES users(id),
    name VARCHAR(255) NOT NULL,
    starts_at TIMESTAMPTZ NOT NULL,
    ends_at TIMESTAMPTZ NOT NULL,
    monitor_ids UUID[],
    created_at TIMESTAMPTZ NOT NULL DEFAULT NOW()
);

-- PagerDuty integration key stored in alert_rules
ALTER TABLE alert_rules ADD COLUMN notify_pagerduty_key VARCHAR(500);
