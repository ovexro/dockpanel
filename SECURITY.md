# Security Policy

## Reporting Vulnerabilities

If you discover a security vulnerability in DockPanel, please report it responsibly:

**Email:** security@dockpanel.dev

Please include:
- Description of the vulnerability
- Steps to reproduce
- Potential impact
- Suggested fix (if any)

We will acknowledge receipt within 48 hours and provide a timeline for resolution.

**Do NOT** open a public GitHub issue for security vulnerabilities.

## Security Model

### Authentication
- User passwords are hashed with **Argon2id** (memory-hard, side-channel resistant)
- Sessions use **JWT tokens** stored in HttpOnly, Secure, SameSite=Strict cookies
- Login is rate-limited to 5 attempts per IP per 15 minutes

### Agent Communication
- The agent listens on a **Unix socket** (not a network port), preventing remote access
- All agent requests require a shared **AGENT_TOKEN** header
- The agent runs as root (required for Docker/Nginx/file operations)

### Data Protection
- Database credentials are stored encrypted in PostgreSQL
- SSL private keys have `0600` permissions (root-only read)
- The `.env` file and `api.env` have `0600` permissions

### Input Validation
- Domain names: RFC-compliant format validation
- File paths: traversal prevention (no `..`, no absolute paths, no null bytes)
- SQL: parameterized queries via sqlx (no string interpolation)
- Container IDs: hex-only validation

### Network Security
- API and frontend bind to `127.0.0.1` (localhost only), proxied via Nginx
- Nginx adds security headers: HSTS, X-Content-Type-Options, X-Frame-Options, CSP, Referrer-Policy
- UFW firewall management built into the panel

## Key Rotation

### JWT Secret
```bash
# Generate new secret
NEW_SECRET=$(openssl rand -hex 32)

# Update in .env or api.env
# Restart API service
# All existing sessions will be invalidated
```

### Agent Token
```bash
# Generate new token
NEW_TOKEN=$(openssl rand -hex 16)

# Update /etc/dockpanel/agent.token
# Update .env (AGENT_TOKEN) or api.env
# Restart both agent and API services
```

### Database Password
```bash
# Change in PostgreSQL
docker exec -it dockpanel-db-1 psql -U dockpanel -c "ALTER USER dockpanel PASSWORD 'new_password';"

# Update DATABASE_URL in .env or api.env
# Restart API service
```
