-- A security scan belongs to the machine that was scanned.
--
-- `security_scans.server_id` and `file_integrity_baselines.server_id` have
-- existed since the table was created and NOTHING has ever written either one.
-- Both writers — the weekly background scanner and the manual trigger route —
-- sent the scan to the correct agent and then recorded the result with a NULL
-- host, so on a fleet every member's scan was filed as nobody's.
--
-- The file-integrity half is worse than mislabelled, it is INERT. The upsert
-- spells `ON CONFLICT (server_id, file_path)` while the INSERT omits
-- `server_id`, and Postgres treats NULLs as distinct in a unique index, so the
-- arbiter has never once matched: every scan APPENDED a new baseline row
-- instead of updating one. This panel is carrying 126 rows for 7 watched paths
-- — 18 duplicates each, one per weekly scan since March. The comparison read
-- has no ORDER BY, so it returns the oldest row: the alarm fires on the FIRST
-- change to a watched file and then keeps firing for ever, and a second,
-- different change is indistinguishable from the first. `/etc/shadow`,
-- `/etc/sudoers` and `/etc/ssh/sshd_config` are in that watched set.
--
-- ⚠ ORDER MATTERS BELOW. Backfilling before deduplicating would collapse 18
-- rows onto each of 7 keys and violate UNIQUE(server_id, file_path), aborting
-- the migration. And the dedupe cannot key on `created_at` — this table has no
-- such column, only `updated_at`, which happens to carry insert time here
-- precisely BECAUSE the upsert that would have touched it never ran.

-- 1. Every scan taken so far was taken on the panel host.
UPDATE security_scans
   SET server_id = (SELECT id FROM servers WHERE is_local = true LIMIT 1)
 WHERE server_id IS NULL;

-- 2. Keep the NEWEST baseline per watched path, drop the rest. `id` breaks ties
--    so the result does not depend on scan order.
DELETE FROM file_integrity_baselines a
      USING file_integrity_baselines b
      WHERE a.server_id IS NULL
        AND b.server_id IS NULL
        AND a.file_path = b.file_path
        AND (a.updated_at, a.id) < (b.updated_at, b.id);

-- 3. Now the backfill cannot collide.
UPDATE file_integrity_baselines
   SET server_id = (SELECT id FROM servers WHERE is_local = true LIMIT 1)
 WHERE server_id IS NULL;

-- 4. Anything still unattributed has no local server row to belong to, which
--    means it cannot be compared against any future scan either. Drop it rather
--    than leave a row that is unreachable by every reader.
DELETE FROM security_scans             WHERE server_id IS NULL;
DELETE FROM file_integrity_baselines   WHERE server_id IS NULL;

-- 5. NOT NULL is the point of the exercise, not decoration: it is what makes an
--    INSERT that forgets to bind the host fail loudly at the first scan instead
--    of silently recreating the inert-upsert bug this migration exists to end.
ALTER TABLE security_scans           ALTER COLUMN server_id SET NOT NULL;
ALTER TABLE file_integrity_baselines ALTER COLUMN server_id SET NOT NULL;

-- The comparison read is `WHERE server_id = $1 AND file_path = $2`; the old
-- index led on server_id already but the baseline table had only the implicit
-- unique index, which serves this exactly. Nothing to add there. The scan
-- history list is per server, newest first:
CREATE INDEX IF NOT EXISTS idx_security_scans_server_created
    ON security_scans (server_id, created_at DESC);
