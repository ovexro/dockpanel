# Roles and Site Ownership

DockPanel has one ownership rule and it is deliberately simple: **every site
belongs to exactly one account.** There is no access-control list, no sharing,
and no second owner. Almost everything below follows from that.

## The roles

| Role | Can create sites | Sees | Intended for |
|------|------------------|------|--------------|
| `admin` | Yes | Everything on the panel | You |
| `reseller` | Yes | Its own sub-accounts and their sites | Selling hosting on |
| `user` | Yes | Only what it owns | An ordinary self-serve account |
| `client` | **No** | Only what it owns | Someone who manages domains you gave them |
| `suspended` | — | Nothing; every request is refused | Set by the Suspend action, not assigned directly |

`client` is the newest and the only one that is a *restriction* rather than a
promotion. A client manages the sites it holds — mail, PHP version, containers,
settings, files, backups, SSL — and cannot bring a new domain into service.

## Giving a client a domain

1. **Users → New User**, role **Client**. (Or edit an existing account's role.)
2. Create the site yourself, as admin, in the normal way.
3. Open the site → **Transfer** → the client's email address.

The client can now sign in and manage that site. Repeat step 3 for each domain
they should hold.

### Transfer is exclusive

**The account you transfer to becomes the owner, and the previous owner stops
being one.** This is a handover, not a share. You keep seeing the site because
you are an admin and admins see everything — but if you transfer a site from one
ordinary account to another, the first one loses it.

If what you want is two accounts managing one domain at the same time, DockPanel
does not do that today. Say so on
[the issue tracker](https://github.com/ovexro/dockpanel/issues) rather than
working around it — the workaround people reach for is a second account with a
shared password, and that removes the audit trail that makes the panel worth
having.

### What moves with the site

The `sites` row, and the four kinds of record that keep their own copy of the
owner beside the site: **alerts, monitors, secret vaults, and WHMCS service
mappings**. Everything else a site owns — databases, cron jobs, backups, SSL
certificates — is reached *through* the site, so it follows automatically.

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

## Suspending and restoring

Suspend is a separate action, not a role you assign. It stashes the account's
current role, sets it to `suspended`, and revokes its sessions. Un-suspending
restores the stashed role — including `client`, so a suspended client comes back
a client rather than being quietly promoted to an account that can create sites.

## A note on what Teams is not

The panel has `/api/teams` endpoints. **They grant no access to anything** —
nothing in the authorization path consults team membership, and there is no
Teams UI. If you are looking for multi-user access, the roles above are the
mechanism; Teams is not. See `FEATURES.md` §Withdrawn Claims.
