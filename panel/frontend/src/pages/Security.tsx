import { useState, useEffect } from "react";
import { Navigate } from "react-router-dom";
import { useAuth } from "../context/AuthContext";
import { api } from "../api";

interface SecurityOverview {
  firewall_active: boolean;
  firewall_rules_count: number;
  fail2ban_running: boolean;
  fail2ban_banned_total: number;
  ssh_port: number;
  ssh_password_auth: boolean;
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
  const [tab, setTab] = useState<"overview" | "scans">("overview");

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
    } finally {
      setLoading(false);
    }
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
    if (score >= 80) return "text-emerald-600";
    if (score >= 50) return "text-amber-600";
    return "text-red-400";
  };

  const scoreBg = (score: number) => {
    if (score >= 80) return "bg-emerald-500";
    if (score >= 50) return "bg-amber-500";
    return "bg-red-500";
  };

  const severityBadge = (severity: string) => {
    switch (severity) {
      case "critical":
        return "bg-red-500/15 text-red-400 border-red-500/20";
      case "warning":
        return "bg-amber-500/15 text-amber-400 border-amber-200";
      default:
        return "bg-blue-500/15 text-blue-400 border-blue-200";
    }
  };

  const checkTypeBadge = (type: string) => {
    switch (type) {
      case "malware":
        return "bg-red-500/10 text-red-400";
      case "file_integrity":
        return "bg-purple-500/10 text-purple-400";
      case "open_port":
        return "bg-amber-500/10 text-amber-400";
      case "ssl_expiry":
        return "bg-orange-500/10 text-orange-400";
      default:
        return "bg-dark-900 text-dark-200";
    }
  };

  if (loading) {
    return (
      <div className="p-6 lg:p-8">
        <h1 className="text-2xl font-bold text-dark-50 mb-6">Security</h1>
        <div className="grid grid-cols-1 md:grid-cols-2 lg:grid-cols-4 gap-4">
          {[...Array(4)].map((_, i) => (
            <div key={i} className="bg-dark-800 rounded-xl border border-dark-500 p-5 animate-pulse">
              <div className="h-4 bg-dark-600 rounded w-20 mb-3" />
              <div className="h-8 bg-dark-600 rounded w-16" />
            </div>
          ))}
        </div>
      </div>
    );
  }

  return (
    <div className="p-6 lg:p-8">
      <div className="flex items-center justify-between mb-6">
        <h1 className="text-2xl font-bold text-dark-50">Security</h1>
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

      {message.text && (
        <div
          className={`mb-4 px-4 py-3 rounded-lg text-sm border ${
            message.type === "success"
              ? "bg-emerald-500/10 text-emerald-400 border-emerald-500/20"
              : "bg-red-500/10 text-red-400 border-red-500/20"
          }`}
        >
          {message.text}
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
      </div>

      {tab === "overview" && (
        <>
          {/* Security Posture + Overview Cards */}
          <div className="grid grid-cols-1 md:grid-cols-2 lg:grid-cols-5 gap-4 mb-6">
            {/* Security Score */}
            {posture && posture.score >= 0 && (
              <div className="bg-dark-800 rounded-xl border border-dark-500 p-5">
                <div className="flex items-center gap-2.5">
                  <div className="w-9 h-9 rounded-lg bg-indigo-500/10 flex items-center justify-center">
                    <svg className="w-5 h-5 text-indigo-400" fill="none" viewBox="0 0 24 24" stroke="currentColor" strokeWidth={1.5}>
                      <path strokeLinecap="round" strokeLinejoin="round" d="M9 12.75 11.25 15 15 9.75m-3-7.036A11.959 11.959 0 0 1 3.598 6 11.99 11.99 0 0 0 3 9.749c0 5.592 3.824 10.29 9 11.623 5.176-1.332 9-6.03 9-11.622 0-1.31-.21-2.571-.598-3.751h-.152c-3.196 0-6.1-1.248-8.25-3.285Z" />
                    </svg>
                  </div>
                  <p className="text-sm font-medium text-dark-200">Security Score</p>
                </div>
                <div className="flex items-end gap-1 mt-2">
                  <span className={`text-3xl font-bold ${scoreColor(posture.score)}`}>{posture.score}</span>
                  <span className="text-sm text-dark-300 mb-1">/100</span>
                </div>
                <div className="mt-2 h-1.5 bg-dark-700 rounded-full overflow-hidden">
                  <div className={`h-full rounded-full ${scoreBg(posture.score)}`} style={{ width: `${posture.score}%` }} />
                </div>
                <p className="text-xs text-dark-300 mt-1">{posture.total_scans} scans total</p>
              </div>
            )}

            {overview && (
              <>
                <div className="bg-dark-800 rounded-xl border border-dark-500 p-5">
                  <div className="flex items-center gap-2.5">
                    <div className="w-9 h-9 rounded-lg bg-orange-500/10 flex items-center justify-center">
                      <svg className="w-5 h-5 text-orange-400" fill="none" viewBox="0 0 24 24" stroke="currentColor" strokeWidth={1.5}>
                        <path strokeLinecap="round" strokeLinejoin="round" d="M15.362 5.214A8.252 8.252 0 0 1 12 21 8.25 8.25 0 0 1 6.038 7.047 8.287 8.287 0 0 0 9 9.601a8.983 8.983 0 0 1 3.361-6.867 8.21 8.21 0 0 0 3 2.48Z" />
                        <path strokeLinecap="round" strokeLinejoin="round" d="M12 18a3.75 3.75 0 0 0 .495-7.468 5.99 5.99 0 0 0-1.925 3.547 5.975 5.975 0 0 1-2.133-1.001A3.75 3.75 0 0 0 12 18Z" />
                      </svg>
                    </div>
                    <p className="text-sm font-medium text-dark-200">Firewall</p>
                  </div>
                  <div className="flex items-center gap-2 mt-2">
                    <div className={`w-3 h-3 rounded-full ${overview.firewall_active ? "bg-emerald-500" : "bg-red-500"}`} />
                    <span className="text-lg font-bold text-dark-50">
                      {overview.firewall_active ? "Active" : "Inactive"}
                    </span>
                  </div>
                  <p className="text-xs text-dark-300 mt-1">{overview.firewall_rules_count} rules</p>
                </div>

                <div className="bg-dark-800 rounded-xl border border-dark-500 p-5">
                  <div className="flex items-center gap-2.5">
                    <div className="w-9 h-9 rounded-lg bg-red-500/10 flex items-center justify-center">
                      <svg className="w-5 h-5 text-red-400" fill="none" viewBox="0 0 24 24" stroke="currentColor" strokeWidth={1.5}>
                        <path strokeLinecap="round" strokeLinejoin="round" d="M18.364 18.364A9 9 0 0 0 5.636 5.636m12.728 12.728A9 9 0 0 1 5.636 5.636m12.728 12.728L5.636 5.636" />
                      </svg>
                    </div>
                    <p className="text-sm font-medium text-dark-200">Fail2Ban</p>
                  </div>
                  <div className="flex items-center gap-2 mt-2">
                    <div className={`w-3 h-3 rounded-full ${overview.fail2ban_running ? "bg-emerald-500" : "bg-gray-300"}`} />
                    <span className="text-lg font-bold text-dark-50">
                      {overview.fail2ban_running ? "Running" : "Stopped"}
                    </span>
                  </div>
                  <p className="text-xs text-dark-300 mt-1">{overview.fail2ban_banned_total} banned IPs</p>
                </div>

                <div className="bg-dark-800 rounded-xl border border-dark-500 p-5">
                  <div className="flex items-center gap-2.5">
                    <div className="w-9 h-9 rounded-lg bg-emerald-500/10 flex items-center justify-center">
                      <svg className="w-5 h-5 text-emerald-400" fill="none" viewBox="0 0 24 24" stroke="currentColor" strokeWidth={1.5}>
                        <path strokeLinecap="round" strokeLinejoin="round" d="M15.75 5.25a3 3 0 0 1 3 3m3 0a6 6 0 0 1-7.029 5.912c-.563-.097-1.159.026-1.563.43L10.5 17.25H8.25v2.25H6v2.25H2.25v-2.818c0-.597.237-1.17.659-1.591l6.499-6.499c.404-.404.527-1 .43-1.563A6 6 0 1 1 21.75 8.25Z" />
                      </svg>
                    </div>
                    <p className="text-sm font-medium text-dark-200">SSH</p>
                  </div>
                  <p className="text-lg font-bold text-dark-50 mt-2">Port {overview.ssh_port}</p>
                  <p className="text-xs mt-1">
                    <span className={overview.ssh_password_auth ? "text-amber-600" : "text-emerald-600"}>
                      Password auth: {overview.ssh_password_auth ? "On" : "Off"}
                    </span>
                  </p>
                </div>

                <div className="bg-dark-800 rounded-xl border border-dark-500 p-5">
                  <div className="flex items-center gap-2.5">
                    <div className="w-9 h-9 rounded-lg bg-yellow-500/10 flex items-center justify-center">
                      <svg className="w-5 h-5 text-yellow-400" fill="none" viewBox="0 0 24 24" stroke="currentColor" strokeWidth={1.5}>
                        <path strokeLinecap="round" strokeLinejoin="round" d="M16.5 10.5V6.75a4.5 4.5 0 1 0-9 0v3.75m-.75 11.25h10.5a2.25 2.25 0 0 0 2.25-2.25v-6.75a2.25 2.25 0 0 0-2.25-2.25H6.75a2.25 2.25 0 0 0-2.25 2.25v6.75a2.25 2.25 0 0 0 2.25 2.25Z" />
                      </svg>
                    </div>
                    <p className="text-sm font-medium text-dark-200">SSL Certs</p>
                  </div>
                  <p className="text-3xl font-bold text-dark-50 mt-2">{overview.ssl_certs_count}</p>
                  <p className="text-xs text-dark-300 mt-1">Active certificates</p>
                </div>
              </>
            )}
          </div>

          {/* Latest scan findings */}
          {posture?.latest_scan && posture.latest_scan.findings_count > 0 && (
            <div className="bg-dark-800 rounded-xl border border-dark-500 overflow-hidden mb-6">
              <div className="px-5 py-3 border-b border-dark-600 flex items-center justify-between">
                <h3 className="text-sm font-medium text-dark-50">Latest Scan Findings</h3>
                <span className="text-xs text-dark-300">
                  {new Date(posture.latest_scan.completed_at || posture.latest_scan.started_at).toLocaleString()}
                </span>
              </div>
              <div className="px-5 py-3 flex gap-4 text-sm border-b border-dark-600">
                {posture.latest_scan.critical_count > 0 && (
                  <span className="text-red-400 font-medium">{posture.latest_scan.critical_count} critical</span>
                )}
                {posture.latest_scan.warning_count > 0 && (
                  <span className="text-amber-600 font-medium">{posture.latest_scan.warning_count} warning</span>
                )}
                {posture.latest_scan.info_count > 0 && (
                  <span className="text-blue-600 font-medium">{posture.latest_scan.info_count} info</span>
                )}
              </div>
              <div className="p-3">
                <button
                  onClick={() => { setTab("scans"); handleViewScan(posture.latest_scan!.id); }}
                  className="text-sm text-rust-500 hover:text-rust-700 font-medium"
                >
                  View details
                </button>
              </div>
            </div>
          )}

          <div className="grid grid-cols-1 lg:grid-cols-2 gap-6">
            {/* Firewall Rules */}
            <div className="bg-dark-800 rounded-xl border border-dark-500 overflow-hidden">
              <div className="px-5 py-3 border-b border-dark-600 flex items-center justify-between">
                <h3 className="text-sm font-medium text-dark-50">Firewall Rules</h3>
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
                      <th scope="col" className="text-left text-xs font-medium text-dark-200 uppercase px-5 py-2">#</th>
                      <th scope="col" className="text-left text-xs font-medium text-dark-200 uppercase px-5 py-2">To</th>
                      <th scope="col" className="text-left text-xs font-medium text-dark-200 uppercase px-5 py-2">Action</th>
                      <th scope="col" className="text-left text-xs font-medium text-dark-200 uppercase px-5 py-2">From</th>
                      <th scope="col" className="text-right text-xs font-medium text-dark-200 uppercase px-5 py-2 w-16"></th>
                    </tr>
                  </thead>
                  <tbody className="divide-y divide-dark-600">
                    {firewall.rules.map((rule) => (
                      <tr key={rule.number} className="hover:bg-dark-800">
                        <td className="px-5 py-2.5 text-sm text-dark-300">{rule.number}</td>
                        <td className="px-5 py-2.5 text-sm text-dark-50 font-mono">{rule.to}</td>
                        <td className="px-5 py-2.5">
                          <span className={`text-xs font-medium ${rule.action.toLowerCase().includes("allow") ? "text-emerald-600" : "text-red-400"}`}>
                            {rule.action}
                          </span>
                        </td>
                        <td className="px-5 py-2.5 text-sm text-dark-200">{rule.from}</td>
                        <td className="px-5 py-2.5 text-right">
                          {deleteTarget === rule.number ? (
                            <div className="flex gap-1 justify-end">
                              <button onClick={() => handleDeleteRule(rule.number)} className="px-1.5 py-0.5 bg-red-600 text-white rounded text-[10px]">Del</button>
                              <button onClick={() => setDeleteTarget(null)} className="px-1.5 py-0.5 bg-dark-600 text-dark-200 rounded text-[10px]">No</button>
                            </div>
                          ) : (
                            <button onClick={() => setDeleteTarget(rule.number)} className="text-dark-300 hover:text-red-500" aria-label="Delete rule">
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
            <div className="bg-dark-800 rounded-xl border border-dark-500 overflow-hidden">
              <div className="px-5 py-3 border-b border-dark-600">
                <h3 className="text-sm font-medium text-dark-50">Fail2Ban Jails</h3>
              </div>
              {fail2ban && fail2ban.jails.length > 0 ? (
                <table className="w-full">
                  <thead>
                    <tr className="bg-dark-900">
                      <th scope="col" className="text-left text-xs font-medium text-dark-200 uppercase px-5 py-2">Jail</th>
                      <th scope="col" className="text-right text-xs font-medium text-dark-200 uppercase px-5 py-2">Banned</th>
                    </tr>
                  </thead>
                  <tbody className="divide-y divide-dark-600">
                    {fail2ban.jails.map((jail) => (
                      <tr key={jail.name}>
                        <td className="px-5 py-2.5 text-sm text-dark-50">{jail.name}</td>
                        <td className="px-5 py-2.5 text-sm text-right">
                          <span className={`font-medium ${jail.banned_count > 0 ? "text-red-400" : "text-dark-300"}`}>
                            {jail.banned_count}
                          </span>
                        </td>
                      </tr>
                    ))}
                  </tbody>
                </table>
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
            <div className="bg-dark-800 rounded-xl border border-dark-500 p-8 text-center">
              <p className="text-dark-300 text-sm">No security scans yet. Click "Run Security Scan" to start.</p>
            </div>
          ) : (
            scans.map((scan) => (
              <div key={scan.id} className="bg-dark-800 rounded-xl border border-dark-500 overflow-hidden">
                <button
                  onClick={() => handleViewScan(scan.id)}
                  className="w-full px-5 py-4 flex items-center justify-between hover:bg-dark-800 transition-colors text-left"
                >
                  <div className="flex items-center gap-4">
                    <div className={`w-10 h-10 rounded-lg flex items-center justify-center ${
                      scan.critical_count > 0 ? "bg-red-500/15" : scan.warning_count > 0 ? "bg-amber-500/15" : "bg-emerald-500/15"
                    }`}>
                      <svg className={`w-5 h-5 ${
                        scan.critical_count > 0 ? "text-red-400" : scan.warning_count > 0 ? "text-amber-600" : "text-emerald-600"
                      }`} fill="none" viewBox="0 0 24 24" stroke="currentColor" strokeWidth={1.5}>
                        <path strokeLinecap="round" strokeLinejoin="round" d="M9 12.75L11.25 15 15 9.75m-3-7.036A11.959 11.959 0 0 1 3.598 6 11.99 11.99 0 0 0 3 9.749c0 5.592 3.824 10.29 9 11.623 5.176-1.332 9-6.03 9-11.622 0-1.31-.21-2.571-.598-3.751h-.152c-3.196 0-6.1-1.248-8.25-3.285Z" />
                      </svg>
                    </div>
                    <div>
                      <p className="text-sm font-medium text-dark-50">
                        {scan.scan_type === "full" ? "Full Security Scan" : scan.scan_type}
                      </p>
                      <p className="text-xs text-dark-300">
                        {new Date(scan.started_at).toLocaleString()}
                        {scan.status === "running" && " (running...)"}
                      </p>
                    </div>
                  </div>
                  <div className="flex items-center gap-3">
                    {scan.critical_count > 0 && (
                      <span className="px-2 py-0.5 bg-red-500/15 text-red-400 rounded text-xs font-medium">{scan.critical_count} critical</span>
                    )}
                    {scan.warning_count > 0 && (
                      <span className="px-2 py-0.5 bg-amber-500/15 text-amber-400 rounded text-xs font-medium">{scan.warning_count} warning</span>
                    )}
                    {scan.info_count > 0 && (
                      <span className="px-2 py-0.5 bg-blue-500/15 text-blue-400 rounded text-xs font-medium">{scan.info_count} info</span>
                    )}
                    {scan.findings_count === 0 && (
                      <span className="px-2 py-0.5 bg-emerald-500/15 text-emerald-400 rounded text-xs font-medium">Clean</span>
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
                      <div className="divide-y divide-gray-50">
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
                                  <span className={`px-1.5 py-0.5 rounded text-[10px] font-medium ${checkTypeBadge(f.check_type)}`}>
                                    {f.check_type.replace("_", " ")}
                                  </span>
                                  {f.file_path && (
                                    <span className="text-[11px] text-dark-300 font-mono truncate">{f.file_path}</span>
                                  )}
                                </div>
                                {f.remediation && (
                                  <p className="text-xs text-rust-500 mt-1">{f.remediation}</p>
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

      {/* Add Rule Dialog */}
      {showAddRule && (
        <div className="fixed inset-0 bg-black/30 flex items-center justify-center z-50" onKeyDown={(e) => { if (e.key === "Escape") setShowAddRule(false); }}>
          <div className="bg-dark-800 rounded-xl shadow-xl p-6 w-96" role="dialog" aria-labelledby="add-rule-title">
            <h3 id="add-rule-title" className="text-lg font-semibold text-dark-50 mb-4">Add Firewall Rule</h3>
            <div className="space-y-3">
              <div>
                <label htmlFor="rule-port" className="block text-sm font-medium text-dark-100 mb-1">Port</label>
                <input
                  id="rule-port"
                  type="number"
                  value={rulePort}
                  onChange={(e) => setRulePort(e.target.value)}
                  className="w-full px-3 py-2 border border-dark-500 rounded-lg text-sm"
                  placeholder="e.g. 8080"
                  autoFocus
                />
              </div>
              <div className="grid grid-cols-2 gap-3">
                <div>
                  <label htmlFor="rule-protocol" className="block text-sm font-medium text-dark-100 mb-1">Protocol</label>
                  <select
                    id="rule-protocol"
                    value={ruleProto}
                    onChange={(e) => setRuleProto(e.target.value)}
                    className="w-full px-3 py-2 border border-dark-500 rounded-lg text-sm"
                  >
                    <option value="tcp">TCP</option>
                    <option value="udp">UDP</option>
                    <option value="tcp/udp">TCP/UDP</option>
                  </select>
                </div>
                <div>
                  <label htmlFor="rule-action" className="block text-sm font-medium text-dark-100 mb-1">Action</label>
                  <select
                    id="rule-action"
                    value={ruleAction}
                    onChange={(e) => setRuleAction(e.target.value)}
                    className="w-full px-3 py-2 border border-dark-500 rounded-lg text-sm"
                  >
                    <option value="allow">Allow</option>
                    <option value="deny">Deny</option>
                  </select>
                </div>
              </div>
              <div>
                <label htmlFor="rule-from" className="block text-sm font-medium text-dark-100 mb-1">From IP (optional)</label>
                <input
                  id="rule-from"
                  type="text"
                  value={ruleFrom}
                  onChange={(e) => setRuleFrom(e.target.value)}
                  className="w-full px-3 py-2 border border-dark-500 rounded-lg text-sm"
                  placeholder="Any (leave empty)"
                />
              </div>
            </div>
            <div className="flex justify-end gap-2 mt-5">
              <button onClick={() => setShowAddRule(false)} className="px-4 py-2 text-sm text-dark-200">Cancel</button>
              <button
                onClick={handleAddRule}
                disabled={!rulePort}
                className="px-4 py-2 bg-rust-500 text-white rounded-lg text-sm font-medium hover:bg-rust-600 disabled:opacity-50"
              >
                Add Rule
              </button>
            </div>
          </div>
        </div>
      )}
    </div>
  );
}
