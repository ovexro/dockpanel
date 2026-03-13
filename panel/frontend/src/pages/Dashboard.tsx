import { useState, useEffect } from "react";
import { api } from "../api";
import { formatSize, formatUptime } from "../utils/format";

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

function ProgressBar({ pct, color }: { pct: number; color: string }) {
  return (
    <div
      className="w-full h-2 bg-dark-700 rounded-full overflow-hidden mt-3"
      role="progressbar"
      aria-valuenow={Math.min(pct, 100)}
      aria-valuemin={0}
      aria-valuemax={100}
    >
      <div
        className={`h-full rounded-full transition-all duration-500 ${color}`}
        style={{ width: `${Math.min(pct, 100)}%` }}
      />
    </div>
  );
}

function barColor(pct: number): string {
  if (pct < 60) return "bg-emerald-500";
  if (pct < 85) return "bg-amber-500";
  return "bg-red-500";
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

  const fetchData = () => {
    api
      .get<SystemInfo>("/system/info")
      .then(setSystem)
      .catch((e) => setError(e.message));
    api
      .get<{ id: string; status: string }[]>("/sites")
      .then((list) =>
        setSites({
          total: list.length,
          active: list.filter((s) => s.status === "active").length,
        })
      )
      .catch((e) => console.error("Failed to load sites:", e));
    api
      .get<{ id: string }[]>("/databases")
      .then((list) => setDbCount(list.length))
      .catch((e) => console.error("Failed to load databases:", e));
    api
      .get<Process[]>("/system/processes")
      .then(setProcesses)
      .catch((e) => console.error("Failed to load processes:", e));
    api
      .get<NetworkIface[]>("/system/network")
      .then(setNetwork)
      .catch((e) => console.error("Failed to load network:", e));
  };

  useEffect(() => {
    fetchData();
    const interval = setInterval(fetchData, 10000);
    return () => clearInterval(interval);
  }, []);

  return (
    <div className="p-6 lg:p-8">
      <h1 className="text-2xl font-bold text-dark-50 mb-6">Dashboard</h1>

      {error && (
        <div className="bg-red-500/10 text-red-400 text-sm px-4 py-3 rounded-lg border border-red-500/20 mb-6">
          {error}
        </div>
      )}

      {!system ? (
        <div className="grid grid-cols-1 md:grid-cols-2 lg:grid-cols-3 gap-4" role="status" aria-live="polite">
          {[...Array(6)].map((_, i) => (
            <div key={i} className="bg-dark-800 rounded-xl border border-dark-500 p-5 animate-pulse">
              <div className="h-4 bg-dark-600 rounded w-20 mb-3" />
              <div className="h-8 bg-dark-600 rounded w-32" />
            </div>
          ))}
        </div>
      ) : (
        <>
          {/* Resource cards */}
          <div className="grid grid-cols-1 md:grid-cols-2 lg:grid-cols-3 gap-4 mb-6">
            {/* CPU */}
            <div className="bg-dark-800 rounded-xl border border-dark-500 p-5">
              <div className="flex items-center justify-between">
                <div className="flex items-center gap-2.5">
                  <div className="w-9 h-9 rounded-lg bg-blue-500/10 flex items-center justify-center">
                    <svg className="w-5 h-5 text-blue-400" fill="none" viewBox="0 0 24 24" stroke="currentColor" strokeWidth={1.5}>
                      <path strokeLinecap="round" strokeLinejoin="round" d="M8.25 3v1.5M4.5 8.25H3m18 0h-1.5M4.5 12H3m18 0h-1.5m-15 3.75H3m18 0h-1.5M8.25 19.5V21M12 3v1.5m0 15V21m3.75-18v1.5m0 15V21m-9-1.5h10.5a2.25 2.25 0 0 0 2.25-2.25V6.75a2.25 2.25 0 0 0-2.25-2.25H6.75A2.25 2.25 0 0 0 4.5 6.75v10.5a2.25 2.25 0 0 0 2.25 2.25Zm.75-12h9v9h-9v-9Z" />
                    </svg>
                  </div>
                  <p className="text-sm font-medium text-dark-200">CPU Usage</p>
                </div>
                <span className="text-xs text-dark-300">{system.cpu_count} cores</span>
              </div>
              <p className="text-3xl font-bold text-dark-50 mt-2">
                {system.cpu_usage.toFixed(1)}%
              </p>
              {system.load_avg_1 !== undefined && (
                <p className="text-xs text-dark-300 mt-1">
                  Load: {system.load_avg_1?.toFixed(2)} / {system.load_avg_5?.toFixed(2)} / {system.load_avg_15?.toFixed(2)}
                </p>
              )}
              <ProgressBar pct={system.cpu_usage} color={barColor(system.cpu_usage)} />
            </div>

            {/* Memory */}
            <div className="bg-dark-800 rounded-xl border border-dark-500 p-5">
              <div className="flex items-center justify-between">
                <div className="flex items-center gap-2.5">
                  <div className="w-9 h-9 rounded-lg bg-purple-500/10 flex items-center justify-center">
                    <svg className="w-5 h-5 text-purple-400" fill="none" viewBox="0 0 24 24" stroke="currentColor" strokeWidth={1.5}>
                      <path strokeLinecap="round" strokeLinejoin="round" d="M3.75 6A2.25 2.25 0 0 1 6 3.75h2.25A2.25 2.25 0 0 1 10.5 6v2.25a2.25 2.25 0 0 1-2.25 2.25H6a2.25 2.25 0 0 1-2.25-2.25V6ZM3.75 15.75A2.25 2.25 0 0 1 6 13.5h2.25a2.25 2.25 0 0 1 2.25 2.25V18a2.25 2.25 0 0 1-2.25 2.25H6A2.25 2.25 0 0 1 3.75 18v-2.25ZM13.5 6a2.25 2.25 0 0 1 2.25-2.25H18A2.25 2.25 0 0 1 20.25 6v2.25A2.25 2.25 0 0 1 18 10.5h-2.25a2.25 2.25 0 0 1-2.25-2.25V6ZM13.5 15.75a2.25 2.25 0 0 1 2.25-2.25H18a2.25 2.25 0 0 1 2.25 2.25V18A2.25 2.25 0 0 1 18 20.25h-2.25a2.25 2.25 0 0 1-2.25-2.25v-2.25Z" />
                    </svg>
                  </div>
                  <p className="text-sm font-medium text-dark-200">Memory</p>
                </div>
                <span className="text-xs text-dark-300">
                  {(system.mem_used_mb / 1024).toFixed(1)} / {(system.mem_total_mb / 1024).toFixed(1)} GB
                </span>
              </div>
              <p className="text-3xl font-bold text-dark-50 mt-2">
                {system.mem_usage_pct.toFixed(1)}%
              </p>
              {system.swap_total_mb > 0 && (
                <p className="text-xs text-dark-300 mt-1">
                  Swap: {(system.swap_used_mb / 1024).toFixed(2)} / {(system.swap_total_mb / 1024).toFixed(2)} GB
                </p>
              )}
              <ProgressBar pct={system.mem_usage_pct} color={barColor(system.mem_usage_pct)} />
            </div>

            {/* Disk */}
            <div className="bg-dark-800 rounded-xl border border-dark-500 p-5">
              <div className="flex items-center justify-between">
                <div className="flex items-center gap-2.5">
                  <div className="w-9 h-9 rounded-lg bg-amber-500/10 flex items-center justify-center">
                    <svg className="w-5 h-5 text-amber-400" fill="none" viewBox="0 0 24 24" stroke="currentColor" strokeWidth={1.5}>
                      <path strokeLinecap="round" strokeLinejoin="round" d="M21.75 17.25v-.228a4.5 4.5 0 0 0-.12-1.03l-2.268-9.64a3.375 3.375 0 0 0-3.285-2.602H7.923a3.375 3.375 0 0 0-3.285 2.602l-2.268 9.64a4.5 4.5 0 0 0-.12 1.03v.228m19.5 0a3 3 0 0 1-3 3H5.25a3 3 0 0 1-3-3m19.5 0a3 3 0 0 0-3-3H5.25a3 3 0 0 0-3 3m16.5 0h.008v.008h-.008v-.008Zm-3 0h.008v.008h-.008v-.008Z" />
                    </svg>
                  </div>
                  <p className="text-sm font-medium text-dark-200">Disk</p>
                </div>
                <span className="text-xs text-dark-300">
                  {system.disk_used_gb.toFixed(0)} / {system.disk_total_gb.toFixed(0)} GB
                </span>
              </div>
              <p className="text-3xl font-bold text-dark-50 mt-2">
                {system.disk_usage_pct.toFixed(1)}%
              </p>
              <p className="text-xs text-dark-300 mt-1">
                {(system.disk_total_gb - system.disk_used_gb).toFixed(0)} GB free
              </p>
              <ProgressBar pct={system.disk_usage_pct} color={barColor(system.disk_usage_pct)} />
            </div>

            {/* Sites */}
            <div className="bg-dark-800 rounded-xl border border-dark-500 p-5">
              <div className="flex items-center gap-2.5">
                <div className="w-9 h-9 rounded-lg bg-emerald-500/10 flex items-center justify-center">
                  <svg className="w-5 h-5 text-emerald-400" fill="none" viewBox="0 0 24 24" stroke="currentColor" strokeWidth={1.5}>
                    <path strokeLinecap="round" strokeLinejoin="round" d="M12 21a9.004 9.004 0 0 0 8.716-6.747M12 21a9.004 9.004 0 0 1-8.716-6.747M12 21c2.485 0 4.5-4.03 4.5-9S14.485 3 12 3m0 18c-2.485 0-4.5-4.03-4.5-9S9.515 3 12 3m0 0a8.997 8.997 0 0 1 7.843 4.582M12 3a8.997 8.997 0 0 0-7.843 4.582m15.686 0A11.953 11.953 0 0 1 12 10.5c-2.998 0-5.74-1.1-7.843-2.918m15.686 0A8.959 8.959 0 0 1 21 12c0 .778-.099 1.533-.284 2.253m0 0A17.919 17.919 0 0 1 12 16.5a17.92 17.92 0 0 1-8.716-2.247m0 0A9 9 0 0 1 3 12c0-1.47.353-2.856.978-4.082" />
                  </svg>
                </div>
                <p className="text-sm font-medium text-dark-200">Sites</p>
              </div>
              <p className="text-3xl font-bold text-dark-50 mt-2">{sites.total}</p>
              <p className="text-sm text-dark-300 mt-2">
                {sites.active} active
              </p>
            </div>

            {/* Databases */}
            <div className="bg-dark-800 rounded-xl border border-dark-500 p-5">
              <div className="flex items-center gap-2.5">
                <div className="w-9 h-9 rounded-lg bg-cyan-500/10 flex items-center justify-center">
                  <svg className="w-5 h-5 text-cyan-400" fill="none" viewBox="0 0 24 24" stroke="currentColor" strokeWidth={1.5}>
                    <path strokeLinecap="round" strokeLinejoin="round" d="M20.25 6.375c0 2.278-3.694 4.125-8.25 4.125S3.75 8.653 3.75 6.375m16.5 0c0-2.278-3.694-4.125-8.25-4.125S3.75 4.097 3.75 6.375m16.5 0v11.25c0 2.278-3.694 4.125-8.25 4.125s-8.25-1.847-8.25-4.125V6.375m16.5 0v3.75m-16.5-3.75v3.75m16.5 0v3.75C20.25 16.153 16.556 18 12 18s-8.25-1.847-8.25-4.125v-3.75m16.5 0c0 2.278-3.694 4.125-8.25 4.125s-8.25-1.847-8.25-4.125" />
                  </svg>
                </div>
                <p className="text-sm font-medium text-dark-200">Databases</p>
              </div>
              <p className="text-3xl font-bold text-dark-50 mt-2">{dbCount}</p>
              <p className="text-sm text-dark-300 mt-2">
                PostgreSQL / MariaDB
              </p>
            </div>

            {/* Uptime */}
            <div className="bg-dark-800 rounded-xl border border-dark-500 p-5">
              <div className="flex items-center gap-2.5">
                <div className="w-9 h-9 rounded-lg bg-rose-500/10 flex items-center justify-center">
                  <svg className="w-5 h-5 text-rose-400" fill="none" viewBox="0 0 24 24" stroke="currentColor" strokeWidth={1.5}>
                    <path strokeLinecap="round" strokeLinejoin="round" d="M12 6v6h4.5m4.5 0a9 9 0 1 1-18 0 9 9 0 0 1 18 0Z" />
                  </svg>
                </div>
                <p className="text-sm font-medium text-dark-200">Uptime</p>
              </div>
              <p className="text-3xl font-bold text-dark-50 mt-2">
                {formatUptime(system.uptime_secs)}
              </p>
              <p className="text-sm text-dark-300 mt-2">{system.hostname}</p>
            </div>
          </div>

          {/* System Information */}
          <div className="bg-dark-800 rounded-xl border border-dark-500 overflow-hidden mb-6">
            <div className="px-5 py-3 border-b border-dark-600">
              <h3 className="text-sm font-medium text-dark-50">System Information</h3>
            </div>
            <div className="grid grid-cols-1 md:grid-cols-2 xl:grid-cols-3 divide-y md:divide-y-0 divide-dark-600">
              {/* Hostname */}
              <div className="flex items-center justify-between md:border-r md:border-dark-600 px-5 py-3">
                <span className="text-sm text-dark-300">Hostname</span>
                <span className="text-sm text-dark-50 font-mono">{system.hostname}</span>
              </div>
              {/* OS */}
              <div className="flex items-center justify-between md:border-r xl:border-r md:border-dark-600 px-5 py-3">
                <span className="text-sm text-dark-300">Operating System</span>
                <span className="text-sm text-dark-50">{system.os}</span>
              </div>
              {/* Kernel */}
              <div className="flex items-center justify-between px-5 py-3">
                <span className="text-sm text-dark-300">Kernel</span>
                <span className="text-sm text-dark-50 font-mono">{system.kernel}</span>
              </div>
              {/* CPU */}
              <div className="flex items-center justify-between md:border-r md:border-t md:border-dark-600 px-5 py-3">
                <span className="text-sm text-dark-300">Processor</span>
                <span className="text-sm text-dark-50">{system.cpu_model}, {system.cpu_count} cores</span>
              </div>
              {/* CPU Temp */}
              <div className="flex items-center justify-between md:border-r xl:border-r md:border-t md:border-dark-600 px-5 py-3">
                <span className="text-sm text-dark-300">CPU Temperature</span>
                {system.cpu_temp != null ? (
                  <span className={`text-sm font-medium ${tempColor(system.cpu_temp)}`}>
                    {system.cpu_temp.toFixed(0)}°C
                  </span>
                ) : (
                  <span className="text-sm text-dark-400">N/A</span>
                )}
              </div>
              {/* Processes */}
              <div className="flex items-center justify-between md:border-t md:border-dark-600 px-5 py-3">
                <span className="text-sm text-dark-300">Running Processes</span>
                <span className="text-sm text-dark-50">{system.process_count.toLocaleString()}</span>
              </div>
            </div>
          </div>

          {/* Network & Processes */}
          <div className="grid grid-cols-1 lg:grid-cols-2 gap-6">
            {/* Network I/O */}
            {network.length > 0 && (
              <div className="bg-dark-800 rounded-xl border border-dark-500 overflow-hidden">
                <div className="px-5 py-3 border-b border-dark-600">
                  <h3 className="text-sm font-medium text-dark-50">Network I/O</h3>
                </div>
                <table className="w-full">
                  <thead>
                    <tr className="bg-dark-900">
                      <th className="text-left text-xs font-medium text-dark-200 uppercase px-5 py-2">Interface</th>
                      <th className="text-right text-xs font-medium text-dark-200 uppercase px-5 py-2">RX</th>
                      <th className="text-right text-xs font-medium text-dark-200 uppercase px-5 py-2">TX</th>
                    </tr>
                  </thead>
                  <tbody className="divide-y divide-dark-600">
                    {network.filter(n => n.rx_bytes > 0 || n.tx_bytes > 0).map((iface) => (
                      <tr key={iface.name}>
                        <td className="px-5 py-2.5 text-sm text-dark-50 font-mono">{iface.name}</td>
                        <td className="px-5 py-2.5 text-sm text-dark-200 text-right">
                          <span className="text-emerald-600">{formatSize(iface.rx_bytes)}</span>
                        </td>
                        <td className="px-5 py-2.5 text-sm text-dark-200 text-right">
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
              <div className="bg-dark-800 rounded-xl border border-dark-500 overflow-hidden">
                <div className="px-5 py-3 border-b border-dark-600">
                  <h3 className="text-sm font-medium text-dark-50">Top Processes</h3>
                </div>
                <table className="w-full">
                  <thead>
                    <tr className="bg-dark-900">
                      <th className="text-left text-xs font-medium text-dark-200 uppercase px-5 py-2">Process</th>
                      <th className="text-right text-xs font-medium text-dark-200 uppercase px-5 py-2">PID</th>
                      <th className="text-right text-xs font-medium text-dark-200 uppercase px-5 py-2">CPU</th>
                      <th className="text-right text-xs font-medium text-dark-200 uppercase px-5 py-2">MEM</th>
                    </tr>
                  </thead>
                  <tbody className="divide-y divide-dark-600">
                    {processes.slice(0, 10).map((p) => (
                      <tr key={p.pid}>
                        <td className="px-5 py-2 text-sm text-dark-50 font-mono truncate max-w-[200px]">{p.name}</td>
                        <td className="px-5 py-2 text-sm text-dark-200 text-right">{p.pid}</td>
                        <td className="px-5 py-2 text-sm text-right">
                          <span className={p.cpu_pct > 50 ? "text-red-400 font-medium" : "text-dark-200"}>
                            {p.cpu_pct.toFixed(1)}%
                          </span>
                        </td>
                        <td className="px-5 py-2 text-sm text-dark-200 text-right">{p.mem_mb.toFixed(0)} MB</td>
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
