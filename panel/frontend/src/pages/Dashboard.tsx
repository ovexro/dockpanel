import { useState, useEffect, useCallback } from "react";
import { Link } from "react-router-dom";
import { api } from "../api";
import { formatSize, formatUptime } from "../utils/format";

interface OnboardingStep {
  id: string;
  label: string;
  description: string;
  link: string;
  check: () => boolean;
}


interface SystemInfo {
  cpu_count: number;
  cpu_usage: number;
  cpu_model: string;
  cpu_temp: number | null;
  mem_total_mb: number;
  mem_used_mb: number;
  mem_usage_pct: number;
  swap_total_mb: number;
  swap_used_mb: number;
  disk_total_gb: number;
  disk_used_gb: number;
  disk_usage_pct: number;
  uptime_secs: number;
  hostname: string;
  os: string;
  kernel: string;
  load_avg_1?: number;
  load_avg_5?: number;
  load_avg_15?: number;
  process_count: number;
}

interface Process {
  pid: number;
  name: string;
  cpu_pct: number;
  mem_mb: number;
}

interface NetworkIface {
  name: string;
  rx_bytes: number;
  tx_bytes: number;
}

interface SiteSummary {
  total: number;
  active: number;
}

interface SslCountdown {
  domain: string;
  days_left: number;
  severity: string;
}

interface TopIssue {
  title: string;
  severity: string;
  type: string;
  since: string;
}

interface Intelligence {
  health_score: number;
  grade: string;
  firing_alerts: number;
  acknowledged_alerts: number;
  ssl_countdowns: SslCountdown[];
  top_issues: TopIssue[];
}

interface MetricPoint {
  cpu: number;
  mem: number;
  disk: number;
  time: string;
}

function Sparkline({ data, color, height = 60 }: { data: number[]; color: string; height?: number }) {
  if (data.length < 2) return null;
  const max = Math.max(...data, 1);
  const width = 300;
  const points = data.map((v, i) => {
    const x = (i / (data.length - 1)) * width;
    const y = height - (v / max) * (height - 4) - 2;
    return `${x},${y}`;
  }).join(" ");

  const fillPoints = `0,${height} ${points} ${width},${height}`;

  return (
    <svg viewBox={`0 0 ${width} ${height}`} className="w-full" preserveAspectRatio="none" aria-hidden="true">
      <polygon points={fillPoints} fill={color} opacity="0.08" />
      <polyline points={points} fill="none" stroke={color} strokeWidth="2" strokeLinejoin="round" strokeLinecap="round" vectorEffect="non-scaling-stroke" />
    </svg>
  );
}

function barColor(pct: number): string {
  if (pct < 60) return "bg-emerald-500";
  if (pct < 85) return "bg-amber-500";
  return "bg-red-500";
}

function pctColor(pct: number): string {
  if (pct < 60) return "text-emerald-400";
  if (pct < 85) return "text-amber-400";
  return "text-red-400";
}

function tempColor(temp: number): string {
  if (temp < 60) return "text-emerald-400";
  if (temp < 80) return "text-amber-400";
  return "text-red-400";
}

export default function Dashboard() {
  const [system, setSystem] = useState<SystemInfo | null>(null);
  const [sites, setSites] = useState<SiteSummary>({ total: 0, active: 0 });
  const [dbCount, setDbCount] = useState(0);
  const [processes, setProcesses] = useState<Process[]>([]);
  const [network, setNetwork] = useState<NetworkIface[]>([]);
  const [error, setError] = useState("");
  const [intel, setIntel] = useState<Intelligence | null>(null);
  const [appCount, setAppCount] = useState(0);
  const [updateCount, setUpdateCount] = useState(0);
  const [rebootRequired, setRebootRequired] = useState(false);
  const [dismissed, setDismissed] = useState(() => localStorage.getItem("dp-onboarding-dismissed") === "1");
  const [metricsHistory, setMetricsHistory] = useState<MetricPoint[]>([]);

  const dismissOnboarding = useCallback(() => {
    setDismissed(true);
    localStorage.setItem("dp-onboarding-dismissed", "1");
  }, []);

  const fetchData = () => {
    api
      .get<SystemInfo>("/system/info")
      .then(setSystem)
      .catch((e) => setError(e instanceof Error ? e.message : "Failed to load system info"));
    api
      .get<{ id: string; status: string }[]>("/sites")
      .then((list) =>
        setSites({
          total: list.length,
          active: list.filter((s) => s.status === "active").length,
        })
      )
      .catch(() => setError("Failed to load sites. Please try again."));
    api
      .get<{ id: string }[]>("/databases")
      .then((list) => setDbCount(list.length))
      .catch(() => setError("Failed to load databases. Please try again."));
    api
      .get<Process[]>("/system/processes")
      .then(setProcesses)
      .catch(() => {}); // Non-critical: processes panel simply stays empty
    api
      .get<NetworkIface[]>("/system/network")
      .then(setNetwork)
      .catch(() => {}); // Non-critical: network panel simply stays empty
    api
      .get<Intelligence>("/dashboard/intelligence")
      .then(setIntel)
      .catch(() => {}); // Non-critical: intelligence panel simply stays empty
    api
      .get<{ container_id: string }[]>("/apps")
      .then((list) => setAppCount(list.length))
      .catch(() => {}); // Non-critical: only affects onboarding step count
    api
      .get<{ count: number; security: number; reboot_required: boolean }>("/system/updates/count")
      .then((d) => { setUpdateCount(d.count); setRebootRequired(d.reboot_required); })
      .catch(() => {});
    api
      .get<{ points: MetricPoint[] }>("/dashboard/metrics-history")
      .then((d) => setMetricsHistory(d.points || []))
      .catch(() => {});
  };

  useEffect(() => {
    fetchData();
    const interval = setInterval(fetchData, 10000);
    return () => clearInterval(interval);
  }, []);

  return (
    <div className="p-6 lg:p-8">
      <div className="flex flex-col sm:flex-row sm:items-center justify-between gap-3 mb-6 pb-4 border-b border-dark-600">
        <h1 className="text-sm font-medium text-dark-300 uppercase font-mono tracking-widest">Dashboard</h1>
        <div className="flex items-center gap-2 flex-wrap">
          <Link to="/apps" className="px-3 py-1.5 bg-dark-800 border border-dark-500 rounded-lg text-xs font-medium text-dark-100 hover:bg-dark-700 hover:text-dark-50 flex items-center gap-1.5">
            <svg className="w-3.5 h-3.5" fill="none" viewBox="0 0 24 24" stroke="currentColor" strokeWidth={2}><path strokeLinecap="round" strokeLinejoin="round" d="M12 4.5v15m7.5-7.5h-15" /></svg>
            Deploy App
          </Link>
          <Link to="/sites" className="px-3 py-1.5 bg-dark-800 border border-dark-500 rounded-lg text-xs font-medium text-dark-100 hover:bg-dark-700 hover:text-dark-50 flex items-center gap-1.5">
            <svg className="w-3.5 h-3.5" fill="none" viewBox="0 0 24 24" stroke="currentColor" strokeWidth={2}><path strokeLinecap="round" strokeLinejoin="round" d="M12 4.5v15m7.5-7.5h-15" /></svg>
            Add Site
          </Link>
          <Link to="/diagnostics" className="px-3 py-1.5 bg-dark-800 border border-dark-500 rounded-lg text-xs font-medium text-dark-100 hover:bg-dark-700 hover:text-dark-50 flex items-center gap-1.5">
            <svg className="w-3.5 h-3.5" fill="none" viewBox="0 0 24 24" stroke="currentColor" strokeWidth={2}><path strokeLinecap="round" strokeLinejoin="round" d="M9.75 3.104v5.714a2.25 2.25 0 01-.659 1.591L5 14.5" /></svg>
            Diagnostics
          </Link>
        </div>
      </div>

      {error && (
        <div className="bg-red-500/10 text-red-400 text-sm px-4 py-3 rounded-lg border border-red-500/20 mb-6">
          <div className="flex items-start gap-3">
            <svg className="w-5 h-5 shrink-0 mt-0.5" fill="none" viewBox="0 0 24 24" stroke="currentColor" strokeWidth={1.5}>
              <path strokeLinecap="round" strokeLinejoin="round" d="M12 9v3.75m-9.303 3.376c-.866 1.5.217 3.374 1.948 3.374h14.71c1.73 0 2.813-1.874 1.948-3.374L13.949 3.378c-.866-1.5-3.032-1.5-3.898 0L2.697 16.126ZM12 15.75h.007v.008H12v-.008Z" />
            </svg>
            <div>
              <p className="font-medium">{error}</p>
              {error.includes("Agent offline") && (
                <p className="text-xs text-dark-300 mt-1 font-mono">
                  Run: <span className="text-dark-100">systemctl restart dockpanel-agent</span>
                </p>
              )}
            </div>
          </div>
        </div>
      )}

      {/* Getting Started */}
      {!dismissed && system && (() => {
        const steps: OnboardingStep[] = [
          { id: "site", label: "Create your first site", description: "Set up a website with Nginx, PHP, or reverse proxy", link: "/sites", check: () => sites.total > 0 },
          { id: "app", label: "Deploy a Docker app", description: "One-click deploy from 34 templates", link: "/apps", check: () => appCount > 0 },
          { id: "2fa", label: "Enable 2FA", description: "Protect your panel with two-factor authentication", link: "/settings", check: () => false },
          { id: "backup", label: "Set up backups", description: "Configure scheduled backups for your sites", link: "/sites", check: () => false },
          { id: "diagnostics", label: "Run diagnostics", description: "Check your server health and fix issues", link: "/diagnostics", check: () => false },
        ];
        const completed = steps.filter(s => s.check()).length;
        if (completed >= 3) return null; // Auto-hide after 3+ steps done
        return (
          <div className="mb-6 bg-dark-800 rounded-lg border border-dark-500 p-4">
            <div className="flex items-center justify-between mb-3">
              <div className="flex items-center gap-2">
                <svg className="w-5 h-5 text-rust-400" fill="none" viewBox="0 0 24 24" stroke="currentColor" strokeWidth={1.5}><path strokeLinecap="round" strokeLinejoin="round" d="M15.59 14.37a6 6 0 01-5.84 7.38v-4.8m5.84-2.58a14.98 14.98 0 006.16-12.12A14.98 14.98 0 009.631 8.41m5.96 5.96a14.926 14.926 0 01-5.841 2.58m-.119-8.54a6 6 0 00-7.381 5.84h4.8m2.581-5.84a14.927 14.927 0 00-2.58 5.84m2.699 2.7c-.103.021-.207.041-.311.06a15.09 15.09 0 01-2.448-2.448 14.9 14.9 0 01.06-.312m-2.24 2.39a4.493 4.493 0 00-1.757 4.306 4.493 4.493 0 004.306-1.758M16.5 9a1.5 1.5 0 11-3 0 1.5 1.5 0 013 0z" /></svg>
                <h3 className="text-xs font-medium text-dark-300 uppercase font-mono tracking-widest">Getting Started</h3>
                <span className="text-[10px] text-dark-300">{completed}/{steps.length}</span>
              </div>
              <button onClick={dismissOnboarding} className="text-dark-300 hover:text-dark-200 text-xs">Dismiss</button>
            </div>
            <div className="grid grid-cols-1 sm:grid-cols-2 lg:grid-cols-5 gap-2">
              {steps.map(step => (
                <Link
                  key={step.id}
                  to={step.link}
                  className={`rounded-lg border p-3 transition-colors ${
                    step.check()
                      ? "border-emerald-500/30 bg-emerald-500/5"
                      : "border-dark-500 bg-dark-900/50 hover:border-dark-400"
                  }`}
                >
                  <div className="flex items-center gap-1.5 mb-1">
                    {step.check() ? (
                      <svg className="w-3.5 h-3.5 text-emerald-400" fill="none" viewBox="0 0 24 24" stroke="currentColor" strokeWidth={2.5}><path strokeLinecap="round" strokeLinejoin="round" d="M5 13l4 4L19 7" /></svg>
                    ) : (
                      <div className="w-3.5 h-3.5 rounded-full border-2 border-dark-400" />
                    )}
                    <span className={`text-xs font-medium ${step.check() ? "text-emerald-400" : "text-dark-100"}`}>{step.label}</span>
                  </div>
                  <p className="text-[10px] text-dark-300 leading-relaxed">{step.description}</p>
                </Link>
              ))}
            </div>
          </div>
        );
      })()}

      {!system ? (
        <div className="grid grid-cols-1 md:grid-cols-2 lg:grid-cols-3 gap-4" role="status" aria-live="polite">
          {[...Array(6)].map((_, i) => (
            <div key={i} className="bg-dark-800 rounded-lg border border-dark-500 p-4 animate-pulse">
              <div className="h-4 bg-dark-700 rounded w-20 mb-3" />
              <div className="h-8 bg-dark-700 rounded w-32" />
            </div>
          ))}
        </div>
      ) : (
        <>
          {/* Resource Metrics — 3 column */}
          <div className="grid grid-cols-1 md:grid-cols-3 gap-4 mb-6">
            {[
              { label: "CPU Usage", pct: system.cpu_usage, detail: `${system.cpu_count} cores${system.load_avg_1 !== undefined ? ` · Load ${system.load_avg_1?.toFixed(2)}` : ""}`,
                icon: <svg className="w-4 h-4" fill="none" viewBox="0 0 24 24" stroke="currentColor" strokeWidth={1.5}><rect x="6" y="6" width="12" height="12" rx="1" /><path d="M9 1v4m6-4v4M9 19v4m6-4v4M1 9h4m-4 6h4M19 9h4m-4 6h4" strokeLinecap="round" /></svg> },
              { label: "Memory", pct: system.mem_usage_pct, detail: `${(system.mem_used_mb / 1024).toFixed(1)} / ${(system.mem_total_mb / 1024).toFixed(1)} GB${system.swap_total_mb > 0 ? ` · Swap ${(system.swap_used_mb / 1024).toFixed(1)}G` : ""}`,
                icon: <svg className="w-4 h-4" fill="none" viewBox="0 0 24 24" stroke="currentColor" strokeWidth={1.5}><rect x="3" y="4" width="18" height="16" rx="1" /><path d="M7 4v3m4-3v3m4-3v3M3 10h18" strokeLinecap="round" /></svg> },
              { label: "Disk", pct: system.disk_usage_pct, detail: `${system.disk_used_gb.toFixed(0)} / ${system.disk_total_gb.toFixed(0)} GB · ${(system.disk_total_gb - system.disk_used_gb).toFixed(0)} GB free`,
                icon: <svg className="w-4 h-4" fill="none" viewBox="0 0 24 24" stroke="currentColor" strokeWidth={1.5}><path strokeLinecap="round" strokeLinejoin="round" d="M21.75 17.25v-.228a4.5 4.5 0 0 0-.12-1.03l-2.268-9.64a3.375 3.375 0 0 0-3.285-2.602H7.923a3.375 3.375 0 0 0-3.285 2.602l-2.268 9.64a4.5 4.5 0 0 0-.12 1.03v.228m19.5 0a3 3 0 0 1-3 3H5.25a3 3 0 0 1-3-3m19.5 0a3 3 0 0 0-3-3H5.25a3 3 0 0 0-3 3m16.5 0h.008v.008h-.008v-.008Zm-3 0h.008v.008h-.008v-.008Z" /></svg> },
            ].map(({ label, pct, detail, icon }) => (
              <div key={label} className="border border-dark-500 bg-dark-800 p-5 relative overflow-hidden">
                <div className={`absolute inset-0 opacity-[0.03] ${pct < 60 ? "bg-emerald-500" : pct < 85 ? "bg-amber-500" : "bg-red-500"}`} />
                <div className="relative text-center">
                  <div className="flex items-center justify-center gap-1.5 text-dark-200 mb-1">
                    <span className="opacity-60">{icon}</span>
                    <span className="text-xs uppercase tracking-widest font-medium">{label}</span>
                  </div>
                  <div className={`text-5xl font-bold font-mono my-2 ${pctColor(pct)}`}>
                    {pct.toFixed(0)}<span className="text-xl text-dark-300 ml-0.5">%</span>
                  </div>
                  <div className="h-2 bg-dark-700 rounded-full overflow-hidden mt-3 mx-auto max-w-[80%]">
                    <div className={`h-full rounded-full transition-all duration-700 ${barColor(pct)}`} style={{ width: `${Math.min(pct, 100)}%` }} />
                  </div>
                  <p className="text-xs text-dark-300 mt-3">{detail}</p>
                </div>
              </div>
            ))}
          </div>

          {/* Historical Charts */}
          {metricsHistory.length >= 2 && (
            <div className="grid grid-cols-1 md:grid-cols-3 gap-4 mb-6">
              {[
                { label: "CPU", data: metricsHistory.map(p => p.cpu), color: "#10b981" },
                { label: "Memory", data: metricsHistory.map(p => p.mem), color: "#6366f1" },
                { label: "Disk", data: metricsHistory.map(p => p.disk), color: "#f59e0b" },
              ].map(({ label, data, color }) => (
                <div key={label} className="border border-dark-500 bg-dark-800 p-4">
                  <div className="flex items-center justify-between mb-2">
                    <span className="text-[10px] uppercase tracking-widest text-dark-300 font-medium">{label} (24h)</span>
                    <span className="text-xs text-dark-200 font-mono">{data[data.length - 1]?.toFixed(1)}%</span>
                  </div>
                  <Sparkline data={data} color={color} height={48} />
                </div>
              ))}
            </div>
          )}

          {/* Status Bar — grid of stat cells */}
          <div className="grid grid-cols-2 sm:grid-cols-3 lg:grid-cols-7 gap-px bg-dark-600 border border-dark-500 mb-6">
            <div className="bg-dark-800 px-4 py-3 flex flex-col">
              <span className="text-[10px] text-dark-300 uppercase tracking-widest mb-1">Uptime</span>
              <span className="text-sm text-dark-50 font-medium">{formatUptime(system.uptime_secs)}</span>
            </div>
            <div className="bg-dark-800 px-4 py-3 flex flex-col">
              <span className="text-[10px] text-dark-300 uppercase tracking-widest mb-1">Sites</span>
              <span className="text-sm text-dark-50 font-medium">{sites.total}{sites.active > 0 && <span className="text-rust-400 ml-1 text-xs">({sites.active} active)</span>}</span>
            </div>
            <div className="bg-dark-800 px-4 py-3 flex flex-col">
              <span className="text-[10px] text-dark-300 uppercase tracking-widest mb-1">Databases</span>
              <span className="text-sm text-dark-50 font-medium">{dbCount}</span>
            </div>
            {intel && <>
              <div className={`px-4 py-3 flex flex-col ${
                intel.health_score < 60 ? "bg-red-500/5" : "bg-dark-800"
              }`}>
                <span className="text-[10px] text-dark-300 uppercase tracking-widest mb-1">Health</span>
                <span className={`text-sm font-bold ${
                  intel.health_score >= 90 ? "text-emerald-400" :
                  intel.health_score >= 75 ? "text-blue-400" :
                  intel.health_score >= 60 ? "text-amber-400" : "text-red-400"
                }`}>{intel.health_score}/100 {intel.grade}</span>
              </div>
              <div className={`px-4 py-3 flex flex-col ${
                intel.firing_alerts > 0 ? "bg-red-500/5" : "bg-dark-800"
              }`}>
                <span className="text-[10px] text-dark-300 uppercase tracking-widest mb-1">Alerts</span>
                {intel.firing_alerts > 0
                  ? <span className="text-sm text-red-400 font-bold">{intel.firing_alerts} firing</span>
                  : <span className="text-sm text-emerald-400 font-medium">0</span>
                }
              </div>
              <div className="bg-dark-800 px-4 py-3 flex flex-col">
                <span className="text-[10px] text-dark-300 uppercase tracking-widest mb-1">SSL</span>
                <span className="text-sm text-dark-50 font-medium">{intel.ssl_countdowns.length} certs</span>
              </div>
            </>}
            <div className="bg-dark-800 px-4 py-3 flex flex-col">
              <span className="text-[10px] text-dark-300 uppercase tracking-widest mb-1">Updates</span>
              {updateCount > 0
                ? <span className="text-sm text-amber-400 font-bold">{updateCount} available</span>
                : <span className="text-sm text-emerald-400 font-medium">up to date</span>
              }
            </div>
          </div>

          {/* Reboot Required Warning */}
          {rebootRequired && (
            <div className="border border-amber-500/50 bg-amber-500/5 p-4 mb-6 flex items-start gap-3">
              <svg className="w-5 h-5 text-amber-400 shrink-0 mt-0.5" fill="none" viewBox="0 0 24 24" stroke="currentColor" strokeWidth={1.5}><path strokeLinecap="round" strokeLinejoin="round" d="M12 9v3.75m-9.303 3.376c-.866 1.5.217 3.374 1.948 3.374h14.71c1.73 0 2.813-1.874 1.948-3.374L13.949 3.378c-.866-1.5-3.032-1.5-3.898 0L2.697 16.126ZM12 15.75h.007v.008H12v-.008Z" /></svg>
              <div className="flex-1">
                <p className="text-sm text-amber-400 font-bold">Reboot Required</p>
                <p className="text-xs text-dark-300 mt-1">Recent package updates (such as a new kernel version) require a reboot to be fully applied.</p>
              </div>
              <Link to="/updates" className="px-4 py-2 bg-amber-500 text-dark-900 text-xs font-bold uppercase tracking-wider hover:bg-amber-400 transition-colors shrink-0">
                View Updates
              </Link>
            </div>
          )}

          {/* Active Issues + SSL — side by side */}
          {intel && (intel.top_issues.length > 0 || intel.ssl_countdowns.length > 0) && (
            <div className="grid grid-cols-1 lg:grid-cols-2 gap-4 mb-6">
              {intel.top_issues.length > 0 && (
                <div className="border border-dark-500 bg-dark-800 p-4">
                  <div className="flex items-center justify-between mb-3">
                    <h3 className="text-xs text-dark-300 uppercase tracking-widest">Active Issues</h3>
                    <Link to="/alerts" className="text-xs text-rust-400 hover:text-rust-300">View all</Link>
                  </div>
                  <div className="space-y-2">
                    {intel.top_issues.slice(0, 4).map((issue, i) => (
                      <div key={i} className="flex items-start gap-2">
                        <div className={`w-2 h-2 rounded-full mt-1.5 flex-shrink-0 ${
                          issue.severity === "critical" ? "bg-red-500" :
                          issue.severity === "warning" ? "bg-amber-500" : "bg-blue-500"
                        }`} />
                        <p className="text-xs text-dark-100 leading-tight">{issue.title}</p>
                      </div>
                    ))}
                  </div>
                </div>
              )}
              {intel.ssl_countdowns.length > 0 && (
                <div className="border border-dark-500 bg-dark-800 p-4">
                  <h3 className="text-xs text-dark-300 uppercase tracking-widest mb-3">SSL Certificates</h3>
                  <div className="space-y-2">
                    {intel.ssl_countdowns.map((ssl, i) => (
                      <div key={i} className="flex items-center justify-between">
                        <span className="text-xs text-dark-100 truncate max-w-[200px]">{ssl.domain}</span>
                        <span className={`text-xs ${
                          ssl.severity === "critical" ? "text-red-400" :
                          ssl.severity === "warning" ? "text-amber-400" :
                          ssl.severity === "info" ? "text-blue-400" : "text-emerald-400"
                        }`}>{ssl.days_left}d left</span>
                      </div>
                    ))}
                  </div>
                </div>
              )}
            </div>
          )}

          {/* System Information — neofetch style */}
          <div className="border border-dark-500 bg-dark-800 p-5 mb-6">
            <h3 className="text-xs text-dark-300 uppercase tracking-widest mb-4">System Information</h3>
            <div className="grid grid-cols-1 lg:grid-cols-2 gap-x-8 gap-y-1.5 text-sm">
              {[
                ["Hostname", system.hostname],
                ["OS", system.os],
                ["Kernel", system.kernel],
                ["Processor", `${system.cpu_model}, ${system.cpu_count} cores`],
                ["Temperature", system.cpu_temp != null ? `${system.cpu_temp.toFixed(0)}°C` : "N/A"],
                ["Processes", system.process_count.toLocaleString()],
              ].map(([label, value]) => (
                <div key={label} className="flex items-baseline gap-2 min-w-0">
                  <span className="text-rust-400 shrink-0">{">"}</span>
                  <span className="text-dark-300 w-24 shrink-0">{label}</span>
                  <span className="text-dark-500 flex-1 border-b border-dotted border-dark-500 mx-1 mb-1 min-w-4" />
                  <span title={String(value)} className={`text-dark-50 shrink-0 max-w-[55%] truncate ${label === "Temperature" && system.cpu_temp != null ? tempColor(system.cpu_temp) : ""}`}>{value}</span>
                </div>
              ))}
            </div>
          </div>

          {/* Network & Processes */}
          <div className="grid grid-cols-1 lg:grid-cols-2 gap-6">
            {/* Network I/O */}
            {network.length > 0 && (
              <div className="bg-dark-800 rounded-lg border border-dark-500 overflow-x-auto">
                <div className="px-5 py-3 border-b border-dark-600">
                  <h3 className="text-xs font-medium text-dark-300 uppercase font-mono tracking-widest">Network I/O</h3>
                </div>
                <table className="w-full">
                  <thead>
                    <tr className="bg-dark-900">
                      <th className="text-left text-xs font-medium text-dark-200 uppercase font-mono tracking-widest px-5 py-2">Interface</th>
                      <th className="text-right text-xs font-medium text-dark-200 uppercase font-mono tracking-widest px-5 py-2">RX</th>
                      <th className="text-right text-xs font-medium text-dark-200 uppercase font-mono tracking-widest px-5 py-2">TX</th>
                    </tr>
                  </thead>
                  <tbody className="divide-y divide-dark-600">
                    {network.filter(n => n.rx_bytes > 0 || n.tx_bytes > 0).map((iface) => (
                      <tr key={iface.name} className="hover:bg-dark-700/30 transition-colors">
                        <td className="px-5 py-2.5 text-sm text-dark-50 font-mono">{iface.name}</td>
                        <td className="px-5 py-2.5 text-sm text-dark-200 text-right font-mono">
                          <span className="text-emerald-600">{formatSize(iface.rx_bytes)}</span>
                        </td>
                        <td className="px-5 py-2.5 text-sm text-dark-200 text-right font-mono">
                          <span className="text-blue-600">{formatSize(iface.tx_bytes)}</span>
                        </td>
                      </tr>
                    ))}
                  </tbody>
                </table>
              </div>
            )}

            {/* Top Processes */}
            {processes.length > 0 && (
              <div className="bg-dark-800 rounded-lg border border-dark-500 overflow-x-auto">
                <div className="px-5 py-3 border-b border-dark-600">
                  <h3 className="text-xs font-medium text-dark-300 uppercase font-mono tracking-widest">Top Processes</h3>
                </div>
                <table className="w-full">
                  <thead>
                    <tr className="bg-dark-900">
                      <th className="text-left text-xs font-medium text-dark-200 uppercase font-mono tracking-widest px-5 py-2">Process</th>
                      <th className="text-right text-xs font-medium text-dark-200 uppercase font-mono tracking-widest px-5 py-2">PID</th>
                      <th className="text-right text-xs font-medium text-dark-200 uppercase font-mono tracking-widest px-5 py-2">CPU</th>
                      <th className="text-right text-xs font-medium text-dark-200 uppercase font-mono tracking-widest px-5 py-2">MEM</th>
                    </tr>
                  </thead>
                  <tbody className="divide-y divide-dark-600">
                    {processes.slice(0, 10).map((p) => (
                      <tr key={p.pid} className="hover:bg-dark-700/30 transition-colors">
                        <td className="px-5 py-2 text-sm text-dark-50 font-mono truncate max-w-[200px]">{p.name}</td>
                        <td className="px-5 py-2 text-sm text-dark-200 text-right font-mono">{p.pid}</td>
                        <td className="px-5 py-2 text-sm text-right font-mono">
                          <span className={p.cpu_pct > 50 ? "text-red-400 font-medium" : "text-dark-200"}>
                            {p.cpu_pct.toFixed(1)}%
                          </span>
                        </td>
                        <td className="px-5 py-2 text-sm text-dark-200 text-right font-mono">{p.mem_mb.toFixed(0)} MB</td>
                      </tr>
                    ))}
                  </tbody>
                </table>
              </div>
            )}
          </div>
        </>
      )}
    </div>
  );
}
