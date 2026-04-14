import { useState, useEffect, FormEvent } from "react";
import { api } from "../api";
import { formatDate } from "../utils/format";

interface Monitor {
  id: string;
  url: string;
  name: string;
  check_interval: number;
  status: string;
  last_checked_at: string | null;
  last_response_time: number | null;
  last_status_code: number | null;
  enabled: boolean;
  alert_email: boolean;
  alert_slack_url: string | null;
  alert_discord_url: string | null;
  monitor_type: string;
  port: number | null;
  keyword: string | null;
  keyword_must_contain: boolean;
  custom_headers: Record<string, string> | null;
  created_at: string;
}

interface UptimeData {
  uptime_24h: number;
  uptime_7d: number;
  uptime_30d: number;
  avg_response_ms: number;
}

interface ChartPoint {
  time: number;
  ms: number;
}

const Sparkline = ({ data }: { data: ChartPoint[] }) => {
  if (data.length < 2) return null;
  const max = Math.max(...data.map(d => d.ms), 1);
  const w = 300, h = 40;
  const points = data.map((d, i) => `${(i / (data.length - 1)) * w},${h - (d.ms / max) * (h - 4)}`).join(' ');
  return (
    <svg viewBox={`0 0 ${w} ${h}`} className="w-full h-10">
      <polyline fill="none" stroke="currentColor" strokeWidth="1.5" className="text-rust-400" points={points} />
    </svg>
  );
};

interface CheckRecord {
  id: string;
  status_code: number | null;
  response_time: number | null;
  error: string | null;
  checked_at: string;
}

interface Incident {
  id: string;
  started_at: string;
  resolved_at: string | null;
  cause: string | null;
}

const statusColors: Record<string, string> = {
  up: "bg-rust-500/15 text-rust-400",
  down: "bg-danger-500/15 text-danger-400",
  pending: "bg-dark-700 text-dark-200",
};

const statusDot: Record<string, string> = {
  up: "bg-rust-500",
  down: "bg-danger-500",
  pending: "bg-dark-400",
};

const TYPE_LABELS: Record<string, string> = {
  http: "HTTP(S)",
  tcp: "TCP Port",
  ping: "Ping",
  heartbeat: "Heartbeat",
};

export default function Monitors() {
  const [monitors, setMonitors] = useState<Monitor[]>([]);
  const [loading, setLoading] = useState(true);
  const [showForm, setShowForm] = useState(false);
  const [error, setError] = useState("");
  const [success, setSuccess] = useState("");
  const [expanded, setExpanded] = useState<string | null>(null);
  const [checks, setChecks] = useState<CheckRecord[]>([]);
  const [incidents, setIncidents] = useState<Incident[]>([]);

  // Delete confirmation state
  const [deleteTarget, setDeleteTarget] = useState<string | null>(null);

  // Form state
  const [formName, setFormName] = useState("");
  const [formUrl, setFormUrl] = useState("");
  const [formInterval, setFormInterval] = useState("60");
  const [formSlackUrl, setFormSlackUrl] = useState("");
  const [formDiscordUrl, setFormDiscordUrl] = useState("");
  const [formMonitorType, setFormMonitorType] = useState("http");
  const [formPort, setFormPort] = useState("");
  const [formKeyword, setFormKeyword] = useState("");
  const [formKeywordMustContain, setFormKeywordMustContain] = useState(true);
  const [prevAutoName, setPrevAutoName] = useState("");
  const [submitting, setSubmitting] = useState(false);
  const [displayCount, setDisplayCount] = useState(25);

  // Custom headers state
  const [customHeaders, setCustomHeaders] = useState<{ key: string; value: string }[]>([]);
  const [showHeaders, setShowHeaders] = useState(false);

  // Uptime + chart data (per expanded monitor)
  const [uptimeData, setUptimeData] = useState<UptimeData | null>(null);
  const [chartData, setChartData] = useState<ChartPoint[]>([]);
  const [checkNowMsg, setCheckNowMsg] = useState("");

  // Global notification defaults (pre-fill from Settings)
  const [globalSlackUrl, setGlobalSlackUrl] = useState("");
  const [globalDiscordUrl, setGlobalDiscordUrl] = useState("");

  const fetchMonitors = () => {
    api.get<Monitor[]>("/monitors")
      .then(setMonitors)
      .catch((e) => setError(e.message))
      .finally(() => setLoading(false));
  };

  useEffect(() => {
    fetchMonitors();
    const id = setInterval(fetchMonitors, 30000);

    // Fetch global notification channels to pre-fill form defaults
    api.get<{ notify_slack_url?: string; notify_discord_url?: string }[]>("/alert-rules")
      .then((rules) => {
        if (rules.length > 0) {
          setGlobalSlackUrl(rules[0].notify_slack_url || "");
          setGlobalDiscordUrl(rules[0].notify_discord_url || "");
        }
      })
      .catch(() => {});

    return () => clearInterval(id);
  }, []);

  const handleUrlChange = (url: string) => {
    setFormUrl(url);
    if (formMonitorType === "tcp" || formMonitorType === "ping") {
      // For TCP/ping, the URL is just a hostname — use it as auto-name
      if (!formName || formName === prevAutoName) {
        setFormName(url.trim());
        setPrevAutoName(url.trim());
      }
    } else if (formMonitorType === "http") {
      try {
        const hostname = new URL(url).hostname;
        if (!formName || formName === prevAutoName) {
          setFormName(hostname);
          setPrevAutoName(hostname);
        }
      } catch {}
    }
  };

  const handleCreate = async (e: FormEvent) => {
    e.preventDefault();
    setError("");
    setSubmitting(true);
    try {
      // Build custom headers object
      const headers: Record<string, string> = {};
      customHeaders.forEach((h) => {
        if (h.key.trim()) headers[h.key.trim()] = h.value;
      });

      await api.post("/monitors", {
        name: formName,
        url: formMonitorType === "heartbeat" ? (formUrl || formName) : formUrl,
        check_interval: parseInt(formInterval),
        alert_slack_url: formSlackUrl || null,
        alert_discord_url: formDiscordUrl || null,
        monitor_type: formMonitorType,
        port: formMonitorType === "tcp" && formPort ? parseInt(formPort) : null,
        keyword: formKeyword || null,
        keyword_must_contain: formKeywordMustContain,
        custom_headers: Object.keys(headers).length > 0 ? headers : null,
      });
      setShowForm(false);
      setFormName("");
      setFormUrl("");
      setFormInterval("60");
      setFormSlackUrl("");
      setFormDiscordUrl("");
      setFormMonitorType("http");
      setFormPort("");
      setFormKeyword("");
      setFormKeywordMustContain(true);
      setPrevAutoName("");
      setCustomHeaders([]);
      setShowHeaders(false);
      setSuccess("Monitor created");
      fetchMonitors();
    } catch (err) {
      setError(err instanceof Error ? err.message : "Failed to create monitor");
    } finally {
      setSubmitting(false);
    }
  };

  const handleToggle = async (id: string, enabled: boolean) => {
    try {
      await api.put(`/monitors/${id}`, { enabled: !enabled });
      setSuccess(!enabled ? "Monitor enabled" : "Monitor paused");
      fetchMonitors();
    } catch (err) {
      setError(err instanceof Error ? err.message : "Toggle failed");
    }
  };

  const handleDelete = async (id: string) => {
    try {
      await api.delete(`/monitors/${id}`);
      setDeleteTarget(null);
      if (expanded === id) setExpanded(null);
      setSuccess("Monitor deleted");
      fetchMonitors();
    } catch (err) {
      setError(err instanceof Error ? err.message : "Delete failed");
      setDeleteTarget(null);
    }
  };

  const toggleExpand = async (id: string) => {
    if (expanded === id) {
      setExpanded(null);
      return;
    }
    setExpanded(id);
    setUptimeData(null);
    setChartData([]);
    setCheckNowMsg("");
    try {
      const [c, i, u, ch] = await Promise.all([
        api.get<CheckRecord[]>(`/monitors/${id}/checks`),
        api.get<Incident[]>(`/monitors/${id}/incidents`),
        api.get<UptimeData>(`/monitors/${id}/uptime`),
        api.get<{ points: ChartPoint[] }>(`/monitors/${id}/chart`),
      ]);
      setChecks(c);
      setIncidents(i);
      setUptimeData(u);
      setChartData(ch.points || []);
    } catch (err) {
      setError(err instanceof Error ? err.message : "Failed to load details");
    }
  };

  const handleForceCheck = async (id: string) => {
    try {
      const res = await api.post<{ message: string }>(`/monitors/${id}/check`, {});
      setCheckNowMsg(res.message || "Check queued");
      setTimeout(() => setCheckNowMsg(""), 5000);
    } catch (err) {
      setError(err instanceof Error ? err.message : "Force check failed");
    }
  };

  // URL label depends on monitor type
  const getUrlLabel = () => {
    switch (formMonitorType) {
      case "tcp": return "Host";
      case "ping": return "Host / IP";
      case "heartbeat": return "Identifier";
      default: return "URL";
    }
  };

  const getUrlPlaceholder = () => {
    switch (formMonitorType) {
      case "tcp": return "db.example.com";
      case "ping": return "192.168.1.1 or example.com";
      case "heartbeat": return "my-cron-job";
      default: return "https://example.com";
    }
  };

  const getUrlHint = () => {
    switch (formMonitorType) {
      case "tcp": return "Hostname or IP address";
      case "ping": return "Hostname or IP to ping via ICMP";
      case "heartbeat": return "Name/identifier for this heartbeat endpoint";
      default: return "The full URL to monitor, including https://";
    }
  };

  // Summary
  const upCount = monitors.filter((m) => m.status === "up").length;
  const downCount = monitors.filter((m) => m.status === "down").length;

  if (loading) {
    return (
      <div className="animate-fade-up">
        <div className="bg-dark-800 rounded-lg border border-dark-500 p-6 animate-pulse">
          <div className="h-6 bg-dark-700 rounded w-48 mb-4" />
          <div className="h-4 bg-dark-700 rounded w-32" />
        </div>
      </div>
    );
  }

  return (
    <div>
      <div className="page-header">
        <div>
          <h1 className="page-header-title">Monitoring</h1>
          <p className="page-header-subtitle">
            {monitors.length > 0 ? (
              <>{upCount} up, {downCount} down, {monitors.length - upCount - downCount} pending</>
            ) : (
              "Monitor your sites and services"
            )}
          </p>
        </div>
        <div className="flex items-center gap-2">
          <button
            onClick={() => {
              if (!showForm) {
                // Pre-fill notification fields with global defaults
                setFormSlackUrl(globalSlackUrl);
                setFormDiscordUrl(globalDiscordUrl);
              }
              setShowForm(!showForm);
            }}
            className="px-4 py-2 bg-rust-500 text-white rounded-lg text-sm font-medium hover:bg-rust-600 transition-colors"
          >
            Add Monitor
          </button>
        </div>
      </div>

      <div className="p-6 lg:p-8">

      {success && (
        <div className="bg-rust-500/10 text-rust-400 text-sm px-4 py-3 rounded-lg border border-rust-500/20 mb-4">
          {success}
          <button onClick={() => setSuccess("")} className="ml-2 font-medium hover:underline">Dismiss</button>
        </div>
      )}
      {error && (
        <div className="bg-danger-500/10 text-danger-400 text-sm px-4 py-3 rounded-lg border border-danger-500/20 mb-4">
          {error}
          <button onClick={() => setError("")} className="ml-2 font-medium hover:underline">Dismiss</button>
        </div>
      )}

      {/* Create form */}
      {showForm && (
        <form onSubmit={handleCreate} className="bg-dark-800 rounded-lg border border-dark-500 p-5 mb-6">
          <h3 className="text-xs font-medium text-dark-300 uppercase font-mono tracking-widest mb-3">New Monitor</h3>
          <div className="grid grid-cols-2 gap-4 mb-4">
            <div>
              <label className="block text-xs font-medium text-dark-200 mb-1">Monitor Type</label>
              <select value={formMonitorType} onChange={(e) => { setFormMonitorType(e.target.value); setFormUrl(""); setFormPort(""); setCustomHeaders([]); setShowHeaders(false); }} className="w-full px-3 py-2 border border-dark-500 rounded-lg text-sm focus:ring-2 focus:ring-accent-500 focus:border-accent-500 outline-none">
                <option value="http">HTTP(S)</option>
                <option value="tcp">TCP Port</option>
                <option value="ping">Ping (ICMP)</option>
                <option value="heartbeat">Heartbeat (Dead Man's Switch)</option>
              </select>
            </div>
            <div>
              <label className="block text-xs font-medium text-dark-200 mb-1">Name</label>
              <input type="text" value={formName} onChange={(e) => setFormName(e.target.value)} required placeholder="My Website" className="w-full px-3 py-2 border border-dark-500 rounded-lg text-sm focus:ring-2 focus:ring-accent-500 focus:border-accent-500 outline-none" />
            </div>
            {formMonitorType !== "heartbeat" && (
              <div>
                <label className="block text-xs font-medium text-dark-200 mb-1">{getUrlLabel()}</label>
                <input type={formMonitorType === "http" ? "url" : "text"} value={formUrl} onChange={(e) => handleUrlChange(e.target.value)} required placeholder={getUrlPlaceholder()} className="w-full px-3 py-2 border border-dark-500 rounded-lg text-sm focus:ring-2 focus:ring-accent-500 focus:border-accent-500 outline-none" />
                <p className="text-xs text-dark-300 mt-1">{getUrlHint()}</p>
              </div>
            )}
            {formMonitorType === "heartbeat" && (
              <div>
                <label className="block text-xs font-medium text-dark-200 mb-1">{getUrlLabel()}</label>
                <input type="text" value={formUrl} onChange={(e) => setFormUrl(e.target.value)} placeholder={getUrlPlaceholder()} className="w-full px-3 py-2 border border-dark-500 rounded-lg text-sm focus:ring-2 focus:ring-accent-500 focus:border-accent-500 outline-none" />
                <p className="text-xs text-dark-300 mt-1">{getUrlHint()}</p>
              </div>
            )}
            {formMonitorType === "tcp" && (
              <div>
                <label className="block text-xs font-medium text-dark-200 mb-1">Port</label>
                <input type="number" value={formPort} onChange={(e) => setFormPort(e.target.value)} placeholder="3306" min="1" max="65535" className="w-full px-3 py-2 border border-dark-500 rounded-lg text-sm focus:ring-2 focus:ring-accent-500 focus:border-accent-500 outline-none" />
                <p className="text-xs text-dark-300 mt-1">TCP port to check (e.g. 3306 for MySQL, 5432 for PostgreSQL)</p>
              </div>
            )}
            <div>
              <label className="block text-xs font-medium text-dark-200 mb-1">
                {formMonitorType === "heartbeat" ? "Expected Interval" : "Check Interval"}
              </label>
              <select value={formInterval} onChange={(e) => setFormInterval(e.target.value)} className="w-full px-3 py-2 border border-dark-500 rounded-lg text-sm focus:ring-2 focus:ring-accent-500 focus:border-accent-500 outline-none">
                <option value="30">30 seconds</option>
                <option value="60">1 minute</option>
                <option value="300">5 minutes</option>
                <option value="600">10 minutes</option>
                <option value="1800">30 minutes</option>
                <option value="3600">1 hour</option>
              </select>
              <p className="text-xs text-dark-300 mt-1">
                {formMonitorType === "heartbeat"
                  ? "How often the external service should ping this endpoint"
                  : "How often to check, in seconds"}
              </p>
            </div>
          </div>

          {/* Heartbeat info */}
          {formMonitorType === "heartbeat" && (
            <div className="mb-4 p-3 bg-dark-700/50 rounded-lg border border-dark-600">
              <p className="text-xs text-dark-200">
                After creating this monitor, you'll receive a heartbeat URL. Configure your cron job or service to POST to that URL on each successful run.
                If no ping is received within 2x the expected interval, an alert will fire.
              </p>
            </div>
          )}

          {/* Keyword monitoring (HTTP only) */}
          {formMonitorType === "http" && (
            <div className="mb-4 space-y-2">
              <div>
                <label className="block text-xs font-medium text-dark-200 mb-1">Keyword Check (optional)</label>
                <input type="text" value={formKeyword} onChange={(e) => setFormKeyword(e.target.value)} placeholder="Keyword to check in response body" className="w-full px-3 py-2 border border-dark-500 rounded-lg text-sm focus:ring-2 focus:ring-accent-500 focus:border-accent-500 outline-none" />
              </div>
              {formKeyword && (
                <label className="flex items-center gap-2 text-xs text-dark-200">
                  <input type="checkbox" checked={formKeywordMustContain} onChange={(e) => setFormKeywordMustContain(e.target.checked)} className="rounded border-dark-500" />
                  Response must contain this keyword (uncheck = must NOT contain)
                </label>
              )}
            </div>
          )}

          {/* Custom Headers (HTTP only) */}
          {formMonitorType === "http" && (
            <div className="mb-4">
              <button
                type="button"
                onClick={() => setShowHeaders(!showHeaders)}
                className="text-xs text-rust-400 hover:text-rust-300 font-medium"
              >
                {showHeaders ? "- Hide Custom Headers" : "+ Custom Headers (optional)"}
              </button>
              {showHeaders && (
                <div className="mt-2 space-y-1">
                  {customHeaders.map((h, i) => (
                    <div key={i} className="flex gap-2">
                      <input
                        type="text"
                        placeholder="Header-Name"
                        value={h.key}
                        onChange={(e) => {
                          const updated = [...customHeaders];
                          updated[i] = { ...updated[i], key: e.target.value };
                          setCustomHeaders(updated);
                        }}
                        className="w-1/3 px-3 py-1.5 border border-dark-500 rounded-lg text-xs focus:ring-2 focus:ring-accent-500 outline-none font-mono"
                      />
                      <input
                        type="text"
                        placeholder="value"
                        value={h.value}
                        onChange={(e) => {
                          const updated = [...customHeaders];
                          updated[i] = { ...updated[i], value: e.target.value };
                          setCustomHeaders(updated);
                        }}
                        className="flex-1 px-3 py-1.5 border border-dark-500 rounded-lg text-xs focus:ring-2 focus:ring-accent-500 outline-none font-mono"
                      />
                      <button
                        type="button"
                        onClick={() => setCustomHeaders(customHeaders.filter((_, idx) => idx !== i))}
                        className="text-danger-400 hover:text-danger-300 px-1 text-sm font-bold"
                      >
                        x
                      </button>
                    </div>
                  ))}
                  <button
                    type="button"
                    onClick={() => setCustomHeaders([...customHeaders, { key: "", value: "" }])}
                    className="text-xs text-rust-400 hover:text-rust-300"
                  >
                    + Add header
                  </button>
                  <p className="text-[10px] text-dark-300 mt-1">Send custom headers with each HTTP check (e.g. Authorization, X-API-Key)</p>
                </div>
              )}
            </div>
          )}

          <div className="grid grid-cols-2 gap-4 mb-4">
            <div>
              <label className="block text-xs font-medium text-dark-200 mb-1">Slack Webhook URL</label>
              <input type="url" value={formSlackUrl} onChange={(e) => setFormSlackUrl(e.target.value)} placeholder="https://hooks.slack.com/services/..." className="w-full px-3 py-2 border border-dark-500 rounded-lg text-sm focus:ring-2 focus:ring-accent-500 focus:border-accent-500 outline-none font-mono" />
              {formSlackUrl && formSlackUrl === globalSlackUrl && <p className="text-[10px] text-dark-300 mt-0.5">Inherited from global settings</p>}
            </div>
            <div>
              <label className="block text-xs font-medium text-dark-200 mb-1">Discord Webhook URL</label>
              <input type="url" value={formDiscordUrl} onChange={(e) => setFormDiscordUrl(e.target.value)} placeholder="https://discord.com/api/webhooks/..." className="w-full px-3 py-2 border border-dark-500 rounded-lg text-sm focus:ring-2 focus:ring-accent-500 focus:border-accent-500 outline-none font-mono" />
              {formDiscordUrl && formDiscordUrl === globalDiscordUrl && <p className="text-[10px] text-dark-300 mt-0.5">Inherited from global settings</p>}
            </div>
          </div>
          <div className="flex gap-3">
            <button type="submit" disabled={submitting} className="flex items-center gap-2 px-4 py-2 bg-rust-500 text-white rounded-lg text-sm font-medium hover:bg-rust-600 disabled:opacity-50">
              {submitting && <span className="w-4 h-4 border-2 border-white/30 border-t-white rounded-full animate-spin" />}
              {submitting ? "Adding..." : "Create Monitor"}
            </button>
            <button type="button" onClick={() => setShowForm(false)} className="px-4 py-2 text-sm text-dark-300 border border-dark-600 rounded-lg hover:text-dark-100 hover:border-dark-400 transition-colors">
              Cancel
            </button>
          </div>
        </form>
      )}

      {/* Monitor list */}
      {!showForm && monitors.length === 0 ? (
        <div className="bg-dark-800 rounded-lg border border-dark-500 p-12 text-center">
          <svg className="w-12 h-12 text-dark-300 mx-auto mb-4" fill="none" viewBox="0 0 24 24" stroke="currentColor" strokeWidth={1}>
            <path strokeLinecap="round" strokeLinejoin="round" d="M12 6v6h4.5m4.5 0a9 9 0 1 1-18 0 9 9 0 0 1 18 0Z" />
          </svg>
          <p className="text-dark-200 font-medium">No monitors yet</p>
          <p className="text-dark-300 text-sm mt-2 max-w-md mx-auto">Monitor HTTP endpoints, TCP ports, and ping targets with customizable intervals, alerting via Slack, Discord, and email, and a public status page.</p>
          <button onClick={() => { setFormSlackUrl(globalSlackUrl); setFormDiscordUrl(globalDiscordUrl); setShowForm(true); }} className="mt-3 px-4 py-2 bg-rust-500 text-white rounded-lg text-sm font-medium hover:bg-rust-600 transition-colors">
            Add your first monitor
          </button>
        </div>
      ) : monitors.length > 0 ? (
        <div className="space-y-3">
          {monitors.slice(0, displayCount).map((m) => (
            <div key={m.id} className="bg-dark-800 rounded-lg border border-dark-500 card-interactive">
              <div className="p-4 cursor-pointer" onClick={() => toggleExpand(m.id)}>
                <div className="flex items-start sm:items-center justify-between gap-2">
                  <div className="flex items-center gap-3 min-w-0">
                    <div className={`w-2.5 h-2.5 rounded-full shrink-0 ${statusDot[m.status] || "bg-dark-400"} ${m.status === "up" ? "animate-pulse" : ""}`} />
                    <div className="min-w-0">
                      <div className="flex items-center gap-2">
                        <p className="text-sm font-medium text-dark-50 truncate">{m.name}</p>
                        {m.monitor_type !== "http" && (
                          <span className="text-[10px] px-1.5 py-0.5 rounded bg-dark-700 text-dark-200 font-mono shrink-0">
                            {TYPE_LABELS[m.monitor_type] || m.monitor_type.toUpperCase()}
                            {m.monitor_type === "tcp" && m.port ? `:${m.port}` : ""}
                          </span>
                        )}
                      </div>
                      <p className="text-xs text-dark-200 font-mono truncate">
                        {m.monitor_type === "heartbeat" ? `Heartbeat: ${m.url}` : m.url}
                      </p>
                    </div>
                  </div>
                  <div className="flex items-center gap-2 sm:gap-4 shrink-0">
                    {m.last_response_time != null && m.monitor_type !== "heartbeat" && (
                      <span className={`text-xs font-medium font-mono ${m.last_response_time > 2000 ? "text-danger-500" : m.last_response_time > 500 ? "text-warn-500" : "text-rust-400"}`}>
                        {m.last_response_time}ms
                      </span>
                    )}
                    <span className={`inline-flex px-2 py-0.5 rounded-full text-xs font-medium ${statusColors[m.status] || "bg-dark-700 text-dark-200"}`}>
                      {m.status}
                    </span>
                    <button
                      onClick={(e) => { e.stopPropagation(); handleToggle(m.id, m.enabled); }}
                      role="switch"
                      aria-checked={m.enabled}
                      aria-label={m.enabled ? "Pause monitor" : "Resume monitor"}
                      className={`relative inline-flex h-5 w-9 items-center rounded-full transition-colors shrink-0 ${m.enabled ? "bg-rust-500" : "bg-dark-600"}`}
                    >
                      <span className={`inline-block h-3.5 w-3.5 transform rounded-full bg-white transition-transform ${m.enabled ? "translate-x-4" : "translate-x-1"}`} />
                    </button>
                    {deleteTarget === m.id ? (
                      <div className="flex items-center gap-1" onClick={(e) => e.stopPropagation()}>
                        <button onClick={() => handleDelete(m.id)} className="px-2 py-1 bg-danger-600 text-white rounded-md text-xs">Confirm</button>
                        <button onClick={() => setDeleteTarget(null)} className="px-2 py-1 bg-dark-600 text-dark-200 rounded-md text-xs">Cancel</button>
                      </div>
                    ) : (
                      <button
                        onClick={(e) => { e.stopPropagation(); setDeleteTarget(m.id); }}
                        className="p-1 text-dark-300 hover:text-danger-500 transition-colors shrink-0"
                        title="Delete"
                        aria-label="Delete monitor"
                      >
                        <svg className="w-4 h-4" fill="none" viewBox="0 0 24 24" stroke="currentColor" strokeWidth={2}>
                          <path strokeLinecap="round" strokeLinejoin="round" d="m14.74 9-.346 9m-4.788 0L9.26 9m9.968-3.21c.342.052.682.107 1.022.166m-1.022-.165L18.16 19.673a2.25 2.25 0 0 1-2.244 2.077H8.084a2.25 2.25 0 0 1-2.244-2.077L4.772 5.79m14.456 0a48.108 48.108 0 0 0-3.478-.397m-12 .562c.34-.059.68-.114 1.022-.165m0 0a48.11 48.11 0 0 1 3.478-.397m7.5 0v-.916c0-1.18-.91-2.164-2.09-2.201a51.964 51.964 0 0 0-3.32 0c-1.18.037-2.09 1.022-2.09 2.201v.916m7.5 0a48.667 48.667 0 0 0-7.5 0" />
                        </svg>
                      </button>
                    )}
                  </div>
                </div>
                {m.last_checked_at && (
                  <p className="text-xs text-dark-300 mt-1 ml-5.5">
                    Last checked {formatDate(m.last_checked_at)} · every {m.check_interval}s
                  </p>
                )}
              </div>

              {/* Expanded details */}
              {expanded === m.id && (
                <div className="border-t border-dark-600 p-4">
                  {/* Heartbeat URL */}
                  {m.monitor_type === "heartbeat" && (
                    <div className="mb-4 p-3 bg-dark-700/50 rounded-lg border border-dark-600">
                      <p className="text-xs font-medium text-dark-100 mb-1">Heartbeat URL</p>
                      <code className="text-xs text-rust-400 font-mono break-all select-all">
                        {window.location.origin}/api/heartbeat/{m.id}/ping
                      </code>
                      <p className="text-[10px] text-dark-300 mt-1">
                        POST to this URL from your cron/service. If no ping within {m.check_interval * 2}s, an alert fires.
                      </p>
                    </div>
                  )}

                  {/* Uptime stats + Check Now */}
                  <div className="flex items-center justify-between mb-4">
                    <div className="flex items-center gap-4">
                      {uptimeData && (
                        <>
                          <div className="text-center">
                            <p className="text-lg font-mono font-bold text-rust-400">{uptimeData.uptime_24h}%</p>
                            <p className="text-[10px] text-dark-300">24h</p>
                          </div>
                          <div className="text-center">
                            <p className="text-lg font-mono font-bold text-dark-100">{uptimeData.uptime_7d}%</p>
                            <p className="text-[10px] text-dark-300">7d</p>
                          </div>
                          <div className="text-center">
                            <p className="text-lg font-mono font-bold text-dark-100">{uptimeData.uptime_30d}%</p>
                            <p className="text-[10px] text-dark-300">30d</p>
                          </div>
                          <div className="text-center">
                            <p className="text-lg font-mono font-bold text-dark-200">{uptimeData.avg_response_ms}ms</p>
                            <p className="text-[10px] text-dark-300">avg</p>
                          </div>
                        </>
                      )}
                    </div>
                    <div className="flex items-center gap-2">
                      {checkNowMsg && <span className="text-xs text-rust-400">{checkNowMsg}</span>}
                      {m.monitor_type !== "heartbeat" && (
                        <button
                          onClick={(e) => { e.stopPropagation(); handleForceCheck(m.id); }}
                          className="px-3 py-1.5 text-xs text-rust-400 hover:text-rust-300 border border-dark-600 rounded-lg hover:border-dark-400 transition-colors"
                        >
                          Check Now
                        </button>
                      )}
                    </div>
                  </div>

                  {/* Response time sparkline */}
                  {chartData.length >= 2 && (
                    <div className="mb-4">
                      <h4 className="text-xs font-semibold text-dark-100 mb-1">Response Time (24h)</h4>
                      <Sparkline data={chartData} />
                    </div>
                  )}

                  <div className="grid grid-cols-1 md:grid-cols-2 gap-6">
                    {/* Recent checks */}
                    <div>
                      <h4 className="text-xs font-semibold text-dark-100 mb-2">Recent Checks</h4>
                      {checks.length === 0 ? (
                        <p className="text-xs text-dark-300">No checks yet</p>
                      ) : (
                        <div className="space-y-1 max-h-48 overflow-y-auto">
                          {checks.slice(0, 20).map((c) => (
                            <div key={c.id} className="flex items-center justify-between text-xs">
                              <div className="flex items-center gap-2">
                                <div className={`w-1.5 h-1.5 rounded-full ${c.status_code !== null && c.status_code >= 0 && c.status_code < 400 ? "bg-rust-500" : "bg-danger-500"}`} />
                                <span className="text-dark-200">{formatDate(c.checked_at)}</span>
                              </div>
                              <div className="flex items-center gap-2">
                                {c.status_code != null && c.status_code > 0 && <span className="text-dark-200 font-mono">{c.status_code}</span>}
                                {c.status_code === 0 && <span className="text-dark-200 font-mono">{m.monitor_type === "ping" ? "Ping OK" : "TCP OK"}</span>}
                                {c.response_time != null && <span className="text-dark-300 font-mono">{c.response_time}ms</span>}
                                {c.error && <span className="text-danger-500 truncate max-w-32">{c.error}</span>}
                              </div>
                            </div>
                          ))}
                        </div>
                      )}
                    </div>

                    {/* Incidents */}
                    <div>
                      <h4 className="text-xs font-semibold text-dark-100 mb-2">Incidents</h4>
                      {incidents.length === 0 ? (
                        <p className="text-xs text-dark-300">No incidents recorded</p>
                      ) : (
                        <div className="space-y-2 max-h-48 overflow-y-auto">
                          {incidents.map((i) => (
                            <div key={i.id} className="text-xs border border-dark-600 rounded-lg p-2">
                              <div className="flex items-center justify-between">
                                <span className={`font-medium ${i.resolved_at ? "text-rust-400" : "text-danger-400"}`}>
                                  {i.resolved_at ? "Resolved" : "Ongoing"}
                                </span>
                                <span className="text-dark-300">{formatDate(i.started_at)}</span>
                              </div>
                              {i.cause && <p className="text-dark-200 mt-1 truncate">{i.cause}</p>}
                              {i.resolved_at && (
                                <p className="text-dark-300 mt-0.5">
                                  Duration: {Math.round((new Date(i.resolved_at).getTime() - new Date(i.started_at).getTime()) / 1000)}s
                                </p>
                              )}
                            </div>
                          ))}
                        </div>
                      )}
                    </div>
                  </div>
                </div>
              )}
            </div>
          ))}
          {monitors.length > displayCount && (
            <button
              onClick={() => setDisplayCount((c) => c + 25)}
              className="w-full py-2 text-sm text-dark-300 hover:text-dark-100 border border-dark-600 rounded-lg hover:border-dark-400 transition-colors"
            >
              Show more ({monitors.length - displayCount} remaining)
            </button>
          )}
        </div>
      ) : null}
      </div>
    </div>
  );
}
