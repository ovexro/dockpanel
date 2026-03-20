# Backups Guide

## Create a Manual Backup

### From the Panel

1. Go to **Backups** in the sidebar
2. Click **Create Backup**
3. Select the site to back up
4. Click **Create**

The backup includes the site files, database (if attached), and Nginx configuration. It is saved as a compressed tarball in `/var/backups/dockpanel/`.

### From the CLI

```bash
dockpanel backup create example.com
```

Sample output:

```
Backup created: example.com_2026-03-20_143022.tar.gz (45.2 MB)
Location: /var/backups/dockpanel/example.com/example.com_2026-03-20_143022.tar.gz
```

### List Backups

```bash
dockpanel backup list example.com
```

Sample output:

```
FILENAME                                    SIZE      DATE
example.com_2026-03-20_143022.tar.gz        45.2 MB   2026-03-20 14:30
example.com_2026-03-19_020000.tar.gz        44.8 MB   2026-03-19 02:00
example.com_2026-03-18_020000.tar.gz        44.1 MB   2026-03-18 02:00
```

## Set Up Scheduled Backups

1. Go to **Backups** in the sidebar
2. Click the **Schedules** tab
3. Click **Create Schedule**
4. Configure:
   - **Site**: Select the site (or "All sites")
   - **Frequency**: Daily, weekly, or custom cron expression
   - **Time**: When to run (e.g., 02:00)
   - **Retention**: Number of backups to keep (older ones are automatically deleted)
5. Click **Save**

Scheduled backups run in the background. The backup scheduler checks for pending jobs at the configured interval.

## Configure S3 / Remote Destination

Store backups off-server for disaster recovery. DockPanel supports any S3-compatible storage (AWS S3, Backblaze B2, MinIO, Wasabi, DigitalOcean Spaces, etc.).

1. Go to **Backups** > **Destinations**
2. Click **Add Destination**
3. Enter:
   - **Name**: A label (e.g., `backblaze-b2`)
   - **Type**: S3-compatible
   - **Endpoint**: `https://s3.us-west-001.backblazeb2.com` (varies by provider)
   - **Bucket**: `my-server-backups`
   - **Access Key**: Your access key ID
   - **Secret Key**: Your secret access key
   - **Region**: `us-west-001` (varies by provider)
4. Click **Test Connection** to verify access
5. Click **Save**

Once a destination is configured, edit your backup schedule and select it as the remote destination. Backups will be uploaded after creation.

### Provider-specific endpoints

| Provider | Endpoint |
|----------|----------|
| AWS S3 | `https://s3.amazonaws.com` (or regional: `https://s3.us-east-1.amazonaws.com`) |
| Backblaze B2 | `https://s3.REGION.backblazeb2.com` |
| DigitalOcean Spaces | `https://REGION.digitaloceanspaces.com` |
| Wasabi | `https://s3.REGION.wasabisys.com` |
| MinIO (self-hosted) | `https://your-minio-server:9000` |

## Restore from Backup

### From the Panel

1. Go to **Backups**
2. Find the backup you want to restore
3. Click **Restore**
4. Confirm the restore

The restore replaces the site files and database with the backup contents. The current state is not automatically backed up before restore -- create a manual backup first if you want a safety net.

### From the CLI

```bash
dockpanel backup restore example.com example.com_2026-03-20_143022.tar.gz
```

Sample output:

```
Restoring example.com from example.com_2026-03-20_143022.tar.gz...
  [1/3] Extracting files...
  [2/3] Restoring database...
  [3/3] Reloading nginx...
Restore complete.
```

## Delete a Backup

### From the CLI

```bash
dockpanel backup delete example.com example.com_2026-03-18_020000.tar.gz
```

### From the Panel

Click the delete icon next to any backup in the list.

## Database Backups

DockPanel runs an automatic daily database backup cron job for the panel's own PostgreSQL database:

- **Schedule**: Daily at 2:00 AM
- **Retention**: 7 days (older backups are automatically deleted)
- **Location**: `/var/backups/dockpanel/`

This is separate from site backups. Site backups include the site's own database (MySQL or PostgreSQL container) as part of the site backup tarball.

### Manual database-only backup

To back up a specific database container:

```bash
# PostgreSQL
docker exec CONTAINER_NAME pg_dump -U USERNAME DBNAME > /tmp/db-backup.sql

# MySQL / MariaDB
docker exec CONTAINER_NAME mysqldump -u root -pPASSWORD DBNAME > /tmp/db-backup.sql
```

Replace `CONTAINER_NAME`, `USERNAME`, `PASSWORD`, and `DBNAME` with your actual values. Find these in the panel under Databases.
