-- Phase 2: Incident Management + Enhanced Status Page

-- Status page configuration
CREATE TABLE IF NOT EXISTS status_page_config (
    id UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    user_id UUID NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    title VARCHAR(200) NOT NULL DEFAULT 'Service Status',
    description TEXT NOT NULL DEFAULT 'Current status of our services',
    logo_url VARCHAR(500),
    custom_css TEXT,
    timezone VARCHAR(50) NOT NULL DEFAULT 'UTC',
    -- Branding
    accent_color VARCHAR(20) NOT NULL DEFAULT '#22c55e',
    show_subscribe BOOLEAN NOT NULL DEFAULT TRUE,
    show_incident_history BOOLEAN NOT NULL DEFAULT TRUE,
    history_days INTEGER NOT NULL DEFAULT 90,
    enabled BOOLEAN NOT NULL DEFAULT TRUE,
    created_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT NOW()
);
CREATE INDEX IF NOT EXISTS idx_status_page_config_user ON status_page_config(user_id);

-- Status page components (group monitors into logical services)
CREATE TABLE IF NOT EXISTS status_page_components (
    id UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    user_id UUID NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    name VARCHAR(100) NOT NULL,
    description VARCHAR(500),
    sort_order INTEGER NOT NULL DEFAULT 0,
    -- Status override (null = auto from monitors)
    status_override VARCHAR(30),
    -- Group name for component grouping
    group_name VARCHAR(100),
    created_at TIMESTAMPTZ NOT NULL DEFAULT NOW()
);
CREATE INDEX IF NOT EXISTS idx_status_page_components_user ON status_page_components(user_id);

-- Link components to monitors (many-to-many)
CREATE TABLE IF NOT EXISTS status_page_component_monitors (
    component_id UUID NOT NULL REFERENCES status_page_components(id) ON DELETE CASCADE,
    monitor_id UUID NOT NULL REFERENCES monitors(id) ON DELETE CASCADE,
    PRIMARY KEY (component_id, monitor_id)
);

-- Enhanced incidents (managed incidents with lifecycle)
-- Adds to existing incidents table which only tracks auto-detected downtime
CREATE TABLE IF NOT EXISTS managed_incidents (
    id UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    user_id UUID NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    title VARCHAR(200) NOT NULL,
    -- Status: investigating, identified, monitoring, resolved, postmortem
    status VARCHAR(30) NOT NULL DEFAULT 'investigating',
    -- Severity: minor, major, critical, maintenance
    severity VARCHAR(20) NOT NULL DEFAULT 'major',
    -- Impact description
    description TEXT,
    -- Affected components
    started_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    resolved_at TIMESTAMPTZ,
    -- Postmortem
    postmortem TEXT,
    postmortem_published BOOLEAN NOT NULL DEFAULT FALSE,
    -- Visibility
    visible_on_status_page BOOLEAN NOT NULL DEFAULT TRUE,
    created_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT NOW()
);
CREATE INDEX IF NOT EXISTS idx_managed_incidents_user ON managed_incidents(user_id);
CREATE INDEX IF NOT EXISTS idx_managed_incidents_status ON managed_incidents(status);
CREATE INDEX IF NOT EXISTS idx_managed_incidents_started ON managed_incidents(started_at DESC);

-- Link incidents to affected components
CREATE TABLE IF NOT EXISTS managed_incident_components (
    incident_id UUID NOT NULL REFERENCES managed_incidents(id) ON DELETE CASCADE,
    component_id UUID NOT NULL REFERENCES status_page_components(id) ON DELETE CASCADE,
    PRIMARY KEY (incident_id, component_id)
);

-- Incident updates (timeline entries)
CREATE TABLE IF NOT EXISTS incident_updates (
    id UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    incident_id UUID NOT NULL REFERENCES managed_incidents(id) ON DELETE CASCADE,
    -- Status at time of update
    status VARCHAR(30) NOT NULL,
    message TEXT NOT NULL,
    -- Who posted (for audit)
    author_email VARCHAR(255),
    created_at TIMESTAMPTZ NOT NULL DEFAULT NOW()
);
CREATE INDEX IF NOT EXISTS idx_incident_updates_incident ON incident_updates(incident_id, created_at ASC);

-- Status page subscribers (email notifications for incidents)
CREATE TABLE IF NOT EXISTS status_page_subscribers (
    id UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    email VARCHAR(255) NOT NULL,
    -- Verification
    verified BOOLEAN NOT NULL DEFAULT FALSE,
    verify_token VARCHAR(64),
    -- Preferences
    notify_incidents BOOLEAN NOT NULL DEFAULT TRUE,
    notify_maintenance BOOLEAN NOT NULL DEFAULT TRUE,
    created_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    UNIQUE(email)
);
CREATE INDEX IF NOT EXISTS idx_status_page_subscribers_verified ON status_page_subscribers(verified) WHERE verified = TRUE;
