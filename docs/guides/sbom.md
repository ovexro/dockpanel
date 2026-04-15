## SBOMs (Software Bill of Materials)

DockPanel can generate an SPDX 2.3 JSON SBOM for any deployed Docker app's image, listing every package present in the container. This is the **composition** companion to image vulnerability scanning, which reports **risk**.

SBOMs are useful for:

- **Compliance** — the EU Cyber Resilience Act (CRA) requires SBOMs for software placed on the EU market by September 2026.
- **Supply-chain auditing** — answer "what's actually inside this image?" without trusting the registry's metadata.
- **Vulnerability triage** — feed the SBOM into other scanners (Grype, Trivy, Dependency-Track) for a second opinion or richer reporting.

SBOM generation is **disabled by default** — admins opt in from the Settings UI.

## Enable

1. Go to **Settings → Services → SBOM Generation**.
2. Click **Install Generator**. DockPanel downloads [syft](https://github.com/anchore/syft) (~80 MB) into `/var/lib/dockpanel/scanners/`. Self-contained — nothing is written to `/usr/local/bin`, and it lives entirely inside the agent's hardened sandbox.

That's the whole setup. SBOMs are generated on demand — there is no scheduled sweep.

## Download an SBOM for a deployed app

1. On the **Apps** page, click the row of any running app to open its scan drawer.
2. Click **Download SBOM**. DockPanel runs syft against the app's image (10 – 60 s on first generation), persists the result, and triggers a browser download of `<app>.spdx.json`.

Subsequent downloads of the same image return the persisted SBOM immediately — re-click **Download SBOM** to regenerate from a fresh image pull if the image has been updated.

## Verify the SBOM matches the binary you're auditing

The SBOM endpoint returns SPDX 2.3 JSON. Quick spot-checks:

```bash
# How many packages does syft see in this image?
jq '.packages | length' my-app.spdx.json

# What's the SPDX document namespace (proves which image was scanned)?
jq -r '.documentNamespace' my-app.spdx.json

# List package + version pairs
jq -r '.packages[] | "\(.name)\t\(.versionInfo)"' my-app.spdx.json
```

The SBOM also pairs cleanly with `grype` to repeat the vulnerability assessment without re-pulling the image:

```bash
grype sbom:./my-app.spdx.json
```

## Verifying DockPanel's own release SBOMs

Since v2.7.10, DockPanel itself ships SPDX SBOMs for `dockpanel-agent`, `dockpanel-api`, and `dockpanel` (CLI), all signed with cosign keyless via Sigstore. See [SECURITY.md](https://github.com/ovexro/dockpanel/blob/main/SECURITY.md#verifying-release-signatures) for the full verification snippet — short version:

```bash
cosign verify-blob \
  --certificate dockpanel-agent.spdx.json.pem \
  --signature  dockpanel-agent.spdx.json.sig \
  --certificate-identity-regexp '^https://github\.com/ovexro/dockpanel/\.github/workflows/release\.yml@refs/tags/v.+$' \
  --certificate-oidc-issuer 'https://token.actions.githubusercontent.com' \
  dockpanel-agent.spdx.json
```

A successful verify proves the SBOM was produced by this repository's release workflow and recorded in the public Rekor transparency log.

## API

All endpoints require admin auth.

| Method | Path | Purpose |
|--------|------|---------|
| `GET` | `/api/sbom/settings` | Whether the syft binary is installed |
| `POST` | `/api/sbom/install` | Install syft |
| `POST` | `/api/sbom/uninstall` | Remove syft |
| `POST` | `/api/sbom/generate` | Generate SBOM for `{"image": "..."}` |
| `GET` | `/api/sbom/image/:ref` | Fetch the persisted SBOM for an image ref |
| `POST` | `/api/apps/:name/sbom` | Generate SBOM for an app's image (returns JSON) |
| `GET` | `/api/apps/:name/sbom` | Download the persisted SBOM as `application/json` attachment |

## Storage

SBOMs live in the `image_sbom` table — one row per image, overwritten on regeneration. Stored as `JSONB` so the API serves the SPDX document directly without re-parsing on the agent.

The syft binary lives at `/var/lib/dockpanel/scanners/syft`. Uninstalling from the UI removes it.

## Relationship to image vulnerability scanning

Image scanning (grype) and SBOM generation (syft) are siblings — both share the install pattern, both target the same images, but they answer different questions:

- **Image Vulnerability Scanning (grype)** — *what's broken?* Per-app CVE counts, deploy gating, scheduled rescans.
- **SBOM Generation (syft)** — *what's in there?* On-demand SPDX export per image, suitable for compliance and external tooling.

Operators who care about supply-chain provenance should enable both.
