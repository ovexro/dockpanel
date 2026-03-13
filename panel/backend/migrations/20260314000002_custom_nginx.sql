-- Add custom_nginx column for user-defined nginx directives per site
ALTER TABLE sites ADD COLUMN IF NOT EXISTS custom_nginx TEXT;
