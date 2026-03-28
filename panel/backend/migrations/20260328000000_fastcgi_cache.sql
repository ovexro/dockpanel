-- FastCGI cache toggle per site (PHP performance feature)
ALTER TABLE sites ADD COLUMN IF NOT EXISTS fastcgi_cache BOOLEAN NOT NULL DEFAULT FALSE;
