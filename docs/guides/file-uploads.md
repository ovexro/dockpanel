# Getting files onto a site

The panel's file manager is for editing a config file, not for moving a website.
This guide says plainly what it can and cannot do, and what to use instead.

## The short version

| You want to | Use |
|-------------|-----|
| Fix a line in `wp-config.php` | The file manager |
| Upload a handful of small files | The file manager |
| Move thousands of files | `rsync` or `scp` over the server's own SSH |
| Move a whole site from another panel | The Migration wizard |
| Let a customer upload their own site | **Not available yet** — see the end |

## The file manager's limit is 1.5 MB per file

The number is worth explaining, because it is derived rather than chosen.

An upload is base64-encoded and sent inside a JSON request. The request body is
capped at 2 MiB, and base64 costs a third in overhead, so **the file itself can
be about 1.5 MB** (2 MiB × ¾ ≈ 1,572,864 bytes, published and enforced as
1,500,000 to leave room for the path and filename, which travel in the same
body). The same 2 MiB cap applies again on the hop between the panel and the
agent, so both would have to change together.

**You are told before you spend the upload.** Since v2.137.0 the browser checks
the size first and names the limit, and if a request does reach the server
oversize the server answers with the same sentence instead of an unreadable
`413`. Two larger numbers used to sit in the code — 100 MB in the panel, 50 MB
in the agent — and neither could ever fire, so the panel advertised a limit
forty times the one it enforced and then refused in silence. That was
[issue #121](https://github.com/ovexro/dockpanel/issues/121); both numbers now
read 1.5 MB.

## Other upload limits, and which is which

Four different numbers govern four different things. Confusing them is easy:

| Limit | Applies to | Typical value |
|-------|-----------|---------------|
| ~1.5 MB | Uploading through the panel's **file manager** | fixed |
| 2 MB exactly | **Opening** a file in the panel's editor | fixed |
| `max_upload_mb` | What **visitors** can upload to the site (PHP + nginx) | 64 MB |
| nginx body size | The panel's own front door | 100 MB on a standard install |

(The public demo is the exception: it fronts the panel with a 10 MB nginx body
limit, so a demo upload can be refused by nginx before any of the above applies.)

So a visitor uploading media to a WordPress site gets 64 MB, while the operator
using the file manager gets about 1.5 MB. That asymmetry is not intentional
design so much as an accident of how the file manager posts data, but it is the
current behaviour.

**The editor has one more sharp edge**: the 2 MB read check measures the file on
disk, while saving pays JSON escaping on top of the same 2 MiB envelope. A
quote-heavy file just under 2 MB can open and then fail to save.

## There is no archive extraction

The file manager cannot unzip or untar. It lists, reads, writes, creates,
renames, deletes, downloads and uploads — that is the whole set. Uploading a
`.zip` gets you a `.zip` sitting on disk.

An administrator with a server shell can extract normally. A site-scoped shell
is more restricted and should not be relied on for this.

## Moving a real site: use rsync

For anything beyond a few files, copy over the server's own SSH:

```bash
# from the machine holding the files
rsync -avz --progress ./site-files/  root@your-server:/var/www/example.com/public/

# then hand ownership to the web server (Debian/Ubuntu)
chown -R www-data:www-data /var/www/example.com
```

### Put the files in the right directory

This trips people up, so check it before copying:

| Runtime | Document root |
|---------|---------------|
| Static, PHP, WordPress, Laravel, Symfony | `/var/www/<domain>/public` |
| Magento | `/var/www/<domain>/pub` |
| Reverse proxy, Node, Python | `/var/www/<domain>` |

The site *directory* is always `/var/www/<domain>`, but for most runtimes the web
server serves the `public` subdirectory inside it. Copying a WordPress install to
`/var/www/example.com/` rather than `/var/www/example.com/public/` puts every file
one level above the document root, and the site will not load.

**The panel does not display the document root on the site's page.** Use the table
above. (The web terminal does print the site directory when you open it.)

## Coming from another control panel

The **Migration** wizard imports from cPanel, Plesk and HestiaCP, and moves
**sites and databases**. It is administrator-only.

**Mail accounts are not migrated.** For cPanel archives the wizard lists the mail
accounts it found so you know what to recreate, and for Plesk and HestiaCP it does
not even list them. Either way, mailboxes have to be recreated by hand in the Mail
section.

## Per-site SFTP does not exist yet

Today there are two ways into a site's files: the file manager, with the limits
above, and shell access. Both mean the operator does the work.

There is no per-site SFTP account, and no per-site system user to hang one on —
every site's files are owned by the web server user. Giving a customer scoped
upload access is a decided direction rather than an open question, and it is the
most-requested missing piece, but it is not built and this guide will not pretend
otherwise.

If you are running sites for other people today, the honest answer is that file
upload is still your job.
