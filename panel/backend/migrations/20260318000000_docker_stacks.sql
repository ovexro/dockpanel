-- Docker Compose stacks — groups of containers deployed from a single YAML file
CREATE TABLE IF NOT EXISTS docker_stacks (
    id UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    user_id UUID NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    name VARCHAR(255) NOT NULL,
    yaml TEXT NOT NULL,
    service_count INT NOT NULL DEFAULT 0,
    created_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT NOW()
);

CREATE INDEX IF NOT EXISTS idx_docker_stacks_user_id ON docker_stacks(user_id);
