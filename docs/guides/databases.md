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

## Importing a dump you already have

**Databases → Import** loads a `.sql.gz` you put on the server yourself. It replaces
data in the target database, so it asks before it runs.

**It is administrator-only**, and that is a security boundary rather than a policy
preference. The dump directory is named after the database, and a database name can be
reused: deleting a database removes its container but not its dumps, so a directory can
outlive the database that filled it. Restricting the door to administrators means a
tenant who creates a database with a previously-used name cannot read what the previous
holder left behind. It costs nothing in practice — placing a file in that directory needs
root on the server, and an administrator can already read `/var/backups` directly.

The panel does not accept the dump through the browser, and that is deliberate
rather than an omission. An upload travels base64-encoded inside a JSON request
body capped at 2 MiB, which works out to about 1.5 MB of file — see
[Getting files onto a site](file-uploads.md). Almost no real dump fits inside
that, so the panel uses the same shape the Migration wizard has always used:
**you place the file, the panel takes its name.**

Copy it into the database's own backup directory, over SSH, as root:

```bash
mkdir -p /var/backups/dockpanel/databases/mydb
scp mydb.sql.gz root@server:/var/backups/dockpanel/databases/mydb/
chmod 600 /var/backups/dockpanel/databases/mydb/mydb.sql.gz
```

Then open **Databases → Import** and pick it from the list.

A few things that are easy to get wrong, and what the panel does about each:

- **The directory may not exist yet.** It is created the first time a database is
  backed up, so for a database that has never been backed up you have to
  `mkdir -p` it. It is root-owned and re-tightened to `0700` every time the agent
  starts, so a file placed there by a non-root account will not be readable.
- **`/tmp` will not work.** The agent runs with a private `/tmp` and cannot see
  the host's — the same constraint the Migration wizard documents.
- **The dump must be gzipped.** A SQL database imports `.sql.gz` and nothing else,
  because the restore path streams the file through `gunzip` straight into the
  database container. A plain `.sql` is **listed anyway**, greyed out, with the exact
  `gzip` command that fixes it — it is not silently ignored, which is what used to
  happen.
- **Encrypted panel backups are not imported here.** A `.enc` file in that
  directory is one the panel took and encrypted with a key derived from its own
  secret, so **Backup Manager** is the door that can restore it. Import lists them
  and says so rather than offering a button that could only fail.
- **`.archive.gz` is MongoDB's format.** On a PostgreSQL or MariaDB database it is
  listed but not offered, because feeding it to `psql` fails deep inside a
  decompression pipe rather than at the door.
- **A name with a space in it will not work.** `scp "my dump.sql.gz" …` produces one,
  and backup filenames may contain letters, digits, dash, underscore and dot only. Such
  a file is listed with the exact `mv` command that renames it.
- **Do not import a file that is still being copied.** `scp` writes to the final name
  as it goes, so a half-copied dump looks complete in the listing. Wait for the copy to
  finish before you press Import.
- **A large import can outlast the page.** The panel stops waiting after 270
  seconds, because its own request timeout is 300. If that happens it says so and
  tells you the import is still running — it does **not** report a failure, and
  you should not start it again. Watch the tables to see it finish. Behind
  Cloudflare's proxy the cut comes earlier, at 100 seconds; the panel recognises that
  case too and says the same thing.

The import streams the file into the container and never buffers it in memory, so
size is a matter of time rather than RAM.

If the import itself fails — a truncated dump, a dump for a different engine, SQL the
database rejects — you get the database's own error message, not an incident reference.
That is worth stating because it was not true of the restore path before v2.138.0.

The same door is available on the command line:

```bash
dockpanel backup db-list mydb        # shows importable dumps, and rejected ones with the reason
```

## What is not built

Being explicit, so you do not go looking:

- **No database GUI that works against a managed database from a container.**
  Covered above; this is a design decision, not a gap.
- **Managed databases belong to sites, not to Docker apps.** If you want a
  standalone database for a container to use, run it in the app's own compose
  stack.
- **No cross-server database access.** A database is reachable on the machine
  that runs it.
