import { useState, useEffect } from "react";
import { api } from "../api";
import { Link } from "react-router-dom";

interface DashboardData {
  panel_name: string | null;
  used_users: number;
  max_users: number | null;
  used_sites: number;
  max_sites: number | null;
  used_databases: number;
  max_databases: number | null;
  server_count: number;
}

interface ServerItem {
  id: string;
  name: string;
  status: string;
  ip_address: string | null;
}

export default function ResellerDashboard() {
  const [data, setData] = useState<DashboardData | null>(null);
  const [servers, setServers] = useState<ServerItem[]>([]);
  const [error, setError] = useState("");

  useEffect(() => {
    api.get<DashboardData>("/reseller/dashboard").then(setData).catch((e) => setError(e.message));
    api.get<ServerItem[]>("/reseller/servers").then(setServers).catch(() => {});
  }, []);

  const pct = (used: number, max: number | null) => max ? Math.min(100, Math.round((used / max) * 100)) : 0;
  const barColor = (used: number, max: number | null) => {
    if (!max) return "bg-rust-500";
    const p = (used / max) * 100;
    return p >= 90 ? "bg-danger-500" : p >= 70 ? "bg-warn-500" : "bg-rust-500";
  };

  if (error) return <div className="p-6"><div className="px-4 py-3 bg-danger-500/10 border border-danger-500/30 rounded-lg text-sm text-danger-400">{error}</div></div>;
  if (!data) return <div className="p-6"><div className="w-6 h-6 border-2 border-dark-600 border-t-rust-500 rounded-full animate-spin" /></div>;

  return (
    <div className="p-6 space-y-6">
      <div>
        <h1 className="text-2xl font-bold text-dark-50 font-mono">
          {data.panel_name || "Reseller Dashboard"}
        </h1>
        <p className="text-sm text-dark-300 mt-1">Manage your users, sites, and resource quotas</p>
      </div>

      {/* Quota Cards */}
      <div className="grid grid-cols-1 md:grid-cols-3 gap-4">
        {[
          { label: "Users", used: data.used_users, max: data.max_users, link: "/reseller/users" },
          { label: "Sites", used: data.used_sites, max: data.max_sites },
          { label: "Databases", used: data.used_databases, max: data.max_databases },
        ].map((q) => (
          <div key={q.label} className="bg-dark-800 border border-dark-600 rounded-lg p-5">
            <div className="flex items-center justify-between mb-2">
              <span className="text-sm text-dark-300 uppercase tracking-wider">{q.label}</span>
              <span className="text-2xl font-bold text-dark-50 font-mono">
                {q.used}{q.max != null && <span className="text-sm text-dark-400">/{q.max}</span>}
              </span>
            </div>
            {q.max != null && (
              <div className="h-2 bg-dark-700 rounded-full overflow-hidden">
                <div className={`h-full ${barColor(q.used, q.max)} rounded-full transition-all`} style={{ width: `${pct(q.used, q.max)}%` }} />
              </div>
            )}
            {q.link && (
              <Link to={q.link} className="text-xs text-rust-400 hover:text-rust-300 mt-2 inline-block">Manage &rarr;</Link>
            )}
          </div>
        ))}
      </div>

      {/* Allocated Servers */}
      <div className="bg-dark-800 border border-dark-600 rounded-lg p-5">
        <h2 className="text-lg font-bold text-dark-50 font-mono mb-3">Allocated Servers ({data.server_count})</h2>
        {servers.length === 0 ? (
          <p className="text-sm text-dark-400">No servers allocated yet. Contact your admin.</p>
        ) : (
          <div className="space-y-2">
            {servers.map((s) => (
              <div key={s.id} className="flex items-center gap-3 px-3 py-2 bg-dark-900/50 rounded">
                <div className={`w-2 h-2 rounded-full ${s.status === "online" ? "bg-rust-500" : "bg-danger-500"}`} />
                <span className="text-sm text-dark-50 font-mono">{s.name}</span>
                <span className="text-xs text-dark-400">{s.ip_address || "local"}</span>
                <span className={`ml-auto text-xs ${s.status === "online" ? "text-rust-400" : "text-danger-400"}`}>{s.status}</span>
              </div>
            ))}
          </div>
        )}
      </div>

      {/* Quick Actions */}
      <div className="flex gap-3">
        <Link to="/reseller/users" className="px-4 py-2 bg-rust-500 text-dark-950 rounded-lg text-sm font-bold hover:bg-rust-400 transition-colors">
          Manage Users
        </Link>
        <Link to="/sites" className="px-4 py-2 bg-dark-700 text-dark-200 rounded-lg text-sm hover:bg-dark-600 transition-colors">
          View Sites
        </Link>
      </div>
    </div>
  );
}
