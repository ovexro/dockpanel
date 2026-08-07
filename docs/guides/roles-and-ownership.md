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

The `admin` row is the one that changed most recently, and it changed because the
promise came before the capability. For four days it described a reach the code
did not have; the reach now exists and is described under *Transfer hands over
ownership* below. A `reseller` still sees its sub-accounts and a site **count**
per account, not the sites themselves.

`client` is the newest and the only one that is a *restriction* rather than a
promotion. A client manages the sites it holds — PHP version, settings, files,
backups, SSL, and a shell inside each site it owns — and cannot bring a new
domain into service.

**Two capabilities this sentence used to list are not a client's, and never
were.** *Mail* is administrator-only end to end: every handler in `routes/mail.rs`
takes the `AdminUser` extractor and the Mail page redirects anybody else away, so
a client cannot add, edit or remove a mailbox on its own domain — an admin does it
for them. *Containers* likewise: every handler in `routes/docker_apps.rs` calls
`require_admin`, because a container on this box belongs to the box rather than to
a site. Corrected in v2.77.0 after an operator followed the list and found two of
its seven items missing from the panel; the capability is what changed here, not
the description — see *Withdrawn Claims* in `FEATURES.md`.

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

Still administrator-only, and correctly so: mail, containers, DNS, CDN, the server
shell, and everything under the Admin group.

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
control at all — the screen's own save omitted them, which silently reset them
to their defaults on every use.

## A note on what Teams is not

The panel has `/api/teams` endpoints. **They grant no access to anything** —
nothing in the authorization path consults team membership, and there is no
Teams UI. If you are looking for multi-user access, the roles above are the
mechanism; Teams is not. See `FEATURES.md` §Withdrawn Claims.
