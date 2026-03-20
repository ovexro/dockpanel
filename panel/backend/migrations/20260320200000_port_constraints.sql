-- Add unique constraint on preview host_port to prevent concurrent allocation race
CREATE UNIQUE INDEX IF NOT EXISTS idx_git_previews_host_port ON git_previews(host_port);

-- Add unique constraint on site proxy_port (partial - only non-NULL values)
CREATE UNIQUE INDEX IF NOT EXISTS idx_sites_proxy_port ON sites(proxy_port) WHERE proxy_port IS NOT NULL;
