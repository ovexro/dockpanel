# Image Vulnerability Scanning

DockPanel can scan every Docker app's image against Anchore's [grype](https://github.com/anchore/grype) vulnerability database, surface a severity badge next to each app, and optionally refuse deploys on images that exceed a CVE threshold.

Scanning is **disabled by default**. Existing installs see no behaviour change after upgrade — admins opt in from the Settings UI.

## Enable

1. Go to **Settings → Services → Image Vulnerability Scanning**.
2. Click **Install Scanner**. DockPanel downloads grype (~70 MB) into `/var/lib/dockpanel/scanners/`, primes the CVE database, and registers it with the agent. This is self-contained — nothing is written to `/usr/local/bin`, and the database lives inside the agent's sandbox.
3. *(Optional)* Toggle **Enable scheduled scans** to let the background sweeper rescan every running app's image at the configured interval (default 24 h, range 1 – 720).
4. *(Optional)* Toggle **Scan on deploy** and pick a **Deploy-gate threshold** — `critical`, `high`, or `medium`. Leave at `none` to observe without blocking.

## What the Apps page shows

Each running app row shows two badges next to its name:

- **Update** — an image tag is behind the registry.
- **CVE** — the latest scan's highest-severity count (critical/high/medium/low). Hover for exact counts; click the row to open the drawer.

The drawer lists every vulnerability from the latest scan: CVE ID, severity, affected package, installed version, and the fixed version (if one is available upstream). Click **Scan Now** to force an immediate rescan.

## Deploy gate

When the deploy gate is set to `critical`, `high`, or `medium`, new deploys check the template's image against the latest stored scan:

- If the last scan is within the past 7 days and exceeds the threshold, the deploy is refused with a clear message naming the counts and the image.
- If no recent scan exists, the deploy is allowed and a best-effort background scan starts. The next attempt of the same image will be gated.

The gate is intentionally soft on first contact — it never blocks the very first deploy of an image on a cold database, so operators aren't stuck waiting 30–180 s on a blocking scan the first time they use a template.

## API

All endpoints require admin auth.

| Method | Path | Purpose |
|--------|------|---------|
| `GET` | `/api/image-scan/settings` | Current toggles + installed state |
| `PUT` | `/api/image-scan/settings` | Update toggles |
| `POST` | `/api/image-scan/install` | Install grype + prime DB |
| `POST` | `/api/image-scan/uninstall` | Remove grype + DB |
| `POST` | `/api/image-scan/scan` | Ad-hoc scan of `{"image": "..."}` |
| `POST` | `/api/apps/:name/scan` | Scan the image used by a specific app |
| `GET` | `/api/apps/:name/scan` | Latest stored scan for an app's image |
| `GET` | `/api/image-scan/recent` | Latest row per scanned image |

## Storage

Scan results are stored in the `image_scan_findings` table. Per image, only the most recent 30 scans are retained; older rows are trimmed after each scan to keep the table lean.

The grype binary lives at `/var/lib/dockpanel/scanners/grype` and its vulnerability database at `/var/lib/dockpanel/scanners/grype-db`. Uninstalling from the UI removes both.

## Relationship to the full-server security scan

This is distinct from the weekly full-server security scan (Settings → Security → Security Scans), which aggregates container vuln counts into a single server-wide report. Image scanning drills into each image individually so you can act on a specific app.
