-- Give `alerts` the entity key that `alert_state` has always had.
--
-- `alert_state` is keyed `(server_id, alert_type, state_key)` — enforced by
-- `idx_alert_state_server` — and every per-entity caller passes a real key: the
-- service name, the container name, the GPU index. `alerts` had no such column,
-- so `resolve_alert` could only scope its UPDATE to
-- `(user_id, server_id, alert_type)`.
--
-- The consequence was a silent false all-clear. With two containers down on one
-- server there are two firing `container_down` rows; the first container to
-- recover cleared its own `alert_state` row correctly and then resolved BOTH
-- alert rows. The second container was still down, its alert read `resolved`,
-- and because its `alert_state` row stayed `firing` the engine never re-fired it
-- — a transition-triggered engine cannot re-announce a condition it still
-- believes it has announced. The outage became invisible to the panel, to
-- `check_escalations`, and to the operator.
--
-- Empty string, not NULL, is the "this alert is about the whole server" key —
-- the same convention `alert_state.state_key` already uses (its own DEFAULT is
-- `''`), so the two tables now agree on both the key and its absent value. NULL
-- would have made every comparison need `IS NOT DISTINCT FROM` and would have
-- silently matched nothing under plain `=`.
ALTER TABLE alerts ADD COLUMN IF NOT EXISTS state_key VARCHAR(100) NOT NULL DEFAULT '';

-- The resolve path's exact lookup shape. Partial on `status = 'firing'` because
-- that is the only status it ever targets, and resolved rows are the bulk of the
-- table (the engine prunes them at 30 days, so they accumulate until then).
CREATE INDEX IF NOT EXISTS idx_alerts_resolve
    ON alerts (user_id, server_id, alert_type, state_key)
    WHERE status = 'firing';

-- Retire the `slow_response` rows that could never be resolved.
--
-- That alert type was given a dedup guard and no recovery path, so on any
-- install where a monitor was once slow there is a row stuck at 'firing'
-- forever. It is not merely cosmetic: `idx_alerts_escalation_sweep` selects on
-- (status = 'firing' AND acknowledged_at IS NULL), so `check_escalations` keeps
-- paging on-call about it, and the dedup guard reads the same row and suppresses
-- every future slow-response alert for that monitor. One stuck row both pages
-- forever and blinds the check.
--
-- These rows also carry the empty `state_key` just added above, so the new
-- resolve path — which keys on the monitor's id — can never reach them. Clearing
-- them here is the only way they ever clear. This is deliberately scoped to the
-- one alert type with no resolver: it is NOT a general "resolve everything"
-- sweep, and every other type's firing rows are left exactly as they are,
-- because for those a firing row means the condition is genuinely unresolved.
--
-- Safe against a monitor that is still slow: the next check re-fires it within
-- one interval, this time with a real `state_key` and a working recovery path.
UPDATE alerts
   SET status = 'resolved', resolved_at = NOW()
 WHERE alert_type = 'slow_response'
   AND status = 'firing';
