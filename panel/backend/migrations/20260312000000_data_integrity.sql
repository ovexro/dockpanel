-- Data integrity improvements

-- 1. Change database name uniqueness from global to per-site
ALTER TABLE databases DROP CONSTRAINT IF EXISTS databases_name_key;
ALTER TABLE databases ADD CONSTRAINT databases_site_name_unique UNIQUE(site_id, name);

-- 2. Add port uniqueness (only non-NULL ports)
CREATE UNIQUE INDEX IF NOT EXISTS idx_databases_port_unique ON databases(port) WHERE port IS NOT NULL;

-- 3. Add index on databases.container_id for agent lookups
CREATE INDEX IF NOT EXISTS idx_databases_container_id ON databases(container_id) WHERE container_id IS NOT NULL;

-- 4. Add index on activity_logs.action for filtered queries
CREATE INDEX IF NOT EXISTS idx_activity_logs_action ON activity_logs(action);

-- 5. Add proxy_port range constraint
ALTER TABLE sites ADD CONSTRAINT sites_proxy_port_range
    CHECK (proxy_port IS NULL OR (proxy_port >= 1 AND proxy_port <= 65535));

-- 6. Make activity_logs.user_id nullable and add FK (ON DELETE SET NULL)
ALTER TABLE activity_logs ALTER COLUMN user_id DROP NOT NULL;
ALTER TABLE activity_logs
    ADD CONSTRAINT fk_activity_logs_user
    FOREIGN KEY (user_id) REFERENCES users(id) ON DELETE SET NULL;
