-- Tier 2 ACME cert tracking columns on each server row.
-- agent_acme_cert_domain: the domain used for the ACME order (must resolve via HTTP-01).
-- agent_acme_cert_expiry: expiry of the current ACME-issued agent TLS cert.
--   Renewal is triggered when expiry < NOW() + 3 days (50% of the 6-day shortlived lifetime).
ALTER TABLE servers
    ADD COLUMN IF NOT EXISTS agent_acme_cert_domain TEXT,
    ADD COLUMN IF NOT EXISTS agent_acme_cert_expiry  TIMESTAMPTZ;

-- Running counters for ACME cert renewal failures.
-- The Prom exporter renders these as dockpanel_cert_renewal_failures_total{cert_kind,reason}.
-- Using a DB table so counts survive backend restarts (unlike in-memory atomics).
CREATE TABLE IF NOT EXISTS cert_renewal_failures (
    cert_kind TEXT    NOT NULL,
    reason    TEXT    NOT NULL,
    count     BIGINT  NOT NULL DEFAULT 0,
    PRIMARY KEY (cert_kind, reason)
);

-- Pre-populate every (cert_kind × reason) combination so the metric is
-- always present in Prometheus scrapes even before any failure occurs.
-- ON CONFLICT DO NOTHING preserves counts on re-runs (e.g. migration re-apply).
INSERT INTO cert_renewal_failures (cert_kind, reason, count)
SELECT c.kind, r.reason, 0
FROM   (VALUES ('agent_pinned'), ('panel_public'), ('site')) AS c(kind)
CROSS JOIN
       (VALUES ('acme_error'), ('dns_check'), ('rate_limit'), ('network'), ('other')) AS r(reason)
ON CONFLICT DO NOTHING;
