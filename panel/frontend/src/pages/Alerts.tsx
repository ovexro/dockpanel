import { useState, useEffect, useRef } from "react";
import { api } from "../api";
import { logger } from "../utils/logger";

interface Alert {
  id: string;
  server_id: string | null;
  site_id: string | null;
  alert_type: string;
  severity: string;
  title: string;
  message: string;
  status: string;
  notified_at: string;
  resolved_at: string | null;
  acknowledged_at: string | null;
  created_at: string;
}

interface AlertSummary {
  firing: number;
  acknowledged: number;
  resolved: number;
}

const TYPE_LABELS: Record<string, string> = {
  cpu: "CPU",
  memory: "Memory",
  disk: "Disk",
  offline: "Offline",
  backup_failure: "Backup",
  ssl_expiry: "SSL",
  service_down: "Service",
  flapping: "Flapping",
};

const SEVERITY_STYLES: Record<string, { bg: string; text: string; dot: string }> = {
  critical: { bg: "bg-danger-500/10", text: "text-danger-400", dot: "bg-danger-500" },
  warning: { bg: "bg-warn-500/10", text: "text-warn-400", dot: "bg-warn-500" },
  info: { bg: "bg-accent-500/10", text: "text-accent-400", dot: "bg-accent-500" },
};

export default function Alerts() {
  const [alerts, setAlerts] = useState<Alert[]>([]);
  const [summary, setSummary] = useState<AlertSummary>({ firing: 0, acknowledged: 0, resolved: 0 });
  const [loading, setLoading] = useState(true);
  const [statusFilter, setStatusFilter] = useState("firing");
  const [typeFilter, setTypeFilter] = useState("");
  const [message, setMessage] = useState<{text: string; type: string} | null>(null);
  const refreshTimer = useRef<ReturnType<typeof setInterval>>(undefined);

  const fetchAlerts = async () => {
    try {
      let path = `/alerts?limit=100`;
      if (statusFilter) path += `&status=${statusFilter}`;
      if (typeFilter) path += `&alert_type=${typeFilter}`;

      const [data, sum] = await Promise.all([
        api.get<Alert[]>(path),
        api.get<AlertSummary>("/alerts/summary"),
      ]);
      setAlerts(data);
      setSummary(sum);
    } catch (e) {
      logger.error("Failed to load alerts:", e);
    } finally {
      setLoading(false);
    }
  };

  useEffect(() => {
    fetchAlerts();
    refreshTimer.current = setInterval(fetchAlerts, 30000);
    return () => {
      if (refreshTimer.current) clearInterval(refreshTimer.current);
    };
  }, [statusFilter, typeFilter]);

  const handleAcknowledge = async (id: string) => {
    try {
      await api.put(`/alerts/${id}/acknowledge`, {});
      setMessage({ text: "Alert acknowledged", type: "success" });
      setTimeout(() => setMessage(null), 3000);
      fetchAlerts();
    } catch (e) {
      logger.error("Failed to acknowledge alert:", e);
      setMessage({ text: "Failed to acknowledge alert", type: "error" });
      setTimeout(() => setMessage(null), 3000);
    }
  };

  const handleResolve = async (id: string) => {
    try {
      await api.put(`/alerts/${id}/resolve`, {});
      setMessage({ text: "Alert resolved", type: "success" });
      setTimeout(() => setMessage(null), 3000);
      fetchAlerts();
    } catch (e) {
      logger.error("Failed to resolve alert:", e);
      setMessage({ text: "Failed to resolve alert", type: "error" });
      setTimeout(() => setMessage(null), 3000);
    }
  };

  const ago = (dateStr: string) => {
    const diff = Date.now() - new Date(dateStr).getTime();
    const mins = Math.floor(diff / 60000);
    if (mins < 1) return "just now";
    if (mins < 60) return `${mins}m ago`;
    const hrs = Math.floor(mins / 60);
    if (hrs < 24) return `${hrs}h ago`;
    return `${Math.floor(hrs / 24)}d ago`;
  };

  return (
    <div>
      <div className="page-header">
        <div>
          <h1 className="page-header-title">Alerts</h1>
          <p className="page-header-subtitle">Monitor and manage system alerts</p>
        </div>
        <div className="flex items-center gap-2">
          <button
            onClick={fetchAlerts}
            className="px-3 py-1.5 bg-dark-700 text-dark-200 hover:bg-dark-600 hover:text-dark-100 border border-dark-600 rounded-lg text-sm transition-colors"
          >
            Refresh
          </button>
        </div>
      </div>

      <div className="p-6 lg:p-8">

      {message && (
        <div className={`mb-4 px-4 py-3 rounded-lg text-sm border ${
          message.type === "success"
            ? "bg-rust-500/10 text-rust-400 border-rust-500/20"
            : "bg-danger-500/10 text-danger-400 border-danger-500/20"
        }`}>
          {message.text}
        </div>
      )}

      {/* Summary cards */}
      <div className="grid grid-cols-1 sm:grid-cols-3 gap-4 mb-6">
        <div
          className={`p-4 rounded-lg border cursor-pointer transition-colors ${
            statusFilter === "firing"
              ? "bg-danger-500/10 border-danger-500/30"
              : "bg-dark-800 border-dark-500 hover:border-dark-400"
          }`}
          onClick={() => setStatusFilter(statusFilter === "firing" ? "" : "firing")}
        >
          <div className="text-2xl font-bold text-danger-400">{summary.firing}</div>
          <div className="text-sm text-dark-200">Firing</div>
        </div>
        <div
          className={`p-4 rounded-lg border cursor-pointer transition-colors ${
            statusFilter === "acknowledged"
              ? "bg-warn-500/10 border-warn-500/30"
              : "bg-dark-800 border-dark-500 hover:border-dark-400"
          }`}
          onClick={() => setStatusFilter(statusFilter === "acknowledged" ? "" : "acknowledged")}
        >
          <div className="text-2xl font-bold text-warn-400">{summary.acknowledged}</div>
          <div className="text-sm text-dark-200">Acknowledged</div>
        </div>
        <div
          className={`p-4 rounded-lg border cursor-pointer transition-colors ${
            statusFilter === "resolved"
              ? "bg-rust-500/10 border-rust-500/30"
              : "bg-dark-800 border-dark-500 hover:border-dark-400"
          }`}
          onClick={() => setStatusFilter(statusFilter === "resolved" ? "" : "resolved")}
        >
          <div className="text-2xl font-bold text-rust-400">{summary.resolved}</div>
          <div className="text-sm text-dark-200">Resolved (30d)</div>
        </div>
      </div>

      {/* Type filter */}
      <div className="flex gap-2 mb-4 flex-wrap">
        <button
          onClick={() => setTypeFilter("")}
          className={`px-3 py-1 rounded-lg text-xs font-medium ${
            !typeFilter ? "bg-rust-500 text-white" : "bg-dark-700 text-dark-200"
          }`}
        >
          All
        </button>
        {Object.entries(TYPE_LABELS).map(([key, label]) => (
          <button
            key={key}
            onClick={() => setTypeFilter(typeFilter === key ? "" : key)}
            className={`px-3 py-1 rounded-lg text-xs font-medium ${
              typeFilter === key ? "bg-rust-500 text-white" : "bg-dark-700 text-dark-200"
            }`}
          >
            {label}
          </button>
        ))}
      </div>

      {/* Alert list */}
      {loading ? (
        <div className="space-y-3">
          {[1, 2, 3].map((i) => (
            <div key={i} className="h-20 bg-dark-800 rounded-lg animate-pulse" />
          ))}
        </div>
      ) : alerts.length === 0 ? (
        <div className="text-center py-16">
          <svg className="w-12 h-12 mx-auto text-dark-300 mb-3" fill="none" viewBox="0 0 24 24" stroke="currentColor" strokeWidth={1.5}>
            <path strokeLinecap="round" strokeLinejoin="round" d="M14.857 17.082a23.848 23.848 0 0 0 5.454-1.31A8.967 8.967 0 0 1 18 9.75V9A6 6 0 0 0 6 9v.75a8.967 8.967 0 0 1-2.312 6.022c1.733.64 3.56 1.085 5.455 1.31m5.714 0a24.255 24.255 0 0 1-5.714 0m5.714 0a3 3 0 1 1-5.714 0" />
          </svg>
          <p className="text-dark-200 text-sm">
            {statusFilter === "firing"
              ? "No active alerts -- all systems operational"
              : "No alerts match the current filters"}
          </p>
        </div>
      ) : (
        <div className="space-y-2">
          {alerts.map((alert) => {
            const sev = SEVERITY_STYLES[alert.severity] || SEVERITY_STYLES.info;
            return (
              <div
                key={alert.id}
                className={`p-4 rounded-lg border border-dark-500 ${
                  alert.status === "resolved" ? "bg-dark-800/50 opacity-70" : "bg-dark-800"
                }`}
              >
                <div className="flex items-start justify-between gap-3">
                  <div className="flex items-start gap-3 min-w-0">
                    <div className={`mt-1 w-2.5 h-2.5 rounded-full shrink-0 ${sev.dot} ${
                      alert.status === "firing" ? "animate-pulse" : ""
                    }`} />
                    <div className="min-w-0">
                      <div className="flex items-center gap-2 mb-1 flex-wrap">
                        <span className="font-medium text-dark-50 text-sm">{alert.title}</span>
                        <span className={`px-1.5 py-0.5 rounded text-xs font-medium ${sev.bg} ${sev.text}`}>
                          {alert.severity}
                        </span>
                        <span className="px-1.5 py-0.5 rounded text-xs bg-dark-700 text-dark-300 font-mono">
                          {TYPE_LABELS[alert.alert_type] || alert.alert_type}
                        </span>
                      </div>
                      <p className="text-sm text-dark-200 mb-1">{alert.message}</p>
                      <p className="text-xs text-dark-300 font-mono">
                        {ago(alert.created_at)}
                        {alert.resolved_at && ` -- resolved ${ago(alert.resolved_at)}`}
                        {alert.acknowledged_at && !alert.resolved_at && ` -- acknowledged ${ago(alert.acknowledged_at)}`}
                      </p>
                    </div>
                  </div>
                  {alert.status === "firing" && (
                    <div className="flex gap-1.5 shrink-0">
                      <button
                        onClick={() => handleAcknowledge(alert.id)}
                        className="px-2.5 py-1 bg-warn-500/15 text-warn-400 rounded-lg text-xs hover:bg-warn-500/25"
                      >
                        Ack
                      </button>
                      <button
                        onClick={() => handleResolve(alert.id)}
                        className="px-2.5 py-1 bg-rust-500/15 text-rust-400 rounded-lg text-xs hover:bg-rust-500/25"
                      >
                        Resolve
                      </button>
                    </div>
                  )}
                  {alert.status === "acknowledged" && (
                    <button
                      onClick={() => handleResolve(alert.id)}
                      className="px-2.5 py-1 bg-rust-500/15 text-rust-400 rounded-lg text-xs hover:bg-rust-500/25 shrink-0"
                    >
                      Resolve
                    </button>
                  )}
                </div>
              </div>
            );
          })}
        </div>
      )}
      </div>
    </div>
  );
}
