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

For private repositories, DockPanel needs SSH access.

1. Generate a deploy key on the server:
   ```bash
   ssh-keygen -t ed25519 -C "dockpanel-deploy" -f /tmp/deploy-key -N ""
   ```

2. Add the public key to your repository:
   - **GitHub**: Repository > Settings > Deploy keys > Add deploy key
   - **GitLab**: Repository > Settings > Repository > Deploy keys
   ```bash
   cat /tmp/deploy-key.pub
   ```

3. Use the SSH repository URL when creating the Git Deploy:
   ```
   git@github.com:you/your-private-app.git
   ```

4. Move the private key where the agent can access it:
   ```bash
   sudo mkdir -p /etc/dockpanel/deploy-keys
   sudo mv /tmp/deploy-key /etc/dockpanel/deploy-keys/your-app
   sudo chmod 600 /etc/dockpanel/deploy-keys/your-app
   ```

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
