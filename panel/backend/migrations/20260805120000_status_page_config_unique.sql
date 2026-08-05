-- One status-page config row per operator.
--
-- `update_config` shipped with `INSERT ... ON CONFLICT DO NOTHING` and no
-- conflict target. The only unique constraint on the table was the primary key,
-- whose default is gen_random_uuid(), so the clause could never fire and every
-- PUT /api/status-page/config inserted another row. The UPDATE that followed
-- then matched N rows and returned an arbitrary one, while the public reader
-- took an unordered LIMIT 1 — so an operator could untick "Enabled", get a 200
-- and a form showing it off, and still be publishing from a different row.
--
-- Dedupe keeps the OLDEST row per operator, which is the one the public reader
-- now selects (ORDER BY created_at ASC, id ASC): whatever the page has been
-- serving keeps serving, so this migration cannot change what is published.
-- The newer rows are the accidental ones — each was created by a PUT that
-- immediately updated a different row.

DELETE FROM status_page_config a
USING status_page_config b
WHERE a.user_id = b.user_id
  AND (b.created_at, b.id) < (a.created_at, a.id);

DROP INDEX IF EXISTS idx_status_page_config_user;

CREATE UNIQUE INDEX IF NOT EXISTS idx_status_page_config_user
    ON status_page_config(user_id);
