-- Extra nginx directives and an upload limit for a Git Deploy's vhost (#132).
-- NULL = none / the panel default, so existing deploys are untouched.
ALTER TABLE git_deploys ADD COLUMN IF NOT EXISTS custom_nginx TEXT;
ALTER TABLE git_deploys ADD COLUMN IF NOT EXISTS max_upload_mb INT;
