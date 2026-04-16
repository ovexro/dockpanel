# ACME Profiles & Renewal

DockPanel issues and renews certificates via the ACME protocol
([RFC 8555](https://datatracker.ietf.org/doc/rfc8555/)) with two 2026-ready
extensions enabled by default:

- **Profiles** — client picks which certificate "shape" (lifetime,
  extensions, policies) the CA should issue. Let's Encrypt exposes
  `classic`, `tlsserver`, `shortlived`, and `tlsclient`.
- **ACME Renewal Information (ARI)**
  ([RFC 9773](https://datatracker.ietf.org/doc/rfc9773/)) — CA tells the
  client *when* to renew instead of the client guessing.

Together they prepare DockPanel for Let's Encrypt's staged move to
shorter-lived certificates:

| Date | Change |
|------|--------|
| 2026-01-15 | `shortlived` profile (~6-day certs) generally available |
| 2026-05-13 | `tlsserver` profile switches to 45-day certs (opt-in) |
| 2027-02-10 | Default `classic` profile → 64-day with 10-day authz reuse |
| 2028-02-16 | Default `classic` profile → 45-day with 7-hour authz reuse |

You don't have to change anything to remain working — classic stays the
default until 2027. This guide describes how to opt into the newer
profiles when you're ready.

## Picking a default profile

Settings → **ACME Profile** lists whatever profiles the CA advertises in
its server directory, with the descriptions the CA publishes.

Recommendations:

- **`classic`** (DockPanel default) — 90-day certs today, 64-day in Feb
  2027, 45-day in Feb 2028. Widest compatibility. Pick this if you're
  unsure.
- **`tlsserver`** — same as classic today; becomes **45-day on
  2026-05-13**. Opt into this when you want to rehearse short-lived
  renewals before the automatic classic flip in 2028.
- **`shortlived`** — ~6-day (160-hour) certs.
  [Requires](https://letsencrypt.org/2026/01/15/6day-and-ip-general-availability/)
  renewal every 2-3 days. Use only if you fully trust your automation and
  want the smallest possible compromise window.
- **`tlsclient`** — legacy, phased out by 2026-07-08. Don't pick it.

The default applies to new issuances. Existing certificates keep their
original profile until renewed. You can override the default per
certificate via the site's SSL panel.

## How renewal decides when to run

The auto-healer runs every 120 seconds and does two things:

1. **Refresh ARI.** For each SSL-enabled site whose last ARI fetch is
   older than 6 hours, ask the CA's `/renewalInfo` endpoint for a
   suggested renewal window (`ssl_renewal_at` and `ssl_renewal_before`).
2. **Renew when the window opens.** If `ssl_renewal_at` has passed, kick
   off an ACME order. The previous certificate's authority-key-identifier
   and serial are attached as the ARI `replaces` hint so the CA can
   correlate the renewal with the original issuance.

When the CA doesn't advertise ARI (or the fetch fails), DockPanel falls
back to a profile-aware threshold:

| Profile | Fallback: renew when `days_remaining` ≤ |
|---------|-----------------------------------------|
| `shortlived` | 2 |
| `tlsserver` | 15 |
| `classic` / unknown | 30 |

Failed renewals trigger a 6-hour cooldown per domain to avoid rate-limit
exhaustion.

## Force-renewing a certificate

**Certificates page → Renew** issues a fresh cert immediately. This calls
the agent's `/ssl/{domain}/renew` endpoint, which uses the same ACME path
as provisioning (the previous hybrid certbot-CLI path was removed in
v2.7.17). Force-renew respects the cert's stored profile — it does not
reset to the panel default.

## API surface (admin)

```
GET  /api/ssl/profiles              # list CA-advertised profiles + current default
POST /api/ssl/default-profile       # { "profile": "classic" | "tlsserver" | "shortlived" | null }
POST /api/sites/{id}/ssl?profile=X  # provision with explicit profile (overrides default)
POST /api/ssl/{id}/renew            # force-renew (preserves site's profile)
```

## What's not done yet

- **DNS-PERSIST-01** — Let's Encrypt targets a Q2 2026 production
  rollout. DockPanel will land support after the LE production date is
  announced and the upstream `instant-acme` crate exposes a stable API.
  The current Cloudflare DNS-01 flow continues to work as before.
- **Grafana dashboard JSON** for the cert-renewal metrics — planned with
  the Prometheus endpoint polish pass.
