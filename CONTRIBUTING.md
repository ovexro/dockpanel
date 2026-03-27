# Contributing to DockPanel

Thanks for your interest in contributing! This guide covers development setup, code style, and the PR process.

## Development Setup

### Prerequisites

- **Rust 1.94+**: `curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh`
- **Node.js 20+**: For building the frontend
- **Docker**: For running PostgreSQL locally
- **Build tools**: `build-essential cmake pkg-config` (Ubuntu/Debian) or `gcc gcc-c++ cmake make` (RHEL/Fedora)

### Getting Started

```bash
git clone https://github.com/ovexro/dockpanel.git
cd dockpanel

# Start PostgreSQL
docker run -d --name dockpanel-postgres \
  -e POSTGRES_USER=dockpanel \
  -e POSTGRES_PASSWORD=dockpanel \
  -e POSTGRES_DB=dockpanel \
  -p 5450:5432 postgres:16

# Create config
sudo mkdir -p /etc/dockpanel
cat <<EOF | sudo tee /etc/dockpanel/api.env
DATABASE_URL=postgresql://dockpanel:dockpanel@127.0.0.1:5450/dockpanel
JWT_SECRET=$(openssl rand -hex 32)
AGENT_SOCKET=/var/run/dockpanel/agent.sock
AGENT_TOKEN=$(uuidgen)
LISTEN_ADDR=127.0.0.1:3080
EOF

# Build everything
cargo build --release --manifest-path panel/agent/Cargo.toml
cargo build --release --manifest-path panel/backend/Cargo.toml
cargo build --release --manifest-path panel/cli/Cargo.toml
cd panel/frontend && npm install && npx vite build && cd ../..

# Run (agent needs root for system operations)
sudo ./panel/agent/target/release/dockpanel-agent &
./panel/backend/target/release/dockpanel-api &
cd panel/frontend && npm run dev
```

The frontend dev server proxies `/api` to `127.0.0.1:3080` (see `panel/frontend/vite.config.ts`).

## Architecture

```
panel/
├── agent/       # Rust — host-level operations (Docker, Nginx, SSL, terminal)
│   ├── src/routes/     # HTTP endpoint handlers (33 files)
│   └── src/services/   # Business logic (29 files)
├── backend/     # Rust — API server, auth, DB, multi-server dispatch
│   ├── src/routes/     # REST endpoints (50 files)
│   ├── src/services/   # Background tasks (20 files)
│   └── migrations/     # SQL migrations (69 files)
├── cli/         # Rust — CLI tool (clap-based)
│   └── src/commands/   # Subcommand handlers (11 files)
└── frontend/    # React 19 + TypeScript + Tailwind 4
    └── src/pages/      # Lazy-loaded page components (48 files)
```

**Agent** handles host-level operations: Docker, Nginx config, SSL certificates, file system, terminal (PTY), backups. Runs as root. Communicates via Unix socket (local) or HTTPS (remote servers).

**Backend** handles coordination: auth (JWT + 2FA), PostgreSQL persistence, multi-server agent dispatch, background services (alerts, monitoring, auto-healing, scheduled backups/deploys). Runs as unprivileged user.

**Frontend** is a React SPA with lazy-loaded pages. Each major feature maps to a page file in `src/pages/`.

## Code Style

- **Rust**: Edition 2024. Run `cargo fmt` before committing. `cargo clippy` warnings should be addressed.
- **TypeScript**: Strict mode enabled. Minimize `as any` casts. No `console.log` in production code (use `logger.ts`).
- **SQL migrations**: Use `IF NOT EXISTS` / `IF EXISTS` where possible. Timestamp prefix format: `YYYYMMDDHHMMSS_description.sql`.

## Making Changes

1. **Fork and branch**: Create a feature branch from `main`.
2. **Read before editing**: Understand existing code before modifying. Check `FEATURES.md` for the feature manifest.
3. **Keep it focused**: One feature or fix per PR. Don't bundle unrelated changes.
4. **Build all crates**: Changes to shared types or agent routes may affect multiple crates.
5. **Test manually**: Run the panel locally and verify your changes work end-to-end.

## Pull Request Process

1. Describe what the PR does and why.
2. List any new environment variables, migrations, or dependencies.
3. Confirm you've built and tested locally.
4. Keep PRs reasonably sized — large PRs are harder to review.

## Filing Issues

- **Bug reports**: Include OS, DockPanel version, steps to reproduce, and relevant logs (`journalctl -u dockpanel-api -n 50`).
- **Feature requests**: Describe the use case, not just the solution.

## Key Files

| What | Where |
|------|-------|
| Feature manifest | `FEATURES.md` |
| API config | `panel/backend/src/config.rs` |
| Agent startup | `panel/agent/src/main.rs` |
| API startup | `panel/backend/src/main.rs` |
| DB schema | `panel/backend/migrations/` |
| Frontend routes | `panel/frontend/src/main.tsx` |
