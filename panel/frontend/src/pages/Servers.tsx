import { useAuth } from "../context/AuthContext";
import { Navigate } from "react-router-dom";
import { useState, useEffect, useCallback } from "react";
import { api, ApiError } from "../api";
import { useServer, type Server } from "../context/ServerContext";

interface CreateForm {
  name: string;
  ip_address: string;
}

export default function Servers() {
  const { user } = useAuth();
  if (!user || user.role !== "admin") return <Navigate to="/" replace />;
  const { servers, refreshServers } = useServer();
  const [creating, setCreating] = useState(false);
  const [form, setForm] = useState<CreateForm>({ name: "", ip_address: "" });
  const [installScript, setInstallScript] = useState<string | null>(null);
  const [error, setError] = useState("");
  const [testing, setTesting] = useState<string | null>(null);
  const [testResult, setTestResult] = useState<Record<string, string>>({});
  const [editing, setEditing] = useState<string | null>(null);
  const [editForm, setEditForm] = useState({ name: "", ip_address: "", agent_url: "" });
  const [pendingDelete, setPendingDelete] = useState<{ id: string; name: string } | null>(null);
  const [pendingRotate, setPendingRotate] = useState<{ id: string; name: string } | null>(null);
  const [rotateResult, setRotateResult] = useState<Record<string, string>>({});

  const handleCreate = useCallback(async () => {
    if (!form.name.trim()) return;
    setError("");
    try {
      const res = await api.post<{ install_script: string; id: string }>("/servers", {
        name: form.name.trim(),
        ip_address: form.ip_address.trim() || undefined,
      });
      setInstallScript(res.install_script);
      setForm({ name: "", ip_address: "" });
      await refreshServers();
    } catch (e) {
      setError(e instanceof ApiError ? e.message : "Failed to create server");
    }
  }, [form, refreshServers]);

  const handleDelete = useCallback((id: string, name: string) => {
    setPendingDelete({ id, name });
  }, []);

  const executeDelete = useCallback(async () => {
    if (!pendingDelete) return;
    setPendingDelete(null);
    try {
      await api.delete(`/servers/${pendingDelete.id}`);
      await refreshServers();
    } catch (e) {
      setError(e instanceof ApiError ? e.message : "Failed to delete server");
    }
  }, [refreshServers]);

  const executeRotate = useCallback(async () => {
    if (!pendingRotate) return;
    const { id } = pendingRotate;
    setPendingRotate(null);
    try {
      await api.post(`/servers/${id}/rotate-cert-pin`);
      setRotateResult((prev) => ({ ...prev, [id]: "Pin cleared — next agent checkin will re-capture." }));
      await refreshServers();
    } catch (e) {
      setRotateResult((prev) => ({ ...prev, [id]: e instanceof ApiError ? e.message : "Rotate failed" }));
    }
  }, [pendingRotate, refreshServers]);

  const handleTest = useCallback(async (id: string) => {
    setTesting(id);
    setTestResult((prev) => ({ ...prev, [id]: "testing" }));
    try {
      const res = await api.post<{ status: string; version: string }>(`/servers/${id}/test`);
      setTestResult((prev) => ({ ...prev, [id]: `Online (v${res.version})` }));
      await refreshServers();
    } catch (e) {
      setTestResult((prev) => ({ ...prev, [id]: e instanceof ApiError ? e.message : "Connection failed" }));
    } finally {
      setTesting(null);
    }
  }, [refreshServers]);

  const startEdit = (s: Server) => {
    setEditing(s.id);
    setEditForm({ name: s.name, ip_address: s.ip_address || "", agent_url: s.agent_url || "" });
  };

  const handleEdit = useCallback(async (id: string) => {
    setError("");
    try {
      await api.put(`/servers/${id}`, {
        name: editForm.name.trim() || undefined,
        ip_address: editForm.ip_address.trim() || undefined,
        agent_url: editForm.agent_url.trim() || undefined,
      });
      setEditing(null);
      await refreshServers();
    } catch (e) {
      setError(e instanceof ApiError ? e.message : "Failed to update server");
    }
  }, [editForm, refreshServers]);

  const formatUptime = (secs: number | null) => {
    if (!secs) return "-";
    const days = Math.floor(secs / 86400);
    const hours = Math.floor((secs % 86400) / 3600);
    return days > 0 ? `${days}d ${hours}h` : `${hours}h`;
  };

  const relTime = (iso: string | null): string => {
    if (!iso) return "never";
    const secs = (Date.now() - new Date(iso).getTime()) / 1000;
    if (secs < 0) return "just now";
    if (secs < 60) return `${Math.round(secs)}s ago`;
    if (secs < 3600) return `${Math.round(secs / 60)}m ago`;
    if (secs < 86400) return `${(secs / 3600).toFixed(1)}h ago`;
    return `${(secs / 86400).toFixed(1)}d ago`;
  };

  return (
    <div>
      <div className="page-header">
        <div>
          <h1 className="page-header-title">Servers</h1>
          <p className="page-header-subtitle">Manage local and remote servers</p>
        </div>
        <div className="flex items-center gap-2">
          <button
            onClick={() => { setCreating(!creating); setInstallScript(null); setError(""); }}
            className="px-4 py-2 bg-rust-500 text-dark-950 rounded-lg text-sm font-bold hover:bg-rust-400 transition-colors"
          >
            + Add Server
          </button>
        </div>
      </div>

      <div className="p-6 space-y-6">

      {error && (
        <div className="px-4 py-3 bg-danger-500/10 border border-danger-500/30 rounded-lg text-sm text-danger-400">{error}</div>
      )}

      {pendingDelete && (
        <div className="border border-danger-500/30 bg-danger-500/5 rounded-lg px-4 py-3 flex items-center justify-between">
          <span className="text-xs text-danger-400 font-mono">Delete server "{pendingDelete.name}"? All sites, databases, and apps will be removed.</span>
          <div className="flex items-center gap-2 shrink-0 ml-4">
            <button onClick={executeDelete} className="px-3 py-1.5 bg-danger-500 text-white text-xs font-bold uppercase tracking-wider hover:bg-danger-400 transition-colors">Confirm</button>
            <button onClick={() => setPendingDelete(null)} className="px-3 py-1.5 bg-dark-600 text-dark-200 text-xs font-bold uppercase tracking-wider hover:bg-dark-500 transition-colors">Cancel</button>
          </div>
        </div>
      )}

      {pendingRotate && (
        <div className="border border-warn-500/30 bg-warn-500/5 rounded-lg px-4 py-3 flex items-center justify-between">
          <span className="text-xs text-warn-400 font-mono">Rotate TLS pin for "{pendingRotate.name}"? This clears the stored fingerprint; the next checkin re-captures (TOFU window). Only do this after a legitimate agent cert rotation.</span>
          <div className="flex items-center gap-2 shrink-0 ml-4">
            <button onClick={executeRotate} className="px-3 py-1.5 bg-warn-500 text-dark-950 text-xs font-bold uppercase tracking-wider hover:bg-warn-400 transition-colors">Rotate</button>
            <button onClick={() => setPendingRotate(null)} className="px-3 py-1.5 bg-dark-600 text-dark-200 text-xs font-bold uppercase tracking-wider hover:bg-dark-500 transition-colors">Cancel</button>
          </div>
        </div>
      )}

      {creating && (
        <div className="bg-dark-800 border border-dark-600 rounded-lg p-5 space-y-4">
          <h2 className="text-lg font-bold text-dark-50 font-mono">Add Remote Server</h2>
          <div className="grid grid-cols-1 md:grid-cols-2 gap-4">
            <div>
              <label className="block text-sm text-dark-200 mb-1">Server Name</label>
              <input
                value={form.name}
                onChange={(e) => setForm((f) => ({ ...f, name: e.target.value }))}
                placeholder="Production Server"
                className="w-full px-3 py-2 bg-dark-900 border border-dark-600 rounded-lg text-dark-50 text-sm focus:border-rust-500 focus:outline-none"
              />
            </div>
            <div>
              <label className="block text-sm text-dark-200 mb-1">IP Address</label>
              <input
                value={form.ip_address}
                onChange={(e) => setForm((f) => ({ ...f, ip_address: e.target.value }))}
                placeholder="192.168.1.100"
                className="w-full px-3 py-2 bg-dark-900 border border-dark-600 rounded-lg text-dark-50 text-sm focus:border-rust-500 focus:outline-none"
              />
            </div>
          </div>
          <div className="flex gap-3">
            <button onClick={handleCreate} className="px-4 py-2 bg-rust-500 text-dark-950 rounded-lg text-sm font-bold hover:bg-rust-400 transition-colors">
              Create Server
            </button>
            <button onClick={() => setCreating(false)} className="px-4 py-2 bg-dark-700 text-dark-200 rounded-lg text-sm hover:bg-dark-600 transition-colors">
              Cancel
            </button>
          </div>

          {installScript && (
            <div className="mt-4 space-y-2">
              <p className="text-sm text-dark-200">Run this command on the remote server to install the DockPanel agent:</p>
              <div className="relative">
                <pre className="bg-dark-950 border border-dark-600 rounded-lg p-4 text-sm text-rust-400 font-mono overflow-x-auto whitespace-pre-wrap">{installScript}</pre>
                <button
                  onClick={() => navigator.clipboard.writeText(installScript)}
                  className="absolute top-2 right-2 px-2 py-1 bg-dark-700 text-dark-200 rounded text-xs hover:bg-dark-600 transition-colors"
                >
                  Copy
                </button>
              </div>
              <p className="text-xs text-dark-400">After installation, click "Test Connection" to verify the agent is running.</p>
            </div>
          )}
        </div>
      )}

      {/* Server list */}
      <div className="space-y-3">
        {servers.map((s) => (
          <div key={s.id} className="bg-dark-800 border border-dark-600 rounded-lg p-5 card-interactive">
            <div className="flex items-start justify-between">
              <div className="flex items-center gap-3">
                <div className={`w-3 h-3 rounded-full ${s.status === "online" ? "bg-rust-500" : s.status === "offline" ? "bg-danger-500 animate-pulse" : "bg-dark-400"}`} />
                <div>
                  <h3 className="text-base font-bold text-dark-50 font-mono flex items-center gap-2">
                    {s.name}
                    {s.is_local && <span className="text-[10px] px-2 py-0.5 bg-rust-500/20 text-rust-400 rounded-full uppercase font-bold">Local</span>}
                  </h3>
                  <p className="text-sm text-dark-300 mt-0.5">
                    {s.ip_address || "127.0.0.1"} &middot; {s.status}
                    {s.agent_version && ` &middot; v${s.agent_version}`}
                  </p>
                  <p className="text-[11px] text-dark-400 mt-0.5 font-mono">
                    Last seen {relTime(s.last_seen_at)}
                  </p>
                </div>
              </div>
              <div className="flex items-center gap-2">
                {!s.is_local && (
                  <>
                    <button
                      onClick={() => startEdit(s)}
                      className="px-3 py-1.5 bg-dark-700 text-dark-200 rounded text-xs font-medium hover:bg-dark-600 transition-colors"
                    >
                      Edit
                    </button>
                    <button
                      onClick={() => handleTest(s.id)}
                      disabled={testing === s.id}
                      className="px-3 py-1.5 bg-dark-700 text-dark-200 rounded text-xs font-medium hover:bg-dark-600 transition-colors disabled:opacity-50"
                    >
                      {testing === s.id ? "Testing..." : "Test"}
                    </button>
                    <button
                      onClick={() => handleDelete(s.id, s.name)}
                      className="px-3 py-1.5 bg-danger-500/10 text-danger-400 rounded text-xs font-medium hover:bg-danger-500/20 transition-colors"
                    >
                      Delete
                    </button>
                  </>
                )}
              </div>
            </div>

            {testResult[s.id] && (
              <div className={`mt-3 px-3 py-2 rounded text-sm ${testResult[s.id].startsWith("Online") ? "bg-rust-500/10 text-rust-400" : testResult[s.id] === "testing" ? "bg-dark-700 text-dark-300" : "bg-danger-500/10 text-danger-400"}`}>
                {testResult[s.id]}
              </div>
            )}

            {editing === s.id && (
              <div className="mt-3 p-3 bg-dark-900/50 rounded-lg space-y-3">
                <div className="grid grid-cols-1 md:grid-cols-3 gap-3">
                  <div>
                    <label className="block text-xs text-dark-300 mb-1">Name</label>
                    <input value={editForm.name} onChange={(e) => setEditForm((f) => ({ ...f, name: e.target.value }))} className="w-full px-2 py-1.5 bg-dark-900 border border-dark-600 rounded text-dark-50 text-sm focus:border-rust-500 focus:outline-none" />
                  </div>
                  <div>
                    <label className="block text-xs text-dark-300 mb-1">IP Address</label>
                    <input value={editForm.ip_address} onChange={(e) => setEditForm((f) => ({ ...f, ip_address: e.target.value }))} className="w-full px-2 py-1.5 bg-dark-900 border border-dark-600 rounded text-dark-50 text-sm focus:border-rust-500 focus:outline-none" />
                  </div>
                  <div>
                    <label className="block text-xs text-dark-300 mb-1">Agent URL</label>
                    <input value={editForm.agent_url} onChange={(e) => setEditForm((f) => ({ ...f, agent_url: e.target.value }))} placeholder="https://ip:9443" className="w-full px-2 py-1.5 bg-dark-900 border border-dark-600 rounded text-dark-50 text-sm focus:border-rust-500 focus:outline-none" />
                  </div>
                </div>
                <div className="flex gap-2">
                  <button onClick={() => handleEdit(s.id)} className="px-3 py-1.5 bg-rust-500 text-dark-950 rounded text-xs font-bold hover:bg-rust-400 transition-colors">Save</button>
                  <button onClick={() => setEditing(null)} className="px-3 py-1.5 bg-dark-700 text-dark-200 rounded text-xs hover:bg-dark-600 transition-colors">Cancel</button>
                </div>
              </div>
            )}

            {/* Metrics row */}
            {s.status === "online" && (s.cpu_cores || s.ram_mb) && (
              <div className="mt-3 flex gap-6 text-xs text-dark-300 font-mono">
                {s.cpu_cores && <span>CPU: {s.cpu_cores} cores{s.cpu_usage != null ? ` (${s.cpu_usage.toFixed(0)}%)` : ""}</span>}
                {s.ram_mb && <span>RAM: {(s.ram_mb / 1024).toFixed(1)} GB{s.mem_used_mb != null ? ` (${((s.mem_used_mb / s.ram_mb) * 100).toFixed(0)}%)` : ""}</span>}
                {s.disk_gb && <span>Disk: {s.disk_gb} GB</span>}
                {s.uptime_secs && <span>Uptime: {formatUptime(s.uptime_secs)}</span>}
              </div>
            )}

            <UptimeStrip serverId={s.id} />


            {/* TLS cert pin (remote servers only) */}
            {!s.is_local && (
              <div className="mt-3 pt-3 border-t border-dark-700 flex items-center justify-between gap-3 text-xs font-mono">
                <div className="min-w-0 flex-1">
                  <span className="text-dark-300 uppercase tracking-widest text-[10px]">TLS pin</span>
                  {s.cert_fingerprint ? (
                    <span className="ml-2 text-dark-100 break-all" title={`SHA-256 of agent's inbound TLS cert — captured on first checkin (TOFU). Full: ${s.cert_fingerprint}`}>
                      {s.cert_fingerprint.slice(0, 16)}…{s.cert_fingerprint.slice(-16)}
                    </span>
                  ) : (
                    <span className="ml-2 text-warn-400">not yet captured</span>
                  )}
                </div>
                <button
                  onClick={() => setPendingRotate({ id: s.id, name: s.name })}
                  className="px-3 py-1 bg-dark-700 text-dark-200 rounded text-[11px] hover:bg-dark-600 transition-colors whitespace-nowrap"
                  title="Clear the pin so the next agent checkin re-captures it. Use after legitimately rotating the agent cert."
                >
                  Rotate pin
                </button>
              </div>
            )}
            {rotateResult[s.id] && (
              <div className="mt-2 px-3 py-2 rounded text-xs bg-rust-500/10 text-rust-400 font-mono">
                {rotateResult[s.id]}
              </div>
            )}
          </div>
        ))}

        {servers.length === 0 && (
          <div className="text-center py-12 text-dark-300 text-sm">
            No servers found. The local server should appear automatically.
          </div>
        )}
      </div>
      </div>
    </div>
  );
}

interface UptimeData {
  buckets: boolean[];
  window_hours: number;
  bucket_minutes: number;
}

function UptimeStrip({ serverId }: { serverId: string }) {
  const [data, setData] = useState<UptimeData | null>(null);

  useEffect(() => {
    let cancelled = false;
    api.get<UptimeData>(`/servers/${serverId}/uptime`)
      .then((d) => { if (!cancelled) setData(d); })
      .catch(() => {});
    return () => { cancelled = true; };
  }, [serverId]);

  if (!data) return null;

  const total = data.buckets.length;
  const onlineCount = data.buckets.filter(Boolean).length;
  const onlinePct = total > 0 ? (onlineCount / total) * 100 : 0;

  return (
    <div className="mt-3 pt-3 border-t border-dark-700">
      <div className="flex items-center justify-between mb-1.5">
        <span className="text-[10px] uppercase tracking-widest text-dark-300">
          Uptime · last {data.window_hours}h
        </span>
        <span className="text-xs font-mono text-dark-100">
          {onlinePct.toFixed(1)}%
        </span>
      </div>
      <div className="flex gap-px h-3">
        {data.buckets.map((up, i) => {
          const minutesAgoEnd = (total - 1 - i) * data.bucket_minutes;
          const minutesAgoStart = minutesAgoEnd + data.bucket_minutes;
          const label = minutesAgoEnd === 0
            ? `now: ${up ? "online" : "no data"}`
            : `${minutesAgoStart}–${minutesAgoEnd} min ago: ${up ? "online" : "no data"}`;
          return (
            <div
              key={i}
              className={`flex-1 rounded-sm ${up ? "bg-rust-500/70" : "bg-dark-600"}`}
              title={label}
            />
          );
        })}
      </div>
    </div>
  );
}
