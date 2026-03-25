import { useState, useEffect, useCallback, useRef, useMemo } from "react";
import { Link } from "react-router-dom";
import { api } from "../api";
import { formatSize, formatRate, formatUptime, timeAgo } from "../utils/format";

interface SiteDetail {
  id: string;
  status: string;
  backup_schedule?: string;
}

function useCountUp(target: number, duration = 800): number {
  const [value, setValue] = useState(0);
  const prev = useRef(0);
  useEffect(() => {
    const start = prev.current;
    const diff = target - start;
    if (Math.abs(diff) < 0.5) { setValue(target); prev.current = target; return; }
    const steps = Math.max(Math.floor(duration / 16), 1);
    let step = 0;
    const timer = setInterval(() => {
      step++;
      const progress = step / steps;
      const eased = 1 - Math.pow(1 - progress, 3); // ease-out cubic
      setValue(start + diff * eased);
      if (step >= steps) {
        setValue(target);
        prev.current = target;
        clearInterval(timer);
      }
    }, 16);
    return () => clearInterval(timer);
  }, [target, duration]);
  return value;
}

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
  rx_rate?: number;
  tx_rate?: number;
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

interface Recommendation {
  severity: string;
  message: string;
  action: string;
}

interface Intelligence {
  health_score: number;
  grade: string;
  firing_alerts: number;
  acknowledged_alerts: number;
  open_incidents: number;
  stale_backups: number;
  scan_critical: number;
  scan_warnings: number;
  ssl_countdowns: SslCountdown[];
  top_issues: TopIssue[];
  recommendations: Recommendation[];
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

function CountUp({ value }: { value: number }) {
  const displayed = useCountUp(value);
  return <>{displayed.toFixed(0)}</>;
}

function barColor(pct: number, type: "cpu" | "mem" | "disk" = "cpu"): string {
  if (type === "disk") {
    if (pct < 80) return "bg-rust-500";
    if (pct <= 90) return "bg-warn-500";
    return "bg-danger-500";
  }
  // CPU and Memory
  if (pct < 70) return "bg-rust-500";
  if (pct <= 90) return "bg-warn-500";
  return "bg-danger-500";
}

function pctColor(pct: number): string {
  if (pct < 60) return "text-dark-50";
  if (pct < 80) return "text-warn-400";
  return "text-danger-400";
}

function tempColor(temp: number): string {
  if (temp < 60) return "text-rust-400";
  if (temp < 80) return "text-warn-400";
  return "text-danger-400";
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
  const [onboardingCollapsed, setOnboardingCollapsed] = useState(() => localStorage.getItem('dp-onboarding-collapsed') === '1');
  const [metricsHistory, setMetricsHistory] = useState<MetricPoint[]>([]);
  const [twoFaEnabled, setTwoFaEnabled] = useState(false);
  const [sitesList, setSitesList] = useState<SiteDetail[]>([]);
  const [wsConnected, setWsConnected] = useState(false);
  const wsRef = useRef<WebSocket | null>(null);
  const wsConnectedRef = useRef(false);
  const reconnectTimer = useRef<ReturnType<typeof setTimeout> | null>(null);

  // Feature #1: Docker container overview
  const [dockerInfo, setDockerInfo] = useState<{ total: number; running: number; stopped: number } | null>(null);
  // Feature #2: Recent activity feed
  const [recentActivity, setRecentActivity] = useState<any[]>([]);
  // Feature #6: Bandwidth usage summary
  const [bandwidthTotal, setBandwidthTotal] = useState({ rx: 0, tx: 0 });
  // Feature #8: Docker image disk usage
  const [dockerDiskUsage, setDockerDiskUsage] = useState<string | null>(null);
  // Feature #9: Mail queue widget
  const [mailQueue, setMailQueue] = useState<number | null>(null);
  // Feature #4: Quick server action messages
  const [actionMessage, setActionMessage] = useState<{ text: string; type: string } | null>(null);
  // Feature #3: Disk I/O metrics
  const [diskIo, setDiskIo] = useState<{ read_bytes_sec: number; write_bytes_sec: number } | null>(null);
  // Feature #6: Customizable dashboard layout
  const [widgetConfig, setWidgetConfig] = useState<Record<string, boolean>>(() => {
    try { return JSON.parse(localStorage.getItem('dp-dashboard-widgets') || '{}'); } catch { return {}; }
  });
  const [showWidgetConfig, setShowWidgetConfig] = useState(false);
  // Feature #12: Custom dashboard widgets (Quick Links / Bookmarks)
  const [bookmarks, setBookmarks] = useState<{ label: string; url: string }[]>(() => {
    try { return JSON.parse(localStorage.getItem('dp-dashboard-bookmarks') || '[]'); } catch { return []; }
  });
  const [showAddBookmark, setShowAddBookmark] = useState(false);
  const [bmLabel, setBmLabel] = useState("");
  const [bmUrl, setBmUrl] = useState("");

  const isVisible = (widget: string) => widgetConfig[widget] !== false; // default visible
  const toggleWidget = (widget: string) => {
    const next = { ...widgetConfig, [widget]: !isVisible(widget) };
    setWidgetConfig(next);
    localStorage.setItem('dp-dashboard-widgets', JSON.stringify(next));
  };

  const dismissOnboarding = useCallback(() => {
    setDismissed(true);
    localStorage.setItem("dp-onboarding-dismissed", "1");
  }, []);

  // Fetch endpoints NOT covered by WebSocket (slow-changing data)
  const fetchSlowData = useCallback(() => {
    api
      .get<SiteDetail[]>("/sites")
      .then((list) => {
        setSitesList(list);
        setSites({
          total: list.length,
          active: list.filter((s) => s.status === "active").length,
        });
      })
      .catch(() => setError("Failed to load sites. Please try again."));
    api
      .get<{ id: string }[]>("/databases")
      .then((list) => setDbCount(list.length))
      .catch(() => setError("Failed to load databases. Please try again."));
    api
      .get<Intelligence>("/dashboard/intelligence")
      .then(setIntel)
      .catch(() => setError("Failed to load dashboard intelligence"));
    api
      .get<{ container_id: string }[]>("/apps")
      .then((list) => setAppCount(list.length))
      .catch(() => console.warn("Optional: failed to load app count"));
    api
      .get<{ count: number; security: number; reboot_required: boolean }>("/system/updates/count")
      .then((d) => { setUpdateCount(d.count); setRebootRequired(d.reboot_required); })
      .catch(() => setError("Failed to load system update status"));
    api
      .get<{ points: MetricPoint[] }>("/dashboard/metrics-history")
      .then((d) => setMetricsHistory(d.points || []))
      .catch(() => console.warn("Optional: failed to load metrics history"));
    api
      .get<{ enabled: boolean }>("/auth/2fa/status")
      .then((d) => setTwoFaEnabled(d.enabled))
      .catch(() => console.warn("Optional: failed to load 2FA status"));
    // Feature #1: Docker container overview
    api
      .get<{ total: number; running: number; stopped: number }>("/dashboard/docker")
      .then(setDockerInfo)
      .catch(() => console.warn("Optional: failed to load docker summary"));
    // Feature #2: Recent activity feed
    api
      .get<any[]>("/activity?limit=5")
      .then(setRecentActivity)
      .catch(() => {});
    // Feature #8: Docker image disk usage
    api
      .get<any>("/apps/images")
      .then((d) => {
        const images = Array.isArray(d) ? d : (d.images || []);
        const totalMb = images.reduce((sum: number, img: any) => {
          const size = img.size || img.Size || "0";
          if (typeof size === "number") return sum + size / (1024 * 1024);
          const match = String(size).match(/([\d.]+)\s*(GB|MB|KB)/i);
          if (match) {
            const val = parseFloat(match[1]);
            if (match[2].toUpperCase() === "GB") return sum + val * 1024;
            if (match[2].toUpperCase() === "MB") return sum + val;
            return sum + val / 1024;
          }
          return sum;
        }, 0);
        setDockerDiskUsage(totalMb > 1024 ? `${(totalMb / 1024).toFixed(1)} GB` : `${totalMb.toFixed(0)} MB`);
      })
      .catch(() => {});
    // Feature #3: Disk I/O metrics (endpoint takes ~1s due to sampling)
    api
      .get<any>("/system/disk-io")
      .then(setDiskIo)
      .catch(() => {});
    // Feature #9: Mail queue count
    api
      .get<any>("/mail/queue")
      .then((d) => {
        const count = d.count ?? (Array.isArray(d.queue) ? d.queue.length : (Array.isArray(d) ? d.length : 0));
        setMailQueue(count);
      })
      .catch(() => {});
  }, []);

  // Fetch real-time system endpoints (only needed when WS is disconnected)
  const fetchRealtimeData = useCallback(() => {
    api
      .get<SystemInfo>("/system/info")
      .then(setSystem)
      .catch((e) => setError(e instanceof Error ? e.message : "Failed to load system info"));
    api
      .get<Process[]>("/system/processes")
      .then(setProcesses)
      .catch(() => console.warn("Optional: failed to load processes"));
    api
      .get<NetworkIface[]>("/system/network")
      .then(setNetwork)
      .catch(() => console.warn("Optional: failed to load network interfaces"));
  }, []);

  // WebSocket connection for live metrics
  useEffect(() => {
    function connect() {
      if (reconnectTimer.current) {
        clearTimeout(reconnectTimer.current);
        reconnectTimer.current = null;
      }
      const protocol = window.location.protocol === "https:" ? "wss:" : "ws:";
      const ws = new WebSocket(`${protocol}//${window.location.host}/api/ws/metrics`);

      ws.onopen = () => {
        setWsConnected(true);
        wsConnectedRef.current = true;
      };

      ws.onclose = () => {
        setWsConnected(false);
        wsConnectedRef.current = false;
        wsRef.current = null;
        // Reconnect after 3 seconds
        reconnectTimer.current = setTimeout(connect, 3000);
      };

      ws.onerror = () => {
        setWsConnected(false);
        wsConnectedRef.current = false;
      };

      ws.onmessage = (event) => {
        try {
          const data = JSON.parse(event.data);
          if (data.type === "metrics") {
            if (data.system) setSystem(data.system);
            if (data.processes) setProcesses(data.processes);
            if (data.network) {
              setNetwork(data.network);
              // Feature #6: Track cumulative bandwidth
              const rx = (data.network as NetworkIface[]).reduce((sum, iface) => sum + (iface.rx_bytes || 0), 0);
              const tx = (data.network as NetworkIface[]).reduce((sum, iface) => sum + (iface.tx_bytes || 0), 0);
              setBandwidthTotal({ rx, tx });
            }
          }
        } catch {
          console.warn("Failed to parse WebSocket message");
        }
      };

      wsRef.current = ws;
    }

    connect();

    return () => {
      if (reconnectTimer.current) clearTimeout(reconnectTimer.current);
      if (wsRef.current) {
        // Prevent reconnect on intentional close
        wsRef.current.onclose = null;
        wsRef.current.close();
      }
    };
  }, []);

  // Polling logic — interval depends on WebSocket state
  useEffect(() => {
    // Initial fetch of everything
    fetchSlowData();
    if (!wsConnectedRef.current) fetchRealtimeData();

    const tick = () => {
      fetchSlowData();
      // Only poll real-time endpoints when WS is disconnected
      if (!wsConnectedRef.current) fetchRealtimeData();
    };

    // WS connected: poll slow data every 15s; disconnected: poll everything every 5s
    const interval = setInterval(tick, wsConnected ? 15000 : 5000);
    return () => clearInterval(interval);
  }, [wsConnected, fetchSlowData, fetchRealtimeData]);

  // Feature #7: Disk full prediction based on historical usage trend
  const diskForecast = useMemo(() => {
    if (!metricsHistory || metricsHistory.length < 10) return null;
    const points = metricsHistory;
    const first = points[0];
    const last = points[points.length - 1];
    // Estimate hours between first and last point (each point ~15min apart)
    const hours = (points.length - 1) * 0.25;
    if (hours < 1) return null;
    const firstDisk = first.disk;
    const lastDisk = last.disk;
    const ratePerHour = (lastDisk - firstDisk) / hours;
    if (ratePerHour <= 0) return null; // Disk not growing
    const remaining = 100 - lastDisk;
    const hoursLeft = remaining / ratePerHour;
    const daysLeft = Math.floor(hoursLeft / 24);
    return daysLeft > 365 ? null : daysLeft; // Only show if meaningful
  }, [metricsHistory]);

  // Feature #10: Visual health indicator
  const overallStatus = useMemo(() => {
    const alertCount = intel?.firing_alerts ?? 0;
    const healthScore = intel?.health_score ?? 100;
    if (alertCount > 0 || healthScore < 50)
      return { label: "System Issues Detected", color: "bg-danger-500/10 border-danger-500/20 text-danger-400", dot: "bg-danger-400" };
    if (healthScore < 80)
      return { label: "Degraded Performance", color: "bg-warn-500/10 border-warn-500/20 text-warn-400", dot: "bg-warn-500" };
    return { label: "All Systems Operational", color: "bg-rust-500/10 border-rust-500/20 text-rust-400", dot: "bg-rust-500" };
  }, [intel]);

  // Feature #6: Also update bandwidth when network changes via polling
  useEffect(() => {
    if (network.length > 0) {
      const rx = network.reduce((sum, iface) => sum + (iface.rx_bytes || 0), 0);
      const tx = network.reduce((sum, iface) => sum + (iface.tx_bytes || 0), 0);
      setBandwidthTotal({ rx, tx });
    }
  }, [network]);

  return (
    <div className="p-4 sm:p-6 lg:p-8 animate-fade-up">
      <div className="page-header">
        <div className="flex items-center gap-3">
          <div>
            <h1 className="page-header-title">Dashboard</h1>
            <p className="text-xs text-dark-400 mt-0.5">{system?.hostname || "Loading..."}</p>
          </div>
          <span className="flex items-center gap-1.5" title={wsConnected ? "Receiving live metrics via WebSocket" : "Polling metrics via HTTP"}>
            <span className={`w-1.5 h-1.5 rounded-full ${wsConnected ? "bg-rust-500 animate-pulse" : "bg-dark-400"}`} />
            <span className="text-[10px] text-dark-400 font-mono">{wsConnected ? "Live" : "Polling"}</span>
          </span>
        </div>
        <div className="flex items-center gap-2 flex-wrap">
          {intel && (
            <span className={`hidden sm:flex items-center gap-1.5 px-2.5 py-1.5 rounded-lg border text-xs font-medium ${overallStatus.color}`}>
              <span className={`w-1.5 h-1.5 rounded-full ${overallStatus.dot} ${overallStatus.dot === "bg-rust-500" ? "animate-pulse" : ""}`} />
              {overallStatus.label}
            </span>
          )}
          <div className="h-4 w-px bg-dark-600 hidden sm:block" />
          <button onClick={() => setShowWidgetConfig(!showWidgetConfig)}
            className="px-3 py-1.5 bg-dark-800 text-dark-300 hover:bg-dark-700 hover:text-dark-100 border border-dark-600 rounded-lg text-xs transition-colors">
            {showWidgetConfig ? "Done" : "Customize"}
          </button>
          <div className="h-4 w-px bg-dark-600 hidden sm:block" />
          <Link to="/apps" className="hidden sm:flex px-3 py-1.5 bg-dark-800 text-dark-300 hover:bg-dark-700 hover:text-dark-100 border border-dark-600 rounded-lg text-xs font-medium items-center gap-1.5 transition-colors">
            <svg className="w-3.5 h-3.5" fill="none" viewBox="0 0 24 24" stroke="currentColor" strokeWidth={2}><path strokeLinecap="round" strokeLinejoin="round" d="M12 4.5v15m7.5-7.5h-15" /></svg>
            Deploy App
          </Link>
          <Link to="/sites" className="px-3 py-1.5 bg-dark-800 text-dark-300 hover:bg-dark-700 hover:text-dark-100 border border-dark-600 rounded-lg text-xs font-medium flex items-center gap-1.5 transition-colors">
            <svg className="w-3.5 h-3.5" fill="none" viewBox="0 0 24 24" stroke="currentColor" strokeWidth={2}><path strokeLinecap="round" strokeLinejoin="round" d="M12 4.5v15m7.5-7.5h-15" /></svg>
            Add Site
          </Link>
          <Link to="/security" className="hidden sm:flex px-3 py-1.5 bg-dark-800 text-dark-300 hover:bg-dark-700 hover:text-dark-100 border border-dark-600 rounded-lg text-xs font-medium items-center gap-1.5 transition-colors">
            <svg className="w-3.5 h-3.5" fill="none" viewBox="0 0 24 24" stroke="currentColor" strokeWidth={2}><path strokeLinecap="round" strokeLinejoin="round" d="M21.75 6.75a4.5 4.5 0 01-4.884 4.484c-1.076-.091-2.264.071-2.95.904l-7.152 8.684a2.548 2.548 0 11-3.586-3.586l8.684-7.152c.833-.686.995-1.874.904-2.95a4.5 4.5 0 016.336-4.486l-3.276 3.276a3.004 3.004 0 002.25 2.25l3.276-3.276c.256.565.398 1.192.398 1.852z" /></svg>
            Diagnostics
          </Link>
          {/* Feature #4: Quick Server Actions — hidden on mobile */}
          <div className="h-4 w-px bg-dark-600 hidden sm:block" />
          <button onClick={async () => {
            if (!confirm("Restart Nginx? This will briefly interrupt all web traffic.")) return;
            try { await api.post("/agent/diagnostics/fix", { fix: "restart_nginx" }); setActionMessage({ text: "Nginx restarted", type: "success" }); setTimeout(() => setActionMessage(null), 3000); }
            catch { setActionMessage({ text: "Failed to restart Nginx", type: "error" }); setTimeout(() => setActionMessage(null), 3000); }
          }} className="hidden sm:inline-block px-3 py-1.5 bg-dark-800 text-dark-300 hover:bg-dark-700 hover:text-dark-100 border border-dark-600 rounded-lg text-xs transition-colors">
            Restart Nginx
          </button>
          <button onClick={async () => {
            if (!confirm("Restart PHP-FPM? Active requests may be interrupted.")) return;
            try { await api.post("/agent/diagnostics/fix", { fix: "restart_php" }); setActionMessage({ text: "PHP-FPM restarted", type: "success" }); setTimeout(() => setActionMessage(null), 3000); }
            catch { setActionMessage({ text: "Failed to restart PHP-FPM", type: "error" }); setTimeout(() => setActionMessage(null), 3000); }
          }} className="hidden sm:inline-block px-3 py-1.5 bg-dark-800 text-dark-300 hover:bg-dark-700 hover:text-dark-100 border border-dark-600 rounded-lg text-xs transition-colors">
            Restart PHP
          </button>
          <button onClick={async () => {
            if (!confirm("REBOOT SERVER?\n\nAll services will be temporarily unavailable. Are you sure?")) return;
            try { await api.post("/system/reboot"); setActionMessage({ text: "Server rebooting...", type: "success" }); setTimeout(() => setActionMessage(null), 5000); }
            catch { setActionMessage({ text: "Failed to reboot server", type: "error" }); setTimeout(() => setActionMessage(null), 3000); }
          }} className="hidden sm:inline-block px-2.5 py-1.5 bg-danger-500/10 border border-danger-500/20 rounded-lg text-xs text-danger-400 hover:bg-danger-500/20">
            Reboot
          </button>
        </div>
      </div>

      {/* Feature #6: Widget customization panel */}
      {showWidgetConfig && (
        <div className="bg-dark-800 rounded-lg border border-dark-500 p-4 mb-4">
          <h3 className="text-xs font-medium text-dark-300 uppercase font-mono tracking-widest mb-3">Dashboard Widgets</h3>
          <div className="grid grid-cols-2 md:grid-cols-4 gap-2">
            {[
              { id: "metrics", label: "CPU / Memory / Disk" },
              { id: "disk_io", label: "Disk I/O" },
              { id: "charts", label: "Historical Charts" },
              { id: "status_bar", label: "Status Bar" },
              { id: "health_banner", label: "Health Indicator" },
              { id: "sites_grid", label: "Sites Grid" },
              { id: "activity", label: "Recent Activity" },
              { id: "issues", label: "Active Issues" },
              { id: "ssl_countdown", label: "SSL Countdown" },
              { id: "network", label: "Network I/O" },
              { id: "processes", label: "Top Processes" },
              { id: "system_info", label: "System Info" },
              { id: "onboarding", label: "Getting Started" },
              { id: "bookmarks", label: "Quick Links" },
            ].map(w => (
              <label key={w.id} className="flex items-center gap-2 text-xs text-dark-200 cursor-pointer hover:text-dark-100">
                <input type="checkbox" checked={isVisible(w.id)} onChange={() => toggleWidget(w.id)}
                  className="w-3.5 h-3.5 accent-rust-500" />
                {w.label}
              </label>
            ))}
          </div>
          <button onClick={() => { setWidgetConfig({}); localStorage.removeItem('dp-dashboard-widgets'); }}
            className="mt-3 text-[10px] text-dark-400 hover:text-dark-200">Reset to defaults</button>
        </div>
      )}

      {/* Health banner moved to page header */}

      {/* Feature #4: Action message toast */}
      {actionMessage && (
        <div className={`rounded-lg border px-4 py-2.5 mb-6 text-sm font-medium ${
          actionMessage.type === "success" ? "bg-rust-500/10 border-rust-500/20 text-rust-400" : "bg-danger-500/10 border-danger-500/20 text-danger-400"
        }`}>
          {actionMessage.text}
        </div>
      )}

      {error && (
        <div className="bg-danger-500/10 text-danger-400 text-sm px-4 py-3 rounded-lg border border-danger-500/20 mb-6">
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
      {isVisible("onboarding") && !dismissed && system && (() => {
        const steps: OnboardingStep[] = [
          { id: "site", label: "Create your first site", description: "Set up a website with Nginx, PHP, or reverse proxy", link: "/sites", check: () => sites.total > 0 },
          { id: "app", label: "Deploy a Docker app", description: "One-click deploy from 34 templates", link: "/apps", check: () => appCount > 0 },
          { id: "2fa", label: "Enable 2FA", description: "Protect your panel with two-factor authentication", link: "/settings", check: () => twoFaEnabled },
          { id: "backup", label: "Set up backups", description: "Set up backups for any site", link: sitesList.length > 0 ? `/sites/${sitesList[0].id}` : "/sites", check: () => sitesList.some(s => !!s.backup_schedule) },
          { id: "diagnostics", label: "Run diagnostics", description: "Check your server health and fix issues", link: "/diagnostics", check: () => true },
        ];
        const completed = steps.filter(s => s.check()).length;
        const isCollapsed = onboardingCollapsed || (completed >= 3 && localStorage.getItem('dp-onboarding-collapsed') !== '0');
        const toggleCollapse = () => {
          const next = !isCollapsed;
          setOnboardingCollapsed(next);
          localStorage.setItem('dp-onboarding-collapsed', next ? '1' : '0');
        };

        if (isCollapsed) {
          return (
            <div className="mb-6 bg-dark-800 border border-dark-500 rounded-lg px-5 py-3 animate-fade-up flex items-center justify-between">
              <div className="flex items-center gap-3">
                <span className="text-xs font-medium text-dark-200">Setup: {completed}/{steps.length} complete</span>
                <div className="flex gap-1">
                  {steps.map(step => (
                    <div key={step.id} className={`w-2 h-2 rounded-full ${step.check() ? "bg-rust-500" : "bg-dark-600"}`} />
                  ))}
                </div>
              </div>
              <div className="flex items-center gap-3">
                <button onClick={toggleCollapse} className="text-rust-400 hover:text-rust-300 text-xs font-medium">Expand</button>
                <button onClick={dismissOnboarding} className="text-dark-400 hover:text-dark-300 text-xs">Dismiss</button>
              </div>
            </div>
          );
        }

        return (
          <div className="mb-6 bg-dark-800 border border-dark-500 rounded-lg p-5 animate-fade-up">
            <div className="flex items-center justify-between mb-1">
              <h3 className="text-sm font-bold text-dark-50 typewriter inline-block">Welcome to DockPanel</h3>
              <div className="flex items-center gap-3">
                <button onClick={toggleCollapse} className="text-dark-300 hover:text-dark-200 text-xs shrink-0">Collapse</button>
                <button onClick={dismissOnboarding} className="text-dark-300 hover:text-dark-200 text-xs shrink-0">Dismiss</button>
              </div>
            </div>
            <p className="text-xs text-dark-300 mb-4">Complete these steps to set up your server. <span className="text-dark-200">{completed}/{steps.length} done</span></p>
            <div className="grid grid-cols-1 sm:grid-cols-2 lg:grid-cols-5 gap-3 stagger-children">
              {steps.map(step => (
                <Link
                  key={step.id}
                  to={step.link}
                  className={`border p-4 transition-all hover-lift ${
                    step.check()
                      ? "border-rust-500/30 bg-dark-900/50 opacity-60"
                      : "border-dark-500 bg-dark-900/50 hover:border-rust-500/40"
                  }`}
                >
                  <div className="flex items-center gap-2 mb-2">
                    {step.check() ? (
                      <div className="w-5 h-5 rounded-full bg-rust-500 flex items-center justify-center">
                        <svg className="w-3 h-3 text-white" fill="none" viewBox="0 0 24 24" stroke="currentColor" strokeWidth={3}><path strokeLinecap="round" strokeLinejoin="round" d="M5 13l4 4L19 7" /></svg>
                      </div>
                    ) : (
                      <div className="w-5 h-5 border border-dark-400 flex items-center justify-center">
                        <span className="text-[8px] text-dark-300 font-bold">{steps.indexOf(step) + 1}</span>
                      </div>
                    )}
                    <span className={`text-xs font-medium ${step.check() ? "text-rust-400" : "text-dark-50"}`}>{step.label}</span>
                  </div>
                  <p className="text-[10px] text-dark-300 leading-relaxed mb-2">{step.description}</p>
                  {!step.check() && (
                    <span className="text-[10px] text-rust-500 font-medium">Start &rarr;</span>
                  )}
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
          {/* System Information */}
          {isVisible("system_info") && <div className="hidden sm:grid grid-cols-2 sm:grid-cols-3 md:grid-cols-6 gap-px bg-dark-600 border border-dark-500 rounded-lg overflow-hidden mb-6">
            {[
              ["Hostname", system.hostname],
              ["OS", system.os],
              ["Kernel", system.kernel],
              ["Processor", system.cpu_model],
              ["Temperature", system.cpu_temp != null ? `${system.cpu_temp.toFixed(0)}°C` : "N/A"],
              ["Processes", system.process_count.toLocaleString()],
            ].map(([label, value]) => (
              <div key={label} className="bg-dark-800 px-4 py-3 flex flex-col card-interactive">
                <span className="text-[10px] text-dark-300 uppercase tracking-widest mb-1">{label}</span>
                <span title={String(value)} className={`text-sm text-dark-50 font-medium truncate ${label === "Temperature" && system.cpu_temp != null ? tempColor(system.cpu_temp) : ""}`}>{value}</span>
              </div>
            ))}
          </div>}

          {/* Resource Metrics — 3 column */}
          {isVisible("metrics") && <div className="grid grid-cols-1 sm:grid-cols-3 gap-4 mb-6 stagger-children">
            {[
              { label: "CPU Usage", pct: system.cpu_usage, type: "cpu" as const, detail: `${system.cpu_count} cores${system.load_avg_1 !== undefined ? ` · Load ${system.load_avg_1?.toFixed(2)}` : ""}`,
                icon: <svg className="w-4 h-4" fill="none" viewBox="0 0 24 24" stroke="currentColor" strokeWidth={1.5}><rect x="6" y="6" width="12" height="12" rx="1" /><path d="M9 1v4m6-4v4M9 19v4m6-4v4M1 9h4m-4 6h4M19 9h4m-4 6h4" strokeLinecap="round" /></svg> },
              { label: "Memory", pct: system.mem_usage_pct, type: "mem" as const, detail: `${(system.mem_used_mb / 1024).toFixed(1)} / ${(system.mem_total_mb / 1024).toFixed(1)} GB${system.swap_total_mb > 0 ? ` · Swap ${(system.swap_used_mb / 1024).toFixed(1)}G` : ""}`,
                icon: <svg className="w-4 h-4" fill="none" viewBox="0 0 24 24" stroke="currentColor" strokeWidth={1.5}><rect x="3" y="4" width="18" height="16" rx="1" /><path d="M7 4v3m4-3v3m4-3v3M3 10h18" strokeLinecap="round" /></svg> },
              { label: "Disk", pct: system.disk_usage_pct, type: "disk" as const, detail: `${system.disk_used_gb.toFixed(0)} / ${system.disk_total_gb.toFixed(0)} GB · ${(system.disk_total_gb - system.disk_used_gb).toFixed(0)} GB free${dockerDiskUsage ? ` · Images: ${dockerDiskUsage}` : ""}`,
                icon: <svg className="w-4 h-4" fill="none" viewBox="0 0 24 24" stroke="currentColor" strokeWidth={1.5}><path strokeLinecap="round" strokeLinejoin="round" d="M21.75 17.25v-.228a4.5 4.5 0 0 0-.12-1.03l-2.268-9.64a3.375 3.375 0 0 0-3.285-2.602H7.923a3.375 3.375 0 0 0-3.285 2.602l-2.268 9.64a4.5 4.5 0 0 0-.12 1.03v.228m19.5 0a3 3 0 0 1-3 3H5.25a3 3 0 0 1-3-3m19.5 0a3 3 0 0 0-3-3H5.25a3 3 0 0 0-3 3m16.5 0h.008v.008h-.008v-.008Zm-3 0h.008v.008h-.008v-.008Z" /></svg> },
            ].map(({ label, pct, type, detail, icon }) => (
              <div key={label} className="border border-dark-600 bg-dark-800 rounded-lg p-5 relative overflow-hidden shadow-lg shadow-black/10 elevation-1">
                <div className={`absolute inset-0 opacity-[0.03] ${pct < 60 ? "bg-dark-50" : pct < 85 ? "bg-warn-500" : "bg-danger-500"}`} />
                <div className="relative text-center">
                  <div className="flex items-center justify-center gap-1.5 text-dark-200 mb-1">
                    <span className="opacity-60">{icon}</span>
                    <span className="text-xs uppercase tracking-widest font-medium">{label}</span>
                  </div>
                  <div className={`text-5xl font-bold font-mono my-2 ${pctColor(pct)}`}>
                    <CountUp value={pct} /><span className="text-xl text-dark-300 ml-0.5">%</span>
                  </div>
                  <div className="h-2 bg-dark-700 rounded-full overflow-hidden mt-3 mx-auto max-w-[80%]">
                    <div className={`h-full rounded-full transition-all duration-500 ${barColor(pct, type)}`} style={{ width: `${Math.min(pct, 100)}%` }} />
                  </div>
                  <p className="text-xs text-dark-300 mt-3">{detail}</p>
                  {/* Feature #7: Disk full forecast */}
                  {label === "Disk" && diskForecast !== null && diskForecast < 30 && (
                    <p className={`text-[10px] mt-1 font-medium ${diskForecast < 7 ? "text-danger-400" : "text-warn-400"}`}>
                      Disk full in ~{diskForecast}d at current rate
                    </p>
                  )}
                </div>
              </div>
            ))}
          </div>}

          {/* Disk I/O moved into stat bar below */}

          {/* Historical Charts */}
          {isVisible("charts") && metricsHistory.length >= 2 && (
            <div className="grid grid-cols-1 sm:grid-cols-3 gap-4 mb-6 animate-fade-up">
              {[
                { label: "CPU", data: metricsHistory.map(p => p.cpu), color: "var(--color-rust-500)" },
                { label: "Memory", data: metricsHistory.map(p => p.mem), color: "var(--color-accent-500)" },
                { label: "Disk", data: metricsHistory.map(p => p.disk), color: "var(--color-warn-500)" },
              ].map(({ label, data, color }) => (
                <div key={label} className="border border-dark-500 bg-dark-800 rounded-lg p-4 elevation-1">
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
          {isVisible("status_bar") && <div className="grid grid-cols-2 sm:grid-cols-3 md:grid-cols-4 lg:grid-cols-5 gap-px bg-dark-600 border border-dark-600 rounded-lg overflow-hidden mb-6 shadow-sm shadow-black/5 stagger-children">
            <div className="bg-dark-800 px-4 py-3 flex flex-col card-interactive">
              <span className="text-[10px] text-dark-300 uppercase tracking-widest mb-1">Uptime</span>
              <span className="text-sm text-dark-50 font-medium">{formatUptime(system.uptime_secs)}</span>
            </div>
            <div className="bg-dark-800 px-4 py-3 flex flex-col card-interactive">
              <span className="text-[10px] text-dark-300 uppercase tracking-widest mb-1">Sites</span>
              <span className="text-sm text-dark-50 font-medium">{sites.total}{sites.active > 0 && <span className="text-rust-400 ml-1 text-xs">({sites.active} active)</span>}</span>
            </div>
            <div className="bg-dark-800 px-4 py-3 flex flex-col card-interactive">
              <span className="text-[10px] text-dark-300 uppercase tracking-widest mb-1">Databases</span>
              <span className="text-sm text-dark-50 font-medium">{dbCount}</span>
            </div>
            {/* Feature #1: Docker container overview */}
            <div className="bg-dark-800 px-4 py-3 flex flex-col card-interactive">
              <span className="text-[10px] text-dark-300 uppercase tracking-widest mb-1">Docker</span>
              <span className="text-sm text-dark-50 font-medium">
                {dockerInfo?.running ?? 0}<span className="text-xs text-dark-300 font-normal">/{dockerInfo?.total ?? 0}</span>
                <span className="text-[10px] text-dark-400 ml-1">running</span>
              </span>
            </div>
            {intel && <>
              <div className={`px-4 py-3 flex flex-col card-interactive ${
                intel.health_score < 60 ? "bg-danger-500/5" : "bg-dark-800"
              }`}>
                <span className="text-[10px] text-dark-300 uppercase tracking-widest mb-1">Health</span>
                <span className={`text-sm font-bold ${
                  intel.health_score >= 90 ? "text-rust-400" :
                  intel.health_score >= 75 ? "text-accent-400" :
                  intel.health_score >= 60 ? "text-warn-400" : "text-danger-400"
                }`}>{intel.health_score}/100 {intel.grade}</span>
              </div>
              <div className={`px-4 py-3 flex flex-col card-interactive ${
                intel.firing_alerts > 0 ? "bg-danger-500/5" : "bg-dark-800"
              }`}>
                <span className="text-[10px] text-dark-300 uppercase tracking-widest mb-1">Alerts</span>
                {intel.firing_alerts > 0
                  ? <span className="text-sm text-danger-400 font-bold">{intel.firing_alerts} firing</span>
                  : <span className="text-sm text-rust-400 font-medium">0</span>
                }
              </div>
              <div className="bg-dark-800 px-4 py-3 flex flex-col card-interactive">
                <span className="text-[10px] text-dark-300 uppercase tracking-widest mb-1">SSL</span>
                <span className="text-sm text-dark-50 font-medium">{intel.ssl_countdowns.length} certs</span>
              </div>
              <div className={`px-4 py-3 flex flex-col card-interactive ${
                intel.open_incidents > 0 ? "bg-danger-500/5" : "bg-dark-800"
              }`}>
                <span className="text-[10px] text-dark-300 uppercase tracking-widest mb-1">Incidents</span>
                {intel.open_incidents > 0
                  ? <span className="text-sm text-danger-400 font-bold">{intel.open_incidents} open</span>
                  : <span className="text-sm text-rust-400 font-medium">0</span>
                }
              </div>
              <div className={`px-4 py-3 flex flex-col card-interactive ${
                intel.stale_backups > 0 ? "bg-warn-500/5" : "bg-dark-800"
              }`}>
                <span className="text-[10px] text-dark-300 uppercase tracking-widest mb-1">Backups</span>
                {intel.stale_backups > 0
                  ? <span className="text-sm text-warn-400 font-bold">{intel.stale_backups} stale</span>
                  : <span className="text-sm text-rust-400 font-medium">fresh</span>
                }
              </div>
            </>}
            <div className="bg-dark-800 px-4 py-3 flex flex-col card-interactive">
              <span className="text-[10px] text-dark-300 uppercase tracking-widest mb-1">Updates</span>
              {updateCount > 0
                ? <span className="text-sm text-warn-400 font-bold">{updateCount} available</span>
                : <span className="text-sm text-rust-400 font-medium">up to date</span>
              }
            </div>
            {/* Disk I/O */}
            {diskIo && <>
              <div className="bg-dark-800 px-4 py-3 flex flex-col card-interactive">
                <span className="text-[10px] text-dark-300 uppercase tracking-widest mb-1">Disk Read</span>
                <span className="text-sm text-dark-50 font-medium font-mono">{formatRate(diskIo.read_bytes_sec)}</span>
              </div>
              <div className="bg-dark-800 px-4 py-3 flex flex-col card-interactive">
                <span className="text-[10px] text-dark-300 uppercase tracking-widest mb-1">Disk Write</span>
                <span className="text-sm text-dark-50 font-medium font-mono">{formatRate(diskIo.write_bytes_sec)}</span>
              </div>
            </>}
            {/* Feature #6: Bandwidth usage summary */}
            <div className="bg-dark-800 px-4 py-3 flex flex-col card-interactive">
              <span className="text-[10px] text-dark-300 uppercase tracking-widest mb-1">Bandwidth</span>
              <span className="text-sm text-dark-50 font-medium">
                <span className="text-dark-400">{"\u2193"}</span>{formatSize(bandwidthTotal.rx)}
                <span className="text-dark-400 ml-1">{"\u2191"}</span>{formatSize(bandwidthTotal.tx)}
              </span>
            </div>
            {/* Feature #9: Mail queue widget */}
            <div className="bg-dark-800 px-4 py-3 flex flex-col card-interactive">
              <span className="text-[10px] text-dark-300 uppercase tracking-widest mb-1">Mail Queue</span>
              <span className={`text-sm font-medium ${(mailQueue ?? 0) > 0 ? "text-warn-400" : "text-dark-50"}`}>
                {mailQueue ?? 0} <span className="text-[10px] text-dark-400">messages</span>
              </span>
            </div>
          </div>}

          {/* Reboot Required Warning */}
          {rebootRequired && (
            <div className="border border-warn-500/50 bg-warn-500/5 p-4 mb-6 flex items-start gap-3">
              <svg className="w-5 h-5 text-warn-400 shrink-0 mt-0.5" fill="none" viewBox="0 0 24 24" stroke="currentColor" strokeWidth={1.5}><path strokeLinecap="round" strokeLinejoin="round" d="M12 9v3.75m-9.303 3.376c-.866 1.5.217 3.374 1.948 3.374h14.71c1.73 0 2.813-1.874 1.948-3.374L13.949 3.378c-.866-1.5-3.032-1.5-3.898 0L2.697 16.126ZM12 15.75h.007v.008H12v-.008Z" /></svg>
              <div className="flex-1">
                <p className="text-sm text-warn-400 font-bold">Reboot Required</p>
                <p className="text-xs text-dark-300 mt-1">Recent package updates (such as a new kernel version) require a reboot to be fully applied.</p>
              </div>
              <Link to="/updates" className="px-4 py-2 bg-warn-500 text-dark-900 text-xs font-bold uppercase tracking-wider hover:bg-warn-400 transition-colors shrink-0">
                View Updates
              </Link>
            </div>
          )}

          {/* Feature #5: Site Status Mini-Grid */}
          {isVisible("sites_grid") && sitesList.length > 0 && (
            <div className="bg-dark-800 rounded-lg border border-dark-500 overflow-hidden mb-6">
              <div className="px-4 py-2.5 border-b border-dark-600 flex justify-between items-center">
                <h3 className="text-xs font-medium text-dark-300 uppercase font-mono tracking-widest">Sites</h3>
                <Link to="/sites" className="text-[10px] text-rust-400 hover:text-rust-300">Manage</Link>
              </div>
              <div className="grid grid-cols-2 md:grid-cols-3 lg:grid-cols-4 gap-px bg-dark-600">
                {sitesList.slice(0, 12).map((s) => (
                  <Link key={s.id} to={`/sites/${s.id}`} className="bg-dark-800 px-3 py-2 flex items-center gap-2 hover:bg-dark-700/50 transition-colors">
                    <div className={`w-2 h-2 rounded-full shrink-0 ${s.status === "active" ? "bg-rust-500" : "bg-dark-500"}`} />
                    <span className="text-xs text-dark-100 truncate font-mono">{(s as any).domain || s.id}</span>
                  </Link>
                ))}
              </div>
            </div>
          )}

          {/* Feature #2: Recent Activity Feed */}
          {isVisible("activity") && recentActivity.length > 0 && (
            <div className="bg-dark-800 rounded-lg border border-dark-500 overflow-hidden mb-6">
              <div className="px-4 py-2.5 border-b border-dark-600 flex justify-between items-center">
                <h3 className="text-xs font-medium text-dark-300 uppercase font-mono tracking-widest">Recent Activity</h3>
                <Link to="/logs" className="text-[10px] text-rust-400 hover:text-rust-300">View all</Link>
              </div>
              <div className="divide-y divide-dark-600">
                {recentActivity.map((a: any, i: number) => (
                  <div key={i} className="px-4 py-2 flex items-center justify-between text-xs">
                    <div className="flex items-center gap-2 min-w-0">
                      <span className={`px-1.5 py-0.5 rounded text-[10px] font-medium shrink-0 ${
                        a.action?.includes("create") || a.action?.includes("deploy") ? "bg-rust-500/15 text-rust-400" :
                        a.action?.includes("delete") || a.action?.includes("remove") ? "bg-danger-500/15 text-danger-400" :
                        "bg-dark-700 text-dark-200"
                      }`}>{a.action}</span>
                      {a.target_name && <span className="text-dark-100 font-mono truncate">{a.target_name}</span>}
                    </div>
                    <span className="text-dark-400 text-[10px] shrink-0 ml-2">{timeAgo(a.created_at)}</span>
                  </div>
                ))}
              </div>
            </div>
          )}

          {/* Active Issues + SSL — side by side */}
          {isVisible("issues") && intel && (intel.top_issues.length > 0 || intel.ssl_countdowns.length > 0) && (
            <div className="grid grid-cols-1 lg:grid-cols-2 gap-4 mb-6">
              {intel.top_issues.length > 0 && (
                <div className="border border-dark-500 bg-dark-800 rounded-lg p-4">
                  <div className="flex items-center justify-between mb-3">
                    <h3 className="text-xs text-dark-300 uppercase tracking-widest">Active Issues</h3>
                    <Link to="/monitors" className="text-xs text-rust-400 hover:text-rust-300">View all</Link>
                  </div>
                  <div className="space-y-2">
                    {intel.top_issues.slice(0, 4).map((issue, i) => (
                      <div key={i} className="flex items-start gap-2">
                        <div className={`w-2 h-2 rounded-full mt-1.5 flex-shrink-0 ${
                          issue.severity === "critical" ? "bg-danger-500" :
                          issue.severity === "warning" ? "bg-warn-500" : "bg-accent-500"
                        }`} />
                        <p className="text-xs text-dark-100 leading-tight">{issue.title}</p>
                      </div>
                    ))}
                  </div>
                </div>
              )}
              {intel.ssl_countdowns.length > 0 && (
                <div className="border border-dark-500 bg-dark-800 rounded-lg p-4">
                  <h3 className="text-xs text-dark-300 uppercase tracking-widest mb-3">SSL Certificates</h3>
                  <div className="space-y-2">
                    {intel.ssl_countdowns.map((ssl, i) => (
                      <div key={i} className="flex items-center justify-between">
                        <span className="text-xs text-dark-100 truncate max-w-[200px]">{ssl.domain}</span>
                        <span className={`text-xs ${
                          ssl.severity === "critical" ? "text-danger-400" :
                          ssl.severity === "warning" ? "text-warn-400" :
                          ssl.severity === "info" ? "text-accent-400" : "text-rust-400"
                        }`}>{ssl.days_left}d left</span>
                      </div>
                    ))}
                  </div>
                </div>
              )}
            </div>
          )}

          {/* Smart Recommendations */}
          {intel && intel.recommendations && intel.recommendations.length > 0 && (
            <div className="border border-dark-500 bg-dark-800 rounded-lg overflow-hidden mb-6">
              <div className="px-4 py-2.5 border-b border-dark-600 flex justify-between items-center">
                <h3 className="text-xs font-medium text-dark-300 uppercase font-mono tracking-widest">Recommendations</h3>
                <span className="text-[10px] text-dark-400">{intel.recommendations.length} item{intel.recommendations.length !== 1 ? "s" : ""}</span>
              </div>
              <div className="divide-y divide-dark-600">
                {intel.recommendations.map((rec, i) => (
                  <div key={i} className="px-4 py-3 flex items-start gap-3">
                    <div className={`w-2 h-2 rounded-full mt-1.5 flex-shrink-0 ${
                      rec.severity === "critical" ? "bg-danger-500" :
                      rec.severity === "warning" ? "bg-warn-500" : "bg-accent-500"
                    }`} />
                    <div className="flex-1 min-w-0">
                      <p className={`text-xs leading-tight ${
                        rec.severity === "critical" ? "text-danger-400" :
                        rec.severity === "warning" ? "text-warn-400" : "text-dark-100"
                      }`}>{rec.message}</p>
                    </div>
                    <span className={`text-[10px] px-1.5 py-0.5 rounded uppercase tracking-wider font-bold flex-shrink-0 ${
                      rec.severity === "critical" ? "bg-danger-500/10 text-danger-400" :
                      rec.severity === "warning" ? "bg-warn-500/10 text-warn-400" : "bg-accent-500/10 text-accent-400"
                    }`}>{rec.severity}</span>
                  </div>
                ))}
              </div>
            </div>
          )}

          {/* System Information — moved to above metrics */}

          {/* Feature #12: Quick Links / Bookmarks */}
          {(bookmarks.length > 0 || showAddBookmark) && isVisible("bookmarks") && (
            <div className="bg-dark-800 rounded-lg border border-dark-500 overflow-hidden mb-6">
              <div className="px-4 py-2.5 border-b border-dark-600 flex justify-between items-center">
                <h3 className="text-xs font-medium text-dark-300 uppercase font-mono tracking-widest">Quick Links</h3>
                <button onClick={() => setShowAddBookmark(!showAddBookmark)} className="text-[10px] text-rust-400 hover:text-rust-300">
                  {showAddBookmark ? "Cancel" : "+ Add"}
                </button>
              </div>
              {showAddBookmark && (
                <div className="px-4 py-3 border-b border-dark-600 flex flex-col sm:flex-row gap-2">
                  <input value={bmLabel} onChange={e => setBmLabel(e.target.value)} placeholder="Label"
                    className="flex-1 px-2 py-1.5 bg-dark-900 border border-dark-500 rounded text-xs text-dark-100 placeholder-dark-400 min-h-[44px] sm:min-h-0" />
                  <input value={bmUrl} onChange={e => setBmUrl(e.target.value)} placeholder="/sites or https://..."
                    className="flex-1 px-2 py-1.5 bg-dark-900 border border-dark-500 rounded text-xs text-dark-100 placeholder-dark-400 min-h-[44px] sm:min-h-0" />
                  <button onClick={() => {
                    if (bmLabel && bmUrl) {
                      const next = [...bookmarks, { label: bmLabel, url: bmUrl }];
                      setBookmarks(next);
                      localStorage.setItem('dp-dashboard-bookmarks', JSON.stringify(next));
                      setBmLabel(""); setBmUrl(""); setShowAddBookmark(false);
                    }
                  }} className="px-3 py-1.5 bg-rust-500 text-white rounded text-xs min-h-[44px] sm:min-h-0">Add</button>
                </div>
              )}
              <div className="grid grid-cols-2 md:grid-cols-4 gap-px bg-dark-600">
                {bookmarks.map((bm, i) => (
                  <div key={i} className="bg-dark-800 px-3 py-2.5 flex items-center justify-between group">
                    <a href={/^(https?:\/\/|\/)/i.test(bm.url) ? bm.url : "#"} rel="noopener noreferrer" className="text-xs text-dark-100 hover:text-rust-400 truncate">{bm.label}</a>
                    <button onClick={() => {
                      const next = bookmarks.filter((_, j) => j !== i);
                      setBookmarks(next);
                      localStorage.setItem('dp-dashboard-bookmarks', JSON.stringify(next));
                    }} className="text-dark-500 hover:text-danger-400 text-xs opacity-0 group-hover:opacity-100 ml-2">&times;</button>
                  </div>
                ))}
              </div>
            </div>
          )}

          {/* Network & Processes */}
          <div className="grid grid-cols-1 lg:grid-cols-2 gap-6">
            {/* Network I/O */}
            {isVisible("network") && network.length > 0 && (
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
                        <td className="px-5 py-2.5 text-right font-mono">
                          <div className="text-sm text-rust-400">
                            <span className="text-dark-400 mr-0.5">{"\u2193"}</span>
                            {iface.rx_rate != null ? formatRate(iface.rx_rate) : formatSize(iface.rx_bytes)}
                          </div>
                          {iface.rx_rate != null && (
                            <div className="text-[10px] text-dark-400 mt-0.5">{formatSize(iface.rx_bytes)} total</div>
                          )}
                        </td>
                        <td className="px-5 py-2.5 text-right font-mono">
                          <div className="text-sm text-accent-400">
                            <span className="text-dark-400 mr-0.5">{"\u2191"}</span>
                            {iface.tx_rate != null ? formatRate(iface.tx_rate) : formatSize(iface.tx_bytes)}
                          </div>
                          {iface.tx_rate != null && (
                            <div className="text-[10px] text-dark-400 mt-0.5">{formatSize(iface.tx_bytes)} total</div>
                          )}
                        </td>
                      </tr>
                    ))}
                  </tbody>
                </table>
              </div>
            )}

            {/* Top Processes */}
            {isVisible("processes") && processes.length > 0 && (
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
                          <span className={p.cpu_pct > 50 ? "text-danger-400 font-medium" : "text-dark-200"}>
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
