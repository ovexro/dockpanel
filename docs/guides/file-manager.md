# File Manager

DockPanel includes a comprehensive, dual-pane File Manager designed for managing site files directly within the web panel without requiring an external SFTP client or shell access for everyday workflows.

---

## Overview & Architecture

The File Manager is available on every site under **Sites → [Site] → Files**. It provides a modern dual-pane desktop-grade file browsing experience with:
- **Left pane**: Collapsible directory tree navigation with lazy loading and instant folder selection.
- **Main pane**: File listing with customizable **Table** and **Grid** views, sortable columns (Name, Size, Modified, Permissions), search filtering, and drag-and-drop file upload.
- **Security sandbox**: All operations are strictly sandboxed inside the site's document root (`/var/www/<domain>`). Canonical path traversal checks prevent directory breakouts, parent directory climbing, or deletion of the root directory.

---

## Keyboard Shortcuts

The File Manager supports desktop keyboard shortcuts for high-efficiency management:

| Shortcut | Action |
|----------|--------|
| `Ctrl + A` / `Cmd + A` | Select all files and folders in current directory |
| `Ctrl + C` / `Cmd + C` | Copy selected items to clipboard |
| `Ctrl + X` / `Cmd + X` | Cut selected items to clipboard |
| `Ctrl + V` / `Cmd + V` | Paste clipboard items into current directory |
| `Delete` / `Backspace` | Prompt bulk deletion of selected items |
| `F2` | Rename selected item |
| `Escape` | Clear selection or close open modals / menus |
| `Ctrl + F` (Editor) | Open Find & Replace in Code Editor |
| `Ctrl + S` (Editor) | Save changes in Code Editor |

---

## File & Directory Operations

### Selection & Multi-Action Bar
- **Single click**: Selects an item.
- **Ctrl / Cmd + Click**: Toggle individual item selection without clearing previous selections.
- **Shift + Click**: Select contiguous ranges of items between the last active item and the clicked item.
- **Floating Action Bar**: When one or more items are selected, a floating bar appears at the bottom with quick actions: Copy, Cut, Permissions (Chmod), Compress, Download, and Delete.

### Clipboard (Copy, Cut, Paste, Duplicate)
- Files and folders can be copied or cut from one folder and pasted into any other directory within the site.
- **Duplicate**: Create a copy of any file or directory directly in place with a `_copy` suffix.
- **Bulk Operations**: Bulk copy, bulk move, and bulk delete operate atomically in the agent with safe relative path checking.

### Context Menu
Right-clicking any file, folder, or empty space brings up a context-sensitive menu:
- **File context**: Preview, Edit, Download, Copy, Cut, Duplicate, Rename, Permissions (Chmod), Properties (Stat & SHA-256), Delete.
- **Archive context** (`.zip`, `.tar`, `.tar.gz`, `.tgz`): Inspect contents, Extract here, Extract to folder, plus standard file actions.
- **Folder context**: Open, Copy, Cut, Compress, Rename, Permissions, Properties, Delete.
- **Empty area context**: New File, New Folder, Paste, Upload Files, Search, Storage Breakdown, Refresh.

---

## Permissions & Properties

### Visual Permissions (Chmod)
- **3x3 Matrix**: Visual checkboxes for Owner, Group, and Public read (`r`), write (`w`), and execute (`x`) bits.
- **Octal Calculator**: Dynamic numeric display (e.g. `0755`, `0644`). Typing an octal mode automatically updates the checkboxes, and checking boxes recalculates the octal value.
- **Recursive Application**: Toggle recursive permission updates across directory trees.

### File Properties & SHA-256 Checksums
- Inspect UNIX file metadata including exact byte size, disk blocks, last modified timestamp, and access permissions.
- **SHA-256 Checksum**: Compute cryptographic SHA-256 checksums on demand for file integrity verification.

---

## Archives & Compression

The File Manager includes native agent-backed archive management:

- **Compress**: Package one or multiple files/folders into `.zip`, `.tar`, or `.tar.gz` archives with customizable archive names.
- **Inspect**: Preview the internal file list and directory structure of `.zip` and `.tar` archives without extracting them to disk.
- **Extract**: Unpack archives directly into the current directory or into a dedicated destination folder.

---

## Search & Storage Breakdown

### Search & Grep
- **Filename Search**: Recursively search for files and folders matching glob patterns.
- **Content Grep**: Search file contents across the site directory with line numbers and matched snippets.

### Directory Storage Breakdown
- Analyze disk usage across folders with interactive size trees, item counts, and percentage gauges to locate large files or bloated cache directories.

---

## Rich Previews & Code Editor

### Multi-Format Preview Modal
- **Markdown**: Formats `.md` documents into rendered HTML using marked and DOMPurify sanitization.
- **Images**: Inline viewer for `.png`, `.jpg`, `.jpeg`, `.gif`, `.webp`, and `.svg`.
- **Audio & Video**: Built-in HTML5 media players for `.mp3`, `.wav`, `.ogg`, `.mp4`, `.webm`.
- **PDF**: Embedded PDF document previewer.
- **Archive Contents**: Structural tree viewer of archive contents.

### Code Editor
- Line numbers and syntax highlighting.
- Integrated **Find & Replace** (`Ctrl+F`) with match counting and replacement.
- Word wrap toggle and fullscreen mode for distraction-free editing.
- Instant save shortcut (`Ctrl+S`).

---

## Static Website Upgrade Wizard

For static websites, the File Manager provides an automated **Upgrade Website** wizard:
1. Upload a single `.zip` archive containing the new static website build.
2. The wizard validates the archive, offers an option to clean/purge old assets in `/public`, unpacks the new files, and sets correct ownership permissions.

---

## Security & Sandboxing

All agent file operations are secured through multi-layered guards:
1. **Canonical Root Verification**: Paths are canonicalized and verified against the site's base root `/var/www/<domain>`. Any path resolving outside this root is rejected with a `403 Forbidden` error.
2. **Root Safety Guard**: The root directory cannot be renamed or deleted (`resolve_safe_child`).
3. **Clean Environment Execution**: Archive utilities (`unzip`, `tar`, `zip`) execute using `safe_command` with cleared environment variables (`env_clear`) and fixed standard PATHs.
4. **Tenant Isolation**: Site ownership is validated on every backend API request before forwarding to the local agent.
