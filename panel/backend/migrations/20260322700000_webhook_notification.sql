-- Gap 31: Generic webhook notification channel
ALTER TABLE alert_rules ADD COLUMN IF NOT EXISTS notify_webhook_url TEXT;
