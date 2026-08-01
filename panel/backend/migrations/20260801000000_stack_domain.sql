-- A Compose stack can be put behind a domain with a certificate (GitHub #50).
--
-- Until v2.54.0 there was no vhost, domain or certificate path for a stack
-- anywhere in the tree: the compose deploy request carried no domain field, the
-- stack table had no column to remember one, and the only way to reach a
-- deployed stack was 127.0.0.1:{published_port} on the host itself. Storing the
-- domain here is also what lets `services::domain_claim` see a stack as a
-- holder, so one door still owns a domain.
ALTER TABLE docker_stacks ADD COLUMN IF NOT EXISTS domain VARCHAR(255);
ALTER TABLE docker_stacks ADD COLUMN IF NOT EXISTS ssl_email VARCHAR(255);

-- Matches the lookup in domain_claim::find_occupant, which compares lower(domain).
CREATE INDEX IF NOT EXISTS idx_docker_stacks_domain ON docker_stacks (lower(domain));
