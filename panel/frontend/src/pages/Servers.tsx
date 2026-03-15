import { useState, useEffect, useRef, FormEvent } from "react";
import { Navigate } from "react-router-dom";
import { useAuth } from "../context/AuthContext";
import { api } from "../api";
import { formatDate } from "../utils/format";

interface Server {
  id: string;
  name: string;
  ip_address: string | null;
  status: string;
  last_seen_at: string | null;
  os_info: string | null;
  cpu_cores: number | null;
  ram_mb: number | null;
  disk_gb: number | null;
  agent_version: string | null;
  cpu_usage: number | null;
  mem_used_mb: number | null;
  uptime_secs: number | null;
  created_at: string;
}

const statusColors: Record<string, string> = {
  online: "bg-rust-500/15 text-rust-400",
  offline: "bg-red-500/15 text-danger-400",
  pending: "bg-warn-500/15 text-warn-400",
};

const statusDot: Record<string, string> = {
  online: "bg-rust-500",
  offline: "bg-red-500",
  pending: "bg-warn-500",
};

function UsageGauge({ label, value, max, unit, color }: { label: string; value: number; max: number; unit: string; color: string }) {
  const pct = max > 0 ? Math.min((value / max) * 100, 100) : 0;
  return (
    <div className="flex-1 min-w-0">
      <div className="flex items-baseline justify-between mb-1">
        <span className="text-xs text-dark-200">{label}</span>
        <span className="text-xs font-medium text-dark-100">{value.toFixed(label === "CPU" ? 1 : 0)}{unit}</span>
      </div>
      <div className="h-2 bg-dark-700 rounded-full overflow-hidden">
        <div className={`h-full rounded-full transition-all duration-500 ${pct > 90 ? "bg-red-500" : pct > 70 ? "bg-warn-500" : color}`} style={{ width: `${pct}%` }} />
      </div>
    </div>
  );
}

function formatUptime(secs: number): string {
  if (secs >= 86400) {
    const days = Math.floor(secs / 86400);
    const hours = Math.floor((secs % 86400) / 3600);
    return `${days}d ${hours}h`;
  }
  if (secs >= 3600) {
    const hours = Math.floor(secs / 3600);
    const mins = Math.floor((secs % 3600) / 60);
    return `${hours}h ${mins}m`;
  }
  return `${Math.floor(secs / 60)}m`;
}

export default function Servers() {
  const { user } = useAuth();
  const [servers, setServers] = useState<Server[]>([]);
  const [loading, setLoading] = useState(true);
  const [showForm, setShowForm] = useState(false);
  const [name, setName] = useState("");
  const [submitting, setSubmitting] = useState(false);
  const [error, setError] = useState("");
  const [actionLoading, setActionLoading] = useState<string | null>(null);
  const [expandedServer, setExpandedServer] = useState<string | null>(null);
  const [metricsData, setMetricsData] = useState<{ points: { value: number; recorded_at: string }[]; summary: { avg: number; min: number; max: number; count: number } } | null>(null);
  const [metricsType, setMetricsType] = useState("cpu");
  const [installInfo, setInstallInfo] = useState<{
    name: string;
    agent_token: string;
    install_script: string;
  } | null>(null);
  const intervalRef = useRef<ReturnType<typeof setInterval>>(undefined);

  const fetchServers = () => {
    api
      .get<Server[]>("/servers")
      .then(setServers)
      .catch((e) => setError(e.message))
      .finally(() => setLoading(false));
  };

  useEffect(() => {
    fetchServers();
    // Auto-refresh every 30s for live metrics
    intervalRef.current = setInterval(fetchServers, 30000);
    return () => clearInterval(intervalRef.current);
  }, []);

  if (user?.role !== "admin") return <Navigate to="/" replace />;

  const handleCreate = async (e: FormEvent) => {
    e.preventDefault();
    setError("");
    setSubmitting(true);
    try {
      const res = await api.post<{
        id: string;
        name: string;
        agent_token: string;
        install_script: string;
      }>("/servers", { name });
      setInstallInfo(res);
      setShowForm(false);
      setName("");
      fetchServers();
    } catch (err) {
      setError(err instanceof Error ? err.message : "Failed to create server");
    } finally {
      setSubmitting(false);
    }
  };

  const handleDelete = async (id: string, serverName: string) => {
    if (!confirm(`Delete server "${serverName}"? This cannot be undone.`)) return;
    try {
      await api.delete(`/servers/${id}`);
      fetchServers();
    } catch (err) {
      setError(err instanceof Error ? err.message : "Delete failed");
    }
  };

  const toggleServerExpand = async (serverId: string) => {
    if (expandedServer === serverId) {
      setExpandedServer(null);
      return;
    }
    setExpandedServer(serverId);
    fetchMetrics(serverId, metricsType);
  };

  const fetchMetrics = async (serverId: string, type: string) => {
    try {
      const data = await api.get<typeof metricsData>(`/servers/${serverId}/metrics?metric_type=${type}&hours=24`);
      setMetricsData(data);
    } catch {
      setMetricsData(null);
    }
  };

  const handleAction = async (serverId: string, action: string) => {
    setActionLoading(`${serverId}-${action}`);
    try {
      await api.post(`/servers/${serverId}/commands`, { action });
      fetchServers();
    } catch (err) {
      setError(err instanceof Error ? err.message : "Action failed");
    } finally {
      setActionLoading(null);
    }
  };

  // Summary stats
  const online = servers.filter((s) => s.status === "online").length;
  const offline = servers.filter((s) => s.status === "offline").length;
  const pending = servers.filter((s) => s.status === "pending").length;

  if (loading) {
    return (
      <div className="p-6 lg:p-8 animate-fade-up">
        <div className="bg-dark-800 rounded-lg border border-dark-500 p-6 animate-pulse">
          <div className="h-6 bg-dark-700 rounded w-48 mb-4" />
          <div className="h-4 bg-dark-700 rounded w-32" />
        </div>
      </div>
    );
  }

  return (
    <div className="p-6 lg:p-8">
      <div className="flex flex-col sm:flex-row sm:items-center justify-between gap-3 mb-6 pb-4 border-b border-dark-600">
        <div>
          <h1 className="text-sm font-medium text-dark-300 uppercase font-mono tracking-widest">Servers</h1>
          <p className="text-sm text-dark-200 font-mono mt-1">Monitor and manage your connected servers</p>
        </div>
        <button
          onClick={() => { setShowForm(!showForm); setInstallInfo(null); }}
          className="px-4 py-2 bg-rust-500 text-white rounded-lg text-sm font-medium hover:bg-rust-600 transition-colors"
        >
          Add Server
        </button>
      </div>

      {error && (
        <div className="bg-red-500/10 text-danger-400 text-sm px-4 py-3 rounded-lg border border-red-500/20 mb-4">
          {error}
          <button onClick={() => setError("")} className="ml-2 font-medium hover:underline">Dismiss</button>
        </div>
      )}

      {/* Status overview */}
      {servers.length > 0 && (
        <div className="grid grid-cols-1 sm:grid-cols-3 gap-4 mb-6">
          <div className="bg-dark-800 rounded-lg border border-dark-500 p-4">
            <div className="flex items-center gap-3">
              <div className="w-10 h-10 bg-rust-500/15 rounded-lg flex items-center justify-center">
                <div className="w-3 h-3 bg-rust-500 rounded-full" />
              </div>
              <div>
                <p className="text-2xl font-bold text-dark-50">{online}</p>
                <p className="text-xs text-dark-200">Online</p>
              </div>
            </div>
          </div>
          <div className="bg-dark-800 rounded-lg border border-dark-500 p-4">
            <div className="flex items-center gap-3">
              <div className="w-10 h-10 bg-red-500/15 rounded-lg flex items-center justify-center">
                <div className="w-3 h-3 bg-red-500 rounded-full" />
              </div>
              <div>
                <p className="text-2xl font-bold text-dark-50">{offline}</p>
                <p className="text-xs text-dark-200">Offline</p>
              </div>
            </div>
          </div>
          <div className="bg-dark-800 rounded-lg border border-dark-500 p-4">
            <div className="flex items-center gap-3">
              <div className="w-10 h-10 bg-warn-500/15 rounded-lg flex items-center justify-center">
                <div className="w-3 h-3 bg-warn-500 rounded-full" />
              </div>
              <div>
                <p className="text-2xl font-bold text-dark-50">{pending}</p>
                <p className="text-xs text-dark-200">Pending</p>
              </div>
            </div>
          </div>
        </div>
      )}

      {/* Install script display */}
      {installInfo && (
        <div className="bg-rust-500/10 border border-rust-500/20 rounded-lg p-5 mb-6">
          <h3 className="text-xs font-medium text-rust-400 uppercase font-mono tracking-widest mb-2">
            Server "{installInfo.name}" created!
          </h3>
          <p className="text-sm text-rust-400 mb-3">
            Run this command on your server to install the DockPanel agent:
          </p>
          <div className="bg-dark-950 text-rust-400 p-4 rounded-lg font-mono text-xs break-all">
            {installInfo.install_script}
          </div>
          <p className="text-xs text-rust-400 mt-3">
            Save your agent token — it won't be shown again.
          </p>
          <button
            onClick={() => setInstallInfo(null)}
            className="mt-3 px-3 py-1.5 bg-rust-500/20 text-rust-400 rounded-lg text-xs font-medium hover:bg-rust-500/30"
          >
            Dismiss
          </button>
        </div>
      )}

      {/* Create form */}
      {showForm && (
        <form onSubmit={handleCreate} className="bg-dark-800 rounded-lg border border-dark-500 p-5 mb-6">
          <h3 className="text-xs font-medium text-dark-300 uppercase font-mono tracking-widest mb-3">Add New Server</h3>
          <div className="flex items-end gap-3">
            <div className="flex-1">
              <label htmlFor="server-name" className="block text-xs font-medium text-dark-200 mb-1">Server Name</label>
              <input
                id="server-name"
                type="text"
                value={name}
                onChange={(e) => setName(e.target.value)}
                placeholder="e.g. Production VPS"
                required
                className="w-full px-3 py-2 border border-dark-500 rounded-lg text-sm focus:ring-2 focus:ring-accent-500 focus:border-accent-500 outline-none"
              />
            </div>
            <button
              type="submit"
              disabled={submitting}
              className="px-4 py-2 bg-rust-500 text-white rounded-lg text-sm font-medium hover:bg-rust-600 disabled:opacity-50"
            >
              {submitting ? "Creating..." : "Create"}
            </button>
            <button
              type="button"
              onClick={() => setShowForm(false)}
              className="px-4 py-2 bg-dark-700 text-dark-100 rounded-lg text-sm font-medium hover:bg-dark-600"
            >
              Cancel
            </button>
          </div>
        </form>
      )}

      {/* Server cards */}
      {servers.length === 0 ? (
        <div className="bg-dark-800 rounded-lg border border-dark-500 p-12 text-center">
          <svg className="w-12 h-12 text-dark-300 mx-auto mb-4" fill="none" viewBox="0 0 24 24" stroke="currentColor" strokeWidth={1}>
            <path strokeLinecap="round" strokeLinejoin="round" d="M5 12h14M5 12a2 2 0 0 1-2-2V6a2 2 0 0 1 2-2h14a2 2 0 0 1 2 2v4a2 2 0 0 1-2 2M5 12a2 2 0 0 0-2 2v4a2 2 0 0 0 2 2h14a2 2 0 0 0 2-2v-4a2 2 0 0 0-2-2m-2-4h.01M17 16h.01" />
          </svg>
          <p className="text-dark-200 text-sm">No servers yet. Add your first server to get started.</p>
        </div>
      ) : (
        <div className="space-y-4">
          {servers.map((s) => (
            <div key={s.id} className="bg-dark-800 rounded-lg border border-dark-500 p-5">
              {/* Header row */}
              <div className="flex items-center justify-between mb-4">
                <div className="flex items-center gap-3">
                  <div className={`w-2.5 h-2.5 rounded-full ${statusDot[s.status] || "bg-gray-400"}`} />
                  <div>
                    <p className="text-sm font-semibold text-dark-50">{s.name}</p>
                    <div className="flex items-center gap-2 mt-0.5">
                      {s.ip_address && (
                        <span className="text-xs text-dark-200 font-mono">{s.ip_address}</span>
                      )}
                      {s.os_info && (
                        <span className="text-xs text-dark-300">{s.os_info}</span>
                      )}
                      {s.agent_version && (
                        <span className="text-xs text-dark-300 font-mono">v{s.agent_version}</span>
                      )}
                    </div>
                  </div>
                </div>
                <div className="flex items-center gap-2">
                  <span className={`inline-flex px-2 py-0.5 rounded-full text-xs font-medium ${statusColors[s.status] || "bg-dark-700 text-dark-200"}`}>
                    {s.status}
                  </span>
                  {s.status === "online" && (
                    <div className="flex items-center gap-1">
                      <button
                        onClick={() => handleAction(s.id, "health")}
                        disabled={actionLoading === `${s.id}-health`}
                        className="p-1.5 text-dark-300 hover:text-indigo-600 transition-colors"
                        title="Health check"
                      >
                        <svg className="w-4 h-4" fill="none" viewBox="0 0 24 24" stroke="currentColor" strokeWidth={2}>
                          <path strokeLinecap="round" strokeLinejoin="round" d="M21 8.25c0-2.485-2.099-4.5-4.688-4.5-1.935 0-3.597 1.126-4.312 2.733-.715-1.607-2.377-2.733-4.313-2.733C5.1 3.75 3 5.765 3 8.25c0 7.22 9 12 9 12s9-4.78 9-12Z" />
                        </svg>
                      </button>
                    </div>
                  )}
                  <button
                    onClick={() => handleDelete(s.id, s.name)}
                    className="p-1.5 text-dark-300 hover:text-danger-500 transition-colors"
                    title="Delete server"
                  >
                    <svg className="w-4 h-4" fill="none" viewBox="0 0 24 24" stroke="currentColor" strokeWidth={2}>
                      <path strokeLinecap="round" strokeLinejoin="round" d="m14.74 9-.346 9m-4.788 0L9.26 9m9.968-3.21c.342.052.682.107 1.022.166m-1.022-.165L18.16 19.673a2.25 2.25 0 0 1-2.244 2.077H8.084a2.25 2.25 0 0 1-2.244-2.077L4.772 5.79m14.456 0a48.108 48.108 0 0 0-3.478-.397m-12 .562c.34-.059.68-.114 1.022-.165m0 0a48.11 48.11 0 0 1 3.478-.397m7.5 0v-.916c0-1.18-.91-2.164-2.09-2.201a51.964 51.964 0 0 0-3.32 0c-1.18.037-2.09 1.022-2.09 2.201v.916m7.5 0a48.667 48.667 0 0 0-7.5 0" />
                    </svg>
                  </button>
                </div>
              </div>

              {/* Resource gauges (online servers only) */}
              {s.status === "online" && s.cpu_usage != null && (
                <div className="flex items-center gap-6 mb-3">
                  <UsageGauge label="CPU" value={s.cpu_usage} max={100} unit="%" color="bg-rust-400" />
                  {s.mem_used_mb != null && s.ram_mb ? (
                    <UsageGauge label="Memory" value={Math.round(s.mem_used_mb / 1024 * 10) / 10} max={Math.round(s.ram_mb / 1024)} unit={`/${Math.round(s.ram_mb / 1024)}GB`} color="bg-violet-500" />
                  ) : null}
                  {s.disk_gb ? (
                    <UsageGauge label="Disk" value={s.disk_gb} max={s.disk_gb} unit="GB" color="bg-cyan-500" />
                  ) : null}
                </div>
              )}

              {/* Footer: specs + last seen */}
              <div className="flex items-center justify-between text-xs text-dark-300">
                <div className="flex items-center gap-3">
                  {s.cpu_cores && <span>{s.cpu_cores} vCPU</span>}
                  {s.ram_mb && <span>{Math.round(s.ram_mb / 1024)} GB RAM</span>}
                  {s.disk_gb && <span>{s.disk_gb} GB Disk</span>}
                  {s.uptime_secs != null && s.status === "online" && (
                    <span>Up {formatUptime(s.uptime_secs)}</span>
                  )}
                </div>
                <div className="flex items-center gap-3">
                  <span>
                    {s.last_seen_at ? `Last seen ${formatDate(s.last_seen_at)}` : `Added ${formatDate(s.created_at)}`}
                  </span>
                  {s.status === "online" && (
                    <button
                      onClick={() => toggleServerExpand(s.id)}
                      className="text-rust-400 hover:text-rust-300 font-medium"
                    >
                      {expandedServer === s.id ? "Hide Metrics" : "Metrics"}
                    </button>
                  )}
                </div>
              </div>

              {/* Expanded metrics panel */}
              {expandedServer === s.id && metricsData && (
                <div className="mt-4 pt-4 border-t border-dark-600">
                  <div className="flex items-center justify-between mb-3">
                    <div className="flex items-center gap-2">
                      <h4 className="text-xs font-semibold text-dark-100">Performance (24h)</h4>
                      <select
                        value={metricsType}
                        onChange={(e) => { setMetricsType(e.target.value); fetchMetrics(s.id, e.target.value); }}
                        aria-label="Metric type"
                        className="text-xs border border-dark-500 rounded px-2 py-0.5 outline-none"
                      >
                        <option value="cpu">CPU %</option>
                        <option value="memory_mb">Memory MB</option>
                      </select>
                    </div>
                    <div className="flex items-center gap-4 text-xs text-dark-200">
                      <span>Avg: <strong className="text-dark-100">{metricsData.summary.avg}</strong></span>
                      <span>Min: <strong className="text-dark-100">{metricsData.summary.min}</strong></span>
                      <span>Max: <strong className="text-dark-100">{metricsData.summary.max}</strong></span>
                      <span>{metricsData.summary.count} points</span>
                    </div>
                  </div>
                  {/* Simple sparkline chart */}
                  {metricsData.points.length > 0 ? (
                    <div className="h-20 flex items-end gap-px">
                      {metricsData.points.slice(-120).map((p, i) => {
                        const maxVal = metricsType === "cpu" ? 100 : metricsData.summary.max * 1.1;
                        const h = maxVal > 0 ? (p.value / maxVal) * 100 : 0;
                        return (
                          <div
                            key={i}
                            className={`flex-1 min-w-0 rounded-t ${h > 90 ? "bg-red-400" : h > 70 ? "bg-warn-400" : "bg-indigo-400"}`}
                            style={{ height: `${Math.max(h, 2)}%` }}
                            title={`${p.value.toFixed(1)} at ${new Date(p.recorded_at).toLocaleTimeString()}`}
                          />
                        );
                      })}
                    </div>
                  ) : (
                    <p className="text-xs text-dark-300 text-center py-4">No metrics data yet. Data populates as the agent reports.</p>
                  )}
                </div>
              )}
            </div>
          ))}
        </div>
      )}
    </div>
  );
}
