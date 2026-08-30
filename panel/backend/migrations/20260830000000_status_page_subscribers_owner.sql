-- `status_page_subscribers` has never had an owner. Every other status-page
-- table (`status_page_config`, `status_page_components`, `managed_incidents`)
-- carries `user_id` since `20260322100000_incident_management.sql`, and
-- `a2915418` (v2.171.0, the s418 fix) scoped every READ of the public status
-- page to the "winning" config row (`ORDER BY created_at ASC, id ASC LIMIT
-- 1`) — a pure query-logic change, no migration needed since the columns
-- already existed. But the fan-out worker in
-- `services::status_notices` still SELECTs every verified subscriber with no
-- filter at all, so on any install with more than one tenant, every
-- subscriber gets every tenant's incident/monitor notices, and the admin
-- `GET /api/status-page/subscribers` endpoint returns every tenant's
-- subscriber email list to any admin who calls it. Documented as a known,
-- deliberate limitation in `services/status_notices.rs`'s own doc comment
-- since it shipped; tracked in memory since s424; scoped and priced at s427.
--
-- Backfill order mirrors the s418 tie-break exactly (a config row's owner),
-- falling back to the install's very first user for the case
-- `services/public_status.rs` already documents as reachable: the global
-- `status_page_enabled` flag lives on a different settings screen than
-- `status_page_config`, so a subscriber can exist even when zero config rows
-- ever have. Total by construction for any install that has ever accepted a
-- subscriber at all: doing so required the app to already be running under at
-- least one user.

ALTER TABLE status_page_subscribers
    ADD COLUMN IF NOT EXISTS owner_id UUID REFERENCES users(id) ON DELETE CASCADE;

-- Pass 1: the config row that s418 already treats as "the published tenant."
UPDATE status_page_subscribers s
   SET owner_id = c.user_id
  FROM (SELECT user_id FROM status_page_config ORDER BY created_at ASC, id ASC LIMIT 1) c
 WHERE s.owner_id IS NULL;

-- Pass 2: installs with subscribers but zero `status_page_config` rows.
UPDATE status_page_subscribers s
   SET owner_id = u.id
  FROM (SELECT id FROM users ORDER BY created_at ASC, id ASC LIMIT 1) u
 WHERE s.owner_id IS NULL;

DO $$
DECLARE
    remaining INT;
BEGIN
    SELECT COUNT(*) INTO remaining FROM status_page_subscribers WHERE owner_id IS NULL;
    IF remaining = 0 THEN
        ALTER TABLE status_page_subscribers ALTER COLUMN owner_id SET NOT NULL;
    ELSE
        -- Guarded rather than unconditional, per 20260807000000's own rule: a
        -- migration that aborts is worse than a column that stays nullable.
        -- Reachable only if `users` itself is empty, which cannot happen on
        -- an install that has ever accepted a real subscription.
        RAISE WARNING 'status_page_subscribers still has % row(s) with no owner_id; leaving the column nullable', remaining;
    END IF;
END $$;

-- Create the scoped constraint BEFORE dropping the global one. If the create
-- fails, the old (strictly stronger for a single tenant) guarantee stays in
-- force rather than none. The same email may now legitimately subscribe to
-- two different tenants' pages.
ALTER TABLE status_page_subscribers
    ADD CONSTRAINT status_page_subscribers_owner_email_key UNIQUE (owner_id, email);
ALTER TABLE status_page_subscribers
    DROP CONSTRAINT IF EXISTS status_page_subscribers_email_key;

CREATE INDEX IF NOT EXISTS idx_status_page_subscribers_owner ON status_page_subscribers(owner_id);
