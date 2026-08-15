-- One pending approval per deployment.
--
-- `deploy()` filed a protected request with a bare INSERT — no dedupe, no
-- existence check. That would have been harmless if the requester could see
-- what happened, but the 202 it returns carries no `deploy_id`, and the client
-- read only that field: the button spun, no message was shown, and the honest
-- response to a screen that looks stuck is to press it again. Every press wrote
-- another row, each row is separately approvable, and each approval spawns its
-- own deploy — so N presses meant N production deploys of the same commit,
-- authored by one operator's confusion rather than by anyone's decision.
--
-- Dedupe keeps the OLDEST pending request per deployment, and `requested_by` is
-- necessarily the same account on every duplicate (`deploy()` is the only writer
-- of these rows and pins the config to `user_id = claims.sub`), so collapsing
-- them cannot change who asked or what an approver would be signing off on.
--
-- The index is PARTIAL because the constraint belongs to the pending state only:
-- once a request has been approved or rejected the deployment must be requestable
-- again, and the resolved rows are the audit trail — `approve_deploy` and
-- `reject_deploy` both stamp `approved_by` and `resolved_at` rather than deleting.

DELETE FROM deploy_approvals a
USING deploy_approvals b
WHERE a.status = 'pending'
  AND b.status = 'pending'
  AND a.deploy_id = b.deploy_id
  AND (b.created_at, b.id) < (a.created_at, a.id);

CREATE UNIQUE INDEX IF NOT EXISTS idx_deploy_approvals_one_pending
    ON deploy_approvals (deploy_id) WHERE status = 'pending';

-- `approved_by` was NO ACTION, and that was survivable only while nothing could
-- write it: approve and reject were routed but reachable from no screen, so the
-- column was NULL on every install in the field and the constraint never had a
-- row to refuse. Now that an administrator can resolve a request, retiring that
-- administrator later would raise a foreign-key violation inside the account
-- deletion's transaction — the approval hangs off SOMEBODY ELSE's deployment, so
-- no cascade reaches it — and `remove_user` would answer 500 and roll back with
-- no UI anywhere able to clear the row. `routes/users.rs` already names this
-- class of FK in a comment; this is the one that just became armed.
--
-- SET NULL rather than CASCADE: the decision is the audit trail. Losing the name
-- of the approver is a smaller loss than losing the record that an approval
-- happened at all, and `resolved_at` still says when.
ALTER TABLE deploy_approvals
    DROP CONSTRAINT IF EXISTS deploy_approvals_approved_by_fkey;
ALTER TABLE deploy_approvals
    ADD CONSTRAINT deploy_approvals_approved_by_fkey
    FOREIGN KEY (approved_by) REFERENCES users(id) ON DELETE SET NULL;
