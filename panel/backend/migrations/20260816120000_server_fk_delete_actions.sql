-- The last two foreign keys in the schema with no delete action, and they make a
-- supported administrative action permanently impossible — the same defect
-- `20260816000000_user_fk_delete_actions.sql` repaired for `users`, one parent table
-- over, where that migration's own census could not see it.
--
-- `app_migrations` records a WHMCS-driven container move between two servers. Both
-- `source_server_id` and `target_server_id` were declared `NOT NULL REFERENCES
-- servers(id)` with no ON DELETE, which Postgres reads as NO ACTION. Across the whole
-- catalogue these were the ONLY two foreign keys that could block a delete: 112 in
-- total, 79 CASCADE, 31 SET NULL, 2 NO ACTION, both of them here. (The adjacent shape
-- — SET NULL on a NOT NULL column, which raises just as surely — is 0 of 31.)
--
-- So a server that had ever been the source or target of an app migration could never
-- be deleted. `routes/servers.rs::remove` issues a bare `DELETE FROM servers`, the
-- constraint raises inside that statement, and `internal_error` turns it into
-- `Internal error (ref: <8 hex>)` with nothing naming the cause. The comment directly
-- above that DELETE says "Cascade delete removes sites, DBs, stacks, etc." — true of
-- the other 18 children and false of these two.
--
-- And unlike the `maintenance_windows` case there is no second edge and no escape
-- hatch of any kind: `app_migrations` is written by exactly one statement
-- (`routes/whmcs.rs`, the INSERT in `start_migration`) and there is no DELETE and no
-- UPDATE against it anywhere in the repository — eight occurrences in four files, all
-- accounted for. The blocking row is permanent by construction, so an operator could
-- not clear it even with administrator rights.
--
-- CASCADE, not SET NULL: both columns are NOT NULL, so SET NULL is not available. And
-- a migration record describes a move between two servers — with either endpoint gone
-- it has nothing left to say, exactly the argument that settled `maintenance_windows`.
--
-- The orphan sweeps are not decoration, for the reason the previous migration states:
-- `sqlx::migrate!` is unwrapped at startup, so a migration that can raise is a panel
-- that will not boot, and `ADD CONSTRAINT` validates every existing row. The old
-- constraint made orphans impossible while it stood, but this is precisely the defect
-- an operator would have worked around by hand in psql, and a hand-written cleanup
-- that dropped the constraint first is exactly the state that would brick the upgrade.
DELETE FROM app_migrations m
 WHERE NOT EXISTS (SELECT 1 FROM servers s WHERE s.id = m.source_server_id)
    OR NOT EXISTS (SELECT 1 FROM servers s WHERE s.id = m.target_server_id);

ALTER TABLE app_migrations
    DROP CONSTRAINT IF EXISTS app_migrations_source_server_id_fkey;
ALTER TABLE app_migrations
    ADD CONSTRAINT app_migrations_source_server_id_fkey
    FOREIGN KEY (source_server_id) REFERENCES servers(id) ON DELETE CASCADE;

ALTER TABLE app_migrations
    DROP CONSTRAINT IF EXISTS app_migrations_target_server_id_fkey;
ALTER TABLE app_migrations
    ADD CONSTRAINT app_migrations_target_server_id_fkey
    FOREIGN KEY (target_server_id) REFERENCES servers(id) ON DELETE CASCADE;
