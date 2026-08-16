-- Two owner links to `users` still had no delete action, and one of them made a
-- supported administrative action permanently impossible.
--
-- `maintenance_windows` is the live one. Its owner column has carried NO ACTION
-- since 20260318900000, and unlike every other table hanging off an account there
-- is no second edge to rescue it: `routes/monitors.rs` is the only module that
-- touches the table, its list and delete handlers are both owner-scoped and take
-- `AuthUser`, and nothing anywhere purges it. So an administrator cannot see or
-- clear another account's windows, and `routes/users.rs::remove` deletes the
-- account without clearing them first. The delete raises inside the transaction,
-- `internal_error` converts it to `Operation failed. Reference: <uuid>`, and the
-- account can never be deleted through the panel again. The trigger is the most
-- ordinary reason anyone opens the Users screen — retiring a departing client or
-- member of staff — and the row is created by the lowest-privilege role, on the
-- product's own advice (`docs/guides/monitoring.md` tells operators to schedule a
-- window before planned downtime, and `docs/guides/roles-and-ownership.md` says a
-- client may do it).
--
-- The project already knew. `routes/users.rs` names maintenance windows and deploy
-- approvals as this exact hazard, and the migration immediately before this one
-- repaired the deploy-approval half, quoted that comment while doing it, and left
-- the sibling the comment names.
--
-- CASCADE, not SET NULL: both columns are NOT NULL, so SET NULL is not available;
-- and a maintenance window exists only to suppress its owner's alerts, so without
-- the owner it has nothing to say. There is no audit value to preserve here — the
-- argument that kept `approved_by` as SET NULL does not transfer.
-- The orphan sweeps below are not decoration. `sqlx::migrate!` is unwrapped at
-- startup, so a migration that can raise is a panel that will not boot — and
-- `ADD CONSTRAINT` validates every existing row. The old constraint made orphans
-- impossible while it stood, but this defect is precisely the one an operator
-- would have worked around by hand in psql, and a hand-written cleanup that
-- dropped the constraint first is exactly the state that would brick the upgrade.
-- Sweeping first costs nothing on a healthy install and cannot be skipped safely.
DELETE FROM maintenance_windows w
 WHERE NOT EXISTS (SELECT 1 FROM users u WHERE u.id = w.user_id);

ALTER TABLE maintenance_windows
    DROP CONSTRAINT IF EXISTS maintenance_windows_user_id_fkey;
ALTER TABLE maintenance_windows
    ADD CONSTRAINT maintenance_windows_user_id_fkey
    FOREIGN KEY (user_id) REFERENCES users(id) ON DELETE CASCADE;

-- `deploy_approvals.requested_by` is the second, and today it is protected by an
-- INVARIANT rather than by the schema: `deploy()` is the only writer and fetches
-- its config owner-scoped, so the requester is always the deployment's owner, and
-- the row therefore dies with the deployment through `deploy_id`'s cascade before
-- this constraint is ever consulted. `routes/git_deploys.rs` states that invariant
-- in a comment and relies on it for an unrelated authorisation decision.
--
-- That is a fine reason for the code to be correct and a poor reason for the
-- schema to stay silent: the day a second writer appears, or the day the requester
-- may be someone other than the owner, this becomes the maintenance-windows defect
-- again, in a table whose repair migration is already written above. Making the
-- schema say what the invariant says costs nothing today — the cascade removes
-- exactly the rows `deploy_id` would already have removed — and removes the
-- dependency on a comment staying true.
DELETE FROM deploy_approvals a
 WHERE NOT EXISTS (SELECT 1 FROM users u WHERE u.id = a.requested_by);

ALTER TABLE deploy_approvals
    DROP CONSTRAINT IF EXISTS deploy_approvals_requested_by_fkey;
ALTER TABLE deploy_approvals
    ADD CONSTRAINT deploy_approvals_requested_by_fkey
    FOREIGN KEY (requested_by) REFERENCES users(id) ON DELETE CASCADE;
