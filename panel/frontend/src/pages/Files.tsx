import { useState, useEffect, useCallback } from "react";
import { useParams, Link } from "react-router-dom";
import { api } from "../api";

interface FileEntry {
  name: string;
  is_dir: boolean;
  size: number;
  modified: string;
}

interface Site {
  id: string;
  domain: string;
}

function formatSize(bytes: number): string {
  if (bytes === 0) return "—";
  const units = ["B", "KB", "MB", "GB"];
  const i = Math.floor(Math.log(bytes) / Math.log(1024));
  return `${(bytes / Math.pow(1024, i)).toFixed(i > 0 ? 1 : 0)} ${units[i]}`;
}

function formatDate(iso: string): string {
  if (!iso) return "—";
  return new Date(iso).toLocaleString(undefined, {
    month: "short",
    day: "numeric",
    hour: "2-digit",
    minute: "2-digit",
  });
}

export default function Files() {
  const { id } = useParams<{ id: string }>();
  const [site, setSite] = useState<Site | null>(null);
  const [entries, setEntries] = useState<FileEntry[]>([]);
  const [currentPath, setCurrentPath] = useState(".");
  const [loading, setLoading] = useState(true);
  const [error, setError] = useState("");
  // Editor
  const [editing, setEditing] = useState<string | null>(null);
  const [editorContent, setEditorContent] = useState("");
  const [saving, setSaving] = useState(false);
  const [saveSuccess, setSaveSuccess] = useState(false);
  // Create dialog
  const [showCreate, setShowCreate] = useState(false);
  const [createName, setCreateName] = useState("");
  const [createType, setCreateType] = useState<"file" | "dir">("file");
  // Rename dialog
  const [renaming, setRenaming] = useState<string | null>(null);
  const [renameTo, setRenameTo] = useState("");
  // Delete confirm
  const [deleteTarget, setDeleteTarget] = useState<string | null>(null);
  // Upload
  const [uploading, setUploading] = useState(false);
  const [uploadMessage, setUploadMessage] = useState("");

  useEffect(() => {
    api.get<Site>(`/sites/${id}`).then(setSite).catch(() => {});
  }, [id]);

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

  const navigateTo = (name: string) => {
    const newPath = currentPath === "." ? name : `${currentPath}/${name}`;
    loadDir(newPath);
    setEditing(null);
  };

  const goUp = () => {
    if (currentPath === ".") return;
    const parts = currentPath.split("/");
    parts.pop();
    loadDir(parts.length === 0 ? "." : parts.join("/"));
    setEditing(null);
  };

  const openFile = async (name: string) => {
    const filePath =
      currentPath === "." ? name : `${currentPath}/${name}`;
    try {
      const data = await api.get<{ content: string }>(
        `/sites/${id}/files/read?path=${encodeURIComponent(filePath)}`
      );
      setEditorContent(data.content);
      setEditing(filePath);
    } catch (e) {
      setError(e instanceof Error ? e.message : "Failed to read file");
    }
  };

  const saveFile = async () => {
    if (!editing) return;
    setSaving(true);
    setSaveSuccess(false);
    try {
      await api.put(`/sites/${id}/files/write`, {
        path: editing,
        content: editorContent,
      });
      setSaveSuccess(true);
      setTimeout(() => setSaveSuccess(false), 2000);
    } catch (e) {
      setError(e instanceof Error ? e.message : "Failed to save file");
    } finally {
      setSaving(false);
    }
  };

  const handleCreate = async () => {
    if (!createName.trim()) return;
    const path =
      currentPath === "."
        ? createName
        : `${currentPath}/${createName}`;
    try {
      await api.post(
        `/sites/${id}/files/create?path=${encodeURIComponent(path)}&type=${createType}`
      );
      setShowCreate(false);
      setCreateName("");
      loadDir(currentPath);
    } catch (e) {
      setError(e instanceof Error ? e.message : "Failed to create");
    }
  };

  const handleRename = async () => {
    if (!renaming || !renameTo.trim()) return;
    const fromPath =
      currentPath === "." ? renaming : `${currentPath}/${renaming}`;
    const toPath =
      currentPath === "." ? renameTo : `${currentPath}/${renameTo}`;
    try {
      await api.post(`/sites/${id}/files/rename`, {
        from: fromPath,
        to: toPath,
      });
      setRenaming(null);
      setRenameTo("");
      loadDir(currentPath);
    } catch (e) {
      setError(e instanceof Error ? e.message : "Failed to rename");
    }
  };

  const handleDelete = async (name: string) => {
    const path =
      currentPath === "." ? name : `${currentPath}/${name}`;
    try {
      await api.delete(
        `/sites/${id}/files?path=${encodeURIComponent(path)}`
      );
      setDeleteTarget(null);
      loadDir(currentPath);
    } catch (e) {
      setError(e instanceof Error ? e.message : "Failed to delete");
    }
  };

  const breadcrumbs = currentPath === "." ? [] : currentPath.split("/");

  return (
    <div className="p-6 lg:p-8">
      {/* Breadcrumb */}
      <div className="mb-6">
        <Link
          to={`/sites/${id}`}
          className="text-sm text-dark-200 hover:text-dark-100"
        >
          {site?.domain || "Site"}
        </Link>
        <span className="text-sm text-dark-300 mx-2">/</span>
        <span className="text-sm text-dark-50 font-medium">Files</span>
      </div>

      {/* Header */}
      <div className="flex flex-col sm:flex-row sm:items-center justify-between gap-3 mb-6 pb-4 border-b border-dark-600">
        <div>
          <h1 className="text-sm font-medium text-dark-300 uppercase font-mono tracking-widest">File Manager</h1>
          <p className="text-sm text-dark-200 mt-1 font-mono">{site?.domain}</p>
        </div>
        <div className="flex items-center gap-2">
          <label className={`px-4 py-2 bg-dark-700 text-dark-100 rounded-lg text-sm font-medium hover:bg-dark-600 cursor-pointer transition-colors ${uploading ? "opacity-50 pointer-events-none" : ""}`}>
            {uploading ? "Uploading..." : "Upload"}
            <input
              type="file"
              className="hidden"
              multiple
              disabled={uploading}
              onChange={async (e) => {
                const files = e.target.files;
                if (!files || files.length === 0) return;
                setUploading(true);
                setUploadMessage("");
                let success = 0;
                let failed = 0;
                for (const file of Array.from(files)) {
                  try {
                    const base64 = await new Promise<string>((resolve, reject) => {
                      const reader = new FileReader();
                      reader.onload = () => resolve((reader.result as string).split(",")[1]);
                      reader.onerror = reject;
                      reader.readAsDataURL(file);
                    });
                    await api.post(`/sites/${id}/files/upload`, {
                      path: currentPath,
                      filename: file.name,
                      content: base64,
                    });
                    success++;
                  } catch {
                    failed++;
                  }
                }
                setUploading(false);
                if (failed > 0) {
                  setUploadMessage(`${success} uploaded, ${failed} failed`);
                } else {
                  setUploadMessage(`${success} file(s) uploaded`);
                }
                loadDir(currentPath);
                e.target.value = "";
                setTimeout(() => setUploadMessage(""), 3000);
              }}
            />
          </label>
          <button
            onClick={() => setShowCreate(true)}
            className="px-4 py-2 bg-rust-500 text-white rounded-lg text-sm font-medium hover:bg-rust-600 transition-colors"
          >
            New File/Folder
          </button>
        </div>
      </div>

      {/* Path breadcrumb bar */}
      <div className="bg-dark-800 rounded-lg border border-dark-500 px-4 py-2.5 mb-4 flex items-center gap-1 text-sm overflow-x-auto">
        <button
          onClick={() => loadDir(".")}
          className="text-rust-400 hover:text-rust-300 font-medium shrink-0 font-mono"
        >
          /
        </button>
        {breadcrumbs.map((part, i) => (
          <span key={i} className="flex items-center gap-1 shrink-0">
            <span className="text-dark-300">/</span>
            {i < breadcrumbs.length - 1 ? (
              <button
                onClick={() =>
                  loadDir(breadcrumbs.slice(0, i + 1).join("/"))
                }
                className="text-rust-400 hover:text-rust-300 font-mono"
              >
                {part}
              </button>
            ) : (
              <span className="text-dark-50 font-medium font-mono">{part}</span>
            )}
          </span>
        ))}
      </div>

      {error && (
        <div className="mb-4 px-4 py-3 bg-danger-500/10 text-danger-400 rounded-lg border border-danger-500/20 text-sm">
          {error}
          <button onClick={() => setError("")} className="ml-2 underline">
            dismiss
          </button>
        </div>
      )}

      {uploadMessage && (
        <div className={`mb-4 px-4 py-3 rounded-lg text-sm border ${
          uploadMessage.includes("failed")
            ? "bg-danger-500/10 text-danger-400 border-danger-500/20"
            : "bg-rust-500/10 text-rust-400 border-rust-500/20"
        }`}>
          {uploadMessage}
        </div>
      )}

      <div className="flex gap-6">
        {/* File listing */}
        <div className={`bg-dark-800 rounded-lg border border-dark-500 overflow-x-auto ${editing ? "w-1/2" : "w-full"}`}>
          {loading ? (
            <div className="flex items-center justify-center py-12">
              <div className="w-6 h-6 border-2 border-dark-600 border-t-rust-500 rounded-full animate-spin" />
            </div>
          ) : (
            <table className="w-full">
              <thead>
                <tr className="bg-dark-900 border-b border-dark-500">
                  <th scope="col" className="text-left text-xs font-medium text-dark-200 uppercase font-mono tracking-widest px-4 py-3">
                    Name
                  </th>
                  <th scope="col" className="text-left text-xs font-medium text-dark-200 uppercase font-mono tracking-widest px-4 py-3 w-24">
                    Size
                  </th>
                  <th scope="col" className="text-left text-xs font-medium text-dark-200 uppercase font-mono tracking-widest px-4 py-3 w-36">
                    Modified
                  </th>
                  <th scope="col" className="text-right text-xs font-medium text-dark-200 uppercase font-mono tracking-widest px-4 py-3 w-28">
                    Actions
                  </th>
                </tr>
              </thead>
              <tbody className="divide-y divide-dark-600">
                {currentPath !== "." && (
                  <tr
                    className="hover:bg-dark-700/30 transition-colors cursor-pointer"
                    onClick={goUp}
                  >
                    <td className="px-4 py-3 text-sm text-dark-100 flex items-center gap-2" colSpan={4}>
                      <svg className="w-4 h-4 text-dark-300" fill="none" viewBox="0 0 24 24" stroke="currentColor" strokeWidth={2}>
                        <path strokeLinecap="round" strokeLinejoin="round" d="M15 19l-7-7 7-7" />
                      </svg>
                      ..
                    </td>
                  </tr>
                )}
                {entries.length === 0 && currentPath === "." && (
                  <tr>
                    <td colSpan={4} className="px-4 py-12 text-center text-dark-300 text-sm">
                      Empty directory
                    </td>
                  </tr>
                )}
                {entries
                  .sort((a, b) => {
                    if (a.is_dir !== b.is_dir) return a.is_dir ? -1 : 1;
                    return a.name.localeCompare(b.name);
                  })
                  .map((entry) => (
                    <tr
                      key={entry.name}
                      className="hover:bg-dark-700/30 transition-colors group"
                    >
                      <td className="px-4 py-3">
                        <button
                          onClick={() =>
                            entry.is_dir
                              ? navigateTo(entry.name)
                              : openFile(entry.name)
                          }
                          className="flex items-center gap-2 text-sm text-dark-50 hover:text-accent-400 font-mono"
                        >
                          {entry.is_dir ? (
                            <svg className="w-4 h-4 text-warn-500" fill="currentColor" viewBox="0 0 20 20">
                              <path d="M2 6a2 2 0 012-2h5l2 2h5a2 2 0 012 2v6a2 2 0 01-2 2H4a2 2 0 01-2-2V6z" />
                            </svg>
                          ) : (
                            <svg className="w-4 h-4 text-dark-300" fill="none" viewBox="0 0 24 24" stroke="currentColor" strokeWidth={1.5}>
                              <path strokeLinecap="round" strokeLinejoin="round" d="M19.5 14.25v-2.625a3.375 3.375 0 0 0-3.375-3.375h-1.5A1.125 1.125 0 0 1 13.5 7.125v-1.5a3.375 3.375 0 0 0-3.375-3.375H8.25m2.25 0H5.625c-.621 0-1.125.504-1.125 1.125v17.25c0 .621.504 1.125 1.125 1.125h12.75c.621 0 1.125-.504 1.125-1.125V11.25a9 9 0 0 0-9-9Z" />
                            </svg>
                          )}
                          {entry.name}
                        </button>
                      </td>
                      <td className="px-4 py-3 text-sm text-dark-200 font-mono">
                        {entry.is_dir ? "—" : formatSize(entry.size)}
                      </td>
                      <td className="px-4 py-3 text-sm text-dark-200 font-mono">
                        {formatDate(entry.modified)}
                      </td>
                      <td className="px-4 py-3 text-right">
                        <div className="flex items-center justify-end gap-1 opacity-0 group-hover:opacity-100 transition-opacity">
                          {!entry.is_dir && (
                            <a
                              href={`/api/sites/${id}/files/download?path=${encodeURIComponent(
                                (currentPath === "." ? "" : currentPath + "/") + entry.name
                              )}`}
                              className="p-1 text-dark-300 hover:text-dark-50 transition-colors"
                              title="Download"
                              download
                            >
                              <svg className="w-4 h-4" fill="none" viewBox="0 0 24 24" stroke="currentColor" strokeWidth={1.5}>
                                <path strokeLinecap="round" strokeLinejoin="round" d="M3 16.5v2.25A2.25 2.25 0 005.25 21h13.5A2.25 2.25 0 0021 18.75V16.5M16.5 12L12 16.5m0 0L7.5 12m4.5 4.5V3" />
                              </svg>
                            </a>
                          )}
                          <button
                            onClick={() => {
                              setRenaming(entry.name);
                              setRenameTo(entry.name);
                            }}
                            className="p-1 text-dark-300 hover:text-dark-200"
                            title="Rename"
                            aria-label="Rename"
                          >
                            <svg className="w-4 h-4" fill="none" viewBox="0 0 24 24" stroke="currentColor" strokeWidth={1.5}>
                              <path strokeLinecap="round" strokeLinejoin="round" d="m16.862 4.487 1.687-1.688a1.875 1.875 0 1 1 2.652 2.652L10.582 16.07a4.5 4.5 0 0 1-1.897 1.13L6 18l.8-2.685a4.5 4.5 0 0 1 1.13-1.897l8.932-8.931Zm0 0L19.5 7.125" />
                            </svg>
                          </button>
                          <button
                            onClick={() => setDeleteTarget(entry.name)}
                            className="p-1 text-dark-300 hover:text-danger-500"
                            title="Delete"
                            aria-label="Delete"
                          >
                            <svg className="w-4 h-4" fill="none" viewBox="0 0 24 24" stroke="currentColor" strokeWidth={1.5}>
                              <path strokeLinecap="round" strokeLinejoin="round" d="m14.74 9-.346 9m-4.788 0L9.26 9m9.968-3.21c.342.052.682.107 1.022.166m-1.022-.165L18.16 19.673a2.25 2.25 0 0 1-2.244 2.077H8.084a2.25 2.25 0 0 1-2.244-2.077L4.772 5.79m14.456 0a48.108 48.108 0 0 0-3.478-.397m-12 .562c.34-.059.68-.114 1.022-.165m0 0a48.11 48.11 0 0 1 3.478-.397m7.5 0v-.916c0-1.18-.91-2.164-2.09-2.201a51.964 51.964 0 0 0-3.32 0c-1.18.037-2.09 1.022-2.09 2.201v.916m7.5 0a48.667 48.667 0 0 0-7.5 0" />
                            </svg>
                          </button>
                        </div>
                      </td>
                    </tr>
                  ))}
              </tbody>
            </table>
          )}
        </div>

        {/* Editor panel */}
        {editing && (
          <div className="w-1/2 bg-dark-800 rounded-lg border border-dark-500 flex flex-col overflow-hidden">
            <div className="px-4 py-3 bg-dark-900 border-b border-dark-500 flex items-center justify-between">
              <span className="text-sm font-medium text-dark-100 truncate font-mono">
                {editing}
              </span>
              <div className="flex items-center gap-2">
                {saveSuccess && <span className="text-xs text-rust-400">Saved</span>}
                <button
                  onClick={saveFile}
                  disabled={saving}
                  className="px-3 py-1 bg-rust-500 text-white rounded-md text-xs font-medium hover:bg-rust-600 disabled:opacity-50"
                >
                  {saving ? "Saving..." : "Save"}
                </button>
                <button
                  onClick={() => setEditing(null)}
                  className="p-1 text-dark-300 hover:text-dark-200"
                >
                  <svg className="w-4 h-4" fill="none" viewBox="0 0 24 24" stroke="currentColor" strokeWidth={2}>
                    <path strokeLinecap="round" strokeLinejoin="round" d="M6 18L18 6M6 6l12 12" />
                  </svg>
                </button>
              </div>
            </div>
            <textarea
              value={editorContent}
              onChange={(e) => setEditorContent(e.target.value)}
              onKeyDown={(e) => {
                if ((e.ctrlKey || e.metaKey) && e.key === "s") {
                  e.preventDefault();
                  saveFile();
                }
              }}
              className="flex-1 p-4 font-mono text-sm text-dark-50 resize-none focus:outline-none"
              spellCheck={false}
            />
          </div>
        )}
      </div>

      {/* Create dialog */}
      {showCreate && (
        <div className="fixed inset-0 bg-black/30 flex items-center justify-center z-50 dp-modal-overlay">
          <div className="bg-dark-800 rounded-lg shadow-xl p-6 w-96 dp-modal">
            <h3 className="text-xs font-medium text-dark-300 uppercase font-mono tracking-widest mb-4">
              Create New
            </h3>
            <div className="flex gap-2 mb-4">
              <button
                onClick={() => setCreateType("file")}
                className={`flex-1 py-2 rounded-lg text-sm font-medium transition-colors ${
                  createType === "file"
                    ? "bg-rust-500 text-white"
                    : "bg-dark-700 text-dark-200 hover:bg-dark-600"
                }`}
              >
                File
              </button>
              <button
                onClick={() => setCreateType("dir")}
                className={`flex-1 py-2 rounded-lg text-sm font-medium transition-colors ${
                  createType === "dir"
                    ? "bg-rust-500 text-white"
                    : "bg-dark-700 text-dark-200 hover:bg-dark-600"
                }`}
              >
                Folder
              </button>
            </div>
            <input
              type="text"
              value={createName}
              onChange={(e) => setCreateName(e.target.value)}
              placeholder="Name..."
              className="w-full px-3 py-2 border border-dark-500 rounded-lg text-sm focus:ring-2 focus:ring-accent-500 focus:border-accent-500"
              autoFocus
              onKeyDown={(e) => e.key === "Enter" && handleCreate()}
            />
            <div className="flex justify-end gap-2 mt-4">
              <button
                onClick={() => {
                  setShowCreate(false);
                  setCreateName("");
                }}
                className="px-4 py-2 text-sm text-dark-200 hover:text-dark-50"
              >
                Cancel
              </button>
              <button
                onClick={handleCreate}
                className="px-4 py-2 bg-rust-500 text-white rounded-lg text-sm font-medium hover:bg-rust-600"
              >
                Create
              </button>
            </div>
          </div>
        </div>
      )}

      {/* Rename dialog */}
      {renaming && (
        <div className="fixed inset-0 bg-black/30 flex items-center justify-center z-50 dp-modal-overlay">
          <div className="bg-dark-800 rounded-lg shadow-xl p-6 w-96 dp-modal">
            <h3 className="text-xs font-medium text-dark-300 uppercase font-mono tracking-widest mb-4">
              Rename "{renaming}"
            </h3>
            <input
              type="text"
              value={renameTo}
              onChange={(e) => setRenameTo(e.target.value)}
              className="w-full px-3 py-2 border border-dark-500 rounded-lg text-sm focus:ring-2 focus:ring-accent-500 focus:border-accent-500"
              autoFocus
              onKeyDown={(e) => e.key === "Enter" && handleRename()}
            />
            <div className="flex justify-end gap-2 mt-4">
              <button
                onClick={() => setRenaming(null)}
                className="px-4 py-2 text-sm text-dark-200 hover:text-dark-50"
              >
                Cancel
              </button>
              <button
                onClick={handleRename}
                className="px-4 py-2 bg-rust-500 text-white rounded-lg text-sm font-medium hover:bg-rust-600"
              >
                Rename
              </button>
            </div>
          </div>
        </div>
      )}

      {/* Delete confirm */}
      {deleteTarget && (
        <div className="fixed inset-0 bg-black/30 flex items-center justify-center z-50 dp-modal-overlay">
          <div className="bg-dark-800 rounded-lg shadow-xl p-6 w-96 dp-modal">
            <h3 className="text-xs font-medium text-dark-300 uppercase font-mono tracking-widest mb-2">
              Delete "{deleteTarget}"?
            </h3>
            <p className="text-sm text-dark-200 mb-4">
              This action cannot be undone.
            </p>
            <div className="flex justify-end gap-2">
              <button
                onClick={() => setDeleteTarget(null)}
                className="px-4 py-2 text-sm text-dark-200 hover:text-dark-50"
              >
                Cancel
              </button>
              <button
                onClick={() => handleDelete(deleteTarget)}
                className="px-4 py-2 bg-danger-500 text-white rounded-lg text-sm font-medium hover:bg-danger-600"
              >
                Delete
              </button>
            </div>
          </div>
        </div>
      )}
    </div>
  );
}
