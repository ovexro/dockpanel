-- A container the panel stopped ON PURPOSE, so the alert engine can tell a
-- deliberate stop from a crash.
--
-- Until now it could not. `check_container_health` sees `exited`/`dead` and
-- fires `container_down`; nothing in `alert_engine.rs` referenced any sleep or
-- stop state, `container_down` is absent from `is_alert_enabled`'s match and
-- falls through to `_ => return true` so no alert rule can switch it off, and
-- the alert was raised at a hardcoded "critical" — which is the exact string the
-- incident branch gates on, so every expected stop also published a CRITICAL
-- incident on the operator's PUBLIC status page and re-paged for seven days.
-- The runbook shipped with that very alert says "a planned `docker stop` is
-- normal operational use, so we don't page on it".
--
-- ── Why a new table instead of a column on `container_sleep_config` ──────────
--
-- Three reasons, each of which bit a draft of this change:
--
--   1. THE KEY. `container_sleep_config` is UNIQUE on `container_id` alone. The
--      alert engine keys `alert_state` on (server_id, alert_type, state_key)
--      where state_key is the container NAME, so a suppression keyed on the id
--      does not join to the thing it suppresses. Names are also the stable half:
--      `update_app`, `change_container_image` and `update_env` all stop, remove
--      and re-create the container, which mints a NEW id and keeps the name — so
--      an id-keyed expectation is silently lost on every app Update, and an app
--      the operator deliberately left stopped would page the moment it was
--      updated. Keying on (server_id, container_name) survives a recreate and
--      matches what the alert is keyed on.
--
--   2. THE SERVER TERM. `container_sleep_config.server_id` was added later
--      (20260807000000) and its UNIQUE constraint was never widened to include
--      it. Container ids are 32 random bytes rather than content-addressed, so
--      cloning a provisioned VPS into a second fleet member — an ordinary
--      hosting workflow — gives two hosts byte-identical container ids and one
--      shared row. The UNIQUE below carries the server.
--
--   3. THE MEANING. `is_sleeping` belongs to the auto-sleep feature and is read
--      by its sweeper (`WHERE auto_sleep_enabled = true AND is_sleeping = false`).
--      Its well-known stuckness is currently the only thing bounding a domainless
--      container — whose `last_activity_at` nothing refreshes — to ONE sleep
--      instead of an unbounded stop/start loop. Clearing it from observation, as
--      an earlier draft of this change proposed, would have armed that loop
--      fleet-wide and unattended. This table is read by the alert engine and the
--      auto-healer's restart leg and by nothing else, which is what makes it safe
--      to clear a row the moment the container is observed running.
--
-- ── Why no backfill ─────────────────────────────────────────────────────────
--
-- An absent row means "no deliberate stop on record", which is the correct and
-- conservative reading of every container that exists when this migration runs.
-- Backfilling would silence a container that is genuinely down right now.

CREATE TABLE IF NOT EXISTS container_expected_stops (
    id UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    server_id UUID NOT NULL REFERENCES servers(id) ON DELETE CASCADE,
    -- The name as the agent reports it in /apps, i.e. carrying the
    -- `dockpanel-app-` prefix. Both sides of the join read that same field, so
    -- the representation never has to be agreed on separately.
    container_name VARCHAR(255) NOT NULL,
    -- 'operator_stop' | 'manual_sleep' | 'auto_sleep' | 'stack_stop'
    reason TEXT NOT NULL,
    -- Who asked for it. NULL for auto_sleep, which no person initiates.
    actor_email TEXT,
    stopped_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    UNIQUE (server_id, container_name)
);

CREATE INDEX IF NOT EXISTS idx_container_expected_stops_server
    ON container_expected_stops(server_id);
