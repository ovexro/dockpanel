# Databases

DockPanel runs each managed database in its own Docker container, deliberately
isolated from everything else on the box. That isolation is the reason most
questions about databases have surprising answers, so this guide leads with it.

## The one thing to know first

**A managed database is reachable from the server itself, and from nothing else.**

Two mechanisms put it there, and they are both on purpose:

- The container's port is published on the host's **loopback address only**, so
  nothing off-box can reach it.
- The container sits on a bridge network with **container-to-container traffic
  switched off**, so nothing in another container can reach it either.

The practical consequence catches almost everyone once: **a database GUI running
in its own container cannot connect to a managed database.** Not with the
internal host, not with `127.0.0.1`, not with any value you can type. Inside that
GUI's container, `127.0.0.1` is the GUI itself.

This is not a misconfiguration and it is not something to work around lightly. It
is what stops a compromised container from reaching another tenant's data.

## Engines

| Engine | Image | Shown in the panel as |
|--------|-------|-----------------------|
| PostgreSQL | `postgres:16` | PostgreSQL 16 |
| MariaDB | `mariadb:11` | MariaDB 11 |
| MySQL | `mariadb:11` | — |

Choosing **MySQL** gives you **MariaDB**. They are wire-compatible for ordinary
application use, but if you depend on a MySQL-specific feature, know that you are
talking to MariaDB.

Redis and MongoDB are **not** managed database engines. They exist only as
one-click app templates, which means none of this guide applies to them — no
managed credentials, no SQL browser, no automatic cleanup.

## Reading the credentials panel

Open **Databases**, pick a database, and open its credentials. Six fields, and
two of them mean less than they look like they mean.

- **Host** — always `127.0.0.1`. Works from a shell on the machine running the
  database container. Not from another container, not from off-box.
- **Port** — the **host-published** port, allocated per database. It is not the
  in-container port, so it is not 3306 or 5432. MariaDB and MySQL databases are
  allocated from 3307 upward and PostgreSQL from 5433 upward.
- **Database** and **Username** — both are the database's name.
- **Password** — masked on screen, copied in clear text by the copy button, and
  also embedded in the connection string shown above the fields. Treat that
  connection string as a secret.
- **Internal Host** — `dockpanel-db-<name>`, which is the **Docker container's
  name**. Use it with `docker exec`, `docker logs`, or the DockPanel CLI. It is
  not a hostname you can connect to from anywhere.

## Connecting to a database

**From a site on the same server** — use `127.0.0.1` and the published port. This
is the supported path and the one the WordPress installer uses. A site's PHP,
Node or Python process runs on the host, not in a container, so the loopback
publish is reachable to it.

**From your own machine** — there is no direct route, by design. Tunnel over SSH:

```bash
ssh -L 5433:127.0.0.1:<port> root@your-server
# then point your local client at 127.0.0.1:5433
```

**From a container** — not possible for a managed database. If you need a
containerised tool to reach a database, run that database **inside the same
compose stack** as the tool. Services in one stack share a network and resolve
each other by service name; that is a different arrangement from a managed
database and it is the right one for that job.

### Adminer, phpMyAdmin, and the other GUI templates

The app catalogue includes database GUIs. They work — but **not against a managed
database**, for the reasons at the top of this guide. Pointing one at a managed
database will fail no matter what you type.

Use them for a database you run inside a compose stack. For managed databases,
use the built-in SQL browser below, or a client over an SSH tunnel.

## The built-in SQL browser

**Databases → your database** gives you a table browser, a schema view and a
query runner. It reaches the database by running the client *inside* the database
container, which is why it works when a networked client would not.

Its limits are real and worth knowing before you rely on it:

| Limit | Value | What happens when you hit it |
|-------|-------|------------------------------|
| Rows returned | 1000 | Result is truncated and labelled as truncated |
| Query time | 15 seconds | Query is cancelled |
| Output size | 5 MB | **Query fails** — it does not truncate |
| Query length | 10 KB | Rejected before running |
| Paging | none | Table browsing is a fixed first 100 rows |

The output cap is the one that surprises people: a wide `SELECT *` can fail
outright rather than returning the first 1000 rows. Select the columns you need.

**The query runner is not read-only.** You are connected as the database's owner,
so `UPDATE`, `DELETE` and `DROP` all execute. There is no confirmation step.

Access is by **site ownership**, not by role: whoever owns the site owns its
databases. An administrator who does not own the site does not get access to its
SQL browser through this route.

## Backups

Managed databases are included in the backup system, and a database can be backed
up on its own from **Backup Manager**. Restoring replaces the database's contents.

## What is not built

Being explicit, so you do not go looking:

- **No database GUI that works against a managed database from a container.**
  Covered above; this is a design decision, not a gap.
- **Managed databases belong to sites, not to Docker apps.** If you want a
  standalone database for a container to use, run it in the app's own compose
  stack.
- **No cross-server database access.** A database is reachable on the machine
  that runs it.
