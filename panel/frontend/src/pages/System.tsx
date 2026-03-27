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
      <div className="flex gap-6 mb-6 text-sm font-mono">
        <button onClick={() => setTab("updates")} className={tab === "updates" ? "border-b-2 border-rust-500 text-dark-50 pb-2" : "text-dark-300 hover:text-dark-100 pb-2"}>Updates</button>
        <button onClick={() => setTab("health")} className={tab === "health" ? "border-b-2 border-rust-500 text-dark-50 pb-2" : "text-dark-300 hover:text-dark-100 pb-2"}>Health</button>
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
  const [loading, setLoading] = useState(true);
  const [error, setError] = useState("");

  useEffect(() => {
    Promise.all([
      api.get<HealthData>("/health").catch(() => null),
      api.get<SystemInfoData>("/system/info").catch(() => null),
    ]).then(([h, s]) => {
      if (h) setHealth(h);
      if (s) setSysInfo(s);
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
    <div className="grid grid-cols-1 md:grid-cols-2 lg:grid-cols-3 gap-4">
      {cards.map((c) => (
        <div key={c.label} className="bg-dark-800 rounded-lg border border-dark-600 p-4">
          <p className="text-xs text-dark-400 font-mono uppercase tracking-wider mb-1">{c.label}</p>
          <p className={`text-lg font-semibold ${c.color || "text-dark-50"}`}>{c.value}</p>
          {c.sub && <p className="text-xs text-dark-400 mt-0.5 truncate">{c.sub}</p>}
        </div>
      ))}
    </div>
  );
}
