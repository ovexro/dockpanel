-- CDN integration (BunnyCDN + Cloudflare CDN)
CREATE TABLE cdn_zones (
    id UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    user_id UUID NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    domain VARCHAR(255) NOT NULL,
    provider VARCHAR(50) NOT NULL DEFAULT 'bunnycdn',
    pull_zone_id VARCHAR(255),
    api_key TEXT NOT NULL,
    origin_url TEXT,
    cdn_hostname TEXT,
    enabled BOOLEAN NOT NULL DEFAULT TRUE,
    cache_ttl INTEGER NOT NULL DEFAULT 86400,
    created_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    UNIQUE(domain, provider)
);
