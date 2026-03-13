-- Remove billing enforcement: set unlimited servers for all users
ALTER TABLE users ALTER COLUMN plan_server_limit SET DEFAULT 9999;
UPDATE users SET plan_server_limit = 9999 WHERE plan_server_limit < 9999;
