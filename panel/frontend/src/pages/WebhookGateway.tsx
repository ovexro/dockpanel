import { useAuth } from "../context/AuthContext";
import { Navigate } from "react-router-dom";
import { useState, useEffect } from "react";
import { api } from "../api";
import { formatDate } from "../utils/format";

interface Endpoint {
  id: string; name: string; description: string | null; token: string;
  verify_mode: string; enabled: boolean; total_received: number;
  last_received_at: string | null; created_at: string;
}

interface Delivery {
  id: string; method: string; headers: Record<string, string>;
  body: string | null; source_ip: string | null; signature_valid: boolean | null;
  forwarded: boolean; forward_status: number | null;
  forward_response: string | null; forward_duration_ms: number | null;
  received_at: string;
}

interface Route {
  id: string; endpoint_id: string; name: string; destination_url: string;
  filter_path: string | null; filter_value: string | null;
  extra_headers: Record<string, string>; retry_count: number;
  enabled: boolean; total_forwarded: number; last_status: number | null;
  created_at: string;
}

type Tab = "endpoints" | "inspector" | "routes";

export default function WebhookGateway() {
  const { user } = useAuth();
  if (!user || user.role !== "admin") return <Navigate to="/" replace />;
  const [tab, setTab] = useState<Tab>("endpoints");
  const [endpoints, setEndpoints] = useState<Endpoint[]>([]);
  const [selectedEndpoint, setSelectedEndpoint] = useState<string | null>(null);
  const [deliveries, setDeliveries] = useState<Delivery[]>([]);
  const [routes, setRoutes] = useState<Route[]>([]);
  const [inspecting, setInspecting] = useState<Delivery | null>(null);
  const [loading, setLoading] = useState(true);
  const [message, setMessage] = useState({ text: "", type: "" });

  // Forms
  const [showEpForm, setShowEpForm] = useState(false);
  const [epForm, setEpForm] = useState({ name: "", description: "", verify_mode: "none", verify_secret: "", verify_header: "" });
  const [showRouteForm, setShowRouteForm] = useState(false);
  const [routeForm, setRouteForm] = useState({ name: "", destination_url: "", filter_path: "", filter_value: "", retry_count: 3 });

  useEffect(() => { loadEndpoints(); }, []);

  const loadEndpoints = async () => {
    setLoading(true);
    try {
      const eps = await api.get<Endpoint[]>("/webhook-gateway/endpoints");
      setEndpoints(eps);
      if (eps.length > 0 && !selectedEndpoint) setSelectedEndpoint(eps[0].id);
    } catch (e) {
      setMessage({ text: e instanceof Error ? e.message : "Failed", type: "error" });
    } finally { setLoading(false); }
  };

  const loadDeliveries = async (epId: string) => {
    try {
      const d = await api.get<Delivery[]>(`/webhook-gateway/endpoints/${epId}/deliveries`);
      setDeliveries(d);
    } catch { setDeliveries([]); }
  };

  const loadRoutes = async (epId: string) => {
    try {
      const r = await api.get<Route[]>(`/webhook-gateway/endpoints/${epId}/routes`);
      setRoutes(r);
    } catch { setRoutes([]); }
  };

  useEffect(() => {
    if (selectedEndpoint) {
      loadDeliveries(selectedEndpoint);
      loadRoutes(selectedEndpoint);
    }
  }, [selectedEndpoint]);

  const createEndpoint = async () => {
    try {
      await api.post("/webhook-gateway/endpoints", epForm);
      setShowEpForm(false);
      setEpForm({ name: "", description: "", verify_mode: "none", verify_secret: "", verify_header: "" });
      setMessage({ text: "Endpoint created", type: "success" });
      loadEndpoints();
    } catch (e) {
      setMessage({ text: e instanceof Error ? e.message : "Failed", type: "error" });
    }
  };

  const deleteEndpoint = async (id: string) => {
    try {
      await api.delete(`/webhook-gateway/endpoints/${id}`);
      setEndpoints(endpoints.filter(e => e.id !== id));
      if (selectedEndpoint === id) setSelectedEndpoint(null);
    } catch (e) {
      setMessage({ text: e instanceof Error ? e.message : "Failed", type: "error" });
    }
  };

  const createRoute = async () => {
    if (!selectedEndpoint) return;
    try {
      await api.post(`/webhook-gateway/endpoints/${selectedEndpoint}/routes`, routeForm);
      setShowRouteForm(false);
      setRouteForm({ name: "", destination_url: "", filter_path: "", filter_value: "", retry_count: 3 });
      setMessage({ text: "Route created", type: "success" });
      loadRoutes(selectedEndpoint);
    } catch (e) {
      setMessage({ text: e instanceof Error ? e.message : "Failed", type: "error" });
    }
  };

  const deleteRoute = async (id: string) => {
    try {
      await api.delete(`/webhook-gateway/routes/${id}`);
      setRoutes(routes.filter(r => r.id !== id));
    } catch (e) {
      setMessage({ text: e instanceof Error ? e.message : "Failed", type: "error" });
    }
  };

  const replayDelivery = async (id: string) => {
    try {
      const r = await api.post<{ replayed_to: number }>(`/webhook-gateway/deliveries/${id}/replay`, {});
      setMessage({ text: `Replayed to ${r.replayed_to} route(s)`, type: "success" });
    } catch (e) {
      setMessage({ text: e instanceof Error ? e.message : "Failed", type: "error" });
    }
  };

  const tabs: { key: Tab; label: string }[] = [
    { key: "endpoints", label: "Endpoints" },
    { key: "inspector", label: "Request Inspector" },
    { key: "routes", label: "Routes" },
  ];

  const selectedEp = endpoints.find(e => e.id === selectedEndpoint);
  const webhookUrl = selectedEp ? `${window.location.origin}/api/webhooks/gateway/${selectedEp.token}` : "";

  if (loading) return <div className="p-8 text-center text-dark-300 font-mono">Loading...</div>;

  return (
    <div className="p-6 lg:p-8">
      <div className="flex flex-col sm:flex-row sm:items-center justify-between gap-3 mb-6 pb-4 border-b border-dark-600">
        <div>
          <h1 className="text-sm font-medium text-dark-300 uppercase font-mono tracking-widest">Webhook Gateway</h1>
          <p className="text-sm text-dark-200 mt-1 font-mono">Receive, inspect, route & replay webhooks</p>
        </div>
      </div>

      {message.text && (
        <div className={`mb-4 px-4 py-3 rounded-lg text-sm border font-mono ${
          message.type === "success" ? "bg-rust-500/10 text-rust-400 border-rust-500/20" : "bg-danger-500/10 text-danger-400 border-danger-500/20"
        }`}>{message.text}</div>
      )}

      {/* Endpoint selector + URL display */}
      <div className="flex items-center gap-3 mb-4">
        <select value={selectedEndpoint || ""} onChange={e => setSelectedEndpoint(e.target.value || null)}
          className="px-3 py-2 bg-dark-800 border border-dark-500 rounded-lg text-sm font-mono text-dark-50 outline-none">
          {endpoints.map(ep => <option key={ep.id} value={ep.id}>{ep.name} ({ep.total_received} received)</option>)}
          {endpoints.length === 0 && <option value="">No endpoints</option>}
        </select>
        {webhookUrl && (
          <div className="flex-1 bg-dark-800 border border-dark-500 rounded-lg px-3 py-2">
            <code className="text-xs text-rust-400 font-mono select-all">{webhookUrl}</code>
          </div>
        )}
      </div>

      <div className="flex gap-1 mb-6 border-b border-dark-600">
        {tabs.map(t => (
          <button key={t.key} onClick={() => setTab(t.key)}
            className={`px-4 py-2 text-xs font-mono uppercase tracking-widest transition-colors ${
              tab === t.key ? "text-rust-400 border-b-2 border-rust-400" : "text-dark-300 hover:text-dark-100"
            }`}>{t.label}</button>
        ))}
      </div>

      {/* Endpoints tab */}
      {tab === "endpoints" && (
        <div className="space-y-4">
          <div className="flex justify-end">
            <button onClick={() => setShowEpForm(!showEpForm)}
              className="px-4 py-2 bg-rust-500 text-white rounded-lg text-sm font-medium font-mono hover:bg-rust-600">
              {showEpForm ? "Cancel" : "New Endpoint"}
            </button>
          </div>

          {showEpForm && (
            <div className="bg-dark-800 rounded-lg border border-dark-500 p-5 space-y-3">
              <div className="grid grid-cols-2 gap-3">
                <div>
                  <label className="block text-xs font-medium text-dark-100 mb-1 font-mono">Name</label>
                  <input type="text" value={epForm.name} onChange={e => setEpForm({ ...epForm, name: e.target.value })}
                    placeholder="e.g., GitHub Webhooks"
                    className="w-full px-3 py-2 bg-dark-900 border border-dark-500 rounded-lg text-sm font-mono text-dark-50 outline-none" />
                </div>
                <div>
                  <label className="block text-xs font-medium text-dark-100 mb-1 font-mono">Signature Verification</label>
                  <select value={epForm.verify_mode} onChange={e => setEpForm({ ...epForm, verify_mode: e.target.value })}
                    className="w-full px-3 py-2 bg-dark-900 border border-dark-500 rounded-lg text-sm font-mono text-dark-50 outline-none">
                    <option value="none">None</option>
                    <option value="hmac_sha256">HMAC-SHA256 (GitHub, Stripe)</option>
                    <option value="hmac_sha1">HMAC-SHA1 (Legacy GitHub)</option>
                  </select>
                </div>
              </div>
              {epForm.verify_mode !== "none" && (
                <div className="grid grid-cols-2 gap-3">
                  <div>
                    <label className="block text-xs font-medium text-dark-100 mb-1 font-mono">Secret Key</label>
                    <input type="password" value={epForm.verify_secret} onChange={e => setEpForm({ ...epForm, verify_secret: e.target.value })}
                      className="w-full px-3 py-2 bg-dark-900 border border-dark-500 rounded-lg text-sm font-mono text-dark-50 outline-none" />
                  </div>
                  <div>
                    <label className="block text-xs font-medium text-dark-100 mb-1 font-mono">Signature Header</label>
                    <input type="text" value={epForm.verify_header} onChange={e => setEpForm({ ...epForm, verify_header: e.target.value })}
                      placeholder="e.g., X-Hub-Signature-256"
                      className="w-full px-3 py-2 bg-dark-900 border border-dark-500 rounded-lg text-sm font-mono text-dark-50 outline-none" />
                  </div>
                </div>
              )}
              <div className="flex justify-end">
                <button onClick={createEndpoint} className="px-4 py-2 bg-rust-500 text-white rounded-lg text-sm font-medium font-mono hover:bg-rust-600">Create</button>
              </div>
            </div>
          )}

          {endpoints.length === 0 ? (
            <div className="p-12 text-center">
              <p className="text-dark-200 text-sm font-mono">No webhook endpoints</p>
              <p className="text-dark-300 text-xs mt-1 font-mono">Create an endpoint to start receiving webhooks</p>
            </div>
          ) : (
            <div className="bg-dark-800 rounded-lg border border-dark-500 overflow-x-auto">
              <table className="w-full">
                <thead>
                  <tr className="bg-dark-900 border-b border-dark-500">
                    {["Name", "Verification", "Received", "Last", ""].map(h => (
                      <th key={h} className="text-left text-xs font-medium text-dark-200 uppercase font-mono tracking-widest px-5 py-3">{h}</th>
                    ))}
                  </tr>
                </thead>
                <tbody className="divide-y divide-dark-600">
                  {endpoints.map(ep => (
                    <tr key={ep.id} className={`hover:bg-dark-700/30 transition-colors cursor-pointer ${selectedEndpoint === ep.id ? "bg-dark-700/20" : ""}`}
                      onClick={() => setSelectedEndpoint(ep.id)}>
                      <td className="px-5 py-4 text-sm text-dark-50 font-mono">{ep.name}</td>
                      <td className="px-5 py-4 text-xs text-dark-200 font-mono">{ep.verify_mode}</td>
                      <td className="px-5 py-4 text-sm text-dark-200 font-mono">{ep.total_received}</td>
                      <td className="px-5 py-4 text-xs text-dark-300 font-mono">{ep.last_received_at ? formatDate(ep.last_received_at) : "Never"}</td>
                      <td className="px-5 py-4">
                        <button onClick={e => { e.stopPropagation(); deleteEndpoint(ep.id); }}
                          className="px-3 py-1 bg-danger-500/10 text-danger-400 rounded-md text-xs font-medium font-mono hover:bg-danger-500/20">Delete</button>
                      </td>
                    </tr>
                  ))}
                </tbody>
              </table>
            </div>
          )}
        </div>
      )}

      {/* Inspector tab */}
      {tab === "inspector" && (
        <div className="space-y-4">
          {inspecting && (
            <div className="bg-dark-800 rounded-lg border border-accent-500/30 p-4 space-y-3">
              <div className="flex items-center justify-between">
                <h3 className="text-xs font-medium text-accent-400 uppercase font-mono tracking-widest">Request Details</h3>
                <button onClick={() => setInspecting(null)} className="text-dark-300 hover:text-dark-100 text-xs font-mono">Close</button>
              </div>
              <div className="grid grid-cols-3 gap-3 text-xs font-mono">
                <div><span className="text-dark-300">Method:</span> <span className="text-dark-50">{inspecting.method}</span></div>
                <div><span className="text-dark-300">IP:</span> <span className="text-dark-50">{inspecting.source_ip || "unknown"}</span></div>
                <div><span className="text-dark-300">Sig Valid:</span> <span className={inspecting.signature_valid === true ? "text-rust-400" : inspecting.signature_valid === false ? "text-danger-400" : "text-dark-300"}>{inspecting.signature_valid === null ? "N/A" : inspecting.signature_valid ? "Yes" : "No"}</span></div>
              </div>
              <div>
                <p className="text-xs text-dark-300 font-mono mb-1">Headers:</p>
                <pre className="bg-dark-900 p-2 rounded text-xs font-mono text-dark-100 overflow-auto max-h-32">{JSON.stringify(inspecting.headers, null, 2)}</pre>
              </div>
              <div>
                <p className="text-xs text-dark-300 font-mono mb-1">Body:</p>
                <pre className="bg-dark-900 p-2 rounded text-xs font-mono text-dark-100 overflow-auto max-h-48">{inspecting.body ? JSON.stringify(JSON.parse(inspecting.body), null, 2) : "(empty)"}</pre>
              </div>
              {inspecting.forwarded && (
                <div>
                  <p className="text-xs text-dark-300 font-mono mb-1">Forward Response ({inspecting.forward_status}, {inspecting.forward_duration_ms}ms):</p>
                  <pre className="bg-dark-900 p-2 rounded text-xs font-mono text-dark-100 overflow-auto max-h-24">{inspecting.forward_response || "(empty)"}</pre>
                </div>
              )}
            </div>
          )}

          {deliveries.length === 0 ? (
            <div className="p-12 text-center">
              <p className="text-dark-200 text-sm font-mono">No deliveries yet</p>
              <p className="text-dark-300 text-xs mt-1 font-mono">Send a webhook to your endpoint URL to see it here</p>
            </div>
          ) : (
            <div className="bg-dark-800 rounded-lg border border-dark-500 overflow-x-auto">
              <table className="w-full">
                <thead>
                  <tr className="bg-dark-900 border-b border-dark-500">
                    {["Time", "IP", "Sig", "Forwarded", "Status", "Duration", ""].map(h => (
                      <th key={h} className="text-left text-xs font-medium text-dark-200 uppercase font-mono tracking-widest px-4 py-3">{h}</th>
                    ))}
                  </tr>
                </thead>
                <tbody className="divide-y divide-dark-600">
                  {deliveries.map(d => (
                    <tr key={d.id} className="hover:bg-dark-700/30 transition-colors cursor-pointer" onClick={() => setInspecting(d)}>
                      <td className="px-4 py-3 text-xs text-dark-200 font-mono">{formatDate(d.received_at)}</td>
                      <td className="px-4 py-3 text-xs text-dark-300 font-mono">{d.source_ip || "-"}</td>
                      <td className="px-4 py-3">
                        {d.signature_valid === null ? <span className="text-xs text-dark-400 font-mono">-</span> :
                          d.signature_valid ? <span className="text-xs text-rust-400 font-mono">Valid</span> :
                            <span className="text-xs text-danger-400 font-mono">Invalid</span>}
                      </td>
                      <td className="px-4 py-3 text-xs font-mono">{d.forwarded ? <span className="text-rust-400">Yes</span> : <span className="text-dark-400">No</span>}</td>
                      <td className="px-4 py-3 text-xs font-mono">
                        {d.forward_status ? (
                          <span className={d.forward_status >= 200 && d.forward_status < 300 ? "text-rust-400" : "text-danger-400"}>{d.forward_status}</span>
                        ) : "-"}
                      </td>
                      <td className="px-4 py-3 text-xs text-dark-300 font-mono">{d.forward_duration_ms ? `${d.forward_duration_ms}ms` : "-"}</td>
                      <td className="px-4 py-3">
                        <button onClick={e => { e.stopPropagation(); replayDelivery(d.id); }}
                          className="px-2 py-1 bg-accent-500/10 text-accent-400 rounded text-xs font-mono hover:bg-accent-500/20">Replay</button>
                      </td>
                    </tr>
                  ))}
                </tbody>
              </table>
            </div>
          )}
        </div>
      )}

      {/* Routes tab */}
      {tab === "routes" && (
        <div className="space-y-4">
          <div className="flex justify-end">
            <button onClick={() => setShowRouteForm(!showRouteForm)}
              className="px-4 py-2 bg-rust-500 text-white rounded-lg text-sm font-medium font-mono hover:bg-rust-600">
              {showRouteForm ? "Cancel" : "Add Route"}
            </button>
          </div>

          {showRouteForm && (
            <div className="bg-dark-800 rounded-lg border border-dark-500 p-5 space-y-3">
              <div className="grid grid-cols-2 gap-3">
                <div>
                  <label className="block text-xs font-medium text-dark-100 mb-1 font-mono">Route Name</label>
                  <input type="text" value={routeForm.name} onChange={e => setRouteForm({ ...routeForm, name: e.target.value })}
                    placeholder="e.g., Forward to Slack"
                    className="w-full px-3 py-2 bg-dark-900 border border-dark-500 rounded-lg text-sm font-mono text-dark-50 outline-none" />
                </div>
                <div>
                  <label className="block text-xs font-medium text-dark-100 mb-1 font-mono">Destination URL</label>
                  <input type="text" value={routeForm.destination_url} onChange={e => setRouteForm({ ...routeForm, destination_url: e.target.value })}
                    placeholder="https://hooks.slack.com/..."
                    className="w-full px-3 py-2 bg-dark-900 border border-dark-500 rounded-lg text-sm font-mono text-dark-50 outline-none" />
                </div>
              </div>
              <div className="grid grid-cols-3 gap-3">
                <div>
                  <label className="block text-xs font-medium text-dark-100 mb-1 font-mono">Filter JSON Path</label>
                  <input type="text" value={routeForm.filter_path} onChange={e => setRouteForm({ ...routeForm, filter_path: e.target.value })}
                    placeholder="/action (optional)"
                    className="w-full px-3 py-2 bg-dark-900 border border-dark-500 rounded-lg text-sm font-mono text-dark-50 outline-none" />
                </div>
                <div>
                  <label className="block text-xs font-medium text-dark-100 mb-1 font-mono">Filter Value</label>
                  <input type="text" value={routeForm.filter_value} onChange={e => setRouteForm({ ...routeForm, filter_value: e.target.value })}
                    placeholder="push (optional)"
                    className="w-full px-3 py-2 bg-dark-900 border border-dark-500 rounded-lg text-sm font-mono text-dark-50 outline-none" />
                </div>
                <div>
                  <label className="block text-xs font-medium text-dark-100 mb-1 font-mono">Retries</label>
                  <input type="number" value={routeForm.retry_count} min={0} max={10}
                    onChange={e => setRouteForm({ ...routeForm, retry_count: parseInt(e.target.value) || 3 })}
                    className="w-full px-3 py-2 bg-dark-900 border border-dark-500 rounded-lg text-sm font-mono text-dark-50 outline-none" />
                </div>
              </div>
              <div className="flex justify-end">
                <button onClick={createRoute} className="px-4 py-2 bg-rust-500 text-white rounded-lg text-sm font-medium font-mono hover:bg-rust-600">Create Route</button>
              </div>
            </div>
          )}

          {routes.length === 0 ? (
            <div className="p-12 text-center">
              <p className="text-dark-200 text-sm font-mono">No routes configured</p>
              <p className="text-dark-300 text-xs mt-1 font-mono">Add a route to forward incoming webhooks to a destination</p>
            </div>
          ) : (
            <div className="bg-dark-800 rounded-lg border border-dark-500 overflow-x-auto">
              <table className="w-full">
                <thead>
                  <tr className="bg-dark-900 border-b border-dark-500">
                    {["Name", "Destination", "Filter", "Retries", "Forwarded", "Last Status", ""].map(h => (
                      <th key={h} className="text-left text-xs font-medium text-dark-200 uppercase font-mono tracking-widest px-4 py-3">{h}</th>
                    ))}
                  </tr>
                </thead>
                <tbody className="divide-y divide-dark-600">
                  {routes.map(r => (
                    <tr key={r.id} className="hover:bg-dark-700/30 transition-colors">
                      <td className="px-4 py-3 text-sm text-dark-50 font-mono">{r.name}</td>
                      <td className="px-4 py-3 text-xs text-dark-200 font-mono truncate max-w-[200px]">{r.destination_url}</td>
                      <td className="px-4 py-3 text-xs text-dark-300 font-mono">{r.filter_path ? `${r.filter_path}=${r.filter_value}` : "-"}</td>
                      <td className="px-4 py-3 text-xs text-dark-200 font-mono">{r.retry_count}</td>
                      <td className="px-4 py-3 text-sm text-dark-200 font-mono">{r.total_forwarded}</td>
                      <td className="px-4 py-3 text-xs font-mono">
                        {r.last_status ? (
                          <span className={r.last_status >= 200 && r.last_status < 300 ? "text-rust-400" : "text-danger-400"}>{r.last_status}</span>
                        ) : "-"}
                      </td>
                      <td className="px-4 py-3">
                        <button onClick={() => deleteRoute(r.id)}
                          className="px-2 py-1 bg-danger-500/10 text-danger-400 rounded text-xs font-mono hover:bg-danger-500/20">Delete</button>
                      </td>
                    </tr>
                  ))}
                </tbody>
              </table>
            </div>
          )}
        </div>
      )}
    </div>
  );
}
