import { useAuth } from "../context/AuthContext";
import { Navigate } from "react-router-dom";
import { useState, useEffect, useCallback } from "react";
import { api, ApiError } from "../api";

interface Extension {
  id: string;
  name: string;
  description: string;
  author: string;
  version: string;
  webhook_url: string;
  api_key_prefix: string | null;
  enabled: boolean;
  event_subscriptions: string; // JSON array string
  api_scopes: string; // JSON array string
  last_webhook_at: string | null;
  last_webhook_status: number | null;
  created_at: string;
}

interface ExtEvent {
  id: string;
  extension_id: string;
  event_type: string;
  response_status: number | null;
  duration_ms: number | null;
  delivered_at: string;
}

const EVENT_TYPES = [
  "site.created", "site.deleted",
  "backup.created", "backup.restored",
  "deploy.started", "deploy.completed", "deploy.failed",
  "app.deployed", "app.removed",
  "auth.login_failed",
  "ssl.provisioned",
];

const API_SCOPES = ["sites:read", "metrics:read", "monitors:read"];

export default function Extensions() {
  const { user } = useAuth();
  if (!user || user.role !== "admin") return <Navigate to="/" replace />;
  const [extensions, setExtensions] = useState<Extension[]>([]);
  const [loading, setLoading] = useState(true);
  const [error, setError] = useState("");
  const [testResult, setTestResult] = useState("");
  const [creating, setCreating] = useState(false);
  const [newKey, setNewKey] = useState<{ api_key: string; webhook_secret: string } | null>(null);

  // Create form
  const [formName, setFormName] = useState("");
  const [formDesc, setFormDesc] = useState("");
  const [formAuthor, setFormAuthor] = useState("");
  const [formUrl, setFormUrl] = useState("");
  const [formEvents, setFormEvents] = useState<Set<string>>(new Set());
  const [formScopes, setFormScopes] = useState<Set<string>>(new Set());

  // Event log
  const [viewingEvents, setViewingEvents] = useState<string | null>(null);
  const [events, setEvents] = useState<ExtEvent[]>([]);
  const [testing, setTesting] = useState<string | null>(null);

  const fetchExtensions = useCallback(async () => {
    try {
      const data = await api.get<Extension[]>("/extensions");
      setExtensions(data);
    } catch (e) {
      setError(e instanceof ApiError ? e.message : "Failed to load");
    } finally {
      setLoading(false);
    }
  }, []);

  useEffect(() => { fetchExtensions(); }, [fetchExtensions]);

  const handleCreate = async () => {
    if (!formName.trim() || !formUrl.trim()) return;
    setError("");
    try {
      const res = await api.post<{ id: string; api_key: string; webhook_secret: string }>("/extensions", {
        name: formName.trim(),
        description: formDesc.trim(),
        author: formAuthor.trim(),
        webhook_url: formUrl.trim(),
        event_subscriptions: JSON.stringify(Array.from(formEvents)),
        api_scopes: JSON.stringify(Array.from(formScopes)),
      });
      setNewKey({ api_key: res.api_key, webhook_secret: res.webhook_secret });
      setFormName(""); setFormDesc(""); setFormAuthor(""); setFormUrl("");
      setFormEvents(new Set()); setFormScopes(new Set());
      await fetchExtensions();
    } catch (e) {
      setError(e instanceof ApiError ? e.message : "Failed to create");
    }
  };

  const handleDelete = async (id: string, name: string) => {
    if (!confirm(`Delete extension "${name}"?`)) return;
    try {
      await api.delete(`/extensions/${id}`);
      await fetchExtensions();
    } catch (e) {
      setError(e instanceof ApiError ? e.message : "Failed to delete");
    }
  };

  const handleToggle = async (id: string, enabled: boolean) => {
    try {
      await api.put(`/extensions/${id}`, { enabled: !enabled });
      await fetchExtensions();
    } catch (e) {
      setError(e instanceof ApiError ? e.message : "Failed to toggle");
    }
  };

  const handleTest = async (id: string) => {
    setTesting(id);
    try {
      const res = await api.post<{ status: number }>(`/extensions/${id}/test`);
      setTestResult(`Webhook test: HTTP ${res.status}`);
    } catch (e) {
      setTestResult(e instanceof ApiError ? e.message : "Test failed");
    } finally {
      setTesting(null);
      await fetchExtensions();
    }
  };

  const handleViewEvents = async (id: string) => {
    setViewingEvents(viewingEvents === id ? null : id);
    if (viewingEvents !== id) {
      try {
        const data = await api.get<ExtEvent[]>(`/extensions/${id}/events`);
        setEvents(data);
      } catch { setEvents([]); }
    }
  };

  if (loading) return <div className="p-6"><div className="w-6 h-6 border-2 border-dark-600 border-t-rust-500 rounded-full animate-spin" /></div>;

  return (
    <div className="p-6 space-y-6">
      <div className="flex items-center justify-between">
        <div>
          <h1 className="text-2xl font-bold text-dark-50 font-mono">Extensions</h1>
          <p className="text-sm text-dark-300 mt-1">Webhook-based integrations that receive DockPanel events</p>
        </div>
        <button onClick={() => { setCreating(!creating); setNewKey(null); setError(""); }} className="px-4 py-2 bg-rust-500 text-dark-950 rounded-lg text-sm font-bold hover:bg-rust-400 transition-colors">
          + Add Extension
        </button>
      </div>

      {error && <div className="px-4 py-3 bg-danger-500/10 border border-danger-500/30 rounded-lg text-sm text-danger-400">{error}</div>}
      {testResult && <div className="px-4 py-3 bg-dark-700 border border-dark-500 rounded-lg text-sm text-dark-100">{testResult}</div>}

      {/* New key display */}
      {newKey && (
        <div className="bg-rust-500/10 border border-rust-500/30 rounded-lg p-5 space-y-3">
          <h3 className="text-sm font-bold text-rust-400">Extension Created — Save These Credentials</h3>
          <div>
            <label className="text-xs text-dark-300">API Key (shown once)</label>
            <div className="flex gap-2 mt-1">
              <code className="flex-1 px-3 py-2 bg-dark-900 rounded text-sm text-dark-50 font-mono">{newKey.api_key}</code>
              <button onClick={() => navigator.clipboard.writeText(newKey.api_key)} className="px-3 py-2 bg-dark-700 text-dark-200 rounded text-xs hover:bg-dark-600">Copy</button>
            </div>
          </div>
          <div>
            <label className="text-xs text-dark-300">Webhook Secret (for verifying signatures)</label>
            <div className="flex gap-2 mt-1">
              <code className="flex-1 px-3 py-2 bg-dark-900 rounded text-sm text-dark-50 font-mono">{newKey.webhook_secret}</code>
              <button onClick={() => navigator.clipboard.writeText(newKey.webhook_secret)} className="px-3 py-2 bg-dark-700 text-dark-200 rounded text-xs hover:bg-dark-600">Copy</button>
            </div>
          </div>
          <button onClick={() => setNewKey(null)} className="text-xs text-dark-400 hover:text-dark-200">Dismiss</button>
        </div>
      )}

      {/* Create form */}
      {creating && !newKey && (
        <div className="bg-dark-800 border border-dark-600 rounded-lg p-5 space-y-4">
          <h2 className="text-lg font-bold text-dark-50 font-mono">Add Extension</h2>
          <div className="grid grid-cols-1 md:grid-cols-2 gap-4">
            <div>
              <label className="block text-sm text-dark-200 mb-1">Name</label>
              <input value={formName} onChange={(e) => setFormName(e.target.value)} placeholder="My Integration" className="w-full px-3 py-2 bg-dark-900 border border-dark-600 rounded-lg text-dark-50 text-sm focus:border-rust-500 focus:outline-none" />
            </div>
            <div>
              <label className="block text-sm text-dark-200 mb-1">Author</label>
              <input value={formAuthor} onChange={(e) => setFormAuthor(e.target.value)} placeholder="Your Name" className="w-full px-3 py-2 bg-dark-900 border border-dark-600 rounded-lg text-dark-50 text-sm focus:border-rust-500 focus:outline-none" />
            </div>
          </div>
          <div>
            <label className="block text-sm text-dark-200 mb-1">Webhook URL</label>
            <input value={formUrl} onChange={(e) => setFormUrl(e.target.value)} placeholder="https://your-server.com/webhook" className="w-full px-3 py-2 bg-dark-900 border border-dark-600 rounded-lg text-dark-50 text-sm focus:border-rust-500 focus:outline-none" />
          </div>
          <div>
            <label className="block text-sm text-dark-200 mb-1">Description</label>
            <input value={formDesc} onChange={(e) => setFormDesc(e.target.value)} placeholder="What this extension does" className="w-full px-3 py-2 bg-dark-900 border border-dark-600 rounded-lg text-dark-50 text-sm focus:border-rust-500 focus:outline-none" />
          </div>
          <div>
            <label className="block text-sm text-dark-200 mb-2">Event Subscriptions</label>
            <div className="flex flex-wrap gap-2">
              {EVENT_TYPES.map((et) => (
                <label key={et} className="flex items-center gap-1.5 px-2 py-1 bg-dark-900 rounded text-xs cursor-pointer">
                  <input type="checkbox" checked={formEvents.has(et)} onChange={(e) => { const next = new Set(formEvents); e.target.checked ? next.add(et) : next.delete(et); setFormEvents(next); }} className="w-3 h-3" />
                  <span className="text-dark-200 font-mono">{et}</span>
                </label>
              ))}
            </div>
          </div>
          <div>
            <label className="block text-sm text-dark-200 mb-2">API Scopes</label>
            <div className="flex flex-wrap gap-2">
              {API_SCOPES.map((s) => (
                <label key={s} className="flex items-center gap-1.5 px-2 py-1 bg-dark-900 rounded text-xs cursor-pointer">
                  <input type="checkbox" checked={formScopes.has(s)} onChange={(e) => { const next = new Set(formScopes); e.target.checked ? next.add(s) : next.delete(s); setFormScopes(next); }} className="w-3 h-3" />
                  <span className="text-dark-200 font-mono">{s}</span>
                </label>
              ))}
            </div>
          </div>
          <div className="flex gap-3">
            <button onClick={handleCreate} className="px-4 py-2 bg-rust-500 text-dark-950 rounded-lg text-sm font-bold hover:bg-rust-400 transition-colors">Create Extension</button>
            <button onClick={() => setCreating(false)} className="px-4 py-2 bg-dark-700 text-dark-200 rounded-lg text-sm hover:bg-dark-600 transition-colors">Cancel</button>
          </div>
        </div>
      )}

      {/* Extension list */}
      <div className="space-y-3">
        {extensions.map((ext) => (
          <div key={ext.id} className="bg-dark-800 border border-dark-600 rounded-lg p-5">
            <div className="flex items-start justify-between">
              <div>
                <h3 className="text-base font-bold text-dark-50 font-mono flex items-center gap-2">
                  {ext.name}
                  <span className={`text-[10px] px-2 py-0.5 rounded-full uppercase font-bold ${ext.enabled ? "bg-rust-500/20 text-rust-400" : "bg-dark-600 text-dark-400"}`}>
                    {ext.enabled ? "Active" : "Disabled"}
                  </span>
                  {ext.last_webhook_status && (
                    <span className={`text-[10px] px-2 py-0.5 rounded-full ${ext.last_webhook_status < 400 ? "bg-rust-500/10 text-rust-400" : "bg-danger-500/10 text-danger-400"}`}>
                      HTTP {ext.last_webhook_status}
                    </span>
                  )}
                </h3>
                <p className="text-sm text-dark-300 mt-0.5">{ext.description || ext.webhook_url}</p>
                <p className="text-xs text-dark-400 mt-1">
                  {ext.author && `by ${ext.author} · `}v{ext.version}
                  {ext.api_key_prefix && ` · Key: ${ext.api_key_prefix}...`}
                  {ext.last_webhook_at && ` · Last delivery: ${new Date(ext.last_webhook_at).toLocaleString()}`}
                </p>
              </div>
              <div className="flex items-center gap-2">
                <button onClick={() => handleToggle(ext.id, ext.enabled)} className="px-3 py-1.5 bg-dark-700 text-dark-200 rounded text-xs font-medium hover:bg-dark-600 transition-colors">
                  {ext.enabled ? "Disable" : "Enable"}
                </button>
                <button onClick={() => handleTest(ext.id)} disabled={testing === ext.id} className="px-3 py-1.5 bg-dark-700 text-dark-200 rounded text-xs font-medium hover:bg-dark-600 transition-colors disabled:opacity-50">
                  {testing === ext.id ? "Testing..." : "Test"}
                </button>
                <button onClick={() => handleViewEvents(ext.id)} className="px-3 py-1.5 bg-dark-700 text-dark-200 rounded text-xs font-medium hover:bg-dark-600 transition-colors">
                  Events
                </button>
                <button onClick={() => handleDelete(ext.id, ext.name)} className="px-3 py-1.5 bg-danger-500/10 text-danger-400 rounded text-xs font-medium hover:bg-danger-500/20 transition-colors">
                  Delete
                </button>
              </div>
            </div>

            {/* Event log */}
            {viewingEvents === ext.id && (
              <div className="mt-3 bg-dark-900/50 rounded-lg p-3">
                <h4 className="text-xs font-bold text-dark-300 uppercase mb-2">Recent Deliveries</h4>
                {events.length === 0 ? (
                  <p className="text-xs text-dark-400">No deliveries yet</p>
                ) : (
                  <div className="space-y-1">
                    {events.map((ev) => (
                      <div key={ev.id} className="flex items-center gap-3 text-xs font-mono">
                        <span className={`w-8 text-right ${ev.response_status && ev.response_status < 400 ? "text-rust-400" : "text-danger-400"}`}>
                          {ev.response_status || "ERR"}
                        </span>
                        <span className="text-dark-300">{ev.event_type}</span>
                        <span className="text-dark-400 ml-auto">{ev.duration_ms}ms</span>
                        <span className="text-dark-400">{new Date(ev.delivered_at).toLocaleString()}</span>
                      </div>
                    ))}
                  </div>
                )}
              </div>
            )}
          </div>
        ))}

        {extensions.length === 0 && !creating && (
          <div className="text-center py-12 text-dark-300 text-sm">
            No extensions installed. Click "Add Extension" to create your first webhook integration.
          </div>
        )}
      </div>
    </div>
  );
}
