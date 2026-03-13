CREATE EXTENSION IF NOT EXISTS "pgcrypto";

CREATE TABLE users (
    id UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    email VARCHAR(255) NOT NULL UNIQUE,
    password_hash TEXT NOT NULL,
    role VARCHAR(20) NOT NULL DEFAULT 'admin',
    created_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT NOW()
);

CREATE TABLE sites (
    id UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    user_id UUID NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    domain VARCHAR(255) NOT NULL UNIQUE,
    runtime VARCHAR(20) NOT NULL DEFAULT 'static',
    status VARCHAR(20) NOT NULL DEFAULT 'creating',
    proxy_port INTEGER,
    php_version VARCHAR(10),
    root_path TEXT,
    ssl_enabled BOOLEAN NOT NULL DEFAULT FALSE,
    ssl_cert_path TEXT,
    ssl_key_path TEXT,
    created_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT NOW()
);

CREATE TABLE databases (
    id UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    site_id UUID NOT NULL REFERENCES sites(id) ON DELETE CASCADE,
    name VARCHAR(255) NOT NULL UNIQUE,
    engine VARCHAR(20) NOT NULL DEFAULT 'postgres',
    db_user VARCHAR(255) NOT NULL,
    db_password_enc TEXT NOT NULL,
    container_id VARCHAR(255),
    port INTEGER,
    created_at TIMESTAMPTZ NOT NULL DEFAULT NOW()
);

CREATE INDEX idx_sites_user_id ON sites(user_id);
CREATE INDEX idx_sites_domain ON sites(domain);
CREATE INDEX idx_databases_site_id ON databases(site_id);
