# Backup Orchestrator Guide

The Backup Orchestrator provides centralized backup management for all databases and Docker volumes on your server. It supports AES-256-GCM encryption, automated policies, restore verification, and a health dashboard.

## Overview

Unlike per-site backups (see [Backups](backups.md)), the Backup Orchestrator works at the infrastructure level:

- **Database backups**: Any PostgreSQL, MySQL/MariaDB, or MongoDB container
- **Volume backups**: Any Docker named volume
- **Policies**: Automated schedules with retention rules
- **Encryption**: Optional AES-256 encryption at rest
- **Verification**: Automated restore tests to confirm backup integrity
- **Destinations**: Push backups to S3, SFTP, Backblaze B2, or Google Cloud Storage

## Quick Start: Protect Everything

The fastest way to get started is the one-click preset.

1. Go to **Backup Orchestrator** in the sidebar
2. Click **Protect Everything**
3. DockPanel creates a policy that backs up all sites, databases, and volumes daily at 2 AM, keeps 7 days of history, and verifies each backup after creation.

## Creating Backup Policies

For more control, create a custom policy.

1. Go to **Backup Orchestrator** > **Policies**
2. Click **Create Policy**
3. Configure:
   - **Name**: A label (e.g., `production-dbs-nightly`)
   - **What to back up**: Toggle sites, databases, and/or volumes
   - **Schedule**: Cron expression (default: `0 2 * * *` = daily at 2 AM)
   - **Destination**: Optional remote destination
   - **Retention**: Backups to keep, 1-365 (default: 7)
   - **Encrypt**: Enable AES-256 encryption
   - **Verify after backup**: Run automated verification after each backup
4. Click **Save**

Policies run on schedule. The last run time and status are shown in the policy list.

## Database Backups

### Create a Database Backup

1. Go to **Backup Orchestrator** > **Database Backups**
2. Select the database
3. Click **Create Backup**

The backup runs `pg_dump` or `mysqldump` inside the container and stores the compressed result.

**From the CLI:**

```bash
dockpanel backup db-create dockpanel-db-myapp myapp --db-type postgres --user root --password secret
```

### List Database Backups

```bash
dockpanel backup db-list myapp
```

### Restore a Database

1. Find the backup in the **Database Backups** list
2. Click **Restore**
3. Confirm

The restore replaces the database contents. Encrypted backups are decrypted automatically.

## Volume Backups

### Create a Volume Backup

1. Go to **Backup Orchestrator** > **Volume Backups**
2. Select the container and volume
3. Click **Create Backup**

**From the CLI:**

```bash
dockpanel backup vol-create my_app_data my-app-container
```

### List Volume Backups

```bash
dockpanel backup vol-list my-app-container
```

### Restore a Volume

1. Find the backup in the **Volume Backups** list
2. Click **Restore**
3. Confirm

The container is temporarily stopped during restore, then restarted.

## Backup Verification

Verification confirms a backup is intact and restorable by checking file integrity, decompression, headers, and structure.

### Automatic Verification

Enable **Verify after backup** on a policy to verify every backup automatically. Results appear in the Verifications tab.

### Manual Verification

1. Find any backup in the list and click **Verify**
2. The check runs in the background

**From the CLI:**

```bash
dockpanel backup verify --type database myapp myapp_2026-03-22_020000.sql.gz
```

Sample output:

```
Verifying database backup: myapp_2026-03-22_020000.sql.gz...
  Verification PASSED (4/4 checks, 320ms)
    File exists and readable
    Decompresses without errors
    SQL header valid
    Table count matches (23 tables)
```

Verification types: `site`, `database`, `volume`.

## Backup Destinations

Store backups off-server. Supported providers:

| Provider | Type | Endpoint example |
|----------|------|-----------------|
| AWS S3 | `s3` | `https://s3.us-east-1.amazonaws.com` |
| Backblaze B2 | `b2` | `https://s3.us-west-001.backblazeb2.com` |
| Google Cloud Storage | `gcs` | `https://storage.googleapis.com` |
| Any SFTP server | `sftp` | `sftp://backup.example.com:22` |
| DigitalOcean Spaces | `s3` | `https://nyc3.digitaloceanspaces.com` |
| MinIO (self-hosted) | `s3` | `https://minio.example.com:9000` |

Add a destination under **Backups** > **Destinations**, then select it in your policies.

## Encryption

When encryption is enabled:

- **Algorithm**: AES-256-GCM
- **Key management**: Keys are stored encrypted in the panel database
- Encrypted backups have a `.enc` extension
- Decryption happens automatically during restore and verification

## Health Dashboard

**From the panel:** Go to **Backup Orchestrator** to see the health overview.

**From the CLI:**

```bash
dockpanel backup health
```

The dashboard shows:

- **Backup counts**: Site, database, and volume backups with total storage
- **Last 24h**: How many scheduled backups succeeded or failed
- **Active policies**: Enabled vs total policies
- **Verification rate**: Passed vs failed verifications
- **Stale resources**: Sites or databases with no backup in over 7 days

## Storage Tracking

The **Storage History** chart shows backup storage growth over the last 30 days. Use it to plan disk capacity.

## API Reference

| Method | Endpoint | Description |
|--------|----------|-------------|
| `GET` | `/api/backup-orchestrator/health` | Health dashboard (admin) |
| `GET` | `/api/backup-orchestrator/policies` | List policies |
| `POST` | `/api/backup-orchestrator/policies` | Create a policy |
| `PUT` | `/api/backup-orchestrator/policies/{id}` | Update a policy |
| `DELETE` | `/api/backup-orchestrator/policies/{id}` | Delete a policy |
| `POST` | `/api/backup-orchestrator/policies/protect-all` | Create "Protect Everything" policy |
| `POST` | `/api/backup-orchestrator/db-backup` | Create a database backup |
| `GET` | `/api/backup-orchestrator/db-backups` | List database backups |
| `DELETE` | `/api/backup-orchestrator/db-backups/{id}` | Delete a database backup |
| `POST` | `/api/backup-orchestrator/db-backups/{id}/restore` | Restore a database |
| `POST` | `/api/backup-orchestrator/volume-backup` | Create a volume backup |
| `GET` | `/api/backup-orchestrator/volume-backups` | List volume backups |
| `POST` | `/api/backup-orchestrator/volume-backups/{id}/restore` | Restore a volume |
| `POST` | `/api/backup-orchestrator/verify` | Trigger verification |
| `GET` | `/api/backup-orchestrator/verifications` | List verifications |
| `GET` | `/api/backup-orchestrator/storage-history` | Storage growth (30 days) |

All endpoints require authentication. Health and storage endpoints require admin access.
