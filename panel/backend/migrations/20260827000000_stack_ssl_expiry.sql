-- Give a Compose stack's row somewhere to record when its certificate expires.
--
-- Until v2.162.0 the panel could not answer "when does this stack's certificate
-- expire?" from its own data. v2.161.0 taught the weekly scanner to renew such a
-- certificate, but the renewal's answer was thrown away: the only record of the
-- expiry lived on the agent's filesystem, so no panel-side reader — a list, a
-- countdown, an alert — could ever include a stack.
--
-- ── Why nullable, with no default and no backfill ───────────────────────────
--
-- NULL means "not recorded", which is a third state distinct from "expired" and
-- from "valid", and it is the only honest value for a row the panel has not yet
-- heard about. A NOT NULL DEFAULT NOW() would have every pre-existing stack
-- assert a certificate expiring today, which is a claim no code ever made.
--
-- A backfill could only guess. The authoritative source is the agent's walk of
-- its own disk, which a migration cannot reach; the panel bootstraps the value
-- the first time it reads a host's certificates, and rewrites it on every
-- renewal. This mirrors the reasoning recorded in
-- 20260826000000_tls_certificate_registry.sql for the columns it added, and the
-- older precedent in 20260311100000_add_ssl_expiry.sql.

ALTER TABLE docker_stacks ADD COLUMN IF NOT EXISTS ssl_expiry TIMESTAMPTZ;

-- Partial, matching the idiom of idx_docker_stacks_tls_certificate: most rows
-- carry no value, and every reader asks for the ones that do.
CREATE INDEX IF NOT EXISTS idx_docker_stacks_ssl_expiry
    ON docker_stacks (ssl_expiry) WHERE ssl_expiry IS NOT NULL;
