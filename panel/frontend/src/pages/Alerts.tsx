import { useState, useEffect, useRef } from "react";
import { api } from "../api";
import { useAuth } from "../context/AuthContext";
import { logger } from "../utils/logger";
import { renderMarkdown } from "../utils/markdown";

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
  // Phase 4 W3: ack actor + optional comment, plus the escalation chain
  // position. Older alerts (pre-v2.9.0) have null/0 here.
  acknowledged_by: string | null;
  acknowledged_by_email: string | null;
  acknowledged_comment: string | null;
  escalation_step_index: number;
  created_at: string;
}

interface MemberInfo {
  id: string;
  email: string;
}

interface OnCallSchedule {
  id: string;
  name: string;
  cadence_days: number;
  anchor_at: string;
  members: MemberInfo[];
  current_on_call: MemberInfo | null;
  created_at: string;
  updated_at: string;
}

interface EscalationStep {
  after_minutes: number;
  route: string;
}

interface EscalationPolicy {
  id: string;
  name: string;
  steps: EscalationStep[];
  used_by_rule_count: number;
  created_at: string;
  updated_at: string;
}

interface AlertSummary {
  firing: number;
  acknowledged: number;
  resolved: number;
}

interface Runbook {
  alert_type: string;
  runbook_md: string;
  severity_default: string;
  is_default: boolean;
  updated_at: string | null;
  updated_by: string | null;
}

const TYPE_LABELS: Record<string, string> = {
  cpu: "CPU",
  memory: "Memory",
  disk: "Disk",
  disk_forecast: "Disk forecast",
  offline: "Offline",
  backup_failure: "Backup",
  ssl_expiry: "SSL",
  service_down: "Service",
  container_down: "Container down",
  container_crashloop: "Container crashloop",
  container_unhealthy: "Container unhealthy",
  gpu_utilization: "GPU utilization",
  gpu_temperature: "GPU temperature",
  gpu_vram: "GPU VRAM",
  memory_leak: "Memory leak",
  flapping: "Flapping",
  // Fired outside alert_engine, and absent here until v2.155.0 — the badge
  // printed the raw column value and the filter row, built from these same
  // keys, offered no way to select them. Certificate renewal failures are the
  // sharpest of the four: eight producers, half of them critical.
  ssl_renewal_failure: "SSL renewal",
  cron_failure: "Cron",
  backup_verification_failed: "Backup verification",
  security: "Security scan",
};

const SEVERITY_STYLES: Record<string, { bg: string; text: string; dot: string }> = {
  critical: { bg: "bg-danger-500/10", text: "text-danger-400", dot: "bg-danger-500" },
  warning: { bg: "bg-warn-500/10", text: "text-warn-400", dot: "bg-warn-500" },
  info: { bg: "bg-accent-500/10", text: "text-accent-400", dot: "bg-accent-500" },
};

type TabId = "alerts" | "runbooks" | "on-call" | "policies";

export default function Alerts() {
  // Alerts themselves are user-scoped and every action on them is open to the
  // owner, which is why this whole surface sits on a nav row with no role flag.
  // The other three tabs are not: their endpoints are administrator-only to the
  // last handler, so showing their triggers to a client offered three lists that
  // could only ever come back refused.
  const { user } = useAuth();
  const isAdmin = user?.role === "admin";
  const [tab, setTab] = useState<TabId>("alerts");
  const [alerts, setAlerts] = useState<Alert[]>([]);
  const [summary, setSummary] = useState<AlertSummary>({ firing: 0, acknowledged: 0, resolved: 0 });
  const [loading, setLoading] = useState(true);
  const [statusFilter, setStatusFilter] = useState("firing");
  const [typeFilter, setTypeFilter] = useState("");
  const [message, setMessage] = useState<{text: string; type: string} | null>(null);
  const [expanded, setExpanded] = useState<string | null>(null);
  const [runbookCache, setRunbookCache] = useState<Record<string, Runbook | null>>({});
  // Phase 4 W3: which alert ID has the ack popover open + the current comment draft.
  const [ackPopover, setAckPopover] = useState<string | null>(null);
  const [ackComment, setAckComment] = useState("");
  const refreshTimer = useRef<ReturnType<typeof setInterval>>(undefined);

  const toggleExpand = async (alert: Alert) => {
    if (expanded === alert.id) {
      setExpanded(null);
      return;
    }
    setExpanded(alert.id);
    // Runbooks are administrator-only to read. Asking anyway would spend a
    // guaranteed 403 and cache `null`, which this component renders exactly like
    // "no runbook has been written for this alert type" — a refusal a client
    // could never tell from an ordinary absence, and so could never report.
    if (isAdmin && runbookCache[alert.alert_type] === undefined) {
      try {
        const rb = await api.get<Runbook>(`/alerts/runbooks/${alert.alert_type}`);
        setRunbookCache((prev) => ({ ...prev, [alert.alert_type]: rb }));
      } catch {
        setRunbookCache((prev) => ({ ...prev, [alert.alert_type]: null }));
      }
    }
  };

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
    if (tab !== "alerts") return;
    fetchAlerts();
    refreshTimer.current = setInterval(fetchAlerts, 30000);
    return () => {
      if (refreshTimer.current) clearInterval(refreshTimer.current);
    };
  }, [statusFilter, typeFilter, tab]);

  const handleAcknowledge = async (id: string, comment?: string) => {
    try {
      const body = comment && comment.trim() ? { comment: comment.trim() } : {};
      await api.put(`/alerts/${id}/acknowledge`, body);
      setMessage({ text: "Alert acknowledged", type: "success" });
      setTimeout(() => setMessage(null), 3000);
      setAckPopover(null);
      setAckComment("");
      fetchAlerts();
    } catch (e) {
      logger.error("Failed to acknowledge alert:", e);
      // Surface 400s from the 500-char cap so operators get a meaningful nudge.
      const detail = (e as { response?: { data?: { error?: string } } })?.response?.data?.error;
      setMessage({ text: detail || "Failed to acknowledge alert", type: "error" });
      setTimeout(() => setMessage(null), 3000);
    }
  };

  const openAckPopover = (id: string) => {
    setAckPopover(id);
    setAckComment("");
  };

  const cancelAckPopover = () => {
    setAckPopover(null);
    setAckComment("");
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
          {tab === "alerts" && (
            <button
              onClick={fetchAlerts}
              className="px-3 py-1.5 bg-dark-700 text-dark-200 hover:bg-dark-600 hover:text-dark-100 border border-dark-600 rounded-lg text-sm transition-colors"
            >
              Refresh
            </button>
          )}
        </div>
      </div>

      <div className="p-6 lg:p-8">

      <div className="flex gap-1 mb-6 border-b border-dark-500">
        <button
          onClick={() => setTab("alerts")}
          className={`px-4 py-2 text-sm font-medium border-b-2 -mb-px transition-colors ${
            tab === "alerts"
              ? "border-rust-500 text-rust-400"
              : "border-transparent text-dark-200 hover:text-dark-100"
          }`}
        >
          Alerts
        </button>
        {isAdmin && (
          <button
            onClick={() => setTab("runbooks")}
            className={`px-4 py-2 text-sm font-medium border-b-2 -mb-px transition-colors ${
              tab === "runbooks"
                ? "border-rust-500 text-rust-400"
                : "border-transparent text-dark-200 hover:text-dark-100"
            }`}
          >
            Runbooks
          </button>
        )}
        {isAdmin && (
          <button
            onClick={() => setTab("on-call")}
            className={`px-4 py-2 text-sm font-medium border-b-2 -mb-px transition-colors ${
              tab === "on-call"
                ? "border-rust-500 text-rust-400"
                : "border-transparent text-dark-200 hover:text-dark-100"
            }`}
          >
            On-call
          </button>
        )}
        {isAdmin && (
          <button
            onClick={() => setTab("policies")}
            className={`px-4 py-2 text-sm font-medium border-b-2 -mb-px transition-colors ${
              tab === "policies"
                ? "border-rust-500 text-rust-400"
                : "border-transparent text-dark-200 hover:text-dark-100"
            }`}
          >
            Escalation policies
          </button>
        )}
      </div>

      {message && (
        <div className={`mb-4 px-4 py-3 rounded-lg text-sm border ${
          message.type === "success"
            ? "bg-rust-500/10 text-rust-400 border-rust-500/20"
            : "bg-danger-500/10 text-danger-400 border-danger-500/20"
        }`}>
          {message.text}
        </div>
      )}

      {tab === "alerts" && (
        <>
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
                const isOpen = expanded === alert.id;
                const runbook = runbookCache[alert.alert_type];
                return (
                  <div
                    key={alert.id}
                    className={`rounded-lg border border-dark-500 overflow-hidden ${
                      alert.status === "resolved" ? "bg-dark-800/50 opacity-70" : "bg-dark-800"
                    }`}
                  >
                    <div
                      className="p-4 cursor-pointer hover:bg-dark-700/40"
                      onClick={() => toggleExpand(alert)}
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
                              <span className="text-xs text-dark-300 ml-1">
                                {isOpen ? "▾" : "▸"} runbook
                              </span>
                            </div>
                            <p className="text-sm text-dark-200 mb-1">{alert.message}</p>
                            <p className="text-xs text-dark-300 font-mono">
                              {ago(alert.created_at)}
                              {alert.resolved_at && ` -- resolved ${ago(alert.resolved_at)}`}
                              {alert.acknowledged_at && !alert.resolved_at && ` -- acknowledged ${ago(alert.acknowledged_at)}`}
                              {alert.escalation_step_index > 0 && (
                                <span className="ml-2 px-1.5 py-0.5 rounded bg-warn-500/15 text-warn-400">
                                  escalated · step {alert.escalation_step_index}
                                </span>
                              )}
                            </p>
                            {alert.acknowledged_by_email && (
                              <p className="text-xs text-dark-200 mt-1">
                                <span className="text-dark-300">Acknowledged by</span>{" "}
                                <span className="text-dark-100">{alert.acknowledged_by_email}</span>
                                {alert.acknowledged_comment && (
                                  <span
                                    className="ml-2 italic text-dark-200"
                                    title={alert.acknowledged_comment}
                                  >
                                    "
                                    {alert.acknowledged_comment.length > 80
                                      ? alert.acknowledged_comment.slice(0, 80) + "…"
                                      : alert.acknowledged_comment}
                                    "
                                  </span>
                                )}
                              </p>
                            )}
                          </div>
                        </div>
                        <div className="flex gap-1.5 shrink-0" onClick={(e) => e.stopPropagation()}>
                          {alert.status === "firing" && ackPopover !== alert.id && (
                            <>
                              <button
                                onClick={() => openAckPopover(alert.id)}
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
                            </>
                          )}
                          {alert.status === "acknowledged" && (
                            <button
                              onClick={() => handleResolve(alert.id)}
                              className="px-2.5 py-1 bg-rust-500/15 text-rust-400 rounded-lg text-xs hover:bg-rust-500/25"
                            >
                              Resolve
                            </button>
                          )}
                        </div>
                      </div>
                      {ackPopover === alert.id && (
                        <div
                          className="mt-3 p-3 bg-dark-900/70 border border-warn-500/30 rounded-lg"
                          onClick={(e) => e.stopPropagation()}
                        >
                          <label className="text-xs text-dark-200 block mb-1">
                            Acknowledge — optional comment (max 500 chars)
                          </label>
                          <textarea
                            value={ackComment}
                            onChange={(e) => setAckComment(e.target.value)}
                            maxLength={500}
                            rows={2}
                            placeholder="e.g. 'looking into it — disk forecast model is hot, no action needed'"
                            className="w-full bg-dark-800 text-dark-100 text-sm border border-dark-500 rounded p-2 outline-none focus:border-warn-500/50"
                            autoFocus
                          />
                          <div className="flex items-center justify-between mt-2">
                            <span className="text-xs text-dark-300">
                              {ackComment.length}/500
                            </span>
                            <div className="flex gap-2">
                              <button
                                onClick={cancelAckPopover}
                                className="px-2.5 py-1 bg-dark-700 text-dark-200 rounded-lg text-xs hover:bg-dark-600"
                              >
                                Cancel
                              </button>
                              <button
                                onClick={() => handleAcknowledge(alert.id, ackComment)}
                                className="px-2.5 py-1 bg-warn-500 text-white rounded-lg text-xs font-medium hover:bg-warn-600"
                              >
                                Acknowledge
                              </button>
                            </div>
                          </div>
                        </div>
                      )}
                    </div>
                    {isOpen && (
                      <div className="border-t border-dark-500 bg-dark-900/40 px-4 py-3">
                        {runbook === undefined ? (
                          <div className="text-sm text-dark-300">Loading runbook...</div>
                        ) : runbook === null ? (
                          <div className="text-sm text-dark-300">No runbook for this alert type yet. Add one in the Runbooks tab.</div>
                        ) : (
                          <div
                            className="markdown-body text-sm text-dark-100"
                            dangerouslySetInnerHTML={{ __html: renderMarkdown(runbook.runbook_md) }}
                          />
                        )}
                      </div>
                    )}
                  </div>
                );
              })}
            </div>
          )}
        </>
      )}

      {/* The `isAdmin` term is repeated on each body deliberately. Hiding a trigger
          decides what is OFFERED; the mount decides what is REQUESTED, and only the
          second one stops the refused calls. Whoever adds the next way to set `tab`
          gets the guard for free. */}
      {tab === "runbooks" && isAdmin && <RunbooksTab onMessage={(m) => { setMessage(m); setTimeout(() => setMessage(null), 3000); }} />}
      {tab === "on-call" && isAdmin && <OnCallTab onMessage={(m) => { setMessage(m); setTimeout(() => setMessage(null), 3000); }} />}
      {tab === "policies" && isAdmin && <PoliciesTab onMessage={(m) => { setMessage(m); setTimeout(() => setMessage(null), 3000); }} />}

      </div>
    </div>
  );
}

function RunbooksTab({ onMessage }: { onMessage: (m: { text: string; type: string }) => void }) {
  const [runbooks, setRunbooks] = useState<Runbook[]>([]);
  const [loading, setLoading] = useState(true);
  const [editing, setEditing] = useState<Runbook | null>(null);
  const [draft, setDraft] = useState("");
  const [draftSeverity, setDraftSeverity] = useState("warning");
  const [seeding, setSeeding] = useState(false);
  const [confirmSeed, setConfirmSeed] = useState(false);

  const fetchRunbooks = async () => {
    try {
      const data = await api.get<Runbook[]>("/alerts/runbooks");
      setRunbooks(data);
    } catch (e) {
      logger.error("Failed to load runbooks:", e);
      onMessage({ text: "Failed to load runbooks", type: "error" });
    } finally {
      setLoading(false);
    }
  };

  useEffect(() => {
    fetchRunbooks();
  }, []);

  const openEdit = (rb: Runbook) => {
    setEditing(rb);
    setDraft(rb.runbook_md);
    setDraftSeverity(rb.severity_default);
  };

  const closeEdit = () => {
    setEditing(null);
    setDraft("");
  };

  const saveEdit = async () => {
    if (!editing) return;
    try {
      await api.put(`/alerts/runbooks/${editing.alert_type}`, {
        runbook_md: draft,
        severity_default: draftSeverity,
      });
      onMessage({ text: `Saved runbook for ${editing.alert_type}`, type: "success" });
      closeEdit();
      fetchRunbooks();
    } catch (e) {
      logger.error("Failed to save runbook:", e);
      onMessage({ text: "Failed to save runbook", type: "error" });
    }
  };

  const restoreDefault = async () => {
    if (!editing) return;
    if (!confirm(`Restore default runbook for ${editing.alert_type}? This discards your edits.`)) return;
    try {
      await api.delete(`/alerts/runbooks/${editing.alert_type}`);
      onMessage({ text: `Restored default for ${editing.alert_type}`, type: "success" });
      closeEdit();
      fetchRunbooks();
    } catch (e) {
      logger.error("Failed to restore default:", e);
      onMessage({ text: "Failed to restore default", type: "error" });
    }
  };

  const seedDefaults = async () => {
    setSeeding(true);
    try {
      const res = await api.post<{ inserted: string[]; skipped: string[] }>("/alerts/runbooks/apply-defaults", {});
      const msg =
        res.inserted.length === 0
          ? `All ${res.skipped.length} runbooks already exist — nothing seeded`
          : `Seeded ${res.inserted.length} runbook${res.inserted.length === 1 ? "" : "s"}` +
            (res.skipped.length ? ` (${res.skipped.length} already customized — skipped)` : "");
      onMessage({ text: msg, type: "success" });
      fetchRunbooks();
    } catch (e) {
      logger.error("Failed to seed runbooks:", e);
      onMessage({ text: "Failed to seed runbooks", type: "error" });
    } finally {
      setSeeding(false);
      setConfirmSeed(false);
    }
  };

  const sortedRunbooks = [...runbooks].sort((a, b) => {
    const sevOrder = { critical: 0, warning: 1, info: 2 } as Record<string, number>;
    const sa = sevOrder[a.severity_default] ?? 3;
    const sb = sevOrder[b.severity_default] ?? 3;
    if (sa !== sb) return sa - sb;
    return a.alert_type.localeCompare(b.alert_type);
  });

  return (
    <div>
      <div className="bg-dark-800 border border-dark-500 rounded-lg p-4 mb-6">
        <div className="flex items-start justify-between gap-4 flex-wrap">
          <div className="min-w-0 max-w-2xl">
            <h2 className="font-medium text-dark-50 mb-1">Runbooks</h2>
            <p className="text-sm text-dark-200">
              Markdown attached to each alert type. Excerpts ride along in slack/discord/pagerduty/webhook payloads, full text appears in incident detail and email. Edit any runbook to customize for your environment — your edits survive panel upgrades.
            </p>
          </div>
          {confirmSeed ? (
            <div className="flex gap-2 items-center bg-dark-700 border border-dark-500 rounded-lg px-3 py-2">
              <span className="text-xs text-dark-200">Seed missing only — won't overwrite edits.</span>
              <button
                onClick={seedDefaults}
                disabled={seeding}
                className="px-2.5 py-1 bg-rust-500 text-white rounded-lg text-xs font-medium hover:bg-rust-600 disabled:opacity-50"
              >
                {seeding ? "Seeding..." : "Confirm"}
              </button>
              <button
                onClick={() => setConfirmSeed(false)}
                className="px-2.5 py-1 bg-dark-700 text-dark-200 rounded-lg text-xs hover:bg-dark-600"
              >
                Cancel
              </button>
            </div>
          ) : (
            <button
              onClick={() => setConfirmSeed(true)}
              className="px-3 py-1.5 bg-rust-500 text-white rounded-lg text-sm font-medium hover:bg-rust-600 shrink-0"
            >
              Seed missing default runbooks
            </button>
          )}
        </div>
      </div>

      {loading ? (
        <div className="space-y-3">
          {[1, 2, 3, 4, 5].map((i) => (
            <div key={i} className="h-16 bg-dark-800 rounded-lg animate-pulse" />
          ))}
        </div>
      ) : (
        <div className="space-y-2">
          {sortedRunbooks.map((rb) => {
            const sev = SEVERITY_STYLES[rb.severity_default] || SEVERITY_STYLES.info;
            return (
              <div key={rb.alert_type} className="p-3 bg-dark-800 border border-dark-500 rounded-lg flex items-center justify-between gap-3">
                <div className="flex items-center gap-3 min-w-0">
                  <div className={`w-2 h-2 rounded-full ${sev.dot}`} />
                  <div className="min-w-0">
                    <div className="flex items-center gap-2 flex-wrap">
                      <span className="font-medium text-dark-50 text-sm">
                        {TYPE_LABELS[rb.alert_type] || rb.alert_type}
                      </span>
                      <span className="px-1.5 py-0.5 rounded text-xs font-mono bg-dark-700 text-dark-300">
                        {rb.alert_type}
                      </span>
                      <span className={`px-1.5 py-0.5 rounded text-xs font-medium ${sev.bg} ${sev.text}`}>
                        {rb.severity_default}
                      </span>
                      {rb.is_default ? (
                        <span className="px-1.5 py-0.5 rounded text-xs bg-dark-700 text-dark-300">default</span>
                      ) : (
                        <span className="px-1.5 py-0.5 rounded text-xs bg-accent-500/10 text-accent-400">customized</span>
                      )}
                    </div>
                  </div>
                </div>
                <button
                  onClick={() => openEdit(rb)}
                  className="px-2.5 py-1 bg-dark-700 text-dark-100 rounded-lg text-xs hover:bg-dark-600 shrink-0"
                >
                  Edit
                </button>
              </div>
            );
          })}
        </div>
      )}

      {editing && (
        <div className="fixed inset-0 bg-black/60 flex items-center justify-center p-4 z-50" onClick={closeEdit}>
          <div className="bg-dark-900 border border-dark-500 rounded-lg max-w-5xl w-full max-h-[90vh] flex flex-col" onClick={(e) => e.stopPropagation()}>
            <div className="px-6 py-4 border-b border-dark-500 flex items-center justify-between">
              <div>
                <h3 className="font-medium text-dark-50">
                  Edit runbook: <span className="font-mono text-sm text-dark-200">{editing.alert_type}</span>
                </h3>
                <p className="text-xs text-dark-300 mt-0.5">
                  {editing.is_default ? "Currently using default — your edits will create a custom version." : "Currently customized."}
                </p>
              </div>
              <button onClick={closeEdit} className="text-dark-300 hover:text-dark-100 text-xl leading-none">×</button>
            </div>
            <div className="flex-1 overflow-hidden grid grid-cols-1 lg:grid-cols-2 gap-0">
              <div className="border-r border-dark-500 flex flex-col">
                <div className="px-4 py-2 bg-dark-800 border-b border-dark-500 flex items-center gap-3">
                  <span className="text-xs font-medium text-dark-200">Markdown</span>
                  <select
                    value={draftSeverity}
                    onChange={(e) => setDraftSeverity(e.target.value)}
                    className="ml-auto bg-dark-700 border border-dark-500 rounded text-xs text-dark-100 px-2 py-1"
                  >
                    <option value="info">info</option>
                    <option value="warning">warning</option>
                    <option value="critical">critical</option>
                  </select>
                </div>
                <textarea
                  value={draft}
                  onChange={(e) => setDraft(e.target.value)}
                  className="flex-1 bg-dark-900 text-dark-100 font-mono text-sm p-4 outline-none resize-none border-0 min-h-[400px]"
                  spellCheck={false}
                />
              </div>
              <div className="flex flex-col bg-dark-800">
                <div className="px-4 py-2 bg-dark-800 border-b border-dark-500">
                  <span className="text-xs font-medium text-dark-200">Preview</span>
                </div>
                <div
                  className="markdown-body flex-1 overflow-y-auto p-4 text-sm text-dark-100 min-h-[400px]"
                  dangerouslySetInnerHTML={{ __html: renderMarkdown(draft) }}
                />
              </div>
            </div>
            <div className="px-6 py-4 border-t border-dark-500 flex items-center justify-between gap-2">
              <div>
                {!editing.is_default && (
                  <button
                    onClick={restoreDefault}
                    className="px-3 py-1.5 bg-dark-700 text-dark-200 hover:bg-dark-600 hover:text-dark-100 border border-dark-500 rounded-lg text-sm"
                  >
                    Restore default
                  </button>
                )}
              </div>
              <div className="flex gap-2">
                <button
                  onClick={closeEdit}
                  className="px-3 py-1.5 bg-dark-700 text-dark-200 hover:bg-dark-600 hover:text-dark-100 border border-dark-500 rounded-lg text-sm"
                >
                  Cancel
                </button>
                <button
                  onClick={saveEdit}
                  className="px-3 py-1.5 bg-rust-500 text-white rounded-lg text-sm font-medium hover:bg-rust-600"
                >
                  Save
                </button>
              </div>
            </div>
          </div>
        </div>
      )}
    </div>
  );
}

// ---------------------------------------------------------------------------
// Phase 4 W3: On-call rotation editor
// ---------------------------------------------------------------------------

function OnCallTab({ onMessage }: { onMessage: (m: { text: string; type: string }) => void }) {
  const [schedules, setSchedules] = useState<OnCallSchedule[]>([]);
  const [users, setUsers] = useState<MemberInfo[]>([]);
  const [loading, setLoading] = useState(true);
  const [editing, setEditing] = useState<OnCallSchedule | null>(null);
  const [draftName, setDraftName] = useState("");
  const [draftMembers, setDraftMembers] = useState<string[]>([]);
  const [draftCadence, setDraftCadence] = useState(7);
  const [confirmDelete, setConfirmDelete] = useState<string | null>(null);

  const fetchSchedules = async () => {
    try {
      const data = await api.get<OnCallSchedule[]>("/on-call/schedules");
      setSchedules(data);
    } catch (e) {
      logger.error("Failed to load schedules:", e);
      onMessage({ text: "Failed to load on-call schedules", type: "error" });
    } finally {
      setLoading(false);
    }
  };

  const fetchUsers = async () => {
    try {
      // Reuse the standard admin users endpoint; it returns full user rows so
      // we project down to {id, email} pairs for the member picker.
      const data = await api.get<{ id: string; email: string }[]>("/users");
      setUsers(data.map((u) => ({ id: u.id, email: u.email })));
    } catch (e) {
      logger.error("Failed to load users:", e);
    }
  };

  useEffect(() => {
    fetchSchedules();
    fetchUsers();
  }, []);

  const openNew = () => {
    setEditing({
      id: "",
      name: "",
      cadence_days: 7,
      anchor_at: new Date().toISOString(),
      members: [],
      current_on_call: null,
      created_at: "",
      updated_at: "",
    });
    setDraftName("");
    setDraftMembers([]);
    setDraftCadence(7);
  };

  const openEdit = (s: OnCallSchedule) => {
    setEditing(s);
    setDraftName(s.name);
    setDraftMembers(s.members.map((m) => m.id));
    setDraftCadence(s.cadence_days);
  };

  const closeEdit = () => {
    setEditing(null);
  };

  const save = async () => {
    if (!editing) return;
    if (!draftName.trim()) {
      onMessage({ text: "Name is required", type: "error" });
      return;
    }
    if (draftMembers.length === 0) {
      onMessage({ text: "At least one member is required", type: "error" });
      return;
    }
    const body = {
      name: draftName.trim(),
      members: draftMembers,
      cadence_days: draftCadence,
    };
    try {
      if (editing.id) {
        await api.put(`/on-call/schedules/${editing.id}`, body);
      } else {
        await api.post(`/on-call/schedules`, body);
      }
      onMessage({ text: "Schedule saved", type: "success" });
      closeEdit();
      fetchSchedules();
    } catch (e) {
      logger.error("Failed to save schedule:", e);
      const detail = (e as { response?: { data?: { error?: string } } })?.response?.data?.error;
      onMessage({ text: detail || "Failed to save schedule", type: "error" });
    }
  };

  const remove = async (id: string) => {
    try {
      await api.delete(`/on-call/schedules/${id}`);
      onMessage({ text: "Schedule deleted", type: "success" });
      setConfirmDelete(null);
      fetchSchedules();
    } catch (e) {
      logger.error("Failed to delete schedule:", e);
      onMessage({ text: "Failed to delete schedule", type: "error" });
    }
  };

  const moveMember = (idx: number, direction: -1 | 1) => {
    const next = [...draftMembers];
    const target = idx + direction;
    if (target < 0 || target >= next.length) return;
    [next[idx], next[target]] = [next[target], next[idx]];
    setDraftMembers(next);
  };

  return (
    <div>
      <div className="bg-dark-800 border border-dark-500 rounded-lg p-4 mb-6">
        <div className="flex items-start justify-between gap-4 flex-wrap">
          <div className="min-w-0 max-w-2xl">
            <h2 className="font-medium text-dark-50 mb-1">On-call rotations</h2>
            <p className="text-sm text-dark-200">
              Ordered list of users plus a cadence (in days). Whoever the
              rotation lands on right now is the one paged when an escalation
              policy routes to <code className="text-dark-100">on_call_schedule:&lt;id&gt;</code>.
              No calendar, no overrides — keep the list small, advance by days.
            </p>
          </div>
          <button
            onClick={openNew}
            className="px-3 py-1.5 bg-rust-500 text-white rounded-lg text-sm font-medium hover:bg-rust-600 shrink-0"
          >
            New rotation
          </button>
        </div>
      </div>

      {loading ? (
        <div className="space-y-3">
          {[1, 2].map((i) => (
            <div key={i} className="h-20 bg-dark-800 rounded-lg animate-pulse" />
          ))}
        </div>
      ) : schedules.length === 0 ? (
        <div className="text-center py-12 text-sm text-dark-300">
          No rotations yet. Create one to route escalations to a specific user.
        </div>
      ) : (
        <div className="space-y-2">
          {schedules.map((s) => (
            <div key={s.id} className="p-4 bg-dark-800 border border-dark-500 rounded-lg">
              <div className="flex items-start justify-between gap-3">
                <div className="min-w-0">
                  <div className="flex items-center gap-2 flex-wrap mb-2">
                    <span className="font-medium text-dark-50">{s.name}</span>
                    <span className="px-1.5 py-0.5 rounded text-xs bg-dark-700 text-dark-300">
                      every {s.cadence_days} day{s.cadence_days === 1 ? "" : "s"}
                    </span>
                    {s.current_on_call ? (
                      <span className="px-1.5 py-0.5 rounded text-xs bg-rust-500/15 text-rust-400">
                        on-call now: {s.current_on_call.email}
                      </span>
                    ) : (
                      <span className="px-1.5 py-0.5 rounded text-xs bg-dark-700 text-dark-300">
                        no current member
                      </span>
                    )}
                  </div>
                  <div className="flex gap-1.5 flex-wrap">
                    {s.members.map((m, i) => (
                      <span
                        key={`${m.id}-${i}`}
                        className={`px-2 py-0.5 rounded text-xs ${
                          s.current_on_call?.id === m.id
                            ? "bg-rust-500/20 text-rust-300"
                            : "bg-dark-700 text-dark-200"
                        }`}
                        title={m.id}
                      >
                        {i + 1}. {m.email}
                      </span>
                    ))}
                  </div>
                </div>
                <div className="flex gap-1.5 shrink-0">
                  <button
                    onClick={() => openEdit(s)}
                    className="px-2.5 py-1 bg-dark-700 text-dark-100 rounded-lg text-xs hover:bg-dark-600"
                  >
                    Edit
                  </button>
                  {confirmDelete === s.id ? (
                    <>
                      <button
                        onClick={() => remove(s.id)}
                        className="px-2.5 py-1 bg-danger-500 text-white rounded-lg text-xs hover:bg-danger-600"
                      >
                        Confirm
                      </button>
                      <button
                        onClick={() => setConfirmDelete(null)}
                        className="px-2.5 py-1 bg-dark-700 text-dark-200 rounded-lg text-xs hover:bg-dark-600"
                      >
                        Cancel
                      </button>
                    </>
                  ) : (
                    <button
                      onClick={() => setConfirmDelete(s.id)}
                      className="px-2.5 py-1 bg-danger-500/15 text-danger-400 rounded-lg text-xs hover:bg-danger-500/25"
                    >
                      Delete
                    </button>
                  )}
                </div>
              </div>
            </div>
          ))}
        </div>
      )}

      {editing && (
        <div className="fixed inset-0 bg-black/60 flex items-center justify-center p-4 z-50" onClick={closeEdit}>
          <div className="bg-dark-900 border border-dark-500 rounded-lg max-w-2xl w-full max-h-[90vh] flex flex-col" onClick={(e) => e.stopPropagation()}>
            <div className="px-6 py-4 border-b border-dark-500 flex items-center justify-between">
              <h3 className="font-medium text-dark-50">
                {editing.id ? "Edit rotation" : "New rotation"}
              </h3>
              <button onClick={closeEdit} className="text-dark-300 hover:text-dark-100 text-xl leading-none">×</button>
            </div>
            <div className="flex-1 overflow-y-auto p-6 space-y-4">
              <div>
                <label className="block text-xs font-medium text-dark-200 mb-1">Name</label>
                <input
                  type="text"
                  value={draftName}
                  onChange={(e) => setDraftName(e.target.value)}
                  placeholder="e.g. Primary, Database team"
                  className="w-full bg-dark-800 border border-dark-500 rounded p-2 text-sm text-dark-100 outline-none focus:border-rust-500/50"
                />
              </div>
              <div>
                <label className="block text-xs font-medium text-dark-200 mb-1">
                  Cadence (days): {draftCadence}
                </label>
                <input
                  type="range"
                  min={1}
                  max={90}
                  value={draftCadence}
                  onChange={(e) => setDraftCadence(parseInt(e.target.value, 10))}
                  className="w-full"
                />
                <div className="text-xs text-dark-300 mt-1">
                  Rotation advances every {draftCadence} day{draftCadence === 1 ? "" : "s"}.
                </div>
              </div>
              <div>
                <label className="block text-xs font-medium text-dark-200 mb-1">
                  Members (ordered — top of list goes first)
                </label>
                {draftMembers.length === 0 ? (
                  <div className="text-xs text-dark-300 mb-2">No members yet — add from the list below.</div>
                ) : (
                  <div className="space-y-1 mb-3">
                    {draftMembers.map((mid, i) => {
                      const u = users.find((x) => x.id === mid);
                      return (
                        <div key={`${mid}-${i}`} className="flex items-center gap-2 bg-dark-800 border border-dark-500 rounded p-2">
                          <span className="text-xs text-dark-300 w-5">{i + 1}.</span>
                          <span className="flex-1 text-sm text-dark-100">{u?.email || mid}</span>
                          <button
                            onClick={() => moveMember(i, -1)}
                            disabled={i === 0}
                            className="px-1.5 py-0.5 text-xs text-dark-200 hover:text-dark-100 disabled:opacity-30"
                          >▲</button>
                          <button
                            onClick={() => moveMember(i, 1)}
                            disabled={i === draftMembers.length - 1}
                            className="px-1.5 py-0.5 text-xs text-dark-200 hover:text-dark-100 disabled:opacity-30"
                          >▼</button>
                          <button
                            onClick={() => setDraftMembers(draftMembers.filter((_, idx) => idx !== i))}
                            className="px-1.5 py-0.5 text-xs text-danger-400 hover:text-danger-300"
                          >Remove</button>
                        </div>
                      );
                    })}
                  </div>
                )}
                <div className="bg-dark-800 border border-dark-500 rounded p-2 max-h-40 overflow-y-auto">
                  <div className="text-xs text-dark-300 mb-1">Click to add:</div>
                  {users.filter((u) => !draftMembers.includes(u.id)).length === 0 ? (
                    <div className="text-xs text-dark-300">All users are already in the rotation.</div>
                  ) : (
                    users
                      .filter((u) => !draftMembers.includes(u.id))
                      .map((u) => (
                        <button
                          key={u.id}
                          onClick={() => setDraftMembers([...draftMembers, u.id])}
                          className="block w-full text-left px-2 py-1 text-sm text-dark-100 hover:bg-dark-700 rounded"
                        >
                          {u.email}
                        </button>
                      ))
                  )}
                </div>
              </div>
            </div>
            <div className="px-6 py-4 border-t border-dark-500 flex justify-end gap-2">
              <button
                onClick={closeEdit}
                className="px-3 py-1.5 bg-dark-700 text-dark-200 hover:bg-dark-600 hover:text-dark-100 border border-dark-500 rounded-lg text-sm"
              >Cancel</button>
              <button
                onClick={save}
                className="px-3 py-1.5 bg-rust-500 text-white rounded-lg text-sm font-medium hover:bg-rust-600"
              >Save</button>
            </div>
          </div>
        </div>
      )}
    </div>
  );
}

// ---------------------------------------------------------------------------
// Phase 4 W3: Escalation policies editor
// ---------------------------------------------------------------------------

function PoliciesTab({ onMessage }: { onMessage: (m: { text: string; type: string }) => void }) {
  const [policies, setPolicies] = useState<EscalationPolicy[]>([]);
  const [schedules, setSchedules] = useState<OnCallSchedule[]>([]);
  const [users, setUsers] = useState<MemberInfo[]>([]);
  const [loading, setLoading] = useState(true);
  const [editing, setEditing] = useState<EscalationPolicy | null>(null);
  const [draftName, setDraftName] = useState("");
  const [draftSteps, setDraftSteps] = useState<EscalationStep[]>([]);
  const [confirmDelete, setConfirmDelete] = useState<string | null>(null);

  const fetchAll = async () => {
    try {
      const [p, s, u] = await Promise.all([
        api.get<EscalationPolicy[]>("/escalation-policies"),
        api.get<OnCallSchedule[]>("/on-call/schedules"),
        api.get<{ id: string; email: string }[]>("/users"),
      ]);
      setPolicies(p);
      setSchedules(s);
      setUsers(u.map((x) => ({ id: x.id, email: x.email })));
    } catch (e) {
      logger.error("Failed to load policies:", e);
      onMessage({ text: "Failed to load escalation policies", type: "error" });
    } finally {
      setLoading(false);
    }
  };

  useEffect(() => {
    fetchAll();
  }, []);

  const openNew = () => {
    setEditing({
      id: "",
      name: "",
      steps: [{ after_minutes: 0, route: "all_channels" }],
      used_by_rule_count: 0,
      created_at: "",
      updated_at: "",
    });
    setDraftName("");
    setDraftSteps([{ after_minutes: 0, route: "all_channels" }]);
  };

  const openEdit = (p: EscalationPolicy) => {
    setEditing(p);
    setDraftName(p.name);
    setDraftSteps(p.steps);
  };

  const closeEdit = () => {
    setEditing(null);
  };

  const save = async () => {
    if (!editing) return;
    if (!draftName.trim()) {
      onMessage({ text: "Name is required", type: "error" });
      return;
    }
    if (draftSteps.length === 0) {
      onMessage({ text: "At least one step is required", type: "error" });
      return;
    }
    if (draftSteps[0].after_minutes !== 0) {
      onMessage({ text: "First step must have after_minutes = 0", type: "error" });
      return;
    }
    for (let i = 1; i < draftSteps.length; i++) {
      if (draftSteps[i].after_minutes <= draftSteps[i - 1].after_minutes) {
        onMessage({ text: `Step ${i + 1} must be after step ${i} in minutes`, type: "error" });
        return;
      }
    }
    try {
      const body = { name: draftName.trim(), steps: draftSteps };
      if (editing.id) {
        await api.put(`/escalation-policies/${editing.id}`, body);
      } else {
        await api.post(`/escalation-policies`, body);
      }
      onMessage({ text: "Policy saved", type: "success" });
      closeEdit();
      fetchAll();
    } catch (e) {
      logger.error("Failed to save policy:", e);
      const detail = (e as { response?: { data?: { error?: string } } })?.response?.data?.error;
      onMessage({ text: detail || "Failed to save policy", type: "error" });
    }
  };

  const remove = async (id: string) => {
    try {
      await api.delete(`/escalation-policies/${id}`);
      onMessage({ text: "Policy deleted", type: "success" });
      setConfirmDelete(null);
      fetchAll();
    } catch (e) {
      logger.error("Failed to delete policy:", e);
      onMessage({ text: "Failed to delete policy", type: "error" });
    }
  };

  const describeRoute = (route: string): string => {
    if (route === "all_channels") return "All configured channels (alert owner)";
    if (route.startsWith("on_call_schedule:")) {
      const id = route.slice("on_call_schedule:".length);
      const s = schedules.find((x) => x.id === id);
      return s ? `On-call schedule: ${s.name}` : `On-call schedule: ${id.slice(0, 8)}…`;
    }
    if (route.startsWith("user:")) {
      const id = route.slice("user:".length);
      const u = users.find((x) => x.id === id);
      return u ? `User: ${u.email}` : `User: ${id.slice(0, 8)}…`;
    }
    if (route.startsWith("webhook:")) {
      return `Webhook: ${route.slice("webhook:".length)}`;
    }
    return route;
  };

  return (
    <div>
      <div className="bg-dark-800 border border-dark-500 rounded-lg p-4 mb-6">
        <div className="flex items-start justify-between gap-4 flex-wrap">
          <div className="min-w-0 max-w-2xl">
            <h2 className="font-medium text-dark-50 mb-1">Escalation policies</h2>
            <p className="text-sm text-dark-200">
              An ordered chain of steps. The first fires the moment an alert is
              created; each later step fires if the alert still isn't
              acknowledged once its threshold passes. After the last step the
              alert does not go quiet — it keeps re-paging that step's route
              every 30 minutes until someone acknowledges or resolves it.
              Attach a policy under Settings → Notifications; without one,
              alerts re-page every 30 minutes once they are 15 minutes old.
            </p>
          </div>
          <button
            onClick={openNew}
            className="px-3 py-1.5 bg-rust-500 text-white rounded-lg text-sm font-medium hover:bg-rust-600 shrink-0"
          >
            New policy
          </button>
        </div>
      </div>

      {loading ? (
        <div className="space-y-3">
          {[1, 2].map((i) => (
            <div key={i} className="h-20 bg-dark-800 rounded-lg animate-pulse" />
          ))}
        </div>
      ) : policies.length === 0 ? (
        <div className="text-center py-12 text-sm text-dark-300">
          No policies yet. Create one to chain escalation through on-call rotations.
        </div>
      ) : (
        <div className="space-y-2">
          {policies.map((p) => (
            <div key={p.id} className="p-4 bg-dark-800 border border-dark-500 rounded-lg">
              <div className="flex items-start justify-between gap-3">
                <div className="min-w-0">
                  <div className="flex items-center gap-2 flex-wrap mb-2">
                    <span className="font-medium text-dark-50">{p.name}</span>
                    <span className="px-1.5 py-0.5 rounded text-xs bg-dark-700 text-dark-300">
                      {p.steps.length} step{p.steps.length === 1 ? "" : "s"}
                    </span>
                    <span className="px-1.5 py-0.5 rounded text-xs bg-accent-500/10 text-accent-400">
                      used by {p.used_by_rule_count} rule{p.used_by_rule_count === 1 ? "" : "s"}
                    </span>
                  </div>
                  <ol className="space-y-1 text-sm">
                    {p.steps.map((step, i) => (
                      <li key={i} className="text-dark-200">
                        <span className="text-dark-300 font-mono">+{step.after_minutes}m:</span>{" "}
                        {describeRoute(step.route)}
                      </li>
                    ))}
                  </ol>
                </div>
                <div className="flex gap-1.5 shrink-0">
                  <button
                    onClick={() => openEdit(p)}
                    className="px-2.5 py-1 bg-dark-700 text-dark-100 rounded-lg text-xs hover:bg-dark-600"
                  >
                    Edit
                  </button>
                  {confirmDelete === p.id ? (
                    <>
                      <button
                        onClick={() => remove(p.id)}
                        className="px-2.5 py-1 bg-danger-500 text-white rounded-lg text-xs hover:bg-danger-600"
                      >Confirm</button>
                      <button
                        onClick={() => setConfirmDelete(null)}
                        className="px-2.5 py-1 bg-dark-700 text-dark-200 rounded-lg text-xs hover:bg-dark-600"
                      >Cancel</button>
                    </>
                  ) : (
                    <button
                      onClick={() => setConfirmDelete(p.id)}
                      className="px-2.5 py-1 bg-danger-500/15 text-danger-400 rounded-lg text-xs hover:bg-danger-500/25"
                    >
                      Delete
                    </button>
                  )}
                </div>
              </div>
            </div>
          ))}
        </div>
      )}

      {editing && (
        <PolicyEditModal
          name={draftName}
          steps={draftSteps}
          schedules={schedules}
          users={users}
          onNameChange={setDraftName}
          onStepsChange={setDraftSteps}
          onClose={closeEdit}
          onSave={save}
          isNew={!editing.id}
        />
      )}
    </div>
  );
}

function PolicyEditModal({
  name,
  steps,
  schedules,
  users,
  onNameChange,
  onStepsChange,
  onClose,
  onSave,
  isNew,
}: {
  name: string;
  steps: EscalationStep[];
  schedules: OnCallSchedule[];
  users: MemberInfo[];
  onNameChange: (s: string) => void;
  onStepsChange: (s: EscalationStep[]) => void;
  onClose: () => void;
  onSave: () => void;
  isNew: boolean;
}) {
  const updateStep = (i: number, patch: Partial<EscalationStep>) => {
    const next = steps.map((s, idx) => (idx === i ? { ...s, ...patch } : s));
    onStepsChange(next);
  };
  const addStep = () => {
    const last = steps[steps.length - 1];
    const nextMinutes = last ? last.after_minutes + 5 : 0;
    onStepsChange([...steps, { after_minutes: nextMinutes, route: "all_channels" }]);
  };
  const removeStep = (i: number) => {
    if (i === 0) return; // Step 0 is structural — can't delete.
    onStepsChange(steps.filter((_, idx) => idx !== i));
  };
  const routeKind = (route: string): "all_channels" | "on_call_schedule" | "user" | "webhook" => {
    if (route === "all_channels") return "all_channels";
    if (route.startsWith("on_call_schedule:")) return "on_call_schedule";
    if (route.startsWith("user:")) return "user";
    return "webhook";
  };

  return (
    <div className="fixed inset-0 bg-black/60 flex items-center justify-center p-4 z-50" onClick={onClose}>
      <div className="bg-dark-900 border border-dark-500 rounded-lg max-w-3xl w-full max-h-[90vh] flex flex-col" onClick={(e) => e.stopPropagation()}>
        <div className="px-6 py-4 border-b border-dark-500 flex items-center justify-between">
          <h3 className="font-medium text-dark-50">
            {isNew ? "New policy" : "Edit policy"}
          </h3>
          <button onClick={onClose} className="text-dark-300 hover:text-dark-100 text-xl leading-none">×</button>
        </div>
        <div className="flex-1 overflow-y-auto p-6 space-y-4">
          <div>
            <label className="block text-xs font-medium text-dark-200 mb-1">Name</label>
            <input
              type="text"
              value={name}
              onChange={(e) => onNameChange(e.target.value)}
              placeholder="e.g. Critical infra escalation"
              className="w-full bg-dark-800 border border-dark-500 rounded p-2 text-sm text-dark-100 outline-none focus:border-rust-500/50"
            />
          </div>

          <div>
            <label className="block text-xs font-medium text-dark-200 mb-2">Steps (ordered, after_minutes must strictly increase)</label>
            <div className="space-y-2">
              {steps.map((step, i) => {
                const kind = routeKind(step.route);
                return (
                  <div key={i} className="bg-dark-800 border border-dark-500 rounded p-3">
                    <div className="flex items-center gap-3 mb-2">
                      <span className="text-xs text-dark-300 font-mono w-12">step {i}</span>
                      <label className="text-xs text-dark-200">After</label>
                      <input
                        type="number"
                        min={0}
                        max={1440}
                        value={step.after_minutes}
                        onChange={(e) => updateStep(i, { after_minutes: parseInt(e.target.value, 10) || 0 })}
                        disabled={i === 0}
                        className="w-20 bg-dark-700 border border-dark-500 rounded px-2 py-1 text-sm text-dark-100 disabled:opacity-50 outline-none"
                      />
                      <span className="text-xs text-dark-300">minutes</span>
                      {i > 0 && (
                        <button
                          onClick={() => removeStep(i)}
                          className="ml-auto px-2 py-0.5 text-xs text-danger-400 hover:text-danger-300"
                        >
                          Remove step
                        </button>
                      )}
                    </div>
                    <div className="flex items-center gap-2 flex-wrap">
                      <select
                        value={kind}
                        onChange={(e) => {
                          const v = e.target.value as "all_channels" | "on_call_schedule" | "user" | "webhook";
                          if (v === "all_channels") updateStep(i, { route: "all_channels" });
                          else if (v === "on_call_schedule") updateStep(i, { route: schedules[0] ? `on_call_schedule:${schedules[0].id}` : "all_channels" });
                          else if (v === "user") updateStep(i, { route: users[0] ? `user:${users[0].id}` : "all_channels" });
                          else updateStep(i, { route: "webhook:https://" });
                        }}
                        className="bg-dark-700 border border-dark-500 rounded text-sm text-dark-100 px-2 py-1"
                      >
                        <option value="all_channels">All channels (alert owner)</option>
                        <option value="on_call_schedule">On-call schedule</option>
                        <option value="user">Specific user</option>
                        <option value="webhook">Webhook URL</option>
                      </select>
                      {kind === "on_call_schedule" && (
                        <select
                          value={step.route.slice("on_call_schedule:".length)}
                          onChange={(e) => updateStep(i, { route: `on_call_schedule:${e.target.value}` })}
                          className="bg-dark-700 border border-dark-500 rounded text-sm text-dark-100 px-2 py-1"
                        >
                          {schedules.length === 0 ? (
                            <option value="">— no schedules defined —</option>
                          ) : (
                            schedules.map((s) => (
                              <option key={s.id} value={s.id}>{s.name}</option>
                            ))
                          )}
                        </select>
                      )}
                      {kind === "user" && (
                        <select
                          value={step.route.slice("user:".length)}
                          onChange={(e) => updateStep(i, { route: `user:${e.target.value}` })}
                          className="bg-dark-700 border border-dark-500 rounded text-sm text-dark-100 px-2 py-1"
                        >
                          {users.length === 0 ? (
                            <option value="">— no users —</option>
                          ) : (
                            users.map((u) => (
                              <option key={u.id} value={u.id}>{u.email}</option>
                            ))
                          )}
                        </select>
                      )}
                      {kind === "webhook" && (
                        <input
                          type="url"
                          value={step.route.slice("webhook:".length)}
                          onChange={(e) => updateStep(i, { route: `webhook:${e.target.value}` })}
                          placeholder="https://hooks.example.com/escalation"
                          className="flex-1 min-w-[200px] bg-dark-700 border border-dark-500 rounded text-sm text-dark-100 px-2 py-1 outline-none"
                        />
                      )}
                    </div>
                  </div>
                );
              })}
              {steps.length < 10 && (
                <button
                  onClick={addStep}
                  className="px-3 py-1.5 bg-dark-700 text-dark-100 rounded-lg text-sm hover:bg-dark-600"
                >
                  + Add step
                </button>
              )}
            </div>
          </div>
        </div>
        <div className="px-6 py-4 border-t border-dark-500 flex justify-end gap-2">
          <button onClick={onClose} className="px-3 py-1.5 bg-dark-700 text-dark-200 hover:bg-dark-600 hover:text-dark-100 border border-dark-500 rounded-lg text-sm">Cancel</button>
          <button onClick={onSave} className="px-3 py-1.5 bg-rust-500 text-white rounded-lg text-sm font-medium hover:bg-rust-600">Save</button>
        </div>
      </div>
    </div>
  );
}
