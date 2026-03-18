-- Support for Node.js/Python app runtimes with managed processes
ALTER TABLE sites ADD COLUMN IF NOT EXISTS app_command TEXT;

-- Update runtime CHECK constraint to include node and python
ALTER TABLE sites DROP CONSTRAINT IF EXISTS chk_sites_runtime;
ALTER TABLE sites ADD CONSTRAINT chk_sites_runtime CHECK (runtime IN ('static', 'php', 'proxy', 'node', 'python'));
