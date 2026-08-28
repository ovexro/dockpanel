-- Give a git deploy the same registered-certificate ("provided" TLS mode)
-- option a Docker Compose stack got in 20260826000000_tls_certificate_registry.sql.
--
-- Until now a git deploy's ONLY route to HTTPS was letting the panel order a
-- Let's Encrypt certificate, and the ONLY signal for "this deploy wants TLS at
-- all" was whether `ssl_email` happened to be present on the row — the exact
-- shape that migration closed for stacks, for the exact same reason: an
-- operator who already holds a certificate (a wildcard, a corporate PKI leaf)
-- had nowhere to put it, and an edit that omitted `ssl_email` would silently
-- rewrite the vhost without its `:443` block behind a year of HSTS. `plan_tls`
-- on the panel side does not gain a new copy of the vocabulary or FK shape
-- here — `tls_certificates` is already a cross-table registry, not a
-- `docker_stacks`-specific one, so `git_deploys` references the same table.
--
-- ── Why `tls_mode` is nullable with no default ──────────────────────────────
--
-- A row written by an OLDER binary carries NULL here, and the code derives
-- the mode for such a row exactly as it always has — an address means Let's
-- Encrypt, no address means plain HTTP. A NOT NULL DEFAULT would turn that
-- derivation into a stored assertion the older binary never made. The
-- backfill below is sound for the same reason it is safe: until this
-- migration the address WAS the mode, so presence is the fact, not a guess.
--
-- ── Why `ON DELETE SET NULL`, not RESTRICT ──────────────────────────────────
--
-- The house FK census carries no RESTRICT FK and treats a blocking constraint
-- as a defect: a delete the database refuses surfaces as a 500 with a
-- reference number, which tells the operator nothing. The "still in use"
-- refusal belongs in the delete handler as a 409 that can name the blocker.
-- Should a row vanish by any other route, the deploy keeps its mode and loses
-- only the pointer, and the next deploy says so instead of silently
-- downgrading to HTTP.

ALTER TABLE git_deploys
    ADD COLUMN IF NOT EXISTS tls_mode TEXT CHECK (tls_mode IN ('none', 'acme', 'provided')),
    ADD COLUMN IF NOT EXISTS tls_certificate_id UUID REFERENCES tls_certificates(id) ON DELETE SET NULL;
CREATE INDEX IF NOT EXISTS idx_git_deploys_tls_certificate
    ON git_deploys (tls_certificate_id) WHERE tls_certificate_id IS NOT NULL;

-- Sound backfill: until this migration, ssl_email was the ONLY signal, so its presence IS the mode.
UPDATE git_deploys
   SET tls_mode = CASE WHEN ssl_email IS NOT NULL AND ssl_email <> '' THEN 'acme' ELSE 'none' END
 WHERE tls_mode IS NULL;
