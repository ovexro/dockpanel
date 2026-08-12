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

## The file manager's real limit is about 1.5 MB per file

Not 2 MB, and the number is worth explaining because the code contains several
larger numbers that do not apply.

An upload is base64-encoded and sent inside a JSON request. The request body is
capped at 2 MiB, and base64 costs a third in overhead, so **the file itself can be
about 1.5 MB** (2 MiB × ¾ ≈ 1,572,864 bytes). The same 2 MiB cap applies again on
the hop between the panel and the agent, so both would have to change together.

Two larger checks exist in the code — 100 MB in the panel, 50 MB in the agent —
and **neither is reachable**, because the body limit rejects the request first.
Do not go by them.

**There is no warning before you hit this.** The browser does not check the size,
and the failure reason is discarded: a batch reports only `N uploaded, M failed`,
with no indication that size was the problem. If uploads are failing and the
files are over ~1.5 MB, that is why.

## Other upload limits, and which is which

Four different numbers govern four different things. Confusing them is easy:

| Limit | Applies to | Typical value |
|-------|-----------|---------------|
| ~1.5 MB | Uploading through the panel's **file manager** | fixed |
| 2 MB exactly | **Opening** a file in the panel's editor | fixed |
| `max_upload_mb` | What **visitors** can upload to the site (PHP + nginx) | 64 MB |
| nginx body size | The panel's own front door | 100 MB on a standard install |

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
