import { useState, useEffect, Fragment } from "react";
import { api } from "../api";
import { formatDate } from "../utils/format";

interface TelemetryEvent {
  id: string;
  event_type: string;
  category: string;
  message: string;
  context: Record<string, unknown>;
  sent_at: string | null;
  created_at: string;
}

interface TelemetryStats {
  total: number;
  unsent: number;
  last_24h: number;
  by_category: { category: string; count: number }[];
  by_type: { type: string; count: number }[];
}

interface TelemetryConfig {
  telemetry_enabled?: string;
  telemetry_endpoint?: string;
  telemetry_installation_id?: string;
  current_version?: string;
  update_available_version?: string;
  update_release_notes?: string;
  update_release_url?: string;
  update_checked_at?: string;
}

const EVENT_TYPE_COLORS: Record<string, string> = {
  panic: "bg-red-500/20 text-red-400 border-red-500/30",
  error: "bg-red-500/10 text-red-400 border-red-500/20",
  warning: "bg-amber-500/10 text-amber-400 border-amber-500/20",
  info: "bg-blue-500/10 text-blue-400 border-blue-500/20",
};

const CATEGORY_COLORS: Record<string, string> = {
  agent: "text-purple-400",
  api: "text-blue-400",
  database: "text-emerald-400",
  ssl: "text-yellow-400",
  docker: "text-cyan-400",
  mail: "text-pink-400",
  security: "text-red-400",
  nginx: "text-green-400",
  backup: "text-orange-400",
  general: "text-dark-300",
};

export default function Telemetry() {
  const [tab, setTab] = useState<"events" | "updates" | "config">("events");
  const [events, setEvents] = useState<TelemetryEvent[]>([]);
  const [stats, setStats] = useState<TelemetryStats | null>(null);
  const [config, setConfig] = useState<TelemetryConfig>({});
  const [loading, setLoading] = useState(true);
  const [total, setTotal] = useState(0);
  const [page, setPage] = useState(0);
  const [message, setMessage] = useState({ text: "", type: "" });
  const [categoryFilter, setCategoryFilter] = useState("");
  const [typeFilter, setTypeFilter] = useState("");
  const [previewData, setPreviewData] = useState<Record<string, unknown> | null>(null);
  const [showPreview, setShowPreview] = useState(false);
  const [expandedEvent, setExpandedEvent] = useState<string | null>(null);

  // Config form
  const [enabled, setEnabled] = useState(false);
  const [endpoint, setEndpoint] = useState("");
  const [saving, setSaving] = useState(false);
  const [sending, setSending] = useState(false);
  const [checking, setChecking] = useState(false);
  const [clearing, setClearing] = useState(false);

  const limit = 25;

  const loadEvents = async () => {
    try {
      let url = `/telemetry/events?limit=${limit}&offset=${page * limit}`;
      if (categoryFilter) url += `&category=${categoryFilter}`;
      if (typeFilter) url += `&event_type=${typeFilter}`;
      const data = await api.get<{ events: TelemetryEvent[]; total: number }>(url);
      setEvents(data.events);
      setTotal(data.total);
    } catch {
      // empty
    }
  };

  const loadStats = async () => {
    try {
      const data = await api.get<TelemetryStats>("/telemetry/stats");
      setStats(data);
    } catch {
      // empty
    }
  };

  const loadConfig = async () => {
    try {
      const data = await api.get<TelemetryConfig>("/telemetry/config");
      setConfig(data);
      setEnabled(data.telemetry_enabled === "true");
      setEndpoint(data.telemetry_endpoint || "");
    } catch {
      // empty
    }
  };

  useEffect(() => {
    Promise.all([loadEvents(), loadStats(), loadConfig()]).finally(() => setLoading(false));
  }, []);

  useEffect(() => { loadEvents(); }, [page, categoryFilter, typeFilter]);

  const flash = (text: string, type: string) => {
    setMessage({ text, type });
    setTimeout(() => setMessage({ text: "", type: "" }), 4000);
  };

  const saveConfig = async () => {
    setSaving(true);
    try {
      await api.put("/telemetry/config", {
        telemetry_enabled: enabled ? "true" : "false",
        telemetry_endpoint: endpoint,
      });
      flash("Configuration saved", "success");
      loadConfig();
    } catch (e: unknown) {
      flash((e as Error).message || "Failed to save", "error");
    } finally {
      setSaving(false);
    }
  };

  const sendNow = async () => {
    setSending(true);
    try {
      await api.post("/telemetry/send");
      flash("Telemetry events being sent", "success");
      setTimeout(loadEvents, 3000);
    } catch (e: unknown) {
      flash((e as Error).message || "Failed to send", "error");
    } finally {
      setSending(false);
    }
  };

  const previewReport = async () => {
    try {
      const data = await api.get<Record<string, unknown>>("/telemetry/preview");
      setPreviewData(data);
      setShowPreview(true);
    } catch (e: unknown) {
      flash((e as Error).message || "Failed to generate preview", "error");
    }
  };

  const exportReport = async () => {
    try {
      const data = await api.get("/telemetry/export");
      const blob = new Blob([JSON.stringify(data, null, 2)], { type: "application/json" });
      const url = URL.createObjectURL(blob);
      const a = document.createElement("a");
      a.href = url;
      a.download = `dockpanel-telemetry-${new Date().toISOString().slice(0, 10)}.json`;
      a.click();
      URL.revokeObjectURL(url);
      flash("Report exported", "success");
    } catch (e: unknown) {
      flash((e as Error).message || "Failed to export", "error");
    }
  };

  const [pendingClear, setPendingClear] = useState<number | "all" | null>(null);

  const clearEvents = async (days?: number) => {
    setPendingClear(days ?? "all");
  };

  const executeClear = async () => {
    const days = pendingClear === "all" ? undefined : (pendingClear ?? undefined);
    setPendingClear(null);
    setClearing(true);
    try {
      const url = days ? `/telemetry/events?before_days=${days}` : "/telemetry/events";
      const data = await api.delete<{ deleted: number }>(url);
      flash(`${data.deleted} events cleared`, "success");
      loadEvents();
      loadStats();
    } catch (e: unknown) {
      flash((e as Error).message || "Failed to clear", "error");
    } finally {
      setClearing(false);
    }
  };

  const checkUpdates = async () => {
    setChecking(true);
    try {
      await api.post("/telemetry/check-updates");
      flash("Update check started", "success");
      setTimeout(loadConfig, 5000);
    } catch (e: unknown) {
      flash((e as Error).message || "Failed to check", "error");
    } finally {
      setChecking(false);
    }
  };

  if (loading) {
    return (
      <div className="flex items-center justify-center h-64">
        <div className="w-6 h-6 border-2 border-dark-600 border-t-rust-500 rounded-full animate-spin" />
      </div>
    );
  }

  const totalPages = Math.ceil(total / limit);

  return (
    <div className="p-4 sm:p-6 lg:p-8 animate-fade-up">
      <div className="page-header">
        <div>
          <h1 className="page-header-title">Telemetry & Updates</h1>
          <p className="text-xs text-dark-400 mt-0.5">
            Diagnostic reporting and version management — v{config.current_version}
          </p>
        </div>
        <div className="flex items-center gap-2">
          <button onClick={exportReport} className="px-3 py-1.5 bg-dark-800 text-dark-300 hover:bg-dark-700 hover:text-dark-100 border border-dark-600 rounded-lg text-xs transition-colors">
            Export Report
          </button>
          <button onClick={previewReport} className="px-3 py-1.5 bg-dark-800 text-dark-300 hover:bg-dark-700 hover:text-dark-100 border border-dark-600 rounded-lg text-xs transition-colors">
            Preview Report
          </button>
        </div>
      </div>

      {message.text && (
        <div className={`mb-4 px-4 py-2.5 rounded-lg border text-sm ${message.type === "success" ? "bg-emerald-500/10 border-emerald-500/20 text-emerald-400" : "bg-red-500/10 border-red-500/20 text-red-400"}`}>
          {message.text}
        </div>
      )}

      {/* Update banner */}
      {config.update_available_version && (
        <div className="mb-4 px-4 py-3 rounded-lg border border-rust-500/30 bg-rust-500/10 flex items-center justify-between">
          <div className="flex items-center gap-3">
            <svg className="w-5 h-5 text-rust-400 flex-shrink-0" fill="none" viewBox="0 0 24 24" stroke="currentColor" strokeWidth={2}><path strokeLinecap="round" strokeLinejoin="round" d="M4 16v1a3 3 0 003 3h10a3 3 0 003-3v-1m-4-4l-4 4m0 0l-4-4m4 4V4" /></svg>
            <div>
              <span className="text-sm font-medium text-rust-300">
                DockPanel v{config.update_available_version} available
              </span>
              <span className="text-xs text-dark-400 ml-2">
                (current: v{config.current_version})
              </span>
            </div>
          </div>
          {config.update_release_url && (
            <a href={config.update_release_url} target="_blank" rel="noopener noreferrer"
              className="px-3 py-1.5 bg-rust-500 hover:bg-rust-600 text-white rounded-lg text-xs font-medium transition-colors">
              View Release
            </a>
          )}
        </div>
      )}

      {/* Stats cards */}
      {stats && (
        <div className="grid grid-cols-2 sm:grid-cols-4 gap-3 mb-6">
          <div className="bg-dark-800 border border-dark-600 rounded-lg p-3">
            <div className="text-xs text-dark-400 mb-1">Total Events</div>
            <div className="text-xl font-mono font-bold text-dark-100">{stats.total}</div>
          </div>
          <div className="bg-dark-800 border border-dark-600 rounded-lg p-3">
            <div className="text-xs text-dark-400 mb-1">Unsent</div>
            <div className="text-xl font-mono font-bold text-amber-400">{stats.unsent}</div>
          </div>
          <div className="bg-dark-800 border border-dark-600 rounded-lg p-3">
            <div className="text-xs text-dark-400 mb-1">Last 24h</div>
            <div className="text-xl font-mono font-bold text-dark-100">{stats.last_24h}</div>
          </div>
          <div className="bg-dark-800 border border-dark-600 rounded-lg p-3">
            <div className="text-xs text-dark-400 mb-1">Status</div>
            <div className={`text-sm font-medium ${enabled ? "text-emerald-400" : "text-dark-400"}`}>
              {enabled ? "Sending enabled" : "Local only"}
            </div>
          </div>
        </div>
      )}

      {/* Tabs */}
      <div className="flex gap-1 mb-4 border-b border-dark-600 pb-px">
        {(["events", "updates", "config"] as const).map(t => (
          <button key={t} onClick={() => setTab(t)}
            className={`px-4 py-2 text-xs font-medium rounded-t-lg transition-colors ${tab === t ? "bg-dark-700 text-dark-100 border border-dark-600 border-b-dark-900" : "text-dark-400 hover:text-dark-200"}`}>
            {t === "events" ? "Events" : t === "updates" ? "Updates" : "Configuration"}
          </button>
        ))}
      </div>

      {/* Events tab */}
      {tab === "events" && (
        <div>
          {/* Filters */}
          <div className="flex flex-wrap items-center gap-2 mb-4">
            <select value={categoryFilter} onChange={e => { setCategoryFilter(e.target.value); setPage(0); }}
              className="bg-dark-800 border border-dark-600 text-dark-200 text-xs rounded-lg px-3 py-1.5">
              <option value="">All Categories</option>
              {stats?.by_category.map(c => (
                <option key={c.category} value={c.category}>{c.category} ({c.count})</option>
              ))}
            </select>
            <select value={typeFilter} onChange={e => { setTypeFilter(e.target.value); setPage(0); }}
              className="bg-dark-800 border border-dark-600 text-dark-200 text-xs rounded-lg px-3 py-1.5">
              <option value="">All Types</option>
              {stats?.by_type.map(t => (
                <option key={t.type} value={t.type}>{t.type} ({t.count})</option>
              ))}
            </select>
            <div className="flex-1" />
            <button onClick={() => clearEvents(30)} disabled={clearing}
              className="px-3 py-1.5 text-xs text-dark-400 hover:text-dark-200 border border-dark-600 rounded-lg transition-colors disabled:opacity-50">
              Clear &gt; 30 days
            </button>
            <button onClick={() => clearEvents()} disabled={clearing}
              className="px-3 py-1.5 text-xs text-red-400 hover:text-red-300 border border-red-500/20 rounded-lg transition-colors disabled:opacity-50">
              Clear All
            </button>
          </div>

          {/* Confirm clear bar */}
          {pendingClear !== null && (
            <div className="border border-danger-500/30 bg-danger-500/5 rounded-lg px-4 py-3 mb-4 flex items-center justify-between">
              <span className="text-xs text-danger-400 font-mono">
                {pendingClear === "all" ? "Clear ALL telemetry events?" : `Clear events older than ${pendingClear} days?`}
              </span>
              <div className="flex items-center gap-2 shrink-0 ml-4">
                <button onClick={executeClear} className="px-3 py-1.5 bg-danger-500 text-white text-xs font-bold uppercase tracking-wider hover:bg-danger-400 transition-colors">Confirm</button>
                <button onClick={() => setPendingClear(null)} className="px-3 py-1.5 bg-dark-600 text-dark-200 text-xs font-bold uppercase tracking-wider hover:bg-dark-500 transition-colors">Cancel</button>
              </div>
            </div>
          )}

          {/* Events table */}
          {events.length === 0 ? (
            <div className="bg-dark-800 border border-dark-600 rounded-lg p-8 text-center text-dark-400 text-sm">
              No telemetry events recorded yet. Events are captured automatically when errors or issues occur.
            </div>
          ) : (
            <div className="bg-dark-800 border border-dark-600 rounded-lg overflow-hidden">
              <table className="w-full text-xs">
                <thead>
                  <tr className="border-b border-dark-600 text-dark-400">
                    <th className="text-left px-3 py-2 font-medium">Type</th>
                    <th className="text-left px-3 py-2 font-medium">Category</th>
                    <th className="text-left px-3 py-2 font-medium">Message</th>
                    <th className="text-left px-3 py-2 font-medium hidden sm:table-cell">Status</th>
                    <th className="text-left px-3 py-2 font-medium">Time</th>
                  </tr>
                </thead>
                <tbody>
                  {events.map(ev => (
                    <Fragment key={ev.id}>
                      <tr onClick={() => setExpandedEvent(expandedEvent === ev.id ? null : ev.id)}
                        className="border-b border-dark-700 hover:bg-dark-750 cursor-pointer transition-colors">
                        <td className="px-3 py-2">
                          <span className={`px-1.5 py-0.5 rounded text-[10px] font-medium border ${EVENT_TYPE_COLORS[ev.event_type] || "text-dark-300"}`}>
                            {ev.event_type}
                          </span>
                        </td>
                        <td className={`px-3 py-2 font-mono ${CATEGORY_COLORS[ev.category] || "text-dark-300"}`}>
                          {ev.category}
                        </td>
                        <td className="px-3 py-2 text-dark-200 max-w-xs truncate">{ev.message}</td>
                        <td className="px-3 py-2 hidden sm:table-cell">
                          {ev.sent_at ? (
                            <span className="text-emerald-400 text-[10px]">Sent</span>
                          ) : (
                            <span className="text-amber-400 text-[10px]">Pending</span>
                          )}
                        </td>
                        <td className="px-3 py-2 text-dark-400 whitespace-nowrap">{formatDate(ev.created_at)}</td>
                      </tr>
                      {expandedEvent === ev.id && (
                        <tr>
                          <td colSpan={5} className="px-4 py-3 bg-dark-850">
                            <pre className="text-[10px] font-mono text-dark-300 whitespace-pre-wrap overflow-x-auto max-h-48">
                              {JSON.stringify(ev.context, null, 2)}
                            </pre>
                          </td>
                        </tr>
                      )}
                    </Fragment>
                  ))}
                </tbody>
              </table>

              {/* Pagination */}
              {totalPages > 1 && (
                <div className="flex items-center justify-between px-3 py-2 border-t border-dark-600">
                  <span className="text-xs text-dark-400">{total} events total</span>
                  <div className="flex items-center gap-1">
                    <button onClick={() => setPage(p => Math.max(0, p - 1))} disabled={page === 0}
                      className="px-2 py-1 text-xs text-dark-300 hover:text-dark-100 disabled:opacity-30">
                      Prev
                    </button>
                    <span className="text-xs text-dark-400 px-2">{page + 1} / {totalPages}</span>
                    <button onClick={() => setPage(p => Math.min(totalPages - 1, p + 1))} disabled={page >= totalPages - 1}
                      className="px-2 py-1 text-xs text-dark-300 hover:text-dark-100 disabled:opacity-30">
                      Next
                    </button>
                  </div>
                </div>
              )}
            </div>
          )}
        </div>
      )}

      {/* Updates tab */}
      {tab === "updates" && (
        <div className="space-y-4">
          <div className="bg-dark-800 border border-dark-600 rounded-lg p-4">
            <div className="flex items-center justify-between mb-4">
              <h3 className="text-sm font-medium text-dark-100">Version Status</h3>
              <button onClick={checkUpdates} disabled={checking}
                className="px-3 py-1.5 bg-dark-700 text-dark-300 hover:bg-dark-600 hover:text-dark-100 border border-dark-500 rounded-lg text-xs transition-colors disabled:opacity-50">
                {checking ? "Checking..." : "Check Now"}
              </button>
            </div>

            <div className="grid grid-cols-1 sm:grid-cols-3 gap-4">
              <div>
                <div className="text-xs text-dark-400 mb-1">Current Version</div>
                <div className="text-sm font-mono text-dark-100">v{config.current_version}</div>
              </div>
              <div>
                <div className="text-xs text-dark-400 mb-1">Latest Available</div>
                <div className={`text-sm font-mono ${config.update_available_version ? "text-rust-400" : "text-emerald-400"}`}>
                  {config.update_available_version ? `v${config.update_available_version}` : "Up to date"}
                </div>
              </div>
              <div>
                <div className="text-xs text-dark-400 mb-1">Last Checked</div>
                <div className="text-sm text-dark-300">
                  {config.update_checked_at ? formatDate(config.update_checked_at) : "Never"}
                </div>
              </div>
            </div>
          </div>

          {config.update_available_version && config.update_release_notes && (
            <div className="bg-dark-800 border border-dark-600 rounded-lg p-4">
              <div className="flex items-center justify-between mb-3">
                <h3 className="text-sm font-medium text-dark-100">
                  Release Notes — v{config.update_available_version}
                </h3>
                {config.update_release_url && (
                  <a href={config.update_release_url} target="_blank" rel="noopener noreferrer"
                    className="text-xs text-rust-400 hover:text-rust-300 transition-colors">
                    View on GitHub
                  </a>
                )}
              </div>
              <pre className="text-xs font-mono text-dark-300 whitespace-pre-wrap max-h-64 overflow-y-auto bg-dark-900 rounded-lg p-3 border border-dark-700">
                {config.update_release_notes}
              </pre>
            </div>
          )}

          {config.update_available_version && (
            <div className="bg-dark-800 border border-dark-600 rounded-lg p-4">
              <h3 className="text-sm font-medium text-dark-100 mb-3">Update Instructions</h3>
              <div className="bg-dark-900 rounded-lg p-3 border border-dark-700">
                <pre className="text-xs font-mono text-dark-300 whitespace-pre-wrap">{`# SSH into your server and run:
cd /path/to/dockpanel

# Pull latest changes
git pull origin main

# Build new binaries
source ~/.cargo/env
cd panel/agent && cargo build --release && cd ../..
cd panel/backend && cargo build --release && cd ../..

# Deploy
systemctl stop dockpanel-agent dockpanel-api
cp panel/agent/target/release/dockpanel-agent /usr/local/bin/
cp panel/backend/target/release/dockpanel-api /usr/local/bin/
systemctl start dockpanel-agent dockpanel-api

# Verify
systemctl is-active dockpanel-agent dockpanel-api`}</pre>
              </div>
            </div>
          )}
        </div>
      )}

      {/* Config tab */}
      {tab === "config" && (
        <div className="space-y-4">
          <div className="bg-dark-800 border border-dark-600 rounded-lg p-4">
            <h3 className="text-sm font-medium text-dark-100 mb-4">Telemetry Configuration</h3>

            <div className="space-y-4">
              <div className="flex items-center justify-between">
                <div>
                  <div className="text-sm text-dark-200">Enable Remote Telemetry</div>
                  <div className="text-xs text-dark-400 mt-0.5">
                    Send diagnostic events to a remote endpoint for analysis
                  </div>
                </div>
                <button onClick={() => setEnabled(!enabled)}
                  className={`relative w-10 h-5 rounded-full transition-colors ${enabled ? "bg-rust-500" : "bg-dark-600"}`}>
                  <span className={`absolute top-0.5 left-0.5 w-4 h-4 bg-white rounded-full transition-transform ${enabled ? "translate-x-5" : ""}`} />
                </button>
              </div>

              <div>
                <label className="text-xs text-dark-400 mb-1 block">Endpoint URL (HTTPS required)</label>
                <input type="url" value={endpoint} onChange={e => setEndpoint(e.target.value)}
                  placeholder="https://telemetry.example.com/collect"
                  className="w-full bg-dark-900 border border-dark-600 rounded-lg px-3 py-2 text-sm text-dark-200 placeholder:text-dark-500 focus:outline-none focus:border-rust-500" />
              </div>

              <div className="flex items-center gap-2">
                <button onClick={saveConfig} disabled={saving}
                  className="px-4 py-2 bg-rust-500 hover:bg-rust-600 text-white rounded-lg text-xs font-medium transition-colors disabled:opacity-50">
                  {saving ? "Saving..." : "Save Configuration"}
                </button>
                {enabled && endpoint && (
                  <button onClick={sendNow} disabled={sending}
                    className="px-4 py-2 bg-dark-700 text-dark-200 hover:bg-dark-600 border border-dark-500 rounded-lg text-xs font-medium transition-colors disabled:opacity-50">
                    {sending ? "Sending..." : "Send Now"}
                  </button>
                )}
              </div>
            </div>
          </div>

          <div className="bg-dark-800 border border-dark-600 rounded-lg p-4">
            <h3 className="text-sm font-medium text-dark-100 mb-3">Privacy</h3>
            <div className="space-y-2 text-xs text-dark-300">
              <p>Telemetry is <strong className="text-dark-100">completely opt-in</strong>. When disabled, all events are stored locally only.</p>
              <p>When enabled, the following is collected:</p>
              <ul className="list-disc list-inside space-y-1 ml-2">
                <li>Error messages and stack context (no file paths or user data)</li>
                <li>Service health status (running/stopped)</li>
                <li>System specs (OS, RAM, CPU count — no IP addresses or hostnames)</li>
                <li>DockPanel version</li>
              </ul>
              <p>All personal information (IPs, emails, domains, usernames, tokens) is <strong className="text-dark-100">automatically stripped</strong> before sending.</p>
              <p>Use the <strong className="text-dark-100">Preview Report</strong> button to see exactly what would be sent.</p>
            </div>
            {config.telemetry_installation_id && (
              <div className="mt-3 pt-3 border-t border-dark-700">
                <span className="text-xs text-dark-400">Installation ID: </span>
                <span className="text-xs font-mono text-dark-300">{config.telemetry_installation_id}</span>
              </div>
            )}
          </div>

          {/* Category breakdown */}
          {stats && stats.by_category.length > 0 && (
            <div className="bg-dark-800 border border-dark-600 rounded-lg p-4">
              <h3 className="text-sm font-medium text-dark-100 mb-3">Events by Category</h3>
              <div className="space-y-2">
                {stats.by_category.map(c => (
                  <div key={c.category} className="flex items-center justify-between">
                    <span className={`text-xs font-mono ${CATEGORY_COLORS[c.category] || "text-dark-300"}`}>{c.category}</span>
                    <div className="flex items-center gap-2">
                      <div className="w-24 h-1.5 bg-dark-700 rounded-full overflow-hidden">
                        <div className="h-full bg-rust-500 rounded-full" style={{ width: `${Math.min(100, (c.count / stats.total) * 100)}%` }} />
                      </div>
                      <span className="text-xs text-dark-400 w-8 text-right">{c.count}</span>
                    </div>
                  </div>
                ))}
              </div>
            </div>
          )}
        </div>
      )}

      {/* Preview modal */}
      {showPreview && previewData && (
        <div className="fixed inset-0 bg-black/60 flex items-center justify-center z-50 p-4" onClick={() => setShowPreview(false)}>
          <div className="bg-dark-800 border border-dark-600 rounded-xl max-w-3xl w-full max-h-[80vh] overflow-hidden flex flex-col" onClick={e => e.stopPropagation()}>
            <div className="flex items-center justify-between px-4 py-3 border-b border-dark-600">
              <h3 className="text-sm font-medium text-dark-100">Telemetry Report Preview</h3>
              <button onClick={() => setShowPreview(false)} className="text-dark-400 hover:text-dark-200">
                <svg className="w-4 h-4" fill="none" viewBox="0 0 24 24" stroke="currentColor" strokeWidth={2}><path strokeLinecap="round" strokeLinejoin="round" d="M6 18L18 6M6 6l12 12" /></svg>
              </button>
            </div>
            <div className="overflow-y-auto p-4">
              <pre className="text-[10px] font-mono text-dark-300 whitespace-pre-wrap">
                {JSON.stringify(previewData, null, 2)}
              </pre>
            </div>
            <div className="px-4 py-2 border-t border-dark-600 text-xs text-dark-400">
              This is exactly what would be sent. All PII has been stripped.
            </div>
          </div>
        </div>
      )}
    </div>
  );
}
