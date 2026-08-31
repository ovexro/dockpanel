-- `on_call_schedules`/`escalation_policies` have carried ZERO tenant scoping
-- since they shipped (20260516000000_on_call_escalation.sql): no user_id
-- column on either table, and every route in on_call.rs / escalation_policies.rs
-- gates on AdminUser (role) alone, never ownership -- the exact bug class
-- v2.188.0/v2.189.0 fixed repeatedly elsewhere in the same arc, just never
-- swept in this pair. Confirmed UI-reachable (Alerts.tsx, Settings.tsx) with
-- zero realized harm on this box (0 rows, 1 admin) but a real cross-tenant
-- leak/write on any install with a second admin.
--
-- Backfill order, cheapest-signal-first, mirroring
-- 20260830000000_status_page_subscribers_owner.sql's own precedent:
--   escalation_policies <- the alert_rules row that references it (earliest
--     by created_at, id) -- alert_rules has carried user_id since its very
--     first migration (20260313000000_alerting_system.sql), so this is real
--     tenant provenance, not a guess.
--   on_call_schedules   <- the escalation_policy (owned by the pass above)
--     whose steps JSONB contains a route naming this schedule (earliest by
--     created_at, id).
--   Either table, if still unresolved after its own signal: the install's
--     very first user -- total by construction, since creating either row
--     required an authenticated admin to already exist.

ALTER TABLE escalation_policies
    ADD COLUMN IF NOT EXISTS user_id UUID REFERENCES users(id) ON DELETE CASCADE;

UPDATE escalation_policies p
   SET user_id = r.user_id
  FROM (
      SELECT DISTINCT ON (escalation_policy_id) escalation_policy_id, user_id
        FROM alert_rules
       WHERE escalation_policy_id IS NOT NULL
       ORDER BY escalation_policy_id, created_at ASC, id ASC
  ) r
 WHERE p.id = r.escalation_policy_id
   AND p.user_id IS NULL;

UPDATE escalation_policies p
   SET user_id = u.id
  FROM (SELECT id FROM users ORDER BY created_at ASC, id ASC LIMIT 1) u
 WHERE p.user_id IS NULL;

ALTER TABLE on_call_schedules
    ADD COLUMN IF NOT EXISTS user_id UUID REFERENCES users(id) ON DELETE CASCADE;

UPDATE on_call_schedules s
   SET user_id = r.user_id
  FROM (
      SELECT DISTINCT ON (schedule_id) schedule_id, user_id
        FROM (
            SELECT substring(elem->>'route' from 'on_call_schedule:(.+)')::uuid AS schedule_id,
                   p.user_id AS user_id,
                   p.created_at AS created_at,
                   p.id AS id
              FROM escalation_policies p, jsonb_array_elements(p.steps) elem
             WHERE elem->>'route' LIKE 'on_call_schedule:%'
        ) x
       ORDER BY schedule_id, created_at ASC, id ASC
  ) r
 WHERE s.id = r.schedule_id
   AND s.user_id IS NULL;

UPDATE on_call_schedules s
   SET user_id = u.id
  FROM (SELECT id FROM users ORDER BY created_at ASC, id ASC LIMIT 1) u
 WHERE s.user_id IS NULL;

DO $$
DECLARE
    remaining INT;
BEGIN
    -- Guarded rather than unconditional, per 20260807000000's own rule (and
    -- 20260830000000's own precedent): a migration that aborts is worse than
    -- a column that stays nullable. Reachable only if `users` itself is
    -- empty, which cannot happen on an install that has ever created either
    -- row (both require an authenticated admin session).
    SELECT COUNT(*) INTO remaining FROM escalation_policies WHERE user_id IS NULL;
    IF remaining = 0 THEN
        ALTER TABLE escalation_policies ALTER COLUMN user_id SET NOT NULL;
    ELSE
        RAISE WARNING 'escalation_policies still has % row(s) with no user_id; leaving the column nullable', remaining;
    END IF;

    SELECT COUNT(*) INTO remaining FROM on_call_schedules WHERE user_id IS NULL;
    IF remaining = 0 THEN
        ALTER TABLE on_call_schedules ALTER COLUMN user_id SET NOT NULL;
    ELSE
        RAISE WARNING 'on_call_schedules still has % row(s) with no user_id; leaving the column nullable', remaining;
    END IF;
END $$;

CREATE INDEX IF NOT EXISTS idx_escalation_policies_user ON escalation_policies(user_id);
CREATE INDEX IF NOT EXISTS idx_on_call_schedules_user ON on_call_schedules(user_id);
