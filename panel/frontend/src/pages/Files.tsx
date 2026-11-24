import { useState, useEffect, useCallback, useRef, useMemo } from "react";
import { useParams, Link } from "react-router-dom";
import { api } from "../api";
import { UPLOAD_MAX_FILE_BYTES, uploadTooLargeMessage } from "../constants";
import { marked } from "marked";
import DOMPurify from "dompurify";

interface FileEntry {
  name: string;
  path: string;
  is_dir: boolean;
  size: number;
  modified: string;
  permissions?: string;
  owner?: string;
  group?: string;
  is_symlink?: boolean;
  symlink_target?: string;
}

interface Site {
  id: string;
  domain: string;
}

interface FileStat {
  name: string;
  path: string;
  is_dir: boolean;
  is_symlink: boolean;
  symlink_target?: string;
  size: number;
  mime_type: string;
  permissions_octal: string;
  permissions_symbolic: string;
  owner: string;
  group: string;
  uid: number;
  gid: number;
  created: string;
  modified: string;
  accessed: string;
  sha256?: string;
}

interface ArchiveEntry {
  name: string;
  size: number;
  is_dir: boolean;
  modified?: string;
}

interface SearchResult {
  name: string;
  path: string;
  is_dir: boolean;
  size: number;
  modified: string;
  line_matches: { line_number: number; line_content: string }[];
}

interface StorageStats {
  total_bytes: u64 | number;
  file_count: number;
  dir_count: number;
  breakdown: { name: string; path: string; is_dir: boolean; size: number }[];
}

type u64 = number;

interface ClipboardState {
  action: "copy" | "cut";
  items: string[];
  sourceDir: string;
}

function formatSize(bytes: number): string {
  if (bytes === 0) return "0 B";
  const units = ["B", "KB", "MB", "GB", "TB"];
  const i = Math.floor(Math.log(bytes) / Math.log(1024));
  return `${(bytes / Math.pow(1024, i)).toFixed(i > 0 ? 1 : 0)} ${units[i]}`;
}

function formatDate(iso: string): string {
  if (!iso) return "—";
  return new Date(iso).toLocaleString(undefined, {
    year: "numeric",
    month: "short",
    day: "numeric",
    hour: "2-digit",
    minute: "2-digit",
  });
}

function getFileIcon(name: string, is_dir: boolean, is_symlink?: boolean) {
  if (is_dir) return "📁";
  if (is_symlink) return "🔗";
  const ext = name.split(".").pop()?.toLowerCase() || "";
  switch (ext) {
    case "html":
    case "htm":
      return "🌐";
    case "css":
      return "🎨";
    case "js":
    case "ts":
    case "jsx":
    case "tsx":
    case "mjs":
      return "⚡";
    case "json":
    case "yaml":
    case "yml":
    case "xml":
      return "⚙️";
    case "php":
      return "🐘";
    case "png":
    case "jpg":
    case "jpeg":
    case "gif":
    case "webp":
    case "svg":
    case "ico":
      return "🖼️";
    case "pdf":
      return "📕";
    case "zip":
    case "tar":
    case "gz":
    case "tgz":
    case "7z":
      return "📦";
    case "mp4":
    case "webm":
      return "🎬";
    case "mp3":
    case "wav":
    case "ogg":
      return "🎵";
    case "sql":
      return "🗄️";
    case "sh":
    case "bash":
      return "💻";
    case "md":
    case "txt":
      return "📄";
    case "env":
    case "htaccess":
    case "ini":
    case "conf":
      return "🔒";
    default:
      return "📄";
  }
}

export default function Files() {
  const { id } = useParams<{ id: string }>();
  const [site, setSite] = useState<Site | null>(null);
  const [entries, setEntries] = useState<FileEntry[]>([]);
  const [currentPath, setCurrentPath] = useState(".");
  const [loading, setLoading] = useState(true);
  const [error, setError] = useState("");
  const [successMessage, setSuccessMessage] = useState("");

  // View mode: table vs grid
  const [viewMode, setViewMode] = useState<"table" | "grid">("table");
  const [showHidden, setShowHidden] = useState(false);
  const [sortBy, setSortBy] = useState<"name" | "size" | "modified" | "type">("name");
  const [sortAsc, setSortAsc] = useState(true);

  // Folder Tree Sidebar
  const [treeOpen, setTreeOpen] = useState(true);
  const [folderTree, setFolderTree] = useState<{ [path: string]: FileEntry[] }>({});
  const [expandedFolders, setExpandedFolders] = useState<Set<string>>(new Set(["."]));

  // Selection & Clipboard
  const [selectedItems, setSelectedItems] = useState<Set<string>>(new Set());
  const [lastSelected, setLastSelected] = useState<string | null>(null);
  const [clipboard, setClipboard] = useState<ClipboardState | null>(null);

  // Context Menu
  const [contextMenu, setContextMenu] = useState<{
    x: number;
    y: number;
    item: FileEntry | null;
  } | null>(null);

  // Modals & Drawers
  const [showCreate, setShowCreate] = useState(false);
  const [createName, setCreateName] = useState("");
  const [createType, setCreateType] = useState<"file" | "dir">("file");

  const [renamingItem, setRenamingItem] = useState<FileEntry | null>(null);
  const [renameValue, setRenameValue] = useState("");

  const [deleteConfirm, setDeleteConfirm] = useState<{
    items: string[];
    isBulk: boolean;
  } | null>(null);

  // Editor Modal
  const [editingFile, setEditingFile] = useState<{
    path: string;
    name: string;
    content: string;
  } | null>(null);
  const [editorWrap, setEditorWrap] = useState(true);
  const [editorFullScreen, setEditorFullScreen] = useState(false);
  const [editorSaving, setEditorSaving] = useState(false);
  const [editorSearch, setEditorSearch] = useState("");
  const [editorReplace, setEditorReplace] = useState("");
  const [showEditorSearch, setShowEditorSearch] = useState(false);

  // Preview Modal
  const [previewFile, setPreviewFile] = useState<{
    path: string;
    name: string;
    type: "image" | "markdown" | "text" | "audio" | "video" | "pdf" | "archive" | "other";
    content?: string;
    archiveEntries?: ArchiveEntry[];
  } | null>(null);

  // Properties / Chmod Modal
  const [statFile, setStatFile] = useState<FileStat | null>(null);
  const [chmodModal, setChmodModal] = useState<{
    path: string;
    name: string;
    mode: number;
    ownerR: boolean;
    ownerW: boolean;
    ownerX: boolean;
    groupR: boolean;
    groupW: boolean;
    groupX: boolean;
    otherR: boolean;
    otherW: boolean;
    otherX: boolean;
    recursive: boolean;
  } | null>(null);

  // Compress & Extract Modals
  const [compressModal, setCompressModal] = useState<{
    sources: string[];
    archiveName: string;
    targetDir: string;
  } | null>(null);

  const [extractModal, setExtractModal] = useState<{
    archivePath: string;
    archiveName: string;
    targetDir: string;
    overwrite: boolean;
  } | null>(null);

  // Search Modal
  const [showSearch, setShowSearch] = useState(false);
  const [searchQuery, setSearchQuery] = useState("");
  const [searchInContent, setSearchInContent] = useState(false);
  const [searchResults, setSearchResults] = useState<SearchResult[]>([]);
  const [searching, setSearching] = useState(false);

  // Storage Stats Drawer
  const [storageStats, setStorageStats] = useState<StorageStats | null>(null);
  const [showStorage, setShowStorage] = useState(false);
  const [loadingStorage, setLoadingStorage] = useState(false);

  // Static Site Upgrade Wizard
  const [showUpgradeWizard, setShowUpgradeWizard] = useState(false);
  const [upgradeStep, setUpgradeStep] = useState<1 | 2 | 3 | 4>(1);
  const [upgradeFile, setUpgradeFile] = useState<File | null>(null);
  const [upgradeBackupFirst, setUpgradeBackupFirst] = useState(true);
  const [upgradeStatus, setUpgradeStatus] = useState("");
  const [upgradeWorking, setUpgradeWorking] = useState(false);

  // Upload Management
  const [dragOver, setDragOver] = useState(false);
  const [uploadQueue, setUploadQueue] = useState<{
    id: string;
    name: string;
    size: number;
    progress: number;
    status: "pending" | "uploading" | "completed" | "error";
    error?: string;
  }[]>([]);
  const [showUploadDrawer, setShowUploadDrawer] = useState(false);
  const [uploadConflictAction, setUploadConflictAction] = useState<"overwrite" | "skip" | "rename">("overwrite");

  const fileInputRef = useRef<HTMLInputElement | null>(null);
  const folderInputRef = useRef<HTMLInputElement | null>(null);

  useEffect(() => {
    api.get<Site>(`/sites/${id}`).then(setSite).catch(() => {});
  }, [id]);

  const showSuccess = (msg: string) => {
    setSuccessMessage(msg);
    setTimeout(() => setSuccessMessage(""), 3500);
  };

  const loadDir = useCallback(
    async (path: string) => {
      setLoading(true);
      setError("");
      try {
        const data = await api.get<FileEntry[]>(
          `/sites/${id}/files?path=${encodeURIComponent(path)}`
        );
        setEntries(data);
        setCurrentPath(path);
        setSelectedItems(new Set());
        setLastSelected(null);

        // Update folder tree cache
        setFolderTree((prev) => ({
          ...prev,
          [path]: data.filter((e) => e.is_dir),
        }));
      } catch (e) {
        setError(e instanceof Error ? e.message : "Failed to load directory");
      } finally {
        setLoading(false);
      }
    },
    [id]
  );

  useEffect(() => {
    loadDir(".");
  }, [loadDir]);

  // Load subfolders for tree
  const loadTreeFolder = async (folderPath: string) => {
    try {
      const data = await api.get<FileEntry[]>(
        `/sites/${id}/files?path=${encodeURIComponent(folderPath)}`
      );
      setFolderTree((prev) => ({
        ...prev,
        [folderPath]: data.filter((e) => e.is_dir),
      }));
    } catch {
      // ignore tree prefetch error
    }
  };

  const toggleFolderExpand = (folderPath: string, e: React.MouseEvent) => {
    e.stopPropagation();
    setExpandedFolders((prev) => {
      const next = new Set(prev);
      if (next.has(folderPath)) {
        next.delete(folderPath);
      } else {
        next.add(folderPath);
        loadTreeFolder(folderPath);
      }
      return next;
    });
  };

  // Keyboard navigation & Shortcuts
  useEffect(() => {
    const handleKeyDown = (e: KeyboardEvent) => {
      // If modal or input active, do not hijack global shortcuts
      const activeEl = document.activeElement;
      const isInput = activeEl?.tagName === "INPUT" || activeEl?.tagName === "TEXTAREA";

      if (e.key === "Escape") {
        setContextMenu(null);
        if (!isInput) {
          setSelectedItems(new Set());
        }
      }

      if (isInput) return;

      if ((e.ctrlKey || e.metaKey) && e.key.toLowerCase() === "a") {
        e.preventDefault();
        const allNames = visibleEntries.map((item) => item.name);
        setSelectedItems(new Set(allNames));
      } else if ((e.ctrlKey || e.metaKey) && e.key.toLowerCase() === "c") {
        if (selectedItems.size > 0) {
          e.preventDefault();
          setClipboard({
            action: "copy",
            items: Array.from(selectedItems),
            sourceDir: currentPath,
          });
          showSuccess(`Copied ${selectedItems.size} item(s) to clipboard`);
        }
      } else if ((e.ctrlKey || e.metaKey) && e.key.toLowerCase() === "x") {
        if (selectedItems.size > 0) {
          e.preventDefault();
          setClipboard({
            action: "cut",
            items: Array.from(selectedItems),
            sourceDir: currentPath,
          });
          showSuccess(`Cut ${selectedItems.size} item(s) to clipboard`);
        }
      } else if ((e.ctrlKey || e.metaKey) && e.key.toLowerCase() === "v") {
        if (clipboard && clipboard.items.length > 0) {
          e.preventDefault();
          handlePaste();
        }
      } else if (e.key === "Delete" || e.key === "Backspace") {
        if (selectedItems.size > 0) {
          e.preventDefault();
          setDeleteConfirm({
            items: Array.from(selectedItems),
            isBulk: selectedItems.size > 1,
          });
        }
      } else if (e.key === "F2") {
        if (selectedItems.size === 1) {
          e.preventDefault();
          const targetName = Array.from(selectedItems)[0];
          const item = entries.find((e) => e.name === targetName);
          if (item) {
            setRenamingItem(item);
            setRenameValue(item.name);
          }
        }
      }
    };

    window.addEventListener("keydown", handleKeyDown);
    return () => window.removeEventListener("keydown", handleKeyDown);
  }, [selectedItems, clipboard, entries, currentPath]);

  // Filtered & Sorted entries
  const visibleEntries = useMemo(() => {
    let list = entries.filter((e) => showHidden || !e.name.startsWith("."));

    list.sort((a, b) => {
      if (a.is_dir !== b.is_dir) {
        return a.is_dir ? -1 : 1;
      }
      let cmp = 0;
      if (sortBy === "name") {
        cmp = a.name.localeCompare(b.name, undefined, { sensitivity: "base" });
      } else if (sortBy === "size") {
        cmp = a.size - b.size;
      } else if (sortBy === "modified") {
        cmp = new Date(a.modified).getTime() - new Date(b.modified).getTime();
      } else if (sortBy === "type") {
        const extA = a.name.split(".").pop() || "";
        const extB = b.name.split(".").pop() || "";
        cmp = extA.localeCompare(extB);
      }
      return sortAsc ? cmp : -cmp;
    });

    return list;
  }, [entries, showHidden, sortBy, sortAsc]);

  // Selection handlers
  const handleItemSelect = (name: string, e: React.MouseEvent) => {
    if (e.shiftKey && lastSelected) {
      const idxLast = visibleEntries.findIndex((i) => i.name === lastSelected);
      const idxCurr = visibleEntries.findIndex((i) => i.name === name);
      if (idxLast !== -1 && idxCurr !== -1) {
        const start = Math.min(idxLast, idxCurr);
        const end = Math.max(idxLast, idxCurr);
        const range = visibleEntries.slice(start, end + 1).map((i) => i.name);
        setSelectedItems(new Set(range));
        return;
      }
    }

    if (e.ctrlKey || e.metaKey) {
      setSelectedItems((prev) => {
        const next = new Set(prev);
        if (next.has(name)) next.delete(name);
        else next.add(name);
        return next;
      });
      setLastSelected(name);
    } else {
      setSelectedItems(new Set([name]));
      setLastSelected(name);
    }
  };

  const handleSelectAll = () => {
    if (selectedItems.size === visibleEntries.length) {
      setSelectedItems(new Set());
    } else {
      setSelectedItems(new Set(visibleEntries.map((i) => i.name)));
    }
  };

  const handleInvertSelection = () => {
    const next = new Set<string>();
    visibleEntries.forEach((i) => {
      if (!selectedItems.has(i.name)) next.add(i.name);
    });
    setSelectedItems(next);
  };

  // Directory Navigation
  const navigateTo = (name: string) => {
    const newPath = currentPath === "." ? name : `${currentPath}/${name}`;
    loadDir(newPath);
  };

  const goUp = () => {
    if (currentPath === ".") return;
    const parts = currentPath.split("/");
    parts.pop();
    loadDir(parts.length === 0 ? "." : parts.join("/"));
  };

  const breadcrumbs = currentPath === "." ? [] : currentPath.split("/");

  // Actions
  const handleCreate = async () => {
    if (!createName.trim()) return;
    const path = currentPath === "." ? createName.trim() : `${currentPath}/${createName.trim()}`;
    try {
      await api.post(`/sites/${id}/files/create?path=${encodeURIComponent(path)}&type=${createType}`);
      setShowCreate(false);
      setCreateName("");
      showSuccess(`Created ${createType === "dir" ? "folder" : "file"} "${createName}"`);
      loadDir(currentPath);
    } catch (e) {
      setError(e instanceof Error ? e.message : "Failed to create item");
    }
  };

  const handleRename = async () => {
    if (!renamingItem || !renameValue.trim()) return;
    const fromPath = currentPath === "." ? renamingItem.name : `${currentPath}/${renamingItem.name}`;
    const toPath = currentPath === "." ? renameValue.trim() : `${currentPath}/${renameValue.trim()}`;
    try {
      await api.post(`/sites/${id}/files/rename`, {
        from: fromPath,
        to: toPath,
      });
      setRenamingItem(null);
      setRenameValue("");
      showSuccess(`Renamed to "${renameValue.trim()}"`);
      loadDir(currentPath);
    } catch (e) {
      setError(e instanceof Error ? e.message : "Failed to rename");
    }
  };

  const handleDelete = async () => {
    if (!deleteConfirm) return;
    const { items } = deleteConfirm;
    try {
      if (items.length === 1) {
        const path = currentPath === "." ? items[0] : `${currentPath}/${items[0]}`;
        await api.delete(`/sites/${id}/files?path=${encodeURIComponent(path)}`);
        showSuccess(`Deleted "${items[0]}"`);
      } else {
        const paths = items.map((name) => (currentPath === "." ? name : `${currentPath}/${name}`));
        await api.post(`/sites/${id}/files/bulk/delete`, { paths });
        showSuccess(`Deleted ${items.length} item(s)`);
      }
      setDeleteConfirm(null);
      setSelectedItems(new Set());
      loadDir(currentPath);
    } catch (e) {
      setError(e instanceof Error ? e.message : "Failed to delete item(s)");
    }
  };

  const handleDuplicate = async (item: FileEntry) => {
    const path = currentPath === "." ? item.name : `${currentPath}/${item.name}`;
    try {
      const res = await api.post<{ success: boolean; name: string }>(`/sites/${id}/files/duplicate`, { path });
      showSuccess(`Duplicated as "${res.name}"`);
      loadDir(currentPath);
    } catch (e) {
      setError(e instanceof Error ? e.message : "Failed to duplicate");
    }
  };

  const handlePaste = async () => {
    if (!clipboard || clipboard.items.length === 0) return;
    const { action, items, sourceDir } = clipboard;
    const targetDir = currentPath;

    try {
      const sources = items.map((name) => (sourceDir === "." ? name : `${sourceDir}/${name}`));

      if (action === "copy") {
        await api.post(`/sites/${id}/files/bulk/copy`, {
          sources,
          target_dir: targetDir,
        });
        showSuccess(`Copied ${items.length} item(s) to current directory`);
      } else {
        await api.post(`/sites/${id}/files/bulk/move`, {
          sources,
          target_dir: targetDir,
        });
        showSuccess(`Moved ${items.length} item(s) to current directory`);
        setClipboard(null);
      }
      loadDir(currentPath);
    } catch (e) {
      setError(e instanceof Error ? e.message : "Failed to paste item(s)");
    }
  };

  // Open & Edit File
  const handleOpenFile = async (item: FileEntry) => {
    const ext = item.name.split(".").pop()?.toLowerCase() || "";
    const filePath = currentPath === "." ? item.name : `${currentPath}/${item.name}`;

    // Categorize for preview vs code editor
    const isImage = ["png", "jpg", "jpeg", "gif", "webp", "svg", "ico"].includes(ext);
    const isAudio = ["mp3", "wav", "ogg"].includes(ext);
    const isVideo = ["mp4", "webm"].includes(ext);
    const isPdf = ext === "pdf";
    const isArchive = ["zip", "tar", "gz", "tgz"].includes(ext);
    const isMarkdown = ["md", "markdown"].includes(ext);

    if (isImage || isAudio || isVideo || isPdf) {
      setPreviewFile({
        path: filePath,
        name: item.name,
        type: isImage ? "image" : isAudio ? "audio" : isVideo ? "video" : "pdf",
      });
      return;
    }

    if (isArchive) {
      try {
        const entries = await api.get<ArchiveEntry[]>(
          `/sites/${id}/files/inspect?path=${encodeURIComponent(filePath)}`
        );
        setPreviewFile({
          path: filePath,
          name: item.name,
          type: "archive",
          archiveEntries: entries,
        });
      } catch (e) {
        setError(e instanceof Error ? e.message : "Failed to inspect archive");
      }
      return;
    }

    // Default to Code Editor
    try {
      const res = await api.get<{ content: string; is_binary: boolean }>(
        `/sites/${id}/files/read?path=${encodeURIComponent(filePath)}`
      );
      if (res.is_binary) {
        setPreviewFile({
          path: filePath,
          name: item.name,
          type: "other",
        });
      } else {
        if (isMarkdown) {
          setPreviewFile({
            path: filePath,
            name: item.name,
            type: "markdown",
            content: res.content,
          });
        } else {
          setEditingFile({
            path: filePath,
            name: item.name,
            content: res.content,
          });
        }
      }
    } catch (e) {
      setError(e instanceof Error ? e.message : "Failed to read file");
    }
  };

  const handleSaveEditor = async () => {
    if (!editingFile) return;
    setEditorSaving(true);
    try {
      await api.put(`/sites/${id}/files/write`, {
        path: editingFile.path,
        content: editingFile.content,
      });
      showSuccess(`Saved "${editingFile.name}"`);
    } catch (e) {
      setError(e instanceof Error ? e.message : "Failed to save file");
    } finally {
      setEditorSaving(false);
    }
  };

  // Inspect Stat & Properties
  const handleInspectProperties = async (item: FileEntry) => {
    const filePath = currentPath === "." ? item.name : `${currentPath}/${item.name}`;
    try {
      const stat = await api.get<FileStat>(
        `/sites/${id}/files/stat?path=${encodeURIComponent(filePath)}&checksum=true`
      );
      setStatFile(stat);
    } catch (e) {
      setError(e instanceof Error ? e.message : "Failed to fetch file properties");
    }
  };

  // Permissions (Chmod)
  const handleOpenChmod = (item: FileEntry) => {
    const filePath = currentPath === "." ? item.name : `${currentPath}/${item.name}`;
    const octal = item.permissions ? parseInt(item.permissions, 8) : 0o644;
    const mode = isNaN(octal) ? 0o644 : octal;

    const ownerR = ((mode >> 6) & 4) !== 0;
    const ownerW = ((mode >> 6) & 2) !== 0;
    const ownerX = ((mode >> 6) & 1) !== 0;

    const groupR = ((mode >> 3) & 4) !== 0;
    const groupW = ((mode >> 3) & 2) !== 0;
    const groupX = ((mode >> 3) & 1) !== 0;

    const otherR = (mode & 4) !== 0;
    const otherW = (mode & 2) !== 0;
    const otherX = (mode & 1) !== 0;

    setChmodModal({
      path: filePath,
      name: item.name,
      mode,
      ownerR,
      ownerW,
      ownerX,
      groupR,
      groupW,
      groupX,
      otherR,
      otherW,
      otherX,
      recursive: false,
    });
  };

  const handleApplyChmod = async () => {
    if (!chmodModal) return;
    const owner = (chmodModal.ownerR ? 4 : 0) + (chmodModal.ownerW ? 2 : 0) + (chmodModal.ownerX ? 1 : 0);
    const group = (chmodModal.groupR ? 4 : 0) + (chmodModal.groupW ? 2 : 0) + (chmodModal.groupX ? 1 : 0);
    const other = (chmodModal.otherR ? 4 : 0) + (chmodModal.otherW ? 2 : 0) + (chmodModal.otherX ? 1 : 0);
    const numericMode = owner * 64 + group * 8 + other;

    try {
      await api.post(`/sites/${id}/files/chmod`, {
        path: chmodModal.path,
        mode: numericMode,
        recursive: chmodModal.recursive,
      });
      setChmodModal(null);
      showSuccess(`Permissions updated to 0${owner}${group}${other}`);
      loadDir(currentPath);
    } catch (e) {
      setError(e instanceof Error ? e.message : "Failed to change permissions");
    }
  };

  // Compress & Extract
  const handleOpenCompress = (sources: string[]) => {
    const first = sources[0];
    const defaultName = sources.length === 1 ? `${first.replace(/\.[^/.]+$/, "")}.zip` : `archive.zip`;
    setCompressModal({
      sources,
      archiveName: defaultName,
      targetDir: currentPath,
    });
  };

  const handleApplyCompress = async () => {
    if (!compressModal || !compressModal.archiveName.trim()) return;
    try {
      const sources = compressModal.sources.map((name) =>
        currentPath === "." ? name : `${currentPath}/${name}`
      );
      await api.post(`/sites/${id}/files/compress`, {
        sources,
        archive_name: compressModal.archiveName.trim(),
        target_dir: compressModal.targetDir,
      });
      setCompressModal(null);
      showSuccess(`Created archive "${compressModal.archiveName.trim()}"`);
      loadDir(currentPath);
    } catch (e) {
      setError(e instanceof Error ? e.message : "Failed to compress files");
    }
  };

  const handleOpenExtract = (item: FileEntry) => {
    const archivePath = currentPath === "." ? item.name : `${currentPath}/${item.name}`;
    setExtractModal({
      archivePath,
      archiveName: item.name,
      targetDir: currentPath,
      overwrite: true,
    });
  };

  const handleApplyExtract = async () => {
    if (!extractModal) return;
    try {
      await api.post(`/sites/${id}/files/extract`, {
        archive: extractModal.archivePath,
        target_dir: extractModal.targetDir,
        overwrite: extractModal.overwrite,
      });
      setExtractModal(null);
      showSuccess(`Extracted "${extractModal.archiveName}"`);
      loadDir(currentPath);
    } catch (e) {
      setError(e instanceof Error ? e.message : "Failed to extract archive");
    }
  };

  // Search
  const handleExecuteSearch = async () => {
    if (!searchQuery.trim()) return;
    setSearching(true);
    try {
      const res = await api.get<SearchResult[]>(
        `/sites/${id}/files/search?path=${encodeURIComponent(currentPath)}&query=${encodeURIComponent(
          searchQuery.trim()
        )}&in_content=${searchInContent}&limit=100`
      );
      setSearchResults(res);
    } catch (e) {
      setError(e instanceof Error ? e.message : "Search failed");
    } finally {
      setSearching(false);
    }
  };

  // Directory Storage Stats
  const handleLoadStorageStats = async () => {
    setShowStorage(true);
    setLoadingStorage(true);
    try {
      const res = await api.get<StorageStats>(
        `/sites/${id}/files/storage?path=${encodeURIComponent(currentPath)}`
      );
      setStorageStats(res);
    } catch (e) {
      setError(e instanceof Error ? e.message : "Failed to calculate storage");
    } finally {
      setLoadingStorage(false);
    }
  };

  // Upload Processing
  const handleUploadFiles = async (files: FileList | File[]) => {
    if (!files || files.length === 0) return;
    setShowUploadDrawer(true);

    for (const file of Array.from(files)) {
      const uploadId = `${file.name}-${Date.now()}`;
      setUploadQueue((prev) => [
        ...prev,
        {
          id: uploadId,
          name: file.name,
          size: file.size,
          progress: 0,
          status: "uploading",
        },
      ]);

      if (file.size > UPLOAD_MAX_FILE_BYTES) {
        setUploadQueue((prev) =>
          prev.map((item) =>
            item.id === uploadId
              ? {
                  ...item,
                  status: "error",
                  error: uploadTooLargeMessage(file.name, file.size),
                }
              : item
          )
        );
        continue;
      }

      try {
        const base64 = await new Promise<string>((resolve, reject) => {
          const reader = new FileReader();
          reader.onload = () => resolve((reader.result as string).split(",")[1]);
          reader.onerror = reject;
          reader.readAsDataURL(file);
        });

        // Determine destination filename with conflict resolution
        let finalFilename = file.name;
        const exists = entries.some((e) => e.name === file.name);
        if (exists && uploadConflictAction === "skip") {
          setUploadQueue((prev) =>
            prev.map((item) =>
              item.id === uploadId
                ? { ...item, status: "completed", progress: 100 }
                : item
            )
          );
          continue;
        } else if (exists && uploadConflictAction === "rename") {
          const parts = file.name.split(".");
          const ext = parts.length > 1 ? `.${parts.pop()}` : "";
          finalFilename = `${parts.join(".")}_${Date.now()}${ext}`;
        }

        await api.post(`/sites/${id}/files/upload`, {
          path: currentPath,
          filename: finalFilename,
          content: base64,
        });

        setUploadQueue((prev) =>
          prev.map((item) =>
            item.id === uploadId
              ? { ...item, status: "completed", progress: 100 }
              : item
          )
        );
      } catch (err) {
        setUploadQueue((prev) =>
          prev.map((item) =>
            item.id === uploadId
              ? {
                  ...item,
                  status: "error",
                  error: err instanceof Error ? err.message : "Upload failed",
                }
              : item
          )
        );
      }
    }

    // setUploading(false)
    loadDir(currentPath);
  };

  // Static Site Upgrade Workflow
  const handleExecuteSiteUpgrade = async () => {
    if (!upgradeFile) return;
    setUpgradeWorking(true);
    setUpgradeStatus("Uploading site ZIP archive...");

    try {
      // Step 1: Optional automated backup
      if (upgradeBackupFirst) {
        setUpgradeStatus("Creating backup before deployment...");
        try {
          await api.post(`/sites/${id}/backups`);
        } catch {
          // continue even if backup skipped
        }
      }

      // Step 2: Upload ZIP into root
      setUpgradeStatus("Staging archive onto server...");
      const base64 = await new Promise<string>((resolve, reject) => {
        const reader = new FileReader();
        reader.onload = () => resolve((reader.result as string).split(",")[1]);
        reader.onerror = reject;
        reader.readAsDataURL(upgradeFile);
      });

      const zipName = `upgrade_${Date.now()}.zip`;
      await api.post(`/sites/${id}/files/upload`, {
        path: ".",
        filename: zipName,
        content: base64,
      });

      // Step 3: Extract archive directly to webroot
      setUpgradeStatus("Extracting new website files...");
      await api.post(`/sites/${id}/files/extract`, {
        archive: zipName,
        target_dir: ".",
        overwrite: true,
      });

      // Step 4: Remove temporary zip
      setUpgradeStatus("Finalizing deployment...");
      try {
        await api.delete(`/sites/${id}/files?path=${encodeURIComponent(zipName)}`);
      } catch {
        // ignore cleanup error
      }

      setUpgradeStep(4);
      showSuccess("Static website successfully upgraded and deployed!");
      loadDir(".");
    } catch (e) {
      setError(e instanceof Error ? e.message : "Upgrade failed");
    } finally {
      setUpgradeWorking(false);
    }
  };

  // Download File Helper
  const triggerDownload = (path: string, name: string) => {
    const url = `/api/sites/${id}/files/download?path=${encodeURIComponent(path)}`;
    const link = document.createElement("a");
    link.href = url;
    link.download = name;
    document.body.appendChild(link);
    link.click();
    document.body.removeChild(link);
  };

  return (
    <div
      className="p-4 sm:p-6 lg:p-8 min-h-screen relative"
      onDragOver={(e) => {
        e.preventDefault();
        setDragOver(true);
      }}
      onDragLeave={(e) => {
        e.preventDefault();
        setDragOver(false);
      }}
      onDrop={(e) => {
        e.preventDefault();
        setDragOver(false);
        if (e.dataTransfer.files && e.dataTransfer.files.length > 0) {
          handleUploadFiles(e.dataTransfer.files);
        }
      }}
      onClick={() => setContextMenu(null)}
    >
      {/* Drag & Drop full-screen overlay */}
      {dragOver && (
        <div className="fixed inset-0 z-40 bg-rust-500/20 border-4 border-dashed border-rust-500 flex flex-col items-center justify-center pointer-events-none backdrop-blur-xs">
          <div className="bg-dark-900 border border-rust-500 p-8 rounded-2xl shadow-2xl flex flex-col items-center text-center max-w-md">
            <span className="text-5xl mb-3 animate-bounce">📤</span>
            <h3 className="text-lg font-bold text-dark-50">Drop files to upload</h3>
            <p className="text-xs text-dark-300 mt-1 font-mono">
              Files will be uploaded to <span className="text-rust-400 font-bold">{currentPath}</span>
            </p>
          </div>
        </div>
      )}

      {/* Hidden file inputs */}
      <input
        ref={fileInputRef}
        type="file"
        multiple
        className="hidden"
        onChange={(e) => {
          if (e.target.files) handleUploadFiles(e.target.files);
          e.target.value = "";
        }}
      />
      <input
        ref={folderInputRef}
        type="file"
        // @ts-expect-error directory attribute
        webkitdirectory="true"
        className="hidden"
        onChange={(e) => {
          if (e.target.files) handleUploadFiles(e.target.files);
          e.target.value = "";
        }}
      />

      {/* Top Breadcrumb & Actions */}
      <div className="flex flex-col sm:flex-row sm:items-center justify-between gap-4 mb-4 pb-4 border-b border-dark-600">
        <div>
          <div className="flex items-center gap-2 text-xs font-mono text-dark-300 mb-1">
            <Link to="/sites" className="hover:text-dark-100">
              Sites
            </Link>
            <span>/</span>
            <Link to={`/sites/${id}`} className="text-rust-400 hover:text-rust-300">
              {site?.domain || "Site"}
            </Link>
            <span>/</span>
            <span className="text-dark-100 font-bold">File Manager</span>
          </div>
          <h1 className="text-xl font-bold text-dark-50 tracking-tight flex items-center gap-2">
            <span>📁</span>
            <span>File Explorer</span>
            <span className="text-xs px-2 py-0.5 rounded-full bg-dark-700 text-dark-200 font-mono">
              /var/www/{site?.domain}
            </span>
          </h1>
        </div>

        <div className="flex flex-wrap items-center gap-2">
          {/* Static Site Upgrade Wizard button */}
          <button
            onClick={() => {
              setUpgradeStep(1);
              setUpgradeFile(null);
              setShowUpgradeWizard(true);
            }}
            className="px-3 py-1.5 bg-gradient-to-r from-rust-600 to-amber-600 hover:from-rust-500 hover:to-amber-500 text-white rounded-lg text-xs font-bold shadow flex items-center gap-1.5 transition-all"
            title="Deploy new ZIP to website with rollback"
          >
            <span>🚀</span>
            <span>Upgrade Site (ZIP)</span>
          </button>

          {/* Storage stats */}
          <button
            onClick={handleLoadStorageStats}
            className="px-3 py-1.5 bg-dark-700 hover:bg-dark-600 text-dark-200 hover:text-dark-50 rounded-lg text-xs font-medium border border-dark-600 flex items-center gap-1.5 transition-colors"
          >
            <span>📊</span>
            <span>Storage Usage</span>
          </button>

          {/* Search */}
          <button
            onClick={() => setShowSearch(true)}
            className="px-3 py-1.5 bg-dark-700 hover:bg-dark-600 text-dark-200 hover:text-dark-50 rounded-lg text-xs font-medium border border-dark-600 flex items-center gap-1.5 transition-colors"
          >
            <span>🔍</span>
            <span>Search</span>
          </button>

          {/* New Item */}
          <button
            onClick={() => {
              setCreateName("");
              setCreateType("file");
              setShowCreate(true);
            }}
            className="px-3 py-1.5 bg-rust-500 hover:bg-rust-600 text-white rounded-lg text-xs font-bold shadow flex items-center gap-1.5 transition-colors"
          >
            <span>+</span>
            <span>New File / Folder</span>
          </button>

          {/* Upload Dropdown */}
          <div className="relative group">
            <button
              onClick={() => fileInputRef.current?.click()}
              className="px-3 py-1.5 bg-emerald-600 hover:bg-emerald-500 text-white rounded-lg text-xs font-bold shadow flex items-center gap-1.5 transition-colors"
            >
              <span>⬆️</span>
              <span>Upload</span>
            </button>
          </div>
        </div>
      </div>

      {/* Alert Notices */}
      {error && (
        <div className="mb-4 p-3 bg-rose-500/10 border border-rose-500/30 rounded-xl text-rose-400 text-xs flex items-center justify-between">
          <div className="flex items-center gap-2">
            <span>⚠️</span>
            <span>{error}</span>
          </div>
          <button onClick={() => setError("")} className="text-rose-400 hover:text-rose-200 font-bold">
            ✕
          </button>
        </div>
      )}

      {successMessage && (
        <div className="mb-4 p-3 bg-emerald-500/10 border border-emerald-500/30 rounded-xl text-emerald-400 text-xs flex items-center justify-between">
          <div className="flex items-center gap-2">
            <span>✓</span>
            <span>{successMessage}</span>
          </div>
          <button onClick={() => setSuccessMessage("")} className="text-emerald-400 hover:text-emerald-200 font-bold">
            ✕
          </button>
        </div>
      )}

      {/* Path Toolbar & Navigation Strip */}
      <div className="bg-dark-800 border border-dark-600 rounded-xl p-3 mb-4 flex flex-col md:flex-row md:items-center justify-between gap-3 shadow-sm">
        {/* Navigation buttons & Breadcrumbs */}
        <div className="flex items-center gap-2 overflow-x-auto text-xs font-mono">
          <button
            onClick={goUp}
            disabled={currentPath === "."}
            className={`p-1.5 rounded-lg border border-dark-600 bg-dark-700 hover:bg-dark-600 text-dark-200 hover:text-dark-50 transition-colors ${
              currentPath === "." ? "opacity-40 pointer-events-none" : ""
            }`}
            title="Go up one directory"
          >
            ⬆️
          </button>
          <button
            onClick={() => loadDir(currentPath)}
            className="p-1.5 rounded-lg border border-dark-600 bg-dark-700 hover:bg-dark-600 text-dark-200 hover:text-dark-50 transition-colors"
            title="Refresh current directory"
          >
            🔄
          </button>
          <button
            onClick={() => setTreeOpen(!treeOpen)}
            className={`p-1.5 rounded-lg border border-dark-600 text-xs transition-colors ${
              treeOpen ? "bg-rust-500/20 text-rust-400 border-rust-500/30" : "bg-dark-700 text-dark-300 hover:text-dark-100"
            }`}
            title="Toggle tree sidebar"
          >
            🗂️
          </button>

          <div className="flex items-center gap-1 px-3 py-1 bg-dark-900 border border-dark-700 rounded-lg overflow-x-auto max-w-xl">
            <button
              onClick={() => loadDir(".")}
              className="text-rust-400 hover:text-rust-300 font-bold hover:underline"
            >
              /
            </button>
            {breadcrumbs.map((part, i) => (
              <span key={i} className="flex items-center gap-1 shrink-0">
                <span className="text-dark-500">/</span>
                <button
                  onClick={() => loadDir(breadcrumbs.slice(0, i + 1).join("/"))}
                  className={`hover:underline ${
                    i === breadcrumbs.length - 1 ? "text-dark-50 font-bold" : "text-rust-400 hover:text-rust-300"
                  }`}
                >
                  {part}
                </button>
              </span>
            ))}
          </div>
        </div>

        {/* View Controls & Toggles */}
        <div className="flex items-center gap-2">
          {/* Hidden files toggle */}
          <button
            onClick={() => setShowHidden(!showHidden)}
            className={`px-2.5 py-1.5 rounded-lg text-xs font-mono flex items-center gap-1.5 border transition-colors ${
              showHidden
                ? "bg-amber-500/15 border-amber-500/30 text-amber-300 font-bold"
                : "bg-dark-700 border-dark-600 text-dark-300 hover:text-dark-100"
            }`}
          >
            <span>{showHidden ? "👁️" : "👁️‍🗨️"}</span>
            <span>{showHidden ? "Hidden Shown" : "Hidden"}</span>
          </button>

          {/* View mode toggle */}
          <div className="flex items-center bg-dark-700 p-0.5 rounded-lg border border-dark-600">
            <button
              onClick={() => setViewMode("table")}
              className={`p-1.5 rounded text-xs transition-colors ${
                viewMode === "table" ? "bg-dark-600 text-dark-50 shadow" : "text-dark-400 hover:text-dark-200"
              }`}
              title="Table view"
            >
              📋
            </button>
            <button
              onClick={() => setViewMode("grid")}
              className={`p-1.5 rounded text-xs transition-colors ${
                viewMode === "grid" ? "bg-dark-600 text-dark-50 shadow" : "text-dark-400 hover:text-dark-200"
              }`}
              title="Grid view"
            >
              ▦
            </button>
          </div>

          {/* Paste button if clipboard has items */}
          {clipboard && (
            <button
              onClick={handlePaste}
              className="px-2.5 py-1.5 bg-rust-500/20 hover:bg-rust-500/30 border border-rust-500/40 text-rust-300 rounded-lg text-xs font-bold flex items-center gap-1 animate-pulse"
              title={`Paste ${clipboard.items.length} item(s)`}
            >
              <span>📋</span>
              <span>Paste ({clipboard.items.length})</span>
            </button>
          )}
        </div>
      </div>

      {/* Main Dual-Pane Explorer Container */}
      <div className="flex gap-4 items-start">
        {/* Left Folder Tree Sidebar */}
        {treeOpen && (
          <div className="w-64 shrink-0 bg-dark-800 border border-dark-600 rounded-xl p-3 text-xs font-mono shadow-sm hidden md:block max-h-[75vh] overflow-y-auto">
            <div className="flex items-center justify-between pb-2 mb-2 border-b border-dark-700">
              <span className="text-dark-300 font-bold uppercase tracking-wider text-[10px]">Folder Tree</span>
              <button
                onClick={() => loadTreeFolder(".")}
                className="text-dark-400 hover:text-dark-200 text-[10px]"
              >
                Refresh
              </button>
            </div>

            <div className="space-y-1">
              <div
                onClick={() => loadDir(".")}
                className={`flex items-center gap-1.5 p-1.5 rounded-lg cursor-pointer transition-colors ${
                  currentPath === "." ? "bg-rust-500/20 text-rust-300 font-bold" : "text-dark-200 hover:bg-dark-700"
                }`}
              >
                <span>🏠</span>
                <span>/ (root)</span>
              </div>

              {/* Recursive Tree Node Renderer */}
              {renderTreeNodes(".", folderTree["."] || [], currentPath, expandedFolders, toggleFolderExpand, loadDir)}
            </div>
          </div>
        )}

        {/* Right Main Content Area */}
        <div className="flex-1 min-w-0 bg-dark-800 border border-dark-600 rounded-xl shadow-sm overflow-hidden flex flex-col">
          {loading ? (
            <div className="p-16 flex flex-col items-center justify-center gap-3">
              <div className="w-8 h-8 border-2 border-dark-600 border-t-rust-500 rounded-full animate-spin" />
              <p className="text-xs text-dark-300 font-mono">Reading directory contents...</p>
            </div>
          ) : visibleEntries.length === 0 ? (
            <div className="p-16 text-center">
              <span className="text-4xl mb-3 block">📭</span>
              <h3 className="text-sm font-bold text-dark-100">Directory is empty</h3>
              <p className="text-xs text-dark-300 mt-1 font-mono">
                Drop files here, create a new file/folder, or upload from your device.
              </p>
              <div className="mt-4 flex justify-center gap-2">
                <button
                  onClick={() => setShowCreate(true)}
                  className="px-3 py-1.5 bg-dark-700 hover:bg-dark-600 text-dark-100 rounded-lg text-xs font-medium"
                >
                  + New Item
                </button>
                <button
                  onClick={() => fileInputRef.current?.click()}
                  className="px-3 py-1.5 bg-rust-500 hover:bg-rust-600 text-white rounded-lg text-xs font-bold"
                >
                  Upload Files
                </button>
              </div>
            </div>
          ) : viewMode === "table" ? (
            /* Table View */
            <div className="overflow-x-auto">
              <table className="w-full text-left text-xs">
                <thead>
                  <tr className="bg-dark-900 border-b border-dark-700 text-dark-300 font-mono select-none">
                    <th className="p-3 w-10 text-center">
                      <input
                        type="checkbox"
                        checked={selectedItems.size === visibleEntries.length && visibleEntries.length > 0}
                        onChange={handleSelectAll}
                        className="rounded bg-dark-700 border-dark-500 text-rust-500 focus:ring-0 cursor-pointer"
                      />
                    </th>
                    <th
                      className="p-3 cursor-pointer hover:text-dark-100"
                      onClick={() => {
                        if (sortBy === "name") setSortAsc(!sortAsc);
                        else {
                          setSortBy("name");
                          setSortAsc(true);
                        }
                      }}
                    >
                      Name {sortBy === "name" && (sortAsc ? "▲" : "▼")}
                    </th>
                    <th
                      className="p-3 w-28 text-right cursor-pointer hover:text-dark-100 hidden sm:table-cell"
                      onClick={() => {
                        if (sortBy === "size") setSortAsc(!sortAsc);
                        else {
                          setSortBy("size");
                          setSortAsc(true);
                        }
                      }}
                    >
                      Size {sortBy === "size" && (sortAsc ? "▲" : "▼")}
                    </th>
                    <th className="p-3 w-28 text-center font-mono hidden md:table-cell">Perms</th>
                    <th className="p-3 w-24 text-center font-mono hidden lg:table-cell">Owner</th>
                    <th
                      className="p-3 w-40 text-right cursor-pointer hover:text-dark-100 hidden sm:table-cell"
                      onClick={() => {
                        if (sortBy === "modified") setSortAsc(!sortAsc);
                        else {
                          setSortBy("modified");
                          setSortAsc(true);
                        }
                      }}
                    >
                      Modified {sortBy === "modified" && (sortAsc ? "▲" : "▼")}
                    </th>
                    <th className="p-3 w-20 text-center">Actions</th>
                  </tr>
                </thead>
                <tbody className="divide-y divide-dark-700 font-mono">
                  {visibleEntries.map((item) => {
                    const isSelected = selectedItems.has(item.name);
                    const isCut = clipboard?.action === "cut" && clipboard.items.includes(item.name);

                    return (
                      <tr
                        key={item.name}
                        onClick={(e) => handleItemSelect(item.name, e)}
                        onDoubleClick={() => {
                          if (item.is_dir) navigateTo(item.name);
                          else handleOpenFile(item);
                        }}
                        onContextMenu={(e) => {
                          e.preventDefault();
                          e.stopPropagation();
                          if (!selectedItems.has(item.name)) {
                            setSelectedItems(new Set([item.name]));
                          }
                          setContextMenu({
                            x: e.clientX,
                            y: e.clientY,
                            item,
                          });
                        }}
                        className={`group cursor-pointer transition-colors ${
                          isSelected
                            ? "bg-rust-500/15 text-dark-50"
                            : "hover:bg-dark-700/60 text-dark-200"
                        } ${isCut ? "opacity-40" : ""}`}
                      >
                        <td className="p-3 text-center" onClick={(e) => e.stopPropagation()}>
                          <input
                            type="checkbox"
                            checked={isSelected}
                            onChange={(e) => {
                              setSelectedItems((prev) => {
                                const next = new Set(prev);
                                if (e.target.checked) next.add(item.name);
                                else next.delete(item.name);
                                return next;
                              });
                            }}
                            className="rounded bg-dark-700 border-dark-500 text-rust-500 focus:ring-0 cursor-pointer"
                          />
                        </td>
                        <td className="p-3 font-medium flex items-center gap-2">
                          <span className="text-base select-none">
                            {getFileIcon(item.name, item.is_dir, item.is_symlink)}
                          </span>
                          <span
                            className={`truncate ${
                              item.is_dir ? "text-dark-100 font-bold hover:text-rust-300" : "text-dark-100"
                            }`}
                          >
                            {item.name}
                          </span>
                          {item.name.startsWith(".env") && (
                            <span className="text-[10px] px-1.5 py-0.5 rounded bg-rose-500/20 text-rose-300 font-bold">
                              ENV
                            </span>
                          )}
                          {item.name === ".htaccess" && (
                            <span className="text-[10px] px-1.5 py-0.5 rounded bg-amber-500/20 text-amber-300 font-bold">
                              CONFIG
                            </span>
                          )}
                        </td>
                        <td className="p-3 text-right text-dark-300 hidden sm:table-cell">
                          {item.is_dir ? "—" : formatSize(item.size)}
                        </td>
                        <td
                          className="p-3 text-center text-dark-300 hover:text-rust-300 cursor-pointer hidden md:table-cell"
                          onClick={(e) => {
                            e.stopPropagation();
                            handleOpenChmod(item);
                          }}
                          title="Click to edit permissions"
                        >
                          <span className="px-1.5 py-0.5 bg-dark-900 rounded border border-dark-700 text-[11px]">
                            {item.permissions || "0644"}
                          </span>
                        </td>
                        <td className="p-3 text-center text-dark-400 text-[11px] hidden lg:table-cell">
                          {item.owner || "www-data"}
                        </td>
                        <td className="p-3 text-right text-dark-400 text-[11px] hidden sm:table-cell">
                          {formatDate(item.modified)}
                        </td>
                        <td className="p-3 text-center" onClick={(e) => e.stopPropagation()}>
                          <div className="flex items-center justify-center gap-1 opacity-80 group-hover:opacity-100">
                            {item.is_dir ? (
                              <button
                                onClick={() => navigateTo(item.name)}
                                className="p-1 hover:bg-dark-600 rounded text-dark-200 hover:text-dark-50"
                                title="Open Folder"
                              >
                                📂
                              </button>
                            ) : (
                              <button
                                onClick={() => handleOpenFile(item)}
                                className="p-1 hover:bg-dark-600 rounded text-dark-200 hover:text-dark-50"
                                title="Open / Edit"
                              >
                                ✏️
                              </button>
                            )}
                            <button
                              onClick={(e) => {
                                const rect = (e.currentTarget as HTMLElement).getBoundingClientRect();
                                setContextMenu({
                                  x: rect.left,
                                  y: rect.bottom + 4,
                                  item,
                                });
                              }}
                              className="p-1 hover:bg-dark-600 rounded text-dark-300 hover:text-dark-50"
                              title="More options"
                            >
                              ⋮
                            </button>
                          </div>
                        </td>
                      </tr>
                    );
                  })}
                </tbody>
              </table>
            </div>
          ) : (
            /* Grid View */
            <div className="p-4 grid grid-cols-2 sm:grid-cols-3 md:grid-cols-4 lg:grid-cols-6 gap-3">
              {visibleEntries.map((item) => {
                const isSelected = selectedItems.has(item.name);
                const isCut = clipboard?.action === "cut" && clipboard.items.includes(item.name);

                return (
                  <div
                    key={item.name}
                    onClick={(e) => handleItemSelect(item.name, e)}
                    onDoubleClick={() => {
                      if (item.is_dir) navigateTo(item.name);
                      else handleOpenFile(item);
                    }}
                    onContextMenu={(e) => {
                      e.preventDefault();
                      e.stopPropagation();
                      if (!selectedItems.has(item.name)) {
                        setSelectedItems(new Set([item.name]));
                      }
                      setContextMenu({
                        x: e.clientX,
                        y: e.clientY,
                        item,
                      });
                    }}
                    className={`p-3 rounded-xl border flex flex-col items-center text-center cursor-pointer transition-all ${
                      isSelected
                        ? "bg-rust-500/20 border-rust-500/50 shadow-md text-dark-50"
                        : "bg-dark-900/60 border-dark-700 hover:bg-dark-700 hover:border-dark-500 text-dark-200"
                    } ${isCut ? "opacity-40" : ""}`}
                  >
                    <span className="text-3xl mb-2 select-none">
                      {getFileIcon(item.name, item.is_dir, item.is_symlink)}
                    </span>
                    <span className="text-xs font-mono font-medium truncate max-w-full text-dark-100">
                      {item.name}
                    </span>
                    <span className="text-[10px] text-dark-400 font-mono mt-1">
                      {item.is_dir ? "folder" : formatSize(item.size)}
                    </span>
                  </div>
                );
              })}
            </div>
          )}

          {/* Table Footer Status */}
          <div className="bg-dark-900 border-t border-dark-700 p-2.5 px-4 flex flex-wrap items-center justify-between text-xs font-mono text-dark-400">
            <div className="flex items-center gap-4">
              <span>
                {visibleEntries.length} item(s) {showHidden ? "(including hidden)" : ""}
              </span>
              {selectedItems.size > 0 && (
                <span className="text-rust-400 font-bold">
                  {selectedItems.size} item(s) selected
                </span>
              )}
            </div>

            <div className="flex items-center gap-2">
              {selectedItems.size > 0 && (
                <button
                  onClick={handleInvertSelection}
                  className="hover:text-dark-200 underline"
                >
                  Invert
                </button>
              )}
              {selectedItems.size > 0 && (
                <button
                  onClick={() => setSelectedItems(new Set())}
                  className="hover:text-dark-200 underline"
                >
                  Clear
                </button>
              )}
            </div>
          </div>
        </div>
      </div>

      {/* Sticky Bottom Multi-Action Bar */}
      {selectedItems.size > 0 && (
        <div className="fixed bottom-6 left-1/2 -translate-x-1/2 z-30 bg-dark-900 border border-rust-500/40 rounded-2xl shadow-2xl p-2.5 px-5 flex items-center gap-3 backdrop-blur-md animate-slide-up font-mono text-xs">
          <span className="font-bold text-dark-50 bg-rust-500 px-2 py-0.5 rounded-full">
            {selectedItems.size} selected
          </span>

          <div className="h-4 w-px bg-dark-700" />

          {/* Copy */}
          <button
            onClick={() => {
              setClipboard({
                action: "copy",
                items: Array.from(selectedItems),
                sourceDir: currentPath,
              });
              showSuccess(`Copied ${selectedItems.size} item(s)`);
            }}
            className="p-1.5 px-2.5 rounded-lg bg-dark-800 hover:bg-dark-700 text-dark-100 flex items-center gap-1.5"
            title="Copy (Ctrl+C)"
          >
            <span>📋</span>
            <span>Copy</span>
          </button>

          {/* Cut */}
          <button
            onClick={() => {
              setClipboard({
                action: "cut",
                items: Array.from(selectedItems),
                sourceDir: currentPath,
              });
              showSuccess(`Cut ${selectedItems.size} item(s)`);
            }}
            className="p-1.5 px-2.5 rounded-lg bg-dark-800 hover:bg-dark-700 text-dark-100 flex items-center gap-1.5"
            title="Cut (Ctrl+X)"
          >
            <span>✂️</span>
            <span>Cut</span>
          </button>

          {/* Compress */}
          <button
            onClick={() => handleOpenCompress(Array.from(selectedItems))}
            className="p-1.5 px-2.5 rounded-lg bg-dark-800 hover:bg-dark-700 text-dark-100 flex items-center gap-1.5"
            title="Compress to ZIP"
          >
            <span>📦</span>
            <span>Compress</span>
          </button>

          {/* Delete */}
          <button
            onClick={() =>
              setDeleteConfirm({
                items: Array.from(selectedItems),
                isBulk: selectedItems.size > 1,
              })
            }
            className="p-1.5 px-2.5 rounded-lg bg-rose-500/20 hover:bg-rose-500/30 text-rose-300 border border-rose-500/40 flex items-center gap-1.5 font-bold"
            title="Delete (Delete key)"
          >
            <span>🗑️</span>
            <span>Delete</span>
          </button>
        </div>
      )}

      {/* Floating Right-Click Context Menu */}
      {contextMenu && (
        <div
          style={{ top: contextMenu.y, left: Math.min(contextMenu.x, window.innerWidth - 220) }}
          className="fixed z-50 bg-dark-900 border border-dark-600 rounded-xl shadow-2xl p-1.5 w-52 font-mono text-xs text-dark-200 select-none animate-scale-in"
          onClick={(e) => e.stopPropagation()}
        >
          {contextMenu.item ? (
            <>
              {contextMenu.item.is_dir ? (
                <button
                  onClick={() => {
                    navigateTo(contextMenu.item!.name);
                    setContextMenu(null);
                  }}
                  className="w-full text-left p-2 rounded-lg hover:bg-dark-700 hover:text-dark-50 flex items-center gap-2"
                >
                  <span>📂</span> Open Folder
                </button>
              ) : (
                <>
                  <button
                    onClick={() => {
                      handleOpenFile(contextMenu.item!);
                      setContextMenu(null);
                    }}
                    className="w-full text-left p-2 rounded-lg hover:bg-dark-700 hover:text-dark-50 flex items-center gap-2 font-bold text-dark-50"
                  >
                    <span>✏️</span> Edit / Open
                  </button>
                  <button
                    onClick={() => {
                      const path =
                        currentPath === "."
                          ? contextMenu.item!.name
                          : `${currentPath}/${contextMenu.item!.name}`;
                      triggerDownload(path, contextMenu.item!.name);
                      setContextMenu(null);
                    }}
                    className="w-full text-left p-2 rounded-lg hover:bg-dark-700 hover:text-dark-50 flex items-center gap-2"
                  >
                    <span>⬇️</span> Download
                  </button>
                </>
              )}

              {/* Archive Extract Option if ZIP or TAR */}
              {["zip", "tar", "gz", "tgz"].includes(
                contextMenu.item.name.split(".").pop()?.toLowerCase() || ""
              ) && (
                <button
                  onClick={() => {
                    handleOpenExtract(contextMenu.item!);
                    setContextMenu(null);
                  }}
                  className="w-full text-left p-2 rounded-lg hover:bg-dark-700 text-amber-300 flex items-center gap-2 font-bold"
                >
                  <span>📦</span> Extract Archive
                </button>
              )}

              <div className="my-1 h-px bg-dark-700" />

              <button
                onClick={() => {
                  setClipboard({
                    action: "copy",
                    items: [contextMenu.item!.name],
                    sourceDir: currentPath,
                  });
                  showSuccess(`Copied "${contextMenu.item!.name}"`);
                  setContextMenu(null);
                }}
                className="w-full text-left p-2 rounded-lg hover:bg-dark-700 hover:text-dark-50 flex items-center gap-2"
              >
                <span>📋</span> Copy
              </button>

              <button
                onClick={() => {
                  setClipboard({
                    action: "cut",
                    items: [contextMenu.item!.name],
                    sourceDir: currentPath,
                  });
                  showSuccess(`Cut "${contextMenu.item!.name}"`);
                  setContextMenu(null);
                }}
                className="w-full text-left p-2 rounded-lg hover:bg-dark-700 hover:text-dark-50 flex items-center gap-2"
              >
                <span>✂️</span> Cut
              </button>

              <button
                onClick={() => {
                  handleDuplicate(contextMenu.item!);
                  setContextMenu(null);
                }}
                className="w-full text-left p-2 rounded-lg hover:bg-dark-700 hover:text-dark-50 flex items-center gap-2"
              >
                <span>📑</span> Duplicate
              </button>

              <button
                onClick={() => {
                  setRenamingItem(contextMenu.item!);
                  setRenameValue(contextMenu.item!.name);
                  setContextMenu(null);
                }}
                className="w-full text-left p-2 rounded-lg hover:bg-dark-700 hover:text-dark-50 flex items-center gap-2"
              >
                <span>🏷️</span> Rename (F2)
              </button>

              <button
                onClick={() => {
                  handleOpenCompress([contextMenu.item!.name]);
                  setContextMenu(null);
                }}
                className="w-full text-left p-2 rounded-lg hover:bg-dark-700 hover:text-dark-50 flex items-center gap-2"
              >
                <span>📦</span> Compress
              </button>

              <div className="my-1 h-px bg-dark-700" />

              <button
                onClick={() => {
                  handleOpenChmod(contextMenu.item!);
                  setContextMenu(null);
                }}
                className="w-full text-left p-2 rounded-lg hover:bg-dark-700 hover:text-dark-50 flex items-center gap-2"
              >
                <span>🔒</span> Permissions (chmod)
              </button>

              <button
                onClick={() => {
                  handleInspectProperties(contextMenu.item!);
                  setContextMenu(null);
                }}
                className="w-full text-left p-2 rounded-lg hover:bg-dark-700 hover:text-dark-50 flex items-center gap-2"
              >
                <span>ℹ️</span> Properties
              </button>

              <button
                onClick={() => {
                  const full = `/var/www/${site?.domain}/${
                    currentPath === "." ? contextMenu.item!.name : `${currentPath}/${contextMenu.item!.name}`
                  }`;
                  navigator.clipboard.writeText(full);
                  showSuccess("Copied path to clipboard");
                  setContextMenu(null);
                }}
                className="w-full text-left p-2 rounded-lg hover:bg-dark-700 hover:text-dark-50 flex items-center gap-2"
              >
                <span>🔗</span> Copy Path
              </button>

              <div className="my-1 h-px bg-dark-700" />

              <button
                onClick={() => {
                  setDeleteConfirm({
                    items: [contextMenu.item!.name],
                    isBulk: false,
                  });
                  setContextMenu(null);
                }}
                className="w-full text-left p-2 rounded-lg hover:bg-rose-500/20 text-rose-400 flex items-center gap-2 font-bold"
              >
                <span>🗑️</span> Delete
              </button>
            </>
          ) : (
            /* Context Menu on empty space */
            <>
              <button
                onClick={() => {
                  setCreateType("file");
                  setCreateName("");
                  setShowCreate(true);
                  setContextMenu(null);
                }}
                className="w-full text-left p-2 rounded-lg hover:bg-dark-700 hover:text-dark-50 flex items-center gap-2"
              >
                <span>📄</span> New File
              </button>
              <button
                onClick={() => {
                  setCreateType("dir");
                  setCreateName("");
                  setShowCreate(true);
                  setContextMenu(null);
                }}
                className="w-full text-left p-2 rounded-lg hover:bg-dark-700 hover:text-dark-50 flex items-center gap-2"
              >
                <span>📁</span> New Folder
              </button>
              {clipboard && (
                <button
                  onClick={() => {
                    handlePaste();
                    setContextMenu(null);
                  }}
                  className="w-full text-left p-2 rounded-lg hover:bg-dark-700 text-rust-300 flex items-center gap-2 font-bold"
                >
                  <span>📋</span> Paste ({clipboard.items.length})
                </button>
              )}
              <div className="my-1 h-px bg-dark-700" />
              <button
                onClick={() => {
                  loadDir(currentPath);
                  setContextMenu(null);
                }}
                className="w-full text-left p-2 rounded-lg hover:bg-dark-700 hover:text-dark-50 flex items-center gap-2"
              >
                <span>🔄</span> Refresh
              </button>
            </>
          )}
        </div>
      )}

      {/* Upload Queue Drawer */}
      {showUploadDrawer && (
        <div className="fixed bottom-4 right-4 z-40 bg-dark-900 border border-dark-600 rounded-2xl shadow-2xl p-4 w-80 md:w-96 font-mono text-xs">
          <div className="flex items-center justify-between pb-2 border-b border-dark-700">
            <h4 className="font-bold text-dark-100 flex items-center gap-1.5">
              <span>📤</span> Upload Queue ({uploadQueue.length})
            </h4>
            <div className="flex items-center gap-2">
              <button
                onClick={() => setUploadQueue([])}
                className="text-dark-400 hover:text-dark-200 text-[10px]"
              >
                Clear
              </button>
              <button
                onClick={() => setShowUploadDrawer(false)}
                className="text-dark-400 hover:text-dark-200 font-bold"
              >
                ✕
              </button>
            </div>
          </div>

          {/* Conflict resolution option */}
          <div className="my-2 p-2 bg-dark-800 rounded-lg flex items-center justify-between text-[11px]">
            <span className="text-dark-300">Conflict:</span>
            <select
              value={uploadConflictAction}
              onChange={(e) => setUploadConflictAction(e.target.value as any)}
              className="bg-dark-700 text-dark-100 border border-dark-600 rounded px-1.5 py-0.5 text-[11px]"
            >
              <option value="overwrite">Overwrite</option>
              <option value="skip">Skip Existing</option>
              <option value="rename">Auto-Rename</option>
            </select>
          </div>

          <div className="max-h-56 overflow-y-auto space-y-2 mt-2">
            {uploadQueue.map((item) => (
              <div key={item.id} className="p-2 bg-dark-800 rounded-lg border border-dark-700">
                <div className="flex items-center justify-between text-[11px] mb-1">
                  <span className="truncate max-w-[180px] text-dark-100 font-medium">
                    {item.name}
                  </span>
                  <span className="text-dark-400">{formatSize(item.size)}</span>
                </div>
                <div className="w-full bg-dark-700 rounded-full h-1.5 overflow-hidden">
                  <div
                    className={`h-full transition-all duration-300 ${
                      item.status === "error"
                        ? "bg-rose-500"
                        : item.status === "completed"
                        ? "bg-emerald-500"
                        : "bg-rust-500 animate-pulse"
                    }`}
                    style={{ width: `${item.progress}%` }}
                  />
                </div>
                {item.error && <p className="text-[10px] text-rose-400 mt-1">{item.error}</p>}
              </div>
            ))}
          </div>
        </div>
      )}

      {/* Create File / Folder Dialog */}
      {showCreate && (
        <div className="fixed inset-0 z-50 bg-black/70 flex items-center justify-center p-4 backdrop-blur-xs">
          <div className="bg-dark-800 border border-dark-600 rounded-2xl max-w-md w-full shadow-2xl p-6 font-mono">
            <h3 className="text-base font-bold text-dark-50 mb-4 flex items-center gap-2">
              <span>{createType === "dir" ? "📁" : "📄"}</span>
              <span>Create New {createType === "dir" ? "Folder" : "File"}</span>
            </h3>

            <div className="flex items-center gap-3 mb-4">
              <label className="flex items-center gap-1.5 text-xs text-dark-200 cursor-pointer">
                <input
                  type="radio"
                  name="createType"
                  checked={createType === "file"}
                  onChange={() => setCreateType("file")}
                  className="text-rust-500"
                />
                <span>File</span>
              </label>
              <label className="flex items-center gap-1.5 text-xs text-dark-200 cursor-pointer">
                <input
                  type="radio"
                  name="createType"
                  checked={createType === "dir"}
                  onChange={() => setCreateType("dir")}
                  className="text-rust-500"
                />
                <span>Folder / Directory</span>
              </label>
            </div>

            <input
              type="text"
              value={createName}
              onChange={(e) => setCreateName(e.target.value)}
              placeholder={createType === "dir" ? "e.g. assets" : "e.g. index.php"}
              className="w-full bg-dark-900 border border-dark-600 rounded-xl px-4 py-2.5 text-sm text-dark-50 placeholder-dark-500 focus:outline-hidden focus:border-rust-500 mb-6"
              autoFocus
              onKeyDown={(e) => {
                if (e.key === "Enter") handleCreate();
              }}
            />

            <div className="flex items-center justify-end gap-2">
              <button
                onClick={() => setShowCreate(false)}
                className="px-4 py-2 bg-dark-700 hover:bg-dark-600 text-dark-200 rounded-xl text-xs font-medium"
              >
                Cancel
              </button>
              <button
                onClick={handleCreate}
                disabled={!createName.trim()}
                className="px-4 py-2 bg-rust-500 hover:bg-rust-600 text-white rounded-xl text-xs font-bold disabled:opacity-50"
              >
                Create
              </button>
            </div>
          </div>
        </div>
      )}

      {/* Rename Dialog */}
      {renamingItem && (
        <div className="fixed inset-0 z-50 bg-black/70 flex items-center justify-center p-4 backdrop-blur-xs">
          <div className="bg-dark-800 border border-dark-600 rounded-2xl max-w-md w-full shadow-2xl p-6 font-mono">
            <h3 className="text-base font-bold text-dark-50 mb-2">Rename Item</h3>
            <p className="text-xs text-dark-300 mb-4">Original: {renamingItem.name}</p>

            <input
              type="text"
              value={renameValue}
              onChange={(e) => setRenameValue(e.target.value)}
              className="w-full bg-dark-900 border border-dark-600 rounded-xl px-4 py-2.5 text-sm text-dark-50 placeholder-dark-500 focus:outline-hidden focus:border-rust-500 mb-6"
              autoFocus
              onKeyDown={(e) => {
                if (e.key === "Enter") handleRename();
              }}
            />

            <div className="flex items-center justify-end gap-2">
              <button
                onClick={() => setRenamingItem(null)}
                className="px-4 py-2 bg-dark-700 hover:bg-dark-600 text-dark-200 rounded-xl text-xs font-medium"
              >
                Cancel
              </button>
              <button
                onClick={handleRename}
                disabled={!renameValue.trim() || renameValue === renamingItem.name}
                className="px-4 py-2 bg-rust-500 hover:bg-rust-600 text-white rounded-xl text-xs font-bold disabled:opacity-50"
              >
                Rename
              </button>
            </div>
          </div>
        </div>
      )}

      {/* Delete Confirmation Modal */}
      {deleteConfirm && (
        <div className="fixed inset-0 z-50 bg-black/70 flex items-center justify-center p-4 backdrop-blur-xs">
          <div className="bg-dark-800 border border-rose-500/30 rounded-2xl max-w-md w-full shadow-2xl p-6 font-mono">
            <div className="flex items-center gap-3 mb-3">
              <span className="text-2xl">⚠️</span>
              <h3 className="text-base font-bold text-rose-400">Confirm Deletion</h3>
            </div>
            <p className="text-xs text-dark-200 mb-4">
              Are you sure you want to permanently delete{" "}
              {deleteConfirm.isBulk
                ? `${deleteConfirm.items.length} selected items?`
                : `"${deleteConfirm.items[0]}"?`}
            </p>
            <p className="text-[11px] text-dark-400 mb-6 bg-dark-900 p-2.5 rounded-lg border border-dark-700">
              This action cannot be undone.
            </p>

            <div className="flex items-center justify-end gap-2">
              <button
                onClick={() => setDeleteConfirm(null)}
                className="px-4 py-2 bg-dark-700 hover:bg-dark-600 text-dark-200 rounded-xl text-xs font-medium"
              >
                Cancel
              </button>
              <button
                onClick={handleDelete}
                className="px-4 py-2 bg-rose-600 hover:bg-rose-500 text-white rounded-xl text-xs font-bold"
              >
                Delete Permanently
              </button>
            </div>
          </div>
        </div>
      )}

      {/* Code Editor Modal */}
      {editingFile && (
        <div
          className={`fixed z-50 bg-black/80 flex items-center justify-center p-2 sm:p-4 backdrop-blur-xs ${
            editorFullScreen ? "inset-0 !p-0" : "inset-0"
          }`}
        >
          <div
            className={`bg-dark-900 border border-dark-600 shadow-2xl flex flex-col font-mono overflow-hidden ${
              editorFullScreen ? "w-full h-full rounded-none" : "w-full max-w-5xl h-[85vh] rounded-2xl"
            }`}
          >
            {/* Editor Header */}
            <div className="p-3 px-4 bg-dark-800 border-b border-dark-700 flex items-center justify-between">
              <div className="flex items-center gap-2">
                <span>✏️</span>
                <span className="font-bold text-dark-50 text-sm">{editingFile.name}</span>
                <span className="text-xs text-dark-400 hidden sm:inline">({editingFile.path})</span>
              </div>

              <div className="flex items-center gap-2">
                <button
                  onClick={() => setShowEditorSearch(!showEditorSearch)}
                  className="px-2.5 py-1 bg-dark-700 hover:bg-dark-600 text-dark-200 rounded text-xs"
                  title="Find & Replace (Ctrl+F)"
                >
                  🔍 Find
                </button>
                <button
                  onClick={() => setEditorWrap(!editorWrap)}
                  className={`px-2.5 py-1 rounded text-xs ${
                    editorWrap ? "bg-rust-500/20 text-rust-300 font-bold" : "bg-dark-700 text-dark-300"
                  }`}
                  title="Toggle Word Wrap"
                >
                  Wrap
                </button>
                <button
                  onClick={() => setEditorFullScreen(!editorFullScreen)}
                  className="px-2.5 py-1 bg-dark-700 hover:bg-dark-600 text-dark-200 rounded text-xs"
                  title="Full Screen"
                >
                  {editorFullScreen ? "🗗 Exit Fullscreen" : "🗖 Fullscreen"}
                </button>
                <button
                  onClick={handleSaveEditor}
                  disabled={editorSaving}
                  className="px-4 py-1 bg-rust-500 hover:bg-rust-600 text-white rounded text-xs font-bold shadow flex items-center gap-1"
                >
                  {editorSaving ? "Saving..." : "Save (Ctrl+S)"}
                </button>
                <button
                  onClick={() => setEditingFile(null)}
                  className="px-2.5 py-1 bg-dark-700 hover:bg-dark-600 text-dark-200 rounded text-xs font-bold"
                >
                  ✕
                </button>
              </div>
            </div>

            {/* In-Editor Search / Replace Bar */}
            {showEditorSearch && (
              <div className="p-2 bg-dark-850 border-b border-dark-700 flex flex-wrap items-center gap-2 text-xs">
                <input
                  type="text"
                  placeholder="Find..."
                  value={editorSearch}
                  onChange={(e) => setEditorSearch(e.target.value)}
                  className="bg-dark-800 border border-dark-600 rounded px-2.5 py-1 text-dark-50 text-xs w-48"
                />
                <input
                  type="text"
                  placeholder="Replace with..."
                  value={editorReplace}
                  onChange={(e) => setEditorReplace(e.target.value)}
                  className="bg-dark-800 border border-dark-600 rounded px-2.5 py-1 text-dark-50 text-xs w-48"
                />
                <button
                  onClick={() => {
                    if (!editorSearch) return;
                    setEditingFile((prev) =>
                      prev
                        ? {
                            ...prev,
                            content: prev.content.replaceAll(editorSearch, editorReplace),
                          }
                        : null
                    );
                    showSuccess("Replaced all occurrences");
                  }}
                  className="px-3 py-1 bg-dark-700 hover:bg-dark-600 text-dark-100 rounded"
                >
                  Replace All
                </button>
              </div>
            )}

            {/* Editor Textarea */}
            <div className="flex-1 relative flex overflow-hidden">
              <textarea
                value={editingFile.content}
                onChange={(e) =>
                  setEditingFile((prev) => (prev ? { ...prev, content: e.target.value } : null))
                }
                wrap={editorWrap ? "soft" : "off"}
                className="w-full h-full p-4 bg-dark-950 text-dark-50 font-mono text-sm leading-relaxed resize-none focus:outline-hidden selection:bg-rust-500/30 overflow-auto"
                spellCheck={false}
                onKeyDown={(e) => {
                  if ((e.ctrlKey || e.metaKey) && e.key.toLowerCase() === "s") {
                    e.preventDefault();
                    handleSaveEditor();
                  }
                }}
              />
            </div>

            {/* Editor Status Bar */}
            <div className="p-2 px-4 bg-dark-800 border-t border-dark-700 text-[11px] text-dark-400 flex items-center justify-between">
              <div>
                <span>Lines: {editingFile.content.split("\n").length}</span>
                <span className="mx-2">•</span>
                <span>Length: {editingFile.content.length} chars</span>
              </div>
              <div>UTF-8</div>
            </div>
          </div>
        </div>
      )}

      {/* Rich Preview Modal */}
      {previewFile && (
        <div className="fixed inset-0 z-50 bg-black/80 flex items-center justify-center p-4 backdrop-blur-xs">
          <div className="bg-dark-900 border border-dark-600 rounded-2xl max-w-4xl w-full max-h-[85vh] shadow-2xl flex flex-col font-mono overflow-hidden">
            <div className="p-3 px-4 bg-dark-800 border-b border-dark-700 flex items-center justify-between">
              <span className="font-bold text-dark-50 text-sm flex items-center gap-2">
                <span>👁️</span> Preview: {previewFile.name}
              </span>
              <button
                onClick={() => setPreviewFile(null)}
                className="px-2.5 py-1 bg-dark-700 hover:bg-dark-600 text-dark-200 rounded text-xs font-bold"
              >
                ✕
              </button>
            </div>

            <div className="p-6 overflow-auto flex-1 flex flex-col items-center justify-center">
              {previewFile.type === "image" && (
                <img
                  src={`/api/sites/${id}/files/download?path=${encodeURIComponent(previewFile.path)}`}
                  alt={previewFile.name}
                  className="max-h-[60vh] max-w-full object-contain rounded-lg border border-dark-700"
                />
              )}

              {previewFile.type === "audio" && (
                <audio
                  controls
                  className="w-full max-w-md"
                  src={`/api/sites/${id}/files/download?path=${encodeURIComponent(previewFile.path)}`}
                />
              )}

              {previewFile.type === "video" && (
                <video
                  controls
                  className="max-h-[60vh] max-w-full rounded-lg"
                  src={`/api/sites/${id}/files/download?path=${encodeURIComponent(previewFile.path)}`}
                />
              )}

              {previewFile.type === "pdf" && (
                <iframe
                  title="PDF Preview"
                  className="w-full h-[60vh] rounded-lg border border-dark-700"
                  src={`/api/sites/${id}/files/download?path=${encodeURIComponent(previewFile.path)}`}
                />
              )}

              {previewFile.type === "markdown" && (
                <div
                  className="w-full h-full p-4 prose prose-invert max-w-none overflow-auto font-sans"
                  dangerouslySetInnerHTML={{
                    __html: DOMPurify.sanitize(marked.parse(previewFile.content || "") as string),
                  }}
                />
              )}

              {previewFile.type === "archive" && (
                <div className="w-full text-left">
                  <h4 className="text-xs font-bold text-dark-200 mb-2">
                    Archive Contents ({previewFile.archiveEntries?.length || 0} entries)
                  </h4>
                  <div className="max-h-96 overflow-y-auto divide-y divide-dark-800 border border-dark-700 rounded-xl bg-dark-950 p-2">
                    {previewFile.archiveEntries?.map((entry, idx) => (
                      <div key={idx} className="p-2 flex items-center justify-between text-xs">
                        <span className="text-dark-100 flex items-center gap-1.5 truncate">
                          <span>{entry.is_dir ? "📁" : "📄"}</span>
                          <span>{entry.name}</span>
                        </span>
                        <span className="text-dark-400">{formatSize(entry.size)}</span>
                      </div>
                    ))}
                  </div>
                </div>
              )}

              {previewFile.type === "other" && (
                <div className="text-center p-8">
                  <span className="text-4xl block mb-2">📦</span>
                  <p className="text-sm text-dark-200">Binary file cannot be previewed in text mode.</p>
                  <button
                    onClick={() => triggerDownload(previewFile.path, previewFile.name)}
                    className="mt-4 px-4 py-2 bg-rust-500 hover:bg-rust-600 text-white rounded-xl text-xs font-bold"
                  >
                    Download File ({previewFile.name})
                  </button>
                </div>
              )}
            </div>
          </div>
        </div>
      )}

      {/* Permissions (Chmod) Modal */}
      {chmodModal && (
        <div className="fixed inset-0 z-50 bg-black/70 flex items-center justify-center p-4 backdrop-blur-xs">
          <div className="bg-dark-800 border border-dark-600 rounded-2xl max-w-md w-full shadow-2xl p-6 font-mono">
            <h3 className="text-base font-bold text-dark-50 mb-1">File Permissions (chmod)</h3>
            <p className="text-xs text-dark-300 mb-4">{chmodModal.name}</p>

            {/* Grid for Owner, Group, Others */}
            <div className="grid grid-cols-4 gap-2 text-center text-xs mb-4">
              <div className="font-bold text-dark-400 text-left">Role</div>
              <div className="font-bold text-dark-300">Read (4)</div>
              <div className="font-bold text-dark-300">Write (2)</div>
              <div className="font-bold text-dark-300">Exec (1)</div>

              {/* Owner */}
              <div className="text-left font-bold text-dark-100">Owner</div>
              <div>
                <input
                  type="checkbox"
                  checked={chmodModal.ownerR}
                  onChange={(e) => setChmodModal({ ...chmodModal, ownerR: e.target.checked })}
                  className="rounded bg-dark-700 border-dark-500 text-rust-500"
                />
              </div>
              <div>
                <input
                  type="checkbox"
                  checked={chmodModal.ownerW}
                  onChange={(e) => setChmodModal({ ...chmodModal, ownerW: e.target.checked })}
                  className="rounded bg-dark-700 border-dark-500 text-rust-500"
                />
              </div>
              <div>
                <input
                  type="checkbox"
                  checked={chmodModal.ownerX}
                  onChange={(e) => setChmodModal({ ...chmodModal, ownerX: e.target.checked })}
                  className="rounded bg-dark-700 border-dark-500 text-rust-500"
                />
              </div>

              {/* Group */}
              <div className="text-left font-bold text-dark-100">Group</div>
              <div>
                <input
                  type="checkbox"
                  checked={chmodModal.groupR}
                  onChange={(e) => setChmodModal({ ...chmodModal, groupR: e.target.checked })}
                  className="rounded bg-dark-700 border-dark-500 text-rust-500"
                />
              </div>
              <div>
                <input
                  type="checkbox"
                  checked={chmodModal.groupW}
                  onChange={(e) => setChmodModal({ ...chmodModal, groupW: e.target.checked })}
                  className="rounded bg-dark-700 border-dark-500 text-rust-500"
                />
              </div>
              <div>
                <input
                  type="checkbox"
                  checked={chmodModal.groupX}
                  onChange={(e) => setChmodModal({ ...chmodModal, groupX: e.target.checked })}
                  className="rounded bg-dark-700 border-dark-500 text-rust-500"
                />
              </div>

              {/* Others */}
              <div className="text-left font-bold text-dark-100">Others</div>
              <div>
                <input
                  type="checkbox"
                  checked={chmodModal.otherR}
                  onChange={(e) => setChmodModal({ ...chmodModal, otherR: e.target.checked })}
                  className="rounded bg-dark-700 border-dark-500 text-rust-500"
                />
              </div>
              <div>
                <input
                  type="checkbox"
                  checked={chmodModal.otherW}
                  onChange={(e) => setChmodModal({ ...chmodModal, otherW: e.target.checked })}
                  className="rounded bg-dark-700 border-dark-500 text-rust-500"
                />
              </div>
              <div>
                <input
                  type="checkbox"
                  checked={chmodModal.otherX}
                  onChange={(e) => setChmodModal({ ...chmodModal, otherX: e.target.checked })}
                  className="rounded bg-dark-700 border-dark-500 text-rust-500"
                />
              </div>
            </div>

            {/* Calculated octal display */}
            <div className="p-3 bg-dark-900 rounded-xl border border-dark-700 flex items-center justify-between mb-4">
              <span className="text-xs text-dark-300">Octal Mode:</span>
              <span className="text-base font-bold text-rust-400">
                0
                {(chmodModal.ownerR ? 4 : 0) + (chmodModal.ownerW ? 2 : 0) + (chmodModal.ownerX ? 1 : 0)}
                {(chmodModal.groupR ? 4 : 0) + (chmodModal.groupW ? 2 : 0) + (chmodModal.groupX ? 1 : 0)}
                {(chmodModal.otherR ? 4 : 0) + (chmodModal.otherW ? 2 : 0) + (chmodModal.otherX ? 1 : 0)}
              </span>
            </div>

            <label className="flex items-center gap-2 text-xs text-dark-200 mb-6 cursor-pointer">
              <input
                type="checkbox"
                checked={chmodModal.recursive}
                onChange={(e) => setChmodModal({ ...chmodModal, recursive: e.target.checked })}
                className="rounded bg-dark-700 border-dark-500 text-rust-500"
              />
              <span>Apply recursively to subdirectories and files</span>
            </label>

            <div className="flex items-center justify-end gap-2">
              <button
                onClick={() => setChmodModal(null)}
                className="px-4 py-2 bg-dark-700 hover:bg-dark-600 text-dark-200 rounded-xl text-xs font-medium"
              >
                Cancel
              </button>
              <button
                onClick={handleApplyChmod}
                className="px-4 py-2 bg-rust-500 hover:bg-rust-600 text-white rounded-xl text-xs font-bold"
              >
                Apply Permissions
              </button>
            </div>
          </div>
        </div>
      )}

      {/* Properties & Checksums Modal */}
      {statFile && (
        <div className="fixed inset-0 z-50 bg-black/70 flex items-center justify-center p-4 backdrop-blur-xs">
          <div className="bg-dark-800 border border-dark-600 rounded-2xl max-w-lg w-full shadow-2xl p-6 font-mono text-xs">
            <div className="flex items-center justify-between pb-3 mb-4 border-b border-dark-700">
              <h3 className="text-base font-bold text-dark-50 flex items-center gap-2">
                <span>ℹ️</span> Item Properties
              </h3>
              <button onClick={() => setStatFile(null)} className="text-dark-400 hover:text-dark-200 font-bold">
                ✕
              </button>
            </div>

            <div className="space-y-2.5">
              <div className="flex justify-between py-1 border-b border-dark-750">
                <span className="text-dark-400">Name:</span>
                <span className="text-dark-100 font-bold truncate max-w-[280px]">{statFile.name}</span>
              </div>
              <div className="flex justify-between py-1 border-b border-dark-750">
                <span className="text-dark-400">Path:</span>
                <span className="text-dark-200 truncate max-w-[280px]">{statFile.path}</span>
              </div>
              <div className="flex justify-between py-1 border-b border-dark-750">
                <span className="text-dark-400">MIME Type:</span>
                <span className="text-dark-200">{statFile.mime_type}</span>
              </div>
              <div className="flex justify-between py-1 border-b border-dark-750">
                <span className="text-dark-400">Size:</span>
                <span className="text-dark-100 font-bold">
                  {formatSize(statFile.size)} ({statFile.size.toLocaleString()} bytes)
                </span>
              </div>
              <div className="flex justify-between py-1 border-b border-dark-750">
                <span className="text-dark-400">Permissions:</span>
                <span className="text-rust-400 font-bold">
                  {statFile.permissions_octal} ({statFile.permissions_symbolic})
                </span>
              </div>
              <div className="flex justify-between py-1 border-b border-dark-750">
                <span className="text-dark-400">Owner / Group:</span>
                <span className="text-dark-200">
                  {statFile.owner} / {statFile.group}
                </span>
              </div>
              <div className="flex justify-between py-1 border-b border-dark-750">
                <span className="text-dark-400">Modified:</span>
                <span className="text-dark-200">{formatDate(statFile.modified)}</span>
              </div>
              {statFile.sha256 && (
                <div className="py-1">
                  <span className="text-dark-400 block mb-1">SHA-256 Checksum:</span>
                  <div className="p-2 bg-dark-900 rounded border border-dark-700 text-[10px] break-all text-emerald-400 select-all">
                    {statFile.sha256}
                  </div>
                </div>
              )}
            </div>

            <div className="mt-6 flex justify-end">
              <button
                onClick={() => setStatFile(null)}
                className="px-4 py-2 bg-dark-700 hover:bg-dark-600 text-dark-100 rounded-xl font-medium"
              >
                Close
              </button>
            </div>
          </div>
        </div>
      )}

      {/* Compress Modal */}
      {compressModal && (
        <div className="fixed inset-0 z-50 bg-black/70 flex items-center justify-center p-4 backdrop-blur-xs">
          <div className="bg-dark-800 border border-dark-600 rounded-2xl max-w-md w-full shadow-2xl p-6 font-mono">
            <h3 className="text-base font-bold text-dark-50 mb-1">Compress to Archive</h3>
            <p className="text-xs text-dark-300 mb-4">
              Compressing {compressModal.sources.length} item(s)
            </p>

            <label className="text-xs text-dark-300 block mb-1">Archive Name (.zip or .tar.gz):</label>
            <input
              type="text"
              value={compressModal.archiveName}
              onChange={(e) => setCompressModal({ ...compressModal, archiveName: e.target.value })}
              className="w-full bg-dark-900 border border-dark-600 rounded-xl px-4 py-2 text-sm text-dark-50 mb-6"
            />

            <div className="flex items-center justify-end gap-2">
              <button
                onClick={() => setCompressModal(null)}
                className="px-4 py-2 bg-dark-700 hover:bg-dark-600 text-dark-200 rounded-xl text-xs font-medium"
              >
                Cancel
              </button>
              <button
                onClick={handleApplyCompress}
                className="px-4 py-2 bg-rust-500 hover:bg-rust-600 text-white rounded-xl text-xs font-bold"
              >
                Compress
              </button>
            </div>
          </div>
        </div>
      )}

      {/* Extract Modal */}
      {extractModal && (
        <div className="fixed inset-0 z-50 bg-black/70 flex items-center justify-center p-4 backdrop-blur-xs">
          <div className="bg-dark-800 border border-dark-600 rounded-2xl max-w-md w-full shadow-2xl p-6 font-mono">
            <h3 className="text-base font-bold text-dark-50 mb-1">Extract Archive</h3>
            <p className="text-xs text-dark-300 mb-4">{extractModal.archiveName}</p>

            <label className="text-xs text-dark-300 block mb-1">Destination Directory:</label>
            <input
              type="text"
              value={extractModal.targetDir}
              onChange={(e) => setExtractModal({ ...extractModal, targetDir: e.target.value })}
              className="w-full bg-dark-900 border border-dark-600 rounded-xl px-4 py-2 text-sm text-dark-50 mb-4"
            />

            <label className="flex items-center gap-2 text-xs text-dark-200 mb-6 cursor-pointer">
              <input
                type="checkbox"
                checked={extractModal.overwrite}
                onChange={(e) => setExtractModal({ ...extractModal, overwrite: e.target.checked })}
                className="rounded bg-dark-700 border-dark-500 text-rust-500"
              />
              <span>Overwrite existing files</span>
            </label>

            <div className="flex items-center justify-end gap-2">
              <button
                onClick={() => setExtractModal(null)}
                className="px-4 py-2 bg-dark-700 hover:bg-dark-600 text-dark-200 rounded-xl text-xs font-medium"
              >
                Cancel
              </button>
              <button
                onClick={handleApplyExtract}
                className="px-4 py-2 bg-rust-500 hover:bg-rust-600 text-white rounded-xl text-xs font-bold"
              >
                Extract Here
              </button>
            </div>
          </div>
        </div>
      )}

      {/* Search Modal */}
      {showSearch && (
        <div className="fixed inset-0 z-50 bg-black/70 flex items-center justify-center p-4 backdrop-blur-xs">
          <div className="bg-dark-800 border border-dark-600 rounded-2xl max-w-2xl w-full max-h-[85vh] shadow-2xl p-6 font-mono flex flex-col">
            <div className="flex items-center justify-between pb-3 mb-4 border-b border-dark-700">
              <h3 className="text-base font-bold text-dark-50 flex items-center gap-2">
                <span>🔍</span> Search Files
              </h3>
              <button onClick={() => setShowSearch(false)} className="text-dark-400 hover:text-dark-200 font-bold">
                ✕
              </button>
            </div>

            <div className="flex items-center gap-2 mb-3">
              <input
                type="text"
                placeholder="Search file name or contents..."
                value={searchQuery}
                onChange={(e) => setSearchQuery(e.target.value)}
                onKeyDown={(e) => {
                  if (e.key === "Enter") handleExecuteSearch();
                }}
                className="flex-1 bg-dark-900 border border-dark-600 rounded-xl px-4 py-2 text-sm text-dark-50 focus:outline-hidden focus:border-rust-500"
                autoFocus
              />
              <button
                onClick={handleExecuteSearch}
                disabled={searching || !searchQuery.trim()}
                className="px-4 py-2 bg-rust-500 hover:bg-rust-600 text-white rounded-xl text-xs font-bold disabled:opacity-50"
              >
                {searching ? "Searching..." : "Search"}
              </button>
            </div>

            <div className="flex items-center gap-2 mb-4 text-xs">
              <label className="flex items-center gap-1.5 text-dark-300 cursor-pointer">
                <input
                  type="checkbox"
                  checked={searchInContent}
                  onChange={(e) => setSearchInContent(e.target.checked)}
                  className="rounded bg-dark-700 border-dark-500 text-rust-500"
                />
                <span>Search inside file contents (grep)</span>
              </label>
            </div>

            {/* Results */}
            <div className="flex-1 overflow-y-auto divide-y divide-dark-700 border border-dark-700 rounded-xl bg-dark-900 p-2">
              {searchResults.length === 0 ? (
                <div className="p-8 text-center text-xs text-dark-400">
                  {searching ? "Scanning files..." : "No results yet. Type a keyword and click Search."}
                </div>
              ) : (
                searchResults.map((res, idx) => (
                  <div
                    key={idx}
                    onClick={() => {
                      setShowSearch(false);
                      if (res.is_dir) {
                        loadDir(res.path);
                      } else {
                        const parent = res.path.split("/").slice(0, -1).join("/") || ".";
                        loadDir(parent);
                      }
                    }}
                    className="p-2.5 hover:bg-dark-800 rounded-lg cursor-pointer transition-colors"
                  >
                    <div className="flex items-center justify-between text-xs">
                      <span className="font-bold text-dark-100 flex items-center gap-1.5">
                        <span>{res.is_dir ? "📁" : "📄"}</span>
                        <span>{res.name}</span>
                      </span>
                      <span className="text-dark-400 text-[11px]">{formatSize(res.size)}</span>
                    </div>
                    <p className="text-[11px] text-dark-400 truncate mt-0.5">{res.path}</p>

                    {res.line_matches?.length > 0 && (
                      <div className="mt-2 space-y-1 bg-dark-950 p-2 rounded border border-dark-800">
                        {res.line_matches.map((match, mIdx) => (
                          <div key={mIdx} className="text-[10px] text-amber-300/90 truncate">
                            <span className="text-dark-500 mr-2 font-bold">L{match.line_number}:</span>
                            <span>{match.line_content}</span>
                          </div>
                        ))}
                      </div>
                    )}
                  </div>
                ))
              )}
            </div>
          </div>
        </div>
      )}

      {/* Storage Breakdown Drawer */}
      {showStorage && (
        <div className="fixed inset-0 z-50 bg-black/70 flex items-center justify-center p-4 backdrop-blur-xs">
          <div className="bg-dark-800 border border-dark-600 rounded-2xl max-w-lg w-full shadow-2xl p-6 font-mono text-xs flex flex-col">
            <div className="flex items-center justify-between pb-3 mb-4 border-b border-dark-700">
              <h3 className="text-base font-bold text-dark-50 flex items-center gap-2">
                <span>📊</span> Directory Storage Breakdown
              </h3>
              <button onClick={() => setShowStorage(false)} className="text-dark-400 hover:text-dark-200 font-bold">
                ✕
              </button>
            </div>

            {loadingStorage ? (
              <div className="p-8 text-center text-dark-300">Calculating directory sizes...</div>
            ) : storageStats ? (
              <>
                <div className="p-4 bg-dark-900 rounded-xl border border-dark-700 mb-4 flex justify-between items-center">
                  <div>
                    <p className="text-dark-400 text-[11px]">Total Directory Usage</p>
                    <p className="text-lg font-bold text-rust-400">{formatSize(storageStats.total_bytes)}</p>
                  </div>
                  <div className="text-right text-dark-300 text-[11px]">
                    <p>{storageStats.file_count} files</p>
                    <p>{storageStats.dir_count} directories</p>
                  </div>
                </div>

                <div className="max-h-64 overflow-y-auto space-y-1.5 divide-y divide-dark-750">
                  {storageStats.breakdown.map((item, i) => (
                    <div key={i} className="pt-1.5 flex items-center justify-between">
                      <span className="text-dark-200 truncate max-w-[260px] flex items-center gap-1">
                        <span>{item.is_dir ? "📁" : "📄"}</span>
                        <span>{item.name}</span>
                      </span>
                      <span className="font-bold text-dark-100">{formatSize(item.size)}</span>
                    </div>
                  ))}
                </div>
              </>
            ) : null}

            <div className="mt-6 flex justify-end">
              <button
                onClick={() => setShowStorage(false)}
                className="px-4 py-2 bg-dark-700 hover:bg-dark-600 text-dark-100 rounded-xl font-medium"
              >
                Close
              </button>
            </div>
          </div>
        </div>
      )}

      {/* Static Website Upgrade Wizard Modal */}
      {showUpgradeWizard && (
        <div className="fixed inset-0 z-50 bg-black/75 flex items-center justify-center p-4 backdrop-blur-xs">
          <div className="bg-dark-800 border border-rust-500/30 rounded-2xl max-w-xl w-full shadow-2xl p-6 font-mono text-xs">
            <div className="flex items-center justify-between pb-3 mb-4 border-b border-dark-700">
              <h3 className="text-base font-bold text-dark-50 flex items-center gap-2">
                <span>🚀</span> Static Website Upgrade Wizard
              </h3>
              <button
                onClick={() => setShowUpgradeWizard(false)}
                disabled={upgradeWorking}
                className="text-dark-400 hover:text-dark-200 font-bold"
              >
                ✕
              </button>
            </div>

            {upgradeStep === 1 && (
              <div>
                <p className="text-dark-200 mb-4 leading-relaxed">
                  Upgrade your website in one automated flow: upload a ZIP archive of your new build, and DockPanel will safely extract, verify, and deploy it to your webroot.
                </p>

                <div
                  onClick={() => fileInputRef.current?.click()}
                  className="p-8 border-2 border-dashed border-dark-600 hover:border-rust-500 rounded-2xl text-center cursor-pointer bg-dark-900/50 mb-4 transition-colors"
                >
                  <span className="text-4xl block mb-2">📦</span>
                  <p className="text-sm font-bold text-dark-100">
                    {upgradeFile ? upgradeFile.name : "Select or Drop your website.zip"}
                  </p>
                  <p className="text-[11px] text-dark-400 mt-1">
                    {upgradeFile ? formatSize(upgradeFile.size) : "Standard ZIP containing index.html / build output"}
                  </p>
                </div>

                <input
                  type="file"
                  accept=".zip,.tar.gz,.tgz"
                  className="hidden"
                  onChange={(e) => {
                    if (e.target.files && e.target.files[0]) {
                      setUpgradeFile(e.target.files[0]);
                    }
                  }}
                />

                <label className="flex items-center gap-2 text-dark-200 mb-6 cursor-pointer">
                  <input
                    type="checkbox"
                    checked={upgradeBackupFirst}
                    onChange={(e) => setUpgradeBackupFirst(e.target.checked)}
                    className="rounded bg-dark-700 border-dark-500 text-rust-500"
                  />
                  <span>Create automated backup snapshot before deploying</span>
                </label>

                <div className="flex justify-end gap-2">
                  <button
                    onClick={() => setShowUpgradeWizard(false)}
                    className="px-4 py-2 bg-dark-700 hover:bg-dark-600 text-dark-200 rounded-xl"
                  >
                    Cancel
                  </button>
                  <button
                    onClick={handleExecuteSiteUpgrade}
                    disabled={!upgradeFile || upgradeWorking}
                    className="px-4 py-2 bg-gradient-to-r from-rust-500 to-amber-500 hover:from-rust-600 hover:to-amber-600 text-white font-bold rounded-xl disabled:opacity-50"
                  >
                    {upgradeWorking ? "Upgrading..." : "Deploy Website"}
                  </button>
                </div>
              </div>
            )}

            {upgradeWorking && (
              <div className="p-8 text-center space-y-4">
                <div className="w-10 h-10 border-3 border-dark-600 border-t-rust-500 rounded-full animate-spin mx-auto" />
                <p className="text-sm font-bold text-dark-50">{upgradeStatus}</p>
                <p className="text-dark-400 text-[11px]">Please do not close this window...</p>
              </div>
            )}

            {upgradeStep === 4 && (
              <div className="p-6 text-center space-y-4">
                <span className="text-5xl block">🎉</span>
                <h4 className="text-base font-bold text-emerald-400">Website Successfully Deployed!</h4>
                <p className="text-dark-200 text-xs">
                  Your new files are now live on{" "}
                  <a
                    href={`http://${site?.domain}`}
                    target="_blank"
                    rel="noreferrer"
                    className="text-rust-400 underline font-bold"
                  >
                    {site?.domain}
                  </a>
                </p>
                <div className="pt-4 flex justify-center">
                  <button
                    onClick={() => {
                      setShowUpgradeWizard(false);
                      loadDir(".");
                    }}
                    className="px-6 py-2 bg-rust-500 hover:bg-rust-600 text-white font-bold rounded-xl"
                  >
                    Done
                  </button>
                </div>
              </div>
            )}
          </div>
        </div>
      )}
    </div>
  );
}

// Tree node helper
function renderTreeNodes(
  parentPath: string,
  folders: FileEntry[],
  currentPath: string,
  expandedFolders: Set<string>,
  toggleExpand: (path: string, e: React.MouseEvent) => void,
  loadDir: (path: string) => void
) {
  return folders.map((folder) => {
    const fullPath = parentPath === "." ? folder.name : `${parentPath}/${folder.name}`;
    const isExpanded = expandedFolders.has(fullPath);
    const isCurrent = currentPath === fullPath;

    return (
      <div key={fullPath} className="ml-2">
        <div
          onClick={() => loadDir(fullPath)}
          className={`flex items-center gap-1.5 p-1 rounded-lg cursor-pointer transition-colors ${
            isCurrent ? "bg-rust-500/20 text-rust-300 font-bold" : "text-dark-300 hover:bg-dark-700 hover:text-dark-100"
          }`}
        >
          <span
            onClick={(e) => toggleExpand(fullPath, e)}
            className="text-[10px] w-4 text-center text-dark-500 hover:text-dark-200 select-none"
          >
            {isExpanded ? "▼" : "▶"}
          </span>
          <span>📁</span>
          <span className="truncate">{folder.name}</span>
        </div>
      </div>
    );
  });
}
