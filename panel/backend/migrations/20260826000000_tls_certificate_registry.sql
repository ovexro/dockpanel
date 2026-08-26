-- A certificate registered once, by name, and referenced when a stack claims a
-- domain (GitHub #104) — plus the TLS mode a stack was created with, stored.
--
-- Until now a Compose stack could only be served over HTTPS by letting the
-- panel order a Let's Encrypt certificate, and the ONLY signal for "this stack
-- wants TLS at all" was whether `ssl_email` happened to be present. Two things
-- were wrong with that, and the second one is an outage:
--
--   * An operator who already holds a certificate — a wildcard from their CA, a
--     corporate PKI leaf, an EV certificate — had nowhere to put it. The single
--     -site upload writes into `/etc/dockpanel/ssl/{domain}/`, which is the
--     directory the stack teardown DELETES and every scheduled renewer treats
--     as its own, so it cannot be shared by several stacks or survive one of
--     them being removed.
--
--   * `update` forwarded whatever `ssl_email` arrived in the request and never
--     fell back to the stored one, so an edit that omitted it rewrote the vhost
--     WITHOUT its `:443` block — behind the one-year HSTS header the HTTPS
--     template had already sent to every browser. The mode was never a fact
--     the row could state; it was an inference from a field the client may or
--     may not repeat.
--
-- `tls_certificates` holds METADATA ONLY. The PEM pair lives on the agent, at
-- `/etc/dockpanel/ssl-registry/<alias>/`, deliberately a sibling of — never
-- inside — `/etc/dockpanel/ssl/`, because everything that walks that tree
-- treats each directory as a domain to renew or to delete. Postgres records
-- what the agent parsed out of the certificate (names, issuer, validity,
-- fingerprint) so the panel can list, warn on expiry and refuse a claim the
-- certificate does not cover, without ever holding key material.
--
-- ── Why `tls_mode` is nullable with no default ──────────────────────────────
--
-- A row written by an OLDER binary after a rollback carries NULL here, and the
-- code derives the mode for such a row exactly as the agent always has — an
-- address means Let's Encrypt, no address means plain HTTP. A NOT NULL DEFAULT
-- would turn that derivation into a stored assertion the older binary never
-- made. The backfill below is sound for the same reason it is safe: until this
-- migration the address WAS the mode, so presence is the fact, not a guess.
--
-- ── Why `ON DELETE SET NULL`, not RESTRICT ──────────────────────────────────
--
-- The house FK census carries no RESTRICT FK and treats a blocking constraint
-- as a defect: a delete that the database refuses surfaces as a 500 with a
-- reference number, which tells the operator nothing. The "still in use by
-- these stacks" refusal is a 409 in the delete handler, which can name them.
-- Should a row vanish by any other route, the stack keeps its mode and loses
-- only the pointer, and the next redeploy says so instead of silently
-- downgrading to HTTP.

CREATE TABLE IF NOT EXISTS tls_certificates (
    id            UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    user_id       UUID NOT NULL REFERENCES users(id)   ON DELETE CASCADE,
    server_id     UUID NOT NULL REFERENCES servers(id) ON DELETE CASCADE,
    alias         VARCHAR(64) NOT NULL,
    dns_names     TEXT[] NOT NULL DEFAULT '{}',
    issuer        TEXT,
    not_before    TIMESTAMPTZ,
    not_after     TIMESTAMPTZ,
    fingerprint_sha256 TEXT,
    cert_path     TEXT NOT NULL,
    key_path      TEXT NOT NULL,
    created_at    TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    updated_at    TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    UNIQUE (server_id, alias)
);
CREATE INDEX IF NOT EXISTS idx_tls_certificates_user   ON tls_certificates(user_id);
CREATE INDEX IF NOT EXISTS idx_tls_certificates_server ON tls_certificates(server_id);

ALTER TABLE docker_stacks
    ADD COLUMN IF NOT EXISTS tls_mode TEXT CHECK (tls_mode IN ('none', 'acme', 'provided')),
    ADD COLUMN IF NOT EXISTS tls_certificate_id UUID REFERENCES tls_certificates(id) ON DELETE SET NULL;
CREATE INDEX IF NOT EXISTS idx_docker_stacks_tls_certificate
    ON docker_stacks (tls_certificate_id) WHERE tls_certificate_id IS NOT NULL;

-- Sound backfill: until this migration, ssl_email was the ONLY signal, so its presence IS the mode.
UPDATE docker_stacks
   SET tls_mode = CASE WHEN ssl_email IS NOT NULL AND ssl_email <> '' THEN 'acme' ELSE 'none' END
 WHERE tls_mode IS NULL;
