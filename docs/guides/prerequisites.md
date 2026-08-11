<!-- DO NOT EDIT — generated from panel/backend/src/services/prerequisites/copy.rs -->

# What DockPanel checks before you commit

Some things have to be true before a feature can work — a domain has to resolve, a port has to be free, a backup has to have somewhere to go. DockPanel checks these for you and says so on the screen where it matters, rather than letting the action fail and leaving you to work out why.

This page is generated from the same source the product renders its messages from, so the wording below is the wording you will see.

## How to read the three tiers

| Tier | What it looks like | What it means |
|---|---|---|
| Passive | A line under a field, or text behind an (i) | Context. Nothing is wrong. |
| Warning | An amber callout | It probably won't work, or it will leave a mess — but the choice is yours and the control stays usable. |
| Blocking | A red callout, and the button is disabled | It *will* fail. The gate opens by itself once the condition is met. |

A check DockPanel could not run never blocks anything and never accuses you of anything — it says so and gets out of the way.

## DNS

### Does this domain point at this server yet?

`dns.points_here`

An HTTP-01 certificate order sends the certificate authority to your domain to fetch a file from this server. If the domain resolves nowhere, the CA has nowhere to connect and the order fails — after the panel has already told you the site was created.

**When it blocks, and when it only warns.** Blocking when the domain resolves to nothing at all, because issuance cannot possibly succeed. Only a warning when it resolves somewhere else: a Cloudflare-proxied domain resolves to Cloudflare and is indistinguishable from a misconfigured one by address alone, and issuance through the proxy demonstrably works. Refusing it would block a setup we have evidence is fine.

**Where you see it**

- The create-site form, as you type the domain (never blocks creation)
- The SSL card on a site, where it gates the Let's Encrypt button
- `POST /api/sites/{id}/ssl`, which refuses with 412 for the same reason and in the same words

| | What DockPanel says | What to do |
|---|---|---|
| Not checked | **Enter a domain** — Once you enter a domain, DockPanel checks whether it already points here. | Nothing. |
| Not checked | **Couldn't determine this server's public address** — DockPanel could not detect its own public IP, so it can't check whether the domain points here. SSL issuance will still be attempted. | Nothing, usually — the check is unavailable, not failing. If issuance then fails, confirm the server can reach the internet. |
| **Blocks** | **example.com doesn't resolve yet** — No DNS record was found for example.com. Create the record below at whoever manages this domain's DNS, then check again. New records usually appear within a few minutes. | Create the A (or AAAA) record DockPanel shows, at whoever manages the domain's DNS. The gate opens by itself once the record goes live. |
| OK | **example.com points here** — example.com resolves to this server (203.0.113.10). | Nothing. |
| Warns | **example.com points somewhere else** — example.com resolves to 198.51.100.7 rather than this server (203.0.113.10).  If you use Cloudflare's proxy (the orange cloud), this is expected and certificate issuance normally still works — Cloudflare passes the validation request through to this server. You can continue.  Otherwise the domain is pointed at a different host. Update the record below and check again. | If the domain is behind a proxy such as Cloudflare, nothing — issuance works through it. Otherwise point the record at this server. |
| Warns | **www.example.com points somewhere else** — www.example.com resolves to 198.51.100.7 rather than this server (203.0.113.10).  If you use Cloudflare's proxy (the orange cloud), this is expected and certificate issuance normally still works — Cloudflare passes the validation request through to this server. You can continue.  Otherwise the domain is pointed at a different host. Update the record below and check again — note this is the record for www.example.com, not for example.com. | Same as above, with one trap: edit the record for the subdomain itself. Pointing the apex somewhere new does not move a subdomain that has its own record. |

## Docker apps

### Does every setting this app requires have a value?

`apps.required_env`

104 of the shipped templates declare at least one setting that is required and has no default — database passwords, signing keys, and so on. The agent resolves each setting as "the value you gave, otherwise the default", and **drops it entirely when the result is empty**. The variable is not passed as blank; it is not passed at all. Postgres refuses to initialise, Grafana quietly keeps its built-in password, and you find out from container logs several minutes after the image pull finished.

**When it blocks, and when it only warns.** Blocking. The template's own author declared the setting required, and the fix is to type into a field that is already on screen.

**Where you see it**

- The Docker app deploy dialog, above the Deploy button
- `POST /api/apps/deploy`, which refuses with 412 by calling this same check

| | What DockPanel says | What to do |
|---|---|---|
| OK | **Required settings are filled in** — Every setting this app needs has a value. | Nothing. |
| **Blocks** | **POSTGRES_PASSWORD needs a value** — postgres needs a value for POSTGRES_PASSWORD (POSTGRES_PASSWORD). These are passwords or keys with no safe default — the container is started without them entirely, which usually means it refuses to start or comes up with no protection at all.  Use Generate to have DockPanel pick a strong value. | Press Generate beside the field. DockPanel picks a strong value so there is nothing to invent and nothing to remember. |
| **Blocks** | **MEILI_MASTER_KEY needs a value** — meilisearch needs a value for MEILI_MASTER_KEY (MEILI_MASTER_KEY), Environment (MEILI_ENV). Without it the setting is not passed to the container at all, so the app starts misconfigured — usually failing minutes later, after the image has finished downloading. | Fill in the marked fields before deploying. |

### Is the host port this app wants actually free?

`apps.port_available`

Two containers cannot publish on the same host port. The sites surface has refused colliding ports for years; apps never got the equivalent, so a collision surfaced as a Docker error after the image pull rather than before it.

**When it blocks, and when it only warns.** Blocking when something is known to hold the port, because the deploy will certainly fail. Unknown — never a refusal — when the server could not be asked.

**Where you see it**

- The Docker app deploy dialog, above the Deploy button
- `POST /api/apps/deploy`, which refuses with 412 by calling this same check

| | What DockPanel says | What to do |
|---|---|---|
| **Blocks** | **Choose a port** — Pick the host port this app should be reachable on. | Enter a port. |
| **Blocks** | **Port 5432 is already in use** — Port 5432 is taken by the app `pg-old`. Two things cannot publish on the same host port, so this deploy would fail once the image finished downloading. | Use the free port DockPanel suggests, or stop whatever holds this one. |
| **Blocks** | **Port 5432 is already in use** — Something on this server is already listening on port 5432. It isn't managed by DockPanel, so it may be another service you installed yourself. Pick a different port, or stop whatever is using this one. | Pick a different port. `ss -lntp \| grep :{port}` on the server names the process. |
| OK | **Port 5432 is free** — Nothing is listening on port 5432. | Nothing. |
| Not checked | **Couldn't check the port** — DockPanel couldn't ask this server whether port 5432 is free, so it will be attempted as-is. | Nothing. This happens on a fleet member running an agent older than the port-check route; a check that cannot run must never turn into a refusal. |

### Is this app name already taken on this server?

`apps.name_available`

Container names are unique per host, and the deploy form pre-fills the name with the template id — so deploying a second Postgres collides by default.

**When it blocks, and when it only warns.** Blocking. Docker will refuse the name outright.

**Where you see it**

- The Docker app deploy dialog, above the Deploy button
- `POST /api/apps/deploy`, which refuses with 412 by calling this same check

| | What DockPanel says | What to do |
|---|---|---|
| Not checked | **Name this app** — Give the app a name to continue. | Enter a name. |
| **Blocks** | **An app called postgres already exists** — This server already runs an app named postgres. Container names have to be unique, so this deploy would be refused. Pick a different name. | Use the name DockPanel suggests, or choose your own. |
| OK | **Name is available** — No other app is called postgres. | Nothing. |

### Does this server have the memory this app is being given?

`apps.resource_headroom`

A container whose limit exceeds what the box can supply does not fail at deploy time. It fails later, under load, as an OOM kill — which is the worst possible moment to learn the number was never achievable.

**When it blocks, and when it only warns.** Warning, never blocking. Over-committing memory is ordinary practice, the kernel will page, and the operator may know what is about to be freed. What is not acceptable is finding out from an OOM kill.

**Where you see it**

- The Docker app deploy dialog, above the Deploy button

| | What DockPanel says | What to do |
|---|---|---|
| Not checked | **Memory not checked** — No memory limit was set for this app, or this server's memory couldn't be read. | Nothing. |
| Warns | **That's more memory than this server has** — This app is limited to 8192 MB, but the server only has 2048 MB in total (1100 MB currently free). The limit will be accepted, but the container can never reach it — and the server will start swapping first. | Lower the limit, or resize the server. Nothing stops you deploying as-is. |
| Warns | **Less free memory than this app is allowed** — This app is allowed 1024 MB but only 700 MB of the server's 2048 MB is free right now. It will still start; if it actually uses its full limit, the kernel will have to reclaim memory from something else. | Usually nothing — over-committing memory is normal. Worth a second look if the box is already swapping. |
| OK | **Enough memory** — 1100 MB free, 512 MB requested. | Nothing. |

## Mail

### Does this domain have a DKIM signing key?

`mail.dkim_key`

Without a key there is nothing to sign outgoing mail with and no DKIM record to publish. Adding a domain stores it even when key generation fails, so this state is reachable and would otherwise be invisible.

**When it blocks, and when it only warns.** Warning. Mail still flows; it is simply unsigned, and no control in the panel is gated on it.

**Where you see it**

- The Mail domain's DNS tab
- `GET /api/mail/domains/{id}/preflight`

| | What DockPanel says | What to do |
|---|---|---|
| OK | **DKIM signing key is ready** — Outgoing mail for example.com is signed. | Nothing. |
| Warns | **No DKIM signing key** — DKIM key generation failed when example.com was added, so outgoing mail isn't signed and there is no DKIM record to publish. Check that the mail stack is installed, then remove and re-add the domain to retry. | Confirm the mail server is installed, then remove and re-add the domain. |

### Are this domain's mail records published, and do they point at this server?

`mail.dns_published`

Mail leaves the server whether or not these records exist — it just doesn't arrive. Receiving servers use SPF, DKIM and DMARC to decide whether to believe a message came from your domain, and a domain missing them lands in spam folders with nothing anywhere saying so.

This check verifies **correctness, not existence**, which is the whole point of it: through v2.29.0 the older DNS check passed on any MX record at all (including a competitor's), any string beginning `v=spf1` (including one that forbids this server), and any DKIM record containing `p=` (including a stale key from a previous install). Run against `gmail.com` — a domain whose mail goes to Google and whose SPF does not authorise us at all — the old logic reported MX, SPF and DMARC all passing.

**When it blocks, and when it only warns.** Warning, never blocking, and this follows the rule rather than ducking it: nothing in the panel is gated on these records. There is no control to disable, so there is nothing to block.

**Where you see it**

- The Mail domain's DNS tab
- `GET /api/mail/domains/{id}/preflight`
- `GET /api/mail/domains/{id}/dns-check`, the long-standing Check DNS button, which now delegates here

| | What DockPanel says | What to do |
|---|---|---|
| Not checked | **Couldn't determine this server's public address** — DockPanel could not determine the public address of the server this domain is on, so it can't tell you which records the domain needs. | Nothing to do at the registrar. If the domain is on this server, confirm it can reach the internet; if it is on another server in the fleet, confirm that server's agent is online — its address is recorded when the agent checks in. |
| Not checked | **Couldn't check DNS** — DockPanel couldn't run DNS lookups on this server, so it can't verify these records. Create them if you haven't already. | Create the records shown. They are correct whether or not the lookup ran. |
| OK | **example.com's mail records are published** — MX, SPF, DKIM and DMARC all resolve and point at this server. Mail from this domain can be authenticated. | Nothing. |
| Warns | **example.com has no mail DNS records yet** — 5 of 5 records for example.com are missing or point somewhere else: A example.com, MX example.com, TXT example.com.  Until they are correct, receiving servers can't verify that this server may send mail for example.com — messages are likely to be filed as spam or rejected. Create the records below at whoever manages this domain's DNS; DockPanel re-checks automatically. | Create every record shown. Each card says whether that record is published or missing, so you never have to re-do the ones already correct. |
| Warns | **2 mail record still missing for example.com** — 2 of 5 records for example.com are missing or point somewhere else: TXT dockpanel._domainkey.example.com, TXT _dmarc.example.com.  Until they are correct, receiving servers can't verify that this server may send mail for example.com — messages are likely to be filed as spam or rejected. Create the records below at whoever manages this domain's DNS; DockPanel re-checks automatically. | Create the records marked missing. The published ones need no attention. |

## Backups

### Does this backup policy send its backups anywhere off this server?

`backups.destination_configured`

A policy with no destination writes its archives to a directory on the same disk it is insuring. That protects against a bad deploy or a dropped table, and against nothing else: disk failure, a deleted instance, ransomware or a closed provider account take the backups with the data.

**When it blocks, and when it only warns.** Warning, not blocking — and this one is worth defending. Local-only backups are a legitimate choice for a box whose real protection is provider snapshots, and refusing to save the policy would be the panel overriding a decision that is not its to make. What is not legitimate is making that choice by accident and never being told.

**Where you see it**

- Backup Manager → Overview
- `GET /api/backup-orchestrator/preflight`

| | What DockPanel says | What to do |
|---|---|---|
| OK | **Backups are copied off this server** — 'Nightly' uploads to wasabi-eu. | Nothing. |
| Warns | **This policy's backup destination no longer exists** — 'Nightly' is set to upload to a destination that has since been deleted, so its backups are staying on this server. Pick a destination again, or create a new one. | Choose a destination on the policy, or re-create the one that was removed. |
| Warns | **Backups never leave this server** — 'Nightly' writes its backups to this server's own disk and nowhere else. You already have 2 destination(s) configured — choosing one means these backups survive losing this machine. | Edit the policy and pick one of the destinations you have already configured. |
| Warns | **Backups never leave this server** — 'Nightly' writes its backups to this server's own disk and nowhere else. That protects you from a bad deploy or a dropped table, but not from losing the server: disk failure, a deleted instance or a compromised host take the backups with the data.  Add an S3 or SFTP destination and select it on this policy. | Add an S3 or SFTP destination, then select it on the policy. |

### Is anything being backed up on a schedule at all?

`backups.policy_configured`

Prior to, and distinct from, the destination question — a panel with no enabled policy has nothing to send anywhere. Manual backups still work, but they only exist when somebody remembers.

**When it blocks, and when it only warns.** Warning when the server actually hosts something. A panel with no sites is not neglecting anything, so that reports as unknown rather than as a fault.

**Where you see it**

- Backup Manager → Overview
- `GET /api/backup-orchestrator/preflight`

| | What DockPanel says | What to do |
|---|---|---|
| Not checked | **Nothing to back up yet** — Once you add a site or a database, DockPanel can back it up on a schedule. | Nothing. |
| OK | **Scheduled backups are running** — 1 enabled backup policy/policies. | Nothing. |
| Warns | **Nothing is being backed up on a schedule** — This server hosts 3 site(s) and has no enabled backup policy, so nothing is backed up automatically. Manual backups still work, but they only exist when somebody remembers. | Create a backup policy. "Protect Everything" configures sites, databases and volumes in one step. |

## Field help

The quiet tier. These are the explanations under the fields themselves; where a field has a check behind it, the same concern reappears as a callout when it stops being hypothetical.

### Create your admin account

**Password** — At least 8 characters. This signs you in to the panel itself — not to any site you host with it.

This is the panel's first account and a full administrator: it can reach every site, database and container on this server. There is no password-reset email configured yet, so store it somewhere you trust before continuing. You can add two-factor authentication straight afterwards from Settings.

### Create a site

**Domain** — Your site's public domain name (e.g. example.com). It needs to point at this server before HTTPS can be issued — DockPanel checks that for you below.

You can create the site before the domain resolves; only the certificate needs DNS. If you are moving a live site, create it here first, confirm it serves correctly, and change the DNS record last — that way the switchover has no gap.

Checked by `dns.points_here`.

**Runtime**

Static serves files as they are. PHP runs them through PHP-FPM. Node and Python get a managed systemd process with nginx in front. Reverse Proxy is the one to pick when something else already listens on a port — a Docker app, or a service you run yourself — and DockPanel then owns only the front end and the certificate.

**Admin Password** — Leave blank and DockPanel generates one, then stores it in this site's Secrets vault — it is not shown again anywhere else.

Open the site, then Secrets, to read it back. Earlier versions generated this password, used it and discarded it, which left you unable to log in to the site you had just created.

**Proxy Port** — The local port your application listens on.

The port as seen from the server itself, not a public one. nginx connects to it on 127.0.0.1; nothing needs to be opened in the firewall.

### Deploy a Docker app

**Container Name** — Names the container on this server. Must be unique — DockPanel suggests a free one if it isn't.

The container is created as `dockpanel-app-<name>`, so this name is what you will see in `docker ps` minus that prefix.

Checked by `apps.name_available`.

**Host Port** — The port on this server the app will answer on. DockPanel checks it is free before the image is pulled.

Published on 127.0.0.1 by default. To expose the app publicly, put a Reverse Proxy site in front of it — that gets you a certificate and a domain instead of a port number.

Checked by `apps.port_available`.

**Memory (MB)** — Hard ceiling for this container. Leave blank for no limit.

A limit above what the server has is accepted but unreachable — the box swaps first. DockPanel warns rather than refuses, because over-committing is ordinary and you may know what is about to be freed.

Checked by `apps.resource_headroom`.

### Backup policy

**Destination** — Where a copy is uploaded after each backup. Leave empty and backups stay on this server's own disk.

A backup on the disk it protects covers a bad deploy or a dropped table, and nothing worse. Destinations are S3-compatible storage or any SFTP server. SFTP destinations that authenticate by PASSWORD need sshpass on the server the backup runs from: fresh installs and servers added through the agent installer get it automatically, but a panel upgraded in place does not, because update.sh upgrades binaries and installs no packages. When it is missing, Test Connection now says so in those words and Settings can install it onto that server for you — you are no longer asked to run a package manager by hand. Key-authenticated SFTP and every S3 destination are unaffected.

Checked by `backups.destination_configured`.

### Mail domain → DNS

**Required DNS Records**

These are the records DockPanel publishes for you when it manages the zone, and the ones to create by hand when it doesn't. Verify DNS checks that each points at this server rather than merely that something exists — a domain whose MX belongs to another provider is reported as such, not as a pass.

Checked by `mail.dns_published`.

