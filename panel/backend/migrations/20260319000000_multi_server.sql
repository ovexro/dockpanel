-- Multi-server management: scope all resources to a server_id

-- Add is_local flag and agent_url to servers table
ALTER TABLE servers ADD COLUMN IF NOT EXISTS is_local BOOLEAN NOT NULL DEFAULT false;
ALTER TABLE servers ADD COLUMN IF NOT EXISTS agent_url VARCHAR(500);

-- Add server_id to sites
ALTER TABLE sites ADD COLUMN IF NOT EXISTS server_id UUID REFERENCES servers(id) ON DELETE CASCADE;

-- Add server_id to docker_stacks
ALTER TABLE docker_stacks ADD COLUMN IF NOT EXISTS server_id UUID REFERENCES servers(id) ON DELETE CASCADE;

-- Add server_id to git_deploys
ALTER TABLE git_deploys ADD COLUMN IF NOT EXISTS server_id UUID REFERENCES servers(id) ON DELETE CASCADE;

-- Add server_id to dns_zones
ALTER TABLE dns_zones ADD COLUMN IF NOT EXISTS server_id UUID REFERENCES servers(id) ON DELETE CASCADE;

-- Add server_id to mail_domains
ALTER TABLE mail_domains ADD COLUMN IF NOT EXISTS server_id UUID REFERENCES servers(id) ON DELETE CASCADE;

-- Add server_id to monitors (nullable — monitors can be global)
ALTER TABLE monitors ADD COLUMN IF NOT EXISTS server_id UUID REFERENCES servers(id) ON DELETE SET NULL;

-- Add server_id to backup_destinations (nullable — can be shared)
ALTER TABLE backup_destinations ADD COLUMN IF NOT EXISTS server_id UUID REFERENCES servers(id) ON DELETE SET NULL;

-- Add server_id to activity_logs (nullable — for filtering)
ALTER TABLE activity_logs ADD COLUMN IF NOT EXISTS server_id UUID REFERENCES servers(id) ON DELETE SET NULL;

-- Create indexes for server_id columns
CREATE INDEX IF NOT EXISTS idx_sites_server_id ON sites(server_id);
CREATE INDEX IF NOT EXISTS idx_docker_stacks_server_id ON docker_stacks(server_id);
CREATE INDEX IF NOT EXISTS idx_git_deploys_server_id ON git_deploys(server_id);
CREATE INDEX IF NOT EXISTS idx_dns_zones_server_id ON dns_zones(server_id);
CREATE INDEX IF NOT EXISTS idx_mail_domains_server_id ON mail_domains(server_id);
CREATE INDEX IF NOT EXISTS idx_monitors_server_id ON monitors(server_id);
CREATE INDEX IF NOT EXISTS idx_activity_logs_server_id ON activity_logs(server_id);

-- Backfill: if there are existing resources, ensure a local server exists and assign it
-- This DO block creates the local server row and backfills server_id on all existing data
DO $$
DECLARE
    local_sid UUID;
    admin_uid UUID;
BEGIN
    -- Find the first admin user
    SELECT id INTO admin_uid FROM users ORDER BY created_at ASC LIMIT 1;

    -- If there are no users yet, skip backfill (fresh install)
    IF admin_uid IS NULL THEN
        RETURN;
    END IF;

    -- Check if a local server already exists
    SELECT id INTO local_sid FROM servers WHERE is_local = true LIMIT 1;

    -- If no local server, create one
    IF local_sid IS NULL THEN
        INSERT INTO servers (id, user_id, name, agent_token, status, is_local)
        VALUES (gen_random_uuid(), admin_uid, 'This Server', 'local', 'online', true)
        RETURNING id INTO local_sid;
    END IF;

    -- Backfill server_id for existing resources
    UPDATE sites SET server_id = local_sid WHERE server_id IS NULL;
    UPDATE docker_stacks SET server_id = local_sid WHERE server_id IS NULL;
    UPDATE git_deploys SET server_id = local_sid WHERE server_id IS NULL;
    UPDATE dns_zones SET server_id = local_sid WHERE server_id IS NULL;
    UPDATE mail_domains SET server_id = local_sid WHERE server_id IS NULL;
    UPDATE monitors SET server_id = local_sid WHERE server_id IS NULL;
    UPDATE activity_logs SET server_id = local_sid WHERE server_id IS NULL;
END $$;

-- Now make server_id NOT NULL on core resource tables (after backfill)
ALTER TABLE sites ALTER COLUMN server_id SET NOT NULL;
ALTER TABLE docker_stacks ALTER COLUMN server_id SET NOT NULL;
ALTER TABLE git_deploys ALTER COLUMN server_id SET NOT NULL;

-- Update unique constraints: domain should be unique per server, not globally
-- Drop old unique constraint on sites.domain
ALTER TABLE sites DROP CONSTRAINT IF EXISTS sites_domain_key;
DROP INDEX IF EXISTS sites_domain_key;
CREATE UNIQUE INDEX IF NOT EXISTS idx_sites_domain_server ON sites(domain, server_id);

-- Same for git_deploys.name — unique per server
ALTER TABLE git_deploys DROP CONSTRAINT IF EXISTS git_deploys_name_key;
DROP INDEX IF EXISTS git_deploys_name_key;
CREATE UNIQUE INDEX IF NOT EXISTS idx_git_deploys_name_server ON git_deploys(name, server_id);

-- Same for git_deploys.host_port — unique per server
ALTER TABLE git_deploys DROP CONSTRAINT IF EXISTS git_deploys_host_port_key;
DROP INDEX IF EXISTS git_deploys_host_port_key;
CREATE UNIQUE INDEX IF NOT EXISTS idx_git_deploys_host_port_server ON git_deploys(host_port, server_id);

-- dns_zones.domain — unique per server
ALTER TABLE dns_zones DROP CONSTRAINT IF EXISTS dns_zones_domain_key;
DROP INDEX IF EXISTS dns_zones_domain_key;
CREATE UNIQUE INDEX IF NOT EXISTS idx_dns_zones_domain_server ON dns_zones(domain, server_id);

-- mail_domains.domain — unique per server
ALTER TABLE mail_domains DROP CONSTRAINT IF EXISTS mail_domains_domain_key;
DROP INDEX IF EXISTS mail_domains_domain_key;
CREATE UNIQUE INDEX IF NOT EXISTS idx_mail_domains_domain_server ON mail_domains(domain, server_id);

-- docker_stacks name — unique per server+user
ALTER TABLE docker_stacks DROP CONSTRAINT IF EXISTS docker_stacks_name_key;
DROP INDEX IF EXISTS docker_stacks_name_key;
