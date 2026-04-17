-- Agent TLS cert pinning (Phase 3 #3 Tier 2).
-- Captures the SHA-256 (hex) fingerprint of the agent's self-signed cert
-- on first checkin (TOFU). Subsequent checkins must present a matching
-- fingerprint. Rotating an agent's cert requires clearing this column
-- via the admin rotate endpoint.

ALTER TABLE servers
    ADD COLUMN IF NOT EXISTS cert_fingerprint VARCHAR(64);

CREATE INDEX IF NOT EXISTS idx_servers_cert_fingerprint
    ON servers(cert_fingerprint)
    WHERE cert_fingerprint IS NOT NULL;
