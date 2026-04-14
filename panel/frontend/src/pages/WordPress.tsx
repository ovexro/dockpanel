import { useState, useEffect } from "react";
import { useParams, Link } from "react-router-dom";
import { api } from "../api";

interface WpInfo {
  installed: boolean;
  version?: string;
  update_available?: string | null;
  auto_update?: boolean;
}

interface WpPlugin {
  name: string;
  status: string;
  version: string;
  update: string;
  title?: string;
}

interface WpTheme {
  name: string;
  status: string;
  version: string;
  update: string;
  title?: string;
}

export default function WordPress() {
  const { id } = useParams<{ id: string }>();
  const [info, setInfo] = useState<WpInfo | null>(null);
  const [plugins, setPlugins] = useState<WpPlugin[]>([]);
  const [themes, setThemes] = useState<WpTheme[]>([]);
  const [loading, setLoading] = useState(true);
  const [tab, setTab] = useState<"plugins" | "themes">("plugins");
  const [message, setMessage] = useState({ text: "", type: "" });
  const [busy, setBusy] = useState("");
  const [deleteTarget, setDeleteTarget] = useState<string | null>(null);

  // Install form
  const [showInstall, setShowInstall] = useState(false);
  const [installData, setInstallData] = useState({
    url: "",
    title: "My WordPress Site",
    admin_user: "admin",
    admin_pass: "",
    admin_email: "",
    db_name: "",
    db_user: "",
    db_pass: "",
    db_host: "localhost",
  });

  const loadInfo = async () => {
    try {
      const data = await api.get<WpInfo>(`/sites/${id}/wordpress`);
      setInfo(data);
      if (data.installed) {
        loadPlugins();
        loadThemes();
      }
    } catch {
      setInfo({ installed: false });
    } finally {
      setLoading(false);
    }
  };

  const loadPlugins = async () => {
    try {
      const data = await api.get<WpPlugin[]>(`/sites/${id}/wordpress/plugins`);
      setPlugins(Array.isArray(data) ? data : []);
    } catch {
      setPlugins([]);
    }
  };

  const loadThemes = async () => {
    try {
      const data = await api.get<WpTheme[]>(`/sites/${id}/wordpress/themes`);
      setThemes(Array.isArray(data) ? data : []);
    } catch {
      setThemes([]);
    }
  };

  useEffect(() => {
    loadInfo();
  }, [id]);

  const flash = (text: string, type: string) => {
    setMessage({ text, type });
    setTimeout(() => setMessage({ text: "", type: "" }), 5000);
  };

  const handleInstall = async () => {
    setBusy("install");
    try {
      await api.post(`/sites/${id}/wordpress/install`, installData);
      flash("WordPress installed successfully!", "success");
      setShowInstall(false);
      loadInfo();
    } catch (e) {
      flash(e instanceof Error ? e.message : "Install failed", "error");
    } finally {
      setBusy("");
    }
  };

  const handleUpdate = async (target: string) => {
    setBusy(`update-${target}`);
    try {
      await api.post(`/sites/${id}/wordpress/update/${target}`, {});
      flash(`${target.charAt(0).toUpperCase() + target.slice(1)} updated`, "success");
      loadInfo();
    } catch (e) {
      flash(e instanceof Error ? e.message : "Update failed", "error");
    } finally {
      setBusy("");
    }
  };

  const handlePluginAction = async (name: string, action: string) => {
    setBusy(`plugin-${action}-${name}`);
    try {
      await api.post(`/sites/${id}/wordpress/plugin/${action}`, { name });
      flash(`Plugin ${action}d: ${name}`, "success");
      loadPlugins();
    } catch (e) {
      flash(e instanceof Error ? e.message : `${action} failed`, "error");
    } finally {
      setBusy("");
    }
  };

  const handleThemeAction = async (name: string, action: string) => {
    setBusy(`theme-${action}-${name}`);
    try {
      await api.post(`/sites/${id}/wordpress/theme/${action}`, { name });
      flash(`Theme ${action}d: ${name}`, "success");
      loadThemes();
    } catch (e) {
      flash(e instanceof Error ? e.message : `${action} failed`, "error");
    } finally {
      setBusy("");
    }
  };

  const handleAutoUpdate = async (enabled: boolean) => {
    setBusy("auto-update");
    try {
      await api.post(`/sites/${id}/wordpress/auto-update`, { enabled });
      setInfo((prev) => (prev ? { ...prev, auto_update: enabled } : prev));
      flash(`Auto-updates ${enabled ? "enabled" : "disabled"}`, "success");
    } catch (e) {
      flash(e instanceof Error ? e.message : "Failed", "error");
    } finally {
      setBusy("");
    }
  };

  const updatesAvailable =
    plugins.filter((p) => p.update === "available").length +
    themes.filter((t) => t.update === "available").length;

  if (loading) {
    return (
      <div className="p-6 lg:p-8">
        <h1 className="text-sm font-medium text-dark-300 uppercase font-mono tracking-widest mb-6">WordPress</h1>
        <div className="bg-dark-800 rounded-lg border border-dark-500 p-6 animate-pulse">
          <div className="h-6 bg-dark-700 rounded w-48 mb-4" />
          <div className="h-4 bg-dark-700 rounded w-32" />
        </div>
      </div>
    );
  }

  return (
    <div className="p-6 lg:p-8">
      {/* Header */}
      <div className="flex items-center gap-3 mb-6 pb-4 border-b border-dark-600">
        <Link
          to={`/sites/${id}`}
          className="p-1.5 text-dark-300 hover:text-dark-100 rounded-lg hover:bg-dark-700"
        >
          <svg className="w-5 h-5" fill="none" viewBox="0 0 24 24" stroke="currentColor" strokeWidth={2}>
            <path strokeLinecap="round" strokeLinejoin="round" d="M15.75 19.5 8.25 12l7.5-7.5" />
          </svg>
        </Link>
        <div className="flex-1">
          <h1 className="text-sm font-medium text-dark-300 uppercase font-mono tracking-widest">WordPress</h1>
          <p className="text-sm text-dark-200 font-mono mt-0.5">Manage WordPress installation</p>
        </div>
      </div>

      {message.text && (
        <div
          className={`mb-4 px-4 py-3 rounded-lg text-sm border ${
            message.type === "success"
              ? "bg-rust-500/10 text-rust-400 border-rust-500/20"
              : "bg-danger-500/10 text-danger-400 border-danger-500/20"
          }`}
        >
          {message.text}
        </div>
      )}

      {!info?.installed ? (
        /* WordPress Not Installed */
        <div className="bg-dark-800 rounded-lg border border-dark-500 p-8">
          <div className="text-center max-w-md mx-auto">
            <div className="w-16 h-16 bg-accent-500/15 rounded-2xl flex items-center justify-center mx-auto mb-4">
              <svg className="w-8 h-8 text-accent-400" viewBox="0 0 24 24" fill="currentColor">
                <path d="M12 2C6.486 2 2 6.486 2 12s4.486 10 10 10 10-4.486 10-10S17.514 2 12 2zm0 2c1.67 0 3.214.52 4.488 1.401L5.401 16.488A7.957 7.957 0 0 1 4 12c0-4.411 3.589-8 8-8zm0 16c-1.67 0-3.214-.52-4.488-1.401L18.599 7.512A7.957 7.957 0 0 1 20 12c0 4.411-3.589 8-8 8z" />
              </svg>
            </div>
            <h2 className="text-lg font-semibold text-dark-50 mb-2">
              WordPress Not Detected
            </h2>
            <p className="text-sm text-dark-200 mb-6">
              No wp-config.php found in this site's document root. Install WordPress with one click.
            </p>
            <button
              onClick={() => setShowInstall(true)}
              className="px-5 py-2.5 bg-accent-600 text-white rounded-lg text-sm font-medium hover:bg-accent-700"
            >
              Install WordPress
            </button>
          </div>

          {showInstall && (
            <div className="mt-8 pt-6 border-t border-dark-500 space-y-4">
              <h3 className="text-xs font-medium text-dark-300 uppercase font-mono tracking-widest">Installation Details</h3>
              <div className="grid grid-cols-1 md:grid-cols-2 gap-4">
                <div>
                  <label className="block text-xs font-medium text-dark-100 mb-1">Site URL</label>
                  <input
                    type="text"
                    value={installData.url}
                    onChange={(e) => setInstallData({ ...installData, url: e.target.value })}
                    placeholder="https://example.com"
                    className="w-full px-3 py-2 border border-dark-500 rounded-lg text-sm focus:ring-2 focus:ring-rust-500 focus:border-rust-500 outline-none"
                  />
                </div>
                <div>
                  <label className="block text-xs font-medium text-dark-100 mb-1">Site Title</label>
                  <input
                    type="text"
                    value={installData.title}
                    onChange={(e) => setInstallData({ ...installData, title: e.target.value })}
                    className="w-full px-3 py-2 border border-dark-500 rounded-lg text-sm focus:ring-2 focus:ring-rust-500 focus:border-rust-500 outline-none"
                  />
                </div>
              </div>
              <h4 className="text-xs font-semibold text-dark-200 uppercase tracking-wider pt-2">Admin Account</h4>
              <div className="grid grid-cols-1 md:grid-cols-3 gap-4">
                <div>
                  <label className="block text-xs font-medium text-dark-100 mb-1">Username</label>
                  <input
                    type="text"
                    value={installData.admin_user}
                    onChange={(e) => setInstallData({ ...installData, admin_user: e.target.value })}
                    className="w-full px-3 py-2 border border-dark-500 rounded-lg text-sm focus:ring-2 focus:ring-rust-500 focus:border-rust-500 outline-none"
                  />
                </div>
                <div>
                  <label className="block text-xs font-medium text-dark-100 mb-1">Password</label>
                  <input
                    type="password"
                    value={installData.admin_pass}
                    onChange={(e) => setInstallData({ ...installData, admin_pass: e.target.value })}
                    placeholder="Strong password"
                    className="w-full px-3 py-2 border border-dark-500 rounded-lg text-sm focus:ring-2 focus:ring-rust-500 focus:border-rust-500 outline-none"
                  />
                </div>
                <div>
                  <label className="block text-xs font-medium text-dark-100 mb-1">Email</label>
                  <input
                    type="email"
                    value={installData.admin_email}
                    onChange={(e) => setInstallData({ ...installData, admin_email: e.target.value })}
                    placeholder="admin@example.com"
                    className="w-full px-3 py-2 border border-dark-500 rounded-lg text-sm focus:ring-2 focus:ring-rust-500 focus:border-rust-500 outline-none"
                  />
                </div>
              </div>
              <h4 className="text-xs font-semibold text-dark-200 uppercase tracking-wider pt-2">MySQL Database</h4>
              <div className="grid grid-cols-1 md:grid-cols-2 gap-4">
                <div>
                  <label className="block text-xs font-medium text-dark-100 mb-1">Database Name</label>
                  <input
                    type="text"
                    value={installData.db_name}
                    onChange={(e) => setInstallData({ ...installData, db_name: e.target.value })}
                    className="w-full px-3 py-2 border border-dark-500 rounded-lg text-sm focus:ring-2 focus:ring-rust-500 focus:border-rust-500 outline-none"
                  />
                </div>
                <div>
                  <label className="block text-xs font-medium text-dark-100 mb-1">Database Host</label>
                  <input
                    type="text"
                    value={installData.db_host}
                    onChange={(e) => setInstallData({ ...installData, db_host: e.target.value })}
                    className="w-full px-3 py-2 border border-dark-500 rounded-lg text-sm focus:ring-2 focus:ring-rust-500 focus:border-rust-500 outline-none"
                  />
                </div>
                <div>
                  <label className="block text-xs font-medium text-dark-100 mb-1">Database User</label>
                  <input
                    type="text"
                    value={installData.db_user}
                    onChange={(e) => setInstallData({ ...installData, db_user: e.target.value })}
                    className="w-full px-3 py-2 border border-dark-500 rounded-lg text-sm focus:ring-2 focus:ring-rust-500 focus:border-rust-500 outline-none"
                  />
                </div>
                <div>
                  <label className="block text-xs font-medium text-dark-100 mb-1">Database Password</label>
                  <input
                    type="password"
                    value={installData.db_pass}
                    onChange={(e) => setInstallData({ ...installData, db_pass: e.target.value })}
                    className="w-full px-3 py-2 border border-dark-500 rounded-lg text-sm focus:ring-2 focus:ring-rust-500 focus:border-rust-500 outline-none"
                  />
                </div>
              </div>
              <p className="text-xs text-dark-300">
                You need a MySQL/MariaDB database. Create one via the Apps page (deploy MariaDB) or use an external database.
              </p>
              <div className="flex items-center gap-3 pt-2">
                <button
                  onClick={handleInstall}
                  disabled={
                    busy === "install" ||
                    !installData.url ||
                    !installData.admin_pass ||
                    !installData.admin_email ||
                    !installData.db_name ||
                    !installData.db_user
                  }
                  className="px-5 py-2 bg-accent-600 text-white rounded-lg text-sm font-medium hover:bg-accent-700 disabled:opacity-50"
                >
                  {busy === "install" ? "Installing..." : "Install WordPress"}
                </button>
                <button
                  onClick={() => setShowInstall(false)}
                  className="px-4 py-2 bg-dark-600 text-dark-100 rounded-lg text-sm font-medium hover:bg-dark-500"
                >
                  Cancel
                </button>
              </div>
            </div>
          )}
        </div>
      ) : (
        /* WordPress Installed */
        <>
          {/* Info Card */}
          <div className="bg-dark-800 rounded-lg border border-dark-500 p-5 mb-6">
            <div className="flex items-center justify-between flex-wrap gap-4">
              <div className="flex items-center gap-4">
                <div className="w-12 h-12 bg-accent-500/15 rounded-lg flex items-center justify-center">
                  <svg className="w-7 h-7 text-accent-400" viewBox="0 0 24 24" fill="currentColor">
                    <path d="M12 2C6.486 2 2 6.486 2 12s4.486 10 10 10 10-4.486 10-10S17.514 2 12 2zm0 2c1.67 0 3.214.52 4.488 1.401L5.401 16.488A7.957 7.957 0 0 1 4 12c0-4.411 3.589-8 8-8zm0 16c-1.67 0-3.214-.52-4.488-1.401L18.599 7.512A7.957 7.957 0 0 1 20 12c0 4.411-3.589 8-8 8z" />
                  </svg>
                </div>
                <div>
                  <div className="flex items-center gap-2">
                    <h2 className="text-lg font-semibold text-dark-50">
                      WordPress <span className="font-mono">{info.version}</span>
                    </h2>
                    {info.update_available && (
                      <span className="px-2 py-0.5 bg-warn-500/15 text-warn-400 rounded text-xs font-medium">
                        {info.update_available} available
                      </span>
                    )}
                  </div>
                  <p className="text-sm text-dark-200">
                    {plugins.length} plugin{plugins.length !== 1 ? "s" : ""},{" "}
                    {themes.length} theme{themes.length !== 1 ? "s" : ""}
                    {updatesAvailable > 0 && (
                      <span className="text-warn-500 ml-1">
                        ({updatesAvailable} update{updatesAvailable !== 1 ? "s" : ""} available)
                      </span>
                    )}
                  </p>
                </div>
              </div>
              <div className="flex items-center gap-2 flex-wrap">
                {info.update_available && (
                  <button
                    onClick={() => handleUpdate("core")}
                    disabled={busy === "update-core"}
                    className="px-3 py-1.5 bg-warn-500 text-white rounded-lg text-xs font-medium hover:bg-warn-600 disabled:opacity-50"
                  >
                    {busy === "update-core" ? "Updating..." : "Update Core"}
                  </button>
                )}
                {plugins.some((p) => p.update === "available") && (
                  <button
                    onClick={() => handleUpdate("plugins")}
                    disabled={busy === "update-plugins"}
                    className="px-3 py-1.5 bg-rust-500 text-white rounded-lg text-xs font-medium hover:bg-rust-600 disabled:opacity-50"
                  >
                    {busy === "update-plugins" ? "Updating..." : "Update All Plugins"}
                  </button>
                )}
                {themes.some((t) => t.update === "available") && (
                  <button
                    onClick={() => handleUpdate("themes")}
                    disabled={busy === "update-themes"}
                    className="px-3 py-1.5 bg-rust-500 text-white rounded-lg text-xs font-medium hover:bg-rust-600 disabled:opacity-50"
                  >
                    {busy === "update-themes" ? "Updating..." : "Update All Themes"}
                  </button>
                )}
                <button
                  onClick={async () => {
                    setBusy("safe-update");
                    try {
                      const result = await api.post<{ rolled_back?: boolean; core_before?: string; core_after?: string; log?: string[] }>(`/sites/${id}/wordpress/update-safe`);
                      if (result.rolled_back) {
                        setMessage({ text: "Update failed health check — rolled back to snapshot", type: "warning" });
                      } else {
                        setMessage({ text: `Safe update complete: ${result.core_before} → ${result.core_after}`, type: "success" });
                      }
                      loadInfo();
                    } catch (e) {
                      setMessage({ text: e instanceof Error ? e.message : "Safe update failed", type: "error" });
                    } finally {
                      setBusy("");
                    }
                  }}
                  disabled={!!busy}
                  className="px-3 py-1.5 bg-accent-500/20 text-accent-400 rounded-lg text-xs font-medium hover:bg-accent-500/30 disabled:opacity-50"
                  title="Creates a snapshot before updating, rolls back automatically if health check fails"
                >
                  {busy === "safe-update" ? "Updating..." : "Safe Update (Rollback)"}
                </button>
                <label className="flex items-center gap-2 ml-2 cursor-pointer">
                  <span className="text-xs text-dark-200">Auto-update</span>
                  <button
                    onClick={() => handleAutoUpdate(!info.auto_update)}
                    disabled={busy === "auto-update"}
                    className={`relative inline-flex h-5 w-9 items-center rounded-full transition-colors ${
                      info.auto_update ? "bg-rust-500" : "bg-dark-400"
                    }`}
                  >
                    <span
                      className={`inline-block h-3.5 w-3.5 rounded-full bg-dark-800 transition-transform ${
                        info.auto_update ? "translate-x-4.5" : "translate-x-0.5"
                      }`}
                    />
                  </button>
                </label>
              </div>
            </div>
          </div>

          {/* Tabs */}
          <div className="flex gap-1 mb-4">
            <button
              onClick={() => setTab("plugins")}
              className={`px-4 py-2 rounded-lg text-sm font-medium transition-colors ${
                tab === "plugins"
                  ? "bg-rust-500 text-white"
                  : "bg-dark-700 text-dark-200 hover:bg-dark-600"
              }`}
            >
              Plugins ({plugins.length})
            </button>
            <button
              onClick={() => setTab("themes")}
              className={`px-4 py-2 rounded-lg text-sm font-medium transition-colors ${
                tab === "themes"
                  ? "bg-rust-500 text-white"
                  : "bg-dark-700 text-dark-200 hover:bg-dark-600"
              }`}
            >
              Themes ({themes.length})
            </button>
          </div>

          {/* Plugin List */}
          {tab === "plugins" && (
            <div className="bg-dark-800 rounded-lg border border-dark-500 overflow-x-auto">
              <table className="w-full text-sm">
                <thead>
                  <tr className="bg-dark-900 text-left">
                    <th className="px-4 py-2.5 text-xs font-medium text-dark-200 uppercase font-mono tracking-widest">
                      Plugin
                    </th>
                    <th className="px-4 py-2.5 text-xs font-medium text-dark-200 uppercase font-mono tracking-widest w-24">
                      Version
                    </th>
                    <th className="px-4 py-2.5 text-xs font-medium text-dark-200 uppercase font-mono tracking-widest w-24">
                      Status
                    </th>
                    <th className="px-4 py-2.5 text-xs font-medium text-dark-200 uppercase font-mono tracking-widest w-24">
                      Update
                    </th>
                    <th className="px-4 py-2.5 text-xs font-medium text-dark-200 uppercase font-mono tracking-widest w-32">
                      Actions
                    </th>
                  </tr>
                </thead>
                <tbody className="divide-y divide-dark-600">
                  {plugins.map((p) => (
                    <tr key={p.name} className="hover:bg-dark-700/30 transition-colors">
                      <td className="px-4 py-2.5 text-dark-50 font-medium">{p.name}</td>
                      <td className="px-4 py-2.5 text-dark-200 font-mono text-xs">{p.version}</td>
                      <td className="px-4 py-2.5">
                        <span
                          className={`inline-flex px-2 py-0.5 rounded text-xs font-medium ${
                            p.status === "active"
                              ? "bg-rust-500/15 text-rust-400"
                              : p.status === "must-use"
                              ? "bg-accent-600/15 text-accent-400"
                              : "bg-dark-700 text-dark-200"
                          }`}
                        >
                          {p.status}
                        </span>
                      </td>
                      <td className="px-4 py-2.5">
                        {p.update === "available" ? (
                          <span className="inline-flex px-2 py-0.5 rounded text-xs font-medium bg-warn-500/15 text-warn-400">
                            available
                          </span>
                        ) : (
                          <span className="text-xs text-dark-300">up to date</span>
                        )}
                      </td>
                      <td className="px-4 py-2.5">
                        <div className="flex items-center gap-1">
                          {p.status === "active" ? (
                            <button
                              onClick={() => handlePluginAction(p.name, "deactivate")}
                              disabled={busy.startsWith("plugin-")}
                              className="px-2 py-1 text-xs bg-dark-700 text-dark-100 rounded hover:bg-dark-600 disabled:opacity-50"
                            >
                              Deactivate
                            </button>
                          ) : (
                            p.status !== "must-use" && (
                              <button
                                onClick={() => handlePluginAction(p.name, "activate")}
                                disabled={busy.startsWith("plugin-")}
                                className="px-2 py-1 text-xs bg-rust-500/15 text-rust-400 rounded hover:bg-rust-200 disabled:opacity-50"
                              >
                                Activate
                              </button>
                            )
                          )}
                          {p.update === "available" && (
                            <button
                              onClick={() => handlePluginAction(p.name, "update")}
                              disabled={busy.startsWith("plugin-")}
                              className="px-2 py-1 text-xs bg-warn-500/15 text-warn-400 rounded hover:bg-warn-400/20 disabled:opacity-50"
                            >
                              Update
                            </button>
                          )}
                          {p.status !== "must-use" && (
                            deleteTarget === `plugin-${p.name}` ? (
                              <div className="flex items-center gap-1">
                                <button
                                  onClick={() => { handlePluginAction(p.name, "delete"); setDeleteTarget(null); }}
                                  className="px-2 py-1 bg-danger-600 text-white rounded-md text-xs"
                                >Confirm</button>
                                <button
                                  onClick={() => setDeleteTarget(null)}
                                  className="px-2 py-1 bg-dark-600 text-dark-200 rounded-md text-xs"
                                >Cancel</button>
                              </div>
                            ) : (
                              <button
                                onClick={() => setDeleteTarget(`plugin-${p.name}`)}
                                disabled={busy.startsWith("plugin-")}
                                className="p-1 text-dark-300 hover:text-danger-500"
                              >
                                <svg
                                  className="w-3.5 h-3.5"
                                  fill="none"
                                  viewBox="0 0 24 24"
                                  stroke="currentColor"
                                  strokeWidth={1.5}
                                >
                                  <path
                                    strokeLinecap="round"
                                    strokeLinejoin="round"
                                    d="m14.74 9-.346 9m-4.788 0L9.26 9m9.968-3.21c.342.052.682.107 1.022.166m-1.022-.165L18.16 19.673a2.25 2.25 0 0 1-2.244 2.077H8.084a2.25 2.25 0 0 1-2.244-2.077L4.772 5.79m14.456 0a48.108 48.108 0 0 0-3.478-.397m-12 .562c.34-.059.68-.114 1.022-.165m0 0a48.11 48.11 0 0 1 3.478-.397m7.5 0v-.916c0-1.18-.91-2.164-2.09-2.201a51.964 51.964 0 0 0-3.32 0c-1.18.037-2.09 1.022-2.09 2.201v.916m7.5 0a48.667 48.667 0 0 0-7.5 0"
                                  />
                                </svg>
                              </button>
                            )
                          )}
                        </div>
                      </td>
                    </tr>
                  ))}
                  {plugins.length === 0 && (
                    <tr>
                      <td colSpan={5} className="px-4 py-8 text-center text-dark-300 text-sm">
                        No plugins installed
                      </td>
                    </tr>
                  )}
                </tbody>
              </table>
            </div>
          )}

          {/* Theme List */}
          {tab === "themes" && (
            <div className="bg-dark-800 rounded-lg border border-dark-500 overflow-x-auto">
              <table className="w-full text-sm">
                <thead>
                  <tr className="bg-dark-900 text-left">
                    <th className="px-4 py-2.5 text-xs font-medium text-dark-200 uppercase font-mono tracking-widest">
                      Theme
                    </th>
                    <th className="px-4 py-2.5 text-xs font-medium text-dark-200 uppercase font-mono tracking-widest w-24">
                      Version
                    </th>
                    <th className="px-4 py-2.5 text-xs font-medium text-dark-200 uppercase font-mono tracking-widest w-24">
                      Status
                    </th>
                    <th className="px-4 py-2.5 text-xs font-medium text-dark-200 uppercase font-mono tracking-widest w-24">
                      Update
                    </th>
                    <th className="px-4 py-2.5 text-xs font-medium text-dark-200 uppercase font-mono tracking-widest w-32">
                      Actions
                    </th>
                  </tr>
                </thead>
                <tbody className="divide-y divide-dark-600">
                  {themes.map((t) => (
                    <tr key={t.name} className="hover:bg-dark-700/30 transition-colors">
                      <td className="px-4 py-2.5 text-dark-50 font-medium">{t.name}</td>
                      <td className="px-4 py-2.5 text-dark-200 font-mono text-xs">{t.version}</td>
                      <td className="px-4 py-2.5">
                        <span
                          className={`inline-flex px-2 py-0.5 rounded text-xs font-medium ${
                            t.status === "active"
                              ? "bg-rust-500/15 text-rust-400"
                              : t.status === "parent"
                              ? "bg-accent-500/15 text-accent-400"
                              : "bg-dark-700 text-dark-200"
                          }`}
                        >
                          {t.status}
                        </span>
                      </td>
                      <td className="px-4 py-2.5">
                        {t.update === "available" ? (
                          <span className="inline-flex px-2 py-0.5 rounded text-xs font-medium bg-warn-500/15 text-warn-400">
                            available
                          </span>
                        ) : (
                          <span className="text-xs text-dark-300">up to date</span>
                        )}
                      </td>
                      <td className="px-4 py-2.5">
                        <div className="flex items-center gap-1">
                          {t.status !== "active" && (
                            <button
                              onClick={() => handleThemeAction(t.name, "activate")}
                              disabled={busy.startsWith("theme-")}
                              className="px-2 py-1 text-xs bg-rust-500/15 text-rust-400 rounded hover:bg-rust-200 disabled:opacity-50"
                            >
                              Activate
                            </button>
                          )}
                          {t.update === "available" && (
                            <button
                              onClick={() => handleThemeAction(t.name, "update")}
                              disabled={busy.startsWith("theme-")}
                              className="px-2 py-1 text-xs bg-warn-500/15 text-warn-400 rounded hover:bg-warn-400/20 disabled:opacity-50"
                            >
                              Update
                            </button>
                          )}
                          {t.status !== "active" && (
                            deleteTarget === `theme-${t.name}` ? (
                              <div className="flex items-center gap-1">
                                <button
                                  onClick={() => { handleThemeAction(t.name, "delete"); setDeleteTarget(null); }}
                                  className="px-2 py-1 bg-danger-600 text-white rounded-md text-xs"
                                >Confirm</button>
                                <button
                                  onClick={() => setDeleteTarget(null)}
                                  className="px-2 py-1 bg-dark-600 text-dark-200 rounded-md text-xs"
                                >Cancel</button>
                              </div>
                            ) : (
                              <button
                                onClick={() => setDeleteTarget(`theme-${t.name}`)}
                                disabled={busy.startsWith("theme-")}
                                className="p-1 text-dark-300 hover:text-danger-500"
                              >
                                <svg
                                  className="w-3.5 h-3.5"
                                  fill="none"
                                  viewBox="0 0 24 24"
                                  stroke="currentColor"
                                  strokeWidth={1.5}
                                >
                                  <path
                                    strokeLinecap="round"
                                    strokeLinejoin="round"
                                    d="m14.74 9-.346 9m-4.788 0L9.26 9m9.968-3.21c.342.052.682.107 1.022.166m-1.022-.165L18.16 19.673a2.25 2.25 0 0 1-2.244 2.077H8.084a2.25 2.25 0 0 1-2.244-2.077L4.772 5.79m14.456 0a48.108 48.108 0 0 0-3.478-.397m-12 .562c.34-.059.68-.114 1.022-.165m0 0a48.11 48.11 0 0 1 3.478-.397m7.5 0v-.916c0-1.18-.91-2.164-2.09-2.201a51.964 51.964 0 0 0-3.32 0c-1.18.037-2.09 1.022-2.09 2.201v.916m7.5 0a48.667 48.667 0 0 0-7.5 0"
                                  />
                                </svg>
                              </button>
                            )
                          )}
                        </div>
                      </td>
                    </tr>
                  ))}
                  {themes.length === 0 && (
                    <tr>
                      <td colSpan={5} className="px-4 py-8 text-center text-dark-300 text-sm">
                        No themes installed
                      </td>
                    </tr>
                  )}
                </tbody>
              </table>
            </div>
          )}
        </>
      )}
    </div>
  );
}
