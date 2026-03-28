-- Terraform/Pulumi IaC provider tokens (separate from API keys for IaC-specific scoping)
CREATE TABLE iac_tokens (
    id UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    user_id UUID NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    name VARCHAR(255) NOT NULL,
    token_hash VARCHAR(255) NOT NULL UNIQUE,
    scopes TEXT NOT NULL DEFAULT 'sites,databases,dns',
    last_used_at TIMESTAMPTZ,
    created_at TIMESTAMPTZ NOT NULL DEFAULT NOW()
);
CREATE INDEX idx_iac_tokens_user_id ON iac_tokens(user_id);
CREATE INDEX idx_iac_tokens_hash ON iac_tokens(token_hash);

-- Horizontal auto-scaling rules
CREATE TABLE autoscale_rules (
    id UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    container_id VARCHAR(255) NOT NULL,
    container_name VARCHAR(255) NOT NULL,
    enabled BOOLEAN NOT NULL DEFAULT true,
    min_replicas INT NOT NULL DEFAULT 1,
    max_replicas INT NOT NULL DEFAULT 5,
    cpu_threshold_up INT NOT NULL DEFAULT 80,
    cpu_threshold_down INT NOT NULL DEFAULT 20,
    cooldown_seconds INT NOT NULL DEFAULT 300,
    current_replicas INT NOT NULL DEFAULT 1,
    last_scale_at TIMESTAMPTZ,
    created_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT NOW()
);
CREATE INDEX idx_autoscale_rules_container_id ON autoscale_rules(container_id);
