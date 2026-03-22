-- Gap #69: Per-event notification channel routing — muted alert types
ALTER TABLE alert_rules ADD COLUMN IF NOT EXISTS muted_types TEXT NOT NULL DEFAULT '';

-- Gap #70: Customizable notification templates (stored in settings table)
-- Templates use keys like notif_template_slack, notif_template_email, notif_template_discord
-- with placeholders: {{title}}, {{message}}, {{severity}}, {{timestamp}}
-- No schema changes needed — uses existing settings table
