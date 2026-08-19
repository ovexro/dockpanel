# Secrets Manager Guide

The Secrets Manager provides encrypted storage for sensitive configuration values like API keys, database passwords, and tokens. Secrets are encrypted with AES-256-GCM and organized into vaults.

## Concepts

- **Vault**: A named collection of secrets (e.g., `production`, `staging`)
- **Secret**: A key-value pair stored encrypted (e.g., `DATABASE_URL=postgres://...`)
- **Version history**: Every update creates a new version; previous versions are retained
- **Injection**: Write a vault's auto-inject secrets into a site's environment

## Create a Vault

### From the Panel

1. Go to **Secrets** in the sidebar
2. Click **New Vault**
3. Enter a name (e.g., `production`)
4. Click **Create**

> **Note:** vaults and secrets are managed from the panel or the HTTP API.
> The `dockpanel` CLI has no `secrets` subcommand.

## Add Secrets

### From the Panel

1. Open a vault
2. Click **Add Secret**
3. Enter the key and value
4. Click **Save**

The value is encrypted immediately and never stored in plaintext.

### From the API

```bash
curl -X POST -H "Authorization: Bearer $TOKEN" \
  -H "Content-Type: application/json" \
  -d '{"key":"DATABASE_URL","value":"postgres://user:pass@host/db","auto_inject":true}' \
  https://panel.example.com/api/secrets/vaults/{vault_id}/secrets
```

## View and Update Secrets

- Secret values are **masked** by default in the UI and API
- Click **Reveal** to temporarily show a value
- Edit a secret to create a new version (the old version is retained)

### Version History

1. Open a secret
2. Click **History**
3. View the change log: version number, who changed it, the kind of change, and when

History is an audit trail, not a recovery feature. Previous **values** are not
returned by the API, and there is no roll-back endpoint -- to restore an old
value you must know it and set it again.

## Inject Secrets into a Site

Write a vault's auto-inject secrets into a **site's** environment.

```
POST /api/secrets/vaults/{vault_id}/inject/{site_id}
```

What it does, precisely:

- Only secrets flagged **auto-inject** are written. If a vault has none, the call
  returns `400 No auto-inject secrets in this vault`.
- The target is a **site**, not a container, and the host is resolved from the
  site's own row -- not from the caller's selection.
- The values are written to that site's nginx environment. Nothing is restarted;
  the new values apply the next time the site's processes are started.

## Pull Secrets (CI/CD)

Use the pull endpoint in your CI/CD pipeline to fetch secrets at deploy time:

```bash
curl -H "Authorization: Bearer $TOKEN" \
  https://panel.example.com/api/secrets/vaults/{vault_id}/pull \
  -o .env
```

## Export and Import

### Export a Vault

```bash
curl -H "Authorization: Bearer $TOKEN" \
  https://panel.example.com/api/secrets/vaults/{vault_id}/export \
  -o production-secrets.json
```

The exported file contains encrypted values. Store it securely.

### Import a Vault

```bash
curl -X POST -H "Authorization: Bearer $TOKEN" \
  -H "Content-Type: application/json" \
  --data-binary @production-secrets.json \
  https://panel.example.com/api/secrets/vaults/{vault_id}/import
```

## Auto-Inject

Auto-inject marks which secrets take part in an injection. It is a flag on the
**individual secret**, not a vault-level setting, and it selects no targets of
its own:

1. Open a vault
2. Create or edit a secret
3. Tick **Auto-inject**

Flagged secrets are the ones written by
`POST /api/secrets/vaults/{vault_id}/inject/{site_id}`, and they are also picked
up when a site is deployed with a vault attached.

Changing a secret does **not** push it anywhere by itself, and nothing is
restarted automatically -- re-run the injection, or deploy, for a new value to
take effect.

## API Reference

See the [Secrets Manager API](../api-reference.md#secrets-manager) for all endpoints.
