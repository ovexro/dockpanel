-- CSP header management per site + Bot protection
ALTER TABLE sites ADD COLUMN csp_policy TEXT;
ALTER TABLE sites ADD COLUMN permissions_policy TEXT;
ALTER TABLE sites ADD COLUMN bot_protection VARCHAR(20) NOT NULL DEFAULT 'off';
