import { useState, useEffect } from "react";

interface StatusData {
  title: string;
  description: string;
  logo_url: string | null;
  accent_color: string;
  show_subscribe: boolean;
  show_incident_history: boolean;
  overall_status: string;
  components: { id: string; name: string; description: string | null; group: string | null; status: string }[];
  incidents: {
    id: string; title: string; status: string; severity: string;
    started_at: string; resolved_at: string | null;
    updates: { id: string; status: string; message: string; created_at: string }[];
  }[];
  auto_incidents: { id: string; monitor_name: string; started_at: string; resolved_at: string | null; cause: string | null }[];
  updated_at: string;
}

const statusLabels: Record<string, string> = {
  operational: "Operational",
  degraded: "Degraded Performance",
  major_outage: "Major Outage",
};

const statusDotColors: Record<string, string> = {
  operational: "bg-green-500",
  degraded: "bg-yellow-500",
  major_outage: "bg-red-500",
};

const statusBgColors: Record<string, string> = {
  operational: "bg-green-500/10 border-green-500/20 text-green-400",
  degraded: "bg-yellow-500/10 border-yellow-500/20 text-yellow-400",
  major_outage: "bg-red-500/10 border-red-500/20 text-red-400",
};

const incStatusColors: Record<string, string> = {
  investigating: "text-red-400",
  identified: "text-yellow-400",
  monitoring: "text-blue-400",
  resolved: "text-green-400",
  postmortem: "text-gray-400",
};

function formatDate(dateStr: string): string {
  return new Date(dateStr).toLocaleString(undefined, {
    month: "short", day: "numeric", hour: "2-digit", minute: "2-digit",
  });
}

function timeAgo(dateStr: string): string {
  const seconds = Math.floor((Date.now() - new Date(dateStr).getTime()) / 1000);
  if (seconds < 60) return "just now";
  if (seconds < 3600) return `${Math.floor(seconds / 60)}m ago`;
  if (seconds < 86400) return `${Math.floor(seconds / 3600)}h ago`;
  return `${Math.floor(seconds / 86400)}d ago`;
}

export default function PublicStatusPage() {
  const [data, setData] = useState<StatusData | null>(null);
  const [loading, setLoading] = useState(true);
  const [error, setError] = useState("");
  const [email, setEmail] = useState("");
  const [subscribed, setSubscribed] = useState(false);

  useEffect(() => {
    fetch("/api/status-page/public")
      .then(r => r.ok ? r.json() : Promise.reject("Status page unavailable"))
      .then(setData)
      .catch(e => setError(typeof e === "string" ? e : "Failed to load status"))
      .finally(() => setLoading(false));
  }, []);

  const subscribe = async () => {
    try {
      await fetch("/api/status-page/subscribe", {
        method: "POST",
        headers: { "Content-Type": "application/json" },
        body: JSON.stringify({ email }),
      });
      setSubscribed(true);
    } catch {}
  };

  if (loading) {
    return (
      <div className="min-h-screen bg-[#0a0a0a] flex items-center justify-center">
        <div className="w-6 h-6 border-2 border-gray-700 border-t-green-500 rounded-full animate-spin" />
      </div>
    );
  }

  if (error || !data) {
    return (
      <div className="min-h-screen bg-[#0a0a0a] flex items-center justify-center">
        <p className="text-gray-400 font-mono text-sm">{error || "Status page not configured"}</p>
      </div>
    );
  }

  // Group components
  const groups = new Map<string, typeof data.components>();
  for (const comp of data.components) {
    const group = comp.group || "Services";
    if (!groups.has(group)) groups.set(group, []);
    groups.get(group)!.push(comp);
  }

  const activeIncidents = data.incidents.filter(i => i.status !== "resolved" && i.status !== "postmortem");
  const pastIncidents = data.incidents.filter(i => i.status === "resolved" || i.status === "postmortem");

  return (
    <div className="min-h-screen bg-[#0a0a0a] text-gray-200">
      <div className="max-w-3xl mx-auto px-4 py-10">
        {/* Header */}
        <div className="text-center mb-8">
          {data.logo_url && <img src={data.logo_url} alt="" className="h-10 mx-auto mb-4" />}
          <h1 className="text-2xl font-bold font-mono text-white">{data.title}</h1>
          <p className="text-sm text-gray-400 font-mono mt-1">{data.description}</p>
        </div>

        {/* Overall status banner */}
        <div className={`rounded-lg border px-6 py-4 mb-8 text-center font-mono ${statusBgColors[data.overall_status] || statusBgColors.operational}`}>
          <div className="flex items-center justify-center gap-2">
            <div className={`w-3 h-3 rounded-full ${statusDotColors[data.overall_status] || "bg-green-500"}`} />
            <span className="text-lg font-medium">{statusLabels[data.overall_status] || "All Systems Operational"}</span>
          </div>
        </div>

        {/* Active incidents */}
        {activeIncidents.length > 0 && (
          <div className="mb-8 space-y-3">
            <h2 className="text-xs font-mono uppercase tracking-widest text-gray-400 mb-3">Active Incidents</h2>
            {activeIncidents.map(inc => (
              <div key={inc.id} className="bg-[#111] rounded-lg border border-red-500/20 p-4">
                <div className="flex items-center gap-2 mb-2">
                  <span className={`text-xs font-mono uppercase font-medium ${incStatusColors[inc.status] || "text-gray-400"}`}>{inc.status}</span>
                  <span className="text-xs text-gray-500 font-mono">{formatDate(inc.started_at)}</span>
                </div>
                <h3 className="text-sm font-mono text-white font-medium mb-2">{inc.title}</h3>
                {inc.updates.length > 0 && (
                  <div className="border-l-2 border-gray-700 ml-1 pl-3 space-y-2">
                    {inc.updates.map(u => (
                      <div key={u.id}>
                        <div className="flex items-center gap-2">
                          <span className={`text-xs font-mono ${incStatusColors[u.status] || "text-gray-400"}`}>{u.status}</span>
                          <span className="text-xs text-gray-500 font-mono">{formatDate(u.created_at)}</span>
                        </div>
                        <p className="text-xs text-gray-300 font-mono mt-0.5">{u.message}</p>
                      </div>
                    ))}
                  </div>
                )}
              </div>
            ))}
          </div>
        )}

        {/* Components */}
        {data.components.length > 0 && (
          <div className="mb-8">
            {Array.from(groups.entries()).map(([group, comps]) => (
              <div key={group} className="mb-4">
                <h2 className="text-xs font-mono uppercase tracking-widest text-gray-400 mb-2">{group}</h2>
                <div className="bg-[#111] rounded-lg border border-gray-800 divide-y divide-gray-800">
                  {comps.map(comp => (
                    <div key={comp.id} className="px-4 py-3 flex items-center justify-between">
                      <div>
                        <span className="text-sm font-mono text-white">{comp.name}</span>
                        {comp.description && <span className="text-xs text-gray-500 ml-2 font-mono">{comp.description}</span>}
                      </div>
                      <div className="flex items-center gap-2">
                        <div className={`w-2.5 h-2.5 rounded-full ${statusDotColors[comp.status] || "bg-green-500"}`} />
                        <span className="text-xs font-mono text-gray-400">{statusLabels[comp.status] || comp.status}</span>
                      </div>
                    </div>
                  ))}
                </div>
              </div>
            ))}
          </div>
        )}

        {/* Past incidents */}
        {data.show_incident_history && pastIncidents.length > 0 && (
          <div className="mb-8">
            <h2 className="text-xs font-mono uppercase tracking-widest text-gray-400 mb-3">Past Incidents</h2>
            <div className="space-y-3">
              {pastIncidents.slice(0, 10).map(inc => (
                <div key={inc.id} className="bg-[#111] rounded-lg border border-gray-800 p-4">
                  <div className="flex items-center justify-between mb-1">
                    <h3 className="text-sm font-mono text-white">{inc.title}</h3>
                    <span className="text-xs text-gray-500 font-mono">{formatDate(inc.started_at)}</span>
                  </div>
                  <span className={`text-xs font-mono ${incStatusColors[inc.status]}`}>{inc.status}</span>
                  {inc.resolved_at && (
                    <span className="text-xs text-gray-500 font-mono ml-2">
                      Resolved {timeAgo(inc.resolved_at)}
                    </span>
                  )}
                </div>
              ))}
            </div>
          </div>
        )}

        {/* Auto-detected incidents */}
        {data.auto_incidents.length > 0 && (
          <div className="mb-8">
            <h2 className="text-xs font-mono uppercase tracking-widest text-gray-400 mb-3">Recent Downtime Events</h2>
            <div className="space-y-2">
              {data.auto_incidents.map(ai => (
                <div key={ai.id} className="bg-[#111] rounded-lg border border-gray-800 px-4 py-3 flex items-center justify-between">
                  <div>
                    <span className="text-sm font-mono text-white">{ai.monitor_name}</span>
                    {ai.cause && <span className="text-xs text-gray-500 font-mono ml-2">{ai.cause}</span>}
                  </div>
                  <div className="text-xs text-gray-400 font-mono">
                    {formatDate(ai.started_at)}
                    {ai.resolved_at ? <span className="text-green-400 ml-2">Resolved</span> : <span className="text-red-400 ml-2">Ongoing</span>}
                  </div>
                </div>
              ))}
            </div>
          </div>
        )}

        {/* Subscribe */}
        {data.show_subscribe && (
          <div className="bg-[#111] rounded-lg border border-gray-800 p-5 text-center">
            {subscribed ? (
              <p className="text-sm text-green-400 font-mono">Subscribed! You'll receive incident updates via email.</p>
            ) : (
              <>
                <p className="text-sm text-gray-300 font-mono mb-3">Subscribe to incident updates</p>
                <div className="flex gap-2 max-w-md mx-auto">
                  <input type="email" value={email} onChange={e => setEmail(e.target.value)} placeholder="you@example.com"
                    className="flex-1 px-3 py-2 bg-[#0a0a0a] border border-gray-700 rounded-lg text-sm font-mono text-white outline-none focus:border-green-500" />
                  <button onClick={subscribe}
                    className="px-4 py-2 bg-green-600 text-white rounded-lg text-sm font-mono font-medium hover:bg-green-700">
                    Subscribe
                  </button>
                </div>
              </>
            )}
          </div>
        )}

        {/* Footer */}
        <div className="text-center mt-10 text-xs text-gray-600 font-mono">
          Last updated {timeAgo(data.updated_at)} — Powered by DockPanel
        </div>
      </div>
    </div>
  );
}
