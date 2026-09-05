-- Loose-ends audit (first dockpanel sweep): monitors.maintenance_until was
-- added for a per-monitor maintenance-window override but never wired into
-- any INSERT/UPDATE/SELECT — the shipped feature uses the separate
-- maintenance_windows table instead (fully wired: monitors.rs create/list/
-- delete_maintenance, alert_engine.rs's maintenance_users check). Dead schema,
-- never written, never read; drop it.
ALTER TABLE monitors DROP COLUMN IF EXISTS maintenance_until;
