# How DockPanel Is Tested

> **Reflects v2.50.0.** The version stamp, the template count and every
> assertion total on this page are checked against the source by
> `tests/docs-claims-pin-e2e.sh`, so this page cannot quietly fall behind the
> code it describes.

Most projects describe their testing as a list of things that pass. This page is
mostly a list of things that failed — because that is the part that tells you
whether the testing is real.

DockPanel is tested in three layers: unit tests and audits on every commit,
regression pins that hold each past defect down, and — the layer this page is
really about — **behavioural drills on throwaway servers, where a journey is
driven all the way to the point where a user would get value from it.**

That last layer is the one that finds things. Every headline defect below was
invisible to source review and invisible to a green test suite. Several of them
had been broken on *every install ever made*.

## The method

Before each release, and whenever install-time behaviour changes:

1. **Deploy a throwaway VPS** — a real cloud server, stock distro image, nothing
   pre-installed. Not a container, not a VM on a developer's laptop with its
   firewall down.
2. **Point a real subdomain at it** and install with a domain, so the box gets a
   **real Let's Encrypt certificate** through the same ACME path a user's box
   takes.
3. **Drive the journey as a user would** — through the panel's own API and UI,
   not by calling internal functions.
4. **Drive it to the payoff step.** Creating a mailbox is not the journey;
   reading mail is. Taking a backup is not the journey; getting your content
   back is. Starting a service is not the journey; a message arriving at
   somebody else's server is.
5. **Fix, redeploy onto the same box, and reproduce the fix** — before and
   after, same machine, same command.
6. **Destroy the box and delete the DNS records.**

Step 4 is the whole method. Four consecutive audits found the same shape of
defect: a feature whose setup half worked perfectly and whose payoff half had
never once run. That base rate is itself the finding — so the working assumption
now is that **any vertical whose payoff step has never been driven is broken
until proven otherwise.**

## What driving it has found

### The mail server installer had never completed — v2.36.0

Two throwaway servers, two real domains, two real certificates, and a question
nobody had asked of this product before: does a message actually travel?

`POST /api/mail/install` returned **500 on every install ever made**:

```
{"error":"Failed to write opendkim.conf: Read-only file system (os error 30)"}
```

The agent runs under `ProtectSystem=strict`. Its `ReadWritePaths` list was not
careless — it named `/etc/postfix`, `/etc/dovecot`, `/var/spool/postfix`,
`/var/vmail`, `/etc/dockpanel`, every mail path the installer touches except the
one that is not under a directory: `/etc/opendkim.conf`, a bare file at the top
of `/etc`. The installer died at that line, so the DKIM key tables, the milter
socket directory and the service enable never ran.

Three things hid it for as long as they did:

- `apt`'s post-install scripts start Postfix, Dovecot and OpenDKIM anyway, so
  the box looked healthy.
- The panel's own `mail_status` asked only *are the packages installed and the
  services up* — which `apt` had made true for free — and reported
  `installed: true, running: true` on a box whose installer had just returned
  500.
- Postfix's `milter_default_action = accept` means an unreachable milter is
  logged as a warning and the mail is delivered regardless. The failure was
  silent by design.

**So no outgoing message had ever been DKIM-signed.** The key was generated
correctly, published as a real DKIM TXT record, and verified green by the
panel's own DNS check — which was deliberately strict, and correct. It answers
*"is the key published?"* while the operator reads it as *"is DKIM working?"*,
and the gap between those two questions is the entire signing path.

The proof, before and after, on the same box with the same command:

```
BEFORE  DKIM-Signature: (absent)     Authentication-Results: (absent)
AFTER   DKIM-Signature: v=1; a=rsa-sha256; d=…; s=dockpanel; …
        Authentication-Results: dkim=pass (2048-bit key)
```

And the receiving server's spam filter — the product's own Rspamd, judging the
product's own mail, live at delivery rather than on a re-scan:

```
BEFORE  [9.74/15] add header   HFILTER_HELO_5(3.00)  HFILTER_HOSTNAME_UNKNOWN(2.50)
                               MID_RHS_NOT_FQDN(0.50)  R_DKIM_NA(0.00)
AFTER   [1.59/15] no action    R_DKIM_ALLOW(-0.20)  R_SPF_ALLOW(-0.20)
                               DMARC_POLICY_ALLOW(-0.50)  MID_RHS_MATCH_FROM
```

The same drill found five more: the mail ports were never opened in the firewall
(Postfix listening on :25 behind a UFW allowing only 22/80/443); Postfix's HELO
name was never set, costing six spam points before a byte of content was read;
**re-running the installer erased every hosted domain, mailbox and password** —
and since the install returned 500, re-running it was the ordinary thing to do;
Dovecot demanded TLS but was never handed the Let's Encrypt certificate the box
already held, so webmail could never log in; and the panel's own
`frame-ancestors 'none'` applied to the webmail it installs.

Two defects in the *fix* survived review and were caught only by sending real
mail again. One was visible at "does the service start?" — a systemd flag
mismatch that timed the start out. The other was not: setting `myhostname`
without narrowing `mydestination` made Postfix treat a hosted domain as local
and bounce real mailboxes as "unknown user", with the daemon perfectly healthy
throughout. A fix verified only to the depth of "the process is running" has
been verified to exactly the depth that produced the original bug.

### The four distros installed, and still could not be reached — v2.38.0

v2.37.0 fixed the RPM install and proved it with `/api/health` answering on all
four families. The next session drove the same install from a **browser** rather
than from inside the box, and found it unusable — for two reasons, neither of
which the previous check could see:

| | |
|---|---|
| The installer printed | `DockPanel installed successfully!` and `Panel URL: https://…` |
| What was true | no certificate, unreachable from any browser, **502 even from the box itself** |

**Two firewalls, and the installer configured the one that was not enforcing.**
Rocky, AlmaLinux, CentOS Stream and Fedora all boot with **firewalld** running
and only SSH allowed. `setup.sh` installed UFW from EPEL next to it, enabled it,
and opened 80/443 **in UFW**. firewalld went on dropping both. Let's Encrypt
therefore could not fetch `/.well-known/acme-challenge/`, so there was no
certificate either — and the failure hint blamed a Cloudflare proxy that was not
in the path. Opening those two ports in firewalld, changing nothing else, made
the box answer from the internet and let the installer's own certbot command
succeed on the next try.

**SELinux blocked nginx from reaching the panel API.** With SELinux Enforcing —
the default on all four — `httpd_can_network_connect` is off, so nginx may not
open a socket to `127.0.0.1:3080`. Every request returned 502, including from
the box itself. The denial is `dontaudit`-ed: nothing in the journal, nothing in
`ausearch`. `setsebool -P httpd_can_network_connect on`, alone, turned 502 into
200.

That second one explains why the previous session read green. It measured
`127.0.0.1:3080`, which is the API directly — **the path a browser takes, through
nginx, was never on the evidence.** A check can be honest, repeatable and still
be measuring a different thing than the one the reader cares about.

The same session found the panel misreporting the box it was running on:
`is_installed()` shelled out to `dpkg`, and there is no dpkg on an RPM system, so
**every package read as absent** — the Services page offered to install PHP and
Fail2Ban while both were installed and running. There were four hand-rolled
copies of that function, which is how it stayed wrong in all of them. Firewall
state had the same shape: the Security page ran `ufw status`, so a firewalled box
reported no firewall at all.

Fixed in v2.38.0: the installer detects the firewall the box is already
enforcing with and configures **that** one (never installing a second),
sets the SELinux boolean up front, and `update.sh` repairs both on installs that
already exist — necessary because a broken box cannot be fixed from a panel it
cannot reach. Package and firewall queries moved behind one implementation each
that dispatches on the real system. Optional-service installers that are still
Debian-only now say so, with the remedy, instead of failing with
`Failed to find executable apt-get`.

Verified on a fresh Rocky 9.8 box from the published release: `200` over a real
Let's Encrypt certificate from outside, `ssl_verify_result=0`, firewalld holding
80/443, no UFW installed, and the Services page reporting PHP and Fail2Ban as
installed and running.

### Four supported distros that could not install — v2.37.0

The README, the docs and the website all said DockPanel ran on CentOS 9+,
Rocky 9+, Fedora 39+ and Amazon Linux 2023. The release smoke-test matrix
contained Debian and Ubuntu images and nothing else. So the RPM half of the
support claim rested on no evidence whatsoever — and when four throwaway boxes
were finally used to check, **all four failed to install**, at three unrelated
places:

| Distro | Failed at | Cause |
|---|---|---|
| Rocky 9.8 | Docker, step 3 of 15 | `download.docker.com/linux/rocky/9/` exists and serves valid metadata, but upstream fills it with `containerd.io` and the plugins only. No `docker-ce`. The install died on `Unable to find a match: docker-ce docker-ce-cli`. |
| AlmaLinux 9.8 | Docker | `get.docker.com` has no `almalinux` branch at all — `ERROR: Unsupported distribution 'almalinux'` — even though the installer greets it by name with "Detected: AlmaLinux 9.8". |
| CentOS Stream 9 | Nginx | The step that comments out RHEL's default server block used a `sed` range ending at the first `}`. Inside a server block that brace belongs to a nested `location`, so the block was half-commented and the remainder landed at `http` level: `"location" directive is not allowed here in /etc/nginx/nginx.conf:52`. |
| Fedora 43 | Systemd services | The agent unit listed `/etc/apt` in `ReadWritePaths`. That path does not exist on an RPM box, systemd refuses to build the mount namespace when any entry is missing, and the agent could not start **at all**. |

The last one is the most instructive. `/etc/apt` was not a typo: `setup.sh` and
`update.sh` each keep a hand-written list of directories to pre-create, both
commented "pre-create everything the canonical unit lists", and neither had
`/etc/apt` — because on Debian and Ubuntu it already exists, so the omission was
invisible on every box anyone had ever tested. Three lists kept in sync from
memory, and the one entry that mattered was in none of the copies.

Fixed in v2.37.0 by pointing the RHEL rebuilds at Docker's `centos` repository
(the packages there are plain `el$releasever` builds), counting braces instead
of guessing at the block end, and marking distro-specific sandbox paths optional
with systemd's `-` prefix so a missing directory can never again make the agent
unstartable. Verified the same way it was found: the same five boxes, before and
after, `/api/health` answering `2.36.0` on Rocky, AlmaLinux, CentOS Stream and
Fedora where the installer had previously aborted.

Amazon Linux 2023 was **removed from the support claim** rather than fixed:
`get.docker.com` has no `amzn` branch, and there is no image available to verify
a fix against. AlmaLinux, which the installer had recognised all along without
being able to install, took its place — and this time the claim has a box behind
it.

### Git deploy had never built — v2.35.0

Deploying a site from a real git repository failed 1.4 seconds in:

```
Build failed: docker build failed:
  ERROR: mkdir /root/.docker: read-only file system
```

The clone succeeded. The build had never run once, on any install. Three writes
in the build path all landed outside the agent's sandbox: `docker build` creating
`$HOME/.docker`; the no-Dockerfile fallback installing itself into
`/usr/local/bin`; and its cache pointing at a directory never in
`ReadWritePaths`. The fallback could not rescue the primary path because it was
broken by the same constraint.

Fixed in the shared command helpers rather than at the failing call site — there
are ~77 docker invocations sharing those helpers, and patching one would have
left 76 a regression away. **The sandbox was not widened**; `/root` and
`/usr/local/bin` stay read-only. Before and after on one box:
`failed 1409ms` → `success 169162ms`, container up, serving 200.

The same session found that `update.sh`'s agent health check probed an
authenticated endpoint without credentials, so it printed
`Agent connectivity check failed` on every update ever run, whether the agent was
healthy or dead. A check that fails identically in both states carries no signal
— it only teaches operators to ignore a warning that would matter if it ever
became real.

### A backup that did not bring your content back — v2.34.0

A site backup was `tar czf` over the document root and nothing else. Driven to
the payoff step: a file marker and a published WordPress post were both deleted,
then the backup restored. The file came back. **The post did not** — and the
panel reported the restore as a success. The documentation claimed the database
was included, twice, and printed a sample transcript of a restore step no code
had ever performed.

Fixing it exposed a second defect that only driving the fix could reach. The
first end-to-end restore still failed, because the restore path ran `mysql`
inside a container running `mariadb:11`, which does not ship the mysql-named
client. **Restoring a MySQL or MariaDB database had never worked on any
install** — not from the Databases page, not from the orchestrator, not from a
scheduled restore. The *dump* half was correct, and every sibling call site
already used the right client; the restore path was the lone outlier.

After: `databases_included: 1`, and `POST ROWS BACK: 1`.

### The mailbox nobody could open — v2.33.0

`/etc/dovecot/users` was written `0600 root:root`. Dovecot's auth worker drops
to the `dovecot` user and cannot read it, so **every IMAP, POP3 and submission
login failed on every install** — while the panel reported each account created
successfully. The only evidence anywhere was in Dovecot's own log.

The same box showed auto-sleep stopping a container that was serving 200s every
16 seconds — the next request got a 502 and nothing woke it — because nothing
recorded visitor activity, only panel activity. And enabling auto-sleep on a
default install did nothing at all: it is a step of the auto-healer, which is off
by default and configured on a different page. The toggle stored the setting,
answered `{"ok": true}`, and was never acted upon.

### The update path — v2.35.0

The drills above only reach existing installs through `update.sh`, so that got
driven too: a published older release installed on a clean box with a live
WordPress site, then updated in place to current.

It works. 96 → 97 migrations, every row intact, both real certificates still
serving, the panel never went dark, and the rollback safety net was neither
needed nor triggered. The previous session's backup fix was confirmed to
*arrive* through the update — a backup taken after it reported
`databases_included: 1` while the pre-update row correctly stayed `0/0` — and a
full restore drill on the upgraded box brought the deleted post back.

## What is still broken

This section is the reason the rest of the page is worth reading.

- **The mail server still refuses to install on the RHEL family.** Its packages
  resolve, but the configuration half — Dovecot's layout, the OpenDKIM socket
  directory, `/etc/dovecot/users` ownership — has never been driven there, and a
  mail stack running with the wrong configuration reports itself healthy and
  delivers nothing. Refusing is the honest state until it is driven.
- **The remaining verticals above install are still apt-only evidence on the RPM
  family.** Backups, git deploy, DNS and PHP pools have been driven end to end on
  Debian/Ubuntu only. Optional-service *installation* is no longer in this list:
  as of v2.40.0 Redis, Node.js, PowerDNS, the WAF, Cloudflare Tunnel, Composer,
  Fail2Ban and PHP extensions were each installed from the panel on Rocky 9.8.

- **The webmail message list can render empty.** Login, delivery and IMAP all
  work, and the mailbox demonstrably holds mail, but a frame is navigated to the
  panel root where the stricter policy refuses it, and the resulting error aborts
  the list request before it is made. **Not root-caused, so not fixed.** Serving
  webmail on its own hostname avoids it; that is the likely durable fix.
- **`HFILTER_HOSTNAME_UNKNOWN` (2.50 spam points) survives the mail fixes.** It
  is the absence of a reverse-DNS PTR record for the sending IP, which only the
  hosting provider can set. Documented in the
  [email guide](guides/email.md); a panel-side warning when the PTR is missing
  would be the real closure.
- **The mail vertical has not been re-driven against the published v2.36.0.**
  The fixes were proven with a locally-built agent deployed onto the two boxes,
  which were then destroyed. A fresh install from the published release
  re-running the whole mail journey is the missing confirmation.
- **The CLI cannot include or restore databases.** It authenticates to the agent,
  which cannot see the panel's database records. v2.34.0 made it say so and
  refuse, rather than restore the files and report success. Giving the CLI its
  own panel API client is the real fix.
- **Multi-service app templates do not exist to test.** A single-container
  template definition cannot express Supabase, Mastodon or PeerTube. That is a
  build item, not a coverage gap.
- **Amazon Linux 2023 is no longer claimed.** `get.docker.com` has no `amzn`
  branch, so the installer cannot provision Docker there, and no image is
  available to us to verify a fix against. Withdrawing the claim was the honest
  option; building and verifying real support is the open item.
- **The RPM families are verified for install, not yet for the verticals.** The
  four boxes were driven to a healthy panel — services up, `/api/health`
  answering — and destroyed. Mail, backups, git deploy and the rest have been
  driven on Debian and Ubuntu only, so anything package-manager-shaped in those
  paths is still unproven on RPM. That is now the largest known blind spot.

## The coverage ledger

Journeys are recorded as driven so the next audit extends the map instead of
repeating it. A journey that needs resources or an external account we did not
have is marked not-driven rather than skipped quietly.

| Journey | Result |
|---|---|
| Install without a domain, self-signed TLS | pass |
| Install with a domain, real Let's Encrypt certificate | pass |
| `BASE_URL` written · `:80`→301→`:443` · `Secure` cookie · TLS 1.0/1.1 refused | pass |
| First run: setup signs you in, the checklist is honest | pass |
| WordPress site, then a real wp-admin login with the generated password | pass |
| A site whose auto-SSL fails stays usable, and promotes itself when the cert lands | pass |
| Install from the published release on a second clean box | pass |
| Docker app from the template catalogue, over a panel-issued certificate | pass — limits genuinely applied, not merely accepted |
| Mail: DKIM/SPF/DMARC published for real and verified | pass |
| **A real SMTP send/receive between two domains, DKIM verified on delivery** | **found the installer had never completed** |
| **Install on each RPM family we claim to support** | **found all four unable to install** — fixed in v2.37.0 |
| **Reaching an RPM-family install from a browser, not from inside the box** | **found it unreachable and 502** — fixed in v2.38.0 |
| Optional-service installers (Redis, Node.js, PowerDNS, mail, WAF) on the RPM family | not supported yet — the panel now says so explicitly |
| Rspamd scoring live mail at delivery | pass |
| Webmail login | fixed and driven; **message list still open** |
| Backup, and a restore that returns the site's content | pass — after two sessions of fixes |
| A restore driven far enough to exercise the database client | **found MySQL restore had never worked** |
| Git deploy from a real repository, to a container serving 200 | **found the build had never run** |
| Update from a published older tag to current, on a box with a live site | pass |
| A restore drill on an upgraded, not fresh, install | pass |
| Auto-sleep on a Docker app | **found it stopped a container that was serving users** |
| **Install on each RPM-family distro we claim to support** | **found all four could not install at all** — now pass on Rocky 9, AlmaLinux 9, CentOS Stream 9, Fedora 43 |
| **Install on the published minimum spec (512 MB RAM, 10 GB disk)** | pass — Debian 12, unmodified installer, both services healthy |
| A push-a-change → redeploy loop | not driven |
| Install on Amazon Linux 2023 | not driven — no image available to us, and the claim was withdrawn rather than left unverified |

## What runs on every commit and every release

The drills are the layer that finds new things. These are the layers that stop
found things from coming back.

**On every commit** (`ci.yml`, `codeql.yml`):

- **339 unit tests** across the crates — 252 in the backend, 87 in the agent.
  (The CLI crate carries none of its own today.) Re-derive rather than trust
  this line: `for c in agent backend cli; do (cd panel/$c && cargo test
  --release); done` and sum the `test result:` lines. Nothing recomputes this
  figure on a push — unlike the regression-pin total below, which
  `docs-claims-pin-e2e.sh` re-reads from each suite — so it is exactly the kind
  of number that goes quietly stale, and it had: this line still read 294 when
  the count was measured for v2.48.0.
- **`cargo audit` on all three crates, enforcing.** A real advisory fails the
  build. Two accepted, upstream-blocked items are ignored narrowly, in a
  committed config, with the reason written down.
- **A frontend audit gate** that fails on anything high or critical and waives
  only advisories listed with a written reason — because one unwaivable advisory
  had kept the job red for six releases, and a job that is always failing cannot
  report a new failure.
- TypeScript type-checking, release builds of every crate and the frontend, and
  CodeQL.

**Regression pins.** Each behavioural finding above leaves a pin suite behind
that reads the source and fails if the fix is undone — including the shapes that
are easy to undo by accident. The mail pins assert, among other things, that the
sandbox was **not** widened to include `/etc/opendkim.conf`, since widening it
would have "fixed" the bug while destroying the reason the bug was
survivable. Twenty-five suites, **732 assertions**, all green at the current commit:

| Suite | Assertions |
|---|---|
| `mail-smtp-dkim-pin-e2e.sh` | 25 |
| `mail-auth-autosleep-pin-e2e.sh` | 28 |
| `site-backup-databases-pin-e2e.sh` | 52 |
| `git-deploy-sandbox-pin-e2e.sh` | 20 |
| `ssl-correctness-pin-e2e.sh` | 37 |
| `nginx-listen-pin-e2e.sh` | 17 |
| `rpm-install-pin-e2e.sh` | 34 |
| `mail-rpm-pin-e2e.sh` | 22 |
| `cpu-metric-pin-e2e.sh` | 17 |
| `sandbox-paths-pin-e2e.sh` | 64 |
| `webmail-spam-pin-e2e.sh` | 18 |
| `registration-gates-pin-e2e.sh` | 14 |
| `settings-controls-pin-e2e.sh` | 19 |
| `auth-doors-pin-e2e.sh` | 15 |
| `nginx-headers-pin-e2e.sh` | 78 |
| `update-rollback-pin-e2e.sh` | 36 |
| `agent-error-propagation-pin-e2e.sh` | 21 |
| `backup-destinations-git-env-pin-e2e.sh` | 27 |
| `agent-sandbox-paths-pin-e2e.sh` | 10 |
| `backup-lands-pin-e2e.sh` | 31 |
| `migration-analyze-async-pin-e2e.sh` | 28 |
| `php-install-from-picker-pin-e2e.sh` | 29 |
| `provision-log-ownership-pin-e2e.sh` | 41 |
| `backup-truth-pin-e2e.sh` | 35 |
| `tier2-pin-e2e.sh` | 14 |

**On a schedule, from outside** (`live-surfaces.yml`, daily). Every layer above
runs because something changed, which is exactly why none of them could catch the
things that go wrong while nothing changes: a certificate approaching expiry, an
install one-liner that quietly starts returning a web page instead of a script, a
marketing site still serving the bundle it was built from three weeks ago, or a
measurement nobody has retaken since it stopped being true.

That last one is not hypothetical. The panel's own memory footprint was published
as `~19 MB` on three separate surfaces for four months, against a real reading of
`~49 MB`; the social card advertised a 10 MB binary for thirty-four releases; the
security page named a release thirty-seven versions old. Each was eventually
found by a person happening to read the page, which is not a mechanism.

So `tests/live-surfaces-check.sh` runs daily on a GitHub runner — deliberately
not on the server that hosts the sites, since a check running there would fetch
its own origin and never see the CDN in front of it. It asserts that:

- the advertised install one-liner still returns a shell script, byte-identical
  to the one in this repository, rather than the single-page app's fallback HTML;
- both sites answer on certificates with more than a fortnight of life left;
- the marketing site serves exactly what this commit builds — compared by hash,
  fetched from outside, because "committed", "built" and "served" are three
  different things and any two of them agreeing has twice proved nothing here;
- the published release's binaries are still the size the register claims;
- the one waived npm advisory is still unfixed inside its major version, which is
  the condition its waiver was written against;
- and no measurement or competitor figure has gone longer than its budget without
  a human confirming it.

It is not in the assertion table above because it needs the network and the world,
so its result is a statement about today rather than about this commit.

**On every release** (`release.yml`, `smoke-test.yml`):

- Static musl binaries for amd64 and arm64 — zero glibc dependency, so a distro's
  libc version cannot break the loader.
- SPDX SBOMs for all three crates, `checksums.txt`, and **keyless Sigstore
  signatures** (`.sig` + `.pem` per asset).
- **Post-publish smoke tests that download the published binaries and run
  them** on Debian 11, 12 and 13, Ubuntu 20.04, 22.04 and 24.04, Rocky 9,
  AlmaLinux 9, CentOS Stream 9, Fedora 39 and 43, and Amazon Linux 2023, plus
  arm64 — asserting static linkage and no loader errors on each. These run
  *after* the release is created, so a green release page is not the all-clear;
  the smoke matrix is. Until v2.37.0 this matrix covered only the apt distros
  while the documentation promised six families, so `docs-claims-pin-e2e.sh`
  now fails the build if a family is named on any published surface and no
  image in the matrix tests it.
- The release body is generated from `CHANGELOG.md`, and the release fails if the
  tag has no changelog entry.

## Why this page exists

A panel that manages your sites, your mail and your backups is asking for a lot
of trust, and the honest basis for that trust is not a feature list. It is
whether the people building it drive the thing to the point where it either
works or does not, and then say which.

Every defect on this page was found by DockPanel's own maintainers, before a user
reported it, by installing the software on a real server and using it. They are
published here for the same reason they were fixed.
