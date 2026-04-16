-- ACME profile selection + ARI-driven renewal scheduling.
--
-- ssl_profile: which Let's Encrypt ACME profile was used at provision time
--   ("classic" / "tlsserver" / "shortlived"). NULL for pre-existing certs.
-- ssl_renewal_at: CA-suggested renewal window start (from RFC 9773 ARI).
--   When NULL, auto-healer falls back to days_remaining < cert_lifetime/3.
-- ssl_renewal_checked_at: last time we fetched ARI for this cert, so we
--   honour the CA's retry-after hint and don't hammer the endpoint.

ALTER TABLE sites
    ADD COLUMN IF NOT EXISTS ssl_profile TEXT,
    ADD COLUMN IF NOT EXISTS ssl_renewal_at TIMESTAMPTZ,
    ADD COLUMN IF NOT EXISTS ssl_renewal_checked_at TIMESTAMPTZ;

CREATE INDEX IF NOT EXISTS idx_sites_ssl_renewal_at
    ON sites (ssl_renewal_at)
    WHERE ssl_enabled = TRUE;
