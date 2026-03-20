# DockPanel vs Alternatives

A honest comparison. Every panel has strengths — this is where DockPanel fits.

## Quick Comparison

| Feature | DockPanel | HestiaCP | CloudPanel | RunCloud | CyberPanel | Ploi |
|---------|-----------|----------|------------|----------|------------|------|
| **Price** | Free | Free | Free | $8/mo+ | Free | $8/mo+ |
| **Self-hosted** | Yes | Yes | Yes | No (SaaS) | Yes | No (SaaS) |
| **Open source** | MIT | GPLv3 | No | No | GPLv3 | No |
| **Language** | Rust | PHP/Bash | PHP | PHP | Python | PHP |
| **Docker native** | Yes | No | No | No | Docker option | No |
| **Multi-server** | Yes | No | No | Yes | No | Yes |
| **Git deploy** | Yes (blue-green) | No | No | Yes | No | Yes |
| **CLI** | Yes | Yes (v-commands) | No | No | Yes | No |
| **IaC (YAML)** | Yes | No | No | No | No | No |
| **ARM64** | Yes | Partial | No | N/A | No | N/A |
| **RAM usage** | ~60MB | ~200MB+ | ~150MB+ | N/A | ~300MB+ | N/A |
| **2FA** | Yes | No | No | Yes | No | Yes |
| **Reseller** | Yes | No | No | No | Yes | No |
| **OAuth/SSO** | Yes | No | No | No | No | No |

## Where DockPanel Wins

**Performance** — Rust binaries total ~35MB, use ~60MB RAM. Starts in <2 seconds. No PHP interpreter overhead.

**Docker native** — Databases, apps, and stacks are Docker containers. 54 one-click templates. Compose stack management. Not bolted on — it's the foundation.

**Developer tools** — Git push-to-deploy with blue-green zero-downtime updates, Nixpacks auto-detection (30+ languages), preview environments, CLI, Infrastructure as Code (YAML export/import).

**Modern stack** — React 19 frontend with 3 layout themes, command palette (Ctrl+K), real-time metrics, WebSocket terminal.

**Business features out of the box** — Reseller accounts, white-label branding, OAuth/SSO, extension API, migration wizard (import from cPanel/Plesk/HestiaCP).

## Where Others Win

**HestiaCP** — Mature, battle-tested, large community. Better for traditional shared hosting setups. Includes its own DNS server (BIND), mail server, and backup system that are proven in production.

**CloudPanel** — Optimized specifically for PHP/Node.js hosting. Simpler interface with fewer features but faster to learn. MySQL/MariaDB management is more polished.

**RunCloud** — SaaS model means zero server maintenance for the panel itself. Their support team handles panel updates and issues. Better for agencies who don't want to self-manage.

**CyberPanel** — Built for OpenLiteSpeed/LiteSpeed. If you need LiteSpeed-specific features (LSCache, QUIC), CyberPanel is the only option.

**Ploi** — Excellent Laravel-specific features. Deep integration with Laravel Forge patterns. Better for Laravel-heavy shops.

## Who Should Use DockPanel

- **Self-hosters** who want full control without SaaS subscriptions
- **Docker users** who want a GUI for container management alongside traditional hosting
- **Developers** who need Git deploy, CLI, and IaC in a hosting panel
- **Homelab enthusiasts** running ARM64 (Raspberry Pi, Oracle Cloud free tier)
- **Agencies** who need reseller accounts and white-label branding
- **Migration projects** moving from cPanel/Plesk to a modern stack

## Who Should NOT Use DockPanel

- Teams that need **commercial support SLAs** (we're open source, community-supported)
- Shops committed to **OpenLiteSpeed/LiteSpeed** (use CyberPanel)
- Users who want a **managed SaaS experience** (use RunCloud or Ploi)
- Environments requiring **FIPS compliance** (not yet certified)
