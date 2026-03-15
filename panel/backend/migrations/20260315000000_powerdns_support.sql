-- Make CF-specific fields nullable for non-Cloudflare providers (e.g. PowerDNS)
ALTER TABLE dns_zones ALTER COLUMN cf_zone_id DROP NOT NULL;
ALTER TABLE dns_zones ALTER COLUMN cf_api_token DROP NOT NULL;

-- PowerDNS settings will be stored in the existing 'settings' table:
-- pdns_api_url  (e.g. http://127.0.0.1:8081)
-- pdns_api_key  (API key for PowerDNS HTTP API)
