-- Per-site resource limits
ALTER TABLE sites ADD COLUMN rate_limit INT;          -- requests per second per IP (NULL = unlimited)
ALTER TABLE sites ADD COLUMN max_upload_mb INT NOT NULL DEFAULT 64;  -- client_max_body_size
ALTER TABLE sites ADD COLUMN php_memory_mb INT NOT NULL DEFAULT 256; -- PHP memory_limit per request
ALTER TABLE sites ADD COLUMN php_max_workers INT NOT NULL DEFAULT 5; -- PHP-FPM pm.max_children
