-- Give the suspend role-stash a column of its own.
--
-- `users.reset_token` was two unrelated things. `toggle_suspend` stashed the
-- account's role there before writing role='suspended'; `forgot_password`
-- overwrites the same column with a SHA-256 reset-token hash and
-- `reset_password` sets it to NULL. Neither of those checked the role, so a
-- suspended account could destroy its own stash from the public "forgot
-- password" form, unauthenticated, without even completing the reset.
--
-- Both outcomes converged on the same place. Reset COMPLETED leaves NULL and the
-- restore falls through `unwrap_or("user")`; reset merely REQUESTED leaves a
-- 64-char hex digest, which is not an assignable role, so the guard beside it
-- also falls back to "user". Either way a suspended `client` came back a `user`
-- and could bring a new domain into service — the one thing the role denies
-- (services::domain_claim::may_claim_new). A suspended `admin` or `reseller`
-- came back a plain `user` the same way, and that path is far older than the
-- `client` role, so it is where any damage already in the field will be.
--
-- The backfill is guarded rather than best-effort, for two independent reasons.
--
--   1. Correctness. A stash is one of four short literals; a reset token is
--      always 64 hex characters. Copying only exact role values means a token
--      can never be mistaken for a role. Where the truth was already destroyed
--      the column stays NULL, and the restore path treats NULL as "unknown" and
--      fails toward the SMALLEST capability. Guessing "user" here is precisely
--      the defect this migration exists to remove, so it must not be the
--      migration's own default.
--
--   2. It would not boot otherwise. `role` is varchar(20) and `reset_token` is
--      varchar(255); an unguarded copy hits "value too long for type character
--      varying(20)" on any install holding a live or stale reset hash — and
--      `sqlx::migrate!` runs at startup, so a failed migration is a panel that
--      does not come up. Verified against a real box carrying a 64-char digest.
--
-- Idempotent: ADD COLUMN IF NOT EXISTS, and the UPDATE matches nothing on a
-- second run because it clears the source as it goes.
ALTER TABLE users ADD COLUMN IF NOT EXISTS prior_role VARCHAR(20);

UPDATE users
   SET prior_role    = reset_token,
       reset_token   = NULL,
       reset_expires = NULL
 WHERE role = 'suspended'
   AND reset_token IN ('admin', 'reseller', 'user', 'client');

-- Then the population the copy above cannot reach, and it is the larger one on
-- any install with billing wired up: accounts suspended by the WHMCS webhook,
-- which never recorded a previous role AT ALL. Nothing was destroyed for these
-- rows; nothing was ever written, so there is no stash for the whitelist to find
-- and every one of them would land on "unknown" and refuse to un-suspend.
--
-- `user` is not a guess for them, it is what they already had. That path creates
-- accounts with the role hardcoded to 'user', and its suspend arm has always
-- refused to touch an `admin` or a `reseller`; and the restore it used before
-- this migration was a hardcoded `role = 'user'`. So this writes down the value
-- those rows were already going to be given, and grants nothing that was not
-- already being granted.
--
-- ⚠ The one case it can get wrong, stated rather than hidden: an existing
-- `client` adopted by billing (the provision hook matches an EXISTING account by
-- email, with no role filter) and then suspended through billing between the
-- release that introduced the `client` role and this upgrade would be recorded
-- as `user`. That window is about a day wide, and the pre-upgrade behaviour for
-- exactly those rows was already `user`, so this is not a new escalation — but
-- it is the reason the panel's own suspensions are NOT backfilled this way.
UPDATE users u
   SET prior_role = 'user'
  FROM whmcs_service_map m
 WHERE m.user_id = u.id
   AND u.role = 'suspended'
   AND u.prior_role IS NULL;
