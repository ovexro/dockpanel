# Secrets Manager Guide

The Secrets Manager provides encrypted storage for sensitive configuration values like API keys, database passwords, and tokens. Secrets are encrypted with AES-256-GCM and organized into vaults.

## Concepts

- **Vault**: A named collection of secrets (e.g., `production`, `staging`)
- **Secret**: A key-value pair stored encrypted (e.g., `DATABASE_URL=postgres://...`)
- **Version history**: Every update creates a new version; previous versions are retained
- **Injection**: Push secrets directly into a container's `.env` file

## Create a Vault

### From the Panel

1. Go to **Secrets** in the sidebar
2. Click **New Vault**
3. Enter a name (e.g., `production`)
4. Click **Create**

### From the CLI

```bash
dockpanel secrets vault create production
```

## Add Secrets

### From the Panel

1. Open a vault
2. Click **Add Secret**
3. Enter the key and value
4. Click **Save**

The value is encrypted immediately and never stored in plaintext.

### From the CLI

```bash
dockpanel secrets set production DATABASE_URL "postgres://user:pass@host/db"
dockpanel secrets set production REDIS_URL "redis://localhost:6379"
```

## View and Update Secrets

- Secret values are **masked** by default in the UI and API
- Click **Reveal** to temporarily show a value
- Edit a secret to create a new version (the old version is retained)

### Version History

1. Open a secret
2. Click **History**
3. View all previous versions with timestamps
4. Optionally roll back to a previous version

## Inject Secrets into Containers

Push all secrets from a vault directly into a running container's environment.

### From the Panel

1. Open a vault
2. Click **Inject**
3. Select the target container
4. Click **Inject**

DockPanel writes the secrets to the container's `.env` file and restarts it.

### From the CLI

```bash
dockpanel secrets inject production --container my-app
```

## Pull Secrets (CI/CD)

Use the pull endpoint in your CI/CD pipeline to fetch secrets at deploy time:

```bash
# Using the CLI
dockpanel secrets pull production --format env > .env

# Using curl
curl -H "Authorization: Bearer $TOKEN" \
  https://panel.example.com/api/secrets/vaults/{id}/pull \
  -o .env
```

## Export and Import

### Export a Vault

```bash
dockpanel secrets export production > production-secrets.json
```

The exported file contains encrypted values. Store it securely.

### Import a Vault

```bash
dockpanel secrets import staging < production-secrets.json
```

## Auto-Inject

Auto-inject automatically pushes vault secrets to containers when secrets are updated. Configure this per vault:

1. Open a vault
2. Click **Settings**
3. Enable **Auto-inject**
4. Select target containers
5. When any secret in the vault changes, the linked containers are automatically restarted with the new values

## API Reference

See the [Secrets Manager API](../api-reference.md#secrets-manager) for all endpoints.
