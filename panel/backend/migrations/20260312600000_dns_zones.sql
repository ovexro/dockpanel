-- DNS zone management (Cloudflare API proxy)
CREATE TABLE dns_zones (
    id UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    user_id UUID NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    domain VARCHAR(255) NOT NULL,
    provider VARCHAR(50) NOT NULL DEFAULT 'cloudflare',
    cf_zone_id VARCHAR(64) NOT NULL,
    cf_api_token TEXT NOT NULL,
    created_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    UNIQUE(domain)
);
