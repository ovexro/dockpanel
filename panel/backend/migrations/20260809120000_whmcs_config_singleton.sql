-- The WHMCS integration has never been configurable, on any install, since it
-- shipped in `3feb92f` on 2026-03-28.
--
-- `update_config` is the only writer of this table, and it is spelled
-- `INSERT ... ON CONFLICT ((true)) DO UPDATE`. Postgres resolves an ON CONFLICT
-- arbiter at PARSE-ANALYSIS time by looking for a unique or exclusion constraint
-- matching the target expression, and the only index on this table is the
-- primary key on a `gen_random_uuid()` default. No such constraint exists, so
-- the statement raises
--
--     there is no unique or exclusion constraint matching the ON CONFLICT specification
--
-- before it ever examines a row. That means it fails on the FIRST save as well
-- as on re-saves — this is not "the second save 500s", it is "the table can
-- never hold a row". Every reader downstream is therefore permanently dead:
-- `get_config` answers `{"configured": false}` and `webhook` answers 404
-- "WHMCS not configured" before it reads the secret, so the feature the manifest
-- advertises has no reachable state at all.
--
-- The fix is the constraint the statement always assumed it had. A unique index
-- on the constant expression `(true)` admits exactly one row, which is the
-- singleton semantics `((true))` was written for — so the existing statement
-- becomes correct as written rather than being rewritten around the defect.
--
-- The dedupe below is defensive and is expected to delete nothing: the only
-- writer could never insert, so every install should be at zero rows. It is kept
-- because a unique index cannot be created over duplicates, and a migration that
-- aborts on an install we did not anticipate is worse than one that keeps the
-- oldest row. Oldest wins for the same reason as
-- `20260805120000_status_page_config_unique.sql`: whatever a reader has been
-- selecting with `LIMIT 1` keeps being selected, so this cannot change behaviour.

DELETE FROM whmcs_config a
USING whmcs_config b
WHERE (b.created_at, b.id) < (a.created_at, a.id);

CREATE UNIQUE INDEX IF NOT EXISTS whmcs_config_singleton
    ON whmcs_config ((true));
