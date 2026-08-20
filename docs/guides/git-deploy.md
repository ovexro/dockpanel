# Git Deploy Guide

## Overview

Git Deploy lets you push code to a Git repository and have DockPanel automatically build and deploy it. The pipeline supports:

- **Webhook-triggered deploys** from GitHub, GitLab, or any Git provider
- **Nixpacks auto-detection** for 30+ languages (no Dockerfile needed)
- **Blue-green zero-downtime deployments** with automatic traffic switching
- **Preview environments** with TTL-based auto-cleanup
- **Rollback** to any previous deployment

## Create a Git Deployment

### From the Panel

1. Go to **Git Deploys** in the sidebar
2. Click **New Deploy**
3. Fill in the form:
   - **Repository URL**: `https://github.com/you/your-app.git` (or SSH URL for private repos)
   - **Branch**: `main`
   - **Port**: The port your app listens on (e.g., `3000`)
   - **Domain** (optional): `app.example.com` for auto reverse proxy + SSL
4. Click **Create**

DockPanel will clone the repository, detect the build method, build a Docker image, and start the container.

## Webhook Setup

Webhooks trigger automatic deploys when you push to your repository.

### GitHub

1. In DockPanel, open your Git Deploy and copy the **Webhook URL** (shown after creation)
2. In GitHub, go to your repository > **Settings** > **Webhooks** > **Add webhook**
3. Configure:
   - **Payload URL**: Paste the webhook URL from DockPanel
   - **Content type**: `application/json`
   - **Secret**: Leave empty (DockPanel validates by repository URL)
   - **Events**: Select "Just the push event"
4. Click **Add webhook**

### GitLab

1. Copy the webhook URL from DockPanel
2. In GitLab, go to your project > **Settings** > **Webhooks**
3. Configure:
   - **URL**: Paste the webhook URL
   - **Trigger**: Push events
   - **Branch filter**: `main` (or your deploy branch)
4. Click **Add webhook**

Now every push to the configured branch triggers a build and deploy.

## Deploy Keys (Private Repositories)

Private repositories are cloned over **SSH**, with a deploy key the panel
generates for you. Generate it from the panel — a key you place on the server by
hand is not used, because the only thing that records where a deploy's key lives
is the **Generate Deploy Key** button.

1. Create the Git Deploy with the **SSH** repository URL:
   ```
   git@github.com:you/your-private-app.git
   ```
   The first deploy will fail to clone until step 3 — that is expected.

2. Open the deploy and click **Generate Deploy Key**. The panel creates the
   keypair on the server that hosts the deploy, stores the private key where the
   agent can read it, and shows you the public half.

3. Add that public key to your repository:
   - **GitHub**: Repository → Settings → Deploy keys → Add deploy key
   - **GitLab**: Repository → Settings → Repository → Deploy keys

4. Click **Deploy** (or push, if the webhook is set up). The clone now succeeds.

Clicking **Generate Deploy Key** again replaces the key, so the old public key
stops working and has to be removed from the provider and re-added.

### Cloning a private repository over HTTPS is not supported

An `https://` URL to a private repository fails with

```
fatal: could not read Username for 'https://github.com': terminal prompts disabled
```

The agent runs git non-interactively and has no credential store, so there is no
password prompt to answer. Use the SSH URL and a deploy key as above.

> **Do not put a token in the repository URL.**
> `https://TOKEN@github.com/you/app.git` clones, which is exactly what makes it
> tempting — but the URL is stored as you typed it, so the token ends up in the
> panel's database, in `.git/config` on the server, in git's own error output,
> and in every pre-update snapshot archive. It also keeps working after you
> rotate the token in the panel, because the checkout on disk still has the old
> one. The panel masks credentials wherever it prints a repository URL, which
> limits who can read it but does not un-store it. A deploy key is scoped to one
> repository, does not expire, and is what this path is built for.

### What the GitHub Token field is for

The **GitHub Token** on the deploy form is **not** a clone credential. It is used
only to post commit statuses back to GitHub after a deploy, so a commit shows a
green tick. Leaving it empty changes nothing about cloning.

## Git Deploys have no persistent storage

**A Git Deploy container has no volumes and no bind mounts, and every deploy
replaces the container. Anything the application writes to its own filesystem —
uploaded files, generated documents, a SQLite database, a cache — is gone on the
next deploy.**

Nothing warns you at the time. The first deploy works, the app writes files, and
the loss happens on the *second* deploy, which is usually much later and looks
unrelated. Design around it from the start:

- **A database** — create one under **Databases** and connect to it with an
  environment variable. This is the right home for anything relational.
- **Object storage** — S3, MinIO or similar for user uploads and generated
  files. This also survives moving the app to another server.
- **Deploy it as a Docker App instead** — Docker Apps *do* bind declared volumes.
  You give up the webhook, preview-environment and rollback features that make
  Git Deploy worth using, so this is a trade rather than a workaround.

Two things that look like solutions and are not:

- **`VOLUME` in the Dockerfile** gives each new container a fresh anonymous
  volume, so nothing carries over.
- **Mounting something by hand with `docker run`** is undone by the next deploy,
  which recreates the container without it — it works until it doesn't.

> **Do not add a `docker-compose.yml` to an existing Git Deploy to get volumes.**
> The presence of a compose file switches the deploy onto a different code path
> that does not read the Domain field, does not write an nginx vhost, does not
> issue a certificate, and does not remove the container already running. Your
> domain would go on serving the previous build indefinitely while the panel
> reported the deploy as successful. It also skips any compose service that
> builds from source rather than naming an image — which is exactly what a repo
> with a Dockerfile will have — and permanently disables blue-green deploys,
> previews and rollback for that deploy.

Volumes on Git Deploys are tracked as unbuilt work rather than declined; see the
issue tracker.

## Nixpacks Auto-Detection

DockPanel uses [Nixpacks](https://nixpacks.com) to automatically detect your app's language and build it into an optimized Docker image -- no Dockerfile required.

Supported languages include: Node.js, Python, Go, Rust, Ruby, PHP, Java, .NET, Elixir, Haskell, Crystal, Dart, Swift, Zig, and more (30+ total).

The build pipeline tries methods in this order:

1. **Nixpacks** -- Auto-detect language from project files (`package.json`, `requirements.txt`, `go.mod`, etc.)
2. **Auto-detect fallback** -- Built-in detection for 6 common languages
3. **Dockerfile** -- If a `Dockerfile` is present in the repository root

The build method used is tracked per deployment in the deploy history.

### Customizing the Build

If Nixpacks does not detect your app correctly, add a `Dockerfile` to your repository root and DockPanel will use it instead.

For Nixpacks-specific customization, add a `nixpacks.toml` to your repository:

```toml
[phases.setup]
nixPkgs = ["...", "ffmpeg"]

[phases.build]
cmds = ["npm run build"]

[start]
cmd = "npm start"
```

## Preview Environments

Preview environments let you deploy branches for testing before merging.

### How It Works

1. Create a Git Deploy for your main branch
2. Configure **preview_ttl_hours** (e.g., `72` for 3-day TTL)
3. Push to a feature branch and trigger the webhook
4. DockPanel creates a preview deployment on an auto-assigned port
5. Access it at `preview-branch-name.example.com` or via the assigned port
6. After the TTL expires, the preview is automatically cleaned up

### Branch Deletion Cleanup

When a branch is deleted in GitHub/GitLab, the webhook notification automatically removes the corresponding preview environment. No manual cleanup needed.

### TTL Reset

If you push new commits to a preview branch, the TTL timer resets. The preview stays alive as long as the branch is active.

## Deploy Protection (two-person rule)

Tick **Require another admin's approval before deploy** on a deployment and pressing
Deploy no longer builds anything. It files a request, and a *different* administrator has
to approve it.

1. The owner presses **Deploy**. The panel confirms that a request will be filed rather
   than a deploy started, and the other administrators are notified once.
2. The request appears under **Pending Approvals** at the top of the Git Deploy page,
   naming the deployment, the repository and branch, and who asked.
3. A second administrator presses **Approve** — which starts the build — or **Reject**,
   which is final; the requester has to ask again.

Notes worth knowing before you rely on it:

- **You cannot approve your own request.** On an install with only one administrator
  nobody can ever approve, so the owner gets a **Withdraw** button instead. That is the
  only way out — a deployment may have just one request waiting at a time.
- **It covers the Deploy button only.** Webhook deploys, scheduled (cron and one-time)
  deploys, and rollbacks all still deploy without approval. If you need those covered,
  turn them off on the deployment as well.
- **The deployment's owner can switch the setting off** — the administrator it constrains,
  and no other: editing a deployment is scoped to the account that owns it, so a second
  administrator cannot clear the flag on someone else's. Doing so cancels whatever was
  waiting, and is written to the security audit log, which cannot be edited or deleted.
  Treat it as a guard against acting alone by accident, not as a control over an
  administrator who does not want it.
- Approvals and rejections are kept as a record of who decided what, and survive the
  deletion of the account that made them.

## Rollback

Every deployment is tracked in the deploy history with a build hash and timestamp.

### From the Panel

1. Go to **Git Deploys** and open your deployment
2. Click the **History** tab
3. Find the version you want to roll back to
4. Click **Rollback**

DockPanel performs a blue-green rollback: it starts the old version in a new container, verifies the health check, and switches traffic -- the same zero-downtime process as a forward deploy.

## Deploy History

Each deploy records:

- **Commit hash** and message
- **Build method** (Nixpacks, Dockerfile, or auto-detect)
- **Build duration**
- **Deploy status** (success, failed, rolled back)
- **Timestamp**

View history from the panel or filter deploys in the activity log.
