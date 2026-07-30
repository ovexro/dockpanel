# Backups Guide

## Create a Manual Backup

### From the Panel

1. Go to **Backups** in the sidebar
2. Click **Create Backup**
3. Select the site to back up
4. Click **Create**

The backup contains the site's directory — its files — **and a dump of every database attached to
the site**. It is saved as a single compressed tarball in `/var/backups/dockpanel/`.

For a CMS such as WordPress, almost everything you think of as "the site" — posts, pages,
comments, users, settings — lives in the database rather than in the files, so a backup that
holds only files cannot bring the site back. Since v2.34.0 both halves travel in the same archive
and are restored together.

> **Backups made before v2.34.0 contain files only.** Nothing rewrites an archive that already
> exists, so any backup taken by an earlier version holds no database. The Backups page marks
> those **Files only**, and restoring one warns you before it starts. Take a fresh backup if you
> want one that includes your content.

The site's Nginx configuration is not in the tarball; the panel rewrites it from its own records.

### What is inside the archive

```
./                             the site's document root, exactly as it is on disk
.dockpanel-backup/manifest.json  what this archive holds
.dockpanel-backup/db/*.sql.gz    one compressed dump per database
```

The `.dockpanel-backup` directory is DockPanel's own; it is extracted separately during a restore
and never lands in your document root.

A database that could not be dumped is **not** silently skipped: the backup reports which ones are
missing, the Backups page shows the archive as incomplete, and a restore tells you before it runs.

### From the CLI

```bash
dockpanel backup create example.com
```

Sample output:

```
Creating backup for example.com...
✓ Backup created
  File:    example.com-20260320-143022.tar.gz
  Size:    45.2 MB
  Content: files only

! This archive contains the site's files but NOT its databases.
  The CLI talks to the agent directly and cannot resolve a site's databases.
  Create the backup from the panel (or the panel API) to include them.
```

> **CLI backups are files only.** The CLI authenticates to the agent, and the agent has no access
> to the panel's database records — it cannot know which databases belong to a site or hold their
> credentials. Only the panel can assemble a complete backup.

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

1. Go to **Backup Manager** > **Destinations**
2. Click **Add Destination**
3. Enter:
   - **Name**: A label (e.g., `backblaze-b2`)
   - **Type**: S3-compatible
   - **Endpoint**: `https://s3.us-west-001.backblazeb2.com` (varies by provider)
   - **Bucket**: `my-server-backups`
   - **Access Key**: Your access key ID
   - **Secret Key**: Your secret access key
   - **Region**: `us-west-001` (varies by provider)
4. Click **Create Destination**. Credentials are encrypted before they are stored.
5. Click **Test** on the saved destination to verify access — nothing is checked at
   save time, and the test runs against the stored credentials.

Use **Edit** to change a destination later. Secret fields come back masked as
`********`; leave them as they are to keep the stored credential, or type a new
value to rotate it. The transport (S3 or SFTP) cannot be changed on an existing
destination — delete it and add a new one instead.

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

The restore replaces the site's files with the backup contents, then loads each database dump the
archive carries **over the live database**, dropping and recreating its tables. The current state
is not automatically backed up before a restore — create a manual backup first if you want a
safety net.

Files are restored first and databases last. A database load that fails rolls back, so the live
data is left as it was rather than half-overwritten.

Two outcomes are reported honestly rather than as a success:

- **The archive has no database** (it was made before v2.34.0, or its dump failed) and the site
  has one. You are told before the restore starts that the content will not come back.
- **The files were restored but a database could not be.** This is called out as a failure, not a
  success with a footnote — the site is at that point running restored files against its previous
  content, and you need to know that.

### From the CLI

```bash
dockpanel backup restore example.com example.com_2026-03-20_143022.tar.gz
```

Sample output:

```
Restoring example.com from example.com_2026-03-20_143022.tar.gz...
✓ Backup restored
  Content: files only (this archive holds no database)
```

> **The CLI cannot restore databases.** It authenticates to the agent, which has no access to the
> panel's database records and so cannot be given the site's database credentials. Restoring an
> archive that carries database dumps therefore **fails** from the CLI rather than restoring the
> files and reporting success. Use the panel for those.

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

This is separate from site backups, and it covers the panel's own database only — not your
sites'. A site's database (its MySQL or PostgreSQL container) is not captured by either one, so
back it up with the manual step below or from the **Databases** page.

### Manual database-only backup

To back up a specific database container:

```bash
# PostgreSQL
docker exec CONTAINER_NAME pg_dump -U USERNAME DBNAME > /tmp/db-backup.sql

# MySQL / MariaDB
docker exec CONTAINER_NAME mysqldump -u root -pPASSWORD DBNAME > /tmp/db-backup.sql
```

Replace `CONTAINER_NAME`, `USERNAME`, `PASSWORD`, and `DBNAME` with your actual values. Find these in the panel under Databases.
