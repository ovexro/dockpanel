-- Mail domains managed by DockPanel
CREATE TABLE IF NOT EXISTS mail_domains (
    id UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    domain VARCHAR(255) NOT NULL UNIQUE,
    dkim_selector VARCHAR(63) NOT NULL DEFAULT 'dockpanel',
    dkim_private_key TEXT,
    dkim_public_key TEXT,
    catch_all VARCHAR(255),
    enabled BOOLEAN NOT NULL DEFAULT true,
    created_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT NOW()
);

-- Mail accounts (virtual mailboxes)
CREATE TABLE IF NOT EXISTS mail_accounts (
    id UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    domain_id UUID NOT NULL REFERENCES mail_domains(id) ON DELETE CASCADE,
    email VARCHAR(255) NOT NULL UNIQUE,
    password_hash VARCHAR(255) NOT NULL,
    display_name VARCHAR(255),
    quota_mb INTEGER NOT NULL DEFAULT 1024,
    enabled BOOLEAN NOT NULL DEFAULT true,
    forward_to VARCHAR(255),
    autoresponder_enabled BOOLEAN NOT NULL DEFAULT false,
    autoresponder_subject VARCHAR(255),
    autoresponder_body TEXT,
    created_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT NOW()
);

-- Mail aliases (forwarding rules)
CREATE TABLE IF NOT EXISTS mail_aliases (
    id UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    domain_id UUID NOT NULL REFERENCES mail_domains(id) ON DELETE CASCADE,
    source_email VARCHAR(255) NOT NULL,
    destination_email VARCHAR(255) NOT NULL,
    created_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    UNIQUE(source_email, destination_email)
);

CREATE INDEX idx_mail_accounts_domain ON mail_accounts(domain_id);
CREATE INDEX idx_mail_aliases_domain ON mail_aliases(domain_id);
