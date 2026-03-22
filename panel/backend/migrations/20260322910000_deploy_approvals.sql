-- Deploy approval workflow for protected deploys
CREATE TABLE IF NOT EXISTS deploy_approvals (
    id UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    deploy_id UUID NOT NULL REFERENCES git_deploys(id) ON DELETE CASCADE,
    requested_by UUID NOT NULL REFERENCES users(id),
    status TEXT NOT NULL DEFAULT 'pending',
    approved_by UUID REFERENCES users(id),
    created_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    resolved_at TIMESTAMPTZ
);

CREATE INDEX IF NOT EXISTS idx_deploy_approvals_deploy_id ON deploy_approvals(deploy_id);
CREATE INDEX IF NOT EXISTS idx_deploy_approvals_status ON deploy_approvals(status);
