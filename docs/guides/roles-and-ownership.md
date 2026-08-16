# Roles and Site Ownership

DockPanel has one ownership rule and it is deliberately simple: **every site
belongs to exactly one account.** There is no access-control list, no sharing,
and no second owner. Almost everything below follows from that.

## The roles

| Role | Can create sites | Sees | Intended for |
|------|------------------|------|--------------|
| `admin` | Yes | **Every site on a machine it runs, and can act on all of them** | You |
| `reseller` | Yes | Its own sub-accounts, and how many sites each holds | Selling hosting on |
| `user` | Yes | Only what it owns | An ordinary self-serve account |
| `client` | **No** | Only what it owns | Someone who manages domains you gave them |
| `suspended` | — | Nothing; every request is refused | Set by the Suspend action, not assigned directly |

`reseller` is the one role you do not assign from the **Users** screen, because it
is not only a role: it carries a profile holding that reseller's quotas, its panel
name and its branding, and the set of servers it may place sites on. Promote an
account in **Admin → Resellers**, which writes both halves. Until v2.118.0 the Users
dropdown offered it and wrote only the role, producing an account that was shown a
reseller menu whose pages then answered 404.

The `admin` row is the one that changed most recently, and it changed because the
promise came before the capability. For four days it described a reach the code
did not have; the reach now exists and is described under *Transfer hands over
ownership* below. A `reseller` still sees its sub-accounts and a site **count**
per account, not the sites themselves.

`client` is the newest and the only one that is a *restriction* rather than a
promotion. A client manages the sites it holds — PHP version, settings, files,
backups, SSL, and a shell inside each site it owns — and cannot bring a new
domain into service.

**One capability this sentence used to list is not a client's, and never was.**
*Containers*: every handler in `routes/docker_apps.rs` calls `require_admin`,
because a container on this box belongs to the box rather than to a site.
Corrected in v2.77.0 after an operator followed the list and found items missing
from the panel; the capability is what changed there, not the description — see
*Withdrawn Claims* in `FEATURES.md`.

*Mail* was in the same position until v2.102.0 and is no longer. A client now
manages the mailboxes and aliases of a mail domain **whose name matches a site it
owns on the same server** — see *Mail follows the site* below.

What a client's shell is, precisely: selecting one of your sites in **Terminal**
opens a session **inside that site's directory, as `www-data`**, under a
restricted shell with no privilege escalation. It is not a server shell — that one
is administrator-only, deliberately, and is the subject of the v2.75.0 security
fix. Before v2.77.0 the Terminal page dialled the *server* shell by default, so a
client met a refusal on a page they were in fact entitled to use.

## Giving a client a domain

1. **Users → New User**, role **Client**. (Or edit an existing account's role.)
2. Create the site yourself, as admin, in the normal way.
3. Open the site → **Transfer** → the client's email address.

The client can now sign in and manage that site. Repeat step 3 for each domain
they should hold.

### Transfer hands over ownership

**The account you transfer to becomes the owner, and the previous owner stops
being one.** Ownership is a single value, not a list. For an ordinary `user` or a
`client` that is the whole story: a transferred site leaves their Sites list and
its page stops answering them.

**For you it is different, because you run the box.** An administrator can open,
edit and delete any site on a machine they operate, whoever owns it. Ownership
still decides whose a site is — who sees it on their own Sites page, and who a
transfer moves it to — but it no longer decides what you are allowed to repair.

Two things bound that reach, and both are deliberate:

- **It stops at the hardware you operate** — this box, plus any server you
  registered yourself. It does not extend to a machine another administrator
  added.
- **Your own Sites page still lists only your own sites.** To see everybody's,
  Sites → tick **All sites on this server**, which shows every site on the box
  with its owner beside it, and a **Transfer** button on each row.

This is the second correction to this page in a week, and the direction reversed.
The first one removed a reach the role did not have. Then the operator who had
been using the feature pointed out that an administrator who cannot repair a
tenant's site is not much use on a server they are responsible for, which was the
better argument ([#51](https://github.com/ovexro/dockpanel/issues/51)).

What still does not exist is two *non-admin* accounts holding one domain at once.
If that is what you need, say so on
[the issue tracker](https://github.com/ovexro/dockpanel/issues) rather than
working around it — the workaround people reach for is a second account with a
shared password, and that removes the audit trail that makes the panel worth
having.

### What moves with the site

The `sites` row, **its staging environment if it has one**, and the four kinds of
record that keep their own copy of the owner beside the site: **alerts, monitors,
secret vaults, and WHMCS service mappings**. Everything else a site owns —
databases, cron jobs, backups, SSL certificates — is reached *through* the site,
so it follows automatically.

A staging environment is a second site in its own right, which is why it has to be
named here. Until v2.82.0 it did not move, and the consequence was not cosmetic:
the previous owner kept a shell inside a full copy of the new owner's files, and
kept the control that pushes that copy over the new owner's live site.

The whole transfer is one database transaction. It either happens completely or
not at all; there is no state where the site answers to the new owner while its
alerts still answer to the old one.

### What a client cannot do

Bring a new domain into service, by any route. That is enforced in the single
place every domain-introducing path goes through, so it holds for creating a
site, cloning one, adding an alias, creating a staging copy, deploying a git
repository to a new domain, deploying a Docker app with a domain, and creating a
Compose stack with one. A client attempting any of these gets:

> This account can manage the domains it holds but cannot bring a new one into
> service. Ask an administrator to create it and transfer it to you.

Renaming a domain the client already holds is **allowed** — that is managing a
site it owns, not creating a new one.

**With one exception: not onto a name whose mailboxes already exist.** A mail
domain holds its name the same way a site, a git deployment, a Compose stack or a
Docker app does, and the occupancy check now says so:

> Domain already in use by a mail domain. Ask an administrator to create the site
> and transfer it to you — claiming a name whose mailboxes already exist would
> hand you those mailboxes.

This refuses every non-administrator, not only clients, because a plain `user`
may create sites freely and would otherwise reach the same place by a different
door. An **administrator** is not refused: putting a site and its mail on the
same name is an ordinary arrangement, and "set the mail up first, add the website
after" has to keep working.

The reason the rule exists is worth stating plainly: mail is scoped by matching a
mail domain's name against the domain of a site the caller owns (GitHub #106,
shipped in v2.102.0). That makes the site's domain an authorisation key — and it
is a key the account being authorised can write. Without this rule, an account
could point a site it already owned at a name whose mailboxes existed and hand
itself every mailbox on it.

## Mail follows the site (v2.102.0)

A mail domain is managed by whoever owns the site of the same name **on the same
server**. That account can list the domain, read its DNS records, and create,
edit and delete its mailboxes and aliases — including setting passwords and
forwards. No new column and no new setting: ownership of the site *is* the grant,
so transferring a site transfers its mailboxes with it.

Three limits are worth knowing before you plan around them:

- **Same server.** `sites` is unique on `(domain, server_id)`, not on the domain
  alone, so the match carries the server too. **A domain whose website and
  mailboxes live on different hosts stays administrator-only** — the panel cannot
  tell that arrangement apart from two unrelated customers holding one name on two
  machines, and it refuses rather than guess.
- **One owner.** If two accounts hold different casings of the same name on one
  server — possible only on installs predating v2.52.0 — neither gets the mail.
  It fails closed.
- **The domain itself stays administrative.** Creating, renaming and deleting a
  mail domain, and setting its catch-all, remain administrator-only. A catch-all
  redirects an entire domain's mail, which is not a per-mailbox decision.

Creating a mail domain over a name a customer already holds a site on is how you
give them their mail — and because that single action hands over every mailbox and
password on it, the activity record now names the account it granted.

### Where they find it (v2.103.0)

v2.102.0 opened the endpoints; the panel showed nothing, so the grant was real but
invisible. **Mail** now appears in the sidebar for every role, and what the page
shows depends on what you own:

| | administrator | owner of the matching site |
|---|---|---|
| Domains, mailboxes, aliases, DNS records | all | their own |
| Add / delete a mail domain, catch-all, Verify DNS | yes | no |
| Mail server install, Rspamd, webmail, relay, blacklist, rate limit, TLS | yes | no |
| Queue and Logs tabs, mailbox backups | yes | no |

The entry is not restricted to the `client` role, because the grant is not either:
a plain `user` and a `reseller` reach a mail domain by exactly the same route if
they hold the matching site. An account that owns no matching domain sees the page
and an explanation of how mail arrives, rather than an invitation to create a
domain the panel would refuse it.

Being a client also means not being an admin, so every administrative surface —
users, servers, panel settings, updates, the firewall — refuses it, the same way
it refuses an ordinary `user`.

**The controls for what a client cannot do are no longer shown to it** (v2.82.0).
Create Site, Clone, Create Staging and Add Alias used to be rendered and then
refused, so the message above arrived only after the person had filled in a domain
and, for a WordPress site, a set of administrator credentials. The rule has not
changed; where you meet it has.

### What a client can see that it could not before

Two screens required an administrator over data that was already limited to the
caller's own rows, so the check decided who was refused rather than what was
returned (v2.82.0):

- **Monitoring → Certificates.** A client sees the expiry dates of its own sites'
  certificates. The dashboard tile had been reporting exactly these all along and
  linking to a page that answered "Admin access required".
- **Monitoring → Maintenance.** A client can schedule a maintenance window, which
  silences its own alerts while it works on its own site.

Still administrator-only, and correctly so: containers, DNS, CDN, the server
shell, and everything under the Admin group. Mail is now partly a client's — the
mailboxes on a domain it owns a site for; the mail domain itself is not.

## Every account owns its own security (v2.83.0)

**My Account** is the one navigation entry every role carries. It holds
two-factor enrolment, passkeys, password change, the list of devices signed in to
the account, API keys, and an export of everything the panel stores about you.

Until v2.83.0 all of it lived on the Settings page, which is administrator-only,
so a `client`, `user` or `reseller` had no entrance to any of it — while the
banner across the top of every page told them two-factor authentication was
required and linked them to that same page. None of it was ever a permissions
question: each of those endpoints was already restricted to the caller's own
rows. The screens simply had no door. The Settings tab administrators already
know renders the same components, so there is one implementation rather than two.

A non-administrator could always reset a password through the public *Forgot
password* form, which needs working SMTP and being signed out. Everything else
listed above was unreachable.

**If you lose your authenticator app**, sign in with one of the recovery codes
saved when you enrolled, then enter another recovery code in the *Disable 2FA*
field on My Account and enrol again. Before v2.83.0 that field took only a live
code from the app, so a lost device could sign in and never repair itself — there
is no administrator-side two-factor reset. Recovery codes are single use, and the
panel does not yet offer a way to generate a fresh set without disabling and
re-enrolling.

Alert channels are the one piece of per-account configuration still behind the
administrator door; the Notifications page no longer offers a link to it that a
client cannot follow.

## Suspending and restoring

Suspend is a separate action, not a role you assign. It records the account's
current role in `users.prior_role`, sets the role to `suspended`, and revokes its
sessions. Un-suspending gives back the recorded role — including `client`, so a
suspended client comes back a client rather than being quietly promoted to an
account that can create sites, and an administrator comes back an administrator.

Until v2.73.0 that record was kept in the same column as the password-reset
token, and asking for a password reset overwrote it. A suspended account could do
that itself from the public *Forgot password* form, so the un-suspend found
nothing to restore and fell back to `user` — promoting a `client` and demoting an
`admin`. The password-reset endpoints now refuse a suspended account, and the
record has a column nothing else writes.

**One case cannot be recovered, and un-suspending refuses rather than guessing.**
An account suspended before v2.73.0 whose record was already destroyed has no
previous role to give back — the activity log stores what a role became, never
what it was. Un-suspending one of those returns a conflict explaining exactly
that; set the role from the user editor, which also lifts the suspension. No
default is applied, in either direction: `user` is the value that caused the bug,
and `client` is not simply a smaller `user`, so there is no ordering of these
roles that makes a guess safe. Accounts suspended on v2.73.0 or later always have
a record and are unaffected.

The specific consequence this used to describe — an account handed `client`
staying visible to its reseller while becoming unmanageable by them — was fixed in
v2.82.0. A reseller may now act on every account its own table lists, whether that
account is a `user`, a `client`, or suspended. Administrators and other resellers
remain out of reach, by an allow-list rather than by an exclusion, so a role added
later does not silently become something a reseller may touch.

Billing-driven suspension through the WHMCS webhook follows the same rules,
including revoking the account's sessions — which it did not do before v2.73.0,
so a billing suspension left the account working until its token expired, up to
two hours later. Two differences remain, both deliberate: billing will not
*suspend* an `admin` or `reseller` (a lapsed invoice must not lock an operator
out of their own panel), and it will not *restore* one either — that direction
had no guard at all until v2.73.0, so the webhook secret alone could return an
operator role to an account the panel had suspended.

Both directions honour the **Auto-suspend** checkbox on Integrations → WHMCS.
Previously only suspension checked it, and none of the three WHMCS flags had a
control at all.

⚠ **Before v2.92.0 none of this could be exercised, because the integration could
not be configured at all.** The one statement that writes the WHMCS settings used
a conflict target no constraint on the table matched, and PostgreSQL resolves that
target when it parses the statement — so the save failed on the first attempt as
well as on re-saves, and the settings row could never exist. Every rule described
above was therefore unreachable on every install from 2026-03-28 until v2.92.0,
which supplies the missing constraint. A note in an earlier revision of this guide
described the flags being "reset to their defaults on every use"; that could not
have happened, since no save ever succeeded.

## A note on what Teams is not

The panel has `/api/teams` endpoints. **They grant no access to anything** —
nothing in the authorization path consults team membership, and there is no
Teams UI. If you are looking for multi-user access, the roles above are the
mechanism; Teams is not. See `FEATURES.md` §Withdrawn Claims.
