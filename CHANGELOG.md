# Changelog

All notable changes to DockPanel will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/).

## [Unreleased]

## [2.84.0] - 2026-08-07

### Security — every door that checks a 2FA code is now rate-limited

Signing in with two-factor enabled was limited to five attempts per five minutes,
because a six-digit code is guessable. Turning two-factor **off** was not limited
at all — and it checks the same six-digit code, against the same secret, to make
the change that removes the factor. The check runs with a one-step tolerance, so
three codes in a million are live at any instant, and nothing sat above the
handler either: the panel installs no rate-limiting middleware, and the `/api/`
block written for the panel's own vhost carries no request limit.

Anyone holding a hijacked session could therefore strip two-factor authentication
off the account by guessing at it — which is precisely what the code prompt on
that screen exists to prevent. v2.83.0 widened this by also accepting recovery
codes there, and moving the screen to **My Account** put it in front of every
role. The limiter has been lifted out of the login handler into one place and
applied to all three doors that treat a 2FA code as a credential.

### Added — an administrator can reset a user's two-factor authentication

Until now the only statement in the panel that could clear a two-factor
enrolment lived behind a code prompt, so an account that had lost its
authenticator could only be repaired by someone who could still produce a code.
For accounts enrolled before v2.83.0 there was no such person: they hold ten
recovery codes that were generated, stored, and **never displayed**, because the
block that draws them sat inside a branch the enable handler cleared in the same
breath. Both factors were gone at once, and the only route back was editing the
database by hand.

**Users** now offers *Reset 2FA* on rows that have an enrolment. It erases the
enrolment and any remaining recovery codes, signs out every session that user
holds, and is recorded in the activity log as `user.reset_2fa`.

It deliberately refuses to act on your own account — doing so would remove the
factor with no code presented at all. Two limits are documented rather than
papered over: an administrator who has lost their own authenticator needs a
second administrator, and a **sole** administrator who has lost both their
authenticator and their recovery codes still cannot be recovered through the
panel. Registering a passkey, or keeping a second administrator, prevents that.

### Added — recovery codes can be reissued, and the remaining count is shown

Recovery codes were written once at enrolment and consumed one per use, with no
way to mint more short of disabling two-factor entirely. After ten recovery
logins the fallback was silently gone, and nothing ever reported how many were
left.

**My Account** now shows how many remain, says so plainly when the answer is
none, and offers *New recovery codes* — confirmed with an authenticator code or
one of the remaining recovery codes. This is the repair for every account that
enrolled before v2.83.0: while you still have your authenticator, one click
replaces a set you were never shown with one you can read.

### Security — recovery codes are twice as wide

Recovery codes were four random bytes — **32 bits** — stored as an unsalted
SHA-256, which is a fast hash. A 32-bit space under a fast hash is exhaustible on
ordinary hardware in seconds, so anyone holding a copy of the `users` table or an
old backup could recover the plaintext codes. That is the situation two-factor
authentication exists to survive: the recovery code is supposed to still mean
something after a password hash has leaked. Since v2.83.0 a recovery code also
*disables* 2FA, which raised what a recovered code is worth.

New codes are eight bytes — 64 bits — and the field that accepts them takes both
widths, so codes already issued keep working. Reissuing a set replaces short codes
with long ones. The hash is deliberately unchanged: matching is by hash, and a
migration cannot re-hash values it cannot read, so width is the lever that works
without invalidating anybody's existing set.

### Fixed — documentation that described protections the code does not apply

The security guide stated that recovery codes are stored as Argon2 hashes. They
are stored as SHA-256 hashes; the guide now says so, and distinguishes them from
passwords, which are Argon2 with a per-user salt. It also claimed that enabling
*Enforce 2FA* prompts users to set it up at their next login — no such prompt
exists. The setting warns and does not refuse, for reasons the guide now gives.

Finishing the second step of a login after an administrator reset that account's
two-factor authentication returned a server error; it now says the enrolment is
gone and to sign in again.

## [2.83.0] - 2026-08-07

### Added — an account of any role can reach its own security settings

Two-factor enrolment, passkeys, password change, active sessions, API keys and
the personal data export all lived on the Settings page, whose navigation entry
is administrator-only. Every account that was not an administrator — `user`,
`client`, `reseller` — therefore had no entrance to any of them.

None of this was a permissions problem. Every endpoint behind those screens was
already authenticated-user-scoped and already restricted to the caller's own
rows; the capability had shipped long ago with no way in. A new **My Account**
page carries all six, visible to every role, and the Settings tab now renders the
same components rather than a second copy of them.

The one thing a non-administrator could already do was reset a password through
the public *Forgot password* form, which requires working SMTP and being logged
out. Everything else was unreachable.

### Fixed — a lost authenticator was a dead end

Recovery codes let you log in and nothing else. Turning two-factor off accepted
only a live code from the authenticator app, so an account whose device was lost
could sign in with a recovery code and then never disable, re-enrol, or repair
itself — and the panel has no administrator-side two-factor reset either. The
browser form made it unreachable a second way, discarding every non-digit and
demanding exactly six characters, while a recovery code is eight characters of
hex and could not be typed into the field at all.

Disabling now accepts a current authenticator code **or** a recovery code, using
the same check the login screen already performed, and the field admits both.
This mattered more after the change above: the button that refuses is now on
every account's own page rather than in an administrator-only corner.

### Fixed — enrolling in two-factor never showed the recovery codes

The codes were rendered inside the branch that draws the QR screen, and the
handler that receives them leaves that branch in the same breath — it sets the
account to enabled and clears the setup state, so the condition was false by the
time there was anything to show. The success message read "2FA enabled! Save your
recovery codes." and no codes were ever drawn. Every account that enrolled has
none, which matters more now that a recovery code is also what turns two-factor
off. They are now shown after enrolling; an account that enrolled earlier can
disable and re-enrol to receive a set.

### Fixed — the two-factor banner pointed at a page the warned account cannot open

All four layouts told a non-administrator "Two-factor authentication is required"
and offered a **Set Up 2FA** button linking to Settings — the one page their
navigation does not carry. The banner now links to My Account. Three of the four
buttons were also plain anchors, so following one reloaded the whole application
instead of navigating.

### Fixed — controls that only an administrator can use were shown to everyone

Inside the account tab, "Require 2FA for all users" and "Revoke every session,
panel-wide" each asserted they were administrator-only — one in a source comment,
one in its own on-screen text — with nothing enforcing it. Both call
administrator-only endpoints and so always refused, which is why neither leaked
anything and why both survived; a control that refuses is a broken promise rather
than a disclosure. Both are now behind the role check their text claimed.

The Notifications page, which every role can open, linked everyone to Settings to
configure alert channels. Alert rules are per-user data, but the screen that edits
them has not moved yet, so the link is shown only to administrators rather than
continuing to offer a door that answers with a refusal.

## [2.82.0] - 2026-08-07

### Fixed — deleting an administrator could delete every site on the server

A server belongs to whoever registered it: the local machine to the first
administrator created, and each fleet member to the administrator who added it.
That ownership is a foreign key with `ON DELETE CASCADE`, and every site carries
a foreign key to its server with `ON DELETE CASCADE` as well. The two compose.
Deleting the account therefore removed the server row, and the server row took
**every site on that machine with it** — every owner's sites, not only the
deleted account's — along with their backups, schedules, databases, cron jobs and
deploy history. The panel reported success. The files and vhosts kept serving from
disk, so nothing appeared wrong until someone opened the panel and found the
inventory gone.

Retiring the founding administrator after appointing a successor is the ordinary
reason to open that screen, so this was reachable by the intended use of the
feature. Deleting a user now hands their machines to the administrator performing
the deletion, inside one transaction, and reports which machines moved — in the
response and in the activity log. Refusing the delete instead would have been
safe for the data and would have made such an account permanently undeletable,
since nothing in the panel can re-assign a server.

### Fixed — a panel's second administrator could not see any servers

The server list was scoped to the caller's own rows for every role, so an
administrator who had not personally registered a machine was shown "No servers
found. The local server should appear automatically." It never could. An
administrator now sees every machine; everyone else continues to see only what
they hold. Seven related reads — the drift checks and the fleet aggregations on
the dashboard — still resolve machines by owner and are unchanged for now; the
limitation is recorded in the source.

### Security — transferring a site left its staging environment behind

A staging environment is a second site row pointing at its parent. Transfer moved
only the parent, so the previous owner kept the staging clone: it stayed in their
site list, they kept a shell inside a full copy of the new owner's document root —
including any configuration file holding database credentials — and they kept the
push-to-production control, which writes that copy over the new owner's live site.
Transfer now moves a site together with its staging children, and the dependent
rows follow the sites that actually moved.

### Security — the site terminal's cross-site guard missed the relative path

The guard that stops a site shell reading another site's files matched only the
absolute path, while the shell's own working directory sits inside the directory
being protected — so the relative spelling walked straight past it. Both spellings
are now refused. The guard's real scope is documented in the source alongside it:
all sites run as the same system user, so this is a barrier against the obvious
mistake, not an isolation boundary. A real boundary needs a per-site user or a
namespace, and is a separate change.

### Fixed — the panel refused people their own certificates and maintenance windows

Four endpoints required an administrator over queries that were already limited to
the caller's own rows, so the check never decided what was returned — only who was
turned away. The visible result was a dashboard tile reporting a site owner's SSL
certificates and expiry dates, linking to a page that answered "Admin access
required" over "No SSL certificates found". Site owners now see their own
certificates and can schedule maintenance windows for their own alerts.

### Fixed — a reseller could not manage accounts its own panel listed

The reseller's user table listed every account beneath them, while the actions on
those accounts required the account to still hold the default role. An account
moved to the client role — the flow the roles guide prescribes — or suspended by
an administrator stayed on screen with its site count and answered "User not
found" to Reset Password and to Delete. Resellers can now act on the accounts they
are shown; administrators and other resellers remain out of reach by an explicit
allow-list.

### Fixed — the panel told non-administrators it was broken, on every page

The sidebar health indicator polled an administrator-only endpoint and rendered the
refusal as a failure, so every non-administrator saw a pulsing red "Disconnected" /
"Issues Detected" from the moment they signed in. The indicator is now shown only
to the administrators it describes something for.

### Fixed — "Require 2FA for all users" was never shown to the users it applied to

The banner warning that two-factor authentication is mandatory read the setting
from an administrator-only endpoint and discarded the refusal, so it could only
ever appear for administrators — the one group that did not need telling. The flag
now rides on the per-user 2FA status endpoint. Enrolment still lives on a page
non-administrators cannot reach, so the panel tells them rather than blocking the
login; blocking before there is a door would lock people out with no way back.

### Fixed — clients were offered the one action their role forbids

An account with the client role holds sites and cannot bring a new domain into
service. The panel still offered Create Site, Clone, Create Staging and Add Alias,
so the refusal arrived only after filling in a domain, a runtime, a PHP version
and — for a WordPress site — an administrator username and password. Those controls
are now hidden for that role, and the empty sites list explains that sites are
handed over by an administrator instead of inviting the person to create one.

## [2.81.0] - 2026-08-07

### Fixed — resetting a MariaDB database password has never worked

The MariaDB branch of the agent's password reset connected as the container's
root account with no credential, on the stated premise that root can
authenticate over the unix socket inside the container. That premise was never
true of a container DockPanel itself creates: the database container is started
with a random root password, which gives `root@localhost` a random
`mysql_native_password` and does not enable socket authentication. Every reset
therefore failed with `ERROR 1045 … Access denied for user 'root'@'localhost'`
and surfaced as a 500. Measured on `mariadb:11` with exactly the environment
DockPanel sets; the PostgreSQL branch was unaffected.

The reset now authenticates **as the tenant** using the current password the
panel already sends, and asks the server to change its own account — the same
connection shape the SQL console has used in production all along. Two
consequences: the feature works, and the `old_password` in the request is now
load-bearing rather than transmitted and discarded. A rejected login is reported
as what it is — the panel's stored credential and the database's real one have
diverged — instead of a bare "access denied".

### Security — a WordPress auto-update toggle could erase root's entire crontab

`crontab` has no partial-update verb, so every writer reads the whole file and
writes the whole file back. `set_auto_update` read it with
`.output().await.map(|o| …stdout…).unwrap_or_default()`, which turns **both** a
spawn failure and a non-zero exit into an empty string, then piped that back as
root's complete new crontab. A single transient failure would have deleted every
scheduled job on the box — every tenant's, and every system entry — leaving at
most one WordPress line. Reachable from an ordinary authenticated request naming
a single site.

The cron routes had already grown a fail-closed reader for exactly this reason,
and its doc comment said so. It was a private helper, so the WordPress twin
never got it. There is now one shared reader and one shared writer.

### Fixed — uploading a certificate stripped a site's security config, and 502'd PHP sites

The agent's SSL routes receive `{domain, certificate, private_key}` and have to
invent the rest of the vhost, so they render one with WAF off, bot protection
off, no CSP or Permissions-Policy, no custom nginx, default rate limits — and,
because they cannot know the site's PHP version, an unversioned `php-fpm.sock`
that exists on no modern Debian or Ubuntu. v2.18.0 added a compensating full
re-render for provision, renew and force-renew. Certificate **upload** is the
fourth sibling and never got it, so uploading a custom certificate silently
disarmed a hardened site — with the panel's own toggles still reading ON, since
the database row is untouched — and took a PHP site off the air outright.

### Fixed — the backup taken before deleting a site had never captured a byte

The pre-delete snapshot ran **after** the call whose agent handler removes the
site's webroot, and `create_backup` refuses when the site root is missing. So it
returned an error every single time, and `let _ =` discarded it. It now runs
before anything destructive, includes the site's databases (which are still
alive at that point), and logs a failure instead of swallowing it.

### Fixed — a regression census could be satisfied by the next function's name

The wrong-host census carved function bodies with a window that ended at the
**successor's** position rather than at its declaration, so every body carried
the next function's `fn name(` line. Membership and compliance are both tested
against alternations of function names, so a handler inherited both from
whatever happened to be declared below it: the census reported 48 members where
there are 31, and two handlers were marked compliant on their neighbour's
resolver. Fixed in all three censuses that share the idiom. Handlers that
receive an already-resolved agent handle are now excluded explicitly, anchored
to the signature so a route handler cannot claim the exemption.

`databases.rs` is guarded by a new arm: it dispatches on container names rather
than domains, so it contributed nothing to that census — the module holding both
this release's fix and the previous one's was covered by no arm at all.

### Added

- `db-credential-auth-pin-e2e.sh` (15 assertions) and
  `sibling-parity-pin-e2e.sh` (13) — every arm mutation-tested by planting the
  defect and confirming the suite's summary line and exit code both go non-zero.


## [2.80.0] - 2026-08-07

### Security — an unattended loop let certificates expire on every fleet member

`auto_renew_ssl` selected every site in the installation and renewed through the
**panel's own agent**. On a single box those are the same machine, which is why
this survived; on a fleet the panel was asked to renew a certificate for a domain
it does not serve, HTTP-01 could not validate, and the certificate simply expired
on a live customer site — unattended, on a 120-second tick, needing no attacker.
The inverse was worse: where the local box could satisfy the challenge, the
renewal succeeded and wrote the LOCAL certificate's expiry onto the remote site's
row, pushing it out of the 45-day window so the loop stopped trying. A guaranteed
outage behind a green panel.

`auto_sleep_idle_containers` was the same shape: it asked the local nginx how
recently a member's domain was served (always "unknown", so real traffic never
counted), listed the local host's containers, and posted the stop locally.
Auto-sleep was quietly inoperative for every fleet member.

Both now resolve per row, and the auto-healer no longer receives a local-agent
handle at all — the parameter is gone, so it cannot be reached for again.

### Security — mail configuration was written to one host, for the whole fleet

`sync_mail_config` ran three SELECTs with **no WHERE clause of any kind** and
posted the result to whichever host the caller's `X-Server-Id` header named. Every
mail domain, every mailbox's `password_hash` and `forward_to`, and every alias in
the installation were written into one host's Postfix and Dovecot maps: a
credential disclosure onto a machine that should never hold them, plus a
mail-interception primitive, since that host would then accept mail for — and
authenticate mailboxes of — domains it does not host.

There was also **no caller predicate on mail at all**. `mail_domains` has no
`user_id` column, so the scope extractor's check on a request header was the
entire boundary between one administrator and another administrator's server.
Removing the extractor without adding a predicate would have widened access, so
both landed together as `MAIL_DOMAIN_CALLER_PREDICATE`.

`create_domain` never bound `server_id`, so every mail domain created through the
panel since the multi-server migration carried a NULL host. Because `/mail/sync`
**rebuilds** the maps rather than merging them, filtering by server without
repairing those rows would have deleted mailboxes from the host actually serving
them. A migration backfills them (matching against `sites.domain` first, so a
domain provisioned onto a member returns to the right host), and the rebuild
refuses outright while any NULL row remains.

`create_alias` never checked that the address belongs to the domain it is filed
under. Postfix applies a virtual alias by address, so an alias filed under a
domain you may write to could redirect mail for a different domain on the same
host — the row said one thing and the map did another, and the map is what runs.

### Security — a panic button that swept one machine

The emergency lockdown wrote panel-global state — `lockdown_state`,
`sessions_revoked_at`, registration, every terminal share — but killed terminals
on a **single caller-selected agent**, then reported `terminals_killed` and
`lockdown_active: true`. On a fleet it left live root shells on every other
member while reading as complete. It now fans out, and names the hosts it could
not sweep; an unreachable member during a panic is the loudest thing on the page.

`lockdown_state.terminals_disabled` was **write-only** — set by `activate_lockdown`
and read by nothing, anywhere. After a panic, an admin who signed back in could
immediately open a fresh root terminal on any host while lockdown was active. The
ticket mint, terminal sharing and shared-output views now honour it. (Revoking and
listing shares deliberately stay open: they are the remedy, not the risk.)

### Security — the wrong-host dispatch family, finished

`v2.79.0` fixed 71 handlers that resolved a resource from its row and then acted
through whichever host the browser named. This release closes the rest — roughly
sixty more across git deploys, databases, backup orchestration, staging, stacks,
mail, migration, logs and Docker apps — plus three shapes no census could see:

- **`backup_destinations::test_connection`** contained no scope token at all. It
  used the panel's own local client unconditionally, sending the **decrypted** S3
  or SFTP credential to the panel for a destination a member will actually use,
  and returned a green that described the panel's egress rather than the backup
  host's.
- **`databases::reset_password` and `query`** sent decrypted database passwords to
  a machine the database does not run on. The agent's reset never verifies the old
  password, so knowing a container name was the entire authorisation.
- **`git_deploys::deploy`, `rollback` and `approve_deploy`** cloned, built and
  deployed on the wrong host, injecting the deployment's full environment there.
  `approve_deploy` additionally had no ownership term: one admin could approve
  another's protected deploy.

`migration::import` wrote vhosts, claimed domains and created database containers
on the caller's selected host rather than the one holding the staged archive.
`logs::log_stats` took a domain straight from a query parameter and never resolved
a row at all.

### Fixed — DNS records named the panel instead of the host

Auto-DNS published the **panel's** public address for resources on any machine, so
a site created on a member got an A record pointing at the wrong box and was
unreachable at the name it had just been given. The delete path only removes a
record whose content matches, so a member's record outlived its site as a dangling
A record — a takeover surface rather than clutter. `servers.ip_address` was already
being kept current by agent check-in and had simply never been consulted when
publishing.

### Fixed — domain claims asked one host about Docker apps

Three of the claim's four legs were fleet-wide. The fourth — the only one that can
see a Docker app, whose domain exists solely as a container label — asked the
caller's host, so a domain held by an app on another member passed the check and
the deploy replaced that app's nginx configuration.

### Fixed — smaller consequences of the same mistake

- Container quotas are per user but were counted on one host, so an account could
  run its full limit on every server and read as within quota on each.
- The port-availability preflight counted ports held by sites **fleet-wide**, so a
  port free on this box was refused because another machine used it.
- SMTP settings reached one host; the rest silently kept whatever they last had.
- The SSH half of the login audit came from one server while the panel half was
  fleet-wide, and the two were presented as siblings — a brute-force campaign
  against a member's sshd was invisible unless that member happened to be selected.
- Cloning a site across servers created the row and consumed the quota slot before
  discovering the source docroot is on another machine. It is now refused up front.

### Changed — the regression pin that was policing this class could not fail

The suite's violation loops ran inside a pipeline, so the `FAIL` counter they
incremented belonged to a subshell that then exited: the arm printed its ✗ marks
and the suite still reported `FAIL 0` and exited 0. The one arm whose entire job
is reporting violations had been unable to fail a build since it was written.

The census itself is rebuilt. It used to require a handler to already use one of
six correct-form spellings before it would be judged, which meant whole modules
contributed zero members while holding live defects — and deleting a fixed
handler's resolver call removed it from the census entirely, so deletion read as
compliance. Membership now comes from the **schema**: any table carrying a
`server_id` names a host, and a handler that reads such a row and then acts
through an agent must take the host from the row. Two new arms cover what no
route-scoped census can see — background services holding the local handle, and
the agentless DNS shape.

### Changed — DockPanel is now free software under the AGPL v3

The licence moves from the Business Source License 1.1 to the **GNU Affero General
Public License v3**. BSL was source-available, not open source: it carried a use
restriction and a 2030 conversion date. That restriction and that date are both gone.

You may now run, read, modify and redistribute DockPanel on any number of servers,
commercially included, with no conversion date to wait for. The one obligation the
AGPL adds over the GPL is the network clause: if you modify DockPanel and offer it
to other people *as a service*, you must make your modified source available to
those users. Running it — modified or not — for yourself or your own organisation
obliges you nothing.

`LICENSE` is now the verbatim AGPL-3.0 text. Package metadata across the agent,
backend, CLI, panel frontend and website declares `AGPL-3.0-only` (the website had
been declaring ISC, which was simply wrong).

### Added — a way to support the project

`.github/FUNDING.yml`, the README and the site's price section now point at GitHub
Sponsors and PayPal. Nothing is gated behind them and nothing ever will be; the
support note sits after the price figures and says what the money buys — test
hardware, so releases are verified on more distributions before they ship.

## [2.79.0] - 2026-08-07

### Security — force-renew and revoke acted outside the administrator's boundary

`POST /api/ssl/{id}/renew` and `DELETE /api/ssl/{id}` loaded the site with
`SELECT * FROM sites WHERE id = $1` — no owner term and no server term — while
three other handlers in the same file went through the shared caller predicate.
An administrator in DockPanel is not a superuser: their reach is the local box
plus the machines they registered themselves. With that predicate missing, the
only thing holding that line was the server-scope extractor's check on a request
header, which is not an authorisation the site's row ever agreed to.

Revoke is the sharper half — it deletes the certificate through the agent and
then blanks every `ssl_*` column on the row, so naming a site outside your
boundary was a cross-boundary write even when the agent leg found nothing to
delete. Both now resolve through the same predicate as their siblings.

### Fixed — a site's host came from the browser, in seventy-one more places

v2.78.0 fixed three handlers that answered *which site* from the row and *which
host* from the server switcher. This release finishes the family: **71 handlers
across 10 route modules** now take both answers from the row, through
`site_agent_for_caller` / `agent_for_site_server`, which refuse an unreachable
host rather than substituting the local one.

The two questions agree on a single-box install, which is why this survived the
whole life of the fleet feature. On a fleet they diverge, and the consequences
ranged from dishonest to destructive:

- `logs::site_logs` and `logs::search_site_logs` asked the wrong machine about a
  domain it does not serve. Nothing comes back, and nothing is what a quiet site
  looks like — a search for an attack in a remote site's access log returned a
  confident "no matching lines".
- `secrets::inject_to_site` wrote decrypted secrets into the nginx environment of
  a machine the site does not run on. The one member of this family where a
  misdispatch is a disclosure rather than a wrong answer.
- `databases::create` built the container on the caller's host and then recorded
  it against a site living elsewhere, leaving a row naming a container that is not
  there while the wrong machine keeps the container and the port.
- `sites::rename_domain` and `sites::add_alias` ran their domain-collision check
  against one machine and wrote the vhost on another. `rename_domain`'s scope
  binding was spelled `_server_id` and was **not** unused — the underscore silenced
  the warning while the value was still being passed to the claim check.
- The rest — file manager, cron, backup, WordPress, SSL provisioning and site
  settings verbs — acted on the wrong host's copy of a domain.

Handlers that legitimately want the caller's server keep it: server-level
operations, the two streaming-ticket mints (whose scope id is the input to the
local-only guard and whose handle is a signing key), site creation and cloning
(where the server is a destination, not a lookup), and queries already pinned to
the scoped server.

### Fixed — two regression arms judged a hand-written list of subjects

`unattended-host-scope-pin-e2e.sh` §J5 iterated two handlers from a literal list,
three lines below a comment added in an earlier release warning never to do that.
It now reuses the source-derived enumeration its neighbour already computes.
Mutation-testing it surfaced something worse than a blind spot: the old arm
grepped for a specific spelling of the discarded binding, so **deleting the
parameter outright read as compliance** — it would have certified the exact
mistake this release could have made.

`wrong-host-dispatch-pin-e2e.sh` §C4 carried the comment *"Subjects derived FROM
SOURCE, not from a literal list"* directly above a hardcoded three-element array.
It judged three handlers while sixty-two call sites spelled the pattern, and
because the comment used the vocabulary of the rule, the gap read as audited. It
now censuses every site-resolving handler in every route module — 92 today — and
asserts that census is non-empty before judging it. Deriving it is what found the
last four defects above; the module-scoped search that preceded it did not see
them.

## [2.78.0] - 2026-08-06

### Fixed — deleting a file could delete the whole site

The file manager resolved a requested path, checked it was inside the site root,
and acted on it. That check asks about containment, and containment is not
identity: `.`, `""` and `/` all canonicalise to the site root itself, and a path
always starts with itself, so all three passed. A delete handed one of them ran a
recursive remove on the webroot and reported success. Destructive verbs now
resolve through a variant that refuses the root and permits everything below it;
listing the root, which is the file manager's default view, is unchanged.

This one needs no fleet and no particular role — it is reachable on an ordinary
single-server install.

### Fixed — a site's files were resolved on one host and acted on from another

Two questions had been collapsed into one. *Which site* is answered by a row the
caller owns; *which host* was answered by the server the caller had selected in
the UI. They agree on a single-box install, which is why the difference stayed
invisible. On a fleet they part company: a client owns no server row — the only
insert is administrator-gated — so no server header is sent and the local agent
is used, while their site's row may name a different machine. An operator with
two servers reaches the same state by opening a site on one while the switcher
says the other, since the single-site read is not server-scoped even though both
list reads are.

The rule was already settled in this codebase. The webhook deploy path resolves
the agent from the row and says why; the git-deploy update path does the same;
every background service that walks these tables routes per row. It was the
authenticated handlers — the buttons a person clicks — that never adopted it.
Three of the worst now do:

- **Deploy trigger** clones a repository into the site's webroot and runs its
  build script as root. It sits in the same file as the webhook path that was
  fixed for precisely this reason, and was left as it was.
- **Site delete** aimed a site's database containers, its Redis index, its
  firewall rule and its webroot at the selected host. The firewall step is the
  one that is not namespaced by domain: it matched on port and action alone, so
  it could remove an unrelated rule on the wrong machine.
- **Stack delete** deleted its database record whatever the agent replied. Aimed
  at the wrong host the removal found nothing, reported success, dropped the
  record, and left the real host's containers and vhost running with nothing
  naming them.

A host that cannot be resolved is now refused rather than quietly replaced with
the panel's own, because substituting is how one tenant's files end up on another
tenant's machine.

### Fixed — listing a folder could create it

The list verb created the site root before resolving. Asked for a domain the host
does not serve, it made `/var/www/<domain>` as root and answered `200` with an
empty array — which then let write, create and upload land in a directory no
vhost serves. Every other verb already failed loudly. That silence was the only
reason a misdirected request could go unnoticed, and it is gone. A site whose
document root was pointed somewhere other than `/var/www/<domain>` now says so
instead of showing an empty invented folder.

### Added

- `wrong-host-dispatch-pin-e2e.sh` — 18 assertions. Thirteen are red at v2.77.0;
  five are green at both tags on purpose, so a harness that measured nothing
  could not read as a pass.

## [2.77.0] - 2026-08-06

### Fixed — a client's dashboard never drew, and the menus pointed the wrong way

Reported on #51 by the operator the `client` role was built for: *"when i log in as
a client .. i can see the sites, databases, but cant see the mail domain … The
Dashboard also have no stats … when i click terminal the client cant access their
section … Also file manager not available for their sites."*

Four complaints, and they were not four bugs. Two were real, one was a page that
already worked, and one was a feature that has never existed.

**The dashboard showed a loading skeleton for ever.** The host `system` read is
admin-only — `fetchRealtimeData` returns early for anybody else, and the live
metrics socket refuses a non-admin — and the entire dashboard body was rendered
behind `{!system ? skeleton : body}`. So a client got six pulsing placeholder
cards and nothing underneath, permanently. Everything below that gate now renders
for every role, with the host-level pieces carrying their own guards.

Unhiding it exposed a second problem worth naming separately: four cells in the
status grid did not merely go blank for a client, they made **affirmative false
statements** — "up to date" from an update count that was never fetched, a mail
queue of "0", bandwidth of "0 B / 0 B", "0/0 running" containers. A blank is
honest; a zero is a claim. Those four are now admin-only, along with the setup
checklist (four of its five steps land on pages a client cannot open) and the
eight widget checkboxes that toggled panels a client can never draw.

**The Terminal advertised the one shell a client cannot have, and hid the one it
has.** A site owner has had a shell on every site they own for some time: it opens
inside `/var/www/<domain>` as `www-data`, under a restricted shell with no
privilege escalation. But the Terminal page auto-connected to the **server** shell
on arrival, which is administrator-only by design — so a client reached the page
from the sidebar and was refused instantly. The page now opens the caller's own
first site when they are not an admin, the "Server root" option and the root SSH
panel are admin-only, the snippet bar offers commands that work in the session you
are actually in, and the refusal names the shell that does work instead of ending
the sentence.

⚠ The server-shell restriction itself is **unchanged and deliberate** — it is the
v2.75.0 root-shell fix, and this release pins it with four regression arms that
must stay green at both tags.

**One menu-visibility rule, in one place.** The sidebar's role filter lived inline
in a single hook, so the command palette — a second menu over the same pages — had
no filter at all and offered `/users`, `/secrets`, `/security` and `/settings` to
every account via Ctrl+K. Every one of those pages guards itself, so nothing
leaked; what the palette handed out was a list of doors that refuse. Both menus now
read one exported predicate over one registry. The dashboard's "Deploy App"
shortcut had the same shape and is gated too — its sibling "Add Site" link, three
lines below in the same file, had already been fixed for exactly this reason.

**Not defects, stated plainly:** the file manager was never gated — all eight
handlers take the ownership path and it is the first card on a site's page, in the
reporter's own version. And DockPanel has no FTP, has never claimed one, and has no
per-site system user to hang one on; the file manager and the per-site shell are
the two ways in.

### Security — the dashboard endpoint answered host-wide to every role

Four reads in `dashboard.rs` were scoped by server but not by caller, so a
non-admin received host facts through a route that looked tenant-shaped:

- `docker_summary` made the very agent `/apps` call that `docker_apps::list_apps`
  guards with `require_admin`, and guarded nothing — the ungated door to the list
  next door.
- `intelligence` forwarded the agent's `/diagnostics` blob verbatim, although that
  endpoint is `AdminUser` on its own route.
- the stale-backup count counted **every tenant's** sites on the box.
- the host security-scan critical/warning totals reached every caller (that table
  has no user column to filter by, so a non-admin now simply does not get one).

Sections 1–4 of that handler were already `user_id`-scoped for everyone; these four
were the exceptions. The scope is now decided once, at the top of the handler.

### Documentation

`docs/guides/roles-and-ownership.md` promised a client seven capabilities and two
of them — **mail** and **containers** — are administrator-only and always were
(`routes/mail.rs` is `AdminUser` on 42 of 42 handlers; every `docker_apps` handler
calls `require_admin`). The guide is corrected, the client's per-site shell is
documented for the first time, and a Withdrawn Claims row records it. The same
sentence also survives in the `client` role migration's own comment, which
**cannot** be corrected — `sqlx::migrate!` checksums applied migrations, so editing
it would break the upgrade on every deployed install. It is left in place
deliberately and recorded in `FEATURES.md` instead.

New regression suite `tests/client-role-honesty-pin-e2e.sh` — 28 assertions,
green at this commit and **red on 24 of them at v2.76.0**; the four §E arms are
context guards that are green at both tags by design.

## [2.76.0] - 2026-08-06

### Security — a non-admin who owned one site could read the host's system logs

The twin of v2.75.0's terminal escalation, in the log streamer. The panel signs a
`log_stream` ticket that authorises one site's logs, resolving the site's domain
under an ownership check — but it returned that domain as a field *beside* the
signed ticket, and the agent read both the domain (from `?domain=`) and the log
type (from `?type=`) out of the browser-controlled query string. Asking for
`type=auth` with the domain omitted streamed `/var/log/auth.log`; `type=syslog`
streamed `/var/log/syslog`; any type outside `access`/`error` fell through the
agent's pass-through arm straight into the system-log allow-list.

So any non-admin who owned a single site could mint a legitimate, ownership-checked
log ticket for that site, then open the stream asking for a system log and read the
host's SSH authentication log — every login, with source IPs — or the full syslog.
The panel's "admin required for system log streaming" check only fires at mint time
for a *site-less* ticket; it never travelled to the agent.

Reproduced end to end on a fresh box against the published v2.75.0: a `user`-role
account owning one site streamed `/var/log/auth.log` and `/var/log/syslog` with the
domain omitted. The scope now rides **inside** the signed ticket — a site's domain,
or the `@system` marker for the admin-only host logs — exactly as the terminal
ticket's scope does since v2.75.0. The agent takes both the domain and the
permitted log types from the signed scope: a site ticket streams only that site's
own `access`/`error`/`php` logs and is refused (`403`) for anything else, and a
lying `?domain=` for a different tenant is ignored. Re-driven fixed on the same
box: the system-log requests are refused, a lying domain resolves to the ticket's
own site, the site owner still streams its own logs, and admin system streaming
still works. Legacy tickets from a pre-fix panel fall back to the old behaviour and
log a warning, so an agent upgraded ahead of its panel keeps working.

This needs the fix on both the panel and the agent. A single-box install updates
both together; on a multi-server panel each managed server's agent must be updated
to close it there.

New pin: `tests/logs-scope-signed-pin-e2e.sh`, 10 assertions, each mutation-tested
against a widening; PASS 3 / FAIL 7 at v2.75.0.

## [2.75.0] - 2026-08-06

### Security — a non-admin who owned one site could open a root shell on the host

The terminal ticket the panel signs authorises a specific shell — a site's own
directory, dropped to that site's `www-data`, or the admin root shell. But the
panel returned the shell's scope (the site domain) as a field *beside* the signed
ticket, and the agent read it from the `?domain=` query string. An empty domain
there opens a **root** shell in `/root` with no privilege drop.

So any non-admin who owned a single site could mint a legitimate, ownership-checked
terminal ticket for that site, then open the WebSocket with the domain omitted and
get an unrestricted root shell on the machine. The panel's "admin required" check
only guards minting a ticket with *no* site; it never travelled to the agent, and
the browser controls that query parameter.

Reproduced end to end on a fresh box against the published v2.73.0: a `user`-role
account owning one site got `uid=33(www-data)` with the domain present and
`uid=0(root)` with it omitted. The scope now rides **inside** the signed ticket —
the same place the recording flag already rides, and for the same reason — so the
ownership check performed when the ticket is minted governs the shell that opens.
Re-driven fixed on the same box: the domain-omitted connection now drops to
`www-data`, the admin server shell still works, and a lying domain parameter is
ignored.

This needs the fix on both the panel and the agent. A single-box install updates
both together; on a multi-server panel each managed server's agent must be updated
to close it there.

New pin: `tests/terminal-scope-signed-pin-e2e.sh`, 9 assertions, each
mutation-tested against a widening; PASS 2 / FAIL 7 at v2.73.0.

## [2.74.0] - 2026-08-06

### Fixed — disabling a site did not take it offline, and where it did, the site came back by itself

Two defects, both in the same place, found in that order.

**Disabling a site did nothing at all on most installs.** The maintenance
response written in the site's place named `listen 80`, while the vhost
templates bind the server's own address — `listen 203.0.113.10:80` — whenever
the panel knows it, which is the ordinary case. nginx attaches each server block
to the socket its own directives name and picks a name among the blocks on the
socket the request actually arrived on, so the maintenance block sat on the
wildcard socket and was never consulted. The site went on serving its content,
the panel displayed it as Disabled, and the agent reported success. Driven on a
fresh box against the previous release: a disabled site answered `200` with its
own page; giving the maintenance block the site's own address, and changing
nothing else, turned the same request into `503 Site Disabled`. The response now
takes its listeners — and its certificate, so a site with TLS can say it is
disabled over HTTPS — from the configuration being replaced.

**And where the maintenance response did work, the site came back on its own.**
Disabling does not remove a site's nginx configuration. It parks the real body
beside the live one and leaves a maintenance response in its place, so nginx
keeps answering on the name while serving nothing.

Five separate places rendered a complete vhost, and every one of them wrote to
the live path without asking whether the operator had taken the site offline. So
an ordinary settings change — PHP version, WAF mode, CSP, cache toggle, custom
nginx, a git deploy, exposing a container, provisioning a certificate — replaced
the maintenance response with a working site and reloaded nginx. The site went
back on the internet immediately, and the panel went on displaying it as
Disabled, because `sites.enabled` had exactly one reader in the whole backend:
the guard inside the toggle that writes it.

The unattended certificate-renewal loop did the same thing with nobody watching.
On a stock install that is the only automatic renewal there is, and it rebuilds
the vhost after every renewal — so a deliberately offline site with a certificate
came back within about a week of that certificate entering its renewal window,
and again on every sweep after that. With auto-heal switched on, within two
minutes.

Where a rendered vhost goes is now one decision in one place, and every writer
asks it. A write for a disabled site refreshes the parked copy instead, and says
so rather than reporting a reload that did not happen.

The same change repairs a second defect. Re-enabling used to restore the body
frozen at the moment the site went offline, silently reverting every setting
changed in between — and a site that gained a certificate while offline came back
on plain HTTP, because the frozen body has no TLS listener. Keeping the parked
copy current means what comes back is the site as it is now. Renaming a disabled
site also carries its parked body to the new name; previously it was stranded
under the old one, where enabling would never look, leaving the site impossible
to bring back from the panel at all.

The panel side declines too: neither unattended loop pushes a vhost for a
disabled site. That half arrives with the panel, so it protects managed servers
before their agents are updated.

- Disabled sites in the interface: the site detail header showed `active` for a
  disabled site, and the dashboard counted it among the active ones and drew it
  with a live dot. All three now agree with the banner that says the site is
  disabled.
- Password-authenticated SFTP backup destinations need `sshpass`, which only the
  panel installer installed. Servers added with `install-agent.sh` now get it
  too, and the prerequisite is documented for panels upgraded in place, since
  `update.sh` upgrades binaries and installs no packages. (#93, partial)
- Sixteen of the thirty-three regression-pin suites had no CI job and were run
  only by hand. Every suite now runs on every push.

New pin: `tests/site-disabled-stays-offline-pin-e2e.sh`, 19 assertions, each
mutation-tested against a widening.

## [2.73.0] - 2026-08-06

### Fixed — a suspended account came back with the wrong role, and could arrange it itself

Suspending an account overwrites `users.role`, which is the only record of what
the account was, so the previous role had to be kept somewhere until the account
was un-suspended. It was kept in `users.reset_token` — the same column the public
password-reset flow writes.

That flow checked no role at all. It was the one door in the whole authentication
surface that didn't: sign-in, the session middleware, two-factor, passkeys and
OAuth all refuse a suspended account explicitly. So a suspended user could open
the ordinary *Forgot password* form, and the request alone replaced the record of
their role with a reset token — no administrator involved, no sign-in required,
and the reset did not even have to be completed. Whether the token was left in
place or cleared by finishing the reset, the un-suspend that followed found
nothing usable and fell back to `user`.

The visible result was an administrator pressing **Unsuspend**, intending to put
an account back as it was, and promoting or demoting it instead:

- a `client` came back as a `user` — and a `user` may bring a **new domain** into
  service, which is the single capability the `client` role exists to withhold;
- an `admin` or a `reseller` came back as a plain `user`, losing everything.

The second of those is much older than the first and needs no `client` role
present, so it is the one more likely to have already happened on a running
install.

**What changed.** The suspend stash has a column of its own, `users.prior_role`,
which nothing else writes. Suspending records the previous role and setting it
back reads that record; the password-reset column is left to the password-reset
flow. Both places that suspend an account — the panel and the WHMCS billing
webhook — now go through one pair of statements rather than keeping a copy each.

**An un-suspend never guesses.** If the previous role is unknown, the account
stays suspended and says so, rather than being handed some default. There is no
ordering of these roles that makes a guess safe in either direction: `user` is
the value that caused this bug, and `client` is not simply "less" than `user` —
reseller management is scoped to `role = 'user'`, so an account handed `client`
would still be listed by its reseller and no longer be manageable by them. The
operator is told instead, and can set the role from the user editor, which also
lifts the suspension.

Five smaller corrections came with it:

- **The billing webhook records a previous role too, and gives back what it
  recorded** instead of a hardcoded `user`. Its deny-list (refusing to *suspend*
  an `admin` or `reseller`) is kept — it guards a different hazard, a lapsed
  invoice locking an operator out of their own panel.
- **Billing can return an ordinary account to service, but never restores an
  `admin` or `reseller`.** That direction previously had no guard at all, so an
  account the *panel* had deliberately suspended could be handed its operator
  role back by anyone holding the webhook secret, leaving no audit record.
- **Suspending an account now cuts its sessions on every path.** The session
  check refuses a token whose claim says `suspended`, and a token issued while
  the account was a `user` keeps saying `user` until it expires — so a
  billing-driven suspension previously did nothing at all for up to two hours.
  The panel had always revoked; the webhook never had.
- **Billing-driven *un*-suspension now honours the `auto_suspend` setting**, as
  suspension and termination already did, and **all three settings finally have
  controls** on the WHMCS integration screen. They were stored and returned by the
  API but had no checkbox, and the screen's own save omitted them — which reset
  them to their defaults every time it was used.
- **The password-reset endpoints refuse a suspended account** — silently, for
  *Forgot password*, which answers identically for every address on purpose so it
  cannot be used to discover which addresses exist. A visible refusal there would
  have disclosed which accounts are suspended to anyone who asked.

**Upgrading.** A migration adds the column and moves any intact record across. It
also fills in accounts suspended by the billing webhook, which never recorded one
— for those, `user` is not a guess but the value they already had and would have
been given anyway. Accounts suspended through the panel whose record was
destroyed by a password reset are left empty on purpose, and un-suspending one
asks an administrator to choose rather than picking for them.

Pinned by `tests/suspend-restore-pin-e2e.sh`.

## [2.72.0] - 2026-08-06

### Changed — an administrator can now repair any site on a machine they run

Ownership decided both whose a site was **and** who could touch it. So handing a
site to a `client` took it away from the administrator of the server it runs on:
its page answered *Site not found*, its settings, logs, files, backups, SSL and
terminal all refused, and the only thing left was to transfer it back. v2.71.0
added a read-only list of every site on the box and called that the way back. The
operator using the feature replied that an administrator who cannot fix a
tenant's site is not much use on a server they are responsible for, and he was
right ([#51](https://github.com/ovexro/dockpanel/issues/51)).

Ownership still decides whose a site is, who sees it on their own Sites page, and
who a transfer moves it to. It no longer decides what an operator may repair.

Two bounds, both deliberate:

- **It stops at the hardware you operate** — this box, plus any server you
  registered yourself. Not a machine another administrator added.
- **The admin's own Sites page is unchanged**, still listing only their own sites.
  Everybody's is still behind Sites → *All sites on this server*.

The rule is now **one predicate in one place**
(`helpers::SITE_CALLER_PREDICATE`), shared by every per-site read. It replaces
**eight** separately-named private copies of one query — two `site_domain`, four
`get_site_domain`, two `get_site` — spread over thirteen modules, of which exactly
one had drifted into being server-scoped while the rest were not. A guard
duplicated per module is a guard that gets widened in one module.

It also now reads `users.role` from the database rather than the role claim in the
token, so a demoted account stops being an administrator immediately instead of
when its session expires.

### Fixed — every non-admin's first screen was an authorization error

Signing in as anything other than an administrator landed you on a dashboard
showing a red **"Admin access required"**, a heading stuck on *Loading…*, and
buttons — Restart Nginx, Restart PHP, Reboot, Add Site — that could only fail.
The dashboard asked the admin-only system endpoints as whoever was logged in, one
of its error paths forwarded the API's own words to the page, and the poll
reissued the whole set every five seconds, so it could not be dismissed.

Present for every `user` and every `client` on every install. It is also, almost
certainly, what was being reported as a client having no access to its own site:
nothing was wrong with the site, and the panel said the most alarming thing it
could on the way in.

The dashboard now asks for what the account may have, and no page surfaces an
authorization refusal aimed at somebody else as its own error.

### Changed — the ownership pin now tests the change it was written to catch

`site-transfer-visibility-pin-e2e.sh` §B existed to fail if a future release
widened the site reads. It could not: its arms asserted a `user_id` predicate was
**present**, and every realistic widening adds a disjunct and leaves it present.
The mutation recorded as proof of that section tested deletion, which it does
catch, rather than widening, which it did not — and one arm was passing on an
unrelated query in the same function.

Rewritten around the invariant instead of the token: one definition of the
predicate, no module holding a private copy, the admin arm bounded to the
machine, the role read from the database, and the admin's own list still scoped.
Six arms, each mutation-tested to redden alone.

## [2.71.1] - 2026-08-05

### Fixed — v2.71.0's own fix did not take effect until a minute after sign-in

The self-heal that drops a stale server selection runs in an effect keyed on
mount. **Signing in is a state change, not a remount**, so after v2.71.0 a client
signing in on a browser that had held an admin session still sent the stale
`X-Server-Id` — and still saw *"Server not found or access denied"* — until the
60-second refresh happened to fire. The fix was correct and arrived a minute late,
which for a first impression is the same as not arriving.

The server list is per-account (`GET /api/servers` is scoped to the caller), so
the effect that reads it is now keyed on the signed-in account. Found by tracing
the fix's own timing rather than by re-reading the diff; a 20th arm pins it.

## [2.71.0] - 2026-08-05

### Fixed — the `client` role shipped in v2.69.0 could not be used, and the handover it enabled was one-way (GitHub #51)

Both defects were reported from the field by the operator who had just adopted
the feature, within a day of it landing. Both are ours.

- **A client signed in and every request was refused with *"Server not found or
  access denied"*.** The selected server is kept in `localStorage`, which is
  per-BROWSER and not per-account, and `GET /api/servers` is scoped to the
  caller — no non-admin owns a `servers` row, so a client's server list comes
  back empty. `ServerContext` only ever *corrected* a stale selection when it
  could find a local server to replace it with, and with an empty list it could
  not, so the id the admin had left on that browser survived and was attached to
  every request as `X-Server-Id`. Anyone testing a client account on their own
  machine hit this immediately; a browser that had never held an admin session
  did not, which is why it passed a first test and failed the second. The stale
  id is now dropped when the account's own list cannot back it, and cleared on
  sign-out so it never crosses accounts in the first place.

- **Transferring a site removed it from the admin's panel with no way back.**
  Ownership is a single axis and every site read asks whether the row belongs to
  the caller — which is exactly why a `client` needed no other change, and also
  meant that an admin who handed a site over lost it from their own list, got
  *Site not found* on its page, and could no longer reach the **Transfer**
  control, because that control was rendered only on that page. `docs/guides/
  roles-and-ownership.md` promised the opposite in print, and so did the
  handler's own doc comment.

### Added — Sites → **All sites on this server**

An admin-only, read-only view of every site on the box with its owner beside it,
and a **Transfer** button on each row — so a handover can be undone. It is a
list, not a back door: `GET /api/admin/sites` is a narrow projection, and no
per-site read was widened. An admin still cannot open, edit or delete a site
they do not own; they transfer it back to themselves first.

- The transfer recipient is now **picked from the account list** rather than
  typed, on both the new list and the site page. The endpoint answered 404 on an
  unknown address, so a text field could only ever fail after the fact.
- New pin suite `site-transfer-visibility-pin-e2e.sh` (19 assertions). Transfer
  had **no** regression coverage at all before this release. Its §B arm is a
  negative one — it exists to go red if a future change widens a site read for
  admins instead of adding a view.
- `docs/guides/roles-and-ownership.md`: the role table's *Sees* column described
  a capability the code has never had, for `admin` **and** for `reseller` (a
  reseller sees its sub-accounts and a site *count*, not their sites). Corrected,
  with the way back documented.

## [2.70.0] - 2026-08-05

### Fixed — the public status page was open on every install, and the documented switch did not govern it (SECURITY)

⚠ **Upgrade impact: `/status` is now closed until you turn it on.** If you were
relying on a status page you never explicitly enabled, re-enable it under
**Settings → Public Status Page**. Read the rest of this entry first — it says
what that page has been publishing.

- **Four routes answered with no authentication and exactly one of them checked
  anything.** `/api/status-page` read `settings.status_page_enabled` and failed
  closed — that is the switch the Settings UI writes and the guide documents.
  But `/status`, the page an actual visitor loads, is served by
  `/api/status-page/public`, which read a **different** flag,
  `status_page_config.enabled`. That flag defaults `TRUE` in the column *and*
  again in the fallback used when the table has no row at all, and no migration
  ever seeds the table — so it failed open twice, and a panel published its
  status page **from first boot, with no operator action.**
  `/api/status-page/subscribe` and `/unsubscribe` read nothing at all.

- **What that page publishes is not just uptime.** The alert engine, the
  auto-healer, the uptime checker and the backup executor all open incidents
  marked visible on the status page automatically, and their titles name your
  infrastructure — *"Service mariadb is stopped on This Server"*, *"Disk at 53%
  on This Server"*. On our own demo host, `/api/status-page/public` was serving
  five unresolved incidents, the oldest from 2026-05-02, to anyone who asked.

- **The publish decision now lives in one place**,
  `services::public_status::require_enabled`, and all four public routes call
  it. It fails closed on an absent setting, on a value that is not the literal
  `"true"`, and on a query that errors. `status_page_config.enabled` survives as
  a display preference — it hides the `/status` page while leaving the monitor
  endpoint up — but it is no longer a publish decision.
  `tests/status-page-gate-pin-e2e.sh` computes the route list from the router
  and fails the build if a fifth public status route is ever added without the
  gate.

- **Ungated `subscribe` wrote rows.** An unauthenticated caller could insert
  arbitrary addresses into `status_page_subscribers` as `verified = TRUE`, and
  delete anyone else's, on an install whose status page was never turned on.

### Fixed — an off switch that reported success and did nothing

- `PUT /api/status-page/config` ran `INSERT ... ON CONFLICT DO NOTHING` with **no
  conflict target**, against a table whose only unique index was the
  `gen_random_uuid()` primary key — so the clause could never fire and every save
  inserted another row. The `UPDATE` that followed then matched N rows and
  returned an arbitrary one, while the public reader took an **unordered**
  `LIMIT 1`. An operator could untick *Enabled*, get a `200` and a form showing
  it off, and still be publishing from a different row.
- Migration `20260805120000_status_page_config_unique.sql` dedupes — keeping the
  **oldest** row per operator, which is the one the public reader now selects, so
  the migration cannot change what a panel publishes — then makes `user_id`
  unique. `update_config` names a real conflict target; the public read is
  ordered.

### Fixed — docs and controls that described a different product

- `docs/guides/status-page.md` promised *"When disabled, visiting `/status`
  returns a 404."* That was false for four and a half months. It is true now, and
  the guide carries an upgrade note saying it was not, plus a **What gets
  published** section naming the auto-filed incidents.
- The Settings toggle described itself as sharing *"monitor status ... at
  `/api/status-page`"* — the endpoint nobody visits. It now says it is the master
  switch for `/status`, that it is off by default, and enumerates what goes
  public.
- The status-page **Enabled** checkbox was an unlabelled control on the Incidents
  screen, absent from the guide's own field list. It is now *"Show /status page"*,
  documented, and sits under a line stating that nothing is public until the
  Settings master switch is on.
- `FEATURES.md` headed its background-services section **"11 supervised"** while
  `main.rs` derives **15**, and the register 130 lines below it already said 15.
  Corrected.

## [2.69.0] - 2026-08-05

### Added — a `client` role, and a way to hand a site to one (GitHub #51)

- **`client`** is a new role for a principal who **holds** sites and manages them
  — mail, PHP version, containers, settings — but **cannot bring a new domain
  into service**. Requested by an operator migrating from ISPConfig, where this
  is the ordinary shape.
- **`POST /api/sites/{id}/transfer`** (admin only) hands a site to another
  account by email, in one transaction: the `sites` row plus the four tables that
  keep their own copy of `user_id` beside a `site_id` (`alerts`, `monitors`,
  `secret_vaults`, `whmcs_service_map`). Everything else a site owns —
  `databases`, `crons`, `backups` — reaches its owner through `site_id` and
  follows with no statement of its own.
- ⚠ **Transfer is exclusive**: the previous owner loses the site. This is
  ownership, not sharing. Shared management of one domain by two accounts does
  not exist and is not what this adds.

  Ownership stays a single axis, `sites.user_id`, deliberately. A client who
  *owns* the row passes all 108 ownership-scoped reads in the backend natively,
  so none of them had to change. The alternative — widening every read to "owner
  or delegate" — would have had to keep 57 INSERTs that stamp the acting user in
  step, and get two name-keyed cleanups right whose own comments record that they
  already shipped a cross-account delete once.

  The refusal to claim a new domain lives at **one** place:
  `services::domain_claim::ensure_claimable`, which every domain-introducing path
  already funnels through. Role checks on the four `INSERT INTO sites` sites
  would have left three doors open — `git_deploys`, `docker_apps` and `stacks`
  all materialise a served vhost without ever inserting into `sites`.

- The role selector offers `client` in both the create and edit forms, and the
  site page grows an admin-only **Transfer** control.

### Fixed — the role a migration and a UI knew about but the write path refused

- `routes/users.rs` carried **three identical copies** of the assignable-role
  allow-list (create, update, and the un-suspend restore). Adding `client` to the
  database CHECK and to the user editor still left all three rejecting it. They
  are now one constant. The restore path was the sharp one: a role missing from
  that list is silently downgraded to `user` when an account is un-suspended —
  which would have handed a suspended client back a role that can create sites.

### Security — the static-vhost retrofit, completed

v2.68.0–.2 closed a disclosure where a site switched from PHP to static served
`wp-config.php`, `.env` and `.git/config` as plain text. Three gaps remained.

- **A disabled site re-enabled into the unpatched body.** `disable_site` backs the
  vhost up to `sites-available/{domain}.conf.disabled`, and the startup retrofit
  scans `sites-enabled` for `*.conf` — so the backup fails both tests, and
  `enable_site` restored it with a plain copy and no re-render. A site switched
  to static and disabled before the operator upgraded came back **armed**. The
  restore now patches on the way in, which is the only path by which a disabled
  body is ever served again.
- **One PHP deny the static branch did not subsume**:
  `location ~ ^/sites/.*/private/`. It has no dot segment and no `.php` suffix,
  so neither static deny covered it, and a Drupal site switched to static served
  everything under `sites/*/private/` — the directory that exists precisely to
  hold what Drupal was told not to publish. Verified by serving both configs
  through a real nginx: 200 with the file's contents before, 403 after, with
  `/index.html`, Drupal's *public* files and `/.well-known/` all still 200.
- **The retrofit could report success on a vhost it did not protect.** Candidacy
  tested `contains("index index.html index.htm;")` (a substring, anywhere) while
  insertion tested `line.trim() ==` (a whole line), so an index line carrying a
  trailing comment satisfied one and never the other: the file was rewritten with
  zero denies added, and the caller — which logs on the write, not on the content
  — named it as retrofitted. One predicate now decides both.
- The retrofit also **adds a deny an earlier version missed** instead of skipping
  a vhost it has already touched, so a box patched by v2.68.2 gains the new one
  without duplicating the other two.

### Changed — claims this project made and does not back

Corrected on every surface that carried them, including two the audit trail had
never listed: `COMPARISON.md` and the public site's landing page. `FEATURES.md`
gains a **Withdrawn Claims** section recording what each said and what is
actually there, because a claim that merely disappears from one surface tends to
survive on the others.

- **Teams** — `routes/teams.rs` is 477 lines of working, routed endpoints that
  grant nothing: `team_members` is read by that file and no other, so no
  authorization path consults team membership. There is no Teams UI, and the
  invite email links to an SPA route that does not exist.
- **API keys** — generated, hashed, stored and handed over with "won't be shown
  again", and no code path ever reads a stored hash back to authenticate a
  request. The Settings card and the API reference now say so.
- **App Migration** — writes one row with `status='in_progress'`; there is no
  `UPDATE app_migrations` anywhere in the repository. The tab now says it is not
  implemented rather than pointing at a control that does not exist.
- **Auto-sleep "scale to zero"** — containers stop and do not come back on their
  own. Wording corrected; the wake path is a tracked defect.

## [2.68.2] - 2026-08-05

### Security — v2.68.1's retrofit reached the panel host, not the fleet

- **`update.sh` is the panel updater, and an agent-only box never runs it.** Its
  own header says so: a fleet member has no git repo, no postgres container and no
  frontend, which is why `agent-self-update.sh` exists and fetches a single binary
  instead. So v2.68.1's retrofit protected sites on the panel host and left a
  fleet member's switched site serving `wp-config.php` exactly as before — the same
  shape as the defect it was fixing, one layer out.
- **The agent now performs the retrofit itself, at startup.** The agent is the
  process that owns `/etc/nginx/sites-enabled` on every box it runs on, panel and
  member alike, so this is the only place the fix reaches the whole population.
  Same scoping and same idempotence as the script version, and it **declines to
  run at all when `nginx -t` is already failing** — that check is whole-server, so
  one unrelated broken vhost would otherwise make the migration revert edits it had
  made correctly and blame itself for a fault it did not cause. On success it
  reloads; on failure every file it wrote is restored.
- The decision is a pure function (`patch_static_vhost`) with unit tests, so
  "which vhosts qualify" is exercised without a box: a static vhost is patched
  once and only once, a PHP vhost is skipped (its `.php` location is a *handler* —
  denying it would stop the site dead), and a proxy vhost is not a candidate.
  The patched output was served through a real nginx: `403` for `/wp-config.php`
  and `/.git/config`, `200` for `/index.html`, `/.well-known/` still reaching ACME.

## [2.68.1] - 2026-08-05

### Security — v2.68.0's fix did not reach the sites that already had the problem

- **Updating to v2.68.0 did not protect a site already switched to static.** The
  fix was to the vhost *template*, and a template reaches a site only when
  something re-renders its vhost — a runtime switch, an SSL issuance, a settings
  change. Nothing re-renders on upgrade. So the one site that most needed it, a
  PHP site switched to static under v2.67.0, kept serving `wp-config.php`, `.env`
  and `.git/config` after its operator had updated and been told it was fixed.
- **`update.sh` now retrofits both denies onto static vhosts already on disk.**
  Scoped by what the vhost *is* — the static branch's `index index.html
  index.htm;` and no FastCGI handler — so PHP vhosts, which already carry these
  denies in their preset branch, are untouched. Additive, idempotent, and every
  edit is validated by `nginx -t` with a per-file rollback: a security retrofit
  must not be the thing that takes a site down. Verified by migrating a real
  rendered v2.67.0 static vhost and serving the result through nginx — `403` for
  `/wp-config.php` and `/.git/config`, `200` for `/index.html`, and `/.well-known/`
  still reaching the ACME path rather than being denied.
- ⚠ **The update stops the disclosure; it cannot un-serve what was already
  fetched.** If a site was static while holding real credentials, treat those
  credentials as exposed for that window. `grep -iE 'wp-config|\.env|/\.git' /var/log/nginx/<domain>.access.log`
  shows whether anything asked for them.

## [2.68.0] - 2026-08-05

### Security — switching a site to static published its PHP source

- **A site switched from PHP to static served `wp-config.php`, `.env` and
  `.git/config` as plain text.** Every one of the twelve `deny all` blocks in the
  vhost templates lives inside a PHP preset branch; the static branch had a
  document root, an index and `try_files` and nothing else. That gap was harmless
  while a static site could only ever hold static files — v2.67.0's runtime switch
  is what let a document root full of application source become static, and both
  runtimes resolve to the same `{site_dir}/public`, which is the property that
  made the switch look safe. Verified by rendering both configs and serving them
  through nginx: `403` for `/wp-config.php` and `/.git/config` under PHP, `200`
  with the database password in the response body under static.
- **The static branch now refuses dotfiles and `.php` source outright**, matching
  what the PHP presets already did. `/.well-known/` is exempt, so ACME challenges
  are unaffected. This applies to every static site on its next vhost rebuild, not
  only to switched ones: a static site that was serving `.php` as plaintext or
  exposing `.git/` stops doing so.
- **The switch's warning no longer reassures.** It said the files are not touched
  and switching back restores execution — both true of execution, neither true of
  disclosure. It now says what stops being hidden and advises moving anything
  sensitive out of `public/` first.
- **A billing webhook can no longer overwrite a privileged role.** `role` doubles
  as the account status column, so WHMCS `SuspendAccount` overwrote it and
  `UnsuspendAccount` restored *everyone* to `user`: an admin round-tripped through
  billing came back as a plain user, with nobody left holding the privilege to
  promote them again. Reachable because provisioning adopts an existing user by
  email with no role filter. The suspend now declines to touch `admin`/`reseller`
  accounts and logs when it does — the panel's own suspend, which stashes the
  prior role first, is unaffected.

### Changed — the chain-of-trust report no longer certifies what it cannot measure

- **The chain-of-trust PDF/JSON reported `Chain valid: YES` from a constant.**
  `chain_valid` is written `TRUE` by all seven of its INSERTs, is UPDATEd nowhere,
  and was read back with `.unwrap_or(true)`; no verifier exists, and none could,
  because `previous_hash` is copied from the newest row rather than recomputed
  from the predecessor artifact. The report rendered that constant twice, in bold
  green, under a footer telling the reader a break "indicates either a missing
  intermediate backup or a tampered artifact" — a tamper-evidence claim the panel
  had no mechanism to evaluate, in a document built to hand to an auditor.
- The report now carries only what the panel genuinely recorded: the SHA-256 and
  previous-hash values, the verification count and the drill count. Its footer
  states that hashes are recorded when the backup is taken and not re-computed
  since. `chain_valid` is no longer rendered, and no longer appears in the
  `/backup-orchestrator` list responses. The column and its writers are untouched.
- **The same promise was in the hardening guide, in a stronger form** — *"If an
  attacker tampers with backup files, the chain breaks and alerts fire."* No code
  re-computes a hash and no such alert exists, so removing the verdict from the
  report while leaving that sentence up would have moved the claim rather than
  retired it. The guide now says what the hashes are for and what they are not, and
  names the two paths that record no hash at all: site backups created by a
  schedule or by a policy (2 of the 7 backup INSERTs; database and volume backups
  do record one).

### Fixed

- **`GET /api/sites/{id}/health-summary` was an unconditional 500.** It selected
  `last_response_ms` from `monitors`, whose column is `last_response_time` — a
  `42703` at parse-analyze, so the endpoint had never once returned successfully
  since it shipped on 2026-03-22. The backend has no compile-checked queries, so
  only execution could have caught it.
- **A predecessor backup with no hash was reported as a database error.** The
  `previous_hash` lookups decoded a nullable column as a non-null `String`, so the
  ordinary "the previous backup has no hash" case became a decode failure —
  logged as `DB error fetching previous backup hash` at one site and silently
  swallowed at the four others. All five now decode it as the nullable value it is.



### Added — change a site between static and PHP (#99)

- **A site can now be switched between the static and PHP runtimes from its own
  page.** Reported by HybridRCG: *"I cant seem to find a way to change a site from
  html to php?"* — the workaround was deleting the site and adding it back. He was
  not missing a button; nothing in the codebase issued `UPDATE sites SET runtime`,
  while fourteen other site columns were updatable. `PUT /api/sites/{id}/runtime`
  rebuilds the full vhost and updates the row, in that order, so a refused nginx
  write leaves the site describing what is actually being served.
- **The document root does not move.** `static` and `php` both resolve to
  `{site_dir}/public`, so existing files keep being served across the switch —
  `.html` keeps working and `.php` starts executing. Switching back stops
  execution without touching the files.
- **Switching to PHP requires an explicit version**, rather than silently reusing
  the `php_version` a site carried from an earlier life. The picker offers to
  install a version the server does not have, and the switch is held back until
  that version is installed *and* its FPM socket is up — the agent already refuses
  otherwise, waits for the per-site pool socket to appear, and keeps the shared
  socket rather than pointing nginx at one that does not exist.
- Deliberately scoped to static⇄PHP. The proxying runtimes (`proxy`, `node`,
  `python`) resolve their document root to the site directory itself rather than
  `public/`, so moving to or from them relocates the operator's files — a
  different change, and not one this control will make by accident: the accepted
  targets are a closed list.

### Fixed

- **`FEATURES.md` still stamped v2.65.0 after the v2.66.0 release**, so
  `docs-claims-pin-e2e.sh` was red at `df11639`. v2.66.0 corrected the sibling
  stamp in `docs/testing.md` and recorded the lesson that counts and stamps are
  separate edits — while a fifth surface of the same class stayed stale. The
  suites had been run before the version bump, which is when that arm can pass.

### Changed

- `live-surfaces-check.sh` now asserts that the newest tag actually has a
  **published release**, not merely that the latest published release is sane.
  Arm 5 reads `/releases/latest`, which answers with whatever published most
  recently — so a tag whose release never publishes left every check green while
  `install.sh`, `install-agent.sh`, `update.sh`, the panel's update banner and the
  website all kept resolving the previous version. The new arm is age-gated: a tag
  is only a failure once it is older than the build window, so a release in flight
  does not turn the run red.

## [2.66.0] - 2026-08-05

### Fixed — the panic button's remaining channels

- **The metrics WebSocket ignored session revocation.** `/api/ws/metrics` decodes
  its JWT by hand rather than going through the `AuthUser` extractor, and never
  consulted `sessions_revoked_at`. A stolen admin token could therefore open a
  **new** socket *after* the panic button was pressed and stream the full process
  list, network connections and system info every 5 seconds until its own 2-hour
  expiry — a live view of the operator's incident response, for the intruder it
  was pressed against. The channel now enforces revocation, with the comparison
  kept identical to `AuthUser` so the two gates cannot drift.
- **Panic did not revoke terminal shares.** A share is an unauthenticated public
  URL holding up to 500KB of terminal output for an hour; lockdown is enforced at
  the doors and a public GET is not a door. Panic now deletes every share and
  reports the count. (`revoke_share`/`list_shares` existed, correct and
  admin-gated, but have never had a frontend caller — so there was no in-panel
  way to close one.)
- **The kill report destroyed itself before it could be read.** Panic revokes the
  pressing admin's own session — correct, since they cannot know theirs is not
  the stolen one — and both panic screens then called `loadData()`, whose 401
  hard-navigated to `/login` within one round trip. The operator lost the report,
  including the "TERMINALS MAY STILL BE RUNNING. Check the server." warning. Both
  screens now hold the result and redirect on their own terms.
- **`sessions_revoked` was computed honestly and thrown away.** Both UIs appended
  "all sessions revoked" unconditionally — the same defect v2.65.0 removed for
  `terminals_killed: true`, one field to the left. Both now report what the
  server returned.

### Fixed — a monitoring claim the query could not reach

- **The disk-full forecast could never fire, on any install.**
  `docs/guides/monitoring.md` advertises an alert when the disk is projected full
  within 48 hours; `disk_full_forecast()` requires a 6-hour window, and the caller
  took `LIMIT 60` against a 30-second collector cadence — a ~30-minute window. The
  query is now bounded by time. Its sibling `memory_leak`, which runs the identical
  query without the time gate, had fired 7 times over the same period; this had
  fired zero times, ever. The anti-storm gate it routes through was verified
  load-bearing before arming it.

### Testing

- `auth-doors-pin-e2e.sh` 28 → 32. **One existing arm was corrected, not added
  to:** it asserted "panic revokes every issued session, not just the doors" by
  grepping the panic function for the watermark — a claim about every *reader*,
  tested against one *writer*. It was green throughout the defect above. It now
  says `WRITES`, and a new class arm requires every hand-rolled `Claims` decoder
  outside `auth.rs` to enforce revocation on its own path.
- `docs-claims-pin-e2e.sh` gains an arm requiring the disk-forecast query to be
  time-bounded. The existing unit test was correct about the pure function and
  supplied its own 8-hour trend; nothing measured the caller's window, which is
  how a dead feature kept a green test and a live doc claim.
- All 32 suites green at 1081 assertions; the three new defect arms verified red
  at v2.65.0, each naming its own defect.

## [2.65.0] - 2026-08-05

**The controls you reach for during an incident now do what they say.** Three of
them did not, and each failed in the direction that reads as safety.

**The panic button could not kill the shell it exists to kill.** It ran
`pkill -u www-data`, which reaches the sandboxed site terminals and misses the
*server* terminal — which is deliberately kept as root, and is therefore the one
shell an intruder is most likely to be holding. It then reset the active-session
counter to zero without having killed those sessions, which handed whoever was
there a fresh full quota of terminals. The agent now kills from a registry of the
shells it actually started, so a server terminal is killed like any other, and
the counter reaches zero because the sessions are gone rather than by decree.

**And it locked the doors while the intruder was already inside.** Lockdown is
enforced at login, OAuth, passkey and site creation — all of them doors. Nothing
tested it against a token that had already been issued, so the stolen admin
session that produced the root shell kept working for the rest of its two hours,
and another terminal was one request away. Panic now revokes every issued
session. This logs the pressing admin out too; during a panic that is the correct
trade, and an admin can log straight back in while lockdown holds everyone else
out.

**It also reported success it had not observed.** The call to the agent discarded
its result and the response asserted `terminals_killed: true` unconditionally, so
an unreachable agent produced exactly the same reassurance as a successful kill.
The response now carries what the agent actually reported — how many shells died,
how many of those were server terminals, and whether the agent was reached at all
— and the UI says so, loudly, when it was not.

**Settings > Sessions now exists.** The guide has been describing it for a long
time: a list of your active sessions, each with a Revoke button, and an Export My
Data download. All three endpoints were already built, correct and scoped to the
calling user — they simply had no caller anywhere in the frontend. Wiring them
gives every user self-service revocation of their own devices, which previously
did not exist at any privilege level: the only session control on the page was
"Revoke All Sessions", which is admin-only and panel-wide.

**And the guide it came from has been corrected.** `docs/guides/sessions.md` made
five claims that were not true: an approximate-location column (there is no
location data in the schema at all), a "Revoke All Other Sessions" button that
spares your current session (it is panel-wide and logs out every user), session
binding to the originating IP, and a per-user concurrent-session limit. Neither
of the last two has ever existed as a setting. The page now says what the code
does, including the honest bound on what the GDPR export contains.

## [2.64.2] - 2026-08-05

**Security: a compromised site could become root on the next deploy.** 2.64.0
handed the whole git checkout to `www-data` so PHP-FPM could write it, and 2.64.1
then told git to accept a repository it no longer owned. Those two together gave
the application write access to its own `.git`, and git reads a repository's
`config` and `hooks/` as instruction — so a site under someone else's control
could have the agent, which runs as root, execute a command of their choosing on
the next deploy. It needs no hook file and no executable bit: one line appended
to `.git/config` is enough. Sites deployed from git on the default (non-atomic)
path are affected; **upgrade before your next deploy.**

The fix keeps both requirements without trading one for the other. The working
tree still belongs to `www-data`, so applications write their own uploads, caches
and `.env` exactly as before, and untracked files still survive a deploy. But
`.git` stays root's, unreadable to anyone else, and the site directory is now
setgid and sticky so the application cannot unlink the repository and leave one
of its own in its place. With root owning every repository it touches, git's
ownership guard is switched back on and the `safe.directory` exceptions are gone.

Two consequences worth stating plainly:

- **A repository that root does not own is discarded and re-cloned, not adopted.**
  Repairing one in place would mean taking ownership of content the web user
  wrote and then executing it, which is the same defect with an extra step. Sites
  deployed under 2.64.0 or 2.64.1 will therefore re-clone once, on their first
  deploy after upgrading. Nothing untracked is lost.
- Every git invocation now also runs with hooks and `core.fsmonitor` disabled at
  command scope, which overrides whatever a repository's own config says. That is
  what protects the first deploy after upgrading, before the re-clone happens.

Also fixed: a failed `git reset` reported a successful deploy, most reachably when
a site's branch changes over a checkout holding local commits. And `list_releases`
can read commit hashes again — atomic-deploy releases have been showing no commit
hash since the feature shipped, because they were handed to `www-data` too.

## [2.64.1] - 2026-08-05

**A deploy could be made twice.** 2.64.0 fixed the first git deploy of a site and
broke the second, on the non-atomic path. Found by driving a real box one step
further than the fix required.

Handing the checkout to `www-data` — which 2.64.0 added so PHP-FPM could write to
it — leaves the agent, which runs as root, looking at a repository owned by
somebody else. Git refuses that by design (`detected dubious ownership`), so the
`fetch` and `reset` of every subsequent deploy failed while the first one still
succeeded.

Each git invocation now declares the specific directory it is about to use, at
command scope. The exception is per path — never a wildcard — and command scope
is required rather than incidental: git deliberately ignores `safe.directory`
written in a repository's own config, which is the one file an attacker would
control.

### The same defect, already shipped, in the atomic path

`atomic_deploy` has always chowned its release directory, and two readers ran
`git rev-parse` against those directories afterwards. Both were silently
answering "no commit" — so every release listed for an atomic-deploy site has
been missing its commit hash for as long as the feature has existed. Fixed by
the same exception.

## [2.64.0] - 2026-08-05

**The first deploy of a site the panel made itself has never worked.**

Creating a site writes a placeholder page into its document root, so the
directory is never empty. `git clone` refuses a destination that already exists
and is not empty. Every site created through the panel therefore failed its
opening git deploy with `fatal: destination path ... already exists and is not
an empty directory`, and only started working once someone emptied the web root
by hand.

Atomic deploys were never affected — they build each release in its own
directory — but atomic deploys are **off by default**, so the path that failed
is the one a new install actually takes.

### What the failure took with it

The panel runs five things only on a successful deploy, and a deploy that dies
at the clone reaches none of them: Laravel migrations, the post-deploy health
check, the completion notification, the deploy-log success record, and the
auto-injection of vault secrets that 2.63.0 had just repaired. That fix was
correct and still unreachable on a first deploy.

### The fix, and the two things it is careful about

The clone now goes to a staging directory beside the site and its contents are
moved into place entry by entry.

- **Files the panel did not write are refused, never deleted.** The placeholder
  is recognised by its content rather than by its name, so an `index.html` an
  operator uploaded is not mistaken for ours. A directory holding anything
  unrecognised stops the deploy and the message names the files that blocked it.
- **A repository with no `public/` still leaves a document root behind.**
  Entries the clone does not supply are left alone, so a static or PHP site
  keeps serving; the deploy log says plainly that the placeholder is still what
  visitors get, instead of reporting a clean deploy over a page nobody asked for.

### Deploys no longer hand the site to the wrong user

`atomic_deploy` has always chowned its release directory to `www-data`. The
non-atomic path never did, so a checkout made by the agent stayed owned by root
and PHP-FPM could not write to it — uploads, caches and generated files all
failed after a deploy that reported success. Both paths now do the same thing.

## [2.63.0] - 2026-08-04

**The automatic path threw away what the manual path kept.**

Two features had a working half and a silent one. In both cases the button an
operator clicks did the job correctly, and the unattended path beside it — same
data, same destination — dropped the result on the floor and had no way to say
so.

### Auto-injected secrets never arrived, and could not report that they hadn't

A vault secret marked **auto-inject** is supposed to be written into the site's
environment after every successful git deploy. Since the vault gained its own
key derivation, that never once happened.

The vault is not encrypted under the JWT secret; it is encrypted under a key
*derived* from it. Every writer used the derivation, and so does the manual
**Inject** button — which runs the same query over the same rows and sends the
same body to the same agent route. The post-deploy path passed the raw JWT
secret instead, so every decryption failed.

Nothing could report it. AES-GCM is authenticated, so the wrong key cannot
produce garbage — it produces an error, which was discarded; the list of
variables then stayed empty, and both the agent call and the only log line sat
inside a branch that could not be entered. A feature listed in the README, in
the feature matrix, in its own guide and behind a checkbox in the UI was inert
and completely silent about it.

Auto-inject now uses the same derivation as every other vault reader, names any
secret it cannot decrypt, and says so when it injected nothing because none of
them could be read. An agent that refuses the write is reported too.

### A renewed certificate left the panel describing the retired one

The security scanner renews certificates that are within 30 days of expiry. It
is the **only** automatic renewal on a stock install, because auto-healing ships
switched off — and it runs for every host the panel knows, its own included.

It renewed correctly and discarded the new expiry date the agent handed back, so
`sites.ssl_expiry` kept the value written when the certificate was first issued.
The dashboard countdown ran down to zero and the expiry alert walked its whole
warning ladder — 30, 14, 7, 3, 1 day — and then raised **EXPIRED**, at critical
severity, on a certificate that had renewed perfectly and was serving traffic.

It could not recover on its own, either: that alert resolves only when the days
remaining *increase*, which cannot happen while nothing rewrites the column.
Escalation then re-paged it for a week.

The scanner now records what it installed, clearing the renewal-hint columns
alongside it so the next cycle does not reuse a window computed for the
certificate it just replaced. When the agent returns an expiry it cannot parse,
that is now a warning rather than silence.

### A post-renewal lookup keyed on a domain instead of the site

After renewing, the scanner re-read the site to rebuild its full nginx config,
looking it up by domain. A domain is unique only *per server*, so on a fleet
that lookup could return a different host's row — and the rebuild it feeds is
sent to the host being scanned. It now reads the row it already resolved, by id.

## [2.62.1] - 2026-08-04

Two corrections to v2.62.0, both found auditing that release's own diff.

### The dashboard polled slow data three times as often on a remote server

v2.62.0 stops opening the metrics socket when a remote server is selected, which
leaves the page permanently in its polling state. That state drove **one** timer
for two different jobs: the five-second interval that keeps the live CPU/memory/
network tiles current also refetched sites, databases, containers, recent
activity and the mail queue on every tick.

While the socket was almost always connected, that only happened during an
outage and nobody noticed. With a member selected it became the steady state —
so the slow reads ran at 5s instead of 15s, each through the panel-to-agent hop,
for values that change on the order of minutes.

The two cadences are now separate timers: slow data every 15 seconds regardless
of socket state, live tiles every 5 seconds and only when the socket is not
already supplying them. A dropped socket now also refetches immediately instead
of waiting out the interval.

### Removed a flag that nothing read

`wsConnectedRef` existed so a long-lived interval callback could see the current
socket state without being recreated. Splitting the timers means both effects
re-run when that state changes and read it directly, leaving the ref written in
five places and read in none. Removed rather than left as a writer with no
reader.

## [2.62.0] - 2026-08-04

**The dashboard describes the machine you picked.**

v2.61.0 fixed two of the three screens that could quietly report the wrong host.
This is the third, and it was the one on the first page you see.

### The dashboard's own tiles showed the panel host under any selected server

The dashboard opens a WebSocket for live CPU, memory and network. A browser
WebSocket cannot send request headers, and `X-Server-Id` — the header that tells
the panel which machine you selected — is attached by the HTTP client wrapper,
which the socket does not go through. So the socket resolved to the panel's own
agent **whatever the server picker said**.

That alone would have been merely wrong. What made it hard to notice is that the
same page also polls three REST endpoints for the same three values whenever the
socket is down, and those *are* correctly scoped to the selected server. Both
paths write the same state. The result: with a fleet member selected, the tiles
showed **that member while the socket was down and the panel host while it was
up**, swapping on the three-second reconnect timer, with the status dot still
reading "Live".

The socket is no longer opened when a remote server is selected. The five-second
poll already reads the right host, so a member's tiles are now correct rather
than merely honest, and the badge names the server it is polling. Switching
servers also refreshes immediately instead of waiting out the interval.

### Removed: the remote-command channel that could never execute a command

Every fleet member ran a poller that asked the panel for queued commands every
five seconds. The panel's dispatch allow-list and the agent's execution
allow-list were written independently and shared exactly one action out of ten —
and that one named an agent route that does not exist. Seven of the agent's own
eleven actions pointed at routes it does not serve either. No screen ever called
it and no command was ever queued.

It is removed rather than completed, because completing it was the dangerous
option: the forwarder pasted a caller-supplied string straight into a request
path, aimed at a **loopback listener that served the entire agent API** —
files, terminal, exec, databases. Reconciling the two allow-lists, the obvious
one-line "fix", would have switched that on. Per-host operations already have a
correct path: the panel's per-server resolver, which authenticates, checks
ownership, and calls the agent's real route directly.

Gone with it: `GET/POST /api/servers/{id}/commands*`, `GET /api/agent/commands`,
`POST /api/agent/commands/result`, and the agent's `127.0.0.1:9090` listener.
The `agent_commands` table is left in place — it is empty and harmless, and
dropping it would destroy any record of attempted commands on an existing
install.

**Upgrade impact:** the agent no longer binds port 9090, so 9090 is no longer on
the security scanner's expected-port list. A box running something else there —
Prometheus and Cockpit both default to 9090 — will now see one "Unexpected open
port" warning. That is the scanner working, not a false positive.

### Regression pins

`unattended-host-scope-pin-e2e.sh` grows from 105 to 111 assertions. The arm
that guards this class **used to iterate a hardcoded list of two**, so a third
surface of the same defect was invisible to it by construction; it now derives
its subjects from source. Two new arms cover the rest of the class: every raw
browser stream in the client must prove which host it describes, and no
server-sent-event handler may resolve a per-server agent.

## [2.61.0] - 2026-08-04

**The panel stops lying.**

Five places where DockPanel told the operator something that was not true. None
of them were failures of the underlying feature — in each case the machinery
worked and the account of it did not.

### A shared terminal outlived its stated expiry, in public

`POST /api/terminal/share` saves terminal output behind a public link and
promises it for one hour. The viewer that serves that link computed how long the
share had left, clamped a negative result to zero, and **rendered the content
regardless** — while the page's own countdown printed "Expired" above it. The
only thing that removed an expired share was a retention sweep that runs once a
day, so a link advertised as lasting an hour served root shell output to anyone
holding it for up to twenty-four.

The operator's own share list had always applied the expiry correctly and hidden
those rows, so the panel reported a share as gone while the URL still worked.

Expiry is now decided by whoever answers the request, and both readers reach that
decision through the same function so they cannot disagree again. An expired
share is answered exactly like an unknown one. The unauthenticated route also
validates the share id, which the revoke path has always done.

### The web terminal and live log streaming say what is actually wrong

Both mint a ticket signed with the **selected** server's agent token, then the
browser opens a socket to the panel's own address — which reaches the panel's
agent, which verifies with its own key and rejects it. With a fleet member
selected, both features therefore failed, and both blamed the network for it: the
terminal reported "Connection lost" and offered to reconnect, and the log viewer
retried every three seconds indefinitely.

Both now answer at the point the ticket is minted, naming the server you have
selected and saying the feature runs on the panel host. The terminal shows that
as a notice rather than an error, and the log viewer no longer retries a decision
the panel has already made. Reading, searching and downloading a member's logs
were never affected and still work.

The server shell being switched off now says so in the same way, and names the
setting.

### The audit log can say which machine an action happened on

`activity_logs.server_id` has been written since 2.58.0 and stamped by the
auto-healer's own writers since 2.60.0. Nothing displayed it: the API's row type
never declared the column, so `SELECT *` dropped it before serialisation, and the
audit page rendered nine fields with no host among them. On a fleet, every entry
read as though it had happened on the panel.

The feed now carries the host and resolves it to the server's name. Automatic log
cleanups previously spent the operator-facing target column on the server's UUID,
because the hourly cooldown keyed on it; the cooldown now keys on `server_id`,
like the four other gates beside it, and the target column names the machine.

Upgrade note: cleanup rows written before this release carry no `server_id`, so
they no longer suppress. A host whose disk alert is firing may clean its logs
once more than the hour would otherwise allow. The action is idempotent and this
does not repeat.

### The multi-server guide no longer promises three things that do not exist

It listed the web terminal, log streaming and "per-server dashboards" among what
you can do on a remote server. The first two are panel-host only, for the reason
above, and there is no per-server dashboard view. The guide now says which
features terminate on the panel host and what to use instead.

### Notes

No migration — every column already existed. Eleven new regression assertions
(1009 → 1020), including a tripwire that fails if a streaming proxy is added
before the terminal ticket binds its own domain.

## [2.60.0] - 2026-08-04

**The nil uuid is not a user.**

Four places in the panel wrote an audit row for "no user" by passing a
zero-valued UUID. `activity_logs.user_id` is nullable and its foreign key points
at `users`, so a nil is not "nobody" — it is a non-NULL value naming a row that
does not exist and cannot be created. Postgres rejected every one of those
inserts, the error was swallowed into a log warning, and **none of the four ever
recorded anything.**

Two of them were reading their own rows back as a rate limit.

### Certificate renewals stopped hammering the CA

`auto_renew_ssl` gates its retries on a count of its own audit rows — the
cooldown whose comment says it exists "to prevent hammering the CA if renewal
keeps failing". That count could only ever return zero, so the gate never
engaged: a certificate DockPanel could not renew was re-ordered from Let's
Encrypt **on every 120-second tick, indefinitely**. The agent does not refuse
these cheaply either; it validates the domain's shape and places a real ACME
order. Renewal attempts are now recorded against the site's owner and stamped
with the site's server, which drops a stuck certificate from roughly 720 orders
a day to 4 (or 24 on the short-lived profile).

The same function filed its failure alert against `SELECT id FROM servers ORDER
BY created_at ASC LIMIT 1` — the oldest row on the panel, with no filter. That
is deterministically the panel's own server, so on a fleet a member's expiring
certificate raised a critical alert attributed to the wrong machine, which the
alerts page then hid from anyone whose server picker was set to the member it
was actually about. It now names the site's own server.

### Failed logins against unknown accounts are recorded at all

The login handler wrote a real user id when the password was wrong for an
existing account, and the nil uuid when the email matched no account. So the
panel recorded the one failure mode that is *not* user enumeration and dropped
the one that is: credential stuffing and username probing are, by definition,
attempts against addresses that do not exist. `GET /api/security/login-audit`
had nothing to show because nothing had been stored.

The audit feed also now carries the account and the origin of each attempt. It
was selecting `target_name` as the "user" column, and both login writers pass no
`target_name` — so that field was NULL on every row the table has ever
rendered. Panel Login Activity gains **Account** and **From** columns, and
flags an attempt against an address with no account.

### Also

- **Auto-sleep leaves an audit record.** Stopping a customer's container wrote
  nothing at all. Nothing initiates an auto-sleep, so this uses the new
  no-user writer, which stores NULL — what the schema has always sanctioned.
- **A heal on one host no longer mutes another host's alert.** The alert
  engine's five-minute "auto-healer recently handled this" suppression counted
  `auto_heal.restart_service` rows by service name with no server predicate,
  while the writer has stamped `server_id` since 2.58.0. Every host has an
  `nginx`, so healing one silenced them all.
- **A panel with no local server row reports it.** The WHMCS app-migration
  endpoint defaulted a missing source server to the nil uuid and fed it to a
  NOT NULL column with a foreign key, turning a clear precondition failure into
  an opaque 500 three statements later.

### Internal

`log_activity_system` is the sanctioned way to say "no user"; `user_id` is an
`Option` in exactly one private function, so a caller must choose between
naming a user and declaring there is none rather than inventing a third answer.
The regression suite gains a **class** arm reading all 109 backend sources and
202 call sites, plus one that forbids a nil-uuid factory — the indirection that
hid the login writer from a call-site grep for eight releases.

## [2.59.0] - 2026-08-02

**The panel host is a machine too.**

Four releases threaded background services so an unattended action names the host
it acts on. This one fixes the half none of them looked at: which hosts the panel
can *see*. DockPanel kept per-host telemetry in two stores written by mutually
exclusive halves of the fleet, so every check built on one was structurally blind
to the other.

`servers.cpu_usage` and its siblings are written only by the check-in handler,
which only an agent phoning home reaches — and phone-home starts only from
`agent.env`, a file only `install-agent.sh` writes. **The panel's own box has no
such file**, so its row held NULL in every one of those columns, on every install
ever made. `metrics_history` was the mirror: written only against the local
server's id, so no member ever had a row in it.

### Fixed — the panel host could not be alerted on

- **The panel host could never raise a CPU, memory or disk alert.** Its readings
  were NULL and the alert engine guards each threshold on the value being
  present, so all three silently skipped, for ever. On a single-server install —
  the default and commonest shape — that is *every* resource alert the product
  offers.
- **Automatic disk recovery had therefore never executed on a single-server
  install.** `auto_clean_disk` is triggered by a firing `disk` alert, and that
  host could not raise one. The feature was reachable only on a fleet, from a
  member.
- The panel's own hardware line on the Servers page was suppressed for the same
  reason: it renders only when CPU cores or RAM are known, and neither was.

### Fixed — members had no history

- **No memory-leak detection for any member.** The trend check reads
  `metrics_history` and a member had no rows in it.
- **A member's 24-hour uptime sparkline was 144 empty buckets** — drawn as a host
  that had been down for a day while it was checking in every 60 seconds. It is
  derived purely from the presence of history rows.
- **The Prometheus scrape returned one server on a fleet of any size**, though
  `FEATURES.md` advertises a gauge per server. The exporter is written
  fleet-wide; the table under it only ever held one host.
- The fleet-overview endpoint's CPU, memory and disk columns were null for every
  member.

### Changed

- `metrics_collector` takes the agent registry and reads **every online server
  through that server's own agent**, concurrently, so one slow host cannot make a
  30-second tick fall behind. It is now the single writer of `metrics_history`
  for the whole fleet, and — for the local row only — of the scalar columns a
  member reports by phoning home. Members keep getting theirs from check-in.
- `AgentRegistry::online_fleet` now reports `is_local`, so an iterating service
  can tell the panel's row from a member's without a second query.
- The offline sweep now names its `is_local` exemption in SQL. This is a no-op
  today and deliberately so: the local row's `last_seen_at` is NULL, and `NULL <
  …` is NULL rather than true, so the sweep has never matched it. That accident
  is now a stated rule, because `status = 'online'` is the predicate of both the
  fleet iterator and the alert engine's own query.

### Upgrade impact

- **Prometheus consumers on a fleet will start receiving one series per server
  where they received one in total.** Any dashboard or alerting rule doing a bare
  `sum` or reading a single value changes meaning without erroring. This is the
  advertised behaviour finally holding; it is still a shape change.
- **The panel host becomes eligible for automatic disk cleaning for the first
  time**, if auto-healing is enabled. That path cleans logs and `/tmp` files
  older than seven days, and — only when separately opted in — reclaims dangling
  images and build cache. It never touches volumes, site files or the database
  directory.
- Existing history rows are untouched; no migration.

### Testing

- `unattended-host-scope-pin-e2e.sh` gains §H (72 → 82 assertions), **10 of them
  watched red against v2.58.0** before being trusted. Among them the arm the last
  four releases each needed and none had: a **class** arm over the spawn sites
  asserting that exactly one background service may still hold the legacy
  single-agent handle. A per-defect arm cannot see a sibling left behind.

## [2.58.0] - 2026-08-02

**A scan belongs to the machine that was scanned.**

The multi-server migration is finished. v2.56.0 threaded one background service,
v2.57.0 threaded eleven more plus two webhook routes, and this release closes the
last three — the two scanners and the alert engine — together with the healer
path that only worked because two bugs cancelled.

### The unit was not uniform

`security_scanner` and `image_scanner` take a **machine** as their subject, not a
row: they ask an agent what it found, so there was no per-row `server_id` to
thread and they needed an iterate-the-fleet loop instead. Both primitives that
required already existed with zero callers — `online_server_ids`, doc-commented
"for background services that need to iterate", and an `Option`-taking resolver
that silently answers a missing id with the local agent and is now documented as
the trap it is. `AgentRegistry::online_fleet` is the one primitive both scanners,
the alert engine and the healer now share, so the skip-don't-substitute rule
cannot drift between call sites.

### Fixed — fleet correctness

- **Members were never security-scanned at all.** Not mislabelled: never looked
  at. The weekly scan asked the panel's own agent and filed the result under no
  server, and the cadence gate was fleet-wide, so the first host scanned
  satisfied it for every other host. The gate is now per server and only a
  **completed** scan satisfies it — it counted rows of any status, so one failed
  scan bought a whole week of no security scanning.
- **File-integrity monitoring detected the FIRST change to a watched file and was
  deaf to every change after it.** The upsert's `ON CONFLICT (server_id, file_path)`
  named a column the INSERT never supplied, and a NULL never conflict-matches, so
  `DO UPDATE` had never once executed: every scan appended a new baseline and the
  comparison read the oldest row. Measured on a live panel: 126 rows for 7 watched
  paths — 18 duplicates each, one per weekly scan since March. The watched set
  includes `/etc/shadow`, `/etc/sudoers` and `/etc/ssh/sshd_config`.
- **Image scan results and SBOMs collided across hosts.** `image_scan_findings`
  had no server column and `image_sbom` was keyed on the bare image string, so
  two machines running the same image overwrote each other. The deploy gate read
  the same bare key, so a clean scan on one host could wave a vulnerable image
  onto another; the 30-row history trim evicted a quiet host's only result; and
  the sweeper's freshness check let one host's scan suppress every other host's
  rescan indefinitely.
- **Auto-restart would have restarted the wrong host's services.** `alert_engine`
  labelled every agent-driven reading with the oldest `servers` row while
  `auto_healer::auto_restart_services` read `alert_state` with no server predicate
  and posted to the local agent. The two cancelled, so the restart landed
  correctly by accident; writing correct server ids ends the accident. Both are
  fixed in this release — separately, either one is a regression.
- **Auto-restart of exited containers had never run once.** It read `state` and
  `id` from the agent's `/apps` payload, which carries `status` and
  `container_id`. A missing JSON field reads as an empty string, so both the
  state test and the emptiness guard failed silently and permanently.
- **The service-restart cooldown had never engaged.** It counted `activity_logs`
  rows written with the nil uuid, which violates the user foreign key, so the
  insert always failed and the count was always 0 — neither the 10-minute gap nor
  the give-up-after-3 rule ever applied, and a crash-looping service was restarted
  every 120 seconds for ever with no audit trail. The record is now written
  against the server's owner and stamped with `server_id`, which also stops two
  hosts running a service of the same name from sharing one budget.
- **A four-month-old alert tombstone suppressed an entire alert type.** A removed
  container never reappears in `/apps`, so neither the fire branch nor the
  recovered branch could ever run for it and its row stayed `firing` for ever —
  silently suppressing every future `container_down` alert for that name, with
  retention unable to help because both purges only delete resolved rows. The
  container health check now clears state for containers the host no longer
  reports.
- **The compliance report paired one server's scan with another's live data.** It
  scoped the agent it queried and then took whichever scan finished most recently.
- Security scan history, posture and detail views are server-scoped; a clean scan
  on one machine no longer resolves another machine's security alerts.

### Added

- `activity_logs.server_id` is written for the first time. The column, its index
  and its foreign key have existed since the multi-server migration with no
  writer and no reader; an unattended action that acts on one machine and records
  a row indistinguishable from the same action on another is not an audit trail.
- 31 new regression-pin arms (§G of the unattended-host-scope suite), covering
  the s300 backup family — which shipped with none — as well as this release.
  30 of the 31 were watched **red** against v2.57.0 before landing.

### Migrations

Two, both deduplicating before backfilling because the order matters: collapsing
18 duplicate baselines onto each of 7 keys before the backfill would violate
`UNIQUE(server_id, file_path)` and abort. `server_id` is then `NOT NULL` on
`security_scans`, `file_integrity_baselines` and `image_scan_findings`, so an
insert that forgets to bind the host fails loudly instead of silently recreating
the inert-upsert defect. `image_sbom`'s primary key becomes `(server_id, image)`.

⚠ **`routes/sboms.rs` had to change in the same commit as that key.** An
`ON CONFLICT` arbiter naming no unique index does not degrade — Postgres raises
`42P10` at execution time — so the migration alone would have turned every SBOM
generation into a 500.

## [2.57.0] - 2026-08-02

**A schedule belongs to the machine that owns it.**

v2.56.0 threaded exactly one background service — the disk healer — through the
per-server agent registry. Eleven others, and two webhook routes nobody had
counted, still queried the whole fleet and acted on whichever machine runs the
panel. This release closes both paths that were destructive rather than merely
wrong, and the chain that made v2.56.0's own fix insufficient.

Driven on a two-box fleet on the published v2.56.0. A member's cron git deploy
ran **entirely on the panel host** — cloned into the panel's own
`/var/lib/dockpanel/git/api`, built there, bound the panel's port — while the
member ran nothing at all. Six times, each logged as
`Deploy success (scheduled)`.

The sharp edge is the checkout path, `/var/lib/dockpanel/git/{name}`, which is
keyed by **name alone** while `idx_git_deploys_name_server` makes a deploy name
unique only **per server**. Two servers may legitimately own an `api`, so on the
executing host they share one working directory: whichever cloned first owns
`origin`, and the other fetches against it, hard-resets it, and builds the
**wrong repository** into the other tenant's container name — then reports
success. Both directions were observed on one box. Nothing catches it, because
the post-deploy health check fetches the deployment's public domain, which still
resolves to the untouched container on the machine that was never deployed to.

**Fixed**

- **Scheduled git deploys run on the server that owns them.** `trigger_deploy_task`
  resolved `AgentHandle::Local` three lines before it read the row carrying
  `server_id`; it now resolves that server and **refuses out loud** when it is
  unreachable, rather than falling back to the local host — the fallback was the
  defect. Both scheduler queries now select `server_id`.
- **Both deploy webhooks too.** A webhook carries a secret, not a session, so it
  has no `ServerScope` to read a server from — which is why both reached for the
  local agent. The row is the authority: a push to a deployment or a site owned
  by a remote server no longer builds, replaces containers and rewrites vhosts on
  the panel host.
- **Preview teardown names its host.** `git_previews` carries no server of its
  own, but the sweep already joins `git_deploys`, whose `server_id` is `NOT NULL`.
  An expired preview is torn down on the machine it actually runs on; an
  unreachable server keeps its row for the next sweep instead of having a
  same-named container destroyed elsewhere.
- **A one-time schedule is no longer lost, or replayed for ever.** Reachability
  is checked *before* the schedule is cleared, so an unreachable member keeps the
  operator's only copy of the instruction. And the clear is now *checked*: a
  successful run leaves the row `running`, which the one-time query does not
  exclude, so a clear that silently failed redeployed production every 60 seconds.
- **`metrics_collector` labels local readings with the local server.** It asked
  for the *oldest* server row under a comment claiming it asked for the local one.
  These readings come from the local agent, so mislabelling them writes the panel
  host's disk usage against a member's `server_id` — which `alert_engine`
  thresholds, and which v2.56.0's now-correctly-scoped healer would then act on,
  cleaning the member because the panel is full. Reachable whenever the local row
  is not the first ever created: `servers.user_id` is `ON DELETE CASCADE`, so
  deleting the founding admin drops the local server row and the next restart
  mints a newer one.
- **`for_server` recognises the local server from the database.** `ensure_local_server`
  returns a nil id until an admin exists, and the local row has no `agent_url`, so
  resolving the local server could return `NotFound`. Threading the fleet onto
  `for_server` is what made that reachable — without this a **single-server**
  install would have had every threaded service refuse to act on its own box.

**Still fleet-blind, and tracked:** `backup_scheduler`, `backup_policy_executor`,
`drill_scheduler`, `backup_verifier`, `security_scanner`, `image_scanner`,
`alert_engine`, `telemetry_collector`. None of them destroy data — they read or
attribute against the wrong host. `telemetry_collector` is legitimately local-only.

New pin `unattended-host-scope-pin-e2e.sh` §F — 15 arms, all 15 red against
v2.56.0, no skips.

## [2.56.0] - 2026-08-01

**An unattended service must name the host it acts on.**

DockPanel's multi-server migration reached every HTTP route and **not one
background service**. `AppState` has carried both handles for releases —
`agents: AgentRegistry` ("dispatches to local or remote agents by server_id")
and `agent: AgentClient` ("Legacy single-agent accessor") — and all twelve
background services were spawned with the legacy one. Each of them queries rows
across the whole fleet and then acts on whichever machine the panel happens to
run on.

For the disk healer that is not a routing bug, it is destruction. `auto_clean_disk`
read one firing `disk` row with no `server_id` predicate, then sent the fixes to
the local agent. `alert_state` is keyed per server, so **any** member crossing its
threshold made the panel host clean and prune **itself**, while the machine that
was actually full was never touched.

Driven on a two-box fleet, on the released v2.55.0: the member was filled to 93%,
the panel host sat at 18% with its own `disk_usage_pct` never even measured, and
forty seconds after auto-healing was switched on the panel host lost a tenant's
container and its image — an app the panel itself had put to sleep. The panel
reported *"Auto-heal: disk cleanup succeeded, disk alert state reset."*

Three further defects, each of which made it worse:

- **The hourly cooldown could never engage.** It counts `activity_logs` rows for
  `auto_heal.clean_logs`, and those inserts were written with `Uuid::nil()`,
  which violates `fk_activity_logs_user`. Every insert failed, the count was
  always zero, and `docker system prune -af --volumes` ran on the same healthy
  host **every 120 seconds** — measured at 19:16:13, 19:18:13, 19:20:13,
  19:22:13. The operator also had no audit record of any of it.
- **The recovery transition was consumed, not completed.** A raw `UPDATE` to
  `'ok'` skipped `notifications::resolve_alert`, so the `alerts` row stayed
  `firing` for ever; the alert engine, seeing state already `'ok'`, never took
  its recovery branch again. Retention only purges `status = 'resolved'`, so
  those rows were also unpurgeable.
- **The prune was indiscriminate.** `docker system prune -af --volumes` removes
  every stopped container — and a sleeping app *is* a stopped container — then
  `-a` takes every image no longer held by a running one, including locally built
  images with no registry to restore them from. `wake` only issues `docker start`
  on an id that no longer exists.

  Precisely, because the blast radius is often overstated: on Docker 23.0+
  `--volumes` reclaims **anonymous** volumes only, and template apps keep their
  data in host bind mounts, so an app's *data* usually survives. What is
  destroyed is the container, its image, and any anonymous volume the image
  declared — which for a tenant database container is where the data lives. On
  the live fleet the panel host went from 2 containers and 2 images to 1 and 1
  while the volume count was unchanged, which is exactly this shape.

### Fixed

- **The disk heal resolves the agent for the server whose alert is firing**, via
  `AgentRegistry::for_server` — the primitive every HTTP route already uses. When
  that agent is unreachable it **refuses and says so**, rather than falling back
  to the local host, which was the whole defect.
- **The cooldown is real and per-server**, its activity row written against the
  server's owner. It gates the next run and gives the operator the audit trail a
  destructive action owes them.
- **Recovery goes through `resolve_alert`, scoped to that server**, so the
  `alerts` row resolves, the operator is told the server recovered, and retention
  can reach the row.
- **`docker system prune -af --volumes` is gone.** A new `docker-reclaim` frees
  dangling images, build cache and unattached networks that DockPanel does not
  manage. **Volumes are deliberately never reclaimed** — an unattached volume is
  indistinguishable from one whose container the panel stopped on purpose. The
  old `docker-prune` id now routes to the scoped reclaim, so an older panel
  pointed at a newer agent cannot get the destructive behaviour back by name.
- **Reclamation is opt-in and separately consented** (`auto_heal_docker_reclaim`,
  default off), with its own control in Settings. The Auto-Healing panel's text
  was wrong on every count: it named 90% where the default is 85, said "cleans
  logs" where the code also pruned Docker host-wide, and promised "All actions
  are logged in the Audit Log" — the log write that always failed.
- **Backup retention no longer destroys the only record of an archive it did not
  delete.** The policy sweep unlinked `/var/backups/dockpanel/databases/{filename}`
  while the writer creates `.../databases/{db_name}/{filename}` (and the same for
  volumes, per container) — a path that can never exist, whose `ENOENT` was
  discarded, with the row deleted regardless. Every database and volume dump was
  orphaned on disk with nothing left pointing at it. The path now matches the
  writer, the row survives a failed unlink, and a backup belonging to another
  server is refused rather than silently forgotten.
- **Policy retention is per resource.** `OFFSET n` over a policy's whole history
  kept `n` backups in total across every database it covered, so a policy
  protecting five databases kept five backups and four databases kept none.

## [2.55.0] - 2026-08-01

**A container name is not a key anybody owns.**

`dockpanel-git-{X}` is the container of a git deployment called `X`. It was also
the container of a *preview* of config `C` on branch `B` whenever
`{C}-pr-{slug(B)}` spelled `X` — and `X` is a name the panel will happily
create, because `is_valid_name` accepts hyphens. The agent resolved that name
with `list_containers` and acted on whatever answered.

What it did with it was blue-green: it read the domain off the container it had
just found, swapped **that** domain's vhost to the pushed build, force-removed
the container behind it, and reported a successful zero-downtime deploy. The
ownership guard added in v2.53.0 authorised the vhost write, correctly — the
victim's container really was the one behind the victim's vhost. The branch half
of the colliding name is chosen by whoever can push to the repo, and
`POST /api/webhooks/git/{id}/{secret}` has no auth extractor, so no panel
account was needed to fire it. The same shared name also meant one checkout
directory, one image repository (so one deploy's `prune` evicted the other's
rollback history) and one unattended TTL sweep.

`services::ownership` had five primitives and every one of them read a file.
The largest thing the agent destroys had none.

### Fixed

- **Previews have their own name space.** A preview is scoped `pr.`, and `.` is a
  character `is_valid_name` rejects, so no deployment can be named into it. The
  scope travels on the wire and is applied to the container name, the image
  repository and the on-disk checkout alike.
- **`services::ownership` gained a container primitive.** Every git container
  records which space it was created in; a deploy that finds a container
  belonging to something else now refuses and changes nothing. The
  compatibility path that reaches the old shared space requires the caller's own
  recorded port to match the container's — every container predating this
  release is unlabelled, including the victim's, so an absent label is not
  evidence of anything there.
- **The blue-green stand-in no longer occupies a real name.** `{name}-blue` is a
  name `is_valid_name` accepts, so updating the app `api` force-removed the
  running container of the app `api-blue`. The separator is now `.`, and the
  leftover-clearing step refuses anything not managed by DockPanel. Both twins.
- **A blue-green swap frees the promoted name before it destroys anything, and
  reports when it cannot.** The commit phase removed the old container and
  renamed the new one with both results discarded — so a failed removal made the
  rename impossible, and the function returned success over a host whose nginx
  pointed at a container the *next* deploy would clear away as a leftover.
- **Editing or adding a git deploy's domain writes its vhost.** `setup_nginx_proxy`
  had one call site, in the first-deploy branch, so a domain change moved the
  label and nothing else — the new hostname never got a server block, on that
  deploy or any later one.
- **A masked environment value sent back is treated as unchanged.** The env
  editor is seeded from the masked read and posts every field back, and the
  container is the only place a Docker app's environment is stored — so saving
  any change wrote the literal mask over every secret and then removed the
  container holding the originals. The mask predicate also matched by unanchored
  substring, so `KEYCLOAK_ADMIN`, `NEXTAUTH_URL` and `AUTHENTIK_POSTGRESQL__HOST`
  were masked and destroyed alongside real secrets; the catalogue's own
  `secret: false` now exempts them.
- **The v2.53.0 ownership guard reaches the paths it missed.** Renaming a site
  moved a shared wildcard certificate directory out from under every sibling
  vhost, and migrated a Fail2Ban jail at the non-injective `nginx-{domain}` name
  without proving either end. Disabling, enabling or saving the `.env` of a site
  ran `systemctl stop`/`restart` on a unit name that collapses `.` to `-`, so a
  tenant could stop a neighbour's app process.
- **The unattended preview sweep keeps the only record of what it could not
  remove**, honours `preview_ttl_hours = 0` in both of its queries rather than
  one, and carries the row's own domain and port so a crashed preview's vhost
  and certificate are still released. Deleting a git deployment now tears down
  its previews before the foreign-key cascade forgets they exist.

### Added

- `tests/container-identity-pin-e2e.sh` — 38 assertions, 37 of them red against
  v2.54.0.

## [2.54.0] - 2026-08-01

**A stack is a network, a namespace, and an honest status.**

DockPanel's Compose support does not run `docker compose` — it creates each
service directly through the Docker API, which is what lets the panel apply its
own sandboxing to a pasted compose file. What that hand-rolled path has to
reproduce deliberately is everything `docker compose` would have done for free,
and until now it reproduced almost none of it.

Services were created with **no user-defined network**. Every one landed on
Docker's default bridge, where container-name DNS does not exist — so
`postgresql://app:pw@database:5432` could not resolve, and **every multi-service
package the panel ships died on boot**: Domain Watchdog, WordPress, Ghost and
Nextcloud alike. The failure looked like a broken image rather than a broken
deployer, because the deploy reported every service `running` on the strength of
`create` and `start` returning `Ok`, and because our only end-to-end test for
stacks deployed a single nginx and asserted that a UUID came back. A one-service
stack cannot fail this way.

Reported by @insxa on #50 — the same thread the Domain Watchdog package was
shipped for in v2.52.0 — as "always crashing apps, ssl doesnt work properly".
Both halves were accurate, and neither was about his box.

### Fixed — Compose stacks (#50)

- **A stack gets its own bridge network**, and every service is attached to it
  under its compose service key as an alias — the name its siblings were
  actually configured with. A `container_name` chosen by the compose author
  becomes an additional alias rather than the host-level Docker name.
- **Names are scoped per stack.** Container names, the network and named volumes
  all carry the stack's scope, so the `db` service of two packages can coexist;
  before, WordPress, Ghost and Nextcloud all wanted `dockpanel-compose-db` and
  the second package to be deployed failed on a name conflict. An existing
  volume is reused rather than renamed, so an upgrade never orphans data.
- **`command:`, `depends_on:`, `labels:` and long-form `ports:` are no longer
  discarded.** None of the four was declared on the parser's service struct, so
  serde dropped them silently — which turned Domain Watchdog's
  `messenger:consume` worker into a second copy of its web server, and made
  `docker compose config` output deploy with no published port at all. Author
  labels now reach Docker; `dockpanel.*` keys stay the panel's.
- **Services deploy in dependency order**, and over a deterministic iteration
  order — a `HashMap` had been randomising it per process.
- **A service is reported running because it is.** The deploy waits for the
  container to still be up and returns its log tail when it is not, and the
  panel refuses to save a stack where nothing came up.

### Added — a stack can be given a domain and a certificate (#50)

There was no vhost, domain or certificate path for a stack anywhere in the tree:
the deploy request carried no domain field and the stack table had no column for
one, so a deployed stack was reachable only on `127.0.0.1:{port}` from the
server itself. Stacks now take an optional **Domain** and **SSL Email**, served
through the same nginx and ACME path Docker apps use.

The domain is claimed through `services::domain_claim`, and a stack is now
visible to that check as a holder — a path that writes a vhost while being
invisible to the claim system is how one domain ends up with two owners. Removal
takes the vhost and certificates down through the v2.53.0 ownership guard, so a
stack cannot delete a domain a site has since taken over.

### Fixed — the dashboard advertised an update that was not one (#98)

Reported by @brunoDruon: after `update.sh` the banner still read *"DockPanel
v2.49.0 is available (current: v2.52.0)"*. Two independent defects, and the
second one fires on **every install**:

- The render decision was not a version test of any kind. `update_available` was
  set because the stored string was non-empty, next to the current version it
  never compared against.
- Nothing cleared the stored value unless GitHub's latest was byte-equal to the
  running version. `update.sh` never touches the settings table and the first
  poll after a restart is six hours out — so **every operator who upgraded saw
  the version they had just installed advertised back at them for six hours.**

The comparison is now one shared semver answer used by the poller, the boot
reconcile, both API surfaces and the apply guard; the stored advertisement is
reconciled against the running binary at startup, off the network; and applying
a target no newer than the installed version is refused, pointing at
`/api/update/rollback` instead. The old comparator dropped a non-numeric segment
and shifted the rest, so `2.53.0-rc.1` outranked its own GA.

### Fixed — other

- **A failed stack edit no longer replaces the only copy of its YAML.**
  `docker_stacks` has no history table, and the write landed after the redeploy
  and unconditionally — while the agent reports per-service failure inside a
  200. The previous definition is now held until the new one is known to run,
  and redeployed when it is not.
- **Stopped containers are listed on the Logs page.** `docker ps` without `-a`
  excluded exactly the containers anybody opens that page for, and stack
  services had no Logs control at all — which is why bug reports about stacks
  arrive with no detail in them.
- **`POST /api/stacks` runs the container-escape validator.** It guarded the
  agent endpoint the UI does not post to.
- **The package cards work on a plain-HTTP panel.** They minted passwords with
  `crypto.randomUUID`, which is secure-context-only, so on the installer's
  cert-failed branch the card silently did nothing.
- **Domain Watchdog ships its own `APP_SECRET`.** Upstream bakes a published one
  into the image, so every install signed with the same value.

### Testing

- New `tests/compose-stack-pin-e2e.sh` — 43 arms, 42 red against v2.53.0.
  Attacked with 17 evasions, of which three beat the first draft: an alias field
  present but empty, a `confirm_running` whose result was discarded, and an arm
  that matched a label name inside the log line narrating the check rather than
  the check itself.
- `tests/deep-e2e.sh` deploys a **two-service** stack and asserts one service
  resolves the other by compose service key.
- 8 new Rust unit tests over the update comparison, 12 over compose parsing.

## [2.53.0] - 2026-07-31

v2.52.0 answered *may this domain be claimed?* This answers the question after
it: **is the thing I am about to destroy actually mine?**

Nothing owned that question, so every removal in the panel named its target by a
key it had never checked — and the key lied in two different ways.

### Fixed — removing a Docker app could destroy a site's configuration and certificates

An app's domain lives in the `dockpanel.app.domain` container label. Removing the
app deleted, on the strength of that label alone: the nginx vhost, the
certificate directory, both access logs, the Traefik route, and the data tree
named by its `dockpanel.app.name` label. None of it checked that the app still
owned any of those things.

On a box installed before v2.52.0 nothing stopped a site from claiming a domain
an app already held, so by removal time those files routinely belonged to the
site. Every one of these deletes now reads the resource and confirms it names
this container: the vhost must `proxy_pass` to the port the container published,
and the data directory must be one the container actually bind-mounts. What
cannot be proved is left in place and logged, loudly — a stale file is an untidy
box, a deleted one is an outage nobody can attribute.

The same shape, in `services/git_build.rs`, is fired **unattended** by the
preview sweep every five minutes. It is guarded now too.

### Fixed — the panel deleted Let's Encrypt certificates it never issued

Two paths removed `/etc/letsencrypt/live`, `/etc/letsencrypt/archive` and the
`renewal/*.conf` that regenerates them: Docker-app removal, by direct `rm`, and
SSL revoke, via `certbot delete --cert-name`.

**Neither tree issues through certbot.** Certificates the panel provisions go to
`/etc/dockpanel/ssl` through its own ACME client, so every lineage those deletes
could reach had been created by the operator, out of band — and a certbot lineage
carries all its SANs, so deleting the one named `example.com` took `www.` and
`mail.` with it, along with the automation that would have renewed them. On a box
whose mail stack the panel configured, that includes the lineage Postfix and
Dovecot are pointed at, whose documented fallback is the distro's self-signed
snakeoil.

Nothing distinguished a panel-era lineage from an operator's, so there is no safe
narrowing. Both deletes are gone. Revoke now reports that a lineage of that name
exists and leaves it alone.

### Fixed — one tenant could destroy another tenant's systemd unit, jail and crons

Three separate keys were not keys at all:

- **Systemd units and Fail2Ban jails.** `domain.replace('.', "-")` maps
  `a.b.com` and `a-b.com` onto one name, and `-` is legal inside a domain label,
  so both are separately claimable and both resolve to
  `dockpanel-app-a-b-com`. Deleting one site stopped, disabled and unlinked the
  other's app unit, and removed its jail; creating one silently overwrote the
  other's unit with the wrong `WorkingDirectory`, latent until the next reboot.
  Both now read the file — a unit records the docroot it runs, a jail records the
  log it watches — and refuse when it names someone else.
- **Cron sync.** The `# dockpanel:` marker is panel-wide, but the payload was one
  site's rows. So any tenant touching a single cron on their own site stripped
  **every other site's jobs** from the crontab, box-wide and silently: the `crons`
  rows are untouched, so the panel kept listing them as enabled while they never
  ran again. The payload now carries the site's full cron set, and the sync
  removes only the lines whose ids it was sent.
- **The WordPress auto-update marker** was matched as an unanchored substring, so
  toggling auto-update on `example.com` — or simply deleting that site — stripped
  `example.community`'s line and silently ended its core, plugin and theme
  security updates.

### Fixed — a Traefik route removal deleted a different, live domain's route

v2.52.0 made the route filename injective by escaping `-` before mapping `.` to
`-`, then kept deleting the pre-fix name as upgrade cleanup, under a comment
reading *"Only safe because the legacy name is what THIS domain would have been
written as."*

That premise is true and insufficient: the legacy name is also what **another
domain is written as today**. `route_key` doubles a literal `-`, so for a domain
containing none it agrees with the old mangle — the legacy name of `a-b.com` is
exactly the current file of `a.b.com`. Removing the first app took the second
one's route down, on a **fresh install with no legacy files at all**, and the
window never closed. The cleanup now reads the file's own `Host()` rule first.

### Fixed — deleting one site could take the whole box's nginx down at the next restart

A DNS-01 wildcard is provisioned once under the zone apex and every site in the
zone points `ssl_certificate` at that one directory. Deleting any one of them
removed it. Nothing failed at the time, because nginx was already serving from
memory — it failed at the next `nginx -t` anyone triggered, and left nginx down
for **every** site on the box at the next full restart. Four paths shared the
defect; each now checks whether another vhost still references the directory.

### Fixed — site deletion and rename touched other accounts' status-page components

`DELETE FROM status_page_components WHERE name = $1` had no `user_id`, and the
rename path had the same gap on an `UPDATE`. The owning module's own delete keys
on both the id and the account; these two had dropped the tenant filter, so they
reached across accounts and took the monitor links with them by cascade.

### Fixed — a rename overwrote an operator's vhost and deleted it on rollback

`rename_site` is the replacing writer the v2.52.0 `restore_or_remove` retrofit
missed. It wrote the destination config with no snapshot and, when the
whole-server `nginx -t` failed, bare-deleted it — and the destination is a path
`domain_claim` cannot vouch for, because that guard clears a name against
`sites`, `git_deploys` and Docker-app labels and never looks at the filesystem.
A vhost the operator wrote by hand was therefore claimable, silently replaced,
and then destroyed with nothing to restore from.

### Fixed — a slow `crontab -l` replaced root's entire crontab

`read_crontab` collapsed a timeout, a spawn failure and a genuine "no crontab"
into the same empty string, which the caller then wrote back as the complete new
crontab. A single slow read destroyed every operator and system entry on the box.
An empty crontab is now returned for exactly one reason.

### Added

- `services::ownership` — the one place the question is answered, with a
  three-state result. `Unknown` (unreadable, or carrying no marker) does **not**
  permit a delete; the asymmetry is the design.
- `tests/ownership-delete-pin-e2e.sh` — 36 assertions, 27 of them red against
  v2.52.0. §0 measures the comment stripper against its own subjects and C0
  asserts the file list is non-empty, because both of this suite's absence
  sections are green and worthless on an empty subject. Attacked with 24
  evasions; seven beat the first draft, including both classes that beat the
  s294 pin.

## [2.52.0] - 2026-07-31

One question nothing owned — *may this domain be claimed?* — and one nobody
asked — *what was here before I wrote?*

### Fixed — a site update could delete that site's nginx configuration

This is the one to upgrade for, and it has nothing to do with domains.

When the agent wrote a vhost it replaced the file at
`/etc/nginx/sites-enabled/{domain}.conf` and then ran `nginx -t`. If the test
failed it **deleted** the file — under a comment reading *"Invalid config —
remove it and restore"*. No backup was ever taken, so there was nothing to
restore from, and a second, undocumented delete did the same when `nginx -t`
merely timed out.

`nginx -t` validates the **whole server**. An unrelated broken vhost anywhere on
the box fails it. So an ordinary self-service change on a healthy site — switching
PHP version, toggling the WAF, editing security headers — could remove that
site's configuration file, as could the auto-healer and the security scanner
with nobody touching anything. Nginx is not reloaded on that path, so the box
kept serving from memory and the loss only surfaced at the next reload.

All three writers now snapshot the existing configuration first and put it back
on failure, deleting only a file they themselves created. The error now says so.

### Fixed — one guard decides whether a domain may be claimed

Eleven paths could cause a vhost to be written and each carried its own subset
of the checks. `sites.rs` had a shared helper whose comment said it existed
*"so the guard set create() enforces cannot drift"* — and two of the eleven
called it.

- **Docker apps were invisible to every guard.** An app's domain lives only in
  the `dockpanel.app.domain` container label, never in the database, so no SQL
  check could see it. Creating or renaming a site onto a domain an app owned
  passed every check and then replaced the app's vhost. The panel now asks the
  agent, which has been returning that domain all along.
- **`POST /api/apps/deploy` checked nothing at all** — not the reserved
  control-plane domain, not sites, not git deploys. It could be pointed at a
  live site's domain, or at the panel's own hostname. The check now runs before
  the DNS record is created, not after.
- **Renaming a git deploy** skipped both conflict queries that creating one
  performs, so a domain that could not be created could still be renamed onto.
- **Staging environments** — the only tenant-reachable path here — consulted the
  sites table alone, and accepted any domain, not just a subdomain of the parent.
- **Preview deployments** built `{branch}.{domain}` from a pushed branch name and
  claimed it unchecked. A branch called `www` took `www.example.com`. Previews
  now deploy without a vhost rather than over someone else's.
- **The migration wizard** imported client-supplied domains with no format or
  reserved-domain check at all.
- **`EXAMPLE.com` walked past `example.com`.** Domain comparison was
  case-sensitive while the reserved-domain check was not. Domains are now
  normalised at the point of claim.

### Fixed — two Traefik route files could collide

The route filename and router name mapped `.` to `-`, which is not reversible:
`a.b.com` and `a-b.com` both became `a-b-com`. The second app deployed silently
truncated the first one's route, removing either deleted the other's, and the
TLS state reported for one was the other's. Route files written by an older
agent are cleaned up on removal.

### Added

- **Domain Watchdog** as a multi-container stack package (Docker Apps → Compose →
  Packages) — RDAP domain monitoring, four services (#50).

### Testing

- New `domain-claim-pin-e2e.sh`, 44 assertions, 29 of them red against v2.51.0.
  Sixteen deliberate evasions were then run against the fixed tree; two beat the
  first draft — a delete reached through a renamed variable, and a guard whose
  result was computed and discarded — and both arms were re-keyed on the
  property rather than the spelling.
- **A comment stripper shared by five pin suites was deleting code.** `/*` inside
  a string literal (a Dockerfile's `COPY … /app/target/release/*`) opened a block
  comment that ran to the next `*/`: `git_build.rs` lost 485 of 1214 lines and
  `agent/routes/nginx.rs` 118 of 2263. A truncated subject makes an
  absence-checking assertion pass on code the stripper merely removed. Fixed in
  all five; every suite re-verified green.

## [2.51.0] - 2026-07-31

Two ends of the panel disagreeing about something each of them already knew.
Both were found while working through GitHub issues that had gone unanswered,
and both turned out to be wider than the reports that led to them.

### Fixed — the scheme is derived, never assumed (#96)

DockPanel printed and fetched `https://<your domain>` in five places while the
answer was in scope. For anyone terminating TLS at a proxy in front of the
server — where the origin only ever serves plain HTTP — every one of them was
wrong.

- **The Docker Apps link** now follows the scheme the server actually serves.
  The agent reports it per app, read from the vhost it wrote itself, or from the
  Traefik route config when Traefik is the proxy.
- **The uptime monitor created for a new app** was pointed at `https://`
  unconditionally — including on a deploy that had just reported *"SSL
  certificate — Skipped"* on screen a moment earlier. It now follows what the
  deploy reported, so a deploy without a certificate no longer creates a monitor
  that can only ever fail.
- **The post-deploy health check for a git deploy** assumed https, under a
  comment saying git deploys "typically" have SSL. On a deploy without a
  certificate it connected to a port serving no TLS and reported a perfectly good
  deploy as unreachable. The GitHub commit-status link had the same assumption
  and now shares one resolver with the health check.
- **Renaming a site's domain** rewrote its monitor's URL to `https://`. A rename
  should change the domain; it now leaves the scheme alone, which also stops it
  overwriting a URL the operator had set by hand on the Monitors screen.

Added: **"TLS is terminated by an upstream proxy"** on the app deploy form. The
SSL email field was labelled optional but was not — leaving it blank substituted
your account address and requested a certificate anyway, so an operator behind an
external terminator had no way to decline and spent a doomed ACME attempt on
every deploy. The default is unchanged: leave the box unticked and you still get
a certificate.

### Fixed — the Migration Wizard imported into a directory nothing serves (#51)

Found while answering a three-month-old request for more source panels. The
wizard advertises cPanel, Plesk and HestiaCP, and on a realistic archive none of
the three could complete an import:

- **Every imported site landed one level above its own document root.** The
  vhost the wizard creates serves `/var/www/<domain>/public`; the importer copied
  the site's files to `/var/www/<domain>` and its `rsync --delete` then removed
  the `public` directory that had just been created. The result was a site the
  panel reported as imported, serving a document root that did not exist. Both
  halves now ask one function where a site lives.
- **Nested archives could not be found at all.** The analyser steps into the
  single top-level directory that a cPanel full backup and a `cpmove` archive
  both nest everything under; the importer rebuilt the path without that step, so
  it looked one directory too high and failed with "Source directory not found".
  Both sides now resolve the archive root through the same function.
- **Plesk and HestiaCP emitted absolute paths**, which the importer's own
  path-traversal guard rejected — so every site from those two failed on
  "Invalid source directory path" — **and bare database filenames** without their
  directory, which resolved one level too high and failed on "SQL file not
  found".

### Added

- `tests/scheme-and-import-pin-e2e.sh` — 35 assertions, 32 of which fail against
  the previous release. Fourteen deliberate evasions were then written against
  the fixed tree; five walked through the first draft and each one is now closed.

## [2.50.0] - 2026-07-31

Eleven backup defects were recorded as findings at v2.48.6 and left unbuilt.
All eleven were re-verified against this tree before any work started; none had
aged out. Nine are fixed here. The theme is that the backup subsystem was
telling the truth about almost everything except whether it had worked.

### Fixed

- **The Restore Confidence card had been dead on every install since it
  shipped.** Two of its queries selected `server_id` from `backups`, a column
  that table has never had, so both failed at plan time on every request and
  their errors were discarded. The card showed "No recent backups to verify yet"
  forever, and took the verify-lag percentiles, the oldest-unverified age, the
  drill strip and the per-server breakdown down with it. Site backups now reach
  their server through `sites`, as the neighbouring list query always did.
- **A measurement that fails is no longer reported as a measurement of zero.**
  The health response carries `sla_unavailable`, and the card says the figure
  could not be computed instead of rendering its empty state.
- **The stale-backup warning went blank exactly when it mattered.** Its query
  deliberately selects sites that have never been backed up, then decoded the
  resulting NULL into a non-nullable timestamp — and since those rows sort
  first, the failure took the whole list, including sites that were genuinely
  overdue. Never-backed-up is now a state the list can express and render.
- **A backup the panel could not record was still counted as a success.** Five
  writers discarded the result of their `INSERT`. A backup destination deleted
  while a policy was running made every later insert a foreign-key violation,
  and the run still reported green over archives that exist on disk and in no
  list or restore path. All five now report the failure; on the manual path the
  live log says so instead of ticking "Backup created".
- **Clearing a destination's secret field destroyed the stored credential.** An
  empty string was neither encrypted nor recognised as the "keep what is
  stored" sentinel, so it replaced a working key with nothing. Empty secrets are
  refused. Separately, an edit no longer drops stored settings the form does not
  send, such as an SFTP key path.
- **Creating or updating a destination echoed its stored secret back.** Both
  handlers returned the row unmasked while the list endpoint masked it — and on
  panels that predate credential encryption the stored value is the real secret,
  so renaming a destination handed it to whoever was watching the response.
- **Policies could be created and deleted but never edited.** Schedule, scope,
  retention, destination, encryption, verification and enabled were all
  create-once, while the panel's own preflight advice said to edit the policy
  and pick a destination. There is now an Edit control, and the endpoint behind
  it no longer resets `server_id`, `destination_id` or `retention_count` when a
  request omits them.
- **"Protect Everything" now exists.** The preset the Quick Start guide and the
  preflight remediation both tell you to click was fully implemented and had no
  button.
- **Retention that cannot be enforced is reported.** The agent answers that it
  cannot prune an SFTP destination; both callers discarded that answer, so
  remote copies accumulated while the panel displayed the retention count as
  settled. It now reaches the system log, once per run, and the policy form says
  so at configuration time.

### Changed

- **The policy "Encrypt" option is now "Encrypt DB dumps", because that is what
  it does.** The agent has no encryption path for site or volume archives, so a
  ticked box left them in cleartext on disk and at the destination — and since
  v2.34.0 a site archive also contains a dump of every database attached to that
  site, so an encrypted policy shipped an encrypted copy of that data and an
  unencrypted copy of the same data to the same place. The label, the guide and
  the policy form now say what is covered; encrypting those archives for real
  needs agent-side work on the create, restore, list and prune legs.
- Restic incremental backup is marked API-only in the README. It is implemented
  end to end and has no panel UI.

### Added

- `backup-truth-pin-e2e.sh` (35 assertions, CI job `backup-truth`). Its arms are
  keyed on the capability a regression must use rather than on today's spelling;
  34 of 35 fail against the previous tree, and seven deliberate evasions were
  run against it, the last of which found a real gap in the suite itself.
- `docs-claims` now compares the pin suites on disk against the rows published
  in `docs/testing.md`. Every earlier check walked only the rows the page lists,
  so a suite that was never added there was invisible to all of them. The new
  arm immediately found `tier2-pin-e2e.sh`, unpublished since it landed.

## [2.49.1] - 2026-07-31

A security fix. One in-memory map held the progress logs of nine unrelated
features under a single flat set of ids, and the endpoints that read it did not
agree on who was allowed to.

### Security

- **A provisioning log could be read by accounts it did not belong to.**
  `state.provision_logs` is one process-wide map keyed by bare uuids — a site id,
  a backup id, a deploy id and a migration id all live in it together with
  nothing to tell them apart. Five endpoints served from it and each decided
  authorization differently. Two of them got it wrong:

  `GET /api/services/install/{id}/log` took the admin extractor, **discarded the
  claims**, and looked the id up with no ownership test at all. Despite its path
  it is the panel's general progress stream — the UI points backup, restore,
  site-deploy, mail-install and system-update ids at it — so it could not
  authorize by feature, and authorized by nothing.

  `GET /api/git-deploys/deploy/{id}/log` consulted the owner table but **fell
  through when the id was absent from it**, on the stated grounds that the
  not-found below would catch it. It would not: absent-from-owners and
  absent-from-logs are different questions. Only 3 of 16 places that created a
  log ever recorded an owner, so absent was the ordinary case — and this route
  requires only a signed-in session, any role.

  That matters because one of those streams carries a credential on purpose. A
  one-click WordPress install emits the generated admin password in cleartext on
  its `credentials` step, so the operator who did not choose one still gets it.
  The comment justifying that pointed at the owner check on the *sites*
  endpoint — true there, and not true of the siblings reading the same map.

  Measured on a throwaway server running 2.49.0: the site's owner, an unrelated
  tenant with the ordinary `user` role, and a second admin all attached to one
  site's provisioning stream and received **the same 1458 bytes, containing the
  same cleartext WordPress admin password**.

  Ownership now travels with the key rather than with the endpoint. A log cannot
  be created without recording who it belongs to, and there is one read path,
  which refuses any id whose owner is not recorded — so a future feature that
  forgets gets a stream nobody can read, instead of one everybody can. "Not
  yours" and "no such log" answer identically, so the endpoint cannot be used to
  discover which ids are live jobs. The two readers that exempted the admin role
  no longer do: `/api/sites` is scoped per user, so holding that role was never a
  licence to read another tenant's log.

  **If you run more than one account on a panel, update.** No configuration
  change is needed. Reading a log still requires a valid session; there is no
  unauthenticated exposure.

### Fixed

- **Owners of backups and site deploys can read their own progress logs again.**
  The same endpoint required the admin role while the routes that *start* those
  jobs only require ownership, so an ordinary user could launch a backup and then
  be refused the log of the job they had just started. Checking the key's owner
  is both narrower than the old role check and correct for this case.

- **A progress stream that ended without reporting anything is no longer drawn as
  success.** The log component treated any stream that closed before its first
  step as a completed one — a green tick over an empty list. It now says the
  stream ended without reporting, and distinguishes that from a finished job.

- **The owner table is pruned on every sweep.** Pruning had been conditional on
  the hourly expiry having evicted something, but each feature retires its own
  log within 30-60 seconds, so that sweep usually finds nothing and the prune
  rarely ran. Survivable while three call sites recorded an owner; not now that
  every one does.

## [2.49.0] - 2026-07-30

Two long jobs that the panel had been holding an HTTP request open across, and a
capability that was complete except for the one link that made it reachable.

### Fixed

- **Analysing a cPanel backup no longer times out (#91).** The analysis ran
  inline in the request. Walking a real cPanel account is minutes of work, and
  every gateway in front of the panel gives up sooner: Cloudflare at 100s, the
  nginx the installer writes at 300s, and the panel's own request ceiling at
  300s — while the agent call was budgeted 600s. So the connection was torn down
  for exactly the archives the wizard exists to import, and the reporter's
  `Request failed (524)` was this repository's own fallback message rendering a
  Cloudflare error page that carried no JSON to read. The work always carried on
  afterwards; nothing was able to come back and say so.

  `POST /api/migration/analyze` now returns **202** immediately and the verdict
  lands in the migration row — `analyzed` with an inventory, or `failed` carrying
  the agent's own sentence. The wizard polls it, shows elapsed time, and picks a
  running analysis back up after a reload, so closing the tab no longer loses
  sight of a job that is still going. Raising a timeout was available and would
  only have moved the archive size at which this breaks.

- **Unpacking an archive is bounded on the side doing the unpacking.** Neither
  side of the socket had a limit on `tar`, so a damaged member or a stalled disk
  could only end by a gateway hanging up on the browser. The agent now owns a
  30-minute ceiling on extraction, kills a timed-out `tar` rather than merely
  stopping waiting for it, removes the scratch directory, and names the archive
  and the budget in the error. The panel's budget sits deliberately above the
  agent's, so the sentence the operator reads is the one written by the side that
  knows what happened.

- **A restart no longer leaves a migration spinning forever.** Analysis and
  import both run in a spawned task, which a restart takes with no chance to
  write a verdict — leaving a row claiming to be in progress and a UI that could
  never clear it. On boot nothing is running, so anything still claiming to be is
  closed out and says why.

- **The PHP version picker can install a PHP version.** Reported by surprises29,
  who expected switching version to offer the install. The version-specific
  installer has existed end to end since v2.8 — the agent route, deb.sury.org for
  Debian, `ppa:ondrej/php` for Ubuntu, module streams for the RHEL family, and a
  backend proxy — and **nothing in the browser had ever called it.** Both pickers
  offered five versions with no idea which of them the server had, and choosing
  one it did not have failed the switch, or the whole site creation, at the agent.

  Both now read the live list, label what is missing, and offer to install it,
  with the install's log streamed as it runs. The switch is held back until the
  version actually works rather than being fired and refused.

- **The advice that refusal used to give was a dead end.** It sent people to
  Settings → Services, which has one PHP tile with no version on it, reports
  installed if *any* version is, and therefore shows a box that already has 8.4
  an "Uninstall" button and no install button at all — over a route that takes no
  version and installs whatever the distro offers. It now names the control that
  can do the job.

- **Installing a service is no longer capped at 60 seconds.** `install_service_with_log`
  used the untimed agent client over an operation the agent budgets 300s for,
  plus repo configuration outside that clock. Nothing was cancelled by it: the
  install finished while the panel wrote *"agent request timed out after 60s"*
  into the log the operator was watching. This affected **every** service install,
  not just PHP — the s289 defect again, at a different call site.

- **An install that leaves PHP unusable is no longer reported as success.** The
  already-installed branch answered from the package database, while the guard
  that sends people there tests the **socket**. A box whose `php8.3-fpm` was
  installed and stopped was told "already installed" and then failed the very
  next action with the same message. All three install paths now start the unit
  and judge the outcome on whether the socket appears.

- **A test arm that had stopped being able to fail.** `docs-claims` verifies that
  the testing page's summary sentence matches the table under it. Its list of
  English number words stopped at "twelve" while the table grew past twenty, so
  the suite-count half silently skipped its own check and agreed with whatever
  was written. Restored — and it immediately caught that the same check could not
  read a hyphenated number either, having been reading "Twenty-two" as "two".

### Added

- Two regression pins, 57 assertions, both watched failing against the pre-fix
  tree: `migration-analyze-async-pin-e2e.sh` (28) and
  `php-install-from-picker-pin-e2e.sh` (29).

## [2.48.6] - 2026-07-30

A backup that the panel calls off-site is now actually off-site. v2.48.5 fixed the
backup credential path and proved **Test Connection**; it never watched a backup
**arrive**. Driving one to the destination found six defects underneath, and four
of them make the panel state something false about data protection.

### Fixed

- **A redirect is no longer reported as a completed upload.** The S3 uploader
  passed `curl --fail`, which fails on 4xx/5xx and says nothing about **3xx** —
  and without `-L`, curl does not follow a redirect: it transfers nothing and
  **exits 0**. A bucket addressed in the wrong region, or an `http://` endpoint
  the provider redirects to `https://`, therefore produced a successful upload
  with no object written, the agent answering `{"success":true}` and the panel
  lighting the **remote** badge. Measured against a redirecting endpoint: a full
  policy run reported *"1 successes, 0 failures, 0 not uploaded off-site"* while
  the bucket stayed empty. Success is now an explicit 2xx. Following the redirect
  is deliberately **not** the fix — an AWS SigV4 signature is bound to the host
  and path it was signed for, and curl downgrades `PUT` to `GET` on 301/302. The
  same latent bug was in `test_s3`, `list_s3` and `delete_s3`; all four now share
  one runner rather than four copies of the flags.

- **A scheduled backup records where its bytes went.** `backup_scheduler`
  computed its upload result into a discarded `_`-prefixed binding and inserted
  five columns, omitting `uploaded` and `destination_id` — the two columns the
  migration had added for precisely this. Every per-site scheduled backup that
  *did* go off-site was filed as local-only, so the **remote** badge could never
  appear for the path most people configure, and nothing recorded which
  destination held the copy. Measured: the SFTP archive sitting at the
  destination while its row read `uploaded=f, destination_id=NULL`.

- **An upload slower than a minute is no longer treated as a failure.** The panel
  capped every agent call at 60s while the agent budgets 600s for this exact
  upload. An off-site copy that took longer — a 12MB archive on a slow uplink was
  enough to measure — had the panel give up while `curl`/`scp` kept running, so
  the bytes landed and the panel then re-sent the whole file twice more, recorded
  it local-only, tripped the destination breaker so every remaining site,
  database and volume in that run skipped off-siting too, and raised an incident
  saying the backups *"exist only on this server"*. Both upload call sites now
  use `post_long` with a budget that outlasts the agent's, so the agent's own
  error surfaces instead of a bare timeout.

- **`sshpass` is installed.** It is the only path a password-authenticated SFTP
  destination can take, and password is the mode the destination form offers
  first — but nothing installed it, so on every fresh install those destinations
  failed at Test Connection with an opaque `502`. v2.48.5 measured SFTP as
  working only because the test rig had installed `sshpass` itself. The installer
  now installs it (best-effort: it comes from EPEL on RHEL-family, and making it
  mandatory would turn a missing repository into a failed install), `openssh-client`
  is declared rather than assumed, and when it is genuinely absent the agent now
  names the binary and the remedy instead of reporting `os error 2`.

- **Test Connection exercises what an upload needs.** The S3 test issued `HEAD`
  on the bucket **root** while an upload `PUT`s into the **prefix** — a different
  permission at a different path, so read-only keys and prefix-scoped keys both
  reported green. It now writes a probe object where the backups will go and
  removes it. The SFTP test connected and ran `exit`, never touching
  `remote_path`; and nothing ever created that directory, which `scp` will not do
  — so a destination whose path did not already exist passed Test and failed
  every upload with *"No such file or directory"*. The default is `/backups`,
  which exists on almost no server. The upload now creates the directory, and the
  test creates and probes it.

- **One `known_hosts`, not two.** v2.48.5 pointed the backup uploader's ssh at
  `/var/lib/dockpanel/known_hosts`, when `deploy.rs` had already established
  `/etc/dockpanel/known_hosts` for git deploys, for exactly the same reason and
  with a docstring saying so. Two trust stores means a host pinned by a git
  deploy is an unknown host to a backup upload. Aligned to the existing path.
  (Both are writable under the unit; this is consistency, not a failure.)

### Changed

- **The ssh options for backup uploads are built in one place.** `upload_sftp` and
  `test_sftp` each built the list by hand under a comment promising they were
  "kept in step deliberately". They were not — `ConnectTimeout` was set on the
  test and missing from the upload — and a test that connects differently from
  the upload is a test of nothing. One builder now serves both.

### Added

- **CI check: `backup-lands`.** A source pin over all six defects above, watched
  failing against the pre-fix tree (29 of 31 arms) before being trusted.


## [2.48.5] - 2026-07-30

SFTP backup destinations have never worked. Both reasons were found by standing
one up and pressing Test — neither is visible in the code that implements SFTP.

### Fixed

- **An SFTP destination can be reached at all.** `ssh` was invoked with
  `StrictHostKeyChecking=accept-new`, which means "trust on first use, then pin"
  — and pinning is a **write**, to `~/.ssh/known_hosts`. The agent runs with
  `ProtectHome=yes` and `ProtectSystem=strict`, so that path does not exist and
  cannot be created:

  ```
  Could not create directory '/root/.ssh' (Read-only file system)
  ```

  Every SFTP destination failed there, before it opened a connection — so none
  could be tested and none could be uploaded to. `known_hosts` now lives at
  `/var/lib/dockpanel/known_hosts`, which the unit already allows. First-use
  pinning is kept; `StrictHostKeyChecking=no` would have "fixed" this by throwing
  away host verification.

- **Password authentication works.** The same commands also passed
  `BatchMode=yes`, which disables password and keyboard-interactive
  authentication outright. `sshpass` was therefore supplying a password that
  `ssh` had already refused to offer, and the server answered
  `Permission denied (publickey,password)`. Two settings that are each correct
  alone and cancel each other out. `BatchMode` is now sent only when
  authenticating by key, where it belongs — password auth stays non-interactive
  because sshpass supplies it over a pty and `ConnectTimeout` bounds the attempt.

  Measured both ways against a live endpoint: with the flag, denied; without it,
  connected.


## [2.48.4] - 2026-07-30

Two defects that only became visible once something finally exercised the code:
one endpoint that had never had a caller, and one feature nobody could see the
error from.

### Fixed

- **Editing a backup destination no longer destroys its stored credential.**
  `PUT /api/backup-destinations/{id}` merged the mask sentinel by copying the
  **already-encrypted** value into the config and then encrypting the whole
  object again. `encrypt_config_secrets` skips only `""` and `"********"`, and a
  stored ciphertext is neither — so the secret was encrypted twice, one decrypt
  on the way out returned ciphertext, and the destination authenticated with
  gibberish while the row looked perfectly normal.

  It had never fired because that endpoint had no caller anywhere in the panel
  until v2.48.3 gave the Destinations tab an Edit button. The first edit of any
  destination would have corrupted it. The order is now inverted — encrypt what
  arrived, then carry masked fields across verbatim — and a masked field with
  nothing stored behind it is dropped rather than passed through as the literal
  string `********`.

- **SSH hardening works at all** (reported on #92). Disable root login, disable
  password authentication and change SSH port all rewrite
  `/etc/ssh/sshd_config`, and `/etc/ssh` was missing from the agent unit's
  `ReadWritePaths` while `ProtectSystem=strict` is in force. Every one of them
  failed with `Failed to write sshd_config: Read-only file system (os error 30)`
  and the panel answered 502 — on **every install since the sandbox landed**.

  This is the third time this exact enumeration miss has shipped (`/etc/apt`,
  `/var/spool/cron`, now `/etc/ssh`), so the unit now carries a regression pin
  rather than another comment: `tests/agent-sandbox-paths-pin-e2e.sh` derives the
  paths each `safe_command`-adjacent writer touches and fails when one is absent
  from the unit. Updating reinstalls the unit, so `scripts/update.sh` is enough.

  Worth noting the failure was only *legible* because v2.48.2 stopped flattening
  the agent's message; before that it read "Agent offline".

## [2.48.3] - 2026-07-30

Two reports, one shape: a feature whose halves were both present and were not
connected to each other. Neither was a missing capability — in both cases the
work had been done and something in between dropped it on the floor without
saying so.

### Fixed

- **Git Deploy now passes the UI's environment variables to the container**
  (#94). The panel sent them under the key `env_vars`; the agent's handler
  declared that field as `env` with `#[serde(default)]`. serde discarded the key
  it did not recognise and defaulted the one it did to an empty map, so the
  container started with no environment and every layer reported success. Only
  `docker inspect` disagreed.

  This was never specific to the Dockerfile build method. It affected Nixpacks
  too, and redeploys, and rollbacks, and PR previews — all five call sites.
  Nixpacks merely *looked* correct because the build endpoint receives the
  variables separately and bakes them into the image. Docker Apps was unaffected
  because its two sides happen to agree on `env`.

  The agent now accepts either spelling, so a panel and an agent on different
  versions still understand each other. All five bodies are built by one
  function whose parameters are not optional.

- **A crashed Git Deploy container no longer rolls back unconfigured and
  unbounded.** The auto-rollback path built its own request from four fields and
  had lost three: it sent no environment under either spelling, and no
  `memory_mb` or `cpu_percent`. A container that crashed and rolled back came
  back with no environment *and no resource limits* — on the one path nobody
  watches, because it runs unattended after a failure.

- **Backup destinations can be added from the panel again** (#93). The S3/SFTP
  form — create, list, test, delete — was written in Settings, then switched off
  in place behind `{false && (…)}` by the commit that moved the Destinations tab
  to Backup Manager. That move brought across the list and the Test button and
  left create, update and delete with no caller anywhere in the panel. The empty
  state went on telling operators to "add one via Settings", which was by then
  the one screen the control had been taken from; the guidance layer told them to
  add a destination and linked to a tab that had no way to.

  Backup Manager → Destinations now creates, edits, tests and deletes, and the
  page honours `?tab=` so the links the panel already emitted land where they
  say. Secret fields come back masked and stay stored unless retyped.

- **Scheduled backups can actually authenticate to their destination.** Three
  places hand a destination to the agent and only one of them decrypted first.
  Test Connection used the decrypting path; the per-site scheduler and the policy
  executor both cloned the stored row and posted the **ciphertext** as the S3
  secret key or the SFTP password. A destination therefore verified green and
  then failed every real upload. All three now share one helper.

- **A destination can be attached to a per-site backup schedule.** The check
  inner-joined `servers` on `backup_destinations.server_id` — a nullable column
  that `create` never populates, so it is NULL on every destination the panel has
  ever made. An inner join on an always-NULL column matches nothing, so the
  endpoint answered `403 Destination not found or not owned by you` for every
  destination that existed. Unscoped destinations are now accepted; ones pinned
  to a server still have to belong to the caller.

### Known issues

- PR preview containers still run without the parent app's memory and CPU limits.
  That predates this release and is left unchanged deliberately rather than
  altered as a side effect of the rollback fix.
- Nixpacks still passes environment variables to `nixpacks build --env`, so
  secrets land in image layers and are visible to `docker history`. Runtime
  injection now works, which makes the build-time copy redundant; removing it is
  a separate change.

## [2.48.2] - 2026-07-30

Three people reported three different bugs this week. All three had been told
"Agent offline — the DockPanel agent is not responding" by a panel whose agent
was answering perfectly clearly.

### Fixed

- **An agent that refuses a request is no longer reported as an agent that is
  down** (#90, #91, #92). `agent_error()` accepted `impl Display`, so
  `AgentError::Status(code, body)` — carrying the agent's real status and its
  real sentence — was formatted into a string and collapsed into one generic
  `502` at 244 call sites. The frontend then rewrote *any* 502 into the
  "agent offline" message. So `Path must be within /var/backups/ or /tmp/`,
  `WAF not installed. Install it from the Services page first.` and a complete
  `nginx -t` diagnostic all reached their operators as the same dead end.

  A 4xx from the agent now keeps its status and its message. Only a transport
  failure or an open circuit breaker — the cases where nothing answered — carry
  the `agent_unreachable` marker the UI keys off. Agent 5xx still returns a
  generic message with an incident id, so internals are not leaked.

- **Stripe and Cloudflare failures no longer blame the agent.** 27 call sites in
  `billing.rs` and `dns.rs` routed third-party HTTP errors through the agent
  path; `billing.rs` makes no agent call at all. They now report which upstream
  actually failed. Enforced by the type system rather than by review:
  `agent_error` takes `AgentError`, which a `reqwest::Error` is not.

- **The WAF install no longer leaves nginx unable to start** (#92). The
  installer copied `unicode.mapping` from `/usr/share/modsecurity-crs/`, and
  when that was absent wrote a **zero-byte file** — which ModSecurity cannot
  parse. Debian 13's `libmodsecurity3t64` ships no `unicode.mapping` anywhere,
  so every Debian 13 install took the empty-file path, reported success, and
  failed `nginx -t` the first time a site actually enabled the WAF. The mapping
  now rides the binary (`include_str!`), and the installer verifies the file it
  just wrote defines the code page `modsecurity.conf` names.

- **The Migration form stops suggesting a path the agent rejects** (#90). The
  Backup File Path placeholder read `/home/user/backup-…` while the agent only
  accepts `/var/backups/` or `/tmp/`. The field now shows an accepted location
  and states the constraint.

### Known issues

- **#91 (Migration `524`) is only half addressed here.** The misleading message
  is fixed, but the underlying cause — backup analysis runs inline in the
  request and can outlive the gateway timeout on a large archive — needs the
  async 202-and-poll treatment used by the restore and update paths. Tracked on
  the issue.

### Added

- `tests/agent-error-propagation-pin-e2e.sh` (21 assertions, CI job
  `agent-error-contract`), proven to fail 20 of 21 against the pre-fix tree.

## [2.48.1] - 2026-07-29

A clean 16-package upgrade reported itself as failed, because the update killed
the process that was reporting on it.

### Fixed

- **The agent no longer gets restarted out from under its own update.** A
  panel-driven update runs apt from inside `dockpanel-agent`; because the agent
  links libc, an ordinary `libc6` upgrade put `dockpanel-agent.service` on
  needrestart's restart list. needrestart then SIGTERMed the process that was
  streaming that very update's progress. apt itself was untouched — it runs in a
  `systemd-run` transient scope — but the NDJSON stream died before its final
  `done` line, so the panel reported a fully successful upgrade as *"Update
  completed with errors"*.

  needrestart already declines to restart the unit that invoked it, precisely to
  avoid this; the escape hatch that lifts apt out of the agent's sandbox also
  lifts it out of the agent's cgroup, so that protection never applied.
  `setup.sh`, `update.sh` and `install-agent.sh` now drop a needrestart override
  excluding only `dockpanel-agent` — every other service still restarts, which is
  the part that matters for security. The agent picks up new libraries on the
  next reboot or agent restart.

- **Streamed operations are now bounded by silence, not by total runtime.** The
  update stream carried a flat 300s cap, and the remote client additionally
  inherited reqwest's 60s total-request timeout. Neither can be set correctly for
  a stream: `apt-get upgrade` finishes in 40s on a fast box and can legitimately
  run far longer on a slow link with a full release of packages, so long updates
  were failed while apt carried on underneath. The bound is now 10 minutes of
  *no output*, rearmed on every line.

- **The update terminal reconnects instead of silently going dead.** A dropped
  SSE connection closed the stream and left the page frozen mid-output with no
  explanation. The backend already replays the whole log on reconnect, so the
  terminal now retries, rebuilds itself from the replay, and — if it really
  cannot reattach — says so in amber, making clear the update is still running
  on the server rather than implying it failed.

- Finished updates' logs are kept 15 minutes instead of 60 seconds, so an
  operator who lost the connection can still come back and find the result.

- `install-agent.sh` also now writes the apt lock-wait drop-in that `setup.sh`
  has always written, so fleet members stop failing agent-driven installs that
  race unattended-upgrades.

- The remote client records a successful round-trip only once a stream has
  actually completed; it previously counted one the moment response headers
  arrived, so a connection that died mid-body still looked healthy to the
  circuit breaker.

## [2.48.0] - 2026-07-28

Fourteen settings the panel read but no screen could set — including the OAuth
client credentials behind the sign-in buttons — now have controls, and the check
that was supposed to have prevented that now exists.

### Added

- **OAuth sign-in is configurable from the panel.** The client ID and secret for
  Google, GitHub and GitLab were writable through the settings API, masked on
  read, encrypted at rest and consumed by the login route — every part of the
  path built except a screen to type them into. Settings → Account now carries a
  card per provider, with the state of each shown plainly (Active / Incomplete /
  Not configured).

  It also prints **the exact redirect URI to register at the provider**, fetched
  from the server rather than composed in the browser: the panel builds that URI
  from `BASE_URL`, and an admin browsing on a different address would otherwise
  be shown, and would register, a URI the panel never sends. When `BASE_URL` is
  unset the card says so — until now that produced a relative redirect URI, which
  no provider accepts, with nothing anywhere explaining why sign-in failed.

  Saving a client ID without a secret is refused rather than accepted. The public
  branding endpoint lists a provider as soon as its client ID is non-empty, so a
  half-configured save puts a working-looking button on the login page for every
  logged-out visitor, which then dead-ends at the callback.

- **Notification templates have an editor.** The four `notif_template_*` keys are
  read on every alert and substituted into; they had no control, so the feature
  was reachable only by whoever could hand-craft a PUT. Settings → Alert Channels
  now has one field per channel, with the four supported placeholders listed.

- **Stripe plan price IDs have inputs.** Checkout answered "Price not configured
  for the pro plan — set it in settings" while no page could. The card also
  reports when `STRIPE_SECRET_KEY` is absent from `api.env`, because that half of
  billing is not a panel setting and the price IDs do nothing without it.

- **A white-label toggle.** `hide_branding` was returned by the branding endpoint
  and honoured by the sidebar, the header and the login page — a switch three
  surfaces obeyed and no screen could throw.

### Fixed

- **The verdict of a failed update is no longer the last thing on the page.** The
  update and rollback result cards rendered inside the Snapshots panel, seventh
  of seven on the Updates tab, so an operator whose update had just failed
  scrolled past six cards to reach the sentence explaining why the panel was
  stopped. They are now first, ordered by time so the newer of the two leads.

- **`ALLOWED_KEYS`' comment claimed a test that did not exist.** It said the pin
  suite "fails when [an entry has no control]". The suite discovered the
  frontend's key list into a variable and never read it — so the drift it named
  was the drift it could not see, and fourteen keys accumulated behind a green
  run. The arm now exists and fails against the pre-fix tree naming all fourteen.

  It tests that the frontend *sends* a key, not that it mentions one: a read is
  not a control, which is exactly how `hide_branding` stayed invisible while
  three components read it. Keys a purpose-built route writes instead — the
  nginx/Traefik selector, which posts to `/traefik/install` — are discovered from
  the backend rather than exempted by name.

## [2.47.4] - 2026-07-28

Closes the two remaining defects in the update subsystem. One could silently undo
a rollback; the other made a failed update look like an update still running.

### Fixed

- **A failure while finishing a rollback could quietly undo that rollback.** The
  restore reverts the database, then puts the previous binaries back. Between
  those two steps it verifies the restored schema and records the rollback — and
  if either step failed, the exit trap restarted `dockpanel-api` to avoid leaving
  the panel down. But the binary it restarted was still the *newer* one, so it
  migrated the just-reverted database forward again. The panel came back on the
  schema the operator had rolled away from, while the result file reported the
  rollback as FAILED.

  Three changes. The steps after the database commits now report rather than
  abort, because once the transaction has applied there is nothing left to
  protect by stopping and a box stranded between two versions to lose. The exit
  trap now refuses to start the API while the database has been reverted and the
  binaries have not, leaving it stopped deliberately and naming the pre-rollback
  dump to recover from — the panel being down is obvious and reversible, a
  database migrated forward behind a failed-rollback verdict is neither. And the
  API binary is restored first, so the window is as short as it can be.

- **An update that failed early reported nothing at all.** `update.sh` re-execs
  itself into a transient systemd unit so it survives stopping the service it was
  launched from. That hands off in about 29ms while the real update runs for
  around a minute, so the panel was reading the exit status of the handoff rather
  than of the update. For an update that succeeds this is harmless — the panel is
  restarted and works out what happened on the way back up. For one that fails
  *before* the services are stopped, such as a bad download or a failed database
  backup, nothing is ever restarted, so nothing ever reported it: the operator
  watched an update sit in progress, frozen on its first log line, until the
  fifteen-minute window expired.

  `update.sh` now records its outcome to `/var/lib/dockpanel/last-panel-update.json`
  on every exit path — including aborts and signals — and the panel reads that
  instead of inferring from the process it launched. A completed in-flight
  rollback is recorded distinctly from a failed one, since a panel healthy on its
  previous version does not need anyone woken up. The outcome is shown in
  System → Telemetry beside the rollback verdict.

### Testing

- New `tests/update-rollback-pin-e2e.sh` (36 assertions, CI job `update-rollback`).
  It drives `restore-snapshot.sh` end to end against a scratch tree with stubbed
  services, so the guard that decides whether to leave a panel stopped is
  executed rather than merely read. Verified against the previous release's
  behaviour, where 15 of its assertions fail — including "the API was not
  restarted", which fails there precisely because the old trap restarts it.

## [2.47.3] - 2026-07-28

Corrects a defect in 2.47.2's own migration — the mechanism that carries 2.47.2's
fix to servers that are already running.

### Fixed

- **The `/assets/` migration could skip the vhost it was meant to repair.** After
  rewriting the block it verified its work by searching the *whole file* for the
  `expires` directive, on the reasoning that the directive should be gone. But
  `expires` can legitimately appear in another location block — a hand-added
  `/media/`, for instance — and the check cannot tell which block it came from.
  On such a server the verification fails, the rewrite is discarded, and the
  updater reports `failed to rewrite the /assets/ block — skipped`.

  The check now reads only the block it just wrote. The failure was already in
  the safe direction — nothing is changed and the skip is logged, so no server
  was left with a broken configuration — and a vhost generated by the installer
  has no other `expires`, so reaching it took a hand-edited file. It is corrected
  because a fix that silently does not arrive is the failure this release series
  exists to address.

### Added

- Eight further assertions in `tests/nginx-headers-pin-e2e.sh` covering the two
  server populations the suite had not: one already running 2.47.1, where the
  document block exists and must be neither skipped nor duplicated, and one with
  `expires` in an unrelated location. Both had been checked by hand, which is
  precisely the state this project keeps finding does not hold.

## [2.47.2] - 2026-07-28

A security release for a header that was missing from exactly the responses it
exists to protect, plus the first regression pin for the whole nginx contract.

### Fixed

- **Every panel served its JavaScript and CSS with no `X-Content-Type-Options:
  nosniff`.** The header was declared at the server level and looked correct
  anywhere you cared to check — but the `location /assets/` block set a cache
  header of its own, and in nginx a location block's `add_header` directives
  *replace* the server block's set rather than merging with it. So the one
  directory whose responses are scripts was the one directory that lost the
  header telling a browser not to guess at their type. `/assets/` carried none
  of the seven headers the rest of the vhost sends.

  This is measured, not estimated: served through a real nginx, the generated
  panel vhost returned a bundle with zero security headers. The same shape was
  in `panel/frontend/nginx.conf` (three of its five) and
  `website/client/nginx.conf` (none of its four). All three now repeat their own
  server block's set in full.

- **`/assets/` responses carried two contradictory `Cache-Control` lines.**
  `expires 1y` emits its own (`max-age=31536000`) on top of the block's
  `add_header Cache-Control "public, immutable"`, so the response went out with
  both, saying different things. One directive replaces both, and it is
  deliberately not `always`: an error response must never be cached, or a 404
  during an update is remembered for a year.

- **The fix reaches boxes that are already running.** As in 2.47.1, a change to
  `setup.sh` alone would reach only installs created after it, because the
  updater never re-runs the installer. `update.sh` gained a second in-place vhost
  migration; like the first, the headers it writes are parsed out of the vhost
  being migrated rather than taken from the script, so an older install keeps its
  own policy instead of being silently handed today's on one class of response. A
  vhost with no server-level header is skipped and logged rather than guessed at,
  and the migration is idempotent.

### Added

- **`tests/nginx-headers-pin-e2e.sh`** — the response contract is now pinned, on
  both surfaces that can produce it: the template `setup.sh` writes at install
  time, and the migration `update.sh` applies to a box that installed before the
  fix existed. It serves them through a real nginx, because this defect class is
  invisible in the source — a location block that reads fine in isolation
  silently strips six headers off the response.

  Every serving assertion proves HTTP 200 *before* reading a header. The harness
  written for 2.47.1 returned 403 for every request and eleven of its twelve
  assertions still passed, because these headers are declared `always` and nginx
  emits them on error pages too. Runs in CI as `nginx-contract`.

- **`deploy/nginx/`** — the vhosts for the project's own three public surfaces,
  under version control for the first time, with a check mode that detects drift
  against the box. They had been hand-edited files with no copy anywhere, which
  is how the 2.47.0 document-cache fix came to be applied to one of them and not
  to the other two that had the identical defect.

## [2.47.1] - 2026-07-28

A delivery release. The fix it carries was written last session and, on its own,
could never have reached a panel that was already running — which was every
panel except the ones installed after it landed.

### Fixed

- **The panel served `index.html` with no cache directive, so an update could be
  invisible to the operator who applied it.** With nothing set on the document, a
  browser falls back to *heuristic* freshness — roughly a tenth of the file's age
  — so on a panel that has been up a month it will not ask the server about that
  page for about three days. What it keeps serving in that window names the
  previous hashed bundle under `/assets/`, which is still on disk, because the
  frontend untars over the directory rather than replacing it. So nothing 404s
  and nothing white-screens: the operator updates, is told it worked, and goes on
  running the old frontend against the new backend. A stale panel that looks
  healthy is harder to notice than a broken one.

  The install template gained the fix immediately, and new installs have had it
  ever since, because the installer tracks `main`. **Existing boxes had no path
  to it at all** — `setup.sh` writes the vhost only at install time and the
  updater never re-runs it. `update.sh` now migrates the panel vhost in place,
  the same way it has handled previous nginx changes. The headers it repeats are
  copied from the vhost being migrated rather than from the script, because a
  location block's `add_header` directives *replace* the server block's set
  instead of merging with it: writing today's list would have stripped the CSP
  off the one response that carries it, and would have quietly given an older
  install a different policy on a single response. A vhost with no server-level
  header is skipped and logged rather than guessed at.

  Verified by serving the migrated config with a real nginx: `/index.html`, the
  SPA root, and a deep client route all return `Cache-Control: no-cache` with all
  seven security headers intact, and `/assets/` stays `public, immutable`. The
  migration is idempotent.

## [2.47.0] - 2026-07-28

Found by driving v2.46.0 on throwaway servers rather than reading it: a fresh
install, an older release upgraded in place, and a two-server fleet. 2.46.0 gave
the panel IP allowlist real range matching and a control — this release makes it
apply to every door that can sign you in, and stops an upgrade from locking your
users out.

### Fixed

- **Upgrading could lock every non-admin user out of the panel.** Auto-lockdown
  counts "N suspicious events within M minutes". Before 2.46.0, ingestion of
  those events sat under `auto_heal_enabled`, which is seeded **off**, so on a
  stock install the agent wrote them to
  `/var/lib/dockpanel/suspicious-events.jsonl` and nothing ever drained the file.
  2.46.0 correctly detached security monitoring from that switch — and the first
  tick after upgrading then read the entire accumulated backlog and stamped every
  line with the time it was *read* rather than the time it *happened*. However
  long the queue took to build, it counted as one instant. Any box that had ever
  seen five suspicious commands locked down immediately, for 24 hours.

  Driven on an upgraded box: sixteen events generated over about three minutes of
  real use were recorded 105 milliseconds apart, lockdown engaged, and a
  non-admin got `503 System is in lockdown mode`. The agent has always written a
  per-event timestamp; the ingest now honours it, so an old backlog lands outside
  the window and a genuine burst still trips the rule. **If you upgraded to
  2.46.0 and were locked out, this is why** — the guide's recovery SQL clears it,
  and upgrading to 2.47.0 stops it recurring.

- **The panel IP allowlist only guarded the password login.** `allowed_panel_ips`
  gates access to the panel, but the check lived inline in one handler, and three
  other endpoints mint exactly the same session cookie: passkey authentication,
  the OAuth callback, and the second step of a 2FA login. An operator who
  restricted the panel to their office range still had those answering from
  anywhere. Verified from an excluded address on a real box: the password door
  returned `403`, while `passkey/auth/begin` returned `200` and a usable WebAuthn
  challenge. All four doors now share one implementation, and a regression pin
  discovers session-minting handlers from the source and fails if one of them
  lacks the check — so a fifth door cannot be added without it.

- **Lockdown did not apply to the OAuth login path.** Lockdown holds non-admins
  out until it expires or an admin clears it. The password and passkey paths
  enforced it; the OAuth callback had no such check, so with SSO configured a
  lockdown did not cover that route. It now enforces it, with the same admin
  escape hatch as the other doors.

### Added

- **The session-recording toggle now says which servers it does not control.**
  Recording is one setting, but each server's agent enforces it: the decision
  travels as a signed claim in the terminal ticket, and an agent older than
  2.46.0 does not read that claim and keeps recording. Switching recording off
  therefore made a fleet-wide promise that was false for any member still behind
  — the same shape as the toggle that reported success while changing nothing.
  Confirmed on a two-server fleet: with the panel asking for `record=false`, the
  2.46.0 agent wrote no `.cast` file and a 2.45.1 agent on the same box wrote one
  anyway. **Settings → Security** now names any server whose agent predates the
  gate, so an operator can see the gap instead of being told it does not exist.

## [2.46.0] - 2026-07-28

Every operator setting is a claim made in three places — the code that reads it,
the API that decides it may be written, and the control that renders it. Nothing
made those three agree, and they had drifted in both directions.

### Fixed

- **Terminal session recording ignored its own toggle.** The switch saved,
  reported "Session recording disabled", and changed nothing: the agent opened a
  `.cast` file for every session unconditionally and the panel had no way to tell
  it otherwise. The decision now travels as a signed claim inside the terminal
  ticket — deliberately not a query parameter, since the browser holds that
  ticket and connects to the agent directly, and a parameter would let a user
  switch off the recording of their own session. Agents older than 2.46.0 ignore
  the claim and keep recording; update the agent for the toggle to take effect
  on a fleet member.

- **Canary file monitoring ignored its own toggle** in the same way, and the
  monitoring itself was gated by the wrong switch. Suspicious-event ingestion,
  auto-lockdown expiry and canary checks all sat under `auto_heal_enabled`, so
  turning off auto-healing silently turned off security monitoring nobody had
  asked to stop. They now run on their own.

  **Upgrade impact, worth reading before you update.** On a box with auto-healing
  **off**, those three were dormant — and this release wakes them. Suspicious
  events start being counted, so **auto-lockdown can now fire where it never
  could before**, and lockdown blocks non-admin access until it expires (24h) or
  an admin clears it. That is the intended behaviour — a security control should
  not be switched off by an unrelated setting — but it is a change in what your
  box does. If you do not want it, set the auto-lockdown threshold deliberately
  in **Settings → Security Hardening** rather than leaving auto-healing off as an
  accidental kill switch. Verified on our own demo, where auto-healing was off:
  the monitoring came up with the upgrade and locked down on the first burst of
  suspicious events.

- **The panel IP allowlist could not be set, and rejected the ranges it
  documented.** `allowed_panel_ips` gates login, and the guide told operators to
  set it in Settings — where no control existed and the API answered `400
  Unknown setting`. It also compared the client address to each entry as a
  string, so the CIDR ranges the same guide promised matched nothing and locked
  the operator out rather than restricting access. It now matches by range for
  IPv4 and IPv6, validates every entry on save so a typo cannot lock you out,
  and has a control. It fails closed when the proxy sends no `X-Real-IP`; the
  guide now says so, and how to recover.

- **The site-creation rate limit was disabled by its own seed.**
  `security_site_rate_limit` is seeded `3` and was read as a boolean, so `3` was
  not `true`, and the ceiling became 999 per hour. An install without the row
  limited to 3; an install with it did not limit at all. It is read as the count
  it always was, `0` turns it off, and it has a control.

- **Exporting a config and importing it dropped your security posture.** The
  writable-key whitelist was spelled out twice and the copies had drifted:
  `import_config`'s was missing the registration gates and every `security_*`
  toggle, under a comment claiming it used the same list as `update`. One list
  now serves both.

### Added

- Controls for settings that were previously only reachable by editing the
  database: the panel IP allowlist, the server-terminal kill switch, the
  auto-lockdown window (the threshold's other half — the rule reads "5 events in
  10 minutes" and only the 5 was adjustable), and the site-creation rate limit.

- `tests/settings-controls-pin-e2e.sh` — 17 assertions that **discover** their
  subjects rather than naming them: the whitelist is parsed out of the source,
  configurable knobs are found by the helper that reads them, and every arm fails
  when discovery finds fewer subjects than are known to exist. It fails if a
  writable key is read by nothing, if a knob is unsettable, if a toggle's
  comparison disagrees with the server's own default, or if a numerically-seeded
  key is read as a boolean.

### Removed

- Three controls that fronted features which were never built: a timezone
  selector claiming to "affect displayed timestamps throughout the panel", an
  email footer "appended to notification emails", and an events webhook that
  "receives POST for site.create, app.deploy, security.scan". All three saved
  successfully and none was read by any code. They will return with the
  behaviour they promise. Values already stored are left in place; the two dead
  seeds no operator could have set (`security_db_backup_retention_days`,
  `security_backup_chain_enabled`) are dropped by migration.

## [2.45.1] - 2026-07-27

The check added in 2.45.0 found a real defect within minutes of being deployed,
and it was a deeper cause than the one it was written for.

### Fixed

- **`npm run build` silently un-deployed the agent installer.**
  `install-agent.sh` lived only in `panel/frontend/dist/`, put there by
  `setup.sh`, `update.sh` or `deploy-demo.sh`. Vite empties `outDir` on every
  build — so building the frontend, an ordinary operation with nothing to do
  with the fleet, **deleted the file from the live panel**, and the command the
  Add-Server dialog prints returned `200` with SPA fallback HTML until an
  installer happened to run again.

  2.45.0 read this as "one deploy path forgot a step" and fixed that path. The
  real defect was that the artefact only ever lived in a directory that a routine
  command wipes. The frontend build now stages it from the single source in
  `scripts/` on every build — the same thing the marketing site has always done
  with `website/client/public/install.sh`. The staged copy is a build artefact
  and is gitignored, so it cannot become a second installer that drifts.

  The stager **fails** when the source is missing rather than skipping: a silent
  skip reproduces the defect exactly, and its symptom is an operator piping a web
  page into `sudo bash`.

## [2.45.0] - 2026-07-27

A release about controls that do not cover what they appear to cover.

The previous release found that the documented demo-deploy script had been
unable to run for eight releases, because a fix applied by derivation to two
installers never reached a third copy that lived outside this repository. That
copy is now in it, and the mechanism that could not see it has been rebuilt to
discover its subjects instead of naming them. Along the way, a question about
whether registration was disabled on one panel turned up a second control with
the same shape: visible, believed, and silently bypassed by a sibling.

### Security

- **Turning off self-registration did not turn off registration.** The panel has
  two doors into an account. `POST /api/auth/register` reads
  `self_registration_enabled` and defaults **closed** — an absent row means
  disabled — and it has a visible toggle in Settings. The OAuth callback
  auto-creates a user on first sign-in, read `oauth_auto_create`, and defaulted
  **open** — an absent row meant *allowed* — and it had **no control anywhere in
  the panel**, though it was writable through the settings API.

  Both rows are absent on a fresh install, so the panel's answer to "is
  registration open?" depended on which door you knocked on. An operator who
  switched the visible toggle off and later configured a GitHub or Google
  provider — an ordinary thing to do — had self-registration silently back on
  through a switch they could not see or reach.

  An explicit `self_registration_enabled=false` now closes the OAuth path too:
  off means off, whichever door. This deliberately reads the *explicit* value
  rather than the effective one, so an install that never touched the row keeps
  its current behaviour and no working OAuth deployment breaks on upgrade — only
  operators who actually asked for registration to be off get what they asked
  for.

- **`oauth_auto_create` is no longer a DB-only switch.** It gates account
  creation and now has a toggle beside Self-Registration. Note that it renders an
  absent row as **on**, matching the server's default rather than its neighbour's
  — a control that showed "off" while signups succeeded would be worse than no
  control at all, because it would end the investigation.

### Added

- **`scripts/deploy-demo.sh`.** The third path that installs DockPanel onto a
  running machine — after `setup.sh` (fresh install) and `update.sh` (upgrade in
  place) — deploying from published release assets rather than a checkout. It
  existed outside this repository, which is exactly why it rotted: no pin, no CI
  job and no documentation check can see a file that is not in the tree. It takes
  its panel hostname and repo path from the caller, so it describes the contract
  rather than one box.

- **`tests/registration-gates-pin-e2e.sh`** (14 assertions). Holds both doors
  above to the same answer: the defaults, the ordering of each gate against the
  `INSERT` it protects, that every gate has an operator control that actually
  writes it, and that each control renders the default its own server uses.

### Fixed

- **The regression pins named their subjects, so they could not see a new one.**
  `sandbox-paths-pin-e2e.sh` pinned `setup.sh`, `update.sh`, `install-agent.sh`
  and `agent-self-update.sh` by name. That is how the third copy of the agent
  unit went unnoticed for eighteen releases, and how the fourth copy of the
  `ReadWritePaths` derivation went unnoticed for eight. The suite now
  **discovers** them: any script that touches `ReadWritePaths` is a copy and
  joins the assertions by existing, and the same rule covers every script that
  populates the frontend dist. Discovery that finds fewer subjects than are known
  to exist fails, rather than reporting a clean run over nothing.

- **`{panel}/install-agent.sh` had no check, while its sibling did.** The
  scheduled live-surfaces check has verified `dockpanel.dev/install.sh` since
  2.44.0 — that a missing file answers `200` with SPA fallback HTML rather than
  `404`, so the advertised command pipes a web page into `bash`. The URL the
  panel's own Add-Server dialog prints has the identical failure mode and nothing
  watched it; it was reproduced by hand at the previous release. It is now
  checked on the same schedule, including that the served installer is
  byte-identical to `scripts/install-agent.sh`.

## [2.44.1] - 2026-07-27

A dependency-and-provenance release, and the numbers work from the previous
commit that had not yet been published under a tag.

Every open advisory against this project was assessed rather than merely bumped,
and **none of them was ever exploitable here** — which is stated plainly below
rather than dressed up as a fix. What was genuinely wrong was quieter: an image
that could change major version under a running mail stack without asking, an
installer that could silently discard the dependency tree the audit gates had
scanned, and a waiver whose reasoning nothing checked.

### Security

- **The Roundcube webmail image was `:latest`.** A major Roundcube upgrade could
  land on a user's mail stack with no warning and no way back — while the panel
  itself warns users to pin exactly this (`docker_apps.rs` flags `:latest` in any
  app they deploy). Now `1.7.x-apache`.

  Deliberately the `.x` line rather than an exact patch: this is a web-facing PHP
  application and nothing in DockPanel would ever bump a frozen tag, so pinning
  to `1.7.2` would have quietly stopped its security rebuilds. Verified by digest
  at the time of pinning — `latest`, `latest-apache` and `1.7.2-apache` were all
  `sha256:aed1b9b5dc34`, so this changed the guarantee without changing the image.

- **Both installers could silently discard the audited dependency tree.**
  `setup.sh` and `update.sh` ran `npm ci --silent 2>/dev/null || npm install
  --silent 2>/dev/null` when building the frontend. `npm ci` installs exactly the
  committed lockfile — the tree the audit gates actually scan; `npm install`
  re-resolves and can pull versions nothing here has ever seen. Either arm could
  fail for any reason and say nothing, and if both failed the script died under
  `set -e` printing no explanation at all.

  The fallback stays, because a box whose lockfile has drifted should still
  update. It is now loud: the reason `npm ci` failed is shown, and the fallback
  announces that the tree is being re-resolved and may differ from the audited
  lockfile.

- **`body-parser` upgraded 2.2.2 → 2.3.0** (GHSA-v422-hmwv-36x6, low), with the
  floor set in `package.json` and not only in the lockfile.

  *Was it exploitable? No.* The advisory needs an invalid `limit` value to be
  passed, at which point size enforcement is silently disabled. The only body
  parser in `website/server` is `express.json()` with no options at all, so there
  was never a `limit` for us to get wrong.

- **The `react-router` advisory (GHSA-qwww-vcr4-c8h2, high) was re-derived from
  scratch rather than carried, and the waiver holds.** It requires RSC mode with
  server actions. Both frontends are Vite SPAs mounting `BrowserRouter`, with no
  `@react-router/*` server package, no `react-router.config.*`, no
  `createRequestHandler` and no server actions. Upstream's first patched version
  is 8.3.0 — a major — so clearing the alert means migrating both frontends, not
  bumping them, and npm's only in-range offer remains a downgrade to 7.11.0.

  What changed is that the waiver is no longer trusted on its own word. Its
  upstream half was already re-checked daily; the half that is a fact about *us*
  — that no frontend here runs a server runtime — is now pinned by
  `ssl-correctness-pin-e2e.sh`, which fails if any frontend gains the machinery
  that would make the advisory apply. The frontends are discovered from the
  manifests that depend on `react-router-dom`, so a third one joins the check by
  existing rather than by being remembered.

- `spin` moved off a yanked release (0.9.8 → 0.9.9) in the backend lockfile,
  clearing the last `cargo audit` warning there. *Was it exploitable? No — it was
  never even compiled.* `cargo tree -i spin` returns nothing and no artifact is
  produced; it is a phantom lockfile entry like the `rsa` one already documented
  in `.cargo/audit.toml`. The warning was real, the exposure was not.

  `rustls-pemfile` (RUSTSEC-2025-0134, unmaintained) in the agent is the one
  finding that genuinely stands: it *is* linked and reachable, parsing
  certificates in `tls.rs`. But "unmaintained" is not a vulnerability, 2.2.0 is
  the newest release in existence, and the other consumer is `axum-server`. It
  stays an accepted, written-down warning rather than a silent one.

### Fixed

- **The panel's memory footprint had been published as `~19 MB` since April. It
  is `~49 MB`.**

  The figure appeared on the README masthead, the README comparison table, the
  README architecture note, `COMPARISON.md`, `docs/getting-started.md` and the
  marketing site. It was measured once, on one box, and then copied. A real
  reading by cgroup accounting gives ~14 MB for the API and ~35 MB for the agent;
  with the bundled PostgreSQL the stack is ~109 MB, not the published ~85 MB.

  The comparison against cPanel and CloudPanel was also unfair in our favour: it
  set DockPanel's services, without a database, against competitors' figures that
  include theirs. Stack against stack it is about 7x lighter rather than 10x —
  still the strongest claim on the page, and now one that survives being checked.

- Binaries were published as `~41 MB` total; the v2.44.0 release assets are
  22 MB (API), 21 MB (agent) and 1.7 MB (CLI) — 45 MB. The install animation on
  the front page announced a 41 MB download for a 22 MB binary.

- `776 API endpoints` was a number nothing derived, and its own decomposition
  (`496 backend + 280 agent`) matched the source on neither side. It is 809
  routes — 527 and 282 — and it is now counted rather than remembered.

- `454 E2E tests`, `89 DB migrations` and `11 background services` were likewise
  stale or undefined. They are 309 regression assertions across eleven suites,
  97 migrations, and 15 supervised background services.

- Two entries of the same FAQ list on the front page gave different answers for
  the same measurement, three lines apart.

### Added

- **A measurement register** (`FEATURES.md` → "Verified Metrics"). Every number
  this project publishes about itself is written down once, with its derivation:
  computed from source, read from the published release, or measured on a real
  box. `docs-claims-pin-e2e.sh` now fails the build when a surface states a
  figure the register does not, when a derived figure no longer matches source,
  or when a corrected figure reappears anywhere.

- **A scheduled check** (`live-surfaces-check.sh`, `live-surfaces.yml`, daily).
  Documentation rot sorts into classes that need different mechanisms, and the
  one that drifts with *time* rather than commits had none — no push-triggered
  job can notice an expiring certificate, an install one-liner that started
  serving HTML, or a site still publishing the bundle it was built from three
  weeks ago. It runs from a GitHub runner rather than the origin, so the CDN in
  front of the sites is inside the test rather than behind it.

- Claims that no machine can verify — competitor pricing and memory, whether the
  screenshots still resemble the product — now carry a last-verified date and a
  budget, and expire. The failure does not assert the claim is wrong; it reports
  that nobody has looked in long enough that we no longer know.

## [2.44.0] - 2026-07-27

### Security

- **Every server added through "Add Server" has been running the agent with no
  sandbox at all, under a comment saying otherwise.**

  `install-agent.sh` — the only documented way to add a remote server, and so the
  installer that built every fleet in existence — hand-wrote its own copy of the
  systemd unit. That copy set all four of systemd's headline protection switches
  to off, declared no writable-path list whatsoever, and omitted eight further
  hardening directives the real unit sets, beneath the line "Create systemd
  service (matching local agent hardening)". It matched nothing. The panel box
  and every box upgraded through `update.sh` were correctly confined; the remote
  servers those panels manage were not, from v2.28.0 to here.

  There is now one unit. `panel/agent/dockpanel-agent.service` is compiled into
  the agent (`--print-unit`) exactly as `agent-self-update.sh` already was, so it
  cannot drift from the binary that runs under it, and all three installers
  obtain it from the same place instead of keeping copies. The unit gained an
  optional `EnvironmentFile=-`, which is what lets one file serve both a panel
  box and an agent-only box.

- **And the fix reaches servers that already exist**, which is the part that
  would otherwise have made it worthless: a corrected installer only ever
  changes what the *next* server gets, and the agent self-update had never
  touched the unit. The agent now reconciles its own unit at startup, and the
  self-update installs the new one before the restart that applies it — with the
  previous unit restored if the agent does not come back. Directories named by
  `ReadWritePaths` are derived and created before either swap, because an
  unprefixed entry that does not exist makes the unit unstartable and an
  agent-only box has no `/etc/nginx`.

  Verified before and after on one Rocky 9 box (SELinux Enforcing) registered to
  a real panel over a real certificate. Before: `ReadWritePaths=` empty and every
  protection off. After the binary landed and the agent restarted: `/usr` and
  `/root` read-only inside its namespace, `/etc/nginx` writable, `/tmp` private,
  `NoNewPrivs` set on the process — and the server still `online`.

### Fixed

- **Adding a remote server has never worked on Rocky, AlmaLinux or CentOS**, and
  it failed without saying anything. `install-agent.sh` installed Docker with
  `get.docker.com`, which points those distributions at
  `download.docker.com/linux/rocky` — a path that carries no `docker-ce`, so the
  run ended at `Unable to find a match`. v2.37.0 fixed exactly this in the panel
  installer and the fix never reached the agent installer, which is the same
  copy-drift the change above is about. Every stream in that step was redirected
  to `/dev/null`, so what a user saw was the script stopping at
  `[2/7] Installing dependencies...` with no error at all. The RHEL clones now
  use the same repository the panel installer uses, and a Docker install that
  did not work now says so and stops.

- **An agent binary older than the flag it is asked for no longer hangs the
  installer.** Until now the agent ignored its arguments entirely, so asking an
  older one for `--print-unit` started the *daemon* — it bound the agent socket
  and never returned, leaving the install stuck at `[7/7]` for ever. Both callers
  now bound the call, and the agent rejects an unrecognised option instead of
  starting up. (Running it with no arguments still starts the agent, as before.)

## [2.43.0] - 2026-07-27

### Fixed

- **Webmail showed an empty inbox on every install that already had it, and the
  one mechanism meant to repair that wrote the broken configuration itself.**

  The `/webmail/` nginx fragment is written only when you click Install. v2.36.0
  fixed a real defect in its contents — without a re-declared header set the
  location inherits the panel's `frame-ancestors 'none'`, Roundcube's content
  frame is refused, and the resulting `SecurityError` aborts `list_mailbox`
  before the message list is ever requested — but that fix reached nobody who
  had already installed webmail. Worse, `update.sh` carried a hand-copied mirror
  of the fragment, frozen at the v2.10.1 shape with no header set, and its heal
  fired only when `sub_filter` was missing: a box with an older fragment was
  *healed into* the broken shape, and a box with any v2.10.1–v2.35.x fragment
  failed the guard and was left untouched indefinitely. Both halves of the only
  recovery path therefore produced or preserved an empty inbox, on a mailbox
  holding real mail.

  The agent now owns the fragment outright and reconciles it against the current
  template at startup, so an upgrade is all it takes. The shell mirror is
  deleted rather than corrected — one writer is the fix. Verified before and
  after on one box: 0 message rows with the frame blocked, then 3 rows and a
  message opened and read, from an agent restart alone.

- **The spam filter could not be installed on the RHEL family at all.** `rspamd`
  is not in EPEL, so on a stock Rocky 9 with EPEL *and* CRB enabled the panel's
  Install button returned `Unable to find a match: rspamd`. DockPanel now adds
  upstream's rpm repository — on the RPM family only, since Debian and Ubuntu
  package rspamd themselves and that path already worked.

- **And once installed, Postfix never consulted it — on every family.** Rspamd
  was wired into Postfix by replacing the literal
  `smtpd_milters = unix:opendkim/opendkim.sock`, a value v2.41.0 stopped writing
  when it moved OpenDKIM's milter to a loopback port. Replacing an absent string
  returns the original unchanged, so `main.cf` was rewritten byte-identical and
  the installer reported success; with `milter_default_action = accept`, mail
  simply flowed unfiltered and nothing was logged. The milter list is now
  derived from what the file actually contains rather than matched as a literal,
  and a `main.cf` with no milter list is reported as an error instead of being
  written back unchanged. Verified on the wire: a GTUBE test message is now
  rejected by the milter at end-of-message.

## [2.42.0] - 2026-07-27

### Fixed

- **The dashboard CPU gauge reported the caller's timing, not the machine's
  load.** It read 99% on a box `top` showed 94.6% idle, while the 24-hour chart
  built from the *same* endpoint read 9.7%. Nothing was wrong with the box.

  CPU percentage is a delta over a window, and `/system/info` refreshed CPU
  inside the request handler — so the window was "time since whichever unrelated
  caller last hit this endpoint". Five callers share it (the metrics collector
  every 30s, the dashboard WebSocket loop every ~5s, the dashboard REST tick,
  the backup scheduler, the Docker-apps page), so each one silently corrupted
  the others' measurements. Compounding it, the handler walked every process in
  `/proc` on each call — 70ms of CPU with 583 processes — purely to obtain a
  *count*, and that walk landed inside the next caller's window, making the
  endpoint the largest consumer in its own measurement. Below sysinfo's 200ms
  minimum a refresh is *skipped* rather than rejected, so closely-spaced callers
  received a stale value with no error anywhere.

  Measured on a single-core box running the published v2.41.0: the same idle
  machine reported 21.9% at a 0.25s request gap and 1.8% at a 10s gap — a
  twelvefold swing with no change in load — and reading 0.3s after a six-second
  burst ended returned 95.5% while the box was idle. That last case is what a
  user meets: the install saturates the box, the dashboard opens, and the first
  reading covers the install rather than the present.

  Fixed structurally. A dedicated sampler owns the window: its own CPU-only
  `System`, a fixed two-second cadence, publishing to an atomic that handlers
  read for free. The number no longer depends on who asks or how often. On the
  same box the reported value now stays within a few points of `/proc/stat` at
  every request spacing, and `/system/info` went from 74–134ms to 3–8ms.

- **Fleet check-ins reported a number made mostly of their own `/proc` walk.**
  `phone_home` built a `System::new_all()`, called `refresh_all()` immediately
  after, and read CPU usage off the pair. On an idle 12-core box whose true
  usage was 4.5%, consecutive runs returned 4.5% to 22.1%. That value is stored
  as `servers.cpu_usage`, which the Servers page displays and the alert engine
  compares against CPU thresholds — so it could both raise and withhold alerts
  on fiction. It now reports the sampled value.

- **A directory the agent must create could be silently unwritable, and one
  already was on upgraded boxes.** In the agent unit's `ReadWritePaths`, a `-`
  prefix means "bind if it exists": when the path is absent systemd skips it
  *silently*, the unit starts and reports success, and every write beneath it
  fails with `Read-only file system` until the next restart — creating the
  directory afterwards does not rescue a running service, because the mount
  namespace is fixed at start.

  The installers pre-create these paths for that reason, and the list was
  hand-copied into both `setup.sh` and `update.sh`. It had drifted:
  `/var/spool/cron` reached the unit and `setup.sh` in v2.41.0 but never
  `update.sh`, so a box that *upgraded* without an existing cron spool got a
  silently unwritable one — defeating, on exactly those installs, the "existing
  installs recover on upgrade" property the v2.41.0 cron fix was built for.
  Driven on a fresh box: with the directory absent at agent start `/crons/sync`
  answers `crontab command failed` and writes nothing; with it present the same
  call syncs. Both scripts now derive the list from the unit itself, so the
  mirror cannot drift from it again.

- `docs/testing.md`'s summary sentence read "Seven suites, 195 assertions" above
  an eight-row table summing to 228. Every row in that table was verified
  against live output by `docs-claims-pin-e2e.sh`; the sentence introducing them
  was not. It is now.

### Internal

- Two new regression pin suites (`cpu-metric-pin-e2e.sh`,
  `sandbox-paths-pin-e2e.sh`), 32 assertions, each negative-tested to confirm it
  fails when the defect is reintroduced.
- `panel/cli/Cargo.lock` had been pinned at 2.38.0 and `package-lock.json` at
  2.26.0 across several releases; both resynced.

## [2.41.0] - 2026-07-26

### Fixed

- **Mail could not work on any RHEL-family box, and the reason it was refused
  was not the reason it was broken.** From v2.39.0 the panel declined to install
  the mail server on Rocky/Alma/CentOS with an honest note that nothing past the
  packages had been driven there. Driving it end to end on two Rocky 9.8 boxes
  found five separate defects, and disproved the refusal's own premise in both
  directions: the packages did **not** install (EPEL's `opendkim` needs
  `libmilter` and `libmemcached`, both in the **CRB** repository, which EPEL
  requires and `setup.sh` never enabled), and the Debian failure mode the
  refusal warned about — a package manager's postinst starting the daemons so
  "installed and running" is true for free — cannot occur on RHEL at all, which
  does not start services on install.

  What actually broke, none of it named by the refusal:

  - **Postfix listened only on loopback.** `inet_interfaces` was never set
    anywhere in the tree; Debian's debconf writes `all`, the RHEL package ships
    `localhost`. No mail could arrive from another host, with the firewall
    correctly opened and the installer reporting success.
  - **OpenDKIM never started, so nothing was ever signed.** The config wrote
    `TrustAnchorFile /usr/share/dns/root.key`, a Debian path; OpenDKIM treats a
    missing anchor as fatal (`status=78/CONFIG`).
  - **The `-f` flag was correct for Debian and wrong here.** v2.36.0 removed it
    because Debian's packaged unit is `Type=forking`; EPEL's is `Type=simple`,
    where the daemon backgrounds itself, systemd reaps the parent and logs
    `Deactivated successfully` while the unit goes inactive — a failure that does
    not register as failed. The drop-in now pins `Type` as well as `ExecStart`,
    so the distro's choice cannot decide the flag.
  - **The milter socket lived in Postfix's chroot**, an arrangement that exists
    only because Debian chroots `smtpd`. RHEL does not, and SELinux forbids it:
    `dkim_milter_t` is denied `search` on `postfix_spool_t`. The milter is now a
    loopback port, which is correct on both families and needs no shared group,
    socket directory or ownership dance.
  - **Every delivered message was silently discarded.** `/var/vmail` is created
    by the installer, so it inherited `var_t`, which `dovecot_t` may not write —
    LMTP failed `mkdir` with "Permission denied" on a directory owned by
    `vmail:vmail` and mode 0755, and every message was deferred forever while
    both services were active. It is now labelled `mail_spool_t`.
  - **No mailbox could be opened.** Mailbox passwords are hashed `{ARGON2ID}`,
    and Rocky 9.8 ships Dovecot 2.3.16 built *without* Argon2 — the scheme is a
    build option, not a version, though the code's own comment cited ">= 2.3.11".
    Every login failed `Unknown scheme ARGON2ID` while the panel reported the
    account created successfully. The panel now asks the agent which schemes its
    Dovecot actually supports and picks Argon2id where present, bcrypt where not,
    so Debian and Ubuntu are unchanged.

  Verified on the wire: a message travelled between two real domains on two
  Rocky 9.8 boxes and arrived `dkim=pass`, verified by the receiving box's own
  OpenDKIM against a key published in real DNS, and the mailbox opened over IMAP
  on the box's real Let's Encrypt certificate. Re-verified on Debian, where the
  hash stays `{ARGON2ID}` and OpenDKIM still starts under its forking unit.

- **Cron jobs could not be created on any install once a WordPress site
  existed.** The panel auto-creates a WordPress cron whose command contains
  `> /dev/null 2>&1`, INSERTed straight into the database — but `> /dev/` is on
  the agent's own blocked-pattern list, and the sync endpoint validated every
  row and rejected the whole batch on the first bad one. From the moment a
  WordPress site was created, adding, editing or deleting *any* cron failed with
  "Command contains disallowed characters or patterns" and nothing reached the
  crontab. The writer no longer emits the redirect, and the reader now skips an
  unsafe row (reporting it) instead of failing the entire sync, so installs that
  already carry the row recover on upgrade.

- **`/var/spool/cron` was missing from the agent's `ReadWritePaths`** while
  `setup.sh` pre-created it, so `crontab` could not write its temp file
  (`mkstemp: Read-only file system`) and cron writes returned 500.

- Mail uninstall and the Rspamd installer still shelled out to `apt-get`
  directly, so both failed on RPM with "Failed to find executable apt-get"; both
  now use the package abstraction, and the Redis unit name is translated
  (`redis-server` on Debian, `redis` on the RHEL family).

- `mail_status` reported `installed: true, running: true` while OpenDKIM was in
  a restart loop, because OpenDKIM was excluded from the summary verdict.

- The mail log viewer read `/var/log/mail.log` unconditionally, so it showed
  zero sent and zero received on RHEL, which writes `/var/log/maillog`.

### Changed

- README and marketing screenshots retaken on the `terminal` theme.

## [2.40.0] - 2026-07-26

### Fixed

- **No package operation could run on any SELinux system, and it never could.**
  Installing Redis, Node.js, PowerDNS, the WAF, Cloudflare Tunnel, Composer,
  Fail2Ban or a PHP extension from the panel failed on every RHEL-family box
  with `Failed to start transient service unit: Connection reset by peer`. This
  had been true since the agent's sandbox was introduced, and the cause was one
  flag: the agent escaped its `ProtectSystem=strict` sandbox with `systemd-run
  --pipe`, which passes stdin/stdout/stderr **as file descriptors over D-Bus**.
  On the RHEL family the system bus is `dbus-broker`, and SELinux checks the
  receiver's access to a passed object — receiving a writable pipe labelled
  `unconfined_service_t`, the label every systemd service's pipes carry, is
  denied. The broker drops the connection, and the rule is `dontaudit`ed, so
  nothing whatsoever is logged. The same command works from a shell because a
  shell's pipes are labelled `unconfined_t`.

  The escape hatch no longer passes descriptors at all: systemd is asked to
  open the capture files itself (`-p StandardOutput=file:…`), which it may do,
  while `--wait` still propagates the inner command's exit status. The change
  applies on every distribution rather than only where it broke, so the path
  Debian and Ubuntu exercise is the same one.

  Verified on Rocky 9.8, before and after on one box: Redis 6.2.22, Composer
  2.10.2, Fail2Ban 1.1.0, Node.js 22.23.1, PowerDNS 5.0.6, ModSecurity 1.0.4,
  cloudflared 2026.7.3 and `php-bcmath` all installed from the panel, on the
  machine that had refused every one of them minutes earlier. Re-verified on
  Debian 12 so the working family stayed working.

  UFW still refuses on a box already running firewalld, and the mail server
  still refuses on RPM — both deliberate, both stated.

## [2.39.0] - 2026-07-26

### Fixed

- **Automatic updates left the agent unable to start on SELinux systems.** A
  binary moved into `/usr/local/bin` keeps the label it had at its source — a
  rename within one filesystem preserves it, it does not adopt the
  destination's. `agent-self-update.sh` stages the download under
  `/var/lib/dockpanel` and `update.sh` moves the release binaries in from
  `/tmp`, so on Rocky, AlmaLinux, CentOS Stream and Fedora the new binary
  arrived labelled `var_lib_t` or `user_tmp_t` instead of `bin_t`, and systemd
  then refused to execute it (`status=203/EXEC`, "Permission denied") — while
  the update reported success. Because the agent's own updater runs on a
  six-hourly timer, every RHEL-family install would have lost its agent at the
  first automatic update after installation. Both paths now restore the
  security context after the swap.

- **A fresh install on the RHEL family got end-of-life PHP 8.0.** `dnf install
  php-fpm` resolves to the non-modular base package unless a module stream is
  selected first, so the installer produced PHP 8.0.30 — older than every
  stream those distros offer (8.1, 8.2, 8.3) and unsupported since November
  2023 — and printed "PHP 8.0 (FPM)" as if that were the intended outcome. It
  also made the panel's own PHP installer unreachable, since that checks
  whether PHP is present, finds 8.0, and reports "already installed". The
  installer now selects the newest stream the system offers before installing.

- **The Services and PHP pages misreported PHP on the RHEL family.** The
  per-version package query collapses onto the single unversioned `php-fpm`
  package there, so every offered version read as installed, while the
  running-check and socket path were both Debian-shaped, so none read as
  running. The version is now read from the package database.

### Changed

- **Package operations dispatch on the package manager the system actually
  has.** `services/pkg.rs` grew from a query layer into an install layer:
  install/remove routed through the real manager, per-family repository setup
  for NodeSource and Cloudflare, PHP module-stream selection, and a systemd
  **unit**-name map alongside the package-name map — a package name and a unit
  name are different strings that merely coincide on Debian, and translating
  only the first installs the right package and then enables a unit that is not
  there. The optional-service installers, the mail installer and the PHP
  version manager all route through it.

- **The panel now states plainly that it cannot install packages on RHEL-family
  systems, instead of failing with an internal error.** The agent performs
  privileged package work by asking systemd for a transient unit; on the RHEL
  family that request is refused when it comes from inside the agent's service
  (`Failed to start transient service unit: Connection reset by peer`). This is
  a long-standing limitation rather than a new one — Composer installation,
  unchanged in this release, fails the same way — and it is not caused by the
  agent's sandbox, which was ruled out by reproducing the failure with every
  restriction disabled. Until it is resolved, these endpoints report the
  limitation and point at the system package manager rather than surfacing a
  D-Bus error. **DockPanel does not claim panel-driven service installation on
  RHEL-family systems in this release.**

- **UFW installation is refused on systems running firewalld**, naming the
  reason. UFW is installable there, which is precisely the hazard: it would
  create a second rule set that nothing consults — the failure mode that made
  v2.37.0 installs unreachable. Ports are opened through whichever firewall is
  actually enforcing.

- **Mail server installation is refused on RHEL-family systems** until the
  configuration half has been verified there. The packages resolve, but a mail
  stack whose daemons are running and whose configuration is wrong reports
  itself healthy and delivers nothing.

## [2.38.0] - 2026-07-26

### Fixed

- **On the RHEL family the panel installed successfully and could not be
  reached.** v2.37.0 made the installer complete on Rocky, AlmaLinux, CentOS
  Stream and Fedora, and verified it with `/api/health` — measured on
  `127.0.0.1:3080`, which bypasses nginx. Driving the same install from a
  browser found it unusable, for two independent reasons:
  - **Two firewalls, and the installer configured the one that was not
    enforcing.** These distros boot with **firewalld** running and only SSH
    allowed. `setup.sh` installed UFW alongside it and opened 80/443 in UFW,
    while firewalld went on dropping them. The panel was unreachable, and
    Let's Encrypt could not fetch the ACME challenge, so no certificate was
    issued either — while the installer printed "installed successfully" and an
    `https://` URL, and blamed Cloudflare for the SSL failure. The installer now
    detects the firewall the box is already enforcing with and configures that
    one, never installing a second; the SSL hint names reachability first; and a
    failed issuance can no longer be reported as an `https://` panel URL.
  - **SELinux blocked nginx from reaching the panel API.** With SELinux
    Enforcing (the default on all four) `httpd_can_network_connect` is off, so
    every request returned 502 — including from the box itself — with no journal
    or `ausearch` entry, because the denial is `dontaudit`-ed. The installer now
    sets the boolean up front.
  - **`update.sh` repairs both on installs that already exist**, which is the
    only path in: a box in either state cannot be fixed from a panel it cannot
    reach.
- **The panel misreported the box it was running on.** `is_installed()` shelled
  out to `dpkg`, and there is no dpkg on an RPM system, so every package read as
  absent — the Services page offered to install PHP and Fail2Ban while both were
  installed and running. Four hand-rolled copies of that function existed, which
  is how it stayed wrong in all of them; package presence now goes through one
  implementation that dispatches on the real package database and maps the names
  that differ (`pdns-server`→`pdns`, `redis-server`→`redis`, Debian's three
  Dovecot packages→`dovecot`). PHP-FPM's *running* check had the same shape in
  the service manager and now also recognises the single `php-fpm` unit.
- **Firewall state was read through `ufw` alone**, so a firewalled RHEL box
  reported no firewall at all on the Security page, and diagnostics warned
  "Firewall (ufw) is not active" while naming a tool the operator does not have.
  Both now dispatch on the running firewall, and the Security page reports
  firewalld's zone, policy and open services.
- **Mail ports were "opened" without checking.** `open_mail_ports()` discarded
  every result and then logged success unconditionally — false on any box
  without ufw. It now reports which ports it could not open. Moving the SSH port
  refuses rather than proceeding if the new port cannot be opened, instead of
  locking the operator out.

### Changed

- Optional-service installers that are still Debian/Ubuntu-only (Redis, Node.js,
  PowerDNS, mail, WAF, Cloudflare Tunnel) now refuse on other distributions with
  a stated reason and a remedy, instead of failing with
  `Failed to find executable apt-get`. The limitation is documented in the
  README and the getting-started requirements.

## [2.37.0] - 2026-07-26

### Fixed

- **DockPanel could not install on any RPM-family distro.** The README, the docs
  and the website all claimed CentOS 9+, Rocky 9+, Fedora 39+ and Amazon Linux
  2023, and the release smoke-test matrix contained only Debian and Ubuntu
  images. Driving all four on real servers found every one of them failing, in
  three different places:
  - **Rocky and AlmaLinux died installing Docker.** `get.docker.com` sends each
    distro to its own repository path, and `download.docker.com/linux/rocky/9/`
    publishes `containerd.io` and the plugins but no `docker-ce` — so the
    install aborted at step 3 of 15 with `Unable to find a match: docker-ce`.
    AlmaLinux is not in that script's distro list at all. Both now get an
    explicit repository pointing at the `centos` path, whose packages are plain
    `el$releasever` builds.
  - **CentOS Stream died configuring Nginx.** The step that comments out RHEL's
    default server block used a `sed` range terminating at the first `}`, which
    inside a server block belongs to a nested `location` — half the block stayed
    live at `http` level and `nginx -t` failed with `"location" directive is not
    allowed here`. It now counts braces to find the block's real end.
  - **Fedora died starting the agent.** The unit listed `/etc/apt` in
    `ReadWritePaths`; systemd fails the entire mount namespace when any entry is
    missing, so on an RPM box the agent could not start at all. Distro-specific
    paths now carry systemd's `-` prefix, so a missing directory can no longer
    make the agent unstartable — the class fix, not just this path.

### Changed

- **Amazon Linux 2023 removed from the support claim, AlmaLinux 9+ added.**
  Docker's install script has no Amazon Linux branch and no image is available
  to verify a fix against, so the claim was withdrawn rather than left standing
  on nothing. AlmaLinux — which the installer already greeted by name while
  being unable to install on it — is now claimed, tested and verified.
- The release smoke-test matrix now covers Rocky 9, AlmaLinux 9, CentOS Stream
  9, Fedora 39 and 43 and Amazon Linux 2023 alongside the apt distros, with a
  per-family package-manager step.

### Added

- `tests/rpm-install-pin-e2e.sh` (7 assertions) pins all three fixes, including
  a check that every sandbox path is either optional or created before the unit
  starts — so the next `/etc/apt` cannot happen.
- `tests/docs-claims-pin-e2e.sh` now fails the build when a distro family is
  named on any published surface and no image in the smoke matrix tests it. It
  locates support claims by pattern across all three surfaces, which turned up a
  fifth claim site nobody had been maintaining.

## [2.36.0] - 2026-07-26

### Fixed

- **The mail server installer had never completed, on any install.** It aborted at
  `Failed to write opendkim.conf: Read-only file system`: the agent runs
  `ProtectSystem=strict`, and `/etc/opendkim.conf` is a bare file in `/etc` — the one
  mail path the unit's `ReadWritePaths` does not cover. Everything after that line
  never ran. OpenDKIM's config now lives at `/etc/dockpanel/opendkim.conf`, inside the
  permitted paths, with a systemd drop-in pointing the daemon at it. The sandbox is not
  widened.
- **No outgoing message had ever been DKIM-signed.** OpenDKIM's `KeyTable` and
  `SigningTable` were written empty and nothing ever populated them, so no domain was
  bound to its key — while the generated key was published in DNS and verified green by
  the panel's own DNS check. Adding or removing a mail domain now rebuilds both tables
  from the keys on disk and reloads OpenDKIM. Verified on the wire between two real
  domains: `dkim=pass`, and the receiving spam filter's score on the same message fell
  from 9.74/15 to 1.59/15.
- **The mail ports were never opened in the firewall.** Postfix and Dovecot were started
  behind a UFW that allowed only 80, 443 and the panel, so no mail could arrive. The
  installer now opens 25, 587, 465, 143, 993, 110 and 995.
- **Postfix announced itself with a short hostname**, costing six spam points before any
  content was examined. `myhostname` is now set from the panel domain, and
  `mydestination` is narrowed to `localhost` so that a hosted mail domain is never
  mistaken for a local one and bounced as "unknown user".
- **Re-running the installer erased every hosted domain, mailbox and password** by
  truncating the Postfix maps and the Dovecot users file. They are now created only when
  absent. Re-running also appended a duplicate `submission` service each time, because
  the guard matched the commented-out stock entry.
- **Roundcube could not log in at all.** Dovecot required TLS but was never given the
  Let's Encrypt certificate the box already held, so it served a self-signed one and
  every IMAP client refused it with "unknown ca". Dovecot and Postfix now use the panel's
  certificate when there is one.
- **The panel's own `frame-ancestors 'none'` applied to the webmail it installs.** The
  `/webmail/` location now allows same-origin framing, which Roundcube's skin requires.
- **`mail_status` reported a failed installation as healthy**, because it asked only
  whether the packages were present and the services up — both of which `apt` provides on
  its own. It now also reports whether the configuration was actually written.

### Known issue

- The Roundcube message list can still render empty even though the mailbox has mail: a
  frame is navigated to the panel root, whose stricter policy refuses it. Login, delivery
  and IMAP are unaffected. Serving webmail on its own hostname avoids it.

## [2.35.0] - 2026-07-26

### Fixed

- **Git deploy could never build on a normal install.** The agent runs
  `ProtectSystem=strict` + `ProtectHome=yes`, and `docker build` creates `$HOME/.docker`
  before it will run — so with `HOME=/root` mounted read-only, every Dockerfile-based
  deploy aborted at `mkdir /root/.docker: read-only file system`. The repository was
  cloned, the build never ran once, on any install. The docker CLI is now pointed at
  `DOCKER_CONFIG=/var/lib/dockpanel/docker`, inside the unit's `ReadWritePaths`. Set in
  the shared `safe_cmd` helpers rather than at a call site: `env_clear()` means a
  unit-level `Environment=` never reaches the child, and ~77 docker invocations share
  those helpers. The sandbox itself is unchanged — `/root` and `/usr/local/bin` stay
  read-only.
- **The Dockerfile-less fallback could not rescue it either.** nixpacks installed itself
  into `/usr/local/bin` (read-only under the same sandbox) and cached into
  `/var/cache/dockpanel` (never in `ReadWritePaths`). Both now live under
  `/var/lib/dockpanel`, matching how the image scanner already handles this. A
  previously downloaded copy is also found again, instead of being re-fetched on every
  agent restart because it sits off the agent's `PATH`.
- **`update.sh`'s agent health check could never pass.** It probed the panel's
  `/api/system/info` with no credentials; that endpoint is authenticated, so it answered
  401, `curl -sf` read that as failure, and every update on every install printed
  "Agent connectivity check failed" whether the agent was healthy or dead. It now asks
  the agent's own `/health` over its unix socket, which is auth-exempt by design, and
  falls back to the legacy `/var/run` socket path on older boxes.
- **`DOCKPANEL_VERSION` was read by the installer and ignored by its only consumer.**
  `install.sh` clones the requested ref, but `setup.sh` always downloaded
  `releases/latest` — so `DOCKPANEL_VERSION=v2.31.2` produced a v2.31.2 tree running
  the newest binaries and reported the newest version on completion. Unit files, nginx
  templates and `install-agent.sh` are deployed from the tree, so that skew is the same
  class that once stranded the v2.8.13 → v2.8.14 upgrade. `setup.sh` now honours the
  pin and names the release it installed.

### Verified

- The update path itself, driven end to end on a fresh box for the first time: a
  published **v2.31.2** install with a live WordPress site upgraded in place to current.
  Panel, agent, CLI, tree and schema all moved forward (96 → 97 migrations), every row
  survived, the site and panel kept serving over their real Let's Encrypt certificates,
  and a backup taken after the upgrade carried its database (`1/1`) while the
  pre-upgrade row correctly stayed `0/0`. A full restore drill on the upgraded box
  brought back a deleted WordPress post. Evidence:
  `dockpanel-update-path-drill-s261.md`.

## [2.34.2] - 2026-07-26

### Fixed

- **Hardening:** the single-file restore now explicitly refuses `.dockpanel-backup`, the archive
  directory holding a backup's database dumps. It could not reach them in practice — that function
  prefixes every request with `./` while the payload members carry no prefix — but the protection
  was an accident of how tar names members rather than a decision, and those dumps are the site's
  entire content in plaintext being extracted into a publicly-served document root.

## [2.34.1] - 2026-07-26

### Fixed

- **Scheduled and policy-driven site backups still contained no database.** v2.34.0 added
  databases to the backup the panel button creates, but both automated paths still asked the
  agent for a files-only archive — so the people most reliant on backups, the ones who set a
  schedule and stopped thinking about it, kept getting archives that could not restore their
  content. All three paths now share one resolver, an incomplete scheduled run is logged
  loudly because nobody is watching it, and the backup row records what the archive holds.
- The agent's backup unit tests wrote to `/var/www` and `/var/backups`, so they passed on a
  provisioned server and failed in CI. They now run entirely under a temp directory; the
  indirection exists only in test builds.

## [2.34.0] - 2026-07-26

### Added

- **Site backups now contain the site's databases.** A backup was a `tar` of the document root and
  nothing else, so restoring a WordPress site returned its files and not one post, page, comment or
  setting — and the panel reported it as a success. Each of the site's databases is now dumped into
  the same archive, under `.dockpanel-backup/db/`, alongside a manifest describing what is inside.
  Restoring puts the files back and then loads each dump over the live database. Verified on a fresh
  server the way the gap was found: a published post was deleted and came back.

### Fixed

- **Restoring a MySQL or MariaDB database never worked.** The restore ran `mysql` inside the
  database container, but DockPanel provisions `mariadb:11`, which no longer ships the mysql-named
  client symlinks — so every restore failed with "executable file not found" while the *dump* half,
  which correctly calls `mariadb-dump`, worked fine. Every sibling call site in the codebase already
  used `mariadb`; this one did not. Affected the Databases page, the backup orchestrator, and
  scheduled restores.
- **A restore that lost your database no longer reports success.** Files restored + database not is
  now a failure that names what happened, because at that point the site is running restored files
  against its previous content. A backup that could not dump a database says so at creation time and
  is marked incomplete in the backups list, so "is my content in here?" is answerable before you
  need the answer. A database restore that fails with no error output reports the exit status
  instead of a bare "restore failed:".
- **Restoring an older backup warns first.** Archives made before 2.34.0 hold no database. Restoring
  one onto a site that has one now says so before it starts rather than quietly leaving the content
  as it was.

### Changed

- The `dockpanel backup` CLI states that its archives contain files only, and why: it authenticates
  to the agent, which has no access to the panel's database records. Restoring an archive that
  carries database dumps fails from the CLI rather than restoring the files and calling it done. The
  documented sample output for both commands now matches what the CLI actually prints.

## [2.33.0] - 2026-07-26

### Fixed

- **Nobody could log in to a mailbox.** The Dovecot password file was written `0600 root:root` —
  the instinctive choice for a file full of password hashes, and one notch too strict: Dovecot's
  authentication worker drops privileges to the `dovecot` user, so it could not open the file at
  all. Every IMAP, POP3 and submission login failed with `Temporary authentication failure`,
  while the panel reported the account as created successfully; the only evidence was a
  `Permission denied` line in Dovecot's own log. The file is now written `0640` owned by group
  `dovecot` — still unreadable to every other user on the box, readable by the one process whose
  job is to read it — with the ownership set before the atomic rename, so the live path is never
  briefly published with permissions that lock authentication out. Found by creating a mailbox
  on a fresh box and trying to log in as its owner.

- **Auto-sleep stopped containers that were serving users.** Nothing ever recorded a visitor:
  `last_activity_at` moved only when an administrator woke a container by hand, so "idle" in
  practice meant "nobody used the *panel*". A container answering a request every sixteen
  seconds was stopped on the timer, and the next visitor got a 502 with nothing to wake it.
  The sleeper now asks nginx when the container's domain last served a request and counts that
  as activity. A domain with no access log reports *unknown* rather than zero, because a silent
  zero is exactly what would stop a container nothing is known about.

- **Enabling auto-sleep on a default install did nothing at all, silently.** The sleeper runs as
  a step of the auto-healer, which is off by default and configured on a different page, so the
  switch stored the setting, answered "enabled", and was never acted upon. The setting still
  saves, but the panel now says plainly when the loop that honours it is switched off, and the
  control no longer describes an idleness it was not measuring.

### Changed

- **A site backup says what it actually contains.** Creating one archives the site directory and
  nothing else, while the panel's own records know the site has a database. For a CMS that is a
  fraction of the site: a restore returns the files and not a single post, page or setting —
  confirmed by restoring one and finding a deleted post still gone. The Backups page now states
  that backups are files only and points at where databases are backed up, rather than leaving
  the word "backup" to be read as "my site is safe". Including databases in the archive is a
  larger change to a destructive restore path and is tracked separately.

- **The installer no longer aborts because it could not ask systemd a question.**
  `systemctl is-active` answers "running", "not running" and "I could not reach the bus" with
  two exit codes; during install the bus can be momentarily unavailable, and the call then
  exits non-zero having printed nothing. The installer read that as the service having failed
  and stopped four steps from the end — observed on a fresh Ubuntu 24.04 box, where the
  journal excerpt printed directly under "Agent failed to start" showed the agent started and
  listening. Both service checks now poll for a bounded window and read the answer rather than
  the exit code: `failed` still gives up at once, and a unit that never starts still fails.
  (Delivered from `main`, which is what `install.sh` clones.)

## [2.32.0] - 2026-07-25

### Fixed

- **A site whose automatic SSL fails is no longer dead on both schemes.** WordPress was
  installed at `https://<domain>` unconditionally — including when the certificate step had
  already failed two steps earlier in the same task. WordPress then redirected HTTP to a
  scheme with no certificate, so a brand-new site answered nothing on either one, while the
  panel reported it `active` and the final step said it was "served over HTTP". Sites are now
  installed at the scheme the server can actually serve, and nothing about that claim is a
  guess.
- **A site moves itself to HTTPS the moment it has a certificate.** Nothing in the codebase
  had ever rewritten a WordPress site address, so a site that started on HTTP stayed there
  even after issuance succeeded. Enabling SSL for a site now moves its stored `siteurl`/`home`
  across with it, on every path that can produce a certificate: first provision, retry, DNS-01,
  wildcard, an uploaded custom certificate, git deploys and Docker apps. It only ever replaces
  the plain-HTTP form of that vhost's own domain, so a site deliberately pointed elsewhere — a
  separate front end, a subdirectory install, a `www.` canonical host — is left untouched.
- **A certificate that stops renewing raises an alert instead of a log line.** Both renewal
  loops bailed out with a `tracing::warn!` when no usable ACME contact could be resolved, and
  the security scanner never alerted on a failed renewal at all. That is precisely the failure
  that hides best: issuance succeeds thanks to the panel-wide contact fallback, and sixty days
  later the certificate expires on an unattended server with nobody watching the log. Both
  loops now raise a critical alert, deduplicated per site so a two-minute loop cannot turn one
  stuck certificate into a flood.

### Changed

- **CI's Security Audit job can fail meaningfully again.** It had failed on every commit for
  six releases over one advisory that cannot affect this build (a React Router RSC-mode CSRF
  bypass; both frontends are Vite SPAs with no server runtime), and `npm audit` has no ignore
  mechanism. A gate that is always red reports a new advisory exactly as well as no gate at
  all. `scripts/npm-audit-gate.mjs` now waives individually reviewed advisories — printing the
  reason, and flagging a waiver that no longer matches anything — while still failing the
  build on everything else. Its behaviour is pinned in both directions.

## [2.31.2] - 2026-07-25

### Fixed

- **Creating your first website no longer takes the control panel offline (domain installs).**
  `certbot --nginx` writes a wildcard `listen 443 ssl;` onto the panel's vhost, while
  agent-generated site vhosts bind `<ip>:443 ssl`. nginx treats those as separate listen
  sockets and the explicit-IP one wins every connection to that address, so the panel's
  `server_name` was never consulted: the first site to receive a certificate became the
  de-facto server for the panel's own domain, and the panel answered with that site's
  content and certificate. `setup.sh` now pins the panel's `:443` to the interface IP after
  certbot runs — the same convention `configure_nginx` already applied to `:80` — and
  `update.sh` repairs boxes already in that state. Found by driving a real domain install on
  a fresh box; verified by reproducing the outage before the fix and the survival after it.
- **The listen repair restarts nginx instead of reloading it.** A reload cannot move an
  already-bound `0.0.0.0:443` listener to a specific address — nginx inherits the old socket
  and the rewrite silently no-ops, leaving an on-disk config that disagrees with what is
  running. Both scripts now restart and then verify the socket rather than trusting an exit
  code.
- **`update.sh`'s `BASE_URL` repair now actually runs.** Its guard searched for `BASE_URL` as
  an unanchored substring, which also matches the `DATABASE_URL=` line that `setup.sh` writes
  first into every `api.env` — so the repair was skipped on every install that has ever
  existed. It is now anchored, treats a valueless key as unset, and rewrites in place instead
  of appending a second key.
- **Automatic SSL renewal honours the panel-wide ACME contact.** `security_scanner` and
  `auto_healer` read the site owner's address directly, bypassing the `acme_contact_email`
  fallback that every issuance path uses. A box whose owner address cannot be a Let's Encrypt
  contact (a reserved TLD, a typo) issued certificates fine and then silently failed to renew
  them. Both renewal paths now resolve the contact the same way issuance does.

## [2.31.1] - 2026-07-25

### Security

- **The reserved control-plane domain guard shipped inert in 2.31.0 — upgrade.** 2.31.0 replaced a
  hardcoded list of vendor domains with the panel's own hostname, derived from `BASE_URL`. That
  turned out to be the wrong source: `BASE_URL` is only written when the installer was given a
  `PANEL_DOMAIN`, so on a box whose nginx serves the panel on a real domain it is routinely empty —
  and an empty value reserved nothing at all. A tenant could then create a site for the panel's own
  domain, whose vhost takes over that `server_name`, leaving the panel itself answering 404. This is
  the squat closed in 2.18.0, briefly reopened.

  The guard now also reserves **the host the request arrived on**, which is by definition the address
  the panel is being used at, and needs no configuration to be correct. It is only ever used to
  reserve *more*, so a forged `Host` cannot weaken it. Wired into every domain-introducing path that
  can see request headers: site create, domain rename, alias add and clone. `BASE_URL` and
  `RESERVED_DOMAINS` continue to apply.

  Found by driving the running panel rather than by the test suite — the unit tests passed against
  the same wrong assumption that produced the bug.

## [2.31.0] - 2026-07-25

The first ten minutes with DockPanel. Everything here is something a new operator meets before they
have any way to know what the product expects of them — the account-creation screen, the checklist,
the certificate buttons, the dashboard on a box with nothing on it yet.

The headline is a security fix: on the install path the README advertises, the panel used to serve
the create-your-admin-password form over plain HTTP.

### Security

- **The panel no longer asks for a new admin password over an unencrypted connection.** Installing
  without a domain left nginx serving the panel on `:8443` as plain HTTP, and the very first screen
  is the one that asks the operator to choose an administrator password. There is no Let's Encrypt
  certificate to be had without a domain, so the installer now generates a self-signed one and
  terminates TLS anyway. The browser warns once, which is a thing an operator can reason about; a
  credential travelling in the clear is not. As a consequence the session cookie now also gains its
  `Secure` flag on that path, since the flag follows the connection scheme nginx reports.

### Added

- Visible password guidance on the account-creation screen. The 8-character minimum was previously
  enforced only by the browser's native validation, which rejects the form without ever explaining
  the rule. The text comes from the copy registry, so it is in the generated manual too.
- "Point your domain here" in the Getting Started checklist — the prerequisite that gates HTTPS and
  every guide that follows, and the one step the list never mentioned.

### Changed

- Creating the first admin account now signs you in, instead of redirecting to the login screen to
  retype the password you just chose.
- The site SSL section leads with a single **Secure this site** action. Cloudflare DNS validation,
  wildcard certificates and custom uploads moved behind "Other options", each with a sentence saying
  when it applies — replacing four flat buttons, one of which was the bare acronym "DNS-01 (CF)".
- Restart Nginx / Restart PHP / Reboot no longer appear in the dashboard header on a box with no
  sites and no apps. The two restarts remain on Diagnostics; Reboot returns whenever the system
  reports one is actually required.
- Reserved control-plane domains are now derived from this install's own panel host (`BASE_URL`),
  plus anything in a new `RESERVED_DOMAINS` variable. The list used to be the hardcoded triple
  `dockpanel.dev`, `docs.dockpanel.dev`, `panel.example.com` — our marketing domains and a
  documentation placeholder, compiled into every customer's build, protecting nothing of theirs.
  Matching is on the exact host, so a panel at an apex no longer blocks its own subdomains.

### Fixed

- The Getting Started checklist counted a step nobody had performed: "Run diagnostics" was hardcoded
  to complete, so a brand-new box reported 1/5 done with nothing done.
- `docs/troubleshooting.md` documented removed behaviour, telling operators that the login cookie's
  `Secure` flag follows `BASE_URL` and that editing it fixes a failed login. That has not been true
  since the flag was tied to the connection scheme (#71); the advice did nothing.
- `docs/getting-started.md` sent operators with a domain to `https://your-domain.com:8443`, though a
  domain install moves the panel to the standard HTTPS port.

## [2.30.0] - 2026-07-25

The guidance layer stops being four verticals that share a type and becomes one content system. Its
copy now lives in a single registry, the documentation is generated from that registry rather than
written beside it, and the passive tier — the line under a field, the text behind an (i) — comes from
the same place as the callouts.

The reason this was worth doing is in the Fixed section: the manual was already contradicting the
product, in a way that broke a real user's mail.

### Added

- **One registry of guidance copy** (`services/prerequisites/copy.rs`). Every sentence the guidance
  layer says — nine checks, thirty-odd outcomes — is data in one greppable file, alongside the prose
  a format string has nowhere to put: what the check proves, why it matters, and why it blocks rather
  than warns. A check now decides *which* outcome a situation is; it cannot decide what that outcome
  says, because the four hand-rolled `PrereqResult` constructors are gone and the only remaining path
  goes through the registry.
- **The documentation is generated from it.** `dockpanel-api --emit-guidance <repo>` writes
  `docs/guides/prerequisites.md` — the shipped binary emits its own manual. A test regenerates and
  compares, so changing a sentence without regenerating fails the suite, naming the line and the
  command to fix it. This is the mechanism, not a convention: a page that is emitted cannot describe
  a version of the product that no longer exists.
- **The passive tier joined the same system.** `components/FieldHelp.tsx` renders field help and a
  click-to-open (i) from `content/guidance.generated.ts`, emitted from the same registry and
  type-checked against it, so a typo in a field id is a compile error rather than a silently blank
  hint. Wired on the create-site form, the Docker deploy dialog, the backup-policy destination and
  the mail DNS tab. Per the brief, tooltips and callouts are now one content system rendered at
  different urgencies.
- Field help for the CMS admin password, which had none, saying where the generated password is
  stored — the custody question v2.28.0 fixed but never explained.

### Fixed

- **The manual told you to publish a DKIM record that could never verify.** `docs/guides/email.md`
  documented the selector as `default`; DockPanel has used `dockpanel` for its entire life. Anyone
  who set mail up from the documentation rather than from the panel got a record at the wrong name,
  unsigned mail, and — since v2.29.0 — a DNS check correctly reporting it missing while the manual
  insisted it was right. The same page described the mail host as `mail.example.com` and the SPF
  policy as `-all`, where the product publishes the apex and `~all`. The records section now sends
  you to the panel, which knows your server's address and your domain's key, instead of restating
  values that go stale.
- **Backup destination types.** The Backup Manager guide listed `b2` and `gcs` as destination types;
  the API accepts `s3` and `sftp` and rejects everything else. Backblaze and Google Cloud Storage do
  work — through their S3-compatible endpoints, as type `s3`, which is now what the table says. The
  same guide sent you to "Backups > Destinations", which is not where they live.
- **Troubleshooting contradicted the DNS check.** It stated a domain must resolve to this server's
  own IP; the panel treats a domain resolving elsewhere as a warning precisely because issuance
  through a proxy such as Cloudflare demonstrably works. Resolving to *nothing* is the case that
  cannot succeed, and that is now what the page says.
- **An (i) opened where it could not be read.** The tooltip was clipped by the viewport edge when its
  trigger sat in a form's right-hand column; it now flips to the right edge when it would overflow.
  It is also no longer nested inside a `<label>`, where clicking it dropped the labelled select's
  list over the text it had just opened.
- **A value containing a placeholder was substituted twice.** Copy filling ran a sequence of
  replacements over its own output, so a backup policy named `{available}` came back rendered as a
  number. Filling is now single-pass — the product does not quietly rewrite what an operator typed.

### Notes

- Retention at a remote destination still covers site archives only; database and volume copies
  accumulate there. Now documented in the Backup Manager guide rather than only in the code.

## [2.29.1] - 2026-07-26

One gap only a fresh box could show, found by driving v2.29.0 on one rather than by re-reading the diff.

### Fixed

- **The app name-collision check was inert.** The agent lists containers under the name it actually
  creates them with (`dockpanel-app-pg-ok`), while the deploy form submits the bare name (`pg-ok`), so
  the check compared two strings that could never be equal and reported every colliding name as
  available. Deploying a second app with an existing name still failed — at Docker, after the image
  pull, which is exactly what the check exists to prevent. The panel now strips the container prefix
  before comparing, and the "port is taken by" sentence names the app the way the user does. Pinned by
  a test asserting the contract in both directions.

## [2.29.0] - 2026-07-26

The prerequisite layer stops being a DNS feature and becomes the systemic one the brief asked for.
Three more verticals — Docker apps, mail, backups — and in each of them the check turned out to be the
smaller half of the work: underneath every one was a surface already telling the operator something
untrue.

### Added — three more prerequisite verticals

- **Docker apps: a deploy preflight** (`GET /api/apps/preflight`, `services/prerequisites/apps.rs`).
  Checks the conditions that otherwise surface as `Deploy failed: …` several minutes into an image
  pull: a required setting with no value, a host port that is already taken, a container name already
  in use, and a memory limit larger than the server's free — or total — memory. Rendered immediately
  above the Deploy button, which is gated on the blocking ones. The same checks now enforce the
  deploy server-side, so the sentence a user is shown and the sentence they are refused with are the
  same sentence.
- **Required settings can be generated rather than explained.** 104 of the 153 app templates declare a
  variable that is required and has no default — the database passwords, `SECRET_KEY_BASE`,
  `MEILI_MASTER_KEY`. Secret ones now offer a Generate button (CSPRNG, unbiased, no ambiguous glyphs,
  and working over plain HTTP where `crypto.subtle` would not).
- **Mail: are these records actually published?** (`GET /api/mail/domains/{id}/preflight`.) Reports
  per-record state with the exact record to create, and separately flags a domain whose DKIM key never
  generated.
- **Backups: will any of this survive losing this machine?**
  (`GET /api/backup-orchestrator/preflight`.) One result per enabled policy, plus the prior question
  of whether anything is scheduled at all. Leads the Overview tab, above the SLA card.

### Fixed

- **The Backup Orchestrator ignored the destination you chose.** `backup_policy_executor` selected
  `destination_id` and never read it, so every policy-driven backup — sites, databases and volumes —
  stayed on the disk it was insuring, while the policy form offered a Destination dropdown, the schema
  carried `destination_id` and `uploaded` columns, and the backup list rendered a `remote` badge that
  could never light for a policy row. Policies now upload with the same retry ladder the per-site
  scheduler has always used, prune remote site archives to the retention count, and record which
  destination received the file. An upload that fails degrades the run to `partial` and raises an
  alert instead of reporting success. A destination that is simply down trips a per-run circuit
  breaker rather than costing ~50 seconds for every file.
- **The mail DNS tab published records that could not work.** It read the agent's *hostname* into a
  variable named `server_ip`, so it told operators to create `A mail.example.com → my-server` and the
  invalid SPF value `v=spf1 a mx ip4:my-server ~all`. It also disagreed with the auto-DNS writer on
  the A record's name, the MX target, the SPF policy and the DKIM selector — two different mail
  topologies from one product. Both paths now derive from one record set.
- **"All DNS records verified" could be shown for a domain whose mail was being rejected.** The
  existing mail DNS check tested only that records of the right *kind* existed: any MX passed
  (including one pointing at another provider), any `v=spf1` passed (including one that explicitly
  forbids this server), and DKIM passed on `contains("p=")` without ever comparing our key. It now
  compares against the records we actually publish, while still accepting a stricter SPF, a backup MX
  and the conventional `mail.<domain>` host — and reports a lookup it could not run as *not checked*
  rather than as missing.
- **A refused deploy was invisible.** The Docker apps page rendered deploy errors in a banner ~700
  lines above the deploy dialog, behind the dialog's own overlay. Same defect class as v2.28.0's F1;
  errors now render inside the dialog, next to the button.
- **Site backups could not report being copied off-site.** `backups` never had the `destination_id` /
  `uploaded` columns its two sibling tables have, so the unified backup list hardcoded `FALSE` for
  every site row.

### Known gaps

- Remote retention still covers site archives only: the agent's prune is keyed on a site domain, so
  database and volume copies accumulate at the destination even though their local copies are pruned.

## [2.28.1] - 2026-07-25

Two gaps that only a fresh box could show, found by verifying v2.28.0 on one rather than by reading
the diff.

### Fixed

- **Per-site PHP-FPM pools were created but never used.** v2.28.0 fixed the agent sandbox so the pool
  config finally gets written, and on a fresh box the pool now exists and runs — but the rendered
  nginx vhost still pointed `fastcgi_pass` at the *shared* `php8.3-fpm.sock`, so every request bypassed
  the pool and the per-site "PHP Memory" and "PHP Workers" limits still did nothing. The vhost now
  points at the site's own socket once that socket actually exists; if it doesn't appear after the
  FPM reload the site keeps the shared pool, so a failed reload degrades to the old behaviour instead
  of 502-ing the site.
- **"Site ready" was still claimed for a CMS site whose HTTPS had failed.** v2.28.0 made the terminal
  provisioning step honest for non-CMS sites but only reported the *install* outcome for CMS ones —
  so a WordPress site whose certificate failed still finished on "Site ready". It now reports
  "WordPress installed — HTTPS not configured" (or "…still in progress" when issuance is genuinely
  still running).

## [2.28.0] - 2026-07-25

First slice of the guidance layer, plus the backend-correctness cluster found alongside it. Both come
from a behavioural audit that installed v2.27.0 on a genuinely fresh box with the public one-liner and
drove the real flows, rather than reading the source.

The theme is silence. In every case below, DockPanel already knew what was wrong — and said nothing
where the user was looking.

### Added — the prerequisite layer

- **A shared prerequisite checker** (`services/prerequisites.rs`). Features declare what must already
  be true; the checker returns structured results — state, severity, what was expected, what was
  observed, and the exact record to fix it — instead of a prose sentence at one call site. Severity is
  chosen by consequence: *blocking* only when the action genuinely cannot succeed, *warning* when it
  probably won't, *info* otherwise.
- **`GET /api/preflight/dns`** and **`GET /api/sites/{id}/preflight`** expose it.
- **A three-tier renderer** (`components/Prerequisite.tsx`) — passive helper text, a reactive callout
  carrying detected-vs-expected plus "Check again", or a blocking gate — and a DNS record card that
  shows the record to create with its values filled in and every field copyable, rather than describing
  one. It re-checks in the background while a prerequisite is unmet, so a gate opens by itself once DNS
  propagates instead of leaving the user to guess when to retry.
- **The create-site form now checks the domain as it is typed.** It never blocks creation — a site is
  legitimately created before its DNS exists — but the prerequisite is stated up front instead of
  surfacing later as an unexplained missing certificate.

### Fixed — the silence

- **Clicking "Let's Encrypt" on a domain that doesn't resolve appeared to do nothing at all.** The
  server returned a precise 412 naming the domain, the client received it and stored it correctly — and
  then rendered it at the very bottom of the page, roughly 1,400 lines of markup below the button that
  had been pressed. The message now renders beside the button, with the prerequisite callout under it.
- **"Provisioning complete" was reported for provisioning that had failed.** The terminal step was
  emitted unconditionally: after a CMS install that errored on the line above, and for non-CMS sites on
  a flat 12-second timer while auto-SSL was still retrying behind it at 30s, 120s and 300s. The terminal
  step now reports the real outcome, including "Site created — HTTPS not configured" and why.
- **The auto-generated WordPress admin password was generated, used, and discarded.** The field offers
  "Auto-generated if blank", so a user who took that at its word could not log into the site they had
  just created — while the vault auto-created for that site sat empty. Generated credentials are now
  stored in that vault (encrypted with the key the Secrets Manager reads), and a password DockPanel
  generated is shown once at creation with the rest of the provisioning output.
- **The Let's Encrypt account contact was silently the operator's panel login address, unvalidated.**
  Registering the panel as, say, `admin@box.test` made the CA reject the contact, so every certificate
  failed — with no reason in the UI and none in `journalctl` either, after four silent retries. The
  address is now validated against what the CA actually accepts, a panel-wide **ACME Contact** setting
  can supply a usable address without changing anyone's login, and the reason is stated in Settings and
  in the provisioning output.
- **Certificate expiry was never recorded, on any code path.** The agent reports expiry via the `time`
  crate's formatting (`2026-10-23 09:41:07.0 +00:00:00`); all five panel-side parsers expected a literal
  `UTC` suffix, so every parse failed silently and `sites.ssl_expiry` stayed NULL forever. That starved
  the dashboard SSL countdown, the ARI renewal bookkeeping, and — because its query is
  `WHERE ssl_enabled = TRUE AND ssl_expiry IS NOT NULL` — the entire SSL-expiry alert ladder, which
  therefore had never fired for any site. All five now share one tolerant parser, pinned by tests.
- **Per-site PHP-FPM pools were never created on a fresh install.** The agent unit runs
  `ProtectSystem=strict` and `/etc/php` was missing from its `ReadWritePaths`, while three code paths
  write pool configs there. Every site silently shared the stock `www` pool, voiding the per-site "PHP
  Memory" and "PHP Workers" limits the UI advertises with an Apply Limits button, and the per-site
  process isolation with them. Sites still served traffic, so nothing surfaced it.

### Changed

- **The SSL DNS guard now refuses only what will actually fail.** It previously refused whenever a
  domain did not resolve to this exact address — which includes every Cloudflare-proxied domain. The
  audit drove a proxied domain end to end and issuance *succeeded*, because Cloudflare forwards the
  ACME challenge to the origin. That case is now a warning the user can proceed past; a domain that
  resolves to nothing at all is still refused.

## [2.27.0] - 2026-07-25

Alert-engine reliability and cross-tenant isolation — Tag 2 of the monitoring/alerting/incidents audit
that produced v2.26.0. Where v2.26.0 closed the outbound-request (SSRF) cluster, this closes the
notification-storm, never-resolve, and tenant-boundary cluster: alerts that fired once per minute
forever, alert rows that no code path could ever resolve, and resolves that matched rows by free-text
title across every tenant on the box.

### Fixed — notification storms and alerts that never resolved

- **An expired SSL certificate paged once per minute, indefinitely.** The expired branch fired on every
  60-second tick with no dedup — roughly 1440 pages per day per lapsed certificate to every configured
  channel, one never-purged `alerts` row per minute, and a fresh auto-created incident every 5 minutes.
  It now pages once, and resolves when the certificate is renewed.
- **`memory_leak` (and `disk_forecast`) fired on every tick for as long as the condition held.** Both
  now go through a cooldown gate claimed atomically in `alert_state`, and both resolve when the
  condition clears. The gate is time-based rather than transition-based, so a metric sitting on its
  threshold and flapping with ordinary jitter cannot re-arm the cooldown every tick.
- **`memory_leak`, `disk_forecast` and `ssl_expiry` alerts had no path to `resolved` at all.** They
  stayed `firing` permanently: re-escalated to on-call every 30 minutes forever, and never collected by
  the purge, which only deletes resolved rows.
- **The SSL warning ladder only ever fired its first rung.** It scanned the configured warning days in
  stored order and stopped at the first match, so with the shipped default `30,14,7,3,1` it always
  selected 30 and the dedup test then suppressed every tighter rung — one warning per certificate
  instead of the configured escalation. It now selects the tightest rung crossed.
- **A certificate that lapsed less than 24 hours ago was not treated as expired.** Expiry was derived
  from a day count that truncates toward zero, so a just-lapsed certificate read as "0 days left" and
  fell through to the ordinary rungs — which were already spent. Browsers showed a security warning for
  a full day with no notification on any channel. Expiry is now taken from the signed duration.
- **A failed alert send still advanced the dedup stamp**, permanently silencing that certificate: the
  stamp recorded "already paged" while nothing had been delivered and no row existed to escalate from.
  The stamp is now written only when the send succeeded.
- **User-configured `ssl_warning_days` was ignored.** The threshold lookup returned defaults for every
  site-scoped (non-server) request, which is every SSL check, so custom warning ladders never applied.

### Fixed — cross-tenant isolation

- **Resolving an incident resolved other tenants' alerts.** The auto-resolve matched `alerts` by title
  alone, and titles are generated from the server or service name ("Server vps is offline"), so
  resolving your own incident silently resolved every identically-titled firing alert on the box —
  clearing it from another tenant's dashboard and stopping escalation on a live outage. Now scoped to
  the verified incident owner.
- **The same unscoped-title-match is fixed in three more places**: monitor recovery auto-resolving
  managed incidents, the auto-healer resolving incidents by a `LIKE` substring on a service name (which
  could resolve any tenant's incident whose title merely contained `nginx`, `redis`, `postgres`, …),
  and the alert engine attaching an auto-created incident's first update by title lookup.
- **On-call schedule deletion silently dropped every page routed through it.** Escalation steps
  reference a schedule by UUID inside a JSONB blob, so no foreign key covers them; the code claimed an
  hourly sweep repaired these, and no such sweep exists. Because the initial page also dispatches
  through step 0, every alert bound to that policy stopped reaching email/Slack/PagerDuty entirely.
  Deletion now rewrites the affected steps in the same transaction, deleting a user scrubs their rota
  membership and routes, and escalation policies reject routes to schedules that do not exist.
- **A page is never silently dropped.** If an escalation route resolves to no one — or to users who
  have no notification channels configured at all, which is the common case for a rota member who never
  opened alert settings — delivery now degrades to the alert owner's channels instead of returning
  silently. A deliberate mute is still honoured.

### Fixed — visibility

- **Account-scoped alerts were permanently invisible.** Backup failures, cron failures, security-scan
  findings and SSL alerts are stored with no `server_id`, and the alerts list and its badge both
  filtered on `server_id = <current server>` — so an entire class of alerts, including criticals, never
  appeared in the UI, while still paging on every external channel. The dashboard's firing count,
  acknowledged count, top-issues widget and recent-events feed had the same gap and are aligned with it.

### Fixed — background-loop load

- **The escalation sweep was unbounded and did 3–5 database round-trips per firing alert per minute**,
  before any eligibility check — so accumulating alerts turned it into self-amplifying load on the
  shared pool. It is now bounded, ordered least-recently-paged first so no backlog can starve a live
  alert out of the window, memoises per-tick lookups, and does the expensive work only for rows that
  actually page. Supported by a new partial index.
- **Status-page subscriber emails no longer block the work that triggers them.** Monitor transitions
  fanned out to an unbounded subscriber list serially over SMTP inside the monitor check — stalling all
  monitoring for the duration — and incident updates did the same inside the HTTP handler, holding the
  operator's request open. Both now hand off to one shared worker draining a bounded queue, which
  preserves ordering (so a recovery notice cannot overtake the outage notice that preceded it), caps
  the fan-out, and skips the work entirely when no mail transport is configured.

## [2.26.0] - 2026-07-24

Monitoring & alerting outbound-request hardening — a security audit of the monitoring, alerting, and
incident stack found that several outbound HTTP paths (alert notifications, uptime checks, the webhook
connectivity test, escalation webhooks, and extension webhooks) could be steered at internal addresses
via HTTP redirects or DNS rebinding, and that an uptime keyword check could buffer an unbounded
response body into memory. All are now closed.

### Security
- **Alert/notification webhooks no longer follow redirects and re-validate the destination at send
  time.** Slack, Discord, and generic webhook alert deliveries used an HTTP client that followed
  redirects, so a webhook URL that passed the internal-address check when saved could be 3xx-redirected
  to a loopback / link-local / private address when the alert later fired (SSRF). The shared
  notification client now refuses redirects and re-checks the destination immediately before each send
  (defeating DNS-rebinding), matching the webhook-gateway forwarder.
- **Per-monitor Slack/Discord alert URLs are now validated against internal addresses** on both create
  and update (previously only the global alert-rule URLs were checked).
- **Uptime HTTP checks re-validate the target at check time, refuse redirects to internal hosts, and
  bound the response body.** A monitor URL that resolves publicly when saved but rebinds to an internal
  IP is now rejected when the check runs; a redirect to an internal address (literal or resolving) is
  refused; and the keyword-match body read is capped (2 MiB) so an attacker-controlled target cannot
  stream an unbounded body into memory.
- **The webhook connectivity test and imported-monitor URLs are validated against internal addresses,**
  and the escalation-policy `webhook:` route now checks its URL against internal addresses (previously
  only the scheme was validated).
- **Extension webhook deliveries** (event emit + webhook-route forwarding) now refuse redirects and
  re-validate the destination at send time, closing the same SSRF class as the alert path.

## [2.25.0] - 2026-07-24

Webroot secret scanning — the weekly security scan now flags hardcoded credentials in each site's
document root, extending the existing scanner in response to user feedback.

### Added
- **Hardcoded-secret detection across site webroots.** The agent's full security scan now runs a
  `scan_secrets` pass over `/var/www`, matching high-precision signatures for exposed credentials —
  private keys (PEM), AWS access keys, Google/Stripe/GitHub/SendGrid/Slack API keys, hardcoded JWTs,
  database URIs with embedded passwords, and labeled `key = "value"` credential assignments — in
  source and config files (`*.php`, `*.js`, `*.py`, `*.yml`, `*.json`, …). Findings appear in the
  existing Security tab with a "move it to an environment variable and rotate the credential"
  remediation. The matched secret value is deliberately never stored in a finding, so a scan result
  cannot itself leak a live credential. `.env` files, `node_modules`, `vendor`, and `.git` are
  excluded, and each pattern is bounded (first-match-per-file cap + timeout) so a large or
  adversarial webroot cannot exhaust the agent.

## [2.24.0] - 2026-07-24

Backups & restore integrity hardening, from a multi-agent audit of the backup/restore surface.

### Fixed
- **Policy-encrypted database backups were unrestorable.** Restore recomputed the decryption key from
  a `backup_destinations.encryption_key` column that is never written, while the policy executor
  encrypts with a key derived from the panel's JWT secret — so every encrypted DB backup failed to
  restore (a hard 400). Both sides now derive the key from one shared function; proven end-to-end on
  a fresh box (encrypt → byte-exact decrypt → restore).
- **Verify and drill certified truncated backups as healthy.** The verify/drill rehearsal paths
  ignored the decompressor's exit status, so a boundary-truncated `.gz` decompressed to a valid
  prefix and passed. They now require the decompressor to succeed (bounded so a timed-out child can't
  wedge the task). Site-backup verification, volume-restore validation, and Mongo dump/restore
  streaming were hardened alongside.

## [2.23.0] - 2026-07-24

Security hardening of the File Manager (per-site root-filesystem operations), from a fresh
multi-agent audit of the file-manager surface.

### Security
- **Fixed a File Manager sandbox escape (authenticated tenant → root-owned write outside the site
  root).** The agent's path resolver validated a not-yet-existing target by canonicalizing only the
  first existing ancestor and appending the remaining components unresolved. Because `exists()`
  follows symlinks, a *dangling* symlink (whose target does not yet exist) was treated as an ordinary
  new component, so a create/upload operation followed it and wrote **outside `/var/www/{domain}` as
  root** (e.g. into `/etc/cron.d`). The resolver now rejects any symlink in the to-be-created path;
  legitimate in-root symlink directories still work. Covered by new tests.
- **Removed the unused `_server` magic-domain upload branch** in the agent — a latent root-write
  primitive (able to target `/etc/nginx`, `/etc/dockpanel`, `/home`, `/opt`) that used a weaker,
  hand-rolled traversal check and had no caller anywhere in the panel.
- **Hardened the download filename header** — the backend's `Content-Disposition` fallback now
  sanitizes the filename (quote/backslash/CRLF) to match the agent, preventing quoted-string breakout.

### Fixed
- **File edits no longer clobber a same-named `.tmp` sibling.** The atomic-write temp file used
  `with_extension("tmp")`, so saving `report.php` could destroy an existing `report.tmp`. It now uses
  a collision-free, fixed-length hidden temp name and cleans it up on failure.
- **Directory listings never leak an absolute server path** — the relative-path strip now fails closed
  to the bare filename when `/var/www` is itself a symlink.

## [2.22.1] - 2026-07-23

### Fixed
- **Suspending a user no longer fails with a 500.** The `chk_users_role` check constraint (recreated
  by the reseller migration) allowed only `admin` / `reseller` / `user`, so writing the `suspended`
  role that suspension uses violated the constraint on every install — suspension had been completely
  non-functional. The constraint now includes `suspended`. Surfaced by driving the v2.22.0 suspension
  hardening against a live database.

## [2.22.0] - 2026-07-23

Authz & multi-tenancy hardening. A fresh audit of the account/role/tenancy core (users, teams,
resellers, and the auth extractors) found that the panel's revocation and suspension machinery did
not actually cut off an active session, that reseller quotas could be raced past their limits, and
that team invites could be abused as an email relay. All fixed.

### Security
- **Suspend / role-change / password-reset / delete now actually revoke the user's active session.**
  These admin actions previously deleted the session rows but never blacklisted the token — and the
  panel authorizes from a stateless 2-hour JWT that carries the role — so a suspended (or demoted, or
  password-reset, or deleted) user kept full access with their *old* role until the token expired; a
  demoted admin could even re-promote themselves in that window. Every de-escalation path now
  blacklists the token(s) through the same choke-point the self-service password-reset already used.
- **Suspension is enforced at login.** Password, 2FA, and OAuth login now reject suspended accounts
  (previously only the passkey path did), so a suspended user can no longer simply log in again.
- **Admins can't orphan the panel of admins.** An admin can no longer change their own role, and the
  last remaining admin can no longer be demoted.
- **Reseller quotas are enforced atomically.** User- and site-creation used a check-then-increment
  that concurrent requests could race past the plan limit; both now reserve the slot in a single
  atomic statement (matching how database creation already worked).
- **Promote-to-reseller clears the ownership pointer** so a newly-promoted reseller can no longer be
  managed or deleted by their former parent reseller.
- **Team invites can't be used as a spam/phishing relay.** The invite email now HTML-escapes the team
  name and inviter address, invites are rate- and count-capped per team, and an invite can only be
  redeemed by the email address it was issued to.
- **OAuth logins are now recorded as sessions** (they previously left no session row, so no revocation
  sweep could reach them) and are subject to the same suspension and revocation controls.

## [2.21.1] - 2026-07-23

Installer overhaul + PHP 8.5 support. A fresh Ubuntu 26.04 install surfaced that PHP could not be
installed (by the installer or the panel), the installer's summary claimed PHP-FPM was installed
anyway, and the failed attempt left broken apt sources behind. All fixed, plus a rebuilt install
experience.

### Fixed
- **PHP installs on Ubuntu 26.04 / PHP 8.5 distros.** PHP 8.5 made OPcache built-in, so
  `php-opcache` has no installation candidate there — and that one dead package name failed the
  installer's all-or-nothing apt transaction, cascading into a ppa:ondrej/php fallback that
  publishes nothing for `resolute` yet. The installer and both agent PHP installers now install the
  PHP core first, add only the extension packages the apt source actually has candidates for, and
  fall back to a 3rd-party repo only after confirming it publishes for this release.
- **A failed 3rd-party PHP repo attempt no longer breaks apt.** The old flow left
  `ondrej-ubuntu-php-*.sources` behind after a failed add, so every later `apt-get update` on the
  box errored with a 404. Installer and agent now remove the dead source and refresh the index.
- **The install summary tells the truth.** "Installed services" was a hardcoded list that named
  PHP-FPM even when its install had just failed. The summary now reports exactly what succeeded
  (with versions) and what didn't, with the retry path and the install log location.
- **Settings → Services → Install PHP worked only up to PHP 8.3.** Version detection probed
  8.3/8.2/8.1 and fell back to a php8.3 install no repo could satisfy on newer distros. It now
  reads the distro's default from `apt-cache depends php-fpm` first, then probes newest-first.
- **PHP 8.5 selectable everywhere** — site create/detail dropdowns, the site PHP-switch API, the
  CLI, the agent allowlist, PHP-FPM pool sweeps, and the IaC export.

### Added — installer experience
- **Live progress**: long operations show an animated spinner with elapsed seconds (static lines
  when not a terminal).
- **Failure box**: if the install dies anywhere it names the exact step, shows the last install-log
  lines, the full log path, and the safe re-run command — instead of stopping mid-paint.
- **Full install log** at `/var/log/dockpanel-install.log` (0600) capturing every step's output.
- **Release-asset integrity**: binaries + frontend downloads are sha256-verified against the
  release's `checksums.txt` (mismatch aborts); downloads retry on transient network errors.
- **Honest progress bar**: sized to the steps that will actually run — no more 93% → 100% jump.
- **All input up front**: the domain prompt happens before the first step, so the install never
  stops to wait for a human once it starts.
- Version line in the summary; `✓` only ever marks completed facts (in-progress actions animate),
  and an SSL-provisioning failure prints as a warning instead of a checkmark.

## [2.21.0] - 2026-07-22

Git Deploy security & correctness hardening — the audit rotation's fresh-eyes pass over the Git
Deploy surface (backend routes, webhook gateway, deploy scheduler/preview cleanup, agent git_build).
Operator-only surface (admin-gated), so no cross-tenant exposure; backend + agent, runtime-only.

### Fixed
- **Preview cleanup actually tears previews down.** `git_previews.container_name` is stored with the
  `dockpanel-git-` prefix the agent re-adds; the three automatic cleanup callers (TTL, stuck-preview,
  webhook branch-delete) passed it verbatim, so the doubled prefix made teardown a no-op that leaked
  the container, image, nginx vhost, SSL cert, and a still-bound port. All callers now strip the
  prefix first.
- **The deploy concurrency lock now exists.** The old guard queried `git_deploy_history` for a
  `building` status that is never written there, so it never fired. Replaced across all three deploy
  paths with an atomic conditional UPDATE on `git_deploys.status` (30-minute self-heal window with a
  heartbeat before long builds; a DB error is a 500, not a false "already building" 409), with the
  config fetch ordered before the lock so a fetch error cannot strand it.
- `deploy_cron` is now format-validated to exactly what the scheduler accepts (stored values are
  grandfathered on edit).

### Security
- **Domain-validation parity** for deploy domains: create/update reject invalid and reserved
  domains (grandfathering unchanged values); the agent adds a defense-in-depth floor blocking nginx
  metacharacters before the config write; preview subdomain slugs are DNS-sanitized.
- **SSRF hardening**: the webhook gateway HTTP client no longer follows redirects (a 302 could
  bounce the allow-checked URL to 127.0.0.1), and the shared internal-address validator now blocks
  IPv6 ULA/link-local, IPv4-mapped IPv6, and CGNAT ranges (also protects monitors and alerts).

## [2.20.0] - 2026-07-22

Tenant-database isolation — the follow-up to v2.19.0's Databases audit, closing the two isolation
findings it deferred. Both are runtime-only (no migration); MariaDB was already isolated and is
unchanged.

### Security
- **PostgreSQL tenant databases no longer run as the cluster superuser (in-container RCE foothold
  closed).** A per-site postgres database was provisioned with `POSTGRES_USER={name}`, which the
  postgres image bootstraps as the cluster **superuser** — so the database's own owner could
  `COPY ... TO/FROM PROGRAM` (arbitrary command execution inside the container) and read server
  files. New postgres databases now keep the image-default `postgres` superuser with a random,
  discarded password (reachable only over the in-container socket, never over the published port),
  and the tenant connects as a separately-provisioned **`NOSUPERUSER`** role that owns only its own
  database and `public` schema. The tenant login name, password and connection string are unchanged;
  it can still create/drop tables, browse, back up/restore and rotate its own password, but
  `COPY..TO/FROM PROGRAM` and server-file reads are now denied. Existing databases keep their prior
  role (the change applies to newly-created databases).
- **Cross-tenant lateral movement between database containers closed on existing installs.** All
  per-tenant database containers share the `dockpanel-db` bridge; v2.19.0 disabled inter-container
  communication (`enable_icc=false`) but only when the network was first created, so installs whose
  network predated that kept ICC on (and even new databases there joined the ICC-on network). The
  agent now reconciles an existing ICC-on `dockpanel-db` network to `enable_icc=false` (one-time,
  idempotent), so a compromised or abusive database container can no longer address sibling tenants'
  database containers. Published `127.0.0.1` ports are preserved across the reconcile.
- **Reserved database names.** Because a postgres tenant is now a real role that owns its database, a
  database name matching a system/admin identity (`postgres`, `template0`, `template1`, `mysql`,
  `sys`, `information_schema`, `performance_schema`) is rejected at creation.

## [2.19.0] - 2026-07-22

Databases-surface security hardening — the audit-coverage rotation's fresh-eyes pass over the
per-tenant Databases surface (`routes/databases.rs`, the agent DB container + backup services,
and the `db-backup` orchestrator handlers). The Databases nav is not admin-only, so every authed
user reaches these; a 10-lens REFUTE-verified audit found 19 confirmed issues.

### Security
- **Cross-tenant database password reset (BOLA/TOCTOU) closed.** Every exec/reset path resolved
  the target Docker container by the per-site (non-unique) name `dockpanel-db-{name}`. During
  `create()`'s agent round-trip a tenant's transient colliding-name row (empty `container_id`) was
  list-visible, letting a `reset-password` call reach another tenant's like-named container (the
  agent's MariaDB reset authenticates as root over the unix socket, needing no victim password).
  `get_db_info` now refuses to operate on a row with an empty `container_id`; Docker's global name
  uniqueness guarantees a non-empty `container_id` owns its uniquely-named container.
- **Per-account database cap + wider port pool.** A single tenant could create databases until the
  shared, host-wide DB port pool was exhausted, denying database creation to everyone. Added a hard
  per-account cap (25) and widened the postgres/mysql port ranges.
- **Reseller database quota is now race-free.** The check-then-increment was TOCTOU-racy (concurrent
  creates bypassed `max_databases`); replaced with an atomic conditional `UPDATE ... RETURNING`
  reservation, released on failure.
- **Query output is streamed with a hard cap + `kill_on_drop`.** `execute_query` buffered the entire
  result set in the agent's memory (the 5 MB cap was checked only after buffering), so a large-output
  query could OOM the shared agent; it now streams with an enforced cap and kills timed-out children.
- **Inter-container communication disabled on the shared DB bridge** (`enable_icc=false`, new
  networks) to cut lateral movement between tenant DB containers; DB containers also get a CPU quota
  (like app containers) so a heavy dump/restore cannot starve co-tenants.
- **Restore no longer reports success on a failed/partial restore.** The postgres/mysql restore
  pipeline discarded `gunzip`'s exit status and ran `psql` without `ON_ERROR_STOP=1
  --single-transaction`, so a truncated/corrupt archive imported partially yet returned success. The
  decompression status is now checked and postgres restores fail-and-roll-back on any error; new
  postgres dumps use `--clean --if-exists` so a restore overwrites rather than merges.
- **Point-in-time recovery made honest.** `pitr_restore` returned a false `ok:true` (the postgres
  path only wrote a WAL marker; the mysql path posted to a non-existent agent route) — it now returns
  `501 Not Implemented`. PITR enable no longer runs the un-revertable, disk-filling `archive_mode=on`
  mutation; only the intent flag is persisted until real WAL archiving lands.
- Defense-in-depth: `restore_db_backup` gained the traversal guard its `delete` sibling already had;
  `is_safe_db_identifier` rejects `..`/`/`; SQL error responses strip the Docker-daemon prefix.

Backend + agent only; runtime-only (no migration). Verified live on the published 2.19.0 demo.

## [2.18.0] - 2026-07-22

Sites & SSL security hardening (audit-coverage rotation). A 10-lens REFUTE sweep over `sites.rs`,
`ssl.rs`, and the agent nginx/ssl services (a per-user-ownership BOLA surface) → 15 confirmed fixes:
2 critical nginx-injection paths (raw `csp_policy`/`permissions_policy` into `add_header` on a
non-autoescaped `.conf`; `custom_nginx` `;`-chaining past a per-line-token validator), 4 high
(domain-hijack via `add_alias`; reserved-domain squat via rename/clone; clone admission-control
bypass; unvalidated `proxy_port` → loopback SSRF + ufw-deny outage), 2 medium (SSL renew + toggle
handlers rebuilt the vhost from a subset config, dropping WAF/CSP/rate-limit — now via
`build_nginx_body`), 1 low (TLS keys written 0600-atomic). Shared `is_reserved_domain` +
`ensure_domain_available`; `is_safe_header_value`; per-statement `validate_custom_nginx`;
`is_safe_proxy_port`. Backend + agent, runtime-only.

## [2.17.0] - 2026-07-22

Cross-surface authorization-guard sweep. After the authz-inversion class (a state-changing handler
weaker than its admin-gated read siblings) recurred twice, a 21-lens REFUTE sweep of all 62 route
files → 10 confirmed inversions admin-gated: CDN (all 8 endpoints + de-scoped get/list), migration
(all 6), backup-policy CRUD (all 5), incident writers (→ AdminUser, also closing a cross-tenant
alert-resolve BOLA), plus surgical guards on `install_php_extension`, `ws_metrics`, and vault-scoped
`secrets` metadata, and an executor volume-branch owner-is-admin defense-in-depth check. Backend
only; runtime-only. The frontend already gated all of these.

## [2.16.0] - 2026-07-21

DNS-surface security hardening — the audit-coverage rotation's fresh-eyes pass over the
DNS management surface (`routes/dns.rs`, the DNS page, the agent PowerDNS installer, and
the DNS-01 SSL path), which had never had a behavioral security audit. The headline fix
closes a cross-tenant DNS-takeover hole; the rest tighten credential handling, fail-closed
behaviour, and input validation.

### Security

- **DNS management is now admin-only.** The zone/record write endpoints (`create`/`delete`
  zone, `create`/`update`/`delete` record) and the zone/record listings were reachable by
  any authenticated user, while every read/analytics endpoint already required admin. A
  non-admin could create and fully control zones and records on the panel's shared
  authoritative PowerDNS server for domains they did not own — traffic redirection, MX
  interception, or a planted `_acme-challenge` TXT to fraudulently pass an ACME DNS-01
  challenge. Every DNS and tunnel handler now calls `require_admin`, and the DNS page is
  gated in the navigation.
- **Cloudflare API tokens are encrypted at rest.** Zone tokens were stored in plaintext in
  `dns_zones.cf_api_token` while the PowerDNS key was already encrypted. Tokens are now
  encrypted with the shared credential cipher on write and decrypted transparently at the
  single Cloudflare-header choke point (existing plaintext rows keep working via the legacy
  fallback — no migration required).
- **Cloudflare Tunnel token no longer world-readable.** The agent wrote the tunnel token
  into a 0644 systemd unit and onto the `cloudflared` command line. It now lives in a
  root-only (0600) `EnvironmentFile` and is passed via `TUNNEL_TOKEN`, never on the
  command line.
- **DNS-01 challenge TXT records are always cleaned up.** Four early-return error paths in
  the wildcard-certificate flow could leave `_acme-challenge` TXT records dangling in
  Cloudflare (a sub-domain-takeover surface); cleanup now runs on every exit.
- **`delete_zone` fails closed.** A PowerDNS zone-deletion error (or unreadable settings)
  previously still removed the panel's tracking row, orphaning an authoritative zone; the
  DB row is now dropped only after the authoritative zone is confirmed gone.
- **`dig`-based propagation / health checks reject option injection.** The DNS-name and
  domain validators now reject a leading `-`/`+` (covered by a new unit test), so a crafted
  value can never be parsed by `dig` as an option.
- **PowerDNS API error bodies are no longer reflected to the client** — they are logged
  server-side and a generic message is returned.
- **Input validation at the door** — `create_zone` now validates the domain and the
  Cloudflare Zone ID before use.

### Fixed

- The DNS change-log endpoint now surfaces a database error instead of silently returning
  an empty history.
- The "Purge Entire Cache" action now asks for confirmation before purging.
- Zone list-load failures are shown as an error instead of the empty "add a zone" state.
- Zone templates no longer write the operator's browser IP (via `api.ipify.org`) as the
  domain's A record.
- The Cloudflare Tunnel configuration comment no longer claims to store a token hash, and
  the settings write is no longer silently discarded.

## [2.15.0] - 2026-07-21

Docker Apps hardening, round two — closing the meatier findings deferred from the
v2.14.0 audit-coverage rotation. All four fixes tighten the admin-only Docker
app-management surface: they refuse where they previously silently allowed, gate a
footgun endpoint, and remove a security control that was displayed but never
enforced.

### Security

- **Container action handlers now verify the target is a DockPanel-managed
  container.** `stop` / `start` / `restart` / `logs` / `exec` / `remove` /
  `change-image` / `update` / `env` / `snapshot` / `update-limits` and the Ollama
  model endpoints previously validated only the container-ID *format*, so a
  well-formed id for the panel's own infrastructure (its PostgreSQL / API / agent
  containers) would be acted on. Each handler now inspects the container and
  refuses (403) unless it carries the `dockpanel.managed=true` label — the same
  boundary the app *list* already enforced, closing a severed read/write scope.
- **`activity-ping` is now admin-only.** The endpoint that resets a container's
  auto-sleep idle timer accepted any authenticated user, so a non-admin could keep
  an arbitrary container awake and defeat auto-sleep. It now requires an admin, in
  line with the sibling wake/sleep handlers. (Its documented "nginx keepalive"
  purpose was never reachable — an nginx reverse proxy cannot present a user JWT —
  so no legitimate caller is affected.)

### Changed

- **Removed the `network_isolation` container-policy control.** The per-user policy
  toggle was persisted and displayed but read by nothing at deploy time — a
  security-labelled control that did nothing, giving false confidence. It has been
  removed from the API and the Container Policies UI. (The dormant database column
  is left in place; no migration.) Real per-user network segmentation, if wanted,
  is tracked as future work.

### Fixed

- **Per-user container quota now fails closed.** When the container-count check
  could not reach the agent (or received a malformed response), deploy previously
  proceeded and silently bypassed the user's `max_containers` limit. A count-check
  failure now refuses the deploy (502) instead of allowing over-quota.

## [2.14.0] - 2026-07-21

Docker Apps hardening — a security, correctness, and safety pass over the Docker
app-management surface (an s237 audit-coverage rotation; the largest and
never-before behaviorally-audited surface). Fixes a routine "change image"
operation that silently stripped a container's isolation, networking, and panel
management; removes an over-broad capability from GPU containers; makes deploy
resource limits actually apply; and adds confirmations/feedback to three
destructive or silent UI actions.

### Security

- **GPU containers no longer receive `CAP_SYS_ADMIN`.** Deploying a template with
  GPU passthrough enabled added `SYS_ADMIN` on top of the otherwise `cap_drop ALL`
  hardened container — a near-root capability that enables container→host escape
  (`mount(2)` / cgroup `release_agent`) and reverses the sandbox. GPU compute via
  the NVIDIA Container Toolkit does not require it, so the capability has been
  removed; GPU containers now keep the same minimal cap set as every other
  template.

### Fixed

- **"Change image" no longer downgrades a container's security or breaks its
  reverse proxy.** The operation recreated the container with a bare `docker run`
  that dropped every hardening flag — `cap_drop ALL` + the minimal cap allowlist,
  `no-new-privileges`, the `127.0.0.1:<port>` publish (so the nginx reverse proxy
  returned 502), the restart policy, the memory/CPU limits, the environment
  variables, and the `dockpanel.managed` labels (so the container disappeared from
  the panel and became unmanageable). It now inspects the existing container and
  recreates it preserving all of that, only swapping the image.
- **Deploy-time CPU limits above one core now apply.** A deploy requesting
  `cpu_percent > 100` (more than one core) silently received no CPU limit at all,
  so the container ran with unlimited CPU. The deploy path now applies the limit
  for any positive value, matching the live "update limits" path (1–10000%).
- **Deploy resource requests are always bounded.** Memory/CPU limits were only
  validated when the operator had configured a per-user container policy, so a
  default operator's deploy was unbounded. Deploy now clamps `memory_mb` to
  4–65536 and `cpu_percent` to 1–10000 unconditionally (a per-user policy remains
  an additional, tighter ceiling), and the memory→bytes conversion is
  overflow-safe.
- **Deleting an Ollama model now reports failures.** A failed model deletion was
  swallowed silently (the model stayed listed with no error); the failure is now
  surfaced.

### Changed

- **Destructive Docker-app actions now require confirmation.** "Prune Unused
  Images" (removes all unused images host-wide, irreversibly), "Remove" on a
  Compose stack (tears down every container in the stack), and "Remove" on an
  Ollama model (a multi-GB delete) now use the same two-step Confirm/Cancel
  already used for removing a single app.

## [2.13.1] - 2026-07-21

Mail-surface hardening — a security and correctness pass over the mail-server
management surface (an s236 audit-coverage rotation). Fixes a defect that made
mailbox login impossible, closes a path traversal and two world-readable
secret-file issues, and removes several ways mail config could silently drift.

### Fixed

- **Mailbox login now works.** Mail-account passwords were hashed with a single
  unsalted-round SHA-512 mislabeled `{SHA512-CRYPT}` — a value Dovecot's crypt(3)
  verifier rejects for every password, so IMAP/POP3/SMTP-AUTH/webmail login had
  never actually authenticated. Passwords are now hashed with **Argon2id** and
  stored in Dovecot's `{ARGON2ID}` scheme (a real key-stretching KDF, verified
  against Dovecot 2.3.21). **Existing mailboxes must have their password reset
  once** to receive a working hash.
- **Path traversal in mailbox restore.** `POST /api/mail/restore` built the
  extraction directory from the request e-mail without rejecting `..` / `/`, so a
  crafted address could redirect the `tar` extraction and a recursive `chown`
  out of `/var/vmail` into agent-writable root-daemon config directories. Restore
  now applies the same path validation the backup path already enforced.
- **World-readable secret files.** The Dovecot users file (password hashes) and
  mailbox-backup tarballs (plaintext mail) were written at the process umask
  (0644). They are now **0600**, and the mailbox-backup directory **0700** —
  parity with the DKIM key and SASL password files.

### Changed / Hardened

- Mail address fields (account e-mail, alias source/destination, catch-all,
  forward-to) are validated at the API against the same character set the agent
  enforces, so an out-of-charset value returns **400** instead of being stored
  and then silently wedging every future Postfix/Dovecot sync.
- Deleting or disabling a mail domain now rebuilds the Postfix/Dovecot maps
  immediately, so a decommissioned domain's mailboxes stop authenticating and
  receiving right away (previously they stayed live until an unrelated change).
- `delete_alias` is scoped to its domain; mailbox quotas are clamped to a sane
  range; Postfix `mynetworks_style` is pinned to `host` so `permit_mynetworks`
  can't trust a shared subnet; and `tar` invocations use `--` before
  user-derived operands. Added the mail surface's first unit tests.
- Added `/var/vmail` to the (sandboxed) panel agent's `ReadWritePaths` so it can
  create and own mailbox maildirs — previously blocked by `ProtectSystem=strict`,
  so a fresh mailbox had no maildir until Dovecot lazily created one and couldn't
  be backed up. Takes effect when the agent unit is redeployed by update/install.

### Notes

- Deferred (tracked in tech debt): the mail tables are global while writes go to
  whichever server the active `X-Server-Id` selects — a multi-server split that
  needs a pin-or-fan-out design decision; and the shared msmtp relay log is
  world-writable (needs a per-pool / syslog restructure).

## [2.13.0] - 2026-07-20

Phase 4 W5 — **fleet configuration-drift detection**. A read-only report that
answers "is my fleet's operational posture consistent?" from one card.

### Added

- **Fleet Configuration Drift report** (Telemetry → Updates → *Fleet
  Configuration Drift*). Pick a reference server and see, per entity, where every
  other server in the fleet diverges from it: **alert rules** (monitoring
  posture), **sites** (inventory asymmetry + per-site config — WAF, SSL, PHP,
  caches, limits), **cron jobs**, and **backup coverage** (how many sites are
  unprotected per server). Read-only and computed on demand from the panel's own
  database — no remote agent call, so even an offline member is comparable, and
  no background scan. Secret-bearing fields (webhook URLs) are compared by
  presence only, never by value. New endpoints `GET /api/drift/servers` and
  `GET /api/drift`. Admin only.

### Notes

- **Report only.** Reconcile (push a source-of-truth server's config to the
  others) is intentionally not in this release — it is cross-server mutation with
  no existing transport, and DockPanel keeps that surface explicit and confirmed.
  Comparing a member's live on-box state against its declared config is a
  separate later leg.

## [2.12.1] - 2026-07-20

Ship-path and CI hardening. No behavioural change to the panel or agent — this
release makes the machinery that builds, publishes, and installs DockPanel fail
loudly and verify what it downloads.

### Security

- **The panel updater now verifies every release asset it downloads.**
  `scripts/update.sh` fetches the release's `checksums.txt` and checks the
  sha256 of the agent, API, CLI, and frontend before installing any of them —
  failing closed if the checksums file is missing, has no entry, or disagrees.
  This is the guarantee the agent self-updater already gave; it closes the
  parity gap where the panel path installed unverified bytes.

- **CI security audits are now enforcing.** The Security Audit job dropped the
  `|| true` that made `cargo audit` (×3) and `npm audit` (×3) unable to fail,
  and it now also audits `website/server`. A real advisory fails the build
  instead of scrolling past green. (The pre-push hook has enforced this locally
  since 2.11.1; CI now agrees.)

### Fixed

- **A Sigstore outage can no longer lose an entire release.** At 2.11.8 the
  release job died fetching the cosign installer — every binary built and the
  tag existed, but no GitHub Release was ever published. The cosign install is
  now retried, checksum-verified against Sigstore's own published sums, and
  non-fatal: if signing is unreachable the release still publishes (unsigned,
  and it says so in the run summary) rather than being lost.

- **`update.sh` no longer aborts before installing binaries on non-git
  layouts.** On a hand-built `/opt/dockpanel` with no repo tree, copying the
  canonical systemd unit failed under `set -euo pipefail` and stopped the whole
  update before the binary swap. It now keeps the existing on-disk unit and
  continues.

- **The agent installer no longer references a panel download route that does
  not exist.** `install-agent.sh`'s fallback hit `/api/agent/download`, which
  was never implemented, so it only stacked a confusing error on top of a real
  one. Removed, with an honest message pointing at the actual cause.

### Added

- **The release smoke-test now exec-proves the arm64 binaries.** Every release
  already verified the amd64 assets are static and load cleanly across the
  distro matrix (#70); the published `linux-arm64` assets are now run under QEMU
  emulation to prove they reach `main()` too.

## [2.12.0] - 2026-07-20

### Added

- **Agents can now keep themselves on the panel's release, and it is off until
  you say otherwise.** Settings → Telemetry → Updates has an **Agent
  Auto-Update** switch. With it on, every remote agent asks the panel every ~6
  hours whether it should move, and a box that is behind updates itself using
  the same checksum-verified, health-verified, rollback-capable updater a fleet
  rolling update uses. With it off — the default, including on upgrade — the
  panel answers every agent "nothing to do", so nothing moves unless you start a
  fleet update yourself.

  Setting the update channel to **Hold** overrides the switch: nothing moves at
  all. The switch is enforced by the panel rather than by the agent, because an
  agent only ever learns things by asking — there is no way to push
  configuration to one.

  This is what closes the gap that made a fleet fix unable to reach existing
  installs: before this, bringing boxes onto a new agent meant either a fleet
  run from the panel or re-running `install-agent.sh` on each one.

### Fixed

- **The agent's periodic update check had never once worked, and the way it
  failed made a broken fleet look like a healthy one.** It failed twice over: it
  sent no `Authorization` header, and `GET /api/agent/version` required a signed
  user token — a credential an agent structurally cannot hold, since it has a
  random token issued at install time. So every check since 2.10.0 was answered
  `401`.

  What kept it hidden for four releases: the agent parsed the error body as if
  it were a version answer. An error body is still valid JSON, the `version`
  field was simply absent, the code fell back to the agent's own version, the
  two compared equal, and it logged **"Agent is up to date"** — at `debug`
  level, below the log level agents actually run at. A permanently dead update
  path was indistinguishable in the journal from a fleet with nothing to do.

  The check now authenticates with the agent's own token, and **inspects the
  HTTP status before the body**: a non-2xx is a warning that names the status
  and is never reported as being up to date. Pinned by a test.

- **The check no longer replaces binaries itself.** It used to download the
  asset, hash it, write a backup that nothing in the codebase ever read back,
  and rename over its own running executable — with no check that the new agent
  came up, and so no way back if it did not. It also staged through `/tmp`,
  which is a cross-device rename (and a hard failure) on any box where `/tmp` is
  a tmpfs. That work now goes through `scripts/agent-self-update.sh`, the same
  updater the fleet path uses: digest checked against the release's own
  `checksums.txt` before anything is installed, atomic swap inside the target
  directory, the new agent proven to answer `/health` on the expected version,
  and a real rollback when it does not.

- `GET /api/agent/version` no longer advertises a download URL and checksum read
  from three settings rows **that nothing in the product ever wrote** — no
  installer, no migration, no release step, no UI. They were unreadable in
  practice and, being single values, could never have been correct for a fleet
  mixing amd64 and arm64 boxes anyway. The endpoint now returns only the target
  version; each box derives its own asset and digest. The three dead keys are
  removed from the settings allowlists and deleted on upgrade.

- `/api/agent/version`, `/api/agent/commands` and `/api/agent/commands/result`
  now share one implementation of agent authentication and one rate limiter
  (120 req/min per server). The version endpoint previously had neither, under a
  comment claiming it had both. (`/api/agent/checkin` keeps its own check, which
  identifies the server from the request body and compares the token in constant
  time; the route comment now says so rather than implying otherwise.)

- A fleet update to a target that fails **fast** no longer wedges the box at
  `409 an update is already in flight`. The agent records its verdict with
  whole-second timestamps while the run's start time is sub-second, and the
  liveness check compared them directly — so a run that reached its verdict
  inside the same wall-clock second it began (a mistyped or unreleased target
  404s in ~0.25s) had its verdict judged to belong to a previous run, leaving
  the box `InFlight` for ever. This is the exact wedge the v2.11.8 liveness
  predicate was written to prevent, defeated by the fastest path through it; the
  comparison is now at whole-second resolution on both sides. (It is also why
  the two-box fleet test looked intermittent.)

- An agent will no longer retry a version that already failed on it *after*
  replacing the binary. Because a failed update leaves the agent on its old
  version, the "am I behind?" test stays true for ever — so a release that
  installs but does not come up on a particular box would otherwise be
  downloaded, swapped in, health-checked, rolled back and restarted again on
  every cycle, indefinitely. The agent now reads its own last verdict, refuses
  that specific target, and says so in the journal. Failures *before* the swap
  (a missing release, a checksum mismatch) are cheap and still retried, and an
  operator-driven fleet update always overrides.

### Notes

- Existing agents are unaffected until you switch this on. Agents older than
  2.12.0 do not understand the new response and simply carry on doing nothing —
  they cannot be pushed into a broken update by it. To bring them onto a version
  that supports this, use a fleet rolling update as before.

## [2.11.9] - 2026-07-19

### Fixed

- **The agent updater's rollback could say it had restored the previous binary
  without having restored anything.** If the newly-installed agent failed to
  come up, the recovery path ran
  `mv "$BACKUP" "$AGENT_BIN" && systemctl restart … || true` and then recorded
  `previous binary restored` whatever happened — so a failed restore was
  indistinguishable from a successful one, on a box that was by definition
  already in trouble. This is the same shape as the `update.sh` rollback that
  printed "Rolled back to previous binaries" over a box it had not rolled back
  (fixed in 2.11.4), reintroduced one release later in the new agent updater.

  The restore's own status is now branched on, and the outcome written to
  `/var/lib/dockpanel/last-agent-update.json` is what the agent reports about
  **itself** afterwards — distinguishing "restored, agent reports X" from
  "could not restore" and from "restored the old binary but the agent is not
  answering". A test pins the shape, negative-controlled against the exact line
  that shipped in 2.11.8.

- The agent updater now removes its staged binary if it dies between staging
  and installing, instead of leaving ~21 MB beside the real one.

## [2.11.8] - 2026-07-19

### Fixed

- **The fleet rolling update could not update a single one of the servers it
  was built for, and once that was unblocked it reported success on a box that
  never moved.** Both halves were found by running the path against a real
  remote agent for the first time, on a two-machine lab.

  - `scripts/install-agent.sh` — the only documented way to add a remote server
    — never creates `/opt/dockpanel`. The agent's update receiver required
    `/opt/dockpanel/scripts/update.sh` and refused with
    `500: update script not found` in 166 ms, so the feature's success rate on
    its entire target population was zero. It failed safely, at least: nothing
    on the remote box was touched.
  - Planting that repo, which is the obvious fix, was probed rather than
    shipped — and it turned the loud failure into a silent one. `update.sh` is
    the *panel* updater: it syncs a git repo, dumps a postgres container, and
    replaces the API, the frontend and the nginx config, none of which exist on
    an agent-only box. It aborted at `No such container: dockpanel-postgres`
    **one second after the panel had already recorded the server as
    succeeded**, and the agent stayed on its old version.
  - The false success came from status being read off the wrong process.
    `update.sh` re-execs itself into a PID1-owned transient unit with
    `exec systemd-run` (no `--wait`), so the child the agent waits on exits 0
    the moment systemd *accepts* the job — measured at 124 ms. The agent
    promoted that into `Succeeded`, and the orchestrator took the agent's word
    for it.

  What changed: a remote agent now updates *itself* — one binary, fetched for
  the requested release tag, **verified against that release's `checksums.txt`
  before it is installed**, swapped in by rename (never a copy onto a running
  executable), then restarted, with the result written to
  `/var/lib/dockpanel/last-agent-update.json` on every exit path. If the new
  binary does not come up reporting the target version, the previous one is
  restored. Full panel installs keep using `update.sh` as before. The procedure
  is compiled into the agent, so it does not depend on any file being present
  on the remote box.

  And the orchestrator now decides from ground truth: it waits until the agent's
  `/health` reports the target version, which is what the W4 design specified in
  the first place. A self-reported success is no longer accepted as evidence;
  a self-reported *failure* still is, because that one an agent can state
  honestly.

- **A failed fleet update left the remote box permanently un-updatable.** An
  updater that fails without restarting the agent never cleared the in-flight
  flag, so every later attempt — including the one correcting the operator's
  typo — was refused `409 an update is already in flight` until someone
  restarted the agent by hand. The guard now asks whether the run actually
  finished rather than trusting a flag nothing clears.

- **A fleet failure took ten minutes to surface a reason that was known in one
  second.** With the update stopped before the restart, the agent's in-memory
  state stayed `in_flight`, so the panel waited out its whole deadline and
  reported a generic timeout. Failures now resolve in ~10 s with the real
  cause (e.g. `could not download …/v2.99.0/dockpanel-agent-linux-amd64`).

- **The rolling update rolled the fleet in the wrong order.** `agent_version`
  is a text column, so ordering by it in SQL sorted `2.9.0` *after* `2.10.0` —
  the "oldest first" plan started with the newest box. Ordering is now done on
  parsed version components.

- **The "also update this panel" checkbox did nothing.** `include_panel` was
  written to the run record and read by no one since v2.10.0. It now starts the
  panel's own update after the fleet finishes, and only if every member
  succeeded — updating the panel on top of a half-rolled fleet is the ordering
  the design explicitly rules out.

- **A panel installed without a domain handed the operator an add-server
  command that could not run.** `BASE_URL` is empty on IP-only installs, which
  produced `curl -sSL /install-agent.sh … --panel-url  --token <token>`; the
  installer's argument parser then consumed `--token` as the value of
  `--panel-url` and died on the token itself. Worse, an agent installed without
  a panel URL never checks in, and a server that never checks in can never be
  selected by a fleet update. The panel now emits a clearly-marked placeholder
  instead of an empty flag, and `install-agent.sh` refuses to install without
  `--panel-url` and `--server-id` rather than producing a box that is silently
  invisible to the fleet.

- `install-agent.sh` no longer discards the result of starting the agent. It
  waits for the unit to become active and prints the failing journal lines
  instead of a success banner, the same failure surface added to the PowerDNS
  installer in 2.11.2.

### Notes

- Upgrading an existing fleet: the fix lives in the agent, so a member still on
  2.11.7 or older cannot be rolled from the panel — it will report that it is
  too old, and naming the remedy. Re-run `install-agent.sh` on those boxes once
  to reach 2.11.8; fleet updates work from then on.

## [2.11.7] - 2026-07-19

### Fixed

- **A rollback merged the snapshot into the database instead of replacing it,
  and that made rolling back across a migration either impossible or fatal.**
  `pg_dump --clean` emits `DROP` statements only for the objects the dump itself
  contains, so anything a newer version's migration had created outlived a
  rollback to an older snapshot — while `_sqlx_migrations`, which *is* in the
  dump, was rewound past it. The database was left describing neither version.
  Two distinct failures follow from that, both reproduced end to end on a lab
  box before this was changed:
  - For a migration that adds a **standalone** table, the rollback succeeded and
    the *next* forward update to that version re-ran the migration against
    objects that already existed: `relation "..." already exists`, the api
    panicked at startup, exited 101 and crash-looped under `Restart=always`
    until `StartLimitBurst` — a permanent 502 out of a rollback that had
    reported success.
  - For a migration that adds a table with a **foreign key** to an existing one
    — 7 of the 15 newest migrations reference `users`/`servers`/`sites` — the
    rollback could not even run: the surviving FK depends on `users_pkey`, none
    of the dump's `DROP TABLE` statements carry `CASCADE`, and psql aborted with
    `cannot drop constraint users_pkey ... because other objects depend on it`.
    Atomic, so nothing was lost — but the snapshot could never be restored.

  The database stage now drops and recreates the `public` schema in the **same
  transaction** as the dump, making a rollback a true point-in-time revert. The
  schema's owner and ACL are restored explicitly (`pg_database_owner`, plus
  `PUBLIC`'s `USAGE`), because the dump carries neither and a bare recreate would
  have silently handed the schema to the restoring role on every rollback. The
  all-or-nothing guarantee is unchanged: the teardown shares the dump's
  transaction, so a dump that fails to apply rolls it back and the database is
  byte-identical afterwards. The pre-rollback dump is still taken first.

  No released version pair could reach this: the newest migration
  (`20260520000000_panel_self_update.sql`) shipped in v2.10.0, so every version
  from v2.10.0 to v2.11.6 has an identical migration set. The defect became
  reachable the moment the next migration shipped.

- **`pg_dump | gzip` reported gzip's exit status in two more places.** v2.11.5
  fixed this for panel snapshots and missed its siblings. The auto-healer's
  24-hourly database backup ran under `sh -c` — dash, which has no `pipefail` —
  so a `pg_dump` that died halfway was stored and logged as "DB auto-backup
  completed"; and the `db-backup.sh` written by `setup.sh` checked no status at
  all. Both now run with `pipefail` and verify the dump's completion marker
  before the file is kept. In both, retention pruning now happens only *after*
  the new backup is known good, so a corrupt run can no longer evict a good one.
  (`scripts/update.sh`'s pre-upgrade backup was already covered by that script's
  file-level `set -o pipefail`.)

- **Backup verification could pass on a partially applied restore.**
  `backup_verify.rs` restored into its scratch database with neither
  `ON_ERROR_STOP=1` nor `--single-transaction`, so a dump whose statements failed
  still produced tables for the table-count check to find and report "verified".
  `backup_drill.rs` had `ON_ERROR_STOP=1` but not `--single-transaction`, so a
  drill could still describe a partial restore. Both now restore atomically.
  These dumps are written with `--no-owner --no-acl` and no `--clean`, so there
  are no `DROP` statements to fail spuriously against a fresh scratch database.

- **The dump-completeness check had no margin left.** Both copies looked for
  `PostgreSQL database dump complete` in the last 5 lines, but PostgreSQL's
  August-2025 minor releases append a trailing `\unrestrict` line: on the lab the
  marker landed at line 6243 of 6247, inside the window by exactly zero lines.
  One more trailer from any future `pg_dump` and every snapshot would have been
  rejected as truncated, disabling rollback entirely. The window is now 20 lines.
  It stays a tail window rather than a whole-file search on purpose — this panel
  stores operator-authored text, so the marker string can legitimately appear in
  the data.

### Changed

- **Behaviour change, stated plainly: a rollback now DELETES database objects
  created after the snapshot, and the data in them.** Previously they survived,
  which is what made the panel unbootable afterwards. The pre-rollback dump is
  the way back, and it is taken before anything is touched.
- Pre-rollback dumps (`/var/lib/dockpanel/pre-rollback-<id>.sql.gz`) are now
  pruned to the three most recent. One is written on every rollback and nothing
  in the product had ever deleted them — the retention sweep only walks
  `panel_snapshots` rows and their tarballs — so they grew without bound, which
  matters more now that they are the only undo for the deletion above.
- Settings → Telemetry now states what a rollback actually removes before the
  operator confirms it, and `docs/api-reference.md` describes the replace
  semantics rather than the old merge semantics.

## [2.11.6] - 2026-07-19

### Added

- **The rollback verdict is now visible in the panel**, not only on the API.
  2.11.5 made a restore report its outcome truthfully to
  `GET /api/update/status`; this surfaces it in Settings → Telemetry above the
  snapshot list, in green when the last rollback completed and in red when it
  failed — naming the stage it stopped at, and stating that a failure before the
  database stage completes leaves the database exactly as it was. A rollback
  stops and restarts the panel, so the operator has no other way to learn what
  happened; shipping the field without the surface would have left them to read
  a JSON endpoint by hand.

## [2.11.5] - 2026-07-19

Snapshot restore works. It never had — every pre-update snapshot the panel has
taken since v2.10.0 was unrestorable, and the way it failed was worse than not
working at all: on a lab box it reduced a 92-table database to 1 table and
reported the restore as a success. This release was driven by running the path,
not by reading it.

### Fixed

- **`POST /api/update/rollback` destroyed the database and reported success.**
  The restore ran inline inside the HTTP request handler, so it competed with the
  panel's own 300-second request timeout (and nginx's `proxy_read_timeout`); a
  restore measured at 394 seconds on a lab box, so the request future was dropped
  while psql was still consuming the dump. Dropping it broke the
  `gunzip | psql` pipe, psql read that as a normal end of input and **exited 0**,
  and the caller's `status.success()` check recorded a successful restore.
  Because `pg_dump --clean` emits all 92 `DROP TABLE` statements before the first
  `CREATE TABLE`, an interruption anywhere in that window leaves a database that
  has been fully dropped and only partly rebuilt — measured at 1 surviving table
  out of 92, with `servers`, `sites`, `metrics_history`, `backup_schedules` and
  `backup_policies` among the casualties, which is exactly the damage reported
  during the previous cycle's investigation.

  Both halves are now closed. The restore runs as a PID1-owned transient systemd
  unit (`scripts/restore-snapshot.sh`), so nothing can cancel it and it safely
  outlives the api process it stops — the endpoint returns `202` immediately
  instead of holding a request open across a service restart. The database is
  applied with `ON_ERROR_STOP=1 --single-transaction`, so it either lands
  completely or changes nothing. Verified both ways on a lab box against a
  deliberately truncated stream: the old form exited 0 having left 1 of 92
  tables; the new form exits non-zero with all 92 intact. As a side effect the
  restore is also roughly forty times faster (394s to ~10s), because one
  transaction commits once instead of fsyncing per statement.

- **Snapshots could be created from an incomplete database dump.** The dump was
  taken with `sh -c "pg_dump … | gzip > file"`, whose exit status is *gzip's* —
  and gzip compresses a truncated stream and exits 0. A `pg_dump` that died
  partway therefore produced a short dump that was stored as a valid snapshot
  with a perfectly correct sha256 over perfectly incomplete contents. The dump
  now runs under `bash` with `set -o pipefail`, and the snapshot is rejected
  unless the dump carries pg_dump's completion marker. The restore re-checks the
  same marker before it takes the panel down, so an incomplete dump can never
  reach the destructive stage.

- **A rollback was not recorded.** `rolled_back_at` was stamped before the
  restore ran, and the restore replaces `panel_snapshots` with the snapshot's own
  copy of itself — so the stamp was overwritten and lost every time (observed
  coming back empty on a lab box). The restore now records it afterwards, in the
  database it just restored.

### Added

- Every restore writes a verdict to `/var/lib/dockpanel/last-restore.json` on
  every exit path, including an abort, and it is surfaced as `last_restore` on
  `GET /api/update/status`. A restore stops and restarts the panel, so its
  outcome cannot be returned through the request that began it; without this a
  failed rollback and a rollback that never ran look identical.
- A rollback now captures the pre-rollback database to
  `/var/lib/dockpanel/pre-rollback-<id>.sql.gz` before applying the snapshot, so
  a successful-but-regretted rollback is recoverable.

### Known issues

- **RESOLVED in 2.11.7.** A rollback restores what the snapshot *contains*.
  Because `pg_dump --clean` can only drop objects it knows about, database
  objects created *after* a snapshot survive a rollback to it while
  `_sqlx_migrations` is rewound — so a later forward update to that same newer
  version can meet a migration whose objects already exist. Nothing outside the
  snapshot (nginx vhosts, Let's Encrypt certificates, site data, docker volumes)
  is rewound at all: a rollback restores the panel, not the machine.

## [2.11.4] - 2026-07-19

The panel's rollback safety net did not work. v2.11.3 made self-update complete
for the first time; this release is what happened when the *failure* path was
finally exercised on a clean box, by deliberately failing an update's health
check rather than by reading the code.

### Fixed

- **A failed update was never rolled back, and said it was.** `update.sh`'s
  `rollback()` restored the previous binaries with `cp`. At that point the new
  `dockpanel-api` and `dockpanel-agent` are already running, and copying onto a
  running executable fails `ETXTBSY` ("Text file busy") — each restore was
  suffixed `2>/dev/null || true`, so the failure was discarded and the script
  printed "Rolled back to previous binaries" while the box kept running the
  binary that had just failed its health check. Only the `dockpanel` CLI (not
  running) was actually restored, so the box then disagreed with itself:
  `dockpanel --version` reported the old version while `/api/health` reported the
  new one. Rollback now stops the services first and restores with `mv`, the same
  primitive the forward swap already used, and reports per-binary success or
  failure instead of discarding it. Verified on a lab box: both binaries return
  byte-for-byte (sha256-matched) to the pre-update release.

- **A rolled-back update was reported as a successful one.** The snapshot row is
  finalized by whichever binary boots after the swap — about 30 seconds *before*
  the health check decides whether that build is any good. On a rollback the new
  api starts, records "succeeded", then fails its check and is replaced by the
  old binary; nothing revisits the row. `/api/update/status` now cross-checks the
  recorded target against the version actually running and reports `rolled_back`
  when they disagree, so the panel can no longer claim an upgrade it is
  demonstrably not running.

- **Rolling back across a migration bricked the panel.** Migrations are applied
  by the new version before the health check; the restored older binary then met
  an applied migration it had no file for, and sqlx's strict validation failed
  startup with `VersionMissing`. Because the call site panics, the api exited 101
  and crash-looped under `Restart=always` until it hit the start limit — a
  permanent 502 with no operator-facing explanation. Migrations are additive, so
  an older binary against a newer schema is safe; startup now tolerates unknown
  applied migrations (missing ones are still applied).

- **Fleet updates handed the wrong version format to remote agents.** The local
  self-update path was fixed in v2.11.3 to re-add the `v` prefix that release
  URLs require; the fleet path was missed and passed the operator's input through
  verbatim, so a bare `2.11.4` became a 404 on every remote node. Fleet targets
  are by definition servers on older builds, whose on-disk `update.sh` predates
  the tolerance added in v2.11.3, so nothing downstream rescued it.

### Security

- **Panel snapshots were world-readable.** `/var/backups/dockpanel/snapshots` was
  `0755` and each tarball `0644`, and every snapshot bundles `/etc/dockpanel` —
  `api.env` (the JWT signing secret and the Postgres password), `agent.token`,
  and the agent's TLS private key. Any local user could read them and mint an
  admin token. The directory is now `0700` and tarballs `0600`, applied to
  existing snapshots as well as new ones.

### Known issues

- `POST /api/update/rollback` (restore from a panel snapshot) still returns a
  500 and has never worked. The failure is safe — it aborts before changing
  anything. Investigation this cycle found that clearing the first fault exposes
  a worse one behind it: the restore then proceeds to leave the database with
  missing tables. It is deliberately left failing until the database-restore
  stage is fixed and verified; see the comment in `panel_snapshot.rs`.
  *(Resolved in 2.11.5 — the database stage was the fault, and it was fixed and
  verified end to end before the binary stage was unblocked.)*

## [2.11.3] - 2026-07-19

Panel self-update actually works now. Running the v2.11.1 → v2.11.2 upgrade
through the panel's own flow on a clean box — the fresh-VPS gate that had been
deferred since the feature shipped — showed it failing at the first download.

### Fixed

- **Panel self-update never completed.** The update poller stores the advertised
  version with the `v` stripped (`2.11.2`), and `/api/update/apply` validates the
  operator's target against that stripped form — so `2.11.2` was the only
  accepted input. It was then handed to `update.sh` verbatim as
  `DOCKPANEL_VERSION`, which documents `vX.Y.Z` and concatenates it straight into
  the release download URL. The result was
  `releases/download/2.11.2/dockpanel-agent-linux-amd64`, which 404s, so every
  self-update died with curl exit 22 before swapping a single binary. The `v` is
  now re-added at that boundary. The failure was at least safe — nothing was
  replaced and the panel stayed on its previous version.
- **A failed update left the panel reporting "in progress" forever.** The
  orchestrator only logged the exit status; nothing transitioned the state, so
  `/api/update/status` sat on `in_flight` until the 15-minute window lapsed, with
  the real error visible only in the journal. A non-zero exit now logs at error
  level and finalizes the snapshot, which surfaces through the existing
  rolled-back state as "attempted `<target>`, still on `<current>`".

## [2.11.2] - 2026-07-19

Fresh-VPS validation release. Every fix below came out of running the panel
on two clean boxes — Ubuntu 24.04 and Debian 12 — rather than reading code:
the PowerDNS installer shipped in 2.11.0 could not actually bring the service
up on Ubuntu, and its PostgreSQL backend had never worked on any install.

### Fixed

- **PowerDNS never started on Ubuntu (and any distro running
  systemd-resolved).** The generated `pdns.conf` set no `local-address`, so
  pdns took its default wildcard `0.0.0.0:53` bind — which collides with the
  systemd-resolved stub listeners on `127.0.0.53` and `127.0.0.54`. pdns died
  with `Unable to bind UDP socket to '0.0.0.0:53': Address already in use` and
  systemd restart-looped it indefinitely. The installer now detects a foreign
  listener on port 53 and pins pdns to the machine's real addresses plus
  loopback, leaving the stub resolver — and the box's own name resolution —
  untouched. Debian 12 ships no stub listener, keeps the wildcard bind, and is
  why this never showed up in CI.
- **Reinstalling PowerDNS failed with `Read-only file system`.** Uninstall runs
  `apt-get purge`, which deletes `/etc/powerdns`. systemd creates that directory
  and its `ReadWritePaths` bind mount when the agent starts, so deleting it
  detaches the mount for the rest of the agent's life and every later install
  failed writing `pdns.conf`. The config is now written through the same
  unsandboxed escape hatch already used for the SQLite schema, staged via a
  live `ReadWritePaths` entry.
- **The PowerDNS PostgreSQL backend could never connect.** The installer looked
  for `PANEL_DB_PASSWORD`/`DATABASE_URL` in the agent's environment — which the
  agent unit does not set — and then silently *generated a random password*,
  guaranteeing `password authentication failed for user "dockpanel"`. It now
  reads the real credential from `/etc/dockpanel/api.env`, and reports an error
  instead of inventing one that cannot work.
- **The PowerDNS installer reported success when pdns never started.** Both
  `systemctl` results were discarded, so a crash-looping service still returned
  `ok: true` and a green "PowerDNS installed" step. It now waits for the service
  to settle and returns the failing journal line. This silence is why the three
  bugs above shipped unnoticed.
- **Taking a manual panel snapshot blocked self-update for 15 minutes.** The
  in-flight probe matched any snapshot with no `to_version`, which includes
  every manual snapshot — so `GET /api/update/status` reported a phantom
  `in_flight` and `start_panel_update` refused with "already in flight". Taking
  a safety snapshot before updating is exactly when an operator hits this. Both
  queries now exclude manual snapshots.
- **Admin endpoints reported CSRF and token errors as "Authentication
  required".** The `AdminUser`/`ResellerUser` extractors flattened the inner
  rejection into a generic 401, hiding the 403 "Missing CSRF header" and
  "Invalid or expired token" cases behind a misleading message.
- **The `pdns.conf` API key was world-readable** (mode 644). It is now installed
  `640 root:pdns`.

### Changed

- The install smoke-test — the ABI/loader regression guard for #70 — is now
  invoked from the release workflow. It declared `on: release: [published]`,
  but releases are published with the default `GITHUB_TOKEN`, which by design
  does not start further workflow runs; the guard had therefore never run on
  a release since it was written.
- `npm audit` and `cargo audit` now run as a blocking pre-push gate across all
  six manifests, and the git hooks are checked into `scripts/hooks/` instead of
  living only in one machine's `.git/hooks`. Every audit step in CI is suffixed
  `|| true`, so this is the project's first enforcing dependency check.
- The manual PowerDNS setup guide in Settings → Services documents the
  systemd-resolved port-53 conflict and the `local-address` line that resolves
  it.

## [2.11.1] - 2026-07-19

Dependency-security release. No feature changes and no application source
changes — the diff is lockfiles, dependency floors, and one Dockerfile
line. Clears all 33 Dependabot advisories on the default branch (10 high,
13 moderate, 10 low) plus three RustSec advisories that Dependabot never
surfaced.

### Security

- **Cleared all 33 Dependabot advisories.** Every one resolved within the
  already-declared semver range, so no application code had to change:
  `react-router`/`react-router-dom` 7.13.1 → 7.18.1 (14 advisories,
  8 of them high — CVE-2026-33245, CVE-2026-42211, CVE-2026-42342,
  CVE-2026-34077, CVE-2026-33244, CVE-2026-40181, CVE-2026-53663),
  `dompurify` 3.4.2 → 3.4.12 (8), `vite` 6.4.2 → 6.4.3 (4, incl.
  CVE-2026-53571 high), `@babel/core` 7.29.0 → 7.29.7 (2), `rand`
  0.8.5 → 0.8.6 (2, RUSTSEC-2026-0097), `qs` 6.15.0 → 6.15.3 (1,
  CVE-2026-8723), `esbuild` 0.27.4 → 0.28.1 (1), `serde_with`
  3.18.0 → 3.21.0 (1, GHSA-7gcf-g7xr-8hxj). `npm audit` now reports
  zero vulnerabilities across all three package manifests.
- **Fixed three RustSec advisories that Dependabot did not report.**
  `cargo audit` catches Rust advisories that Dependabot's Cargo.lock
  scanning missed entirely, which is why it is worth running both:
  `lettre` 0.11.21 → 0.11.22 (RUSTSEC-2026-0141, CVSS 9.1 — TLS
  hostname verification disabled on the Boring backend), `quinn-proto`
  0.11.14 → 0.11.15 (RUSTSEC-2026-0185, CVSS 7.5 — remote memory
  exhaustion via unbounded out-of-order stream reassembly), and
  `crossbeam-epoch` 0.9.18 → 0.9.20 (RUSTSEC-2026-0204 — invalid
  pointer dereference in the `fmt::Pointer` impl). `anyhow`
  1.0.102 → 1.0.104 also clears RUSTSEC-2026-0190 (unsoundness in
  `Error::downcast_mut()`), which had been sitting as an accepted
  warning. `cargo audit` now reports zero vulnerabilities for all three
  crates; the only remaining entries are two informational warnings with
  no upstream fix (`rustls-pemfile` unmaintained, `spin` yanked).
  Of these, only `crossbeam-epoch` is actually compiled into a shipped
  binary — see the notes below.
- **Dependency floors now live in the manifests, not only the
  lockfiles.** `panel/frontend/package.json` still declared
  `react-router-dom: ^7.5.0`, `dompurify: ^3.2.0` and `vite: ^6.4.2`,
  and `website/client/package.json` declared `react-router-dom:
  ^7.13.1` — all of which legally permit the *vulnerable* versions.
  Only `package-lock.json` was holding the patched resolution, so any
  lockfile-less install could resolve straight back below the fix. The
  declared ranges are now `^7.18.1`, `^3.4.12` and `^6.4.3`.
- **`panel/frontend/Dockerfile` now uses `npm ci` instead of
  `npm install`**, matching `website/client/Dockerfile` and
  `website/server/Dockerfile`, which already did. With `npm install` a
  Docker build was free to re-resolve past the audited versions and
  silently rewrite the lockfile, so the image was not provably the
  build that was reviewed.

### Notes

Four of the patched advisories were **never exploitable in DockPanel's
configuration**. They are fixed anyway — depending on a vulnerable
version is worth avoiding on its own — but the honest framing is that no
DockPanel install was at risk from them:

- **RUSTSEC-2026-0141 (`lettre`, CVSS 9.1)** — the inverted
  hostname-verification flag lives entirely inside the crate's
  `boring-tls` feature arms. DockPanel builds `lettre` with
  `default-features = false` and `tokio1-rustls-tls`; `boring` is absent
  from the dependency graph and the rustls arm is byte-identical between
  0.11.21 and 0.11.22. The vulnerable line was never compiled.
- **RUSTSEC-2026-0185 (`quinn-proto`)** — a lockfile-only entry. `http3`
  is not enabled on either binary, so `quinn-proto` never enters the
  normal build graph.
- **GHSA-7gcf-g7xr-8hxj (`serde_with`)** — the panicking `KeyValueMap`
  serializer is never instantiated anywhere in the tree.
- **RUSTSEC-2026-0190 (`anyhow`)** — an orphaned lockfile entry with no
  reverse-dependency edge in either crate (`cargo tree -i anyhow` prints
  nothing even with `--target all -e all`). Bumping it was a verified
  no-op: `cargo build --release` finished in 0.31s, i.e. cargo had
  nothing to recompile, which is itself proof the crate is not linked
  into either binary.
- **The four DOMPurify advisory classes** (hook pollution, `setConfig`
  `ALLOWED_ATTR` pollution, `SAFE_FOR_TEMPLATES` bypass, `IN_PLACE`
  closure leak) each require API surface with zero call sites. The panel
  has exactly one DOMPurify entry point — a bare
  `DOMPurify.sanitize(string)` with no config, in the markdown runbook
  renderer.

Two upstream behavior changes do ship and are worth knowing about:

- **`lettre`** now caps SMTP replies at 1000 bytes per line and 100 KB
  total. RFC 5321 limits reply lines to 512 bytes, so a conformant relay
  cannot trip this — but a non-conformant relay that previously worked
  may now fail with `SMTP response line too long`.
- **DOMPurify** now always strips `patchsrc`, and strips `for` on any
  element other than `<label>`/`<output>`. This only affects raw HTML
  hand-written into a custom runbook; none of the 15 shipped runbooks
  are affected.

## [2.11.0] - 2026-07-19

Three community-requested enhancements
([#67](https://github.com/ovexro/dockpanel/issues/67),
[#63](https://github.com/ovexro/dockpanel/issues/63),
[#50](https://github.com/ovexro/dockpanel/issues/50)/[#58](https://github.com/ovexro/dockpanel/issues/58)).

### Added
- **The dashboard status widgets are now clickable**
  ([#67](https://github.com/ovexro/dockpanel/issues/67)). The overview
  status cells (Health, Alerts, SSL, Incidents, Backups, Sites,
  Databases, Docker), the "Degraded Performance" status banner, and each
  Smart Recommendation row now link straight to the page that resolves
  them — a "critical diagnostic issue" recommendation opens System
  Diagnostics, an open incident opens the Status Page, an SSL warning
  opens Certificates, and the Health score opens the diagnostics that
  drive it. Previously these were dead-ends with no indication of what
  they referred to or how to act on them. The Monitoring and Security
  pages now accept a `?tab=` query parameter so these links land on the
  correct sub-tab.
- **PowerDNS SQLite backend option**
  ([#63](https://github.com/ovexro/dockpanel/issues/63)). The one-click
  PowerDNS installer now offers a choice of **SQLite** (no database
  server required) or PostgreSQL. SQLite removes the dependency on the
  panel's containerized PostgreSQL — the coupling that could make the
  PostgreSQL install silently fail — and has a minimal footprint that
  suits most deployments. The installer's manual setup guide was also
  corrected: it previously instructed `sudo -u postgres createdb pdns`
  on localhost, which cannot work because DockPanel's PostgreSQL runs in
  a Docker container, not on the host.
- **AzuraCast app template**
  ([#50](https://github.com/ovexro/dockpanel/issues/50) /
  [#58](https://github.com/ovexro/dockpanel/issues/58)). One-click deploy
  for the AzuraCast self-hosted web-radio suite (Media category).

## [2.10.2] - 2026-06-07

Fixes two fresh-install blockers reported in
[#70](https://github.com/ovexro/dockpanel/issues/70) and
[#71](https://github.com/ovexro/dockpanel/issues/71).

### Fixed
- **Install failed on Debian 12 with `GLIBC_2.38 / GLIBC_2.39 not
  found`** ([#70](https://github.com/ovexro/dockpanel/issues/70)).
  Release binaries were built on `ubuntu-latest` (now Ubuntu 24.04,
  glibc 2.39), so the dynamically-linked agent/API/CLI demanded
  glibc ≥ 2.38 and the agent refused to start on Debian 12 (glibc 2.36).
  The same break silently affected the *rest* of the documented support
  matrix — Ubuntu 20.04, Debian 11, CentOS 9, Rocky 9, Amazon Linux 2023
  all ship glibc ≤ 2.34. The release workflow now builds **fully static
  musl binaries** (`x86_64-unknown-linux-musl` /
  `aarch64-unknown-linux-musl`) via `cargo-zigbuild`, so the binaries
  carry zero glibc dependency and run on any modern Linux regardless of
  distro libc version (`ldd` reports "statically linked"). DockPanel's
  TLS stack is entirely rustls, so there is no OpenSSL system dependency
  to block static linking.
- **First login bounced straight back to the login screen on a domain
  install served over HTTP**
  ([#71](https://github.com/ovexro/dockpanel/issues/71)). When a domain
  is supplied at setup, `setup.sh` writes `BASE_URL=https://<domain>`,
  but the panel vhost is served over plain HTTP until TLS is added. The
  cookie helper keyed the `Secure` flag off `BASE_URL`, so it stamped
  `Secure` on the session cookie even though the response left over HTTP
  — the browser silently dropped the cookie and the next
  `/api/auth/me` 401'd, bouncing the user back to the login screen in
  every browser. This was the case the [#47](https://github.com/ovexro/dockpanel/issues/47)
  fix left open (it only removed the empty-`BASE_URL` path).
  `routes/auth.rs::cookie_secure_flag` and the OAuth callback now derive
  `Secure` solely from the actual request scheme (`X-Forwarded-Proto`,
  which nginx always sets and is authoritative because the API only
  listens on `127.0.0.1` behind the proxy). HTTPS installs still get a
  `Secure` cookie; HTTP-served installs no longer bounce.

## [2.10.1] - 2026-05-20

Hotfix for the v2.8.22 webmail reverse-proxy regression reported in
[#57](https://github.com/ovexro/dockpanel/issues/57).

### Fixed
- **Webmail "Open" landed on the panel dashboard instead of Roundcube
  login.** Roundcube emits root-anchored URLs in its HTML (form
  `action="/?_task=login"`) and inline JS (`comm_path: "/?_task=login"`)
  — it has no concept that it lives under `/webmail/` on the panel
  vhost. The v2.8.22 nginx fragment proxied with `proxy_redirect off`
  and no body rewriting, so the browser navigated to `/?_task=login` →
  hit the panel's `location /` block → rendered the React SPA
  (dashboard). The CPU spike in the report was Roundcube's container
  booting on first hit. Fix in `panel/agent/src/routes/mail.rs:926`:
  added `proxy_redirect / /webmail/;` to rewrite 30x `Location:`
  headers and `sub_filter '"/?_task=' '"/webmail/?_task=';` to rewrite
  embedded URLs in HTML/JSON/JS bodies. Also clears `Accept-Encoding`
  to upstream so `sub_filter` receives uncompressed responses.
- **Auto-heal for existing webmail installs.** v2.8.22 → v2.10.0 boxes
  already have the broken fragment on disk; the agent only writes on
  Install click, so users would have to Remove + Install to recover.
  `scripts/update.sh:428` detects the old shape (no `sub_filter` line)
  and regenerates the fragment from the current template, using the
  current Roundcube container's host port from `docker inspect`.

## [2.10.0] - 2026-05-16

Phase 4 W4 ships **panel self-update from the UI** with health-check
rollback, **persistent snapshots**, **update channels** (stable /
candidate / hold), and **fleet rolling updates**.

The reframing matters: `scripts/update.sh:430-499` already ships a
production-tested binary-swap + .bak-restore rollback flow. v2.10.0 does
NOT reimplement that — the new orchestrator (`services/panel_update.rs`)
shells out to the same script under a controlled environment
(`DOCKPANEL_NO_SELF_REFRESH=1` + `DOCKPANEL_VERSION=<target>`) so every
bug fix already in update.sh (self-refresh ordering from v2.8.15-16,
lock-wait conf from v2.8.17, fragment-include awk migration from v2.8.22,
ACME cooldown from v2.8.23) keeps working unchanged. The new code is a
presentation + persistence layer over a proven core.

### Added
- **Apply Update button in Telemetry → Updates tab.** Click → confirm
  modal showing target version + 4-step preview ("snapshot, download,
  swap, probe") → status modal that polls `/api/update/status` every 2s.
  Replaces the static SSH copy-paste block.
- **Pre-update snapshot service** (`services/panel_snapshot.rs`). Each
  Apply call first writes a tar.gz triplet to
  `/var/backups/dockpanel/snapshots/` containing
  `binaries/{agent,api,cli}` + `db/dump.sql.gz`
  (`pg_dump --clean --if-exists`) + `etc/dockpanel/` + `metadata.json`.
  Written to `.tmp` then atomically renamed; DB row only inserted after
  rename succeeds. Refuses to create when the snapshot partition has
  less than 2 GiB free.
- **Operator-triggered rollback from the UI.** Each snapshot row has a
  Roll back button (confirms then restores binaries + DB + /etc and
  bounces services). Reach back to any retained snapshot, not just the
  `.bak` that `update.sh` keeps for ~30 seconds.
- **Update channels:** stable (GA only — current default behaviour),
  candidate (includes `prerelease: true` builds, takes the first by
  `published_at` desc), hold (skips the 6h auto-poll entirely; Manual
  Check button still works). Channel selector in Updates tab, single
  `settings.update_channel` row.
- **Fleet rolling update.** Operator-initiated form in Updates tab:
  target version + halt-on-failure + include-panel toggles. Plan = all
  user-owned remote servers reachable in the last 5 minutes, sorted
  oldest agent_version first. POSTs to each agent's new `/panel/update`
  endpoint, polls `/panel/update/status` for terminal state, records
  per-server progress in `fleet_update_runs.progress` JSONB. Halts on
  first failure unless `halt_on_failure: false`.
- **Agent-side `/panel/update` receiver**
  (`panel/agent/src/routes/panel_update.rs`). Distinct from the existing
  OS-package `/system/updates/*` endpoints. Bearer-auth, returns 202,
  spawns `update.sh` detached so the agent's own systemctl restart
  doesn't break the subprocess pipeline.
- **10 new admin endpoints** under `/api/update/*` + `/api/snapshots/*`:
  GET `/status`, POST `/apply`, POST `/manual-check`, POST `/rollback`,
  GET+PUT `/channel`, GET+POST `/api/snapshots`, DELETE
  `/api/snapshots/{id}`, GET+POST `/update/fleet`, GET
  `/update/fleet/{id}`. All admin-gated.
- **Snapshot retention sweep** wired into the existing 24h
  `run_retention_cleanup` ticker in `auto_healer.rs`. Always-keep last 3
  snapshots regardless of age; delete anything older than 7 days beyond
  that floor. File-delete first; DB row stays for retry if the file
  delete fails.
- **Startup finalize hook** (`finalize_pending_on_startup` in
  `services/panel_update.rs`). Closes out any `panel_snapshots` rows
  with `to_version IS NULL` after a process restart by writing
  `to_version = CARGO_PKG_VERSION`. Equal `from_version`/`to_version`
  on a finalized row indicates `update.sh`'s in-flight rollback fired;
  differing values indicate a successful apply.

### Changed
- **Update poller honours `update_channel`**
  (`services/telemetry_collector.rs:302`). `hold` skips the poll;
  `candidate` widens to `/releases?per_page=20` (first by `published_at`
  desc); `stable` keeps the existing `/releases/latest` URL bit-for-bit.
- **`scripts/update.sh` accepts two new env vars.**
  `DOCKPANEL_NO_SELF_REFRESH=1` bypasses the v2.7.13 self-refresh block
  so the orchestrator can stream a single subprocess invocation's stdout
  into its state machine without a mid-flight re-exec breaking the pipe
  (SSH-operator flow keeps self-refresh on by default).
  `DOCKPANEL_VERSION=vX.Y.Z` pins the release tag instead of fetching
  `/releases/latest`, so a candidate-channel pick can't race a GA
  publish between the panel's poll and the operator's click.

### Migration
- One settings row inserted (`update_channel = 'stable'` — the implicit
  pre-W4 default). Two new empty tables (`panel_snapshots`,
  `fleet_update_runs`). No ALTER on existing tables; every install keeps
  current behaviour until an admin clicks Apply or changes the channel.
  Migration file: `20260520000000_panel_self_update.sql`.

### Operator notes
- Snapshots consume disk: ~150-300 MB each typical, retained 7 days
  (last 3 always kept). Stored under `/var/backups/dockpanel/snapshots/`.
  A free-disk pre-check refuses to create a snapshot if the partition
  has less than 2 GiB free.
- `update.sh`'s SSH-only flow continues working unchanged — no
  operator forced to use the UI.
- Cosign signature verification at download time is **not** in W4.
  HTTPS-to-GitHub is the existing trust boundary; cosign verify is a
  separate hardening pass (non-trivial key management).
- The api process will be killed mid-binary-swap by `update.sh`'s
  `systemctl stop dockpanel-api`. The orchestrator state lives in the
  DB rows (`panel_snapshots.to_version`); the new process boots Idle
  and the finalize hook closes out the in-flight row.

## [2.9.0] - 2026-05-16

Phase 4 W3 ships **on-call rotations** and **escalation policies**. A small
team can now self-host their on-call schedule in DockPanel — when an alert
fires, the panel pages whoever is on-call right now (not every channel on
the rule); if the alert isn't acknowledged within a policy-defined
threshold, the panel routes the page to the next step in the chain.

Larger teams that already pay for PagerDuty keep using PagerDuty — the
escalation policy supports a `webhook:<url>` route shape that forwards
directly into their existing PD service key.

### Added

- **`on_call_schedules` table + admin tab.** A rotation = ordered list of
  user IDs plus a cadence in days (1–90). "Who's on-call at time T" is
  one-liner cadence math against an anchor; no calendar widget, no
  per-day overrides, no holiday handling. New endpoints
  `GET/POST/PUT/DELETE /api/on-call/schedules[/{id}]` (admin) and
  `GET /api/on-call/whoami` (any authenticated user) for "am I on the
  hook right now?"
- **`escalation_policies` table + admin tab.** Policies are an ordered
  JSONB array of `{after_minutes, route}` steps. Routes are
  discriminated: `on_call_schedule:<uuid>` resolves to the current
  rotation holder, `user:<uuid>` pages a specific user, `all_channels`
  preserves the pre-W3 default (alert owner's channels),
  `webhook:<url>` is a direct outbound webhook bypass. New endpoints
  `GET/POST/PUT/DELETE /api/escalation-policies[/{id}]` (admin).
- **Per-alert-rule policy attachment.** `alert_rules` gains a nullable
  `escalation_policy_id` FK. NULL = pre-W3 hardcoded 15-min unack →
  30-min re-page (unchanged for every existing rule). Admin-only attach
  endpoint at `PUT /api/alert-rules/{rule_id}/escalation-policy`.
- **Ack actor + optional comment.** `PUT /api/alerts/{id}/acknowledge`
  now accepts an optional `{ "comment": "..." }` body (500-char cap)
  and stores both `acknowledged_by` (the actor) and
  `acknowledged_comment`. Older clients that PUT with no body keep
  working — they just don't carry a comment. The UI surfaces actor
  email + truncated comment inline on each acked alert row.
- **Frontend tabs.** Alerts page grows two new tabs alongside Alerts +
  Runbooks: an On-call editor (rotation CRUD with reorderable member
  list) and an Escalation policies editor (step chain with route picker
  + live route description).

### Changed

- **Escalation pages now carry the runbook payload.** Phase 4 W2 added
  the runbook excerpt + URL to fire payloads via
  `send_notification_with_runbook`, but `check_escalations` was still
  calling bare `send_notification` — so re-pages on unacknowledged
  alerts lost the runbook context that the original fire had carried.
  The W3 rewrite of `check_escalations` extracts the shared
  `load_runbook_payload` helper so fire and escalation paths produce
  identical payloads.

### Migration

No manual action is required. `escalation_policy_id` is added to
`alert_rules` as a nullable FK with default NULL — every existing rule
keeps its pre-W3 behaviour bit-for-bit. The three new alerts columns
(`acknowledged_by`, `acknowledged_comment`, `escalation_step_index`)
default to NULL/0 on existing rows.

## [2.8.23] - 2026-05-16

### Changed

- **SSL renewal cadence is now profile-aware.** The auto-healer previously
  used a hardcoded 6h cooldown for both ARI re-fetch (RFC 9773) and the
  post-attempt retry. For the new `shortlived` profile (6-day certs whose
  renewal window is only ~4 days wide), 6h was 6% of the cert lifetime — a
  CA-issued early-renew nudge could be missed by a full quarter-day, and a
  failed attempt near expiry could burn the whole window. The cooldown is
  now 1h for `shortlived` and stays at 6h for `tlsserver` (45d) and
  `classic` (90d → 64d → 45d across the LE roadmap). Lets-Encrypt's
  tlsserver profile transitioned to 45-day issuance on 2026-05-13, which
  is what unblocked this change.

### Added

- **New Prometheus counter: `dockpanel_cert_renewals_total{result}`** with
  `result="success"` and `result="failure"` labels. Tracks auto-healer
  renewal attempts so operators can graph trend and alert on
  `rate(...{result="failure"}[1h])`. The counter is process-local (resets
  across restart — Prometheus `increase()` handles that gracefully) and
  adds zero DB queries per scrape.

  Exposed at `/metrics` alongside the existing `dockpanel_info`,
  `dockpanel_site_count`, and `dockpanel_alerts_firing` gauges.

## [2.8.22] - 2026-05-16

### Fixed

- **Webmail "Open" button on the Mail page was unreachable** ([#57] third
  finding by @WiskeyPapa). The Roundcube container is bound to
  `127.0.0.1:8888` on the host (loopback only — never exposed to the
  public IP for security), but the frontend Open button generated a
  `http://<panel-hostname>:8888` URL. That URL has nothing listening on
  it (and Cloudflare doesn't proxy port 8888 anyway), so the button
  produced a hang / connection refused.

  Fixed by reverse-proxying Roundcube under `/webmail/` on the existing
  panel nginx vhost via a drop-in fragment file. The frontend Open URL
  is now `${origin}/webmail/` — same-origin, inherits the panel's TLS,
  works on both HTTPS-with-domain and HTTP-on-IP installs. The
  Roundcube container also gets new env vars
  (`ROUNDCUBEMAIL_PROXY_WHITELIST=127.0.0.1`, plus
  `ROUNDCUBEMAIL_TRUSTED_HOSTS` and `ROUNDCUBEMAIL_FORWARDED_PROTO=https`
  when the panel has a configured `server_name`) so Roundcube accepts
  the forwarded headers and generates correct URLs behind the proxy.

### Changed

- `webmail_install` is now idempotent — clicking Install when an existing
  `dockpanel-roundcube` container is present tears it down before
  recreating, so env-var additions across releases (like the v2.8.22
  proxy/trusted-hosts envs) apply automatically on next Install click.
  Users who deployed Roundcube on v2.8.20/v2.8.21 just need to click
  Install again, which now rebuilds in place. The `webmail_remove`
  endpoint also tears down the panel-vhost reverse-proxy fragment for
  clean uninstall.

- Panel nginx vhost gains an `include
  /etc/nginx/conf.d/dockpanel-panel.locations/*.conf;` directive (baked
  into `scripts/setup.sh`'s vhost template; injected into existing
  vhosts by `scripts/update.sh` via an awk-based one-time migration —
  same shape as the v2.8.3 IPv6-listen migration). Drop-in directory
  for path-mounted tool reverse-proxies; webmail is the first user, but
  other tools (phpMyAdmin, Adminer) can use the same mechanism in the
  future.

### Internal

- New helper `panel_server_name()` in
  `panel/agent/src/routes/mail.rs` reads the panel vhost's
  `server_name` directive to drive Roundcube's `TRUSTED_HOSTS`
  computation — same approach `update.sh` uses to detect the panel
  domain for `BASE_URL` auto-population.
- New helpers `write_webmail_nginx(port)` /
  `remove_webmail_nginx()` write the `/webmail/` location fragment,
  validate via `nginx -t`, and reload on success. Failed validation
  unlinks the fragment so nginx is never left in a broken state.

## [2.8.21] - 2026-05-16

### Fixed

- **Firewall add/remove rule returned "Agent offline" with `ufw: ERROR:
  '/etc/ufw/user.rules' is not writable`** ([#57] follow-up by @WiskeyPapa).
  The agent runs under `ProtectSystem=strict` with an explicit
  `ReadWritePaths=` allowlist, and `/etc/ufw` was never in that list. `ufw
  status` (read-only) worked, but writes to `user.rules` during add/delete
  were blocked by the sandbox mount. Added `/etc/ufw` and `/var/lib/ufw` to
  the canonical agent unit's `ReadWritePaths`, plus matching pre-create
  entries in `scripts/setup.sh` and `scripts/update.sh` so the namespace
  mount succeeds even on systems where ufw isn't installed yet. Same shape
  of fix as the v2.8.13 expansion that added `/etc/modsecurity` /
  `/etc/cloudflared` / `/etc/postfix` to the RWP list.

- **Dashboard "Set up backups" onboarding step stayed incomplete after a
  manual backup ran, and the card linked to Sites instead of the Backups
  page** ([#57] follow-up by @WiskeyPapa). The completion check was
  `sitesList.some(s => !!s.backup_schedule)`, but `/api/sites` doesn't
  return a `backup_schedule` field — so the check was always false
  regardless of how many backups had been created. Added a new
  `GET /api/backup-setup-status` endpoint (auth-gated, scoped by user)
  returning `{ has_schedule, has_backup }` derived from real DB counts
  across `backup_schedules`, `backups`, `database_backups`, and
  `volume_backups`. Dashboard now fetches the status once and the card
  flips to complete as soon as any of those exist. Link retargeted from
  `/sites/<id>` to `/backup-orchestrator` (the global backup view).

## [2.8.20] - 2026-05-15

### Fixed

- **WAF install button stayed on "Install" after a successful install on
  Ubuntu 24.04** ([#57] follow-up by @WiskeyPapa). Ubuntu Noble's
  `time_t`-64 ABI transition renamed `libmodsecurity3` →
  `libmodsecurity3t64` as a virtual-provides (no transitional shim). The
  agent's `install_status` route was checking `dpkg -l libmodsecurity3`
  literally, which never matches "ii" on Noble even though the install
  succeeded — frontend therefore kept showing "Install". Detect path in
  `routes/service_installer.rs::install_status` now accepts either name
  (OR-clause, matching the existing PHP fallback pattern). Same fix
  applied to `uninstall_waf`'s apt purge list so uninstall on Noble
  actually removes the package instead of silently no-op'ing.

## [2.8.19] - 2026-05-10

### Fixed

- **Mail server install failed under the agent's strict sandbox** ([#57]
  follow-up by @WiskeyPapa). `routes/mail.rs::install_mail` was running
  `apt-get install` via the sandboxed `safe_command(...)` wrapper, so the
  agent unit's `ProtectSystem=strict` made `/var/lib/dpkg/lock-frontend`
  read-only inside the namespace. apt printed `Not using locking for read
  only lock file /var/lib/dpkg/lock-frontend` warnings and then bailed when
  it tried to `chown` files. Switched the four apt-get call sites in
  `mail.rs` (install / purge / autoremove / rspamd install) to
  `safe_command_unsandboxed`, matching the #54-A pattern v2.8.14 applied to
  vmail `useradd`/`groupadd` (which lived right next to the apt-get call
  that this commit corrects). `routes/system.rs::disk_cleanup`'s
  `apt-get clean` got the same treatment so `/var/cache/apt` actually gets
  cleared.
- **Cloudflare Tunnel install wrote a literal `$(lsb_release -cs)` into
  `/etc/apt/sources.list.d/cloudflared.list`** ([#57] follow-up by
  @WiskeyPapa). The shell pipeline used **single quotes** around the echo
  argument, which prevents bash command substitution. Once the broken
  source landed, every subsequent `apt-get update` on the box failed
  (`The repository '... $(lsb_release Release' does not have a Release
  file`), blocking unrelated installs (Redis, WAF, Mail Server). Pre-resolve
  `VERSION_CODENAME` from `/etc/os-release` in Rust and `printf` the source
  line with the actual codename — also drops the `lsb-release` package
  dependency on minimal Debian images. Defensive: on install failure,
  delete a half-written source file so it doesn't break the rest of apt.
- **`update.sh` now repairs an existing broken cloudflared apt source on
  upgrade.** Operators who already hit the bug get auto-cleanup on
  `INSTALL_FROM_RELEASE=1 bash update.sh` — no manual `rm` needed. Looks
  for the literal `$(lsb_release` string in
  `/etc/apt/sources.list.d/cloudflared.list` and removes the file if
  found.

## [2.8.18] - 2026-05-06

### Added

- **Phase 4 W2: Alert runbooks attached to fired alerts.** Markdown text per
  alert type, indexed by `alert_type`. Excerpts (280 char, truncated at
  sentence boundary) ride along in slack/discord/pagerduty/webhook payloads;
  full markdown is rendered into email HTML and into the new Alerts page row
  expansion. Operator-edited runbooks survive panel upgrades by construction
  (`apply-defaults` uses `ON CONFLICT DO NOTHING` and never overwrites edits).
  Resolution is DB-row-then-default — fresh installs produce useful payloads
  from the compile-time const slice without the operator having to seed first.
- **15 default runbooks** shipped with the panel (`panel/backend/runbooks/`):
  5 critical (offline / service_down / container_crashloop / backup_failure /
  gpu_temperature), 9 warning (cpu / memory / disk / disk_forecast / ssl_expiry
  / container_unhealthy / gpu_utilization / gpu_vram / memory_leak), 1 info
  (container_down). Each follows the same shape: Why this fired → First check
  → Common causes → Escalation. Authored for paging-grade discipline (info
  alerts won't wake anyone, critical ones page clearly).
- **`Alerts → Runbooks` tab** with per-type list, edit modal (split textarea +
  live markdown preview, severity selector, Restore-default button), and
  "Seed missing default runbooks" action with insert-or-skip confirmation.
- **Inline runbook expansion on each fired alert** — click any row in the
  Alerts list, the runbook for that alert type is fetched and rendered
  below the alert detail. Targets the W2 acceptance bar: an admin paged at
  3am sees the runbook in the page, not as a "go look at our wiki" link.
- **5 admin-only API endpoints** under `/api/alerts/runbooks`:
  `GET` (list with `is_default` flag), `GET {alert_type}` (single),
  `PUT {alert_type}` (upsert, 50KB cap, severity validated), `DELETE`
  (restore default by removing DB row), `POST apply-defaults`
  (insert-or-skip from const slice, returns `{ inserted, skipped }`).

### Changed

- `services/notifications.rs::try_fire_alert` now resolves a runbook by
  `alert_type` and threads `runbook_excerpt` + `runbook_url` through a new
  `send_notification_with_runbook` helper (the existing `send_notification`
  is unchanged, so the 14 non-alert callers across auto_healer, uptime,
  security_hardening, git_deploys, and incidents stay on the original API).
  Email gets full pulldown-cmark-rendered HTML appended to the body, slack
  and discord get a link plus excerpt, pagerduty extends `custom_details`,
  generic webhook adds `runbook_url` + `runbook_excerpt` as top-level keys.
- New backend dep: `pulldown-cmark = "0.10"` (no_std-capable, ~50KB binary
  impact, fuzz-tested upstream; rendered output wrapped in `catch_unwind`
  defensively with HTML-escape fallback).
- New frontend deps: `marked@^14` + `dompurify@^3` (~51KB gzipped combined).
  DOMPurify is non-negotiable defense-in-depth — runbook markdown is
  admin-authored but stored in DB and editable via API.
- Email template variables now include `{{runbook_excerpt}}` and
  `{{runbook_url}}` alongside the existing `{{title}}`/`{{message}}`/
  `{{severity}}`/`{{timestamp}}`. Backwards-compatible: existing custom
  templates ignore unknown placeholders.
- Migration `20260507000000_alert_runbooks.sql` adds the table:
  `alert_runbooks(alert_type TEXT PK, runbook_md TEXT, severity_default
  TEXT CHECK (info|warning|critical), updated_by UUID FK users(id) ON
  DELETE SET NULL, updated_at TIMESTAMPTZ)`.

## [2.8.17] - 2026-05-06

### Fixed

- **Agent installers failed with `Could not get lock /var/lib/dpkg/lock-frontend`
  when another apt was running** ([#57 follow-up](https://github.com/ovexro/dockpanel/issues/57)).
  On fresh Debian 13 boots, `unattended-upgrades` runs in the
  background and holds the dpkg frontend lock for several minutes.
  The panel UI's `Install PHP 8.4` (and any other agent-driven apt
  install/purge — services, updates) failed immediately on contention
  instead of waiting. Both `setup.sh` (fresh installs) and `update.sh`
  (existing operators) now drop
  `/etc/apt/apt.conf.d/99-dockpanel-lock-wait.conf` setting
  `DPkg::Lock::Timeout "300";` — every apt invocation on the system
  (agent and otherwise) now waits up to 5 minutes for the dpkg lock
  before giving up. No agent code change needed; the config file is
  read fresh on every apt run. Verified end-to-end on Debian 13
  Trixie: `python3 fcntl.lockf` holding the dpkg lock for 15 s →
  `apt-get install` waits 15 s and succeeds (vs. 0 s fail-fast pre-fix).
- **Settings → Services → `Install Redis` (and Node.js, Composer, WAF,
  Cloudflare Tunnel) returned 404** ([#57 follow-up](https://github.com/ovexro/dockpanel/issues/57)).
  Latent backend gap since these services were added: the agent has
  full install/uninstall implementations in
  `panel/agent/src/routes/service_installer.rs`, but the backend's
  `routes/mod.rs` only proxied install for php/certbot/ufw/fail2ban/
  powerdns. Frontend POST to `/api/services/install/redis` (and the
  other four) hit a non-existent route and returned 404 before
  reaching the agent. Added the 5 missing install handlers + the 2
  missing uninstall handlers (waf, cloudflared) in
  `panel/backend/src/routes/system.rs` and registered all 7 routes in
  `routes/mod.rs`. Each handler is a 5-line proxy mirroring the
  existing pattern.

## [2.8.16] - 2026-05-06

### Fixed

- **PHP install failed on Debian 13 (trixie)** ([#57](https://github.com/ovexro/dockpanel/issues/57)).
  `setup.sh` hardcoded `PHP_VER=8.3` and reached for
  `add-apt-repository -y ppa:ondrej/php` whenever `apt-cache show
  php8.3` returned nothing — but trixie ships PHP 8.4 in its default
  repo, and `ppa:ondrej/php` is an Ubuntu PPA that has no packages
  built for trixie. Fresh Debian 13 installs hit "PHP 8.3 installation
  failed" and ended up with no PHP at all. New flow: try the
  default-repo `php-fpm` metapackage first (covers Debian 13/12 and
  Ubuntu 24.04 cleanly with whatever PHP version each distro ships),
  fall back to `deb.sury.org` for older Debian or `ppa:ondrej/php`
  for Ubuntu when the default repo can't satisfy the install. Same
  Debian-vs-Ubuntu split applied to the panel-driven PHP installer
  in `panel/agent/src/routes/php.rs` so Settings → Services →
  Install PHP works on Debian too.
- **`update.sh` self-refresh never fired on the default code path.**
  Mode auto-detection (`INSTALL_FROM_RELEASE=1` when no Rust toolchain
  / no source) ran *after* the self-refresh check, so a user running
  plain `bash /opt/dockpanel/scripts/update.sh` entered with
  `INSTALL_FROM_RELEASE=0`, failed the self-refresh gate, and then
  got bumped to `1` by auto-detect — but with the stale local script
  still executing. Effect: pre-v2.8.16 panels swapped binaries to the
  latest release just fine, but never picked up script-side fixes
  (unit-file deploys, nginx config tweaks, install-agent.sh drop into
  FE_DIST). That's why issue [#56](https://github.com/ovexro/dockpanel/issues/56)
  resurfaced after the v2.8.14 fix shipped — operators on v2.8.13
  ran update.sh and stayed on v2.8.13's update.sh logic. Fix: move
  mode detection ahead of the self-refresh block so
  `INSTALL_FROM_RELEASE` is correct by the time the gate evaluates.
  Operators on v2.8.13/v2.8.14/v2.8.15 should run
  `INSTALL_FROM_RELEASE=1 bash /opt/dockpanel/scripts/update.sh` once
  to trigger self-refresh; from v2.8.16 onward, plain `bash update.sh`
  works.
- **PHP 8.4 not detected as installed.**
  `panel/agent/src/routes/service_installer.rs` enumerated
  `php8.{1,2,3}-fpm` to determine if PHP was installed/running, so a
  Debian 13 install (which lands PHP 8.4 from the default repo) was
  reported as "PHP not installed" in Settings → Services even when
  it was running fine. Added `php8.4-fpm` to both checks.

## [2.8.15] - 2026-05-06

### Fixed

- **`update.sh` skipped the repo sync in `INSTALL_FROM_RELEASE=1` mode,
  so v2.8.14's canonical-unit changes never deployed on the standard
  upgrade path.** Found by the v2.8.13 → v2.8.14 VPS upgrade test:
  binaries upgraded to v2.8.14 successfully, but the systemd unit file
  on disk was still v2.8.13's content (no `RuntimeDirectory=dockpanel`,
  no `/var/cache/nginx` in `ReadWritePaths=`). Root cause: line 106
  gated `git pull` behind `INSTALL_FROM_RELEASE != 1`, but the code at
  line 215 deploys the canonical unit from `$AGENT_SRC` regardless of
  mode. Same family as the v2.8.13 "dev fiction" bug — canonical file
  in repo, installer reads stale on-disk copy.
  - `git pull --ff-only` also didn't cover installs cloned with
    `-b v2.8.13` (or any explicit tag — they end up on a detached HEAD
    with no local `main`). Replaced the conditional with an
    unconditional `git fetch --depth=1 origin main` + `git reset --hard
    FETCH_HEAD` so the canonical unit, nginx templates, and
    install-agent.sh are always at the latest origin/main when
    update.sh runs. Operators who already upgraded to v2.8.14 via
    `bash update.sh` should re-run it on v2.8.15 to pick up the unit
    changes; the self-refresh logic added in v2.7.13 will fetch the
    fixed update.sh from this release.

## [2.8.14] - 2026-05-06

### Fixed

- **WordPress provisioning failures on every fresh install** ([#54](https://github.com/ovexro/dockpanel/issues/54)).
  Three independent regressions surfaced when the v2.8.12 strict
  sandbox shipped, each only firing on specific paths so they slipped
  through the v2.8.13 verification:
  - `Failed to download wp-cli` — `services/wordpress.rs::ensure_cli`
    ran `safe_command("curl") -o /usr/local/bin/wp`, but
    `/usr/local/bin` is not in the agent's `ReadWritePaths` under
    `ProtectSystem=strict` so the write was blocked silently and
    bubbled up as a 422 on the WP install endpoint. Switched to
    `safe_command_unsandboxed("curl", &[])` (the same `systemd-run`
    escape used for apt/dpkg in v2.8.12) and now surface the curl
    stderr in the error message instead of just the static "Failed
    to download wp-cli" string.
  - `mkdir() "/var/cache/nginx/fastcgi/<site>" failed (ENOENT)` —
    `routes/nginx.rs::put_site` called `create_dir_all` on the
    per-site FastCGI cache path before rendering the vhost, but
    `/var/cache/nginx` was not in the agent's `ReadWritePaths` so
    the create silently failed (only a `tracing::warn!`). The
    config was written anyway; nginx -t then fired its own mkdir of
    the cache leaf, found no parent, and rejected the reload. Added
    `/var/cache/nginx` to `ReadWritePaths=` in the canonical unit,
    pre-created `/var/cache/nginx/fastcgi` in `setup.sh` and
    `update.sh`, and promoted the agent-side `create_dir_all`
    failure from a `warn!` to a 500 with an actionable message
    ("Ensure /var/cache/nginx is in the agent's ReadWritePaths") so
    we never again render a config we know nginx can't validate.
  - `tar: unrecognized option '--no-dereference'` — three call sites
    (`services/backups.rs`, `services/wordpress.rs::create_update_snapshot`,
    `routes/mail.rs::mailbox_backup`) passed `--no-dereference` to
    `tar -c`. GNU tar 1.35 (current Trixie/Noble default and the
    version on this server) does not accept that option in create
    mode, so every site backup, every WP update snapshot, and every
    mail backup since the flag was introduced has been failing
    silently — including on the panel's own demo. GNU tar's
    create-mode default is already "do not follow symlinks", so the
    fix is to drop the flag from all three sites.

- **`curl … {panel_url}/install-agent.sh | bash` returned the SPA
  HTML** ([#56](https://github.com/ovexro/dockpanel/issues/56)). The
  multi-server install command surfaced in `routes/servers.rs`
  pointed users at `{panel_url}/install-agent.sh`, but the panel's
  nginx config has `try_files $uri $uri/ /index.html;` with no
  override for that path — so the URI fell through to the SPA's
  `index.html` and `bash` choked on `<!DOCTYPE html>`. The script
  also wasn't deployed under any served path. Fixed by having
  `setup.sh` and `update.sh` copy `scripts/install-agent.sh` into
  `$FE_ROOT/install-agent.sh` so the existing `try_files $uri` rule
  serves it directly with the right MIME.

- **HTTP-on-IP installs were stuck in a login bounce**
  ([#47](https://github.com/ovexro/dockpanel/issues/47)). The cookie
  helper in `routes/auth.rs::issue_session` set `Secure` whenever
  `BASE_URL` was empty (the assumption being that production
  deployments use HTTPS and an empty default should not regress
  them). For users running on the bare `http://<ip>:<port>` URL
  before adding a domain, the browser silently dropped the `Secure`
  cookie on the plain-HTTP response and `/api/auth/me` then 401'd
  on the very next request — login appeared to succeed and
  immediately bounced back to the login screen. Replaced the
  BASE_URL-only check with a combined `BASE_URL=https://… ||
  X-Forwarded-Proto: https` check (nginx already sets
  `X-Forwarded-Proto $scheme`), and threaded the request `HeaderMap`
  through `issue_session_pub` / `logout` / OAuth `callback` /
  passkey `auth_complete` so every login path uses the same scheme
  detection.

- **`/run/dockpanel` disappeared mid-upgrade and pinned the agent at
  StartLimitBurst** (v2.8.13 followup, surfaced during the demo
  upgrade-path test). `update.sh` mkdir's `/var/run/dockpanel`
  before the `systemctl stop / start` cycle, but between stop and
  start the directory disappeared on Ubuntu — the agent's namespace
  mount (which now resolves `/run/dockpanel` as a `ReadWritePaths=`
  symlink target) failed five times in 60s and the unit refused to
  start until manual `systemd-tmpfiles --create` plus
  `systemctl reset-failed`. Added `RuntimeDirectory=dockpanel` and
  `RuntimeDirectoryPreserve=yes` to the canonical unit so systemd
  creates and persists the directory itself, which fires before the
  namespace setup and survives every restart.

- **Agent socket occasionally left at 0600 root:root, breaking the
  panel's "Failed to load system update status" toast.** The
  systemd unit's `ExecStartPost` was the only thing that chown'd
  the socket to `www-data` and chmod'd it to 0660 — and it failed
  silently in some restart sequences, leaving the panel unable to
  reach the agent over its UNIX socket. The agent now sets the
  permissions inline right after `UnixListener::bind` (via libc
  `getgrnam` / `chown` / `set_permissions`), so the unit's
  `ExecStartPost` is belt-and-suspenders rather than load-bearing.

- **Mail provisioning's `groupadd`/`useradd` for the vmail user
  failed under strict sandbox.** Same family as #54-A: the
  `safe_command` wrapper runs sandboxed, but the user-management
  binaries write `/etc/passwd` / `/etc/shadow` / `/etc/group`,
  which are too sensitive to put in `ReadWritePaths=`. Switched
  both calls to `safe_command_unsandboxed("groupadd", &[])` /
  `safe_command_unsandboxed("useradd", &[])`.

## [2.8.13] - 2026-05-02

### Changed

- **`dockpanel-agent.service` is now deployed from a single source of
  truth** ([#48](https://github.com/ovexro/dockpanel/issues/48)
  followup). The in-repo unit file at
  `panel/agent/dockpanel-agent.service` was historically a hardened
  reference (`ProtectSystem=strict` + a curated `ReadWritePaths=` list)
  that no installer ever deployed — `scripts/setup.sh` and
  `scripts/update.sh` both wrote a permissive
  `ProtectSystem=no`/`ProtectHome=no`/`PrivateTmp=no` unit inline via
  heredoc, so every install.sh-based install ran with no namespace
  hardening at all. v2.8.13 deletes both heredocs and has the install
  scripts `cp` the canonical unit file from the repo. Existing installs
  upgrading via `update.sh` get the strict sandbox automatically on the
  next update; the daemon-reload + agent restart that update.sh already
  performs at the end of its run picks up the new unit. The remote-agent
  installer (`scripts/install-agent.sh`) is intentionally left on its
  own inline heredoc — it deploys a different unit (after
  `docker.service`, no nginx dep, env-file driven) for the multi-host
  remote-agent path.

### Security

- **Hardened the deployed agent sandbox to `ProtectSystem=strict` plus
  the full `Protect*` / `Restrict*` set** ([#48](https://github.com/ovexro/dockpanel/issues/48)
  followup). The new `ReadWritePaths=` covers everything the agent
  actually writes via `std::fs::write` / `tokio::fs::write` /
  `create_dir_all`: the original eight (`/etc/nginx /etc/dockpanel
  /var/run/dockpanel /var/backups/dockpanel /var/lib/dockpanel /var/www
  /var/log /etc/letsencrypt`) plus ten new paths grepped from current
  agent code (`/etc/apt /etc/fail2ban /etc/systemd/system /etc/powerdns
  /etc/modsecurity /etc/cloudflared /etc/postfix /etc/dovecot
  /var/spool/postfix /opt`). v2.8.12's `safe_command_unsandboxed`
  systemd-run wrapper continues to handle the apt/dpkg/snap subprocess
  paths that can't be expressed via `ReadWritePaths=`. Net effect: the
  agent now runs with meaningful kernel-namespace isolation —
  `ProtectKernel{Logs,Modules,Tunables}=yes`,
  `ProtectControlGroups=yes`, `ProtectClock=yes`,
  `ProtectHostname=yes`, `RestrictRealtime=yes`,
  `RestrictSUIDSGID=yes`, `LockPersonality=yes`,
  `RestrictNamespaces=~user`, `NoNewPrivileges=yes`, `ProtectHome=yes`,
  `PrivateTmp=yes`. None of this hardening was ever active on
  install.sh-installed users; demo had a hand-deployed strict version
  which is what surfaced the v2.8.12 EROFS bug.

### Known limitations

- The mail subsystem (`panel/agent/src/routes/mail.rs:173-174`) still
  spawns `useradd`/`groupadd` via the sandboxed `safe_command`, which
  fails under `ProtectSystem=strict` because `/etc/passwd`,
  `/etc/shadow`, and `/etc/group` are too sensitive to add to
  `ReadWritePaths=`. This was already broken under demo's strict
  sandbox; mail provisioning has been silently failing on that path.
  v2.8.14 will wrap the user/group creation calls with the
  v2.8.12 `safe_command_unsandboxed` pattern (systemd-run escape) for
  a clean fix.

## [2.8.12] - 2026-05-01

### Fixed

- **Service Installers + System → Updates fail silently with `Read-only
  file system` errors under the agent's `ProtectSystem=strict`
  sandbox** ([#48](https://github.com/ovexro/dockpanel/issues/48)
  followup). `dockpanel-agent.service` runs with `ProtectSystem=strict`
  and a `ReadWritePaths=` list that omits `/var/cache/apt`,
  `/var/lib/apt`, `/var/lib/dpkg`, and `/usr` — the paths apt and dpkg
  must write to. Every install / upgrade path that spawned `apt-get`,
  `snap install`, `dpkg`, or `curl | bash` from the agent inherited
  the sandbox and EROFS'd the moment it tried to download a `.deb` or
  install a binary into `/usr/bin`. Surfaced when `insxa` clicked
  `Install` on Redis / Composer / Node.js / Cloudflare Tunnel / WAF
  in Settings → Services — every one failed. System → Updates'
  "Update All" button hit the same wall.
  - Added `safe_command_unsandboxed()` (and a sync sibling) to
    `panel/agent/src/safe_cmd.rs`. The helper invokes the binary via
    `systemd-run --quiet --pipe --wait --collect --setenv=... -- <bin>`,
    which routes through PID1 to spawn a transient unit in PID1's
    own mount namespace. The inner binary sees the full filesystem
    read-write while the agent itself stays sandboxed for everything
    else. Every `--setenv` flag explicitly re-establishes the
    sanitized env (`PATH`/`HOME`/`LANG`/`LC_ALL`/`DEBIAN_FRONTEND`)
    so the inner binary doesn't inherit PID1's wider environment.
  - Converted ~25 call sites that legitimately need `/usr` write
    access to use the new helper:
    `panel/agent/src/routes/updates.rs` (`apt-get update` and
    `apt-get install/upgrade`), `service_installer.rs` (every
    `install_*` and `uninstall_*` shell script + the
    `rm /usr/local/bin/composer` in `uninstall_composer`),
    `php.rs` (`add-apt-repository ppa:ondrej/php`, `apt-get install`
    for PHP base + extensions, `apt-get purge`/`autoremove`),
    `server_utils.rs` (`enable_auto_updates`'s `apt-get install
    unattended-upgrades`), and `services/smtp.rs` (`ensure_msmtp`'s
    `apt-get install msmtp`).
  - Read-only callers (`apt list --upgradable`, `apt-cache show`,
    `dpkg -l`, `which <bin>`) keep using the sandboxed `safe_command`
    — `ProtectSystem=strict` permits reads of `/var/lib/apt/lists`
    and `/var/lib/dpkg`, so wrapping them with `systemd-run` would
    just add overhead.
  - Empirically verified: from inside the agent's mount namespace,
    `touch /var/cache/apt/archives/_test` returns `EROFS`; the same
    `touch` wrapped in `systemd-run --quiet --pipe --wait --collect --`
    succeeds because the transient unit gets a fresh mount namespace.
    Smoke-tested on demo: `GET /system/updates` returned 69
    upgradable packages cleanly (was returning empty pre-fix because
    `apt-get update` EROFS'd before populating the lists).

  WAF + Cloudflare Tunnel installers will partially succeed in
  v2.8.12 (apt step now works) but still hit `EROFS` on follow-up
  `std::fs::write` / `create_dir_all` calls into `/etc/modsecurity`
  and `/etc/cloudflared`. v2.8.13 will close those by either adding
  the directories to the unit's `ReadWritePaths` or by routing
  those writes through the same helper.

## [2.8.11] - 2026-05-01

### Fixed

- **Settings → Services tab missing from the tab bar — PowerDNS / Image
  Scan / SBOM / Prometheus config UIs unreachable from the panel for
  over a month** ([#48](https://github.com/ovexro/dockpanel/issues/48)
  followup). Commit `fd44a31` (2026-03-24, "UX: fix overlaps, decompose
  Settings, create System page") removed the `{ id: "services", label:
  "Services" }` entry from `Settings.tsx`'s tab list intending to move
  the contents to the new System page, but the actual content block
  (`{tab === "services" && (<>...</>)}` at lines 2169-2245) was
  orphaned in place and never relocated. The DNS page's "configure
  PowerDNS API in Settings" hint pointed users at a tab that didn't
  exist. Surfaced when an `insxa` followup on issue #48 asked for a
  screenshot of where to find the Services tab — there wasn't one.
  Fix: restored the Services tab button so the existing content block
  is reachable. (A proper move-to-System-page refactor remains on the
  list but is a bigger UX restructure than tonight's scope.)

## [2.8.10] - 2026-05-01

### Fixed

- **Dashboard "Restart nginx" / "Restart PHP-FPM" buttons did nothing
  on click** ([#48](https://github.com/ovexro/dockpanel/issues/48)
  followup). The frontend was POSTing the wrong request shape to the
  agent — `{ fix: "restart_nginx" }` and `{ fix: "restart_php" }`,
  while the agent's `/diagnostics/fix` endpoint deserializes
  `{ fix_id: "restart-service:<name>" }`. Even after deserializing,
  the value `restart_nginx` doesn't match any of the supported
  `apply_fix` actions. Two changes:
  - Frontend (`panel/frontend/src/pages/Dashboard.tsx`) now sends
    `{ fix_id: "restart-service:nginx" }` and
    `{ fix_id: "restart-service:php-fpm" }`.
  - Agent (`panel/agent/src/services/diagnostics.rs`) treats
    `php-fpm` (no version) as a smart alias: it enumerates loaded
    `php<ver>-fpm.service` (Ubuntu/Debian) or plain `php-fpm.service`
    units via `systemctl list-units` and restarts every match, so
    multi-version installs (PHP 8.1 + 8.2 + 8.3) all reload their
    OPcache after the click. Returns a clear error if no PHP-FPM
    unit is installed at all.

- **Disk-full forecast fired during install on otherwise-idle
  systems.** `services/alert_engine.rs` extrapolated linearly from
  the most recent 60 metrics_history rows. On a fresh install the
  first 30-60 minutes show 5-10%/hour disk growth (binary writes,
  frontend tarball, postgres init, container layers); the
  extrapolation predicted "disk full in 9 hours" even at 30%
  usage. Surfaced in the same `insxa` followup on issue #48 — alerts
  fired non-stop on a 40 GB box minutes after install. Forecast now
  requires (a) at least 6 hours of trend data so the install spike
  bleeds out, AND (b) current disk usage already over 60% so we're
  on a runway to a real full disk, not extrapolating from noise on
  an empty box. Existing thresholds (forecast horizon < 48h, severity
  cutoff at 12h) preserved.

## [2.8.9] - 2026-05-01

### Fixed

- **Agent's `restart-service` validator rejected systemd unit names
  containing a dot.** `php8.3-fpm`, `containerd.service`, etc. are
  legitimate unit names but the regex was `[a-z0-9_-]+` only. Surfaced
  in the same `insxa` followup on issue #48 — every PHP-FPM auto-heal
  attempt was returning "Invalid service name" silently. Also affected
  the post-restore PHP-FPM reload in `routes/backups.rs:244`. Fix:
  allow `.` in service names. Dots in systemd unit names cannot be
  used for path traversal because `systemctl restart <name>` doesn't
  treat the argument as a path.

- **Seven Settings toggles silently rejected by the backend whitelist**
  ([#48](https://github.com/ovexro/dockpanel/issues/48) followup).
  `PUT /api/settings` validates incoming keys against a hard-coded
  allow-list. Several security/registration toggles in the Settings UI
  wrote keys that were absent from that list — `self_registration_enabled`,
  `security_approval_required`, `security_geo_alert_enabled`,
  `security_session_recording`, `security_db_backup_enabled`,
  `security_canary_enabled`, `security_lockdown_threshold`. Toggling
  them returned `400 Unknown setting: <key>`, the toast surfaced as
  "Failed", and the value never persisted. Backend code paths
  (`routes/auth.rs`, `services/security_hardening.rs`) already *read*
  these keys, so the runtime behaviour was tied to whatever value was
  set out-of-band. Surfaced when an `insxa` followup on issue #48
  reported "these two in settings are not opted: Self-Registration,
  Require Approval for New Users" — same root cause as v2.8.5's
  ipv6only-strip migration miss: a list that grew implicit coupling
  to other parts of the codebase that nobody updated when new toggles
  were added. Frontend-only would have masked the issue with try/catch;
  the right fix is at the writer-side gate. No agent / cli / frontend
  code changes; binaries recompiled to carry the v2.8.9 version
  string.

## [2.8.8] - 2026-05-01

### Fixed

- **Password reset link bounced to `/login` instead of rendering the
  reset form** ([#48](https://github.com/ovexro/exro/dockpanel/issues/48)
  followup). When an unauthenticated user clicked the reset link from
  their email, `ServerProvider` (mounted at the top of the SPA tree)
  fired `api.get("/servers")` on mount → 401 because no session →
  `api.ts`'s 401 handler redirected to `/login` because its no-redirect
  allow-list only covered `/login` and `/setup`. Net effect: user lands
  on the login form, never sees the reset password fields, and the
  one-time token expires unused. Same hole hit `/forgot-password`,
  `/register`, and `/verify-email` for any unauth visitor — though those
  were less obviously broken because users typically reach them already
  knowing they need to log in. Fix extends the allow-list in
  `panel/frontend/src/api.ts` to all six top-level public routes
  (`/login`, `/setup`, `/register`, `/forgot-password`,
  `/reset-password`, `/verify-email`). Surfaced when an `insxa`
  followup on issue #48 reported the bounce; the page rendering was
  fine on the demo when probed, which made it look like an
  email-client mangling issue at first — empirically confirmed by
  insxa pasting the URL bar after click as `https://your-panel/login`,
  proving a synchronous redirect from the SPA was firing. No backend
  changes; binaries recompiled to carry the v2.8.8 version string.

## [2.8.7] - 2026-05-01

### Added

- **Branding logo upload** ([#48](https://github.com/ovexro/exro/dockpanel/issues/48)
  follow-up). Settings → Branding now exposes an "Upload image…" button
  next to the existing Logo URL field. The frontend POSTs the file's
  raw bytes to a new `POST /api/branding/logo` endpoint (admin-only,
  PNG / JPEG / WebP, 2 MB cap, content-type *and* magic-bytes
  validated to defend against MIME spoofing). Files are stored
  content-addressed at `/var/lib/dockpanel/branding/logo-<hash>.<ext>`
  and served back over `GET /api/branding/logo/{filename}` (public —
  the login page is unauthenticated and needs to render the logo) with
  `Cache-Control: public, max-age=31536000, immutable`. The upload
  handler auto-saves `logo_url` so the new image takes effect on the
  next page render. Surfaced when an `insxa` follow-up on issue #48
  reported "branding image could not be saved" — the existing settings
  field only accepted a URL, with no file upload UI. Admins on
  air-gapped panels with no public CDN can now self-host their logo.

## [2.8.6] - 2026-05-01

### Fixed

- **`update.sh` defaulted to compile-from-source on production VPS
  installs that don't have Rust** — surfaced when an `insxa` follow-up
  on [#48](https://github.com/ovexro/dockpanel/issues/48) hit
  `Rust toolchain not found` then OOM'd on `proc-macro2` after they
  installed rustup. The script already auto-switches to release
  binaries when the source tree is missing, but production installs
  *do* have the source tree (install.sh writes it) — the missing
  signal was whether `cargo` was on `$PATH`. update.sh now also
  auto-switches to the pre-built release binaries when the Rust
  toolchain isn't available, so a fresh `bash /opt/dockpanel/scripts/update.sh`
  on a stock VPS works without the operator having to know about
  `INSTALL_FROM_RELEASE=1` or install ~4 GB of rustup. Developers who
  *do* want to compile from source can set `BUILD_FROM_SOURCE=1` to
  override. The "Rust toolchain not found" error message also rewords
  to recommend dropping `BUILD_FROM_SOURCE=1` over installing rustup,
  with the RAM-cost callout up front.

## [2.8.5] - 2026-05-01

### Fixed

- **v2.8.4 upgrade path still hit `duplicate listen options for [::]:443`
  on multi-site installs that ran v2.8.3 first
  ([#48](https://github.com/ovexro/dockpanel/issues/48)).** v2.8.4
  reverted the agent templates and panel vhost to plain
  `listen [::]:80;` / `listen [::]:443 ssl;`, and v2.8.4's update.sh
  stripped `ipv6only=on` from the panel vhost — but it dropped the
  v2.8.3 site-vhost migration block, so any site provisioned on v2.8.3
  kept `listen [::]:443 ssl ipv6only=on;` on disk. nginx accepts
  panel-plain + ONE site-with-`ipv6only=on` on a shared `[::]:443`
  socket, but rejects TWO-or-more site vhosts both setting
  `ipv6only=on` with `duplicate listen options for [::]:443`. The
  reload triggered by v2.8.4's update.sh therefore failed silently on
  any install with 2+ sites — the new panel listen never took effect,
  the IPv6 hijack from the original #48 persisted, and the next
  `systemctl restart nginx` would refuse to start. update.sh now
  strips `ipv6only=on` from every site vhost in
  `/etc/nginx/sites-enabled/*.conf` (skipping the panel vhost, which is
  already handled), bringing the listener options back in line so
  nginx reloads cleanly. No code changes — fix is pure upgrade-script.
  Manual one-liner for v2.8.4-stuck users:
  ```
  for f in /etc/nginx/sites-enabled/*.conf; do [ "$(basename "$f")" = dockpanel-panel.conf ] && continue; sed -i -E 's|^([[:space:]]*)listen \[::\]:(80\|443 ssl) ipv6only=on;|\1listen [::]:\2;|' "$f"; done && nginx -t && nginx -s reload
  ```

## [2.8.4] - 2026-05-01

### Fixed

- **v2.8.3 nginx `duplicate listen options` regression on multi-site
  installs.** v2.8.3 added `ipv6only=on` to `listen [::]:80` and
  `listen [::]:443 ssl` in agent templates + the panel vhost to fix the
  IPv6 hijack from #48. Two vhosts on the same shared socket both
  declaring `ipv6only=on` caused nginx to emit `duplicate listen
  options for [::]:80` and refuse the config — surfaced when a second
  site was added on a v2.8.3 install. Reverted: agent templates and
  the panel vhost now use plain `listen [::]:80;` and
  `listen [::]:443 ssl;` (dual-stack, no `ipv6only=on`). Linux's default
  dual-stack behaviour means a single shared `[::]` socket handles both
  IPv6 and IPv4-without-specific-binding, and nginx routes by
  `server_name` across that shared socket without conflict. The
  underlying #48 fix still holds — the panel vhost gains a `[::]:` IPv6
  listen so site vhosts can no longer be the only IPv6 listener and
  hijack panel-domain traffic. update.sh now also strips any
  `ipv6only=on` left on a v2.8.3 panel vhost so the upgrade path doesn't
  inherit the regression.

## [2.8.3] - 2026-05-01

### Fixed

- **Manual "Let's Encrypt SSL" provisioning failed with `Template
  render error: Invalid PHP socket path` on PHP sites
  ([#48](https://github.com/ovexro/dockpanel/issues/48)).** Four backend
  call sites built the agent's `php_socket` field as
  `/run/php/phpX-fpm.sock`, but the agent's strict validator
  (`is_safe_php_socket`, `panel/agent/src/services/nginx.rs:149`)
  requires the `unix:/...` prefix and 500'd the request. The auto-SSL
  background task at site-creation time was correct (`sites.rs:557`),
  which is why `Auto-SSL attempt 2` succeeded after the manual click
  500'd in between. Fixed in `routes/ssl.rs:118,404`,
  `services/auto_healer.rs:598`, and `services/security_scanner.rs:271`
  — all now emit `unix:/run/php/phpX-fpm.sock` like the working
  site-creation path.

- **Visiting the panel URL redirected to a freshly-installed WordPress
  site after that site's Let's Encrypt SSL was provisioned
  ([#48](https://github.com/ovexro/dockpanel/issues/48)).** Root cause
  was a dual-stack listen mismatch: agent-rendered site nginx vhosts
  declared `listen [::]:443 ssl;` (no `ipv6only=on`), but
  `scripts/setup.sh` bound the panel's vhost to IPv4 only. The first
  site to provision SSL therefore became the de-facto default for any
  IPv6 (or non-matched-IPv4) request — WordPress saw a Host that didn't
  match `home_url` and 301'd to its canonical domain. Fixed by adding
  `ipv6only=on` to all `[::]:80` and `[::]:443 ssl` listens across
  `panel/agent/src/templates/nginx/{http,https,proxy}.conf`, pairing
  every panel IPv4 listen in `setup.sh` with an `ipv6only=on` IPv6
  listen, and adding a one-shot migration in `scripts/update.sh` so
  existing installs gain the IPv6 listen on next upgrade.

## [2.8.2] - 2026-04-30

### Added

- **Chain-of-trust report extended to database + volume backups.**
  v2.8.1 shipped site-only because only the `backups` table carried
  integrity-hash columns. v2.8.2 lands the matching migration
  (`20260430200000_db_volume_backup_hashes.sql`) — `sha256_hash`,
  `previous_hash`, and `chain_valid` on both `database_backups` and
  `volume_backups`, applied in a single transaction so a partial apply
  can't leave one table chained and the other not. The agent now
  computes SHA-256 on every database dump (mysql / postgres / mongo) and
  every volume tarball, and the backend persists the hash + previous-hash
  link on the same INSERT path that lands the new backup row — both the
  on-demand routes (`POST /api/backup-orchestrator/db-backup` /
  `volume-backup`) and the policy-executor scheduled path. The All
  Backups tab now shows the `Report | JSON | PDF` 3-segment control on
  every row regardless of kind.

  The chain-report routes were collapsed from kind-specific
  (`/chain-report/site/{id}[/pdf]`) into one generic shape:
  - `GET /api/backup-orchestrator/chain-report/{kind}/{id}` — JSON.
  - `GET /api/backup-orchestrator/chain-report/{kind}/{id}/pdf` — PDF.

  `{kind}` ∈ `{site, database, volume}`; bogus kinds 400 cleanly. The
  JSON `backup` object now carries `kind` plus optional kind-specific
  fields (`database_id`, `container_id`, `volume_name`, `db_type`); the
  former `site_name` field was renamed to `resource_name` (domain for
  site, db_name for database, `container:volume` for volume) so the same
  consumer can render any kind. `build_site_chain_report` →
  `build_chain_report(kind, id)` with table-name dispatch. The typst
  template is now a single file that branches on `data.backup.kind` for
  the resource label and kind-specific extras (db engine, container ID),
  so the three kinds can't drift apart.

- **typst tarball SHA-256 pinning.** v2.8.1 trusted GitHub TLS for the
  v0.13.0 musl tarball (matching the existing grype installer). v2.8.2
  pins the per-arch SHA-256 (`x86_64-unknown-linux-musl`:
  `cd1148da…feb6`, `aarch64-unknown-linux-musl`: `1a1b3841…46e6`),
  verified at install time before `tar` ever sees the bytes. Operators
  can override per arch via `DOCKPANEL_TYPST_SHA256_X86_64` /
  `_AARCH64` env vars (e.g. air-gapped mirror, custom typst version).
  Mismatch surfaces as a distinct error rather than a generic install
  failure. Install timeout bumped 90 → 120 s to absorb the second pass
  over the bytes.

### Tests

- **`tests/chain-report-e2e.sh` extended to all three kinds.** The site
  block became a kind-agnostic `assert_kind` helper; the suite now
  iterates `site → database → volume` and runs the same shape of
  assertions per kind (auth gate, kind validation, 404 on bogus id,
  JSON 200, JSON `backup.kind` + `backup.id` + `backup.resource_name`
  round-trip + shape, PDF 200 + Content-Type + Content-Disposition
  + %PDF magic + size > 1 KB). Fixtures are discovered per kind from
  `backups` / `database_backups` / `volume_backups`; missing fixtures
  skip rather than fail (so CI hosts that haven't seeded volume backups
  still green-light). Total suite ~50 assertions.

## [2.8.1] - 2026-04-30

### Added

- **Chain-of-trust report for site backups** (Phase 4 W1.3). Every site
  backup is now downloadable as a single forensic artifact bundling its
  full provenance chain — the backup itself (filename, size, SHA-256,
  previous-hash link, chain-validity flag), every passive verification
  run against it (status, checks-passed/total, duration, errors), and
  every end-to-end restore drill (status, HTTP probe result, body
  excerpt, duration). Two formats from the same data:
  - `GET /api/backup-orchestrator/chain-report/site/{id}` — JSON.
  - `GET /api/backup-orchestrator/chain-report/site/{id}/pdf` — typst-rendered
    PDF with DockPanel branding, status pills, and a full chain-integrity
    summary. Designed to be handed to an auditor as proof a backup was
    actually verified and restorable.

  All Backups tab on the Backup Orchestrator page now shows a `Report
  | JSON | PDF` 3-segment control on every site row. The first PDF
  request lazy-installs the `typst` CLI into `/var/lib/dockpanel/typst/`
  (~30 MB, one-time, ~30 s on a fresh box); subsequent renders are
  instant. Compile timeout 30 s; install timeout 90 s; concurrent
  installs serialised via a process-wide async mutex so a burst of first
  requests doesn't stampede.

  Site-only for v2.8.1 because only `backups.sha256_hash` /
  `previous_hash` / `chain_valid` are populated today (added in audit
  migration `20260324000000`). The db + volume backup tables don't
  carry hashes yet — extending chain reports across all three kinds is
  a v2.8.2 follow-up that needs a hash-columns migration plus agent
  changes to compute SHA-256 during db/volume backup.

### Fixed

- **`/api/backup-orchestrator/health` 500 once any backup exists.**
  `SUM(size_bytes)` returns `NUMERIC` in PostgreSQL (since aggregating
  `BIGINT` can overflow `int8`); the existing query bound it to
  `Option<i64>` without an explicit cast. Empty backup tables returned
  `NULL` and decoded fine, but the moment a real backup row landed the
  endpoint started 500ing with `INT8 not compatible with NUMERIC`. Cast
  to `::bigint` in three sites in `routes/backup_orchestrator.rs::health`
  + the rolled-up SUM in `services/backup_policy_executor.rs`. Caught by
  the v2.8.1 fresh-VPS test once a synthetic backup row was seeded for
  the chain-report PDF round-trip.

### Tests

- New `tests/chain-report-e2e.sh` sub-suite: unauthenticated request
  blocked, bogus uuid → 404, JSON shape, PDF magic bytes / Content-Type
  / Content-Disposition, file-size sanity. Wired into `full-e2e.sh`
  alongside the tier2-pin sub-suite. Self-provisions auth (mints admin
  JWT from `api.env` if `DOCKPANEL_TEST_PASSWORD` is unset). Skips PDF
  assertion when `CHAIN_REPORT_SKIP_PDF=1` (CI without outbound HTTPS)
  and reports 503 cleanly when typst install fails so the suite still
  green-lights on networks that block GitHub releases.

## [2.8.0] - 2026-05-01

### Added

- **Restore Confidence SLA card on Backup Orchestrator overview** (Phase 4
  W1.1). The Overview tab now leads with a single trust signal — "of last
  30 backups, X% verified" — sized as a headline number, color-coded by
  threshold (rust ≥95%, warn ≥80%, danger below). Adjacent cells show p50
  and p95 verify-lag (time from backup creation to verification
  completion), oldest unverified backup age, and a per-server breakdown
  table when more than one server is registered. Empty state when no
  recent backups exist. Backend extends `GET /api/backup-orchestrator/health`
  with `sla_window`, `sla_verified`, `sla_failed`, `sla_pending`,
  `verify_lag_p50_hours`, `verify_lag_p95_hours`, `oldest_unverified_days`
  (previously declared but never populated), and `per_server_sla[]`.
  Latest verification per (backup type, backup id) wins, so re-runs
  supersede stale entries. No schema migration; same endpoint URL.
- **End-to-end backup drills for site backups** (Phase 4 W1.2 part A).
  Click `Drill` on any site row in the All Backups tab — the agent extracts
  the tar to a scratch directory, spins a hardened `nginx:alpine` container
  (`--network none`, `--read-only`, 128MB / 0.5 CPU caps), HTTP-probes
  `localhost/` via `docker exec wget`, and tears everything down. Persisted
  in a new `backup_drills` table; visible in the new Drills tab with status,
  HTTP code, duration, and error message. SLA card on Overview gains a
  "End-to-end drills (30d): N passed · M failed" line when drills exist.
  New endpoints: `POST /api/backup-orchestrator/drill` (admin, async — returns
  202 immediately, drill row updates as the agent finishes) and
  `GET /api/backup-orchestrator/drills` (paginated history). Agent endpoint
  `POST /backups/drill/site`.
- **End-to-end DB drills for postgres + mysql/mariadb** (Phase 4 W1.2 part B).
  Click `Drill` on any database row in the All Backups tab — the agent boots
  a scratch engine container (`postgres:16-alpine` or `mariadb:11`,
  `--network none`, 256MB / 1 CPU caps), pipes `zcat` of the dump into a
  direct-fd `psql`/`mariadb` restore, runs `ANALYZE` (postgres) to populate
  planner stats, then sums table count and row totals from
  `pg_class.reltuples` / `information_schema.tables.table_rows`. Drill body
  records `"N tables, ~M rows restored"` — strictly stronger than verify,
  which only confirms the dump applies. Pass requires tables > 0; row
  total is reported but doesn't gate (legitimate schema-only dumps pass).
  Backend `POST /api/backup-orchestrator/drill` now accepts
  `backup_type = "database"` and dispatches to new agent route
  `POST /backups/drill/db`. Drills tab "HTTP" column renamed to "Result"
  and renders the row/table summary for DB drills. Volume drill is W1.2.c.
- **End-to-end volume drills** (Phase 4 W1.2 part C). Click `Drill` on any
  volume row in the All Backups tab — the agent creates a scratch Docker
  volume, runs a hardened `alpine:3.19` restore container (`--network none`,
  128MB / 0.5 CPU caps) that extracts the tar into the scratch volume
  (parity with `restore_volume`'s actual restore path), then runs a second
  read-only probe container that mounts the volume RO and read-tests up
  to 20 sample files (`head -c 1` through each — enough to fault
  filesystem-level corruption without scanning multi-GB volumes). Drill
  body records `"N files, M bytes restored"`. Pass requires files > 0
  AND read-test exit 0 — strictly stronger than verify, which only
  extracts to a host /tmp dir. Best-effort cleanup of both containers
  and the scratch volume on every exit path. Backend
  `POST /api/backup-orchestrator/drill` now accepts `backup_type = "volume"`
  and dispatches to new agent route `POST /backups/drill/volume`. The
  `—` placeholder on volume rows is replaced with a working `Drill`
  button. W1.2 engine work complete; W1.2.d (per-policy weekly drill
  scheduler) is the remaining slice.
- **Per-policy drill scheduler** (Phase 4 W1.2 part D). Backup policies
  gain a `Drill on schedule` toggle and a separate cron `drill_schedule`
  (default `0 4 * * 0` — 04:00 UTC Sunday) so drills run on a different
  cadence from the backups themselves. New backend service
  `drill_scheduler` ticks every 60s, finds policies due now, looks up
  the latest `database_backups` and `volume_backups` row tied to each
  policy by `policy_id`, and dispatches a real drill against each via
  the same agent endpoints used by on-demand drills. Records land in
  the existing `backup_drills` table — Drills tab can't tell the
  difference between scheduled and on-demand drills (same audit
  trail). Per-server concurrency cap = 1 (skips dispatch if a
  `pending`/`running` drill exists for the same server). Schema
  migration adds `drill_enabled BOOLEAN`, `drill_schedule TEXT`, and
  `last_drill_at TIMESTAMPTZ` to `backup_policies`. Site backups don't
  carry `policy_id` and are not covered by this scheduler — they stay
  on the existing 6h `backup_verifier` cadence. UI: new section in the
  Policy create form with an enabled checkbox + a curated schedule
  selector (weekly / monthly / every 3 days), and a small `drill <cron>`
  badge under the Schedule column on policy rows when enabled. Cron
  validation rejects strings that aren't 5-field whitespace-separated
  on both `schedule` and `drill_schedule` writes (was previously
  unchecked on `schedule` too — small hardening win). W1.2 (engines +
  scheduler) is now complete; W1.3 (chain-of-trust PDF/JSON export)
  ships separately as v2.8.1.

### Polish

- **Backup Orchestrator UX pass**. Drills tab now paginates with
  `Prev`/`Next` (50 per page) instead of silently truncating to the
  first 100; backend `GET /api/backup-orchestrator/drills` returns
  `{items, total}` to drive it. Result column is tone-coded for site
  drills — HTTP 2xx rust, 3xx neutral, 4xx amber, 5xx danger — so
  failures jump out at a glance. Running drills get a pulsing dot in
  the status pill and a `N running` counter + manual `Refresh` button
  above the table. Created column shows relative time with the
  absolute timestamp on hover. Drill button on DB and volume rows now
  asks once before spending — a confirm/cancel pair appears with the
  cost hint (`boots a 256 MB scratch DB engine, ~60s` /
  `boots a 128 MB scratch container + temp volume, ~60s`); site drills
  fire directly since they're cheap.
- **Image scan + SBOM Settings cards** (a25c716). Apps CVE drawer +
  Settings ImageScan/SBOM cards picked up the same dialog/a11y polish
  as the rest of the panel: `role="dialog"` + `aria-modal` + Esc to
  close on the scan drawer, `type="button"` + `aria-label` on every
  trigger, design-system tokens (no raw Tailwind colors), explicit
  load-error + Retry on the Settings cards (no more stuck "Loading…"),
  `Last scan Xh ago · N images on file` derived from
  `/image-scan/recent` when the scanner is installed, and an explicit
  `On-demand only — no schedule, no deploy gate` line on the SBOM
  card so the configuration model is unambiguous.

## [2.7.20] - 2026-04-28

### Security

- **rustls-webpki 0.103.12 → 0.103.13** in both `dockpanel-api` and
  `dockpanel-agent` Cargo locks — fixes `RUSTSEC-2026-0104` (reachable
  panic in CRL parsing). DockPanel calls into rustls-webpki for ACME
  cert verification and pinned-fingerprint TLS (Phase 3 #3 Tier 2), so
  a malformed CRL from a malicious or buggy CA could have crashed the
  process. Patch release, no API changes.
- **postcss 8.5.8 → 8.5.12** in `panel/frontend` and `website/client`
  package locks — fixes `GHSA-7fh5-64p2-3v2j` (XSS via unescaped
  `</style>` in the CSS stringify output). Build-time only; no
  runtime exposure on shipped panels — but worth keeping current.

### Added

- **Servers page: last-seen-at + 24h uptime sparkline.** Each server card
  now shows a small `Last seen 14s ago` line under the IP/status
  subtitle (driven by the existing `last_seen_at` column, refreshed on
  every agent checkin) and a 144-cell horizontal uptime strip — one
  cell per 10-minute bucket over the last 24 hours, derived from
  `metrics_history` row presence. Hover any cell for its time window
  and online/no-data label. New endpoint `GET /api/servers/{id}/uptime`
  returns `{ buckets: bool[], window_hours, bucket_minutes }`. Owner-
  scoped (404 on a server that belongs to a different user); same auth
  shape as the rest of the `/api/servers/*` surface.
- **Pre-built Grafana dashboard (`dashboards/dockpanel-grafana.json`).**
  Drop-in companion to the v2.7.16 Prometheus exporter. Covers fleet
  stats (version / servers reporting / sites / alerts firing by
  severity / GPUs reporting), per-server CPU / memory / disk timeseries
  with sensible thresholds, top-servers bar gauges, sites-by-status
  donut, a collapsible GPUs row (utilization, VRAM%, temperature, power
  draw), and an alerts-firing stacked-bars timeseries. Uses a
  `Datasource` template input so it imports cleanly onto any Prometheus
  that's already scraping `/api/metrics`. UID `dockpanel-fleet` is
  stable so runbook deep-links survive re-imports. A `Server` template
  variable lets operators focus on a single host or any subset. See
  `docs/guides/prometheus.md` "Pre-built Grafana dashboard" for import
  instructions. Closes the Phase 3 #1 follow-up that paired with the
  Prometheus endpoint.
- **Tier 2 cert-pin E2E test suite (`tests/tier2-pin-e2e.sh`).** Covers
  every step of the Phase 3 #3 Tier 2 flow end-to-end against the live
  API: TOFU fingerprint capture on `/api/agent/checkin`, match no-op,
  MITM 403, malformed-fingerprint 400, admin rotate-cert-pin with and
  without the `X-Requested-With` CSRF header, `activity_logs` capture
  of the rotate action, and re-TOFU after rotate. Also includes a
  dedicated regression guard for the v2.7.18 rustls `CryptoProvider`
  panic — it inserts a synthetic online server row with
  `cert_fingerprint` set and a loopback URL with no listener, then
  `POST /api/servers/{id}/test` and asserts status exactly 502
  (graceful connect failure) — a panic would surface as 500 and be
  caught. The suite is self-provisioning: it mints an admin JWT
  locally from `/etc/dockpanel/api.env` when `DOCKPANEL_TEST_PASSWORD`
  is unset, and cleans up all DB rows it creates via an `EXIT` trap.
  Wired into `tests/full-e2e.sh` as a sub-suite at the end of the run.

## [2.7.19] - 2026-04-17

### Fixed

- **Remote-agent TLS pinning no longer panics the API process.** v2.7.18
  shipped the `PinnedFingerprintVerifier` for outbound backend→agent TLS
  but the backend's `main.rs` never installed a process-level rustls
  `CryptoProvider`. On the first request that actually exercised the
  pinned path (i.e. a second server enrolled in the fleet with a
  captured fingerprint), `rustls::ClientConfig::builder()` panicked on
  `CryptoProvider::get_default()`. Pure single-host installs were not
  affected; any multi-server deployment using the pinned verifier was.
  Fix: call `rustls::crypto::aws_lc_rs::default_provider().install_default()`
  at `dockpanel-api` startup (the agent already did this at `main.rs:24`).
  Caught by the v2.7.18 fresh-VPS test before v2.7.18 was declared
  public-ready. No API changes; the Tier 2 part 2 verification flows
  (TOFU capture, MITM 403, rotate-pin, re-TOFU, PinnedFingerprintVerifier
  accept/reject) now all succeed end-to-end.

## [2.7.18] - 2026-04-17

### Added

- **`RemoteAgentClient` cert-pinning enforcement (Phase 3 #3 — Tier 2,
  part 2).** Closes the loop: once an agent's fingerprint has been
  captured by the backend (Tier 2 part 1), every outbound TLS handshake
  to that agent goes through a custom `rustls::client::danger::ServerCertVerifier`
  that only accepts a cert whose DER SHA-256 matches the pinned value.
  Comparison is constant-time via `subtle`; signature verification
  delegates to `rustls::crypto::aws_lc_rs`. When `cert_fingerprint` is
  still NULL for a server (e.g. old agent that doesn't report it), the
  client falls back to the legacy `AGENT_TLS_VERIFY=insecure` env flag
  for backwards compatibility.
  - `AgentRegistry::for_server` now reads `cert_fingerprint` from the
    `servers` row and passes it to `RemoteAgentClient::new_with_pin`.
    Rotating the pin via `POST /api/servers/{id}/rotate-cert-pin` already
    invalidates the cached client (shipped in Tier 2 pt1) so the next
    request rebuilds with the new pin.
- **Agent TLS + cert fingerprint pinning (Phase 3 #3 — Tier 2, part 1).**
  The agent's multi-server listener now terminates TLS instead of shipping
  auth tokens in plaintext, and the central panel captures each agent's
  cert fingerprint on first checkin for later pinning.
  - Agent loads `/etc/dockpanel/ssl/agent.{crt,key}` at startup (generated
    at install time by `install-agent.sh`, or generated on first boot via
    `rcgen` when missing). `AGENT_LISTEN_TCP=0.0.0.0:9443` now binds a
    TLS listener via `axum-server` + `rustls` — the old plaintext bind
    and the `AGENT_ALLOW_INSECURE_BIND` escape hatch are removed, since
    TLS makes the 0.0.0.0 case safe by construction.
  - Agent computes the SHA-256 (hex) fingerprint of its cert at startup,
    logs it on first boot, and includes it in every phone-home checkin.
  - Migration `20260417000000_agent_cert_fingerprint.sql` adds
    `servers.cert_fingerprint` (nullable varchar(64) + partial index).
  - Backend `POST /api/agent/checkin` captures the fingerprint on first
    checkin (Trust On First Use); on subsequent checkins a mismatch is
    rejected with 403 and logged at ERROR level. Format-validated
    (64-char lowercase hex) before storage.
  - New admin endpoint **`POST /api/servers/{id}/rotate-cert-pin`**
    clears the stored fingerprint so the next checkin re-captures. Use
    after a legitimate agent cert rotation or reinstall. Invalidates the
    cached `RemoteAgentClient` and writes an audit log entry.
  - Servers page gains a per-server TLS pin row showing the shortened
    fingerprint (first 16 / last 16 chars, full hash on hover) and a
    "Rotate pin" button with an inline confirmation bar.
  - Pt2 (pin-enforcement in `RemoteAgentClient`) ships in the same
    release — see the first bullet above.
- **Unified fleet-wide backup view (Phase 3 #3 — Tier 1).** The Backup
  Orchestrator page gains an **All Backups** tab that lists site, database,
  and volume backups from every server in a single paginated table, with
  optional filters by server and by kind.
  - New admin endpoint **`GET /api/backup-orchestrator/all`** joins
    `backups`, `database_backups`, and `volume_backups` via a UNION CTE
    and resolves `server_id` to a server name (site backups derive their
    server from `sites.server_id`; database and volume backups carry the
    column directly). Query params: `limit`, `offset`, `kind`
    (`site`|`database`|`volume`), `server_id`. Returns `{ items, total }`.
  - Per-row badges surface `encrypted` (at-rest encryption enabled) and
    `remote` (pushed to a backup destination) so fleet admins can spot
    inconsistencies at a glance.
  - Closes the last missing north-star bullet for "Operate at Scale":
    agent enrollment and cross-host placement were already shipped
    (`ServerScope` + `servers` table + `install-agent.sh`); the unified
    backup view was the remaining gap.

## [2.7.17] - 2026-04-16

### Added

- **2026-ready ACME (Phase 3 #2 — Tier 1).** DockPanel is now ready for
  Let's Encrypt's May 13 2026 `tlsserver` → 45-day flip, the existing 6-day
  `shortlived` profile, and the Feb 2027 / Feb 2028 `classic` reductions.
  - **RFC 9773 ARI-driven renewal.** The auto-healer now queries the CA's
    ACME Renewal Information for each cert and honours the suggested
    renewal window instead of a hard-coded 30-day threshold. Falls back to
    a profile-aware margin (2d / 15d / 30d) when a CA doesn't advertise
    ARI. New columns `sites.ssl_renewal_at`, `sites.ssl_renewal_checked_at`.
  - **ACME profile selection UI.** Settings → ACME Profile lets admins
    pick the default profile (`classic` / `tlsserver` / `shortlived`) for
    all new certificates. List auto-populates from the CA's server
    directory; card hides itself if the CA doesn't advertise the profiles
    extension. New column `sites.ssl_profile` stores which profile issued
    each cert.
  - **Force-renew migrated off certbot CLI.** `/api/ssl/{id}/renew` now
    issues via `instant_acme` and passes the previous cert as the ARI
    `replaces` hint, so the CA sees a continuous issuance chain. Legacy
    certbot-issued certs no longer trigger spurious failures on renew.
  - **`/api/ssl/profiles`** (admin) lists CA-advertised profiles with
    descriptions. **`/api/ssl/default-profile`** (admin) sets or clears the
    panel-wide default. **`/ssl/{domain}/renewal-info`** (agent) exposes
    the raw ARI suggestion per cert.

### Changed

- Auto-heal SSL copy in Settings replaced stale "3 days" threshold
  language with accurate ARI + profile-aware explanation.
- DNS-PERSIST-01 (Q2 2026) intentionally deferred — no Let's Encrypt
  production date yet; will land once instant-acme exposes the draft API.

## [2.7.16] - 2026-04-16

### Added

- **Prometheus `/api/metrics` scrape endpoint (Phase 3 #1).** Hand-formatted
  exposition text — no extra crate, respects the lightness axis. Gated by a
  SHA-256-hashed scrape token (constant-time compare via `subtle`); returns
  404 when disabled so an off panel doesn't advertise a scrape surface.
  Exposes `dockpanel_info`, per-server cpu/memory/disk percents, per-GPU
  utilization / VRAM / temperature / power, per-status site counts, and
  alerts firing by severity. New `PrometheusSettings` card in Settings
  with auto-generated token, reveal-once banner, rotate button, and a
  copy-ready `prometheus.yml` scrape_configs block.

## [2.7.15] - 2026-04-16

### Added

- **GPU history + alerts (Phase 2 #2).** Historical GPU charts in System
  (utilization, VRAM, temperature, power). Alert engine gains GPU-aware
  rules: VRAM > 90%, temp > 85°C, utilization pinned at 100% for 15 min.
- **Ollama model management + vLLM picker + idle-unload (Phase 2 #3).**

### Changed

- **CI on Actions Node 24.** Upgraded action pins to their Node-24-ready
  versions, including `sigstore/cosign-installer@v4.1.1` (no floating v4
  tag exists). `cargo install cargo-sbom` is now called with `--force` so
  restoring a cached `~/.cargo/bin/` doesn't break the release workflow.

## [2.7.14] - 2026-04-15

### Fixed

- **`scripts/update.sh` now self-refreshes from the latest release tag.**
  The v2.7.13 fix to the rollback bug only helped operators who manually
  refreshed their on-disk copy of update.sh, because update.sh wasn't
  in the binary release tarball and never overwrote itself during an
  upgrade. v2.7.14 closes the chicken-and-egg: when run with
  `INSTALL_FROM_RELEASE=1`, update.sh fetches the latest tag's
  `scripts/update.sh` from raw.githubusercontent.com, replaces its own
  on-disk copy if it differs, and re-execs. A `SELF_REFRESHED=1` env
  guard prevents infinite loops.

  **Operators currently stuck on v2.7.11 or v2.7.12** (where the broken
  health check rolls every upgrade back) need to bootstrap once:
  ```
  sudo curl -fsSL https://raw.githubusercontent.com/ovexro/dockpanel/main/scripts/update.sh \
       -o /opt/dockpanel/scripts/update.sh
  sudo INSTALL_FROM_RELEASE=1 bash /opt/dockpanel/scripts/update.sh
  ```
  After the first successful upgrade, future runs self-refresh
  automatically.

## [2.7.13] - 2026-04-15

### Fixed

- **`scripts/update.sh` rolled back every upgrade** — the post-deploy
  health check POSTed to `/api/auth/setup-status`, but that endpoint is
  GET-only and returned 405 Method Not Allowed on every run, triggering
  the rollback path even when the new binaries were healthy. Caught by
  the v2.7.12 fresh-VPS test (the first end-to-end `update.sh` exercise
  in several releases). Operators on v2.7.11 or v2.7.12 who pulled via
  `update.sh` would have been silently held back; manual re-pull or
  reinstall via `install.sh` was unaffected. Fix: switch the check to
  GET.

## [2.7.12] - 2026-04-15

### Added

- **Per-container GPU assignment.** Multi-GPU hosts can now pin specific
  NVIDIA devices to specific containers — pin Ollama to GPU 0, vLLM to
  GPU 1, Stable Diffusion to GPU 2. The deploy form auto-detects available
  GPUs (via the existing `/apps/gpu-info`) and shows a multi-select picker
  on hosts with two or more devices. Single-GPU hosts keep the original
  simple toggle. Backed by Docker's `DeviceRequest.device_ids`; assignment
  persists across `update_app()` recreations because Docker preserves the
  host_config when pulling a new image.
- **vLLM template (AI / Machine Learning).** High-throughput, memory-
  efficient LLM inference server with an OpenAI-compatible API. Defaults
  to `meta-llama/Llama-3.2-1B-Instruct` and accepts an optional
  `HUGGING_FACE_HUB_TOKEN` for gated models. Fills the most-glaring AI
  template gap (the inference-engine peer to Ollama).
- **`gpu_recommended` flag on app templates.** Templates that materially
  benefit from GPU passthrough (Ollama, LocalAI, vLLM, Stable Diffusion
  WebUI, Text Generation WebUI, Whisper) now ship a flag that surfaces a
  small "GPU" badge on the template card and pre-ticks the GPU passthrough
  toggle on the deploy form. Frontends/orchestrators (Open WebUI,
  LiteLLM, Flowise, Langflow, Dify) intentionally remain unflagged.

### Changed

- **LocalAI default image switched to GPU variant.**
  `localai/localai:latest-cpu` → `localai/localai:latest-gpu-nvidia-cuda-12`.
  The previous default silently ignored the GPU passthrough toggle on
  every deploy. Operators on CPU-only hosts can switch back via the Image
  field on the deploy form.
- **Text Generation WebUI pinned** from `:default-nightly` to `:default`
  so shipped deploys don't drift on rebuild.

### Public

- **dockpanel.dev/security launched.** Public security posture page —
  audit count, signed-releases / SBOM story, response SLA, all 7 audit
  rounds with headline fixes, recent advisories, defense-in-depth grid,
  vulnerability-report CTA. Counter-positions DockPanel against the
  Coolify/CyberPanel narratives. Linked from main nav (between Compare
  and Pricing) and footer Product column. SECURITY.md cross-references
  the page at the top.

## [2.7.11] - 2026-04-15

### Added

- **Per-image SBOM generation (syft).** Second half of the Phase 1 supply-chain
  story (after v2.7.10's signed releases). Generate an SPDX 2.3 JSON SBOM for
  any deployed Docker app's image — the composition companion to image
  vulnerability scanning. Defaults to **off**; admins opt in from
  Settings → Services → SBOM Generation.
  - **Install button** pulls Anchore's signed syft installer into
    `/var/lib/dockpanel/scanners/syft` (same self-contained, sandbox-safe
    pattern as grype — works under `ProtectSystem=strict`).
  - **Download SBOM button** in each app's scan drawer. Click runs syft against
    the app's image (10 – 60 s on first generation), persists the SPDX
    document, and triggers a browser download of `<app>.spdx.json`.
  - **Persistence** — `image_sbom` table holds one row per image, overwritten
    on regeneration. Stored as JSONB so the API serves the SPDX document
    directly without re-parsing on the agent.
  - **API surface** mirrors `/api/image-scan/...` shape:
    `/api/sbom/{settings,install,uninstall,generate,image/{ref}}` plus
    `/api/apps/{name}/sbom` for both POST (generate) and GET (download).
  - **Agent image-ref validator** rejects shell metacharacters before invoking
    syft — defence-in-depth against shell-injection via user-supplied refs.

This is the operator-facing half: every container running on the panel now has
a one-click supply-chain artifact to satisfy compliance asks (EU CRA Sep 2026)
and to feed external tooling like Dependency-Track or Grype-on-SBOM.

## [2.7.10] - 2026-04-15

### Added

- **Signed releases via cosign keyless (Sigstore).** Every binary and SBOM in
  the GitHub release is now signed in CI using the release workflow's OIDC
  identity — no long-lived signing key exists, and every signature is recorded
  in the public Rekor transparency log. Verification snippet in
  [SECURITY.md](SECURITY.md#verifying-release-signatures).
- **Per-binary SPDX 2.3 SBOMs.** `cargo-sbom` runs in CI for the agent, API,
  and CLI crates, emitting `dockpanel-{agent,api,cli}.spdx.json` alongside the
  binaries (also signed). Local builds via `scripts/release.sh` now generate
  SBOMs too; signing remains CI-only so the OIDC-bound certificate identity is
  always traceable to this repository's release workflow.

This is the first half of the Phase 1 supply-chain story — the next release
exposes per-deployed-container SBOMs in-panel.

## [2.7.9] - 2026-04-15

### Added

- **Per-image vulnerability scanning (grype).** First feature in the Phase 1
  "Trust by Default" cycle. Scans every Docker app's image for known CVEs and
  surfaces a severity badge per app row on the Apps page, next to the existing
  update badge. Click a row to see the full CVE table (CVE ID, severity,
  package, installed version, fixed version). Defaults to **off** so existing
  installs see no behaviour change on upgrade — admins opt in from
  Settings → Services → Image Vulnerability Scanning.
  - **Install button** pulls Anchore's signed grype installer into
    `/var/lib/dockpanel/scanners/` (self-contained — doesn't pollute
    `/usr/local/bin` and works under the hardened agent sandbox). The
    vulnerability database primes during install.
  - **Scheduled scans** rescan every running app's image in the background at
    a configurable interval (default 24h, range 1–720h).
  - **Soft deploy gate** refuses new deploys if the template's image has a
    recent scan exceeding a threshold (`critical` / `high` / `medium`). First
    encounter of an image triggers a best-effort background scan so the next
    deploy enforces the gate without blocking the first one.
  - **Scan-on-demand** from the per-app drawer. Ad-hoc scan of any image via
    `POST /api/image-scan/scan`.
  - **Agent image-ref validator** rejects shell metacharacters before invoking
    grype — defence-in-depth against shell-injection via user-supplied image
    references.

### Fixed

- **`/var/lib/dockpanel` was missing from the hardened agent sandbox's
  `ReadWritePaths`.** Audit 7 introduced `ProtectSystem=strict` on the agent
  unit file (`panel/agent/dockpanel-agent.service`) but only listed
  `/etc/nginx`, `/etc/dockpanel`, `/var/run/dockpanel`, `/var/backups/dockpanel`,
  `/var/www`, `/var/log`, `/etc/letsencrypt` — which meant git builds, terminal
  recordings, mail backups, Docker app volumes, and the new image scanner would
  all have silently failed if anyone deployed the hardened unit verbatim. Added
  `/var/lib/dockpanel` to the path list. (Installer scripts still emit
  `ProtectSystem=no` units, so fresh installs from `install.sh` / `update.sh`
  were not affected.)

## [2.7.8] - 2026-04-15

### Security (Audit Round 7)
- **tar backups now use `--no-dereference`** — full-site backups, WordPress
  pre-update snapshots, and mailbox archives no longer follow symlinks inside
  the site root. A symlink pointed at `/etc` would previously have been
  archived as the target's content.
- **Cron command filter explicitly rejects `\n` and `\r`** — was implicit
  before; defense-in-depth against scheduled-job newline injection.
- **Web-terminal command blocklist extended** — `chroot`, `pivot_root`,
  `capsh`, `mknod`, `debugfs`, `kexec` added to the pattern list.
- **Agent systemd unit hardened** — `ProtectKernelTunables`,
  `ProtectControlGroups`, `ProtectClock`, `ProtectHostname`, `RestrictRealtime`,
  `RestrictSUIDSGID`, `LockPersonality`, `RestrictNamespaces=~CLONE_NEWUSER`.
- **Frontend URL guards** — Telemetry's update-release link and the public
  status page's operator-supplied logo URL now require `http(s)://` schemes,
  blocking `javascript:` / `data:` URLs routed through backend-controlled
  config fields.

### Fixed
- **Security-scan alert pileup eliminated.** The weekly security scanner fired
  a new alert on every run without resolving prior firing alerts, so
  unacknowledged alerts compounded and the escalation loop re-notified every
  2–5 minutes. New scans now auto-resolve prior firing/acknowledged security
  alerts before firing their own result.

### Improved
- **README / COMPARISON / docs RAM claim updated** — previous "~57MB" figure
  was stale. Fresh Vultr VPS measurement: panel services alone idle at ~19 MB
  (agent 12 MB + API 7 MB), or ~85 MB including the bundled PostgreSQL.
  Landing-page RAM bar now shows 19 MB.

## [2.7.7] - 2026-04-15

### Fixed
- **File Manager uploads were silently broken.** The wired agent upload handler
  expected `{path, content_base64}` while the backend (and frontend) sent
  `{path, filename, content}`. A second handler in `agent/routes/files.rs` had
  the right shape but was never wired to a router. Fixed the wired handler to
  accept the real payload (with `content_base64` alias for backwards
  compatibility) and removed the orphan duplicate.
- **Per-site PHP-FPM pool config changes never took effect.** Agent called
  `write_php_pool_config(...)` but never reloaded PHP-FPM afterwards, so custom
  `php_memory_mb` / `php_max_workers` per site were ignored until a manual
  restart. Wired `reload_php_fpm` right after the pool write.
- **Installer silently fell back to IP-only mode over non-interactive SSH.**
  Piping `install.sh` through an SSH session with no controlling tty made
  `read < /dev/tty` fail silently and cleared `PANEL_DOMAIN`. Now prints a
  clear "no tty — set PANEL_DOMAIN to configure" notice and points at the
  env var.
- **`/var/lib/dockpanel/recordings` was never created on fresh install.** The
  terminal-recording API and auto-healer retention sweep both reference it.
  Added to the installer's `mkdir -p` list.

### Removed
- Agent dead code: `restart_app_service`, `app_service_status`, `build_labels`,
  `connect_to_network` (Docker-label routing superseded by file-provider
  `write_route_config`), `volume_backup::get_backup_path` (duplicate), and
  `BackupInfo::new`.

## [2.7.6] - 2026-04-14

### Improved
- **Complete UX polish pass** — all remaining 12 pages reviewed and polished
- Mail: success feedback for alias/backup delete, queue error handling, logs loading skeleton
- Security: all raw Tailwind colors replaced with design system tokens (lockdown, audit log, approvals)
- Settings: success feedback for destination delete, API key revoke, lockdown threshold save; SSH key error handling; empty states for SSH keys and IP whitelist
- Monitors: success feedback for create/toggle/delete operations
- IncidentManagement: inline delete confirmations (was direct delete), success feedback, settings tab empty state
- WordPressToolkit: success banner for bulk update and hardening actions
- Telemetry: fix unsafe error casts, fix version display bug (`vundefined`), color consistency
- Login: loading spinner instead of blank page during auth check
- Integrations: loading skeletons for WHMCS and Migrations tabs
- NexusLayout: add missing incident count badge (consistent with other 3 layouts)
- Color consistency: `emerald`/`green`/`red` → `rust`/`danger` design tokens across 5 files

### Removed
- **Zero `any`** remaining in entire frontend (37 new TypeScript interfaces, completed in v2.7.5 cycle)

### Security
- Updated `rand` 0.9.2 → 0.9.4 (fixes 2 low-severity Dependabot alerts — soundness with custom loggers)

## [2.7.5] - 2026-04-14

### Improved
- **Systematic UX polish** across 20+ frontend pages
- All `confirm()` dialogs (25) replaced with inline confirmation bars across 5 files
- All `prompt()` calls (6) replaced with inline input forms across 5 files
- All `console.error/warn/log` removed from frontend page components
- All `bg-rust-50` light-mode colors replaced with dark-mode-compatible `bg-rust-500/10` (8 files)
- SiteDetail: loading skeletons for traffic stats, PHP extensions, access logs; WAF empty state
- Databases: success feedback for create/delete/PITR toggle; typed SchemaBrowser generics
- File Manager: save success indicator, Ctrl+S keyboard shortcut
- DNS: 16 `any` type casts replaced with 5 proper TypeScript interfaces

### Security
- Upgraded `rand` 0.8 → 0.9.3 (fixes 2 Dependabot security alerts)
- Upgraded `vite` 6.4.1 → 6.4.2 (fixes 2 high + 2 medium Dependabot alerts)

### Added
- Git hooks: pre-commit (infrastructure leak scan), pre-push (secrets + frontend staleness + version consistency)
- Scripts: `docs-audit.sh`, `release.sh` (x86_64 + ARM64 cross-compile), `deploy-check.sh`

## [2.7.4] - 2026-04-03

### Security
- JWT role staleness: sessions now invalidated immediately on role change (was stale up to 2h)
- Webhook gateway DNS rebinding SSRF: destination URL re-validated at forward time, not just registration
- Agent checkin replay prevention: timestamp validation rejects requests >120s old
- Per-user ACME rate limiting: max 10 SSL certificates per hour per user (HTTP-01 and DNS-01)
- DNS pre-flight check: verify domain resolves to this server's IP before HTTP-01 provisioning
- Request timeout: 300s TimeoutLayer added as defense-in-depth against slow requests
- Agent response streaming limit: uses `http_body_util::Limited` instead of buffering entire response before size check

### Fixed
- Docker container logs now strip ANSI escape sequences instead of returning raw escape codes

## [2.7.3] - 2026-04-03

### Added
- **GPU monitoring dashboard** — VRAM used/free, temperature, power draw, fan speed, per-process usage with automatic Docker container name resolution. Shown in System Health tab. Gracefully hidden when no GPU detected.
- GPU process table maps PIDs to Docker container names via /proc cgroup inspection

### Changed
- Certbot installer upgraded from apt (2.9.0) to snap (4.x with ARI support for upcoming 45-day LE certificates). Falls back to pip if snap unavailable.
- OWASP CRS updated from v4.4.0 to v4.25.0 LTS

### Security
- Fixed CVE-2026-21876 (CVSS 9.3): OWASP CRS multipart charset validation bypass
- Fixed CVE-2026-33691: OWASP CRS file upload whitespace bypass

## [2.7.2] - 2026-04-02

### Changed
- System updates now stream apt output in real-time via NDJSON instead of buffering entire output
- Agent `apply_updates` returns streaming response (newline-delimited JSON) for live terminal experience
- Backend consumes streamed agent response via new `post_long_ndjson()` method, forwarding lines as SSE events
- Added `stream` feature to reqwest for chunked response handling on remote agents

## [2.7.1] - 2026-03-31

### Changed
- Version numbers synced across all packages: 2.0.6 → 2.7.0 in agent, API, CLI, and frontend
- API endpoint count updated to 733 (465 backend + 268 agent) across all docs and marketing
- E2E test count updated to 476 (8 test suites) across all docs and marketing
- Docker template count corrected to 151 across 14 categories in docs site (was stale at 54)
- Security audit rounds updated to 6 (was showing 5) in README and SECURITY.md
- SECURITY.md now documents Audit Round 6 (zero-assumptions, 30 fixes, 260+ total)
- FEATURES.md verified metrics updated with precise counts from code
- CONTRIBUTING.md migration count updated (69 → 81)
- COMPARISON.md corrected: RAM 60→57MB, templates 54→151, themes/layouts names fixed
- Docs site getting-started.md RAM corrected (60→57MB)
- Marketing site Landing.tsx updated with all corrected numbers

### Fixed
- Removed 3 orphaned lazy imports in frontend main.tsx (IncidentManagement, SecurityHardening, WebhookGateway — absorbed into consolidated pages)

## [2.7.0] - 2026-03-30

### Security — Fresh Zero-Assumptions Audit (Audit 6)
- 6 parallel agents audited 222 Rust + 506 TypeScript files from scratch
- 33 findings fixed across 24 files (11 HIGH, 22 MEDIUM)
- MySQL password reset: fixed SQL injection via wrong quote escaping
- Deploy script: added `is_safe_shell_command()` validation before agent forwarding
- Laravel migration: replaced shell interpolation with dedicated safe agent endpoint
- Terminal: sanitized uploaded filename before shell echo
- CSRF: added `X-Requested-With` header enforcement on all mutating cookie-auth requests
- Compose YAML: rewrote validator from string matching to parsed AST (serde_yaml_ng)
- Shell command blocklist: added encoding tools, interpreters, network tools
- Cron filter: blocked `xxd`, `openssl enc`, `python3 -c`, process substitution
- Remote agent TLS: default inverted from insecure to strict
- Agent TCP: refuses `0.0.0.0` bind without explicit `AGENT_ALLOW_INSECURE_BIND=true`
- Stripe webhook: constant-time HMAC comparison
- KDF: upgraded from SHA-256 to HKDF with backwards-compatible legacy fallback
- Symlink attack on security remove_file/quarantine_file: canonicalize before prefix check
- Mail forward_to/catch_all: email format + CRLF + pipe injection validation
- SMTP test email: CRLF header injection prevention
- WordPress plugin/theme: slug validation (alphanumeric + hyphens only)
- Dashboard intelligence: scoped queries to authenticated user (cross-user leak)
- Backup paths: traversal validation on agent URL construction
- Migration: container name validation (DockPanel-managed only)
- Stack templates: random passwords generated at selection time
- Unix socket: permissions tightened from 0o660 to 0o600
- Raw `Command::new()`: replaced 3 instances with `safe_command` (env sanitization)
- `is_safe_relative_path`: now rejects backslashes and enforces length limit
- Compose volumes: long-form object syntax now validated (prevents docker.sock bypass)

## [2.6.9] - 2026-03-29

### Fixed
- 7 browser alert() calls replaced with in-page toast/message UI (SiteDetail, Logs, ResellerUsers, Extensions)
- panic!() on invalid TCP bind (agent) and JWT_SECRET validation (API) replaced with clean exit
- .unwrap() on server await replaced with error logging in agent and API main
- Terminal WebSocket resize handler now wrapped in try-catch
- Dashboard WebSocket cleanup race condition (handlers nulled before close)
- Metrics WebSocket sends explicit Close frame before disconnect
- 3 silent .ok() error discards replaced with tracing::warn logging
- Grafana Docker template default password changed from "admin" to required field
- Cleanup background task now supervised (auto-restarts on panic)
- BackupOrchestrator form typed with PolicyForm interface (replaces `any`)

### Added
- Alert type muting UI in Settings notification channels (suppress per-type from Slack/Discord/PagerDuty)
- Database password reset endpoint and UI (agent ALTER USER for PostgreSQL/MySQL/MariaDB)
- Secrets vault rename and description update with inline edit UI

## [2.6.8] - 2026-03-29

### Fixed
- Mail queue endpoint returns empty result when Postfix not installed (was causing 502 errors every 15s on dashboard)
- Onboarding widget template count updated from 34 to 151
- Real Vultr IP in test script examples replaced with RFC 5737 documentation IP
- Monitoring screenshot scrubbed of test.dockpanel.dev URL

### Added
- 17 fresh screenshots from live VPS for all major pages (dashboard, sites, Docker apps, terminal, security, etc.)

### Security
- 6 CRITICAL/HIGH findings fixed (command injection ×3, auth bypass, timing attack, systemd injection)
- 6 additional HIGH findings fixed (CDN SSRF, WebAuthn RP ID, IaC scope, SSH key injection, DB backup pattern)
- 15 MEDIUM/LOW findings fixed (CORS, rate limiting, input validation, error handling)
- CodeQL: bookmark URL validation hardened, DNS regex escaping fixed

## [2.6.7] - 2026-03-28

### Added — Tier 1 (High Impact)
- Nginx FastCGI cache per site with smart bypass (logged-in users, POST, admin)
- Cloudflare integration: zone settings, cache purge, security controls, SSL mode
- Wildcard SSL via DNS-01 challenge (Cloudflare TXT automation, multi-part TLD support)
- Container auto-update detection (registry digest comparison, update badges, one-click update)
- 50 new Docker app templates (101→151 across 14 categories: AI, Media, Productivity, Communication, etc.)
- Redis object cache per site (isolated DB numbers, WP auto-config via wp-cli)
- WAF: ModSecurity3 + OWASP CRS v4 (per-site detection/prevention mode, event viewer)

### Added — Tier 2 (Strong Differentiators)
- Zero-downtime PHP deploys (Capistrano-style atomic symlink swap, instant rollback)
- WordPress safe updates (pre-update snapshot, post-update health check, auto-rollback)
- Image optimization (server-side WebP/AVIF conversion per site)
- CDN integration (BunnyCDN + Cloudflare CDN, cache purge, bandwidth stats)
- Restic incremental backups (encrypted, deduplicated, snapshot management)
- Docker Compose editor validation (structured errors/warnings/info)
- Auto-optimization recommendations (PHP-FPM workers, nginx workers, disk usage)
- Cloudflare Tunnel (install cloudflared, token-based config, systemd service)

### Added — Tier 3
- CSP header management per site (policy editor + common presets)
- Bot protection per site (off/basic/strict modes)
- Passkey/WebAuthn passwordless login (manual p256+ciborium implementation, max 10 per user)
- Per-user container isolation policies (max containers, memory, CPU, network isolation, allowed images)
- Container auto-sleep / scale to zero (configurable idle threshold, auto-healer integration)
- Visual DB schema browser (tables, columns, indexes, foreign key relationships)
- Point-in-time DB recovery (WAL archiving for PostgreSQL, binlog retention for MySQL)
- GPU passthrough for Docker (NVIDIA Container Toolkit detection, --gpus flag)
- WHMCS billing integration (API config, webhook provisioning/suspension/termination)
- App migration between servers (migration records, progress tracking)
- Terraform/Pulumi IaC provider API (scoped tokens, resource listing)
- Horizontal auto-scaling (rule-based CPU thresholds, min/max replicas, cooldown)

### Added — Infrastructure
- Telemetry & diagnostics: local event collection, opt-in remote sending, PII stripping (19 patterns)
- Update checker: GitHub Releases API polling every 6h, dashboard banner, release notes display

### Fixed
- Agent token desync on fresh install — agent now prefers AGENT_TOKEN env var over file
- WebAuthn RP ID defaulted to "localhost" when BASE_URL unset — now derived from request Origin header
- Sidebar NavLink prefix matching: exact route matching on all layouts
- 5 unbounded SQL queries now have LIMIT 500 (webhook_endpoints, pending_users, servers, backup_policies, git_previews)
- Dependabot: picomatch 4.0.3→4.0.4, path-to-regexp 8.3.0→8.4.0 (website dependencies)

## [2.6.6] - 2026-03-27

### Fixed
- Dashboard fleet overview crash on fresh install (SQL column mismatch)
- Backup creation failure on GNU tar (`--no-dereference` flag)
- Installer: silent package install failures now warn instead of lying
- Installer: Docker volume cleanup prevents DB password mismatch on retry
- 59 silent .ok() failures in agent replaced with proper error handling
- 51 .ok().flatten() anti-patterns in backend replaced with error propagation
- System updates (apt upgrade) broken by API's ProtectSystem=strict — proxied through agent

### Added
- Uninstall routes for all 10 services (PHP, Certbot, UFW, Fail2Ban, PowerDNS, Redis, Node.js, Composer, mail server, PHP versions)
- SSL certificate renewal (certbot force-renewal) and deletion endpoints
- User suspend/unsuspend toggle with session invalidation
- Admin password reset for managed users
- System Health tab shows real data (API status, uptime, CPU/mem/disk)
- Certificates page: renew and delete buttons with confirmation
- Monitor list pagination (limit/offset)
- Backup retention auto-enforcement
- Terminal share token revocation
- 45+ command timeouts in agent (Docker, systemctl, apt, system commands)
- Notifications page link to alert channel configuration

## [2.6.5] - 2026-03-25

### Security
- **Research-driven security audit**: Studied CVEs from CyberPanel, HestiaCP, CloudPanel, VestaCP, Webmin, cPanel — then audited DockPanel against those attack patterns. 55 findings (12 HIGH, 28 MEDIUM, 15 LOW).
- **Command execution safety**: Added `safe_command()` module — `env_clear()` on all 341 `Command::new()` calls across 44 files. Prevents LD_PRELOAD/PATH hijacking.
- **Credential encryption at rest**: All stored credentials (DB passwords, SMTP, S3/SFTP, OAuth, TOTP, DKIM) encrypted with AES-256-GCM using dedicated key derivation.
- **Shell injection fix**: Rewrote database_backup.rs — piped `docker exec` + `gzip` instead of `bash -c` with interpolated strings.
- **Tar symlink attacks**: `--no-dereference` on backup creation, `--no-same-owner` on restore.
- **Session revocation**: `revoke_all_sessions` now actually works — auth middleware checks cached timestamp.
- **Deploy log IDOR**: Ownership verification on both git_deploys and docker_apps SSE streams.
- **Content Security Policy**: Added CSP header to frontend nginx config.
- **Docker exec denylist**: Added 7 escape-relevant commands (unshare, pivot_root, setns, capsh, mknod, debugfs, kexec).
- **Compose volume symlinks**: `canonicalize()` resolves symlinks before path validation.
- **nginx header inheritance**: Security headers re-declared in static asset location blocks.
- **WebSocket security**: Conditional upgrade (prevents h2c smuggling), `access_log off` on token-bearing WS locations.
- **S3 temp files**: RAII TempFileGuard with random names + 0600 permissions.
- **2FA validation**: Explicit HS256 + leeway=0 (was Validation::default()).
- **Account enumeration**: Registration returns generic response.
- **Git history scrubbed**: Removed all passwords, IPs, hostnames, sensitive screenshots from history via git-filter-repo.

## [2.6.1] - 2026-03-22

### Added (LOW Priority Gap Fixes)
- **Domain rename** — New `PUT /api/sites/{id}/domain` endpoint to rename a site's domain. Agent handler renames nginx config, site directory, SSL certs, log files, PHP-FPM pools, Fail2Ban jails, redirects, and htpasswd configs. Backend updates monitors, status page components, and logs activity
- **Auto-firewall for proxy ports** — Sites created with proxy/node/python runtime automatically get a UFW deny rule blocking external access to the allocated proxy port (traffic only allowed through nginx). Rule is auto-removed on site deletion
- **Laravel auto-migrations** — Site deploys for Laravel sites (`php_preset = "laravel"`) now auto-run `php artisan migrate --force` after successful deploy
- **One-time scheduled deploy** — New `POST /api/git-deploys/{id}/schedule` endpoint to schedule a deploy at a specific time. New `scheduled_deploy_at` column on `git_deploys`. Deploy scheduler checks for due one-time schedules every 60s and auto-clears after triggering. Cancel with `DELETE /api/git-deploys/{id}/schedule`
- **Change Docker app image** — New `PUT /api/apps/{container_id}/image` endpoint to change a running container's image tag. Pulls new image, stops old container, creates new one preserving volumes, rolls back on failure
- **Update Docker app resource limits** — New `PUT /api/apps/{container_id}/limits` endpoint to update CPU/memory limits on running containers via `docker update`. Accepts `memory_mb` and `cpu_percent`

## [2.6.0] - 2026-03-22

### Fixed (Automation Gap Audit — Priority 1)
- **Auto-SSL DB update** — Background SSL provisioning now updates `ssl_enabled`, `ssl_cert_path`, `ssl_key_path`, `ssl_expiry` in the database and activates paused monitors (was silently succeeding without DB update)
- **Auto-SSL config preservation** — SSL provisioning now passes `php_preset` and `root_path` to the agent, preventing custom nginx config from being wiped
- **Pre-deploy backup** — All deploy paths (site deploy, git deploy manual, git deploy webhook/scheduled) now create a site backup before deploying
- **Pre-delete backup** — Site deletion creates a final backup before CASCADE-deleting the site record
- **Site deletion cleanup** — Now removes orphaned `status_page_components` matching the deleted domain
- **Database restore** — New `POST /db-backups/{db_name}/restore/{filename}` agent endpoint + `POST /api/backup-orchestrator/db-backups/{id}/restore` API endpoint. Supports MySQL/MariaDB, PostgreSQL, and MongoDB restore from backup files
- **Dashboard health score** — Now factors in backup freshness (-5 per stale site), security scan findings (-10 critical, -3 warning), and open incidents (-10 each)
- **Smart recommendations** — Dashboard intelligence endpoint returns actionable recommendations: stale backups, security findings, open incidents, expiring SSL, firing alerts, diagnostic issues. Rendered as a new Recommendations panel on the dashboard
- **Alert escalation** — Unacknowledged firing alerts re-notify with `[ESCALATED]` prefix after 15 minutes, then every 30 minutes. New `escalated_at` column + migration
- **Alert-to-incident correlation** — Before creating a new incident from an alert, checks for existing active incidents within 5 minutes. Appends as incident update instead of creating duplicates
- **Auto-healer restart limit** — Tracks restart count per service over 30-minute window. After 3 failed restarts, stops healing, creates critical incident, sends notification, and marks state as `exhausted`
- **Disk-full forecast alerting** — Computes disk fill rate from metrics history; alerts when disk projected full within 48h (critical if <12h)
- **Memory leak trend detection** — Compares recent vs older memory averages; warns when sustained >10% increase with usage above 60%
- **Docker container crash detection** — New `check_container_health` in alert engine detects exited, crash-looping, and unhealthy containers
- **Docker container auto-restart** — Auto-healer restarts exited/dead Docker containers with same 3-attempt limit as system services
- **Incidents pause deploys** — All 5 deploy paths (manual site, webhook site, manual git, webhook git, scheduled git) check for active critical/major incidents before proceeding
- **Security scanner auto-fix** — Auto-renews expiring SSL certificates detected by security scans (safe findings only, never auto-deletes)
- **Fail2Ban auto-configuration** — New sites auto-get a Fail2Ban jail monitoring their access log; removed on site deletion
- **Session management** — New `user_sessions` table, `GET /api/auth/sessions` (list with is_current flag), `DELETE /api/auth/sessions/{id}` (revoke), auto-cleanup of expired sessions
- **Notification center** — Bell icon with unread badge in all 4 layouts. New `panel_notifications` table, 4 API endpoints (list, unread-count, mark-read, mark-all-read), `/notifications` page with severity colors. Alerts auto-insert into notification center. 30-day retention cleanup. SSE real-time delivery. Wired into 18 event sources (deploys, incidents, backups, security, SSL, auto-healer, sites, auth)

### Fixed (Automation Gap Audit — MEDIUM Priority, 25 gaps)
- **Clone site auto-provisioning** — Clone now triggers auto-backup schedule, secrets vault, status page component, and site.created event
- **Composite site health** — New `GET /api/sites/{id}/health-summary` combining SSL, backup freshness, uptime, and composite score
- **"Backup Everything" preset** — New `POST /api/backup-orchestrator/policies/protect-all` one-click policy
- **Backup creation retry** — Policy executor retries failed backups once with 5s delay
- **Backup freshness alerting** — Proactive notification when sites have no backup in 48+ hours (throttled to once/hour)
- **Volume restore endpoint** — New `POST /api/backup-orchestrator/volume-backups/{id}/restore`
- **Deploy lock** — Concurrent deploys to same site blocked (checks for active building/deploying status)
- **Response time alerting** — Monitors warn when response time exceeds 5000ms threshold
- **Failed cron detection** — Manual cron execution fires alert on non-zero exit code
- **Postmortem auto-populate** — Transitioning to postmortem status auto-generates timeline template
- **/tmp cleanup + Docker prune** — Auto-healer now cleans /tmp (7d) and runs Docker system prune on disk pressure
- **Oversized log rotation** — Truncates individual log files larger than 500MB during cleanup
- **Welcome email** — New users receive welcome email with panel URL and credentials prompt
- **Audit log IPs** — Security-sensitive actions (site create/delete, user create/delete, security fix) now log client IP
- **Auto-rollback on deploy failure** — Failed site deploys auto-restore from pre-deploy backup
- **Generic webhook notifications** — New `notify_webhook_url` in alert rules for custom integrations (Telegram, Teams, etc.)
- **Weekly digest email** — Monday morning summary with 7-day alert/backup/incident/deploy counts to all admins
- **Post-deploy cache invalidation** — Nginx cache purge after successful deploy (fastcgi + proxy cache)
- **Reseller branding** — `GET /api/branding` now returns per-reseller logo/colors/name when applicable
- **Unified event timeline** — New `GET /api/dashboard/timeline` merging deploys, backups, incidents, alerts, scans

## [2.5.2] - 2026-03-22

### Fixed (Theme & Layout Consistency Audit)
- **Clean-Dark rounding parity** — Added ~120 lines of structural overrides (cards, modals, tables, buttons, scrollbar, selection, focus rings, progress bars, code blocks) so Clean-Dark has round corners everywhere, matching Clean
- **Ember radius normalized** — `--radius-xl` and `--radius-2xl` were 2px smaller than all other themes; fixed to 16px/20px
- **Clean hardcoded border-radius → CSS variables** — All 11 instances of hardcoded `12px/8px/6px/4px` converted to `var(--radius-lg/md/sm/xs)` for theme consistency
- **Status dot glow per-theme** — Green glow was hardcoded for all themes; now uses theme-appropriate accent color (blue for Midnight/Clean-Dark, orange for Ember, teal for Arctic, blue for Clean)
- **Progress bar glow for Arctic & Clean** — Missing glow rules added for both light themes
- **Settings theme picker missing `data-color-scheme`** — Switching to light themes now correctly sets color scheme attribute
- **Default theme mismatch** — Settings.tsx fallback aligned to `midnight` (was `terminal`)
- **FOUC prevention** — Added inline script in index.html to apply theme before CSS loads
- **LayoutSwitcher light variant** — Replaced hardcoded `zinc/blue/white` colors with theme variables
- **2FA banner in all layouts** — Replaced `amber-*` (stock Tailwind) with `warn-*` (theme tokens)
- **NexusLayout logout hover** — `rose-400` replaced with `danger-400` theme token
- **PublicStatusPage full theme adoption** — 40+ hardcoded color references replaced with theme variables
- **Terminal.tsx** — `bg-gray-300` and `bg-red-500` replaced with theme tokens
- **Login.tsx** — Google OAuth button uses theme-mapped text/hover colors
- **Settings.tsx hardcoded colors** — 13 instances of `blue-500/red-500` replaced with `accent/danger` tokens
- **Dashboard stat grid square corners** — Added `rounded-lg overflow-hidden` to stat bar and system info grids; added explicit `rounded-lg` to metric cards, sparkline cards, onboarding section, and issues panels
- **Compact layout flat nav** — GlassLayout now respects `dp-flat-nav` setting (was only implemented in Sidebar layout)
- **Compact layout footer spacing** — Removed nested padding wrapper, aligned `px-3` to match Sidebar layout spacing
- **Layout switcher dropdown redesign** — Added `p-1` padding and `rounded-md` items to match panel dropdown style; compact mode hides label text to save space; removed bordered button style for cleaner ghost-button look

## [2.5.1] - 2026-03-22

### Fixed (Remaining 7 Gaps — Phase D)
- **GAP 7+21: Internal events bridge to webhook gateway** — `fire_event()` now also forwards events to webhook gateway routes with `filter_path=/event` and `filter_value={event_type}`. Users can subscribe gateway routes to any internal event.
- **GAP 12: Docker apps auto-get monitor + status component** — Docker apps deployed with a domain now auto-create an HTTP monitor and a status page component under "Docker Apps" group.
- **GAP 13: Git deploy auto-creates gateway endpoint** — New git deploys auto-create a webhook gateway endpoint for webhook inspection/replay capabilities.
- **GAP 16: Incident resolve cleans up alerts + components** — Resolving a managed incident auto-resolves linked alerts and clears status_override on affected status page components.
- **GAP 17: Vault export/import** — New `GET /api/secrets/vaults/{id}/export` and `POST /api/secrets/vaults/{id}/import` endpoints for encrypted vault backup and transfer between DockPanel instances.

### Automation Audit: Complete
All 21 identified gaps now addressed. Zero manual steps required for: backup scheduling, uptime monitoring, secret injection, incident creation, status page updates, or webhook delivery.

## [2.5.0] - 2026-03-22

### Fixed (21-Gap Automation Audit)
- **GAP 1: Backup policies now execute** — New `backup_policy_executor` background service runs every 60s, evaluates cron schedules, executes backup policies across sites, databases, and volumes. Policies are no longer dead config.
- **GAP 2: Verifier respects policy_id** — Backup verifier checks `verify_after_backup` flag. Policy executor triggers verification after successful backups.
- **GAP 3: Auto-incidents from monitoring** — When a monitor goes down, the system auto-creates a managed incident with timeline, links affected status page components, and auto-resolves when the monitor recovers.
- **GAP 4: Auto status page components** — New sites automatically get a status page component (if status page is enabled).
- **GAP 5: Auto-inject secrets on deploy** — After a successful deploy, the system checks for a linked vault with `auto_inject` secrets and injects them into the site's `.env` file automatically.
- **GAP 6: Auto-vault for new sites** — Every new site gets an auto-created secrets vault linked via `site_id`.
- **GAP 8: fire_event in all new features** — Backup orchestrator, incident management, and secrets manager now emit extension webhook events (`db_backup.created`, `incident.created`, `secrets.injected`, etc.).
- **GAP 9: Critical alerts create incidents** — Critical alerts and server offline/service down alerts auto-create managed incidents visible on the status page.
- **GAP 10: Backup failure creates incident** — When a backup policy has failures, a managed incident is auto-created.
- **GAP 14: Backup for ALL sites** — Removed the `site_count <= 1` gate. Every new site now gets a daily backup schedule automatically.
- **GAP 15: Auto-monitor with deferred activation** — New sites get a paused HTTP monitor that auto-activates after successful SSL provisioning (when DNS is confirmed working).
- **GAP 18: Webhook delivery cleanup** — Added 7-day retention cleanup for `webhook_deliveries` and 90-day for `backup_verifications` in the auto-healer retention cycle.
- **GAP 19: Subscribers notified of auto-downtime** — Status page subscribers now receive email notifications when monitors detect downtime, not just for manually-created incidents.
- **GAP 20: Policy encrypt flag works** — The backup policy executor passes the encrypt flag through to agent backup endpoints when `encrypt = TRUE`.

### Infrastructure
- New background service: `backup_policy_executor` (supervised, 60s interval) — 11th background service
- Modified: `uptime.rs` (auto-incidents + subscriber notifications), `alert_engine.rs` (critical→incident), `sites.rs` (auto-vault, auto-monitor, auto-component, backup for all), `ssl.rs` (activate monitors), `deploy.rs` (auto-inject secrets), `auto_healer.rs` (retention cleanup), `backup_orchestrator.rs` + `incidents.rs` + `secrets.rs` (fire_event calls)

## [2.4.0] - 2026-03-22

### Added
- **Webhook Gateway**: Receive, inspect, route, and replay incoming webhooks.
  - **Inbound endpoints**: Each gets a unique URL (`/api/webhooks/gateway/{token}`). Unlimited endpoints per user.
  - **Signature verification**: HMAC-SHA256 and HMAC-SHA1 modes for GitHub, Stripe, and other providers. Configurable header name and secret.
  - **Request inspector**: Full request logging — headers, body, source IP, signature validation status. Click any delivery to view complete details.
  - **Route builder**: Forward incoming webhooks to any destination URL. JSON path filtering (e.g., only forward `action=push`). Custom header injection. Configurable retry (0-10 attempts with exponential backoff).
  - **Replay**: Re-send any past delivery to all configured routes. Useful for debugging or recovery.
  - **Delivery tracking**: Per-route forwarding status, response body, duration. Endpoint-level counters.
  - **E2E test suite**: `tests/webhook-gateway-e2e.sh` — endpoint CRUD, webhook receive, delivery inspection, routes, replay, filtering.

### Infrastructure
- New crate dependency: `sha1 0.10` for HMAC-SHA1 signature verification.
- New migration: `webhook_endpoints`, `webhook_deliveries`, `webhook_routes` tables.
- 8 new API endpoints (7 admin, 1 public inbound).
- Frontend: `WebhookGateway.tsx` with 3 tabs (Endpoints, Request Inspector, Routes).

## [2.3.0] - 2026-03-22

### Added
- **Secrets Manager**: AES-256-GCM encrypted secret storage with version history.
  - **Secret vaults**: Project-scoped vaults for organizing secrets (global or per-site).
  - **Encrypted storage**: All secret values encrypted with AES-256-GCM (random nonce per secret, key derived from JWT_SECRET via SHA-256).
  - **Secret types**: Environment variables, API keys, passwords, certificates, custom — with type-specific UI badges.
  - **Version history**: Every update creates a versioned snapshot. Full audit trail with who changed what and when.
  - **Auto-inject**: Mark secrets for automatic injection into site `.env` files on deploy. One-click inject from vault to site.
  - **Masked by default**: API returns masked values (`xxxx••••••••`) unless `?reveal=true` is explicitly requested.
  - **Pull endpoint**: `GET /api/secrets/vaults/{id}/pull` returns all secrets as decrypted key-value pairs (for CLI integration).
  - **Vault sidebar UI**: Split-pane layout with vault list on left, secrets table on right. Create/edit/delete with inline forms.
  - **E2E test suite**: `tests/secrets-manager-e2e.sh` — vault CRUD, secret CRUD, encryption roundtrip, version history, pull.

### Infrastructure
- New crate dependencies: `aes-gcm 0.10`, `base64 0.22` for AES-256-GCM encryption.
- New service: `secrets_crypto.rs` — encrypt/decrypt with nonce+ciphertext format, unit tests included.
- New migration: `secret_vaults`, `secrets`, `secret_versions` tables.
- 8 new API endpoints under `/api/secrets/`.
- Frontend: `SecretsManager.tsx` with vault browser, reveal toggle, version history panel.

## [2.2.0] - 2026-03-22

### Added
- **Incident Management**: Full incident lifecycle with real-time status updates.
  - **Managed incidents**: Create, track, and resolve incidents with status lifecycle (investigating → identified → monitoring → resolved → postmortem).
  - **Incident severity**: Minor, major, critical, and maintenance classifications.
  - **Incident timeline**: Post updates with status changes and messages. Full audit trail with author emails and timestamps.
  - **Postmortem support**: Attach post-incident analysis with publish control.
  - **Affected components**: Link incidents to status page components for targeted impact reporting.
- **Enhanced Status Page**: Production-grade public status page replacing the basic monitor list.
  - **Status page configuration**: Customizable title, description, logo URL, accent color, history display settings.
  - **Component groups**: Organize monitors into logical service components (e.g., "API Server", "Website") with grouping.
  - **Overall status indicator**: Automatically computed from component health (operational/degraded/major outage).
  - **Incident history**: Shows active incidents with full timeline, plus resolved incidents within configurable history window.
  - **Auto-detected downtime**: Legacy monitor-based incidents also displayed for complete visibility.
  - **Email subscribers**: Public subscribe/unsubscribe for incident notifications. Verified subscribers receive updates on status changes.
  - **Standalone public page**: Dark-themed, no-auth status page at `/status` with responsive layout.
- **Admin UI**: New "Incidents" page in Operations nav with 3 tabs (Incidents, Components, Settings).
- **11 new API endpoints**: Incidents CRUD + updates, status page config, components CRUD, subscribers, enhanced public endpoint.
- **E2E test suite**: `tests/incident-management-e2e.sh` covering full incident lifecycle, components, public page, subscribers.

### Infrastructure
- New migration: `status_page_config`, `status_page_components`, `status_page_component_monitors`, `managed_incidents`, `managed_incident_components`, `incident_updates`, `status_page_subscribers` tables.
- Frontend: `IncidentManagement.tsx` (admin), `PublicStatusPage.tsx` (public standalone).

## [2.1.0] - 2026-03-22

### Added
- **Backup Orchestrator**: New centralized backup management system for databases, Docker volumes, and sites.
  - **Database backups**: MySQL/MariaDB (`mysqldump`), PostgreSQL (`pg_dump`), and MongoDB (`mongodump`) dump + restore via Docker exec. Compressed with gzip.
  - **Docker volume backups**: Back up any Docker volume to `.tar.gz` using a temporary Alpine container. Restore volumes with one click.
  - **Encryption at rest**: Optional AES-256-CBC encryption (PBKDF2, 100k iterations) for all backup types via OpenSSL. Encrypted files get `.enc` suffix, originals are auto-deleted.
  - **Automatic restore verification**: Verify backups by spinning up temporary database containers and restoring dumps, or extracting archives to temp directories. Checks file integrity, table counts, and entry points.
  - **Backup policies**: Cross-resource policies with cron scheduling, destination selection, retention count, encryption toggle, and auto-verification.
  - **Backup health dashboard**: Global overview with total counts, storage usage, 24h success/failure rates, active policies, verification stats, and stale backup warnings.
  - **Background verifier**: Supervised service running every 6 hours that automatically verifies unverified backups and fires alerts on failures.
  - **B2 and GCS destinations**: Backblaze B2 and Google Cloud Storage now supported as backup destinations (S3-compatible API).
  - **CLI commands**: `dockpanel backup db-create`, `db-list`, `vol-create`, `vol-list`, `verify`, `health` — full backup management from the command line.
  - **E2E test suite**: Dedicated backup orchestrator test script (`tests/backup-orchestrator-e2e.sh`) covering health, policies CRUD, database backup lifecycle with verification.
- **Nav item**: "Backups" in Operations section links to the new Backup Orchestrator page.

### Infrastructure
- New migration: `backup_policies`, `database_backups`, `volume_backups`, `backup_verifications` tables.
- Extended `backup_destinations` with `encryption_enabled`, `encryption_key` columns, and B2/GCS dtype support.
- Agent: 4 new services (`database_backup`, `volume_backup`, `encryption`, `backup_verify`) + 3 new route modules.
- Backend: `backup_orchestrator` routes (11 endpoints), `backup_verifier` supervised background service.
- Frontend: `BackupOrchestrator.tsx` page with 5 tabs (Overview, Policies, DB Backups, Volume Backups, Verifications).

## [2.0.6] - 2026-03-21

### Fixed
- **Nexus themes decoupled from layout**: Nexus and Nexus Dark themes were previously locked to the Nexus layout only. They are now independent color themes that work with any layout (Terminal, Glass, Atlas, Nexus). Theme cycling (Ctrl+K) and Settings picker now include all 6 themes.

### Improved
- **Premium card depth**: Dark theme cards (Terminal, Midnight, Ember, Nexus Dark) now have subtle box shadows creating layered depth instead of flat rectangles.
- **Progress bar polish**: All progress bars now have rounded ends and a subtle accent-colored glow per theme (green/blue/orange).
- **Bolder status indicators**: Status dots (online/offline/warning) are larger (10px) with colored glow halos for better visibility on dense pages.
- **Theme picker expanded**: Settings appearance panel now shows all 6 themes (was 4) with accurate mini-previews including Nexus Dark and Nexus Light.
- **Layout switcher description**: Nexus layout description updated to "Modern SaaS, flat nav" (was "Light, clean SaaS" which was misleading since dark themes now work with it).

## [2.0.5] - 2026-03-21

### Added
- **Nexus Dark theme**: Premium dark mode for the Nexus layout with sun/moon toggle. GitHub Dark-inspired three-layer depth palette, Inter font, rounded corners, blue accent. Persists across sessions.
- **Sidebar group labels**: Navigation groups (Reseller, Operations, Admin) now display small uppercase labels in the Command layout sidebar.
- **Glass sidebar tooltips**: Native browser tooltips show nav item names when the Glass layout sidebar is collapsed.
- **Card elevation system**: Three elevation levels (`.elevation-1/2/3`), `.card-interactive` hover effects, `.hover-lift` card animations. Applied to dashboard cards, sites table, mail service cards, app templates, server/monitor items.
- **Page header system**: Sticky `page-header` bar with title, subtitle, and action buttons. Applied to 13 pages (Dashboard, Sites, Databases, Apps, Security, Settings, Servers, Mail, Monitoring, DNS, Users, Git Deploy, Alerts).
- **Login background gradient**: Subtle radial gradient that adapts per theme (green/blue/teal/orange).
- **Modal portal system**: `dp-modal` / `dp-modal-overlay` CSS classes for Nexus-compatible modal styling across 15 modals in 6 pages.

### Improved
- **Button color hierarchy**: Only primary CTAs (Create Site, Run Scan, Add Record) stay green. All secondary/utility buttons (Customize, Restart Nginx, Export, Refresh, etc.) use neutral gray — breaks the green monotone across 6 pages, ~25 buttons.
- **Dynamic progress bar colors**: CPU/Memory/Disk bars change from green (<70%) → amber (70-90%) → red (>90%). Disk uses 80/90 thresholds. Rounded ends with smooth 500ms transitions.
- **Dashboard visual hierarchy**: Metric cards with elevation, 24h chart fade-in animation, staggered stat grid, collapsible onboarding wizard (auto-collapses after 3+ steps, persists to localStorage).
- **Sidebar footer redesign**: User avatar circle with initial, hover-reveal logout button, descriptive health status ("Connected"/"Disconnected" replaces "OK"/"!"). Applied to both Command and Glass layouts.
- **Typography for non-terminal themes**: Midnight and Ember now remove uppercase/tracking like Nexus. All 5 sans-serif themes get 15px body text for better Inter readability.
- **Security card grid**: Changed from 5-column with orphan card to balanced 3-column grid with equal `min-h-[140px]` heights.
- **Table hover states**: `table-row-hover` class added to Security, DNS, and Users table rows with theme-aware hover colors.
- **Onboarding wizard**: Completed steps show a solid green circle with white checkmark. Collapsible with compact "Setup: X/5 complete" view.
- **Ember theme contrast**: Lightened surfaces and brightened orange accent for better text readability.
- **Atlas layout nav**: Added `shrink-0` to nav items so they scroll horizontally instead of compressing.
- **Richer empty states**: Sites, Databases, Git Deploys, Monitors, and Crons pages show contextual feature descriptions instead of bare "No X yet" text.
- **Login page**: Removed bulky "Made with Rust" gear icon, replaced with minimal "Powered by Rust" text. Card shadows added.

### Fixed
- **Theme switching: Nexus→Terminal white screen**: Switching from Nexus layout to any other layout left `dp-theme=nexus` (white) active, rendering a white Terminal layout. Fixed with `dp-pre-nexus-theme` save/restore in LayoutSwitcher, NexusLayout, useLayoutState, and main.tsx IIFE.
- **Nexus modal clipping**: Modals in Nexus layout were clipped by `overflow-hidden` on the main wrapper, hiding the top fields. Fixed with `createPortal` to render at `document.body`.
- **Nexus modal contrast**: Modal cards in Nexus light had the same `#f9fafb` background as the page (invisible). Fixed with `dp-modal` class providing white background, strong shadow, and proper text colors.
- **Page header spacing**: Added `margin-bottom: 1.25rem` to `.page-header` for consistent spacing between header and content.
- **Nexus light theme: tinted selection buttons**: Migration source cards, Settings proxy selector, and all `bg-rust-500/10`-style toggle buttons were rendering as solid blue blobs. Fixed with properly unescaped selectors.
- **Nexus light theme: accent toggle visibility**: `bg-accent-500/15` toggles now render with readable blue tint and text.

## [2.0.4] - 2026-03-20

### Security
- **CORS lockdown**: Deny all cross-origin requests by default. Same-origin panel UI is unaffected. Previously defaulted to `AllowOrigin::any()` which allowed CSRF from any website.
- **Constant-time token comparison**: Agent auth middleware now uses `subtle::ConstantTimeEq` to prevent timing attacks on token validation.
- **Token hashing in database**: Agent tokens stored as SHA-256 hashes in `agent_token_hash` column. DB dump no longer exposes plaintext tokens for inbound auth.
- **Token rotation**: New `POST /auth/rotate-token` on agent + `POST /api/servers/{id}/rotate-token` on API. 60-second grace period for old token during rotation. Updates `api.env` on disk for persistence.
- **Secure cookie fix**: `BASE_URL` defaulted to `https://panel.example.com`, causing `Secure` flag on cookies over HTTP. Fixed — defaults to empty, setup script sets from domain.
- **jsonwebtoken upgraded 9 → 10.3.0**: Fixes type confusion vulnerability that could lead to authorization bypass.
- **serde_yml replaced with serde_yaml_ng**: `serde_yml` and `libyml` are unsound/unmaintained. Replaced with `serde_yaml_ng` v0.10.0.

### Fixed
- **Cascade cron cleanup**: Deleting a site now removes cron entries from the system crontab. Previously, DB records were cleaned via CASCADE but crontab entries were orphaned.
- **UFW port gap**: Setup script now adds panel ports (80, 443, 8443) to UFW even when the firewall is pre-existing. Previously skipped port rules if UFW was already installed.
- **Token rotation API→agent desync**: Rotating the agent token now updates the API's in-memory `AgentClient` token AND writes to `api.env` on disk. Previously left the API with the old token, breaking all agent communication.

### Added
- **CI pipeline** (`.github/workflows/ci.yml`): Rust clippy, frontend type check, build verification, unit tests, `cargo-audit` + `npm audit` security scanning. Runs on every push to main and PRs.
- **E2E test suite** (`tests/e2e.sh`): 62 tests across 27 categories — full CRUD lifecycle, security edge cases, zero-leftover cleanup. Run: `bash tests/e2e.sh <host> [port]`.
- **Deep E2E test suite** (`tests/deep-e2e.sh`): 51 tests for advanced features — WordPress install, backup restore, git deploy, reseller system, file operations, compose stacks, concurrent operations, extensions API.
- **29 unit tests**: Config parsing (BASE_URL defaults, Secure flag logic), token hashing, input validation (domains, names, container IDs, path traversal, pagination).
- **API reference** (`docs/api-reference.md`): 648 lines documenting all 371 endpoints with request bodies and examples.
- **Competitor comparison** (`COMPARISON.md`): Honest comparison vs HestiaCP, CloudPanel, RunCloud, CyberPanel, Ploi.
- **README overhaul**: Dashboard screenshot, comparison table, collapsible screenshot gallery, cleaner structure.
- **FUNDING.yml**: PayPal sponsor link (paypal.me/ovexro).

### Verified
- **Reboot recovery**: All services start automatically after server reboot. 62/62 E2E tests pass post-reboot.
- **Fresh install E2E**: Full install via `INSTALL_FROM_RELEASE=1` on clean Ubuntu 24.04 VPS — all features operational.

## [2.0.3] - 2026-03-20

### Added
- **Documentation site** at `docs.dockpanel.dev`: mdBook-generated, 8 pages (getting-started, troubleshooting, CLI reference, WordPress, Git deploy, email, multi-server, backups). 1855 lines.

### Changed
- **Docker app templates pinned**: 33 of 39 `:latest` tags replaced with specific major versions (e.g., `redis:7`, `ghost:5`, `grafana/grafana:11`). 6 kept at `:latest` due to non-standard versioning (minio, nocodb, etc.).
- **Auto-monitors removed**: Sites no longer auto-create uptime monitors on creation. Users create monitors manually when DNS is configured.

### Added — Documentation
- **8 documentation pages** at `docs/`: getting-started, troubleshooting, CLI reference, and 5 guides (WordPress, Git deploy, email, multi-server, backups). 1855 lines of practical, copy-paste-friendly docs.

### Fixed — Fresh Install E2E (real clean VPS test)
- **Local server not registered after setup**: API returned 503 on all requests after admin creation. Added `ensure_local_server()` call in the setup endpoint.
- **Site docroot missing /public/ subdirectory**: Agent created `/var/www/{domain}/` but nginx expected `/var/www/{domain}/public/`. Fixed to create the correct subdirectory.
- **Backup tar flag incompatibility**: Replaced `--no-dereference` with `-h` (POSIX-compatible).

### Fixed — Comprehensive Audit (57 findings across 7 audit types)

#### Critical
- **Migration ordering**: `whitelabel_oauth` migration was running before `reseller_system` (ALTERing a table before it existed). Renumbered to `20260320050000`.
- **OAuth bypasses 2FA**: OAuth login issued full session without checking `totp_enabled`. Now redirects to 2FA challenge when enabled.
- **Setup script missing build tools**: Fresh VPS source builds failed — added `build-essential cmake pkg-config` installation.
- **No swap on x86_64 low-RAM VPS**: Swap creation only triggered on ARM. Now applies to all architectures when building from source.
- **install-agent.sh wrong env vars**: Remote agents never entered phone-home mode (`AGENT_TOKEN` vs `DOCKPANEL_SERVER_TOKEN`). Fixed to write both sets.
- **Systemd services never updated during upgrade**: `update.sh` now rewrites service files with current `ReadWritePaths` and hardening.
- **Required directories not created during upgrade**: `update.sh` now creates `/etc/postfix`, `/var/vmail`, and other directories needed by new features.

#### High
- **UFW blocks panel port 8443**: IP-based installs now open the configured panel port in UFW.
- **ExecStartPost hardcodes www-data**: Agent socket `chgrp` now auto-detects nginx group (`www-data` or `nginx`).
- **`read` prompt broken in curl-pipe-bash**: Domain prompt now reads from `/dev/tty` when stdin is piped.
- **Frontend path mismatch after upgrade**: `update.sh` now fixes nginx root path when switching between source and release modes.
- **config.rs default LISTEN_ADDR was 0.0.0.0:3000**: Changed to `127.0.0.1:3080` to match all scripts and nginx config.
- **uninstall.sh incomplete cleanup**: Now removes CLI binary, tmpfiles.d, crontab entries, `/var/www/acme`, `/var/lib/dockpanel`.
- **Stacks INSERT missing server_id**: Docker Compose stacks now include `server_id` in INSERT.
- **Staging site INSERT missing server_id**: Staging environments now inherit parent site's server_id.
- **No domain uniqueness across sites + git_deploys**: Cross-table domain conflict check prevents silent hijacking.
- **Blue-green deploy dropped resource limits**: New container now inherits `memory`/`cpu_period`/`cpu_quota` from config.
- **Git preview port has no unique constraint**: Added `UNIQUE INDEX` on `git_previews(host_port)`.
- **Site proxy_port has no unique constraint**: Added partial `UNIQUE INDEX` on `sites(proxy_port)`.
- **No terminal session limit**: Added `AtomicU32` counter with max 20 concurrent PTY sessions.

### Added
- **CONTRIBUTING.md**: Development setup, architecture overview, code style, PR process.
- **GitHub issue templates**: Bug report and feature request forms with structured fields.
- **GitHub PR template**: Checklist for builds, tests, and changelog.

### Changed
- **README.md**: Added badges (license, release, build), doc links, contributing section, phone-home disclosure.
- **.gitignore**: Added SSL material, database file patterns.

### Fixed — Adversarial Security Pentest
- **Rate limit bypass via X-Forwarded-For**: Login rate limiter now uses `X-Real-IP` (set by nginx, not forgeable) instead of `X-Forwarded-For`.
- **SSRF filter bypass in extensions**: Webhook URL validation replaced string-matching with DNS resolution + `is_loopback()`/`is_private()`/`is_link_local()` checks. Blocks hex IPs, decimal IPs, IPv6 loopback, DNS-to-localhost, cloud metadata.
- **Nginx version disclosure**: Added `server_tokens off` to nginx config.

### Fixed — Disaster Recovery
- **Agent fails after every reboot**: Removed `ReadWritePaths` and `PrivateTmp=yes` from agent systemd service (redundant with `ProtectSystem=no`, and caused NAMESPACE errors for missing dirs). Added `ExecStartPre` to create `/run/dockpanel`.
- **Health endpoint false "ok"**: `/api/health` now checks DB connectivity, returns `"degraded"` when database is unreachable.
- **StartLimitIntervalSec in wrong section**: Moved from `[Service]` to `[Unit]` in all 3 scripts.

### Fixed — UX Walkthrough (fresh VPS testing)
- **Secure cookie over HTTP**: Login cookie conditionally sets `Secure` flag based on `BASE_URL` scheme. `SameSite` changed from `Strict` to `Lax` (Strict blocked OAuth redirects).
- **Site document root not created**: Agent now creates `/var/www/{domain}/public/` with a default `index.html` during site provisioning.
- **PHP site without PHP check**: Agent validates PHP-FPM socket exists before writing PHP nginx config. Returns clear error with install instructions.

### Fixed — Supply Chain
- **`serde_yaml` archived**: Replaced with `serde_yml` in agent and CLI (serde_yaml maintainer archived the crate in 2024).
- **MailHog abandoned**: Replaced `mailhog/mailhog` template with `axllent/mailpit` (MailHog last updated 2020).
- **Stale build templates**: Updated `rust:1.82-slim` → `rust:1.94-slim`, `golang:1.23-alpine` → `golang:1.24-alpine`.

### Fixed — Code Quality
- **Cloudflare auth header deduplication**: 5 inline blocks → shared `helpers::cf_headers()`.
- **Server IP detection deduplication**: 6 inline blocks → shared `helpers::detect_public_ip()`.
- **Agent semaphore split**: Long-running ops (Docker builds) use separate 5-permit semaphore, quick requests keep 20.
- **Extension webhook rate limiting**: Max 20 concurrent deliveries with atomic counter.
- **DB pool acquire timeout**: 5-second timeout prevents indefinite blocking.
- **Uptime monitor N+1 query**: Maintenance window check batched into single query.

## [2.0.2] - 2026-03-20

### Changed
- **Version alignment**: All Cargo.toml and package.json versions bumped to 2.0.2 (were 0.1.0/1.0.0). API health endpoint and CLI --version now report correct version.
- **Binary size claims**: Marketing site, README, and FAQ updated from "~20MB" (agent-only) to "~35MB" (total of agent + API + CLI) for honest comparison.
- **Template count**: FAQ corrected from 53 to 54 app templates.
- **OS support**: Hero section now includes Rocky Linux 9+ alongside other supported distros.

### Fixed
- **install-agent.sh binary naming**: Was downloading `dockpanel-agent-x86_64` / `dockpanel-agent-aarch64` but GitHub Releases publishes `dockpanel-agent-linux-amd64` / `dockpanel-agent-linux-arm64`. Fixed to match release naming.
- **install-agent.sh apt-get hardcoding**: Now detects package manager (apt/dnf/yum) instead of hardcoding apt-get. CentOS, Rocky, Fedora, and Amazon Linux now supported for remote agent installs.
- **install-agent.sh server-id persistence**: `--server-id` was accepted but never written to config. Now persisted to `/etc/dockpanel/api.env` as `SERVER_ID`.
- **install-agent.sh tmpfiles.d**: Added `/run/dockpanel` tmpfiles.d entry so socket directory survives reboots.
- **install-agent.sh systemd hardening**: Remote agent service now matches local agent hardening (MemoryMax, LimitNOFILE, PrivateTmp, ProtectKernelLogs/Modules).
- **update.sh pre-built binary path**: Added `INSTALL_FROM_RELEASE=1` support so ARM users who installed via release binaries can update without Rust toolchain.
- **update.sh redundant health check**: Removed duplicate wait-for-health loop after rollback-capable check.

## [2.0.0] - 2026-03-19

### Added — High-Impact Features
- **Multi-Server Management**: Manage unlimited remote servers from one panel. AgentRegistry dispatches to local (Unix socket) or remote (HTTPS) agents. Server selector in sidebar, test connection, install script for remote agents. ServerScope extractor with user ownership verification on every request.
- **Reseller / Multi-Tenant Accounts**: Admin → Reseller → User hierarchy. Reseller quotas (max users/sites/databases), server allocation, per-reseller branding (logo, colors, hide DockPanel name). Quota enforcement on site/database creation with counter sync.
- **Nixpacks Auto-Detection**: Build any app without a Dockerfile using Nixpacks (30+ languages). Dynamic version resolution from GitHub releases. Deploy pipeline: try Nixpacks → fall back to auto-detect (6 langs) → docker build. Build method tracked per deploy.
- **Preview Environments**: TTL-based auto-cleanup of preview deployments. Branch deletion webhook auto-removes previews. Configurable preview_ttl_hours per deploy. Background cleanup service (5-minute interval).
- **Migration Wizard**: Import sites, databases, and email from cPanel, Plesk, or HestiaCP. 4-step wizard: select source → analyze backup (auto-detect domains, DBs, mail) → select items → SSE-streamed import. cPanel full parser, Plesk/HestiaCP beta stubs.
- **WordPress Toolkit**: Multi-site WP dashboard with parallel detection. Vulnerability scanning against 14 known exploited plugins. Security hardening (7 checks, 6 auto-fixable via wp-cli). Bulk update plugins/themes/core across selected sites.
- **White-Label Branding**: Public `/api/branding` endpoint. Per-reseller logo_url, accent_color, panel_name, hide_branding. BrandingContext provider applies to sidebar + login page. Dynamic accent color via CSS variable.
- **OAuth / SSO Login**: Google, GitHub, GitLab via OAuth 2.0 authorization code flow. CSRF state tokens (10-minute expiry). GitHub private email fallback. Auto-create users on first OAuth login (configurable). Provider-colored login buttons.
- **Traefik Reverse Proxy**: Alternative to nginx for Docker app routing. Traefik v3.3 as Docker container with auto-SSL (Let's Encrypt ACME). File-based dynamic route configs with auto-watch. Install/uninstall/status management. Settings toggle in admin panel.
- **Plugin / Extension API**: Webhook-based integrations with HMAC-SHA256 signed event delivery. Extension CRUD with `dpx_` API keys and `whsec_` webhook secrets. Event types: site/backup/deploy/app/auth/ssl. Delivery log with status tracking. Secret rotation. SSRF protection on webhook URLs.

### Added — Feature Gap Analysis Enhancements
- **SQL Browser**: Built-in query editor for PostgreSQL and MariaDB with schema viewer
- **Node.js + Python Site Runtimes**: Managed systemd services with auto-port allocation
- **Docker Compose Stacks**: Full stack lifecycle (deploy, start, stop, restart, update, remove)
- **Blue-Green Zero-Downtime Deploy**: Docker app updates with traffic swap and rollback
- **Git Push-to-Deploy Pipeline**: Clone → build → deploy with webhook triggers and rollback
- **Container Health Checks**: Docker health status (healthy/unhealthy/starting) in Apps view
- **Container Logs Viewer**: Search, filter, auto-refresh, color-coded log levels
- **Command Palette (Ctrl+K)**: Global search across all panel pages
- **One-Click App Updates**: Pull latest image, preserve config, recreate container
- **34 App Templates**: Database, CMS, monitoring, analytics, tools, dev, storage, media, networking, security
- **Getting Started Wizard**: 5-step onboarding checklist

### Changed
- **Architecture**: Single-agent → multi-agent (AgentRegistry, AgentHandle enum, RemoteAgentClient)
- **Auth**: Added ResellerUser extractor, ServerScope with ownership verification
- **Database**: 8 new tables, server_id FK on all resource tables, reseller profiles, extensions, migrations
- **Frontend**: BrandingContext, ServerContext providers. 8 new pages (Servers, ResellerDashboard, ResellerUsers, Migration, WordPressToolkit, Extensions, plus per-site WP and Git Deploy enhancements)
- **Rust Edition**: 2024 (Rust 1.94)

### Security
- ServerScope verifies `server.user_id == claims.sub` on every request (prevents cross-user server access)
- OAuth: SameSite=Strict cookies, error callback handling, empty oauth_id validation, no auto-link to password accounts
- Extension API: SSRF protection (blocks private IPs, metadata endpoints), HMAC bypass fix, webhook secret rotation
- Migration wizard: command injection fix (direct docker args), path traversal validation, TAR --no-same-owner
- WordPress: domain path validation, targeted chown (not recursive), site path fallback
- Nixpacks: build_context path traversal validation, dynamic version resolution
- Traefik: ACME directory permissions (0700), network cleanup on uninstall
- Branding: logo_url validated (HTTP(S) only), accent_color validated (hex/rgb/hsl only)
- Reseller: quota enforcement wired up, server isolation for reseller users, counter sync on create/delete
- Preview: TTL reset on redeploy, MAKE_INTERVAL for PostgreSQL safety, cleanup error logging

### Fixed
- 100+ findings from 9 comprehensive audits across all features
- server_id filtering added to git_deploys, stacks, databases, dashboard, alerts list endpoints
- Compose deployments now correctly set build_method='compose'
- Preview cleanup query uses MAKE_INTERVAL instead of string concat
- fire_event() wired into site/backup/app handlers (was dead code)
- Traefik Docker app integration (was install-only with no functional routing)
- Frontend SecurityItem type mismatch in WordPress Toolkit fixed
- OAuth parameter mismatch (doc_root vs source_dir) in migration wizard fixed

## [1.1.0] - 2026-03-15

### Added
- **Email Management**: Full mail server with one-click install (Postfix + Dovecot + OpenDKIM). Domains, mailboxes, aliases, catch-all, quotas, autoresponders, DKIM signing, DNS helper (MX/SPF/DKIM/DMARC), mail queue viewer
- **PowerDNS**: Self-hosted DNS alongside Cloudflare. Provider selector, zone creation, record CRUD, setup guide
- **One-Click CMS Install**: WordPress, Drupal, Joomla — create site + database + install + SSL in one click from Sites page
- **Historical Charts**: SVG sparkline charts (CPU/Memory/Disk 24h) with background metrics collector (60s interval, 7-day retention)
- **Light Theme**: CSS variable overrides, sun/moon toggle in sidebar footer, localStorage persistence
- **One-Click Service Installers**: PHP-FPM, Certbot, UFW, Fail2Ban — install from Settings page
- **Smart Port Opener**: Port recognition (28+ ports), safety categories (safe/caution/blocked), quick presets (Web/Mail/Database)
- **SSH Key Management**: List/add/remove authorized keys with SHA256 fingerprints
- **Auto-Updates**: Toggle for unattended-upgrades security patches
- **Panel IP Whitelist**: Restrict panel access to specific IPs
- **Auto-SSL**: Automatic Let's Encrypt provisioning on site creation
- **Webhook Testing**: Test Slack/Discord webhooks from Settings
- **File Upload**: Base64 binary upload with path traversal protection
- **Webmail Template**: Roundcube one-click deploy from Docker Apps
- **Spam Filter Template**: Rspamd one-click deploy from Docker Apps
- **BUILD STABLE Badge**: Build status indicator in sidebar footer

### Changed
- **Harmonized Color Palette**: Green/amber/red at identical saturation/lightness (anchored at #22c55e). Custom `warn-*` and `danger-*` CSS scales. Zero stale emerald/amber/yellow references
- **Dashboard Redesign**: Bar metrics with centered text-5xl numbers (replaced ring gauges), neutral white numbers + gray progress bars (color only for warnings/critical), system info grid (replaced neofetch style)
- **Sidebar Overhaul**: Flat nav (no progressive disclosure), white active state with blinking _ cursor, 19px icons, spacing-only groups
- **Terminal Frame**: Unified bordered container (header + canvas in single frame)
- **Mobile Responsive**: Card layouts for Activity, Users, DNS records. Logs toolbar wrapping. Monitors polish
- **Contrast**: All text-dark-400 bumped to text-dark-300 globally (36 instances, 14 files) for WCAG compliance
- **Animations**: Page fade-up, stagger children, counting numbers, typewriter welcome, hover-lift. Respects prefers-reduced-motion
- **Login Page**: Logo updated to match sidebar brand
- **Apps/Sites Separation**: WordPress/Drupal/Joomla moved from Docker Apps to native PHP in Sites. 32 Docker templates remain for services and tools
- **502 Error UX**: "Agent offline" message with `systemctl restart` command instead of cryptic "Request failed (502)"
- **Security Score**: Prominence increase, singular/plural grammar fix
- **Apps Empty State**: Error message with icon when templates fail to load

### Fixed
- **Diagnostics**: Agent nginx -t check distinguishes [warn] from [emerg]/[error] — no false critical on cosmetic warnings
- **Document Root False Positives**: Changed ProtectHome=yes → read-only so agent can see /home/* directories
- **Agent Socket Persistence**: Added tmpfiles.d config + /run/nginx.pid to ReadWritePaths
- **Agent Permissions**: NoNewPrivileges=no, ReadWritePaths for mail/apt/etc paths — enables package installation
- **CUPS Disabled**: Removed unnecessary print service

### Security
- Setup script auto-installs UFW + Fail2Ban with default rules
- Smart firewall blocks dangerous ports (Telnet, NetBIOS, SMB, MSSQL)
- All cookie flags verified: HttpOnly, Secure, SameSite=Strict, Max-Age=7200

### Infrastructure
- Metrics collector background service (60s interval, 7-day retention)
- Mail config sync to Postfix/Dovecot via atomic file writes
- DKIM key generation via openssl RSA 2048-bit
- Setup script installs PHP, Certbot, UFW, Fail2Ban out of the box

## [1.0.0] - 2026-03-14

### Added
- **Core Panel**: Site management (static, PHP, proxy), database management (PostgreSQL, MariaDB), SSL (Let's Encrypt), file manager, web terminal, backups
- **Docker Apps**: 50+ one-click templates across 10 categories + Docker Compose import
- **CLI**: Full command-line interface — status, sites, db, apps, ssl, backup, logs, security, diagnose, export, apply
- **Infrastructure as Code**: YAML export/import of server configuration
- **Smart Diagnostics**: Pattern-based issue detection across 6 categories with one-click fixes
- **Auto-Healing**: Automatic restart of crashed services, log cleanup on full disk, SSL renewal
- **Alerting System**: 5 alert types (CPU/memory/disk thresholds, server offline, SSL expiry, service health, backup failure) with email, Slack, Discord notifications
- **2FA/TOTP**: Full two-factor authentication with QR setup and recovery codes
- **Dashboard Intelligence**: Health score (0-100), top active issues, SSL expiry countdowns
- **Docker Resource Limits**: Memory and CPU limits on container deploy
- **Container Management**: Health checks, logs viewer, environment viewer, one-click updates
- **Security**: Firewall management, Fail2Ban, SSH hardening, security scanning with scoring
- **DNS Management**: Cloudflare DNS zone management with full record CRUD
- **Git Deploy**: Webhook-triggered deployments from Git repos
- **Staging Environments**: Create staging copies, sync from production, push to live
- **Uptime Monitoring**: HTTP checks with configurable intervals and incident tracking
- **Teams**: Multi-user access with roles and team-based permissions
- **Activity Log**: Full audit trail of all admin actions
- **Multi-Server**: Manage unlimited servers from a single dashboard
- **ARM64 Support**: Pre-built binaries for Raspberry Pi and ARM64 servers
- **Auto Reverse Proxy**: Domain + SSL auto-configured when deploying Docker apps
- **Command Palette**: Ctrl+K global search across all panel pages
- **Notification Channels**: Email toggle, Slack/Discord webhook configuration
- **Custom Nginx Directives**: Per-site textarea for advanced nginx config
- **Onboarding Wizard**: 5-step getting started checklist for new users

### Security
- JWT auth with HttpOnly cookies + Bearer header support
- Token blacklist for logout with periodic cleanup
- Argon2 password hashing
- Rate limiting on login, 2FA, webhooks, and agent endpoints
- Systemd hardening (NoNewPrivileges, ProtectSystem, MemoryMax)
- Nginx rate limiting (30r/s on API)
- 12 CHECK constraints on database status/type fields
- Atomic nginx config writes (tmp+rename)

### Infrastructure
- Supervised background tasks with auto-restart on panic
- Statement timeout on all database pool connections (30s)
- Agent request timeout (60s)
- DB backup cron (daily, 7-day retention)
- Docker prune cron (weekly)
