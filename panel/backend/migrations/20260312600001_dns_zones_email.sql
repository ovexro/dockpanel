-- Support Global API Key auth (email + key) in addition to API Token
ALTER TABLE dns_zones ADD COLUMN cf_api_email VARCHAR(255);
