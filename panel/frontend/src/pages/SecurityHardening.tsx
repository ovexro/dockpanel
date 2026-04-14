import { useState, useEffect } from "react";
import { Navigate } from "react-router-dom";
import { useAuth } from "../context/AuthContext";
import { api } from "../api";

interface AuditEntry {
  id: string;
  event_type: string;
  actor_email: string | null;
  actor_ip: string | null;
  target_type: string | null;
  target_name: string | null;
  details: string | null;
  geo_country: string | null;
  geo_city: string | null;
  severity: string;
  created_at: string;
}

interface LockdownState {
  active: boolean;
  triggered_by: string | null;
  triggered_at: string | null;
  reason: string | null;
}

interface Recording {
  filename: string;
  size_bytes: number;
  created: string | null;
}

interface PendingUser {
  id: string;
  email: string;
  created_at: string;
}

type Tab = "overview" | "audit" | "lockdown" | "recordings" | "approvals";

export default function SecurityHardening() {
  const { user } = useAuth();
  const [tab, setTab] = useState<Tab>("overview");
  const [lockdown, setLockdown] = useState<LockdownState | null>(null);
  const [auditLog, setAuditLog] = useState<AuditEntry[]>([]);
  const [recordings, setRecordings] = useState<Recording[]>([]);
  const [pendingUsers, setPendingUsers] = useState<PendingUser[]>([]);
  const [loading, setLoading] = useState(true);
  const [message, setMessage] = useState({ type: "", text: "" });
  const [pendingConfirm, setPendingConfirm] = useState<{ type: string; label: string } | null>(null);

  if (!user || user.role !== "admin") return <Navigate to="/" replace />;

  const showMsg = (type: string, text: string) => {
    setMessage({ type, text });
    setTimeout(() => setMessage({ type: "", text: "" }), 5000);
  };

  const loadData = async () => {
    try {
      const [lock, audit, recs, pending] = await Promise.all([
        api.get<LockdownState>("/security/lockdown"),
        api.get<AuditEntry[]>("/security/audit-log?limit=50"),
        api.get<{ recordings: Recording[] }>("/security/recordings"),
        api.get<PendingUser[]>("/security/pending-users"),
      ]);
      setLockdown(lock);
      setAuditLog(audit);
      setRecordings(recs.recordings || []);
      setPendingUsers(pending);
    } catch (e) {
      showMsg("error", e instanceof Error ? e.message : "Failed to load security data");
    } finally {
      setLoading(false);
    }
  };

  useEffect(() => { loadData(); }, []);

  const activateLockdown = () => {
    setPendingConfirm({ type: "lockdown", label: "Activate system lockdown? This will block all non-admin access." });
  };

  const deactivateLockdown = async () => {
    try {
      await api.post("/security/lockdown/deactivate", {});
      showMsg("success", "Lockdown deactivated");
      loadData();
    } catch (e) { showMsg("error", e instanceof Error ? e.message : "Failed"); }
  };

  const triggerPanic = () => {
    setPendingConfirm({ type: "panic", label: "EMERGENCY: This will kill all terminals, block non-admins, and disable registration. Continue?" });
  };

  const executeConfirm = async () => {
    if (!pendingConfirm) return;
    const { type } = pendingConfirm;
    setPendingConfirm(null);
    try {
      if (type === "lockdown") {
        await api.post("/security/lockdown/activate", { reason: "Manual admin lockdown" });
        showMsg("success", "Lockdown activated");
      } else if (type === "panic") {
        await api.post("/security/panic", {});
        showMsg("success", "Panic mode activated — all terminals killed, system locked");
      }
      loadData();
    } catch (e) { showMsg("error", e instanceof Error ? e.message : "Failed"); }
  };

  const triggerSnapshot = async () => {
    try {
      const result = await api.post<{ snapshot_dir: string }>("/security/forensic-snapshot", {});
      showMsg("success", `Forensic snapshot saved to ${result.snapshot_dir}`);
    } catch (e) { showMsg("error", e instanceof Error ? e.message : "Failed"); }
  };

  const approveUser = async (id: string) => {
    try {
      await api.post(`/security/users/${id}/approve`, {});
      showMsg("success", "User approved");
      loadData();
    } catch (e) { showMsg("error", e instanceof Error ? e.message : "Failed"); }
  };

  const severityColor = (s: string) => {
    if (s === "critical") return "text-danger-400 bg-danger-500/10";
    if (s === "warning") return "text-warn-400 bg-warn-500/10";
    return "text-accent-400 bg-accent-500/10";
  };

  const tabs: { key: Tab; label: string }[] = [
    { key: "overview", label: "Overview" },
    { key: "lockdown", label: "Lockdown" },
    { key: "audit", label: "Audit Log" },
    { key: "recordings", label: "Recordings" },
    { key: "approvals", label: "Approvals" },
  ];

  if (loading) return <div className="flex items-center justify-center h-64"><div className="w-6 h-6 border-2 border-dark-600 border-t-rust-500 rounded-full animate-spin" /></div>;

  return (
    <div className="p-6 lg:p-8">
      <div className="flex flex-col sm:flex-row sm:items-center justify-between gap-3 mb-6 pb-4 border-b border-dark-600">
        <h1 className="text-sm font-medium text-dark-300 uppercase font-mono tracking-widest">Security Hardening</h1>
        <div className="flex gap-2">
          <button onClick={triggerSnapshot} className="px-3 py-1.5 text-xs font-mono bg-dark-700 hover:bg-dark-600 text-dark-200 rounded-lg border border-dark-500">
            Forensic Snapshot
          </button>
          <button onClick={triggerPanic} className="px-3 py-1.5 text-xs font-mono bg-danger-500 hover:bg-danger-600 text-white rounded-lg">
            Panic Button
          </button>
        </div>
      </div>

      {message.text && (
        <div className={`mb-4 px-4 py-3 rounded-lg text-sm border ${message.type === "success" ? "bg-rust-500/10 text-rust-400 border-rust-500/20" : "bg-danger-500/10 text-danger-400 border-danger-500/20"}`}>
          {message.text}
        </div>
      )}

      {/* Inline confirmation bar */}
      {pendingConfirm && (
        <div className="mb-4 px-4 py-3 rounded-lg border flex items-center justify-between border-danger-500/30 bg-danger-500/5">
          <span className="text-xs font-mono text-danger-400">
            {pendingConfirm.label}
          </span>
          <div className="flex items-center gap-2 shrink-0 ml-4">
            <button onClick={executeConfirm} className="px-3 py-1.5 bg-danger-500 text-white text-xs font-bold uppercase tracking-wider hover:bg-danger-400 transition-colors">
              Confirm
            </button>
            <button onClick={() => setPendingConfirm(null)} className="px-3 py-1.5 bg-dark-600 text-dark-200 text-xs font-bold uppercase tracking-wider hover:bg-dark-500 transition-colors">
              Cancel
            </button>
          </div>
        </div>
      )}

      {/* Tabs */}
      <div className="flex gap-6 mb-6 text-sm font-mono border-b border-dark-700 overflow-x-auto">
        {tabs.map(t => (
          <button key={t.key} onClick={() => setTab(t.key)}
            className={`pb-2 whitespace-nowrap ${tab === t.key ? "border-b-2 border-rust-500 text-dark-50" : "text-dark-400 hover:text-dark-200"}`}>
            {t.label}
            {t.key === "approvals" && pendingUsers.length > 0 && (
              <span className="ml-1.5 px-1.5 py-0.5 text-[10px] bg-rust-500 text-white rounded-full">{pendingUsers.length}</span>
            )}
          </button>
        ))}
      </div>

      {/* Overview Tab */}
      {tab === "overview" && (
        <div className="grid grid-cols-1 md:grid-cols-2 lg:grid-cols-4 gap-4">
          <div className="bg-dark-800 rounded-lg border border-dark-500 p-5">
            <p className="text-xs font-mono text-dark-400 mb-2">Lockdown Status</p>
            <div className={`text-lg font-bold ${lockdown?.active ? "text-danger-400" : "text-rust-400"}`}>
              {lockdown?.active ? "ACTIVE" : "Inactive"}
            </div>
            {lockdown?.active && <p className="text-xs text-dark-400 mt-1">{lockdown.reason}</p>}
          </div>
          <div className="bg-dark-800 rounded-lg border border-dark-500 p-5">
            <p className="text-xs font-mono text-dark-400 mb-2">Audit Events (24h)</p>
            <div className="text-lg font-bold text-dark-50">{auditLog.length}</div>
          </div>
          <div className="bg-dark-800 rounded-lg border border-dark-500 p-5">
            <p className="text-xs font-mono text-dark-400 mb-2">Terminal Recordings</p>
            <div className="text-lg font-bold text-dark-50">{recordings.length}</div>
          </div>
          <div className="bg-dark-800 rounded-lg border border-dark-500 p-5">
            <p className="text-xs font-mono text-dark-400 mb-2">Pending Approvals</p>
            <div className={`text-lg font-bold ${pendingUsers.length > 0 ? "text-warn-400" : "text-dark-50"}`}>
              {pendingUsers.length}
            </div>
          </div>

          {/* Recent critical events */}
          <div className="col-span-full bg-dark-800 rounded-lg border border-dark-500 p-5">
            <p className="text-xs font-mono text-dark-400 mb-3 uppercase">Recent Critical Events</p>
            {auditLog.filter(e => e.severity === "critical" || e.severity === "warning").length === 0 ? (
              <p className="text-sm text-dark-500">No critical events</p>
            ) : (
              <div className="space-y-2">
                {auditLog.filter(e => e.severity === "critical" || e.severity === "warning").slice(0, 10).map(e => (
                  <div key={e.id} className="flex items-center gap-3 text-sm">
                    <span className={`px-2 py-0.5 rounded text-[10px] font-mono uppercase ${severityColor(e.severity)}`}>{e.severity}</span>
                    <span className="text-dark-200 font-mono">{e.event_type}</span>
                    <span className="text-dark-400">{e.actor_email || "-"}</span>
                    <span className="text-dark-500 ml-auto text-xs">{new Date(e.created_at).toLocaleString()}</span>
                  </div>
                ))}
              </div>
            )}
          </div>
        </div>
      )}

      {/* Lockdown Tab */}
      {tab === "lockdown" && (
        <div className="space-y-4">
          <div className={`bg-dark-800 rounded-lg border p-6 ${lockdown?.active ? "border-danger-500/50" : "border-dark-500"}`}>
            <div className="flex items-center justify-between mb-4">
              <div>
                <h3 className="text-dark-50 font-medium">System Lockdown</h3>
                <p className="text-sm text-dark-400 mt-1">
                  {lockdown?.active
                    ? `Lockdown active since ${lockdown.triggered_at ? new Date(lockdown.triggered_at).toLocaleString() : "unknown"}`
                    : "System is operating normally"}
                </p>
                {lockdown?.reason && <p className="text-sm text-warn-400 mt-1">{lockdown.reason}</p>}
              </div>
              {lockdown?.active ? (
                <button onClick={deactivateLockdown} className="px-4 py-2 text-sm font-mono bg-rust-500 hover:bg-rust-600 text-white rounded-lg">
                  Unlock System
                </button>
              ) : (
                <button onClick={activateLockdown} className="px-4 py-2 text-sm font-mono bg-warn-500 hover:bg-warn-600 text-white rounded-lg">
                  Activate Lockdown
                </button>
              )}
            </div>
            <div className="text-xs text-dark-500 space-y-1">
              <p>When locked: terminals disabled, registration blocked, non-admin logins blocked</p>
              <p>Auto-expires after 24 hours. Panic button also activates lockdown.</p>
            </div>
          </div>
        </div>
      )}

      {/* Audit Log Tab */}
      {tab === "audit" && (
        <div className="bg-dark-800 rounded-lg border border-dark-500 overflow-hidden">
          <table className="w-full text-sm">
            <thead>
              <tr className="border-b border-dark-600 text-left text-xs font-mono text-dark-400 uppercase">
                <th className="px-4 py-3">Severity</th>
                <th className="px-4 py-3">Event</th>
                <th className="px-4 py-3">Actor</th>
                <th className="px-4 py-3">IP</th>
                <th className="px-4 py-3">Location</th>
                <th className="px-4 py-3">Details</th>
                <th className="px-4 py-3">Time</th>
              </tr>
            </thead>
            <tbody>
              {auditLog.map(e => (
                <tr key={e.id} className="border-b border-dark-700 hover:bg-dark-750">
                  <td className="px-4 py-2.5">
                    <span className={`px-2 py-0.5 rounded text-[10px] font-mono uppercase ${severityColor(e.severity)}`}>{e.severity}</span>
                  </td>
                  <td className="px-4 py-2.5 font-mono text-dark-200">{e.event_type}</td>
                  <td className="px-4 py-2.5 text-dark-300">{e.actor_email || "-"}</td>
                  <td className="px-4 py-2.5 text-dark-400 font-mono text-xs">{e.actor_ip || "-"}</td>
                  <td className="px-4 py-2.5 text-dark-400 text-xs">
                    {e.geo_country ? `${e.geo_country}${e.geo_city ? `, ${e.geo_city}` : ""}` : "-"}
                  </td>
                  <td className="px-4 py-2.5 text-dark-400 text-xs max-w-[200px] truncate">{e.details || "-"}</td>
                  <td className="px-4 py-2.5 text-dark-500 text-xs whitespace-nowrap">{new Date(e.created_at).toLocaleString()}</td>
                </tr>
              ))}
              {auditLog.length === 0 && (
                <tr><td colSpan={7} className="px-4 py-8 text-center text-dark-500">No audit events yet</td></tr>
              )}
            </tbody>
          </table>
        </div>
      )}

      {/* Recordings Tab */}
      {tab === "recordings" && (
        <div className="bg-dark-800 rounded-lg border border-dark-500 overflow-hidden">
          <table className="w-full text-sm">
            <thead>
              <tr className="border-b border-dark-600 text-left text-xs font-mono text-dark-400 uppercase">
                <th className="px-4 py-3">Filename</th>
                <th className="px-4 py-3">Size</th>
                <th className="px-4 py-3">Created</th>
              </tr>
            </thead>
            <tbody>
              {recordings.map((r, i) => (
                <tr key={i} className="border-b border-dark-700 hover:bg-dark-750">
                  <td className="px-4 py-2.5 font-mono text-dark-200">{r.filename}</td>
                  <td className="px-4 py-2.5 text-dark-400">{(r.size_bytes / 1024).toFixed(1)} KB</td>
                  <td className="px-4 py-2.5 text-dark-500 text-xs">{r.created || "-"}</td>
                </tr>
              ))}
              {recordings.length === 0 && (
                <tr><td colSpan={3} className="px-4 py-8 text-center text-dark-500">No recordings yet. Recordings are created when terminal sessions start.</td></tr>
              )}
            </tbody>
          </table>
        </div>
      )}

      {/* Approvals Tab */}
      {tab === "approvals" && (
        <div className="space-y-4">
          <p className="text-sm text-dark-400">
            Users awaiting admin approval. Enable approval mode in Settings &gt; Security.
          </p>
          <div className="bg-dark-800 rounded-lg border border-dark-500 overflow-hidden">
            <table className="w-full text-sm">
              <thead>
                <tr className="border-b border-dark-600 text-left text-xs font-mono text-dark-400 uppercase">
                  <th className="px-4 py-3">Email</th>
                  <th className="px-4 py-3">Registered</th>
                  <th className="px-4 py-3">Actions</th>
                </tr>
              </thead>
              <tbody>
                {pendingUsers.map(u => (
                  <tr key={u.id} className="border-b border-dark-700">
                    <td className="px-4 py-2.5 text-dark-200">{u.email}</td>
                    <td className="px-4 py-2.5 text-dark-400 text-xs">{new Date(u.created_at).toLocaleString()}</td>
                    <td className="px-4 py-2.5">
                      <button onClick={() => approveUser(u.id)}
                        className="px-3 py-1 text-xs font-mono bg-rust-500 hover:bg-rust-600 text-white rounded">
                        Approve
                      </button>
                    </td>
                  </tr>
                ))}
                {pendingUsers.length === 0 && (
                  <tr><td colSpan={3} className="px-4 py-8 text-center text-dark-500">No pending approvals</td></tr>
                )}
              </tbody>
            </table>
          </div>
        </div>
      )}
    </div>
  );
}
