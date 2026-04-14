import { useState, useEffect } from "react";
import { Navigate } from "react-router-dom";
import { useAuth } from "../context/AuthContext";
import { api } from "../api";
import DiagnosticsContent from "./Diagnostics";

interface SecurityOverview {
  firewall_active: boolean;
  firewall_rules_count: number;
  fail2ban_running: boolean;
  fail2ban_banned_total: number;
  ssh_port: number;
  ssh_password_auth: boolean;
  ssh_root_login: boolean;
  ssl_certs_count: number;
}

interface FirewallRule {
  number: number;
  to: string;
  action: string;
  from: string;
}

interface FirewallStatus {
  active: boolean;
  default_policy: string;
  rules: FirewallRule[];
}

interface JailInfo {
  name: string;
  banned_count: number;
}

interface Fail2banStatus {
  running: boolean;
  jails: JailInfo[];
}

interface ScanSummary {
  id: string;
  scan_type: string;
  status: string;
  findings_count: number;
  critical_count: number;
  warning_count: number;
  info_count: number;
  started_at: string;
  completed_at: string | null;
}

interface ScanFinding {
  id: string;
  check_type: string;
  severity: string;
  title: string;
  description: string | null;
  file_path: string | null;
  remediation: string | null;
}

interface Posture {
  score: number;
  total_scans: number;
  latest_scan: ScanSummary | null;
}

export default function Security() {
  const { user } = useAuth();
  const [overview, setOverview] = useState<SecurityOverview | null>(null);
  const [firewall, setFirewall] = useState<FirewallStatus | null>(null);
  const [fail2ban, setFail2ban] = useState<Fail2banStatus | null>(null);
  const [posture, setPosture] = useState<Posture | null>(null);
  const [scans, setScans] = useState<ScanSummary[]>([]);
  const [selectedScan, setSelectedScan] = useState<string | null>(null);
  const [findings, setFindings] = useState<ScanFinding[]>([]);
  const [loading, setLoading] = useState(true);
  const [scanning, setScanning] = useState(false);
  const [showAddRule, setShowAddRule] = useState(false);
  const [rulePort, setRulePort] = useState("");
  const [ruleProto, setRuleProto] = useState("tcp");
  const [ruleAction, setRuleAction] = useState("allow");
  const [ruleFrom, setRuleFrom] = useState("");
  const [deleteTarget, setDeleteTarget] = useState<number | null>(null);
  const [message, setMessage] = useState({ text: "", type: "" });
  const [tab, setTab] = useState<"overview" | "scans" | "diagnostics" | "audit" | "lockdown" | "recordings" | "approvals">("overview");
  const [loginAudit, setLoginAudit] = useState<{ panel: any[]; ssh: any[] }>({ panel: [], ssh: [] });

  // Security Hardening state (consolidated from SecurityHardening.tsx)
  const [lockdown, setLockdown] = useState<{ active: boolean; triggered_by?: string; triggered_at?: string; reason?: string } | null>(null);
  const [auditLog, setAuditLog] = useState<any[]>([]);
  const [recordings, setRecordings] = useState<any[]>([]);
  const [pendingUsers, setPendingUsers] = useState<any[]>([]);
  const [selectedJail, setSelectedJail] = useState<string | null>(null);
  const [bannedIps, setBannedIps] = useState<string[]>([]);
  const [banIp, setBanIp] = useState("");
  const [banJail, setBanJail] = useState("");
  const [panelJail, setPanelJail] = useState(false);
  const [pendingConfirm, setPendingConfirm] = useState<{ type: string; label: string; data?: any } | null>(null);

  const loadData = async () => {
    try {
      const [ov, fw, fb, pos, sc] = await Promise.all([
        api.get<SecurityOverview>("/security/overview").catch(() => null),
        api.get<FirewallStatus>("/security/firewall").catch(() => null),
        api.get<Fail2banStatus>("/security/fail2ban").catch(() => null),
        api.get<Posture>("/security/posture").catch(() => null),
        api.get<ScanSummary[]>("/security/scans").catch(() => []),
      ]);
      setOverview(ov);
      setFirewall(fw);
      setFail2ban(fb);
      setPosture(pos);
      setScans(sc || []);
      api.get<{ active: boolean }>("/security/panel-jail/status").then(d => setPanelJail(d.active)).catch(() => {});
      // Load hardening data (consolidated from SecurityHardening.tsx)
      api.get<any>("/security/lockdown").then(setLockdown).catch(() => {});
      api.get<any[]>("/security/audit-log?limit=50").then(setAuditLog).catch(() => {});
      api.get<{ recordings: any[] }>("/security/recordings").then(d => setRecordings(d.recordings || [])).catch(() => {});
      api.get<any[]>("/security/pending-users").then(setPendingUsers).catch(() => {});
    } finally {
      setLoading(false);
    }
  };

  const loadBannedIps = async (jail: string) => {
    try {
      const data = await api.get<{ ips: string[] }>(`/security/fail2ban/${jail}/banned`);
      setBannedIps(data.ips);
      setSelectedJail(jail);
    } catch { setBannedIps([]); }
  };

  const loadLoginAudit = async () => {
    try {
      const data = await api.get<{ panel: any[]; ssh: any[] }>("/security/login-audit");
      setLoginAudit(data);
    } catch {}
  };

  const executeConfirm = async () => {
    if (!pendingConfirm) return;
    const { type, data } = pendingConfirm;
    setPendingConfirm(null);
    try {
      switch (type) {
        case "ssh_password": {
          await api.post(data.enabled ? "/security/ssh/disable-password" : "/security/ssh/enable-password", {});
          setMessage({ text: `SSH password auth ${data.enabled ? "disabled" : "enabled"}`, type: "success" });
          loadData();
          break;
        }
        case "ssh_root": {
          await api.post("/security/ssh/disable-root", {});
          setMessage({ text: "SSH root login disabled", type: "success" });
          loadData();
          break;
        }
        case "ssh_port": {
          await api.post("/security/ssh/change-port", { port: data.port });
          setMessage({ text: `SSH port changed to ${data.port}`, type: "success" });
          loadData();
          break;
        }
        case "unban": {
          await api.post("/security/fail2ban/unban", { jail: data.jail, ip: data.ip });
          setMessage({ text: `${data.ip} unbanned from ${data.jail}`, type: "success" });
          loadBannedIps(data.jail);
          loadData();
          break;
        }
        case "ban": {
          await api.post("/security/fail2ban/ban", { jail: data.jail, ip: data.ip });
          setMessage({ text: `${data.ip} banned in ${data.jail}`, type: "success" });
          setBanIp("");
          if (selectedJail === data.jail) loadBannedIps(data.jail);
          loadData();
          break;
        }
        case "quarantine": {
          await api.post("/security/fix", { fix_type: "quarantine_file", target: data.path });
          setMessage({ text: "File quarantined", type: "success" });
          handleScan();
          break;
        }
        case "delete_file": {
          await api.post("/security/fix", { fix_type: "remove_file", target: data.path });
          setMessage({ text: "File deleted", type: "success" });
          handleScan();
          break;
        }
        case "apply_fix": {
          await api.post("/security/fix", { fix_type: data.fix_type, target: data.target });
          setMessage({ text: `Fix applied: ${data.fix_label}`, type: "success" });
          handleScan();
          break;
        }
        case "lockdown": {
          await api.post("/security/lockdown/activate", { reason: "Manual admin lockdown" });
          setMessage({ text: "Lockdown activated", type: "success" });
          loadData();
          break;
        }
        case "panic": {
          await api.post("/security/panic", {});
          setMessage({ text: "Panic mode activated", type: "success" });
          loadData();
          break;
        }
      }
    } catch (e: any) {
      setMessage({ text: e instanceof Error ? e.message : e.message || "Action failed", type: "error" });
    }
  };

  const getFixAction = (finding: ScanFinding): { type: string; target: string; label: string } | null => {
    if (finding.check_type === "open_port" && finding.title.includes("Unexpected open port")) {
      const port = finding.title.match(/port:\s*(\d+)/)?.[1];
      if (port) return { type: "block_port", target: port, label: "Block Port" };
    }
    if (finding.check_type === "malware" && finding.file_path) {
      return { type: "remove_file", target: finding.file_path, label: "Remove File" };
    }
    return null;
  };

  useEffect(() => {
    loadData();
  }, []);

  if (user?.role !== "admin") return <Navigate to="/" replace />;

  const handleScan = async () => {
    setScanning(true);
    setMessage({ text: "", type: "" });
    try {
      const result = await api.post<{ findings_count: number; critical_count: number }>("/security/scan", {});
      setMessage({
        text: `Scan completed: ${result.findings_count} findings (${result.critical_count} critical)`,
        type: result.critical_count > 0 ? "error" : "success",
      });
      loadData();
    } catch (e) {
      setMessage({
        text: e instanceof Error ? e.message : "Scan failed",
        type: "error",
      });
    } finally {
      setScanning(false);
    }
  };

  const handleViewScan = async (id: string) => {
    if (selectedScan === id) {
      setSelectedScan(null);
      return;
    }
    try {
      const result = await api.get<{ findings: ScanFinding[] }>(`/security/scans/${id}`);
      setFindings(result.findings);
      setSelectedScan(id);
    } catch {
      setMessage({ text: "Failed to load scan details", type: "error" });
    }
  };

  const handleAddRule = async () => {
    if (!rulePort) return;
    setMessage({ text: "", type: "" });
    try {
      await api.post("/security/firewall/rules", {
        port: parseInt(rulePort),
        proto: ruleProto,
        action: ruleAction,
        from: ruleFrom || null,
      });
      setShowAddRule(false);
      setRulePort("");
      setRuleFrom("");
      setMessage({ text: "Firewall rule added", type: "success" });
      loadData();
    } catch (e) {
      setMessage({
        text: e instanceof Error ? e.message : "Failed to add rule",
        type: "error",
      });
    }
  };

  const handleDeleteRule = async (num: number) => {
    try {
      await api.delete(`/security/firewall/rules/${num}`);
      setDeleteTarget(null);
      setMessage({ text: "Rule deleted", type: "success" });
      loadData();
    } catch (e) {
      setMessage({
        text: e instanceof Error ? e.message : "Failed to delete rule",
        type: "error",
      });
    }
  };

  const scoreColor = (score: number) => {
    if (score >= 80) return "text-rust-400";
    if (score >= 50) return "text-warn-500";
    return "text-danger-400";
  };

  const scoreBg = (score: number) => {
    if (score >= 80) return "bg-rust-500";
    if (score >= 50) return "bg-warn-500";
    return "bg-danger-500";
  };

  const severityBadge = (severity: string) => {
    switch (severity) {
      case "critical":
        return "bg-danger-500/15 text-danger-400 border-danger-500/20";
      case "warning":
        return "bg-warn-500/15 text-warn-400 border-warn-400/20";
      default:
        return "bg-accent-500/15 text-accent-400 border-accent-200";
    }
  };

  const checkTypeBadge = (type: string) => {
    switch (type) {
      case "malware":
        return "bg-danger-500/10 text-danger-400";
      case "file_integrity":
        return "bg-accent-600/15 text-accent-400";
      case "open_port":
        return "bg-warn-500/10 text-warn-400";
      case "ssl_expiry":
        return "bg-warn-500/10 text-warn-400";
      case "container_vuln":
        return "bg-danger-500/10 text-danger-400";
      case "security_headers":
        return "bg-accent-500/10 text-accent-400";
      default:
        return "bg-dark-900 text-dark-200";
    }
  };

  if (loading) {
    return (
      <div className="p-6 lg:p-8 animate-fade-up">
        <h1 className="text-sm font-medium text-dark-300 uppercase font-mono tracking-widest mb-6">Security</h1>
        <div className="grid grid-cols-1 md:grid-cols-2 lg:grid-cols-4 gap-4">
          {[...Array(4)].map((_, i) => (
            <div key={i} className="bg-dark-800 rounded-lg border border-dark-500 p-5 animate-pulse">
              <div className="h-4 bg-dark-700 rounded w-20 mb-3" />
              <div className="h-8 bg-dark-700 rounded w-16" />
            </div>
          ))}
        </div>
      </div>
    );
  }

  return (
    <div>
      <div className="page-header">
        <div>
          <h1 className="page-header-title">Security</h1>
          <p className="page-header-subtitle">Firewall, fail2ban, and security scanning</p>
        </div>
        <div className="flex items-center gap-2">
          <a
            href="/api/security/report"
            target="_blank"
            className="px-4 py-2 bg-dark-700 text-dark-200 hover:bg-dark-600 hover:text-dark-100 border border-dark-600 rounded-lg text-sm font-medium transition-colors"
          >
            Download Report
          </a>
          <button
            onClick={handleScan}
            disabled={scanning}
            className="px-4 py-2 bg-rust-500 text-white rounded-lg text-sm font-medium hover:bg-rust-600 disabled:opacity-50 flex items-center gap-2"
          >
            {scanning && (
              <svg className="w-4 h-4 animate-spin" fill="none" viewBox="0 0 24 24">
                <circle className="opacity-25" cx="12" cy="12" r="10" stroke="currentColor" strokeWidth="4" />
                <path className="opacity-75" fill="currentColor" d="M4 12a8 8 0 018-8V0C5.373 0 0 5.373 0 12h4z" />
              </svg>
            )}
            {scanning ? "Scanning..." : "Run Security Scan"}
          </button>
        </div>
      </div>

      <div className="p-6 lg:p-8">
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

      {/* Inline confirmation bar */}
      {pendingConfirm && (
        <div className={`mb-4 px-4 py-3 rounded-lg border flex items-center justify-between ${
          ["panic", "delete_file", "lockdown"].includes(pendingConfirm.type) ? "border-danger-500/30 bg-danger-500/5" : "border-warn-500/30 bg-warn-500/5"
        }`}>
          <span className={`text-xs font-mono ${["panic", "delete_file", "lockdown"].includes(pendingConfirm.type) ? "text-danger-400" : "text-warn-400"}`}>
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
      <div className="flex gap-1 mb-6 bg-dark-700 rounded-lg p-1 w-fit">
        <button
          onClick={() => setTab("overview")}
          className={`px-4 py-1.5 rounded-md text-sm font-medium transition-colors ${
            tab === "overview" ? "bg-dark-800 text-dark-50 shadow-sm" : "text-dark-200 hover:text-dark-100"
          }`}
        >
          Overview
        </button>
        <button
          onClick={() => setTab("scans")}
          className={`px-4 py-1.5 rounded-md text-sm font-medium transition-colors ${
            tab === "scans" ? "bg-dark-800 text-dark-50 shadow-sm" : "text-dark-200 hover:text-dark-100"
          }`}
        >
          Scan History
        </button>
        <button
          onClick={() => setTab("diagnostics")}
          className={`px-4 py-1.5 rounded-md text-sm font-medium transition-colors ${
            tab === "diagnostics" ? "bg-dark-800 text-dark-50 shadow-sm" : "text-dark-200 hover:text-dark-100"
          }`}
        >
          Diagnostics
        </button>
        <button
          onClick={() => { setTab("audit"); loadLoginAudit(); }}
          className={`px-4 py-1.5 rounded-md text-sm font-medium transition-colors ${
            tab === "audit" ? "bg-dark-800 text-dark-50 shadow-sm" : "text-dark-200 hover:text-dark-100"
          }`}
        >
          Login Audit
        </button>
        <button
          onClick={() => setTab("lockdown")}
          className={`px-4 py-1.5 rounded-md text-sm font-medium transition-colors ${
            tab === "lockdown" ? "bg-dark-800 text-dark-50 shadow-sm" : "text-dark-200 hover:text-dark-100"
          }`}
        >
          {lockdown?.active ? "🔒 Lockdown" : "Lockdown"}
        </button>
        <button
          onClick={() => setTab("recordings")}
          className={`px-4 py-1.5 rounded-md text-sm font-medium transition-colors ${
            tab === "recordings" ? "bg-dark-800 text-dark-50 shadow-sm" : "text-dark-200 hover:text-dark-100"
          }`}
        >
          Recordings
        </button>
        <button
          onClick={() => setTab("approvals")}
          className={`px-4 py-1.5 rounded-md text-sm font-medium transition-colors ${
            tab === "approvals" ? "bg-dark-800 text-dark-50 shadow-sm" : "text-dark-200 hover:text-dark-100"
          }`}
        >
          Approvals{pendingUsers.length > 0 && <span className="ml-1 px-1.5 py-0.5 text-[10px] bg-rust-500 text-white rounded-full">{pendingUsers.length}</span>}
        </button>
      </div>

      {tab === "overview" && (
        <>
          {/* Security Posture + Overview Cards */}
          <div className="grid grid-cols-1 md:grid-cols-2 lg:grid-cols-3 gap-4 mb-6">
            {/* Security Score */}
            {posture && posture.score >= 0 && (
              <div className="bg-dark-800 rounded-lg border border-dark-500 p-5 min-h-[140px]">
                <div className="flex items-center gap-2.5">
                  <div className="w-10 h-10 rounded-lg bg-accent-600/10 flex items-center justify-center">
                    <svg className="w-6 h-6 text-accent-400" fill="none" viewBox="0 0 24 24" stroke="currentColor" strokeWidth={1.5}>
                      <path strokeLinecap="round" strokeLinejoin="round" d="M9 12.75 11.25 15 15 9.75m-3-7.036A11.959 11.959 0 0 1 3.598 6 11.99 11.99 0 0 0 3 9.749c0 5.592 3.824 10.29 9 11.623 5.176-1.332 9-6.03 9-11.622 0-1.31-.21-2.571-.598-3.751h-.152c-3.196 0-6.1-1.248-8.25-3.285Z" />
                    </svg>
                  </div>
                  <p className="text-sm font-medium text-dark-200 uppercase font-mono tracking-wider">Security Score</p>
                </div>
                <div className="flex items-end gap-1 mt-3">
                  <span className={`text-4xl font-bold ${scoreColor(posture.score)}`}>{posture.score}</span>
                  <span className="text-base text-dark-300 mb-1">/100</span>
                </div>
                <div className="mt-2.5 h-2 bg-dark-700 rounded-full overflow-hidden">
                  <div className={`h-full rounded-full ${scoreBg(posture.score)}`} style={{ width: `${posture.score}%` }} />
                </div>
                <p className="text-xs text-dark-300 mt-1.5">{posture.total_scans} {posture.total_scans === 1 ? "scan" : "scans"} total</p>
              </div>
            )}

            {overview && (
              <>
                <div className="bg-dark-800 rounded-lg border border-dark-500 p-5 min-h-[140px]">
                  <div className="flex items-center gap-2.5">
                    <div className="w-9 h-9 rounded-lg bg-warn-500/10 flex items-center justify-center">
                      <svg className="w-5 h-5 text-warn-400" fill="none" viewBox="0 0 24 24" stroke="currentColor" strokeWidth={1.5}>
                        <path strokeLinecap="round" strokeLinejoin="round" d="M15.362 5.214A8.252 8.252 0 0 1 12 21 8.25 8.25 0 0 1 6.038 7.047 8.287 8.287 0 0 0 9 9.601a8.983 8.983 0 0 1 3.361-6.867 8.21 8.21 0 0 0 3 2.48Z" />
                        <path strokeLinecap="round" strokeLinejoin="round" d="M12 18a3.75 3.75 0 0 0 .495-7.468 5.99 5.99 0 0 0-1.925 3.547 5.975 5.975 0 0 1-2.133-1.001A3.75 3.75 0 0 0 12 18Z" />
                      </svg>
                    </div>
                    <p className="text-xs font-medium text-dark-300 uppercase font-mono tracking-wider">Firewall</p>
                  </div>
                  <div className="flex items-center gap-2 mt-2">
                    <div className={`w-3 h-3 rounded-full ${overview.firewall_active ? "bg-rust-500" : "bg-danger-500"}`} />
                    <span className="text-lg font-bold text-dark-50">
                      {overview.firewall_active ? "Active" : "Inactive"}
                    </span>
                  </div>
                  <p className="text-xs text-dark-300 mt-1">{overview.firewall_rules_count} rules</p>
                </div>

                <div className="bg-dark-800 rounded-lg border border-dark-500 p-5 min-h-[140px]">
                  <div className="flex items-center gap-2.5">
                    <div className="w-9 h-9 rounded-lg bg-danger-500/10 flex items-center justify-center">
                      <svg className="w-5 h-5 text-danger-400" fill="none" viewBox="0 0 24 24" stroke="currentColor" strokeWidth={1.5}>
                        <path strokeLinecap="round" strokeLinejoin="round" d="M18.364 18.364A9 9 0 0 0 5.636 5.636m12.728 12.728A9 9 0 0 1 5.636 5.636m12.728 12.728L5.636 5.636" />
                      </svg>
                    </div>
                    <p className="text-xs font-medium text-dark-300 uppercase font-mono tracking-wider">Fail2Ban</p>
                  </div>
                  <div className="flex items-center gap-2 mt-2">
                    <div className={`w-3 h-3 rounded-full ${overview.fail2ban_running ? "bg-rust-500" : "bg-dark-400"}`} />
                    <span className="text-lg font-bold text-dark-50">
                      {overview.fail2ban_running ? "Running" : "Stopped"}
                    </span>
                  </div>
                  <p className="text-xs text-dark-300 mt-1"><span className="font-mono">{overview.fail2ban_banned_total}</span> banned IPs</p>
                </div>

                <div className="bg-dark-800 rounded-lg border border-dark-500 p-5 min-h-[140px]">
                  <div className="flex items-center gap-2.5">
                    <div className="w-9 h-9 rounded-lg bg-rust-500/10 flex items-center justify-center">
                      <svg className="w-5 h-5 text-rust-400" fill="none" viewBox="0 0 24 24" stroke="currentColor" strokeWidth={1.5}>
                        <path strokeLinecap="round" strokeLinejoin="round" d="M15.75 5.25a3 3 0 0 1 3 3m3 0a6 6 0 0 1-7.029 5.912c-.563-.097-1.159.026-1.563.43L10.5 17.25H8.25v2.25H6v2.25H2.25v-2.818c0-.597.237-1.17.659-1.591l6.499-6.499c.404-.404.527-1 .43-1.563A6 6 0 1 1 21.75 8.25Z" />
                      </svg>
                    </div>
                    <p className="text-xs font-medium text-dark-300 uppercase font-mono tracking-wider">SSH</p>
                  </div>
                  <p className="text-lg font-bold text-dark-50 mt-2">Port <span className="font-mono">{overview.ssh_port}</span></p>
                  <p className="text-xs mt-1">
                    <span className={overview.ssh_password_auth ? "text-warn-400" : "text-rust-400"}>
                      Password auth: {overview.ssh_password_auth ? "On" : "Off"}
                    </span>
                  </p>
                  <p className="text-xs mt-0.5">
                    <span className={overview.ssh_root_login ? "text-warn-400" : "text-rust-400"}>
                      Root login: {overview.ssh_root_login ? "On" : "Off"}
                    </span>
                  </p>
                  <div className="flex flex-wrap gap-1.5 mt-3">
                    <button
                      onClick={() => setPendingConfirm({
                        type: "ssh_password",
                        label: overview.ssh_password_auth ? "Disable SSH password authentication? Make sure you have SSH key access first!" : "Enable SSH password authentication?",
                        data: { enabled: overview.ssh_password_auth }
                      })}
                      className="px-3 py-1.5 bg-dark-700 text-dark-200 hover:bg-dark-600 border border-dark-600 rounded text-xs font-medium transition-colors"
                    >
                      {overview.ssh_password_auth ? "Disable Password" : "Enable Password"}
                    </button>
                    {overview.ssh_root_login && (
                      <button
                        onClick={() => setPendingConfirm({
                          type: "ssh_root",
                          label: "Disable root SSH login? Make sure you have a non-root user with sudo access!"
                        })}
                        className="px-3 py-1.5 bg-dark-700 text-dark-200 hover:bg-dark-600 border border-dark-600 rounded text-xs font-medium transition-colors"
                      >
                        Disable Root
                      </button>
                    )}
                    <button
                      onClick={async () => {
                        const newPort = prompt("Enter new SSH port (1-65535):", String(overview.ssh_port));
                        if (!newPort) return;
                        const port = parseInt(newPort);
                        if (isNaN(port) || port < 1 || port > 65535) { setMessage({ text: "Invalid port number", type: "error" }); return; }
                        setPendingConfirm({
                          type: "ssh_port",
                          label: `Change SSH port to ${port}? A firewall rule will be added automatically.`,
                          data: { port }
                        });
                      }}
                      className="px-3 py-1.5 bg-dark-700 text-dark-200 hover:bg-dark-600 border border-dark-600 rounded text-xs font-medium transition-colors"
                    >
                      Change Port
                    </button>
                  </div>
                </div>

                <div className="bg-dark-800 rounded-lg border border-dark-500 p-5 min-h-[140px]">
                  <div className="flex items-center gap-2.5">
                    <div className="w-9 h-9 rounded-lg bg-warn-500/10 flex items-center justify-center">
                      <svg className="w-5 h-5 text-warn-400" fill="none" viewBox="0 0 24 24" stroke="currentColor" strokeWidth={1.5}>
                        <path strokeLinecap="round" strokeLinejoin="round" d="M16.5 10.5V6.75a4.5 4.5 0 1 0-9 0v3.75m-.75 11.25h10.5a2.25 2.25 0 0 0 2.25-2.25v-6.75a2.25 2.25 0 0 0-2.25-2.25H6.75a2.25 2.25 0 0 0-2.25 2.25v6.75a2.25 2.25 0 0 0 2.25 2.25Z" />
                      </svg>
                    </div>
                    <p className="text-xs font-medium text-dark-300 uppercase font-mono tracking-wider">SSL Certs</p>
                  </div>
                  <p className="text-3xl font-bold text-dark-50 mt-2">{overview.ssl_certs_count}</p>
                  <p className="text-xs text-dark-300 mt-1">Active certificates</p>
                </div>

                <div className="bg-dark-800 rounded-lg border border-dark-500 p-5 min-h-[140px]">
                  <div className="flex items-center gap-2.5">
                    <div className="w-9 h-9 rounded-lg bg-accent-500/10 flex items-center justify-center">
                      <svg className="w-5 h-5 text-accent-400" fill="none" viewBox="0 0 24 24" stroke="currentColor" strokeWidth={1.5}>
                        <path strokeLinecap="round" strokeLinejoin="round" d="M9 12.75L11.25 15 15 9.75m-3-7.036A11.959 11.959 0 013.598 6 11.99 11.99 0 003 9.749c0 5.592 3.824 10.29 9 11.623 5.176-1.332 9-6.03 9-11.622 0-1.31-.21-2.571-.598-3.751h-.152c-3.196 0-6.1-1.248-8.25-3.285z" />
                      </svg>
                    </div>
                    <p className="text-xs font-medium text-dark-300 uppercase font-mono tracking-wider">Panel Protection</p>
                  </div>
                  <div className="flex items-center gap-2 mt-2">
                    <div className={`w-3 h-3 rounded-full ${panelJail ? "bg-rust-500" : "bg-dark-400"}`} />
                    <span className="text-sm text-dark-50">{panelJail ? "Active" : "Not configured"}</span>
                  </div>
                  {!panelJail && (
                    <button
                      onClick={async () => {
                        try {
                          await api.post("/security/panel-jail/setup");
                          setPanelJail(true);
                          setMessage({ text: "Panel Fail2Ban jail activated", type: "success" });
                        } catch (e) { setMessage({ text: e instanceof Error ? e.message : "Failed", type: "error" }); }
                      }}
                      className="mt-3 px-3 py-1.5 bg-dark-700 text-dark-200 hover:bg-dark-600 border border-dark-600 rounded text-xs font-medium transition-colors"
                    >
                      Enable Protection
                    </button>
                  )}
                  {panelJail && <p className="text-xs text-dark-300 mt-1">Bans IPs after 5 failed logins</p>}
                </div>
              </>
            )}
          </div>

          {/* Latest scan findings */}
          {posture?.latest_scan && posture.latest_scan.findings_count > 0 && (
            <div className="bg-dark-800 rounded-lg border border-dark-500 overflow-hidden mb-6">
              <div className="px-5 py-3 border-b border-dark-600 flex items-center justify-between">
                <h3 className="text-xs font-medium text-dark-300 uppercase font-mono tracking-widest">Latest Scan Findings</h3>
                <span className="text-xs text-dark-300 font-mono">
                  {new Date(posture.latest_scan.completed_at || posture.latest_scan.started_at).toLocaleString()}
                </span>
              </div>
              <div className="px-5 py-3 flex gap-4 text-sm border-b border-dark-600">
                {posture.latest_scan.critical_count > 0 && (
                  <span className="text-danger-400 font-medium">{posture.latest_scan.critical_count} critical</span>
                )}
                {posture.latest_scan.warning_count > 0 && (
                  <span className="text-warn-500 font-medium">{posture.latest_scan.warning_count} warning</span>
                )}
                {posture.latest_scan.info_count > 0 && (
                  <span className="text-accent-400 font-medium">{posture.latest_scan.info_count} info</span>
                )}
              </div>
              <div className="p-3">
                <button
                  onClick={() => { setTab("scans"); handleViewScan(posture.latest_scan!.id); }}
                  className="text-sm text-rust-400 hover:text-rust-300 font-medium"
                >
                  View details
                </button>
              </div>
            </div>
          )}

          <div className="grid grid-cols-1 lg:grid-cols-2 gap-6">
            {/* Firewall Rules */}
            <div className="bg-dark-800 rounded-lg border border-dark-500 overflow-x-auto">
              <div className="px-5 py-3 border-b border-dark-600 flex items-center justify-between">
                <h3 className="text-xs font-medium text-dark-300 uppercase font-mono tracking-widest">Firewall Rules</h3>
                <button
                  onClick={() => setShowAddRule(true)}
                  className="px-3 py-1 bg-rust-500 text-white rounded-md text-xs font-medium hover:bg-rust-600"
                >
                  Add Rule
                </button>
              </div>
              {firewall && firewall.rules.length > 0 ? (
                <table className="w-full">
                  <thead>
                    <tr className="bg-dark-900">
                      <th scope="col" className="text-left text-xs font-medium text-dark-200 uppercase font-mono tracking-widest px-5 py-2">#</th>
                      <th scope="col" className="text-left text-xs font-medium text-dark-200 uppercase font-mono tracking-widest px-5 py-2">To</th>
                      <th scope="col" className="text-left text-xs font-medium text-dark-200 uppercase font-mono tracking-widest px-5 py-2">Action</th>
                      <th scope="col" className="text-left text-xs font-medium text-dark-200 uppercase font-mono tracking-widest px-5 py-2">From</th>
                      <th scope="col" className="text-right text-xs font-medium text-dark-200 uppercase font-mono tracking-widest px-5 py-2 w-16"></th>
                    </tr>
                  </thead>
                  <tbody className="divide-y divide-dark-600">
                    {firewall.rules.map((rule) => (
                      <tr key={rule.number} className="table-row-hover">
                        <td className="px-5 py-2.5 text-sm text-dark-300 font-mono">{rule.number}</td>
                        <td className="px-5 py-2.5 text-sm text-dark-50 font-mono">{rule.to}</td>
                        <td className="px-5 py-2.5">
                          <span className={`text-xs font-medium ${rule.action.toLowerCase().includes("allow") ? "text-rust-400" : "text-danger-400"}`}>
                            {rule.action}
                          </span>
                        </td>
                        <td className="px-5 py-2.5 text-sm text-dark-200 font-mono">{rule.from}</td>
                        <td className="px-5 py-2.5 text-right">
                          {deleteTarget === rule.number ? (
                            <div className="flex gap-1 justify-end">
                              <button onClick={() => handleDeleteRule(rule.number)} className="px-1.5 py-0.5 bg-danger-500 text-white rounded text-[10px]">Del</button>
                              <button onClick={() => setDeleteTarget(null)} className="px-1.5 py-0.5 bg-dark-600 text-dark-200 rounded text-[10px]">No</button>
                            </div>
                          ) : (
                            <button onClick={() => setDeleteTarget(rule.number)} className="text-dark-300 hover:text-danger-500" aria-label="Delete rule">
                              <svg className="w-3.5 h-3.5" fill="none" viewBox="0 0 24 24" stroke="currentColor" strokeWidth={2}>
                                <path strokeLinecap="round" strokeLinejoin="round" d="M6 18L18 6M6 6l12 12" />
                              </svg>
                            </button>
                          )}
                        </td>
                      </tr>
                    ))}
                  </tbody>
                </table>
              ) : (
                <div className="p-6 text-center text-sm text-dark-300">
                  {firewall ? "No firewall rules" : "Could not load firewall status"}
                </div>
              )}
            </div>

            {/* Fail2Ban Jails */}
            <div className="bg-dark-800 rounded-lg border border-dark-500 overflow-x-auto">
              <div className="px-5 py-3 border-b border-dark-600">
                <h3 className="text-xs font-medium text-dark-300 uppercase font-mono tracking-widest">Fail2Ban Jails</h3>
              </div>
              {fail2ban && fail2ban.jails.length > 0 ? (
                <>
                  <table className="w-full">
                    <thead>
                      <tr className="bg-dark-900">
                        <th scope="col" className="text-left text-xs font-medium text-dark-200 uppercase font-mono tracking-widest px-5 py-2">Jail</th>
                        <th scope="col" className="text-right text-xs font-medium text-dark-200 uppercase font-mono tracking-widest px-5 py-2">Banned</th>
                      </tr>
                    </thead>
                    <tbody className="divide-y divide-dark-600">
                      {fail2ban.jails.map((jail) => (
                        <tr
                          key={jail.name}
                          className="table-row-hover cursor-pointer"
                          onClick={() => { if (selectedJail === jail.name) { setSelectedJail(null); } else { loadBannedIps(jail.name); if (!banJail) setBanJail(jail.name); } }}
                        >
                          <td className="px-5 py-2.5 text-sm text-dark-50 font-mono flex items-center gap-2">
                            <svg className={`w-3 h-3 text-dark-300 transition-transform ${selectedJail === jail.name ? "rotate-90" : ""}`} fill="none" viewBox="0 0 24 24" stroke="currentColor" strokeWidth={2}>
                              <path strokeLinecap="round" strokeLinejoin="round" d="M8.25 4.5l7.5 7.5-7.5 7.5" />
                            </svg>
                            {jail.name}
                          </td>
                          <td className="px-5 py-2.5 text-sm text-right">
                            <span className={`font-medium ${jail.banned_count > 0 ? "text-danger-400" : "text-dark-300"}`}>
                              {jail.banned_count}
                            </span>
                          </td>
                        </tr>
                      ))}
                    </tbody>
                  </table>

                  {/* Banned IPs for selected jail */}
                  {selectedJail && (
                    <div className="border-t border-dark-600 px-5 py-3">
                      <p className="text-xs font-medium text-dark-300 uppercase font-mono tracking-widest mb-2">
                        Banned IPs in <span className="text-dark-100">{selectedJail}</span>
                      </p>
                      {bannedIps.length > 0 ? (
                        <div className="space-y-1">
                          {bannedIps.map((ip) => (
                            <div key={ip} className="flex items-center justify-between bg-dark-900 rounded px-3 py-1.5">
                              <span className="text-sm text-dark-50 font-mono">{ip}</span>
                              <button
                                onClick={(e) => {
                                  e.stopPropagation();
                                  setPendingConfirm({
                                    type: "unban",
                                    label: `Unban ${ip} from ${selectedJail}?`,
                                    data: { jail: selectedJail, ip }
                                  });
                                }}
                                className="px-2 py-0.5 bg-rust-500/15 text-rust-400 rounded text-xs font-medium hover:bg-rust-500/25 transition-colors"
                              >
                                Unban
                              </button>
                            </div>
                          ))}
                        </div>
                      ) : (
                        <p className="text-xs text-dark-300">No banned IPs</p>
                      )}
                    </div>
                  )}

                  {/* Ban IP form */}
                  <div className="border-t border-dark-600 px-5 py-3">
                    <p className="text-xs font-medium text-dark-300 uppercase font-mono tracking-widest mb-2">Ban IP</p>
                    <div className="flex gap-2">
                      <select
                        value={banJail}
                        onChange={(e) => setBanJail(e.target.value)}
                        className="px-2 py-1.5 border border-dark-500 rounded text-xs bg-dark-900 text-dark-100"
                      >
                        <option value="">Select jail</option>
                        {fail2ban.jails.map((j) => (
                          <option key={j.name} value={j.name}>{j.name}</option>
                        ))}
                      </select>
                      <input
                        type="text"
                        value={banIp}
                        onChange={(e) => setBanIp(e.target.value)}
                        className="flex-1 px-2 py-1.5 border border-dark-500 rounded text-xs bg-dark-900 text-dark-100 font-mono"
                        placeholder="IP address"
                      />
                      <button
                        onClick={() => {
                          if (!banJail || !banIp) return;
                          setPendingConfirm({
                            type: "ban",
                            label: `Ban ${banIp} in jail ${banJail}?`,
                            data: { jail: banJail, ip: banIp }
                          });
                        }}
                        disabled={!banJail || !banIp}
                        className="px-3 py-1.5 bg-danger-500/15 text-danger-400 rounded text-xs font-medium hover:bg-danger-500/25 transition-colors disabled:opacity-50"
                      >
                        Ban
                      </button>
                    </div>
                  </div>
                </>
              ) : (
                <div className="p-6 text-center text-sm text-dark-300">
                  {fail2ban ? "No jails configured" : "Fail2Ban not available"}
                </div>
              )}
            </div>
          </div>
        </>
      )}

      {tab === "scans" && (
        <div className="space-y-4">
          {scans.length === 0 ? (
            <div className="bg-dark-800 rounded-lg border border-dark-500 p-8 text-center">
              <p className="text-dark-300 text-sm">No security scans yet. Click "Run Security Scan" to start.</p>
            </div>
          ) : (
            scans.map((scan) => (
              <div key={scan.id} className="bg-dark-800 rounded-lg border border-dark-500 overflow-hidden">
                <button
                  onClick={() => handleViewScan(scan.id)}
                  className="w-full px-5 py-4 flex items-center justify-between hover:bg-dark-800 transition-colors text-left"
                >
                  <div className="flex items-center gap-4">
                    <div className={`w-10 h-10 rounded-lg flex items-center justify-center ${
                      scan.critical_count > 0 ? "bg-danger-500/15" : scan.warning_count > 0 ? "bg-warn-500/15" : "bg-rust-500/15"
                    }`}>
                      <svg className={`w-5 h-5 ${
                        scan.critical_count > 0 ? "text-danger-400" : scan.warning_count > 0 ? "text-warn-400" : "text-rust-400"
                      }`} fill="none" viewBox="0 0 24 24" stroke="currentColor" strokeWidth={1.5}>
                        <path strokeLinecap="round" strokeLinejoin="round" d="M9 12.75L11.25 15 15 9.75m-3-7.036A11.959 11.959 0 0 1 3.598 6 11.99 11.99 0 0 0 3 9.749c0 5.592 3.824 10.29 9 11.623 5.176-1.332 9-6.03 9-11.622 0-1.31-.21-2.571-.598-3.751h-.152c-3.196 0-6.1-1.248-8.25-3.285Z" />
                      </svg>
                    </div>
                    <div>
                      <p className="text-sm font-medium text-dark-50">
                        {scan.scan_type === "full" ? "Full Security Scan" : scan.scan_type}
                      </p>
                      <p className="text-xs text-dark-300 font-mono">
                        {new Date(scan.started_at).toLocaleString()}
                        {scan.status === "running" && " (running...)"}
                      </p>
                    </div>
                  </div>
                  <div className="flex items-center gap-3">
                    {scan.critical_count > 0 && (
                      <span className="px-2 py-0.5 bg-danger-500/15 text-danger-400 rounded text-xs font-medium">{scan.critical_count} critical</span>
                    )}
                    {scan.warning_count > 0 && (
                      <span className="px-2 py-0.5 bg-warn-500/15 text-warn-400 rounded text-xs font-medium">{scan.warning_count} warning</span>
                    )}
                    {scan.info_count > 0 && (
                      <span className="px-2 py-0.5 bg-accent-500/15 text-accent-400 rounded text-xs font-medium">{scan.info_count} info</span>
                    )}
                    {scan.findings_count === 0 && (
                      <span className="px-2 py-0.5 bg-rust-500/15 text-rust-400 rounded text-xs font-medium">Clean</span>
                    )}
                    <svg className={`w-4 h-4 text-dark-300 transition-transform ${selectedScan === scan.id ? "rotate-180" : ""}`} fill="none" viewBox="0 0 24 24" stroke="currentColor" strokeWidth={2}>
                      <path strokeLinecap="round" strokeLinejoin="round" d="M19.5 8.25l-7.5 7.5-7.5-7.5" />
                    </svg>
                  </div>
                </button>

                {selectedScan === scan.id && (
                  <div className="border-t border-dark-600">
                    {findings.length === 0 ? (
                      <div className="p-6 text-center text-sm text-dark-300">No findings — all checks passed</div>
                    ) : (
                      <div className="divide-y divide-dark-600">
                        {findings.map((f) => (
                          <div key={f.id} className="px-5 py-3">
                            <div className="flex items-start gap-3">
                              <span className={`mt-0.5 px-2 py-0.5 rounded text-[10px] font-semibold uppercase border ${severityBadge(f.severity)}`}>
                                {f.severity}
                              </span>
                              <div className="flex-1 min-w-0">
                                <p className="text-sm font-medium text-dark-50">{f.title}</p>
                                {f.description && (
                                  <p className="text-xs text-dark-200 mt-0.5">{f.description}</p>
                                )}
                                <div className="flex items-center gap-3 mt-1.5">
                                  <span className={`px-1.5 py-0.5 rounded text-[10px] font-medium font-mono ${checkTypeBadge(f.check_type)}`}>
                                    {f.check_type.replace("_", " ")}
                                  </span>
                                  {f.file_path && (
                                    <span className="text-[11px] text-dark-300 font-mono truncate">{f.file_path}</span>
                                  )}
                                </div>
                                {f.remediation && (
                                  <p className="text-xs text-rust-500 mt-1 inline-flex items-center gap-1">
                                    {f.remediation}
                                    {(() => {
                                      // Malware findings get Quarantine + Delete buttons
                                      if (f.check_type === "malware" && f.file_path) {
                                        return (
                                          <span className="flex gap-1 ml-2">
                                            <button
                                              onClick={() => setPendingConfirm({
                                                type: "quarantine",
                                                label: `Quarantine ${f.file_path}? (moves to /var/lib/dockpanel/quarantine/)`,
                                                data: { path: f.file_path }
                                              })}
                                              className="px-2 py-0.5 bg-warn-500/15 text-warn-400 rounded text-xs font-medium hover:bg-warn-500/25"
                                            >
                                              Quarantine
                                            </button>
                                            <button
                                              onClick={() => setPendingConfirm({
                                                type: "delete_file",
                                                label: `DELETE ${f.file_path}? This cannot be undone!`,
                                                data: { path: f.file_path }
                                              })}
                                              className="px-2 py-0.5 bg-danger-500/15 text-danger-400 rounded text-xs font-medium hover:bg-danger-500/25"
                                            >
                                              Delete
                                            </button>
                                          </span>
                                        );
                                      }
                                      const fix = getFixAction(f);
                                      if (!fix) return null;
                                      return (
                                        <button
                                          onClick={() => setPendingConfirm({
                                            type: "apply_fix",
                                            label: `Apply fix: ${fix.label}? This will ${fix.type === "block_port" ? `block port ${fix.target}/tcp` : fix.label.toLowerCase()}`,
                                            data: { fix_type: fix.type, target: fix.target, fix_label: fix.label }
                                          })}
                                          className="ml-2 px-2 py-0.5 bg-rust-500/15 text-rust-400 rounded text-xs font-medium hover:bg-rust-500/25 transition-colors"
                                        >
                                          Fix
                                        </button>
                                      );
                                    })()}
                                  </p>
                                )}
                              </div>
                            </div>
                          </div>
                        ))}
                      </div>
                    )}
                  </div>
                )}
              </div>
            ))
          )}
        </div>
      )}

      {tab === "diagnostics" && (
        <DiagnosticsContent />
      )}

      {tab === "audit" && (
        <div className="space-y-6">
          {/* SSH Logins */}
          <div className="bg-dark-800 rounded-lg border border-dark-500 overflow-hidden">
            <div className="px-5 py-3 border-b border-dark-600">
              <h3 className="text-xs font-medium text-dark-300 uppercase font-mono tracking-widest">SSH Login Attempts</h3>
            </div>
            <div className="overflow-x-auto">
              <table className="w-full">
                <thead><tr className="bg-dark-900">
                  <th className="text-left text-xs font-medium text-dark-200 uppercase font-mono tracking-widest px-5 py-2">Time</th>
                  <th className="text-left text-xs font-medium text-dark-200 uppercase font-mono tracking-widest px-5 py-2">User</th>
                  <th className="text-left text-xs font-medium text-dark-200 uppercase font-mono tracking-widest px-5 py-2">IP</th>
                  <th className="text-left text-xs font-medium text-dark-200 uppercase font-mono tracking-widest px-5 py-2">Method</th>
                  <th className="text-left text-xs font-medium text-dark-200 uppercase font-mono tracking-widest px-5 py-2">Status</th>
                </tr></thead>
                <tbody className="divide-y divide-dark-600">
                  {loginAudit.ssh.map((e: any, i: number) => (
                    <tr key={i} className="table-row-hover">
                      <td className="px-5 py-2 text-xs text-dark-200 font-mono">{e.time}</td>
                      <td className="px-5 py-2 text-sm text-dark-50 font-mono">{e.user}</td>
                      <td className="px-5 py-2 text-sm text-dark-100 font-mono">{e.ip}</td>
                      <td className="px-5 py-2 text-xs text-dark-300">{e.method}</td>
                      <td className="px-5 py-2">
                        <span className={`px-2 py-0.5 rounded-full text-xs font-medium ${e.success ? "bg-rust-500/15 text-rust-400" : "bg-danger-500/15 text-danger-400"}`}>
                          {e.success ? "Success" : "Failed"}
                        </span>
                      </td>
                    </tr>
                  ))}
                </tbody>
              </table>
              {loginAudit.ssh.length === 0 && <p className="px-5 py-4 text-sm text-dark-300">No SSH login attempts found</p>}
            </div>
          </div>

          {/* Panel Logins */}
          <div className="bg-dark-800 rounded-lg border border-dark-500 overflow-hidden">
            <div className="px-5 py-3 border-b border-dark-600">
              <h3 className="text-xs font-medium text-dark-300 uppercase font-mono tracking-widest">Panel Login Activity</h3>
            </div>
            <div className="overflow-x-auto">
              <table className="w-full">
                <thead><tr className="bg-dark-900">
                  <th className="text-left text-xs font-medium text-dark-200 uppercase font-mono tracking-widest px-5 py-2">Time</th>
                  <th className="text-left text-xs font-medium text-dark-200 uppercase font-mono tracking-widest px-5 py-2">Action</th>
                  <th className="text-left text-xs font-medium text-dark-200 uppercase font-mono tracking-widest px-5 py-2">Status</th>
                </tr></thead>
                <tbody className="divide-y divide-dark-600">
                  {loginAudit.panel.map((e: any, i: number) => (
                    <tr key={i} className="table-row-hover">
                      <td className="px-5 py-2 text-xs text-dark-200 font-mono">{new Date(e.time).toLocaleString()}</td>
                      <td className="px-5 py-2 text-sm text-dark-50">{e.action}</td>
                      <td className="px-5 py-2">
                        <span className={`px-2 py-0.5 rounded-full text-xs font-medium ${e.success ? "bg-rust-500/15 text-rust-400" : "bg-danger-500/15 text-danger-400"}`}>
                          {e.success ? "Success" : "Failed"}
                        </span>
                      </td>
                    </tr>
                  ))}
                </tbody>
              </table>
              {loginAudit.panel.length === 0 && <p className="px-5 py-4 text-sm text-dark-300">No panel login activity found</p>}
            </div>
          </div>
        </div>
      )}

      {/* Add Rule Dialog */}
      {showAddRule && (() => {
        const portInfo: Record<string, { service: string; safety: "safe" | "caution" | "blocked" }> = {
          "22": { service: "SSH", safety: "safe" }, "80": { service: "HTTP", safety: "safe" }, "443": { service: "HTTPS", safety: "safe" },
          "8080": { service: "HTTP Alt", safety: "safe" }, "8443": { service: "HTTPS Alt", safety: "safe" },
          "3000": { service: "Web App", safety: "safe" }, "3001": { service: "Web App", safety: "safe" },
          "5000": { service: "Web App", safety: "safe" }, "8000": { service: "Web App", safety: "safe" },
          "25": { service: "SMTP", safety: "caution" }, "587": { service: "SMTP Submission", safety: "caution" },
          "465": { service: "SMTPS", safety: "caution" }, "993": { service: "IMAPS", safety: "caution" },
          "995": { service: "POP3S", safety: "caution" }, "110": { service: "POP3", safety: "caution" },
          "143": { service: "IMAP", safety: "caution" }, "21": { service: "FTP", safety: "caution" },
          "3306": { service: "MySQL", safety: "caution" }, "5432": { service: "PostgreSQL", safety: "caution" },
          "6379": { service: "Redis", safety: "caution" }, "27017": { service: "MongoDB", safety: "caution" },
          "23": { service: "Telnet", safety: "blocked" }, "135": { service: "RPC", safety: "blocked" },
          "136": { service: "NetBIOS", safety: "blocked" }, "137": { service: "NetBIOS", safety: "blocked" },
          "138": { service: "NetBIOS", safety: "blocked" }, "139": { service: "NetBIOS", safety: "blocked" },
          "445": { service: "SMB", safety: "blocked" }, "1433": { service: "MSSQL", safety: "blocked" },
        };

        const info = rulePort ? portInfo[rulePort] : null;
        const isBlocked = info?.safety === "blocked";

        const presets = [
          { label: "Web", ports: "80, 443, 8080", action: () => { setRulePort("80"); } },
          { label: "Mail", ports: "25, 587, 993", action: () => { setRulePort("587"); } },
          { label: "Database", ports: "3306 or 5432", action: () => { setRulePort("3306"); } },
        ];

        return (
        <div className="fixed inset-0 bg-black/30 flex items-center justify-center z-50 dp-modal-overlay" onClick={() => setShowAddRule(false)}>
          <div className="bg-dark-800 border border-dark-500 shadow-xl p-6 w-full max-w-md dp-modal" onClick={(e) => e.stopPropagation()} role="dialog" aria-labelledby="add-rule-title">
            <h3 id="add-rule-title" className="text-xs font-medium text-dark-300 uppercase font-mono tracking-widest mb-4">Open Port</h3>

            {/* Quick presets */}
            <div className="flex gap-2 mb-4">
              {presets.map((p) => (
                <button key={p.label} onClick={p.action} className="px-3 py-1.5 bg-dark-700 border border-dark-500 text-xs text-dark-100 hover:bg-dark-600 transition-colors">
                  {p.label} <span className="text-dark-300 ml-1">{p.ports}</span>
                </button>
              ))}
            </div>

            <div className="space-y-3">
              <div>
                <label htmlFor="rule-port" className="block text-xs font-medium text-dark-100 mb-1">Port Number</label>
                <input
                  id="rule-port"
                  type="number"
                  value={rulePort}
                  onChange={(e) => setRulePort(e.target.value)}
                  className="w-full px-3 py-2 border border-dark-500 rounded-lg text-sm focus:ring-2 focus:ring-accent-500 outline-none"
                  placeholder="e.g. 8080"
                  autoFocus
                />
                {/* Port info badge */}
                {rulePort && (
                  <div className={`mt-2 flex items-center gap-2 text-xs ${isBlocked ? "text-danger-400" : info?.safety === "caution" ? "text-warn-400" : "text-rust-400"}`}>
                    <span className={`w-2 h-2 rounded-full ${isBlocked ? "bg-danger-400" : info?.safety === "caution" ? "bg-warn-400" : "bg-rust-400"}`} />
                    {info ? (
                      <span>{info.service} — {isBlocked ? "Blocked (security risk)" : info.safety === "caution" ? "Use with caution (restrict source IP if possible)" : "Safe to open"}</span>
                    ) : (
                      <span>Custom port — safe to open</span>
                    )}
                  </div>
                )}
              </div>

              <div className="grid grid-cols-2 gap-3">
                <div>
                  <label htmlFor="rule-protocol" className="block text-xs font-medium text-dark-100 mb-1">Protocol</label>
                  <select id="rule-protocol" value={ruleProto} onChange={(e) => setRuleProto(e.target.value)} className="w-full px-3 py-2 border border-dark-500 rounded-lg text-sm">
                    <option value="tcp">TCP</option>
                    <option value="udp">UDP</option>
                    <option value="tcp/udp">TCP/UDP</option>
                  </select>
                </div>
                <div>
                  <label htmlFor="rule-action" className="block text-xs font-medium text-dark-100 mb-1">Action</label>
                  <select id="rule-action" value={ruleAction} onChange={(e) => setRuleAction(e.target.value)} className="w-full px-3 py-2 border border-dark-500 rounded-lg text-sm">
                    <option value="allow">Allow</option>
                    <option value="deny">Deny</option>
                  </select>
                </div>
              </div>

              <div>
                <label htmlFor="rule-from" className="block text-xs font-medium text-dark-100 mb-1">From IP <span className="text-dark-300">(optional — leave empty for any)</span></label>
                <input id="rule-from" type="text" value={ruleFrom} onChange={(e) => setRuleFrom(e.target.value)} className="w-full px-3 py-2 border border-dark-500 rounded-lg text-sm focus:ring-2 focus:ring-accent-500 outline-none" placeholder="Any" />
              </div>
            </div>

            <div className="flex justify-end gap-2 mt-5">
              <button onClick={() => setShowAddRule(false)} className="px-4 py-2 text-dark-300 border border-dark-600 rounded-lg text-sm font-medium hover:text-dark-100 hover:border-dark-400">Cancel</button>
              <button
                onClick={handleAddRule}
                disabled={!rulePort || isBlocked}
                className="px-4 py-2 bg-rust-500 text-white rounded-lg text-sm font-medium hover:bg-rust-600 disabled:opacity-50"
              >
                {isBlocked ? "Port Blocked" : "Open Port"}
              </button>
            </div>
          </div>
        </div>
        );
      })()}
      {/* Lockdown Tab (consolidated from SecurityHardening) */}
      {tab === "lockdown" && (
        <div className="space-y-4">
          <div className={`bg-dark-800 rounded-lg border p-6 ${lockdown?.active ? "border-red-500/50" : "border-dark-500"}`}>
            <div className="flex items-center justify-between mb-4">
              <div>
                <h3 className="text-dark-50 font-medium">System Lockdown</h3>
                <p className="text-sm text-dark-400 mt-1">
                  {lockdown?.active
                    ? `Active since ${lockdown.triggered_at ? new Date(lockdown.triggered_at).toLocaleString() : "unknown"}`
                    : "System is operating normally"}
                </p>
                {lockdown?.reason && <p className="text-sm text-yellow-400 mt-1">{lockdown.reason}</p>}
              </div>
              <div className="flex gap-2">
                {lockdown?.active ? (
                  <button onClick={async () => { try { await api.post("/security/lockdown/deactivate", {}); setMessage({ text: "Lockdown deactivated", type: "success" }); loadData(); } catch (e: any) { setMessage({ text: e.message, type: "error" }); }}}
                    className="px-4 py-2 text-sm font-mono bg-green-600 hover:bg-green-700 text-white rounded-lg">Unlock System</button>
                ) : (
                  <button onClick={() => setPendingConfirm({ type: "lockdown", label: "Activate lockdown? All non-admin access will be blocked." })}
                    className="px-4 py-2 text-sm font-mono bg-yellow-600 hover:bg-yellow-700 text-white rounded-lg">Activate Lockdown</button>
                )}
                <button onClick={() => setPendingConfirm({ type: "panic", label: "EMERGENCY: Kill all terminals, block non-admins, disable registration?" })}
                  className="px-3 py-2 text-sm font-mono bg-red-600 hover:bg-red-700 text-white rounded-lg">Panic Button</button>
                <button onClick={async () => { try { const r = await api.post<{ snapshot_dir: string }>("/security/forensic-snapshot", {}); setMessage({ text: `Snapshot saved: ${r.snapshot_dir}`, type: "success" }); } catch (e: any) { setMessage({ text: e.message, type: "error" }); }}}
                  className="px-3 py-2 text-sm font-mono bg-dark-700 hover:bg-dark-600 text-dark-200 rounded-lg border border-dark-500">Forensic Snapshot</button>
              </div>
            </div>
            <div className="text-xs text-dark-500 space-y-1">
              <p>When locked: terminals disabled, registration blocked, non-admin logins blocked.</p>
              <p>Auto-expires after 24 hours. Panic button also activates lockdown.</p>
            </div>
          </div>

          {/* Immutable Audit Log */}
          <div className="bg-dark-800 rounded-lg border border-dark-500 overflow-hidden">
            <div className="px-5 py-3 border-b border-dark-600">
              <h3 className="text-xs font-medium text-dark-300 uppercase font-mono tracking-widest">Immutable Security Audit Log</h3>
            </div>
            <div className="overflow-x-auto">
              <table className="w-full text-sm">
                <thead><tr className="border-b border-dark-600 text-left text-xs font-mono text-dark-400 uppercase">
                  <th className="px-4 py-2">Severity</th><th className="px-4 py-2">Event</th><th className="px-4 py-2">Actor</th><th className="px-4 py-2">IP</th><th className="px-4 py-2">Location</th><th className="px-4 py-2">Time</th>
                </tr></thead>
                <tbody>
                  {auditLog.map((e: any) => (
                    <tr key={e.id} className="border-b border-dark-700 hover:bg-dark-750">
                      <td className="px-4 py-2"><span className={`px-2 py-0.5 rounded text-[10px] font-mono uppercase ${e.severity === "critical" ? "text-red-400 bg-red-500/10" : e.severity === "warning" ? "text-yellow-400 bg-yellow-500/10" : "text-blue-400 bg-blue-500/10"}`}>{e.severity}</span></td>
                      <td className="px-4 py-2 font-mono text-dark-200">{e.event_type}</td>
                      <td className="px-4 py-2 text-dark-300">{e.actor_email || "-"}</td>
                      <td className="px-4 py-2 text-dark-400 font-mono text-xs">{e.actor_ip || "-"}</td>
                      <td className="px-4 py-2 text-dark-400 text-xs">{e.geo_country || "-"}</td>
                      <td className="px-4 py-2 text-dark-500 text-xs whitespace-nowrap">{new Date(e.created_at).toLocaleString()}</td>
                    </tr>
                  ))}
                  {auditLog.length === 0 && <tr><td colSpan={6} className="px-4 py-8 text-center text-dark-500">No audit events yet</td></tr>}
                </tbody>
              </table>
            </div>
          </div>
        </div>
      )}

      {/* Recordings Tab */}
      {tab === "recordings" && (
        <div className="bg-dark-800 rounded-lg border border-dark-500 overflow-hidden">
          <div className="px-5 py-3 border-b border-dark-600">
            <h3 className="text-xs font-medium text-dark-300 uppercase font-mono tracking-widest">Terminal Session Recordings</h3>
            <p className="text-xs text-dark-200 mt-0.5">Asciicast v2 recordings of all terminal sessions</p>
          </div>
          <table className="w-full text-sm">
            <thead><tr className="border-b border-dark-600 text-left text-xs font-mono text-dark-400 uppercase">
              <th className="px-4 py-2">Filename</th><th className="px-4 py-2">Size</th><th className="px-4 py-2">Created</th>
            </tr></thead>
            <tbody>
              {recordings.map((r: any, i: number) => (
                <tr key={i} className="border-b border-dark-700 hover:bg-dark-750">
                  <td className="px-4 py-2 font-mono text-dark-200">{r.filename}</td>
                  <td className="px-4 py-2 text-dark-400">{(r.size_bytes / 1024).toFixed(1)} KB</td>
                  <td className="px-4 py-2 text-dark-500 text-xs">{r.created || "-"}</td>
                </tr>
              ))}
              {recordings.length === 0 && <tr><td colSpan={3} className="px-4 py-8 text-center text-dark-500">No recordings yet</td></tr>}
            </tbody>
          </table>
        </div>
      )}

      {/* Approvals Tab */}
      {tab === "approvals" && (
        <div className="space-y-4">
          <p className="text-sm text-dark-400">Users awaiting admin approval. Enable approval mode in Settings &rarr; Security.</p>
          <div className="bg-dark-800 rounded-lg border border-dark-500 overflow-hidden">
            <table className="w-full text-sm">
              <thead><tr className="border-b border-dark-600 text-left text-xs font-mono text-dark-400 uppercase">
                <th className="px-4 py-2">Email</th><th className="px-4 py-2">Registered</th><th className="px-4 py-2">Actions</th>
              </tr></thead>
              <tbody>
                {pendingUsers.map((u: any) => (
                  <tr key={u.id} className="border-b border-dark-700">
                    <td className="px-4 py-2 text-dark-200">{u.email}</td>
                    <td className="px-4 py-2 text-dark-400 text-xs">{new Date(u.created_at).toLocaleString()}</td>
                    <td className="px-4 py-2">
                      <button onClick={async () => { try { await api.post(`/security/users/${u.id}/approve`, {}); setMessage({ text: "User approved", type: "success" }); loadData(); } catch (e: any) { setMessage({ text: e.message, type: "error" }); }}}
                        className="px-3 py-1 text-xs font-mono bg-green-600 hover:bg-green-700 text-white rounded">Approve</button>
                    </td>
                  </tr>
                ))}
                {pendingUsers.length === 0 && <tr><td colSpan={3} className="px-4 py-8 text-center text-dark-500">No pending approvals</td></tr>}
              </tbody>
            </table>
          </div>
        </div>
      )}

      </div>
    </div>
  );
}
