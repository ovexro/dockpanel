import { useState, useEffect } from "react";
import { Navigate } from "react-router-dom";
import { useAuth } from "../context/AuthContext";
import { api } from "../api";
import UpdatesContent from "./Updates";

export default function System() {
  const { user } = useAuth();
  const [tab, setTab] = useState<"updates" | "health">("updates");

  if (!user || user.role !== "admin") return <Navigate to="/" replace />;

  return (
    <div className="p-6 lg:p-8">
      <div className="flex flex-col sm:flex-row sm:items-center justify-between gap-3 mb-6 pb-4 border-b border-dark-600">
        <div>
          <h1 className="text-sm font-medium text-dark-300 uppercase font-mono tracking-widest">System</h1>
          <p className="text-sm text-dark-200 mt-1">System updates, services, and health monitoring</p>
        </div>
      </div>
      <div className="flex gap-6 mb-6 text-sm font-mono overflow-x-auto">
        <button onClick={() => setTab("updates")} className={`whitespace-nowrap ${tab === "updates" ? "border-b-2 border-rust-500 text-dark-50 pb-2" : "text-dark-300 hover:text-dark-100 pb-2"}`}>Updates</button>
        <button onClick={() => setTab("health")} className={`whitespace-nowrap ${tab === "health" ? "border-b-2 border-rust-500 text-dark-50 pb-2" : "text-dark-300 hover:text-dark-100 pb-2"}`}>Health</button>
      </div>
      {tab === "updates" && <UpdatesContent />}
      {tab === "health" && <HealthContent />}
    </div>
  );
}

interface HealthData {
  status: string;
  service: string;
  version: string;
  db?: string;
}

interface SystemInfoData {
  hostname: string;
  os: string;
  kernel: string;
  uptime_secs: number;
  cpu_count: number;
  cpu_usage: number;
  cpu_model: string;
  mem_total_mb: number;
  mem_used_mb: number;
  mem_usage_pct: number;
  disk_total_gb: number;
  disk_used_gb: number;
  disk_usage_pct: number;
  load_avg_1: number;
  load_avg_5: number;
  load_avg_15: number;
  process_count: number;
}

interface GpuData {
  index: number;
  name: string;
  memory_total_mb: number;
  memory_used_mb: number;
  memory_free_mb: number;
  utilization_gpu_pct: number;
  utilization_memory_pct: number;
  temperature_c: number | null;
  power_draw_w: number | null;
  power_limit_w: number | null;
  fan_speed_pct: number | null;
  driver_version: string;
  performance_state: string;
}

interface GpuProcess {
  pid: number;
  gpu_uuid: string;
  vram_used_mb: number;
  process_name: string;
  container_name: string | null;
}

interface GpuInfoData {
  available: boolean;
  gpus: GpuData[];
  gpu_count: number;
  nvidia_toolkit_installed: boolean;
  processes: GpuProcess[];
}

function formatUptime(secs: number): string {
  const days = Math.floor(secs / 86400);
  const hours = Math.floor((secs % 86400) / 3600);
  const mins = Math.floor((secs % 3600) / 60);
  if (days > 0) return `${days}d ${hours}h ${mins}m`;
  if (hours > 0) return `${hours}h ${mins}m`;
  return `${mins}m`;
}

function HealthContent() {
  const [health, setHealth] = useState<HealthData | null>(null);
  const [sysInfo, setSysInfo] = useState<SystemInfoData | null>(null);
  const [gpuInfo, setGpuInfo] = useState<GpuInfoData | null>(null);
  const [loading, setLoading] = useState(true);
  const [error, setError] = useState("");

  useEffect(() => {
    Promise.all([
      api.get<HealthData>("/health").catch(() => null),
      api.get<SystemInfoData>("/system/info").catch(() => null),
      api.get<GpuInfoData>("/apps/gpu-info").catch(() => null),
    ]).then(([h, s, g]) => {
      if (h) setHealth(h);
      if (s) setSysInfo(s);
      if (g) setGpuInfo(g);
      if (!h && !s) setError("Failed to fetch health data");
    }).finally(() => setLoading(false));
  }, []);

  if (loading) {
    return (
      <div className="grid grid-cols-1 md:grid-cols-2 lg:grid-cols-3 gap-4">
        {[1, 2, 3, 4, 5, 6].map((i) => (
          <div key={i} className="h-24 bg-dark-700 rounded-lg animate-pulse" />
        ))}
      </div>
    );
  }

  if (error) {
    return (
      <div className="bg-dark-800 rounded-lg border border-danger-500/30 p-5">
        <p className="text-sm text-danger-400">{error}</p>
      </div>
    );
  }

  const cards: { label: string; value: string; sub?: string; color?: string }[] = [];

  if (health) {
    cards.push({
      label: "API Status",
      value: health.status === "ok" ? "Healthy" : "Degraded",
      sub: health.service,
      color: health.status === "ok" ? "text-emerald-400" : "text-warn-400",
    });
    cards.push({
      label: "Version",
      value: health.version,
    });
    cards.push({
      label: "Database",
      value: health.db === "unreachable" ? "Unreachable" : "Connected",
      color: health.db === "unreachable" ? "text-danger-400" : "text-emerald-400",
    });
  }

  if (sysInfo) {
    cards.push({
      label: "Hostname",
      value: sysInfo.hostname,
      sub: sysInfo.os,
    });
    cards.push({
      label: "Uptime",
      value: formatUptime(sysInfo.uptime_secs),
    });
    cards.push({
      label: "CPU",
      value: `${sysInfo.cpu_usage.toFixed(1)}%`,
      sub: `${sysInfo.cpu_count} cores \u00b7 load ${sysInfo.load_avg_1.toFixed(2)}`,
      color: sysInfo.cpu_usage > 90 ? "text-danger-400" : sysInfo.cpu_usage > 70 ? "text-warn-400" : "text-emerald-400",
    });
    cards.push({
      label: "Memory",
      value: `${sysInfo.mem_usage_pct.toFixed(1)}%`,
      sub: `${sysInfo.mem_used_mb.toLocaleString()} / ${sysInfo.mem_total_mb.toLocaleString()} MB`,
      color: sysInfo.mem_usage_pct > 90 ? "text-danger-400" : sysInfo.mem_usage_pct > 70 ? "text-warn-400" : "text-emerald-400",
    });
    cards.push({
      label: "Disk",
      value: `${sysInfo.disk_usage_pct.toFixed(1)}%`,
      sub: `${sysInfo.disk_used_gb.toFixed(1)} / ${sysInfo.disk_total_gb.toFixed(1)} GB`,
      color: sysInfo.disk_usage_pct > 90 ? "text-danger-400" : sysInfo.disk_usage_pct > 80 ? "text-warn-400" : "text-emerald-400",
    });
    cards.push({
      label: "Processes",
      value: String(sysInfo.process_count),
    });
  }

  return (
    <div className="space-y-6">
      <div className="grid grid-cols-1 md:grid-cols-2 lg:grid-cols-3 gap-4">
        {cards.map((c) => (
          <div key={c.label} className="bg-dark-800 rounded-lg border border-dark-600 p-4">
            <p className="text-xs text-dark-400 font-mono uppercase tracking-wider mb-1">{c.label}</p>
            <p className={`text-lg font-semibold ${c.color || "text-dark-50"}`}>{c.value}</p>
            {c.sub && <p className="text-xs text-dark-400 mt-0.5 truncate">{c.sub}</p>}
          </div>
        ))}
      </div>

      {gpuInfo && gpuInfo.available && gpuInfo.gpus.length > 0 && (
        <GpuMonitor gpuInfo={gpuInfo} />
      )}
    </div>
  );
}

interface GpuHistoryPoint {
  gpu_index: number;
  utilization: number;
  vram_pct: number;
  vram_used_mb: number;
  vram_total_mb: number;
  temperature: number | null;
  power: number | null;
  time: string;
}

function GpuHistoryChart({ gpuIndex, gpuName }: { gpuIndex: number; gpuName: string }) {
  const [points, setPoints] = useState<GpuHistoryPoint[]>([]);
  const [metric, setMetric] = useState<"utilization" | "vram_pct" | "temperature" | "power">("utilization");

  useEffect(() => {
    api.get<{ points: GpuHistoryPoint[] }>("/dashboard/gpu-metrics-history")
      .then(d => setPoints(d.points.filter(p => p.gpu_index === gpuIndex)))
      .catch(() => {});
  }, [gpuIndex]);

  if (points.length === 0) return null;

  const values = points.map(p => {
    switch (metric) {
      case "utilization": return p.utilization;
      case "vram_pct": return p.vram_pct;
      case "temperature": return p.temperature ?? 0;
      case "power": return p.power ?? 0;
    }
  });

  const max = Math.max(...values, 1);
  const chartMax = metric === "temperature" ? Math.max(max, 100) : metric === "power" ? max * 1.1 : 100;
  const unit = metric === "utilization" || metric === "vram_pct" ? "%" : metric === "temperature" ? "°C" : "W";
  const labels: Record<string, string> = {
    utilization: "Utilization",
    vram_pct: "VRAM",
    temperature: "Temperature",
    power: "Power Draw",
  };

  return (
    <div className="bg-dark-800 rounded-lg border border-dark-600 p-5">
      <div className="flex items-center justify-between mb-3">
        <h3 className="text-xs text-dark-400 font-mono uppercase tracking-wider">
          GPU {gpuIndex} History — {gpuName}
        </h3>
        <div className="flex gap-1">
          {(["utilization", "vram_pct", "temperature", "power"] as const).map(m => (
            <button key={m} onClick={() => setMetric(m)}
              className={`px-2 py-0.5 rounded text-xs font-mono ${metric === m ? "bg-rust-500/20 text-rust-400" : "text-dark-400 hover:text-dark-200"}`}>
              {labels[m]}
            </button>
          ))}
        </div>
      </div>
      <div className="relative h-32">
        <svg viewBox={`0 0 ${points.length} 100`} preserveAspectRatio="none" className="w-full h-full">
          <defs>
            <linearGradient id={`gpu-grad-${gpuIndex}`} x1="0" y1="0" x2="0" y2="1">
              <stop offset="0%" stopColor="rgb(234,88,12)" stopOpacity="0.3" />
              <stop offset="100%" stopColor="rgb(234,88,12)" stopOpacity="0" />
            </linearGradient>
          </defs>
          <path
            d={`M0,${100 - (values[0] / chartMax) * 100} ${values.map((v, i) => `L${i},${100 - (v / chartMax) * 100}`).join(" ")} L${values.length - 1},100 L0,100 Z`}
            fill={`url(#gpu-grad-${gpuIndex})`}
          />
          <polyline
            points={values.map((v, i) => `${i},${100 - (v / chartMax) * 100}`).join(" ")}
            fill="none" stroke="rgb(234,88,12)" strokeWidth="1.5" vectorEffect="non-scaling-stroke"
          />
        </svg>
        <div className="absolute top-0 right-0 text-xs text-dark-400 font-mono">{chartMax.toFixed(0)}{unit}</div>
        <div className="absolute bottom-0 right-0 text-xs text-dark-400 font-mono">0{unit}</div>
      </div>
      <div className="flex justify-between mt-1">
        <span className="text-xs text-dark-400">{points[0]?.time}</span>
        <span className="text-xs text-dark-400">
          Avg: {(values.reduce((a, b) => a + b, 0) / values.length).toFixed(1)}{unit}
          {" · "}Peak: {Math.max(...values).toFixed(1)}{unit}
        </span>
        <span className="text-xs text-dark-400">{points[points.length - 1]?.time}</span>
      </div>
    </div>
  );
}

function GpuMonitor({ gpuInfo }: { gpuInfo: GpuInfoData }) {
  const vramColor = (used: number, total: number) => {
    if (total === 0) return "text-dark-300";
    const pct = (used / total) * 100;
    return pct > 90 ? "text-danger-400" : pct > 70 ? "text-warn-400" : "text-emerald-400";
  };
  const tempColor = (t: number | null) => {
    if (t === null) return "text-dark-300";
    return t > 85 ? "text-danger-400" : t > 70 ? "text-warn-400" : "text-emerald-400";
  };
  const utilColor = (pct: number) => {
    return pct > 90 ? "text-danger-400" : pct > 70 ? "text-warn-400" : "text-emerald-400";
  };

  return (
    <div>
      <div className="flex items-center gap-2 mb-4">
        <h2 className="text-sm font-medium text-dark-300 uppercase font-mono tracking-widest">GPU Monitoring</h2>
        <span className="text-xs text-dark-400">
          {gpuInfo.gpu_count} GPU{gpuInfo.gpu_count !== 1 ? "s" : ""}
          {gpuInfo.nvidia_toolkit_installed ? " \u00b7 Container Toolkit installed" : ""}
        </span>
      </div>

      {gpuInfo.gpus.map((gpu) => {
        const vramPct = gpu.memory_total_mb > 0 ? (gpu.memory_used_mb / gpu.memory_total_mb) * 100 : 0;
        return (
          <div key={gpu.index} className="bg-dark-800 rounded-lg border border-dark-600 p-5 mb-4">
            <div className="flex items-center justify-between mb-4">
              <div>
                <h3 className="text-sm font-semibold text-dark-50">GPU {gpu.index}: {gpu.name}</h3>
                <p className="text-xs text-dark-400 mt-0.5">Driver {gpu.driver_version} \u00b7 {gpu.performance_state}</p>
              </div>
            </div>

            <div className="grid grid-cols-2 md:grid-cols-4 gap-4 mb-4">
              <div>
                <p className="text-xs text-dark-400 font-mono uppercase tracking-wider mb-1">GPU Utilization</p>
                <p className={`text-lg font-semibold ${utilColor(gpu.utilization_gpu_pct)}`}>{gpu.utilization_gpu_pct}%</p>
              </div>
              <div>
                <p className="text-xs text-dark-400 font-mono uppercase tracking-wider mb-1">VRAM</p>
                <p className={`text-lg font-semibold ${vramColor(gpu.memory_used_mb, gpu.memory_total_mb)}`}>
                  {gpu.memory_used_mb.toLocaleString()} / {gpu.memory_total_mb.toLocaleString()} MB
                </p>
              </div>
              <div>
                <p className="text-xs text-dark-400 font-mono uppercase tracking-wider mb-1">Temperature</p>
                <p className={`text-lg font-semibold ${tempColor(gpu.temperature_c)}`}>
                  {gpu.temperature_c !== null ? `${gpu.temperature_c}\u00b0C` : "N/A"}
                </p>
              </div>
              <div>
                <p className="text-xs text-dark-400 font-mono uppercase tracking-wider mb-1">Power</p>
                <p className="text-lg font-semibold text-dark-50">
                  {gpu.power_draw_w !== null ? `${gpu.power_draw_w.toFixed(0)}W` : "N/A"}
                  {gpu.power_limit_w !== null ? <span className="text-dark-400 text-sm"> / {gpu.power_limit_w.toFixed(0)}W</span> : ""}
                </p>
              </div>
            </div>

            {/* VRAM usage bar */}
            <div className="h-2 bg-dark-700 rounded-full overflow-hidden">
              <div
                className={`h-full rounded-full transition-all ${vramPct > 90 ? "bg-danger-500" : vramPct > 70 ? "bg-warn-500" : "bg-emerald-500"}`}
                style={{ width: `${Math.min(vramPct, 100)}%` }}
              />
            </div>
            <div className="flex justify-between mt-1">
              <span className="text-xs text-dark-400">{vramPct.toFixed(1)}% VRAM used</span>
              <span className="text-xs text-dark-400">{gpu.memory_free_mb.toLocaleString()} MB free</span>
            </div>

            {gpu.fan_speed_pct !== null && (
              <p className="text-xs text-dark-400 mt-2">Fan: {gpu.fan_speed_pct}%</p>
            )}
          </div>
        );
      })}

      {/* GPU Processes */}
      {gpuInfo.processes.length > 0 && (
        <div className="bg-dark-800 rounded-lg border border-dark-600 p-5">
          <h3 className="text-xs text-dark-400 font-mono uppercase tracking-wider mb-3">GPU Processes</h3>
          <div className="overflow-x-auto">
            <table className="w-full text-sm">
              <thead>
                <tr className="border-b border-dark-600">
                  <th className="text-left text-xs text-dark-400 font-mono uppercase pb-2">PID</th>
                  <th className="text-left text-xs text-dark-400 font-mono uppercase pb-2">Process</th>
                  <th className="text-left text-xs text-dark-400 font-mono uppercase pb-2">Container</th>
                  <th className="text-right text-xs text-dark-400 font-mono uppercase pb-2">VRAM</th>
                </tr>
              </thead>
              <tbody>
                {gpuInfo.processes.map((proc) => (
                  <tr key={proc.pid} className="border-b border-dark-700/50">
                    <td className="py-1.5 text-dark-200 font-mono text-xs">{proc.pid}</td>
                    <td className="py-1.5 text-dark-200 text-xs truncate max-w-[200px]" title={proc.process_name}>
                      {proc.process_name.split("/").pop()}
                    </td>
                    <td className="py-1.5 text-xs">
                      {proc.container_name ? (
                        <span className="text-rust-400">{proc.container_name}</span>
                      ) : (
                        <span className="text-dark-400">host</span>
                      )}
                    </td>
                    <td className="py-1.5 text-right text-dark-200 font-mono text-xs">{proc.vram_used_mb} MB</td>
                  </tr>
                ))}
              </tbody>
            </table>
          </div>
        </div>
      )}

      {/* Historical GPU Charts */}
      <div className="flex items-center gap-2 mb-4 mt-6">
        <h2 className="text-sm font-medium text-dark-300 uppercase font-mono tracking-widest">GPU History</h2>
        <span className="text-xs text-dark-400">Last 24 hours</span>
      </div>
      {gpuInfo.gpus.map(gpu => (
        <div key={`hist-${gpu.index}`} className="mb-4">
          <GpuHistoryChart gpuIndex={gpu.index} gpuName={gpu.name} />
        </div>
      ))}
    </div>
  );
}
