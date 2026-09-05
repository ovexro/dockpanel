-- Docker Apps' auto-provisioned SSL certificates had no owner row anywhere in
-- the database, so neither existing renewal door — the `sites` lookup nor the
-- Compose-stack fallback, both inside `security_scanner::auto_fix_safe_findings`
-- — could ever find one. A domain is deliberately not a database row for a
-- Docker app otherwise (`services::domain_claim`'s own comment: "Docker apps
-- are not rows" — the agent's `dockpanel.app.domain` container label is the
-- only source of truth for WHICH domain an app serves), but SSL renewal needs
-- a home for exactly the three facts a renewal decision needs: who owns it
-- (the ACME contact and the alert recipient), which mode it was deployed in
-- (never overwrite a `provided`/operator-uploaded certificate — the same
-- mistake the stack fallback made before v2.161.0), and which server to ask.
--
-- Deliberately NOT a general Docker-app registry — it exists only to close the
-- renewal gap and carries only what a renewal decision needs. Written on
-- deploy (mirroring the `monitors` auto-create block right beside it in
-- `routes::docker_apps::deploy`) and deleted on removal, using the SAME
-- `domain_removed` field the existing DNS-cleanup block in `remove_app` already
-- reads. A domain never changes after deploy for a Docker app
-- (`domain_claim::Holder` has no `App` variant for exactly this reason — a
-- rename is a delete+redeploy under a new name), so there is no update-domain
-- path to wire.
CREATE TABLE IF NOT EXISTS docker_app_domains (
    id            UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    user_id       UUID NOT NULL REFERENCES users(id)   ON DELETE CASCADE,
    server_id     UUID NOT NULL REFERENCES servers(id) ON DELETE CASCADE,
    app_name      VARCHAR(255) NOT NULL,
    domain        VARCHAR(255) NOT NULL,
    ssl_email     VARCHAR(255),
    tls_mode      TEXT CHECK (tls_mode IN ('none', 'acme', 'provided')),
    created_at    TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    updated_at    TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    UNIQUE (server_id, domain)
);

-- Matches the lookup shape `domain_claim::find_occupant` and the sibling
-- `idx_docker_stacks_domain`/`idx_sites_domain_server` indexes use.
CREATE INDEX IF NOT EXISTS idx_docker_app_domains_domain ON docker_app_domains (lower(domain));
