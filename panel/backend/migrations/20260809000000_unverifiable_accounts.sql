-- Release the accounts that could never verify their own email address (#100).
--
-- `auth::setup` — the bootstrap door that creates the first administrator —
-- inserted only (email, password_hash, role), so the row landed with
-- `email_verified = FALSE` (the column's NOT NULL DEFAULT) and, crucially,
-- `email_token = NULL`.
--
-- `login` refuses an unverified account whenever the `smtp_host` setting is
-- merely non-empty. So on every install created since the saas_auth migration,
-- the first administrator was one SMTP configuration away from being locked out
-- of their own panel — and both exits required the mail that was being set up:
--   * the token door matches `WHERE email_token = $1`, and these rows have no
--     token, so it cannot be reached at all — not "the link did not arrive",
--     but "no link can exist";
--   * `reset_password` needs the forgot-password message.
--
-- The predicate below is exactly "cannot ever verify": unverified AND holding no
-- token. It is not "unverified", which would wrongly clear accounts that have a
-- live verification link outstanding. Every other writer of this column already
-- inserts TRUE (oauth.rs, whmcs.rs) or is the token/reset path that sets it and
-- nulls the token together, so no legitimate pending registration is in scope.
--
-- Verifying these rows restores the status quo the operator already had: they
-- could sign in before SMTP was configured, and nothing about configuring mail
-- should revoke access for the person who owns the machine.
UPDATE users
   SET email_verified = TRUE,
       updated_at     = NOW()
 WHERE email_verified = FALSE
   AND email_token IS NULL;
