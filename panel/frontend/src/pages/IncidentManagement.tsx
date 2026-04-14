import { useAuth } from "../context/AuthContext";
import { Navigate } from "react-router-dom";
import { useState, useEffect } from "react";
import { api } from "../api";
import { formatDate, timeAgo } from "../utils/format";

interface Incident {
  id: string;
  title: string;
  status: string;
  severity: string;
  description: string | null;
  started_at: string;
  resolved_at: string | null;
  postmortem: string | null;
  postmortem_published: boolean;
  visible_on_status_page: boolean;
  created_at: string;
  updated_at: string;
}

interface IncidentUpdate {
  id: string;
  status: string;
  message: string;
  author_email: string | null;
  created_at: string;
}

interface Component {
  id: string;
  name: string;
  description: string | null;
  sort_order: number;
  group_name: string | null;
  monitor_ids: string[];
}

interface StatusConfig {
  id: string;
  title: string;
  description: string;
  logo_url: string | null;
  accent_color: string;
  show_subscribe: boolean;
  show_incident_history: boolean;
  history_days: number;
  enabled: boolean;
}

interface Monitor {
  id: string;
  name: string;
  url: string;
  status: string;
}

type Tab = "incidents" | "components" | "settings";

const STATUSES = ["investigating", "identified", "monitoring", "resolved", "postmortem"];
const SEVERITIES = ["minor", "major", "critical", "maintenance"];

const statusColors: Record<string, string> = {
  investigating: "bg-danger-500/15 text-danger-400",
  identified: "bg-warn-500/15 text-warn-400",
  monitoring: "bg-accent-500/15 text-accent-400",
  resolved: "bg-rust-500/15 text-rust-400",
  postmortem: "bg-dark-700 text-dark-200",
};

const severityColors: Record<string, string> = {
  minor: "bg-warn-500/15 text-warn-400",
  major: "bg-danger-500/15 text-danger-400",
  critical: "bg-danger-500/20 text-danger-300",
  maintenance: "bg-accent-500/15 text-accent-400",
};

export default function IncidentManagement() {
  const { user } = useAuth();
  if (!user || user.role !== "admin") return <Navigate to="/" replace />;
  const [tab, setTab] = useState<Tab>("incidents");
  const [incidents, setIncidents] = useState<Incident[]>([]);
  const [components, setComponents] = useState<Component[]>([]);
  const [config, setConfig] = useState<StatusConfig | null>(null);
  const [monitors, setMonitors] = useState<Monitor[]>([]);
  const [loading, setLoading] = useState(true);
  const [message, setMessage] = useState({ text: "", type: "" });

  // Incident form
  const [showCreate, setShowCreate] = useState(false);
  const [form, setForm] = useState({ title: "", severity: "major", description: "", status: "investigating" });

  // Update form
  const [updateIncident, setUpdateIncident] = useState<string | null>(null);
  const [updateForm, setUpdateForm] = useState({ status: "identified", message: "" });
  const [incidentUpdates, setIncidentUpdates] = useState<IncidentUpdate[]>([]);

  // Component form
  const [showCompForm, setShowCompForm] = useState(false);
  const [compForm, setCompForm] = useState({ name: "", description: "", group_name: "", monitor_ids: [] as string[] });

  useEffect(() => { loadAll(); }, []);

  const loadAll = async () => {
    setLoading(true);
    try {
      const [inc, comp, cfg, mon] = await Promise.all([
        api.get<Incident[]>("/incidents").catch(() => []),
        api.get<Component[]>("/status-page/components").catch(() => []),
        api.get<StatusConfig>("/status-page/config").catch(() => null),
        api.get<Monitor[]>("/monitors").catch(() => []),
      ]);
      setIncidents(inc);
      setComponents(comp);
      setConfig(cfg);
      setMonitors(mon);
    } catch (e) {
      setMessage({ text: e instanceof Error ? e.message : "Failed to load", type: "error" });
    } finally {
      setLoading(false);
    }
  };

  const createIncident = async () => {
    try {
      await api.post("/incidents", form);
      setMessage({ text: "Incident created", type: "success" });
      setShowCreate(false);
      setForm({ title: "", severity: "major", description: "", status: "investigating" });
      loadAll();
    } catch (e) {
      setMessage({ text: e instanceof Error ? e.message : "Failed", type: "error" });
    }
  };

  const postUpdate = async () => {
    if (!updateIncident) return;
    try {
      await api.post(`/incidents/${updateIncident}/updates`, updateForm);
      setMessage({ text: "Update posted", type: "success" });
      setUpdateIncident(null);
      loadAll();
    } catch (e) {
      setMessage({ text: e instanceof Error ? e.message : "Failed", type: "error" });
    }
  };

  const [pendingDelete, setPendingDelete] = useState<{ type: "incident" | "component"; id: string; label: string } | null>(null);

  const deleteIncident = async (id: string) => {
    try {
      await api.delete(`/incidents/${id}`);
      setIncidents(incidents.filter(i => i.id !== id));
      setMessage({ text: "Incident deleted", type: "success" });
    } catch (e) {
      setMessage({ text: e instanceof Error ? e.message : "Failed", type: "error" });
    }
  };

  const viewUpdates = async (id: string) => {
    setUpdateIncident(id);
    const inc = incidents.find(i => i.id === id);
    setUpdateForm({ status: inc?.status === "resolved" ? "postmortem" : "identified", message: "" });
    try {
      const updates = await api.get<IncidentUpdate[]>(`/incidents/${id}/updates`);
      setIncidentUpdates(updates);
    } catch { setIncidentUpdates([]); }
  };

  const createComponent = async () => {
    try {
      await api.post("/status-page/components", compForm);
      setMessage({ text: "Component created", type: "success" });
      setShowCompForm(false);
      setCompForm({ name: "", description: "", group_name: "", monitor_ids: [] });
      loadAll();
    } catch (e) {
      setMessage({ text: e instanceof Error ? e.message : "Failed", type: "error" });
    }
  };

  const deleteComponent = async (id: string) => {
    try {
      await api.delete(`/status-page/components/${id}`);
      setComponents(components.filter(c => c.id !== id));
      setMessage({ text: "Component deleted", type: "success" });
    } catch (e) {
      setMessage({ text: e instanceof Error ? e.message : "Failed", type: "error" });
    }
  };

  const saveConfig = async () => {
    if (!config) return;
    try {
      const updated = await api.put<StatusConfig>("/status-page/config", config);
      setConfig(updated);
      setMessage({ text: "Config saved", type: "success" });
    } catch (e) {
      setMessage({ text: e instanceof Error ? e.message : "Failed", type: "error" });
    }
  };

  const tabs: { key: Tab; label: string }[] = [
    { key: "incidents", label: "Incidents" },
    { key: "components", label: "Components" },
    { key: "settings", label: "Status Page Settings" },
  ];

  if (loading) return <div className="p-8 text-center text-dark-300 font-mono">Loading...</div>;

  return (
    <div className="p-6 lg:p-8">
      <div className="flex flex-col sm:flex-row sm:items-center justify-between gap-3 mb-6 pb-4 border-b border-dark-600">
        <div>
          <h1 className="text-sm font-medium text-dark-300 uppercase font-mono tracking-widest">Incident Management</h1>
          <p className="text-sm text-dark-200 mt-1 font-mono">Manage incidents, components & public status page</p>
        </div>
        <a href="/status" target="_blank" className="px-4 py-2 bg-dark-700 text-dark-100 rounded-lg text-sm font-mono hover:bg-dark-600 transition-colors border border-dark-500">
          View Public Status Page
        </a>
      </div>

      {message.text && (
        <div className={`mb-4 px-4 py-3 rounded-lg text-sm border font-mono flex items-center justify-between ${
          message.type === "success" ? "bg-rust-500/10 text-rust-400 border-rust-500/20" : "bg-danger-500/10 text-danger-400 border-danger-500/20"
        }`}>
          <span>{message.text}</span>
          <button onClick={() => setMessage({ text: "", type: "" })} className="ml-2 hover:opacity-70">
            <svg className="w-4 h-4" fill="none" viewBox="0 0 24 24" stroke="currentColor" strokeWidth={2}><path strokeLinecap="round" strokeLinejoin="round" d="M6 18 18 6M6 6l12 12" /></svg>
          </button>
        </div>
      )}

      {pendingDelete && (
        <div className="mb-4 px-4 py-3 rounded-lg border border-danger-500/30 bg-danger-500/5 flex items-center justify-between">
          <span className="text-xs font-mono text-danger-400">{pendingDelete.label}</span>
          <div className="flex items-center gap-2 shrink-0 ml-4">
            <button onClick={() => { const { type, id } = pendingDelete; setPendingDelete(null); type === "incident" ? deleteIncident(id) : deleteComponent(id); }}
              className="px-3 py-1.5 bg-danger-500 text-white text-xs font-bold uppercase tracking-wider hover:bg-danger-400 transition-colors">Confirm</button>
            <button onClick={() => setPendingDelete(null)}
              className="px-3 py-1.5 bg-dark-600 text-dark-200 text-xs font-bold uppercase tracking-wider hover:bg-dark-500 transition-colors">Cancel</button>
          </div>
        </div>
      )}

      <div className="flex gap-1 mb-6 border-b border-dark-600">
        {tabs.map(t => (
          <button key={t.key} onClick={() => setTab(t.key)}
            className={`px-4 py-2 text-xs font-mono uppercase tracking-widest transition-colors ${
              tab === t.key ? "text-rust-400 border-b-2 border-rust-400" : "text-dark-300 hover:text-dark-100"
            }`}>{t.label}</button>
        ))}
      </div>

      {tab === "incidents" && (
        <div className="space-y-4">
          <div className="flex justify-end">
            <button onClick={() => setShowCreate(!showCreate)}
              className="px-4 py-2 bg-rust-500 text-white rounded-lg text-sm font-medium font-mono hover:bg-rust-600">
              {showCreate ? "Cancel" : "Create Incident"}
            </button>
          </div>

          {showCreate && (
            <div className="bg-dark-800 rounded-lg border border-dark-500 p-5 space-y-3">
              <div>
                <label className="block text-xs font-medium text-dark-100 mb-1 font-mono">Title</label>
                <input type="text" value={form.title} onChange={e => setForm({ ...form, title: e.target.value })}
                  placeholder="Brief description of the issue"
                  className="w-full px-3 py-2 bg-dark-900 border border-dark-500 rounded-lg text-sm font-mono text-dark-50 focus:ring-2 focus:ring-accent-500 outline-none" />
              </div>
              <div className="grid grid-cols-2 gap-3">
                <div>
                  <label className="block text-xs font-medium text-dark-100 mb-1 font-mono">Severity</label>
                  <select value={form.severity} onChange={e => setForm({ ...form, severity: e.target.value })}
                    className="w-full px-3 py-2 bg-dark-900 border border-dark-500 rounded-lg text-sm font-mono text-dark-50 outline-none">
                    {SEVERITIES.map(s => <option key={s} value={s}>{s}</option>)}
                  </select>
                </div>
                <div>
                  <label className="block text-xs font-medium text-dark-100 mb-1 font-mono">Initial Status</label>
                  <select value={form.status} onChange={e => setForm({ ...form, status: e.target.value })}
                    className="w-full px-3 py-2 bg-dark-900 border border-dark-500 rounded-lg text-sm font-mono text-dark-50 outline-none">
                    {STATUSES.map(s => <option key={s} value={s}>{s}</option>)}
                  </select>
                </div>
              </div>
              <div>
                <label className="block text-xs font-medium text-dark-100 mb-1 font-mono">Description</label>
                <textarea value={form.description} onChange={e => setForm({ ...form, description: e.target.value })} rows={3}
                  className="w-full px-3 py-2 bg-dark-900 border border-dark-500 rounded-lg text-sm font-mono text-dark-50 outline-none resize-none" />
              </div>
              <div className="flex justify-end">
                <button onClick={createIncident}
                  className="px-4 py-2 bg-rust-500 text-white rounded-lg text-sm font-medium font-mono hover:bg-rust-600">Create</button>
              </div>
            </div>
          )}

          {/* Update panel */}
          {updateIncident && (
            <div className="bg-dark-800 rounded-lg border border-accent-500/30 p-5 space-y-3">
              <div className="flex items-center justify-between">
                <h3 className="text-xs font-medium text-accent-400 uppercase font-mono tracking-widest">Post Update</h3>
                <button onClick={() => setUpdateIncident(null)} className="text-dark-300 hover:text-dark-100 text-sm font-mono">Close</button>
              </div>
              {/* Timeline */}
              {incidentUpdates.length > 0 && (
                <div className="border-l-2 border-dark-600 ml-2 pl-4 space-y-3">
                  {incidentUpdates.map(u => (
                    <div key={u.id}>
                      <div className="flex items-center gap-2">
                        <span className={`inline-flex px-2 py-0.5 rounded-full text-xs font-medium font-mono ${statusColors[u.status] || "bg-dark-700 text-dark-200"}`}>{u.status}</span>
                        <span className="text-xs text-dark-300 font-mono">{formatDate(u.created_at)}</span>
                      </div>
                      <p className="text-sm text-dark-100 font-mono mt-1">{u.message}</p>
                    </div>
                  ))}
                </div>
              )}
              <div className="grid grid-cols-4 gap-3">
                <div>
                  <label className="block text-xs font-medium text-dark-100 mb-1 font-mono">Status</label>
                  <select value={updateForm.status} onChange={e => setUpdateForm({ ...updateForm, status: e.target.value })}
                    className="w-full px-3 py-2 bg-dark-900 border border-dark-500 rounded-lg text-sm font-mono text-dark-50 outline-none">
                    {STATUSES.map(s => <option key={s} value={s}>{s}</option>)}
                  </select>
                </div>
                <div className="col-span-3">
                  <label className="block text-xs font-medium text-dark-100 mb-1 font-mono">Message</label>
                  <input type="text" value={updateForm.message} onChange={e => setUpdateForm({ ...updateForm, message: e.target.value })}
                    placeholder="What's the latest?"
                    className="w-full px-3 py-2 bg-dark-900 border border-dark-500 rounded-lg text-sm font-mono text-dark-50 outline-none" />
                </div>
              </div>
              <div className="flex justify-end">
                <button onClick={postUpdate}
                  className="px-4 py-2 bg-accent-500 text-white rounded-lg text-sm font-medium font-mono hover:bg-accent-600">Post Update</button>
              </div>
            </div>
          )}

          {incidents.length === 0 ? (
            <div className="p-12 text-center">
              <p className="text-dark-200 text-sm font-mono">No incidents</p>
              <p className="text-dark-300 text-xs mt-1 font-mono">All systems operational</p>
            </div>
          ) : (
            <div className="space-y-2">
              {incidents.map(inc => (
                <div key={inc.id} className="bg-dark-800 rounded-lg border border-dark-500 px-5 py-4 flex items-center justify-between hover:bg-dark-700/30 transition-colors">
                  <div className="flex-1">
                    <div className="flex items-center gap-2 mb-1">
                      <span className={`inline-flex px-2 py-0.5 rounded-full text-xs font-medium font-mono ${statusColors[inc.status] || "bg-dark-700 text-dark-200"}`}>{inc.status}</span>
                      <span className={`inline-flex px-2 py-0.5 rounded-full text-xs font-medium font-mono ${severityColors[inc.severity] || "bg-dark-700 text-dark-200"}`}>{inc.severity}</span>
                    </div>
                    <p className="text-sm text-dark-50 font-mono">{inc.title}</p>
                    <p className="text-xs text-dark-300 font-mono mt-1">
                      Started {timeAgo(inc.started_at)}
                      {inc.resolved_at && <> — resolved {timeAgo(inc.resolved_at)}</>}
                    </p>
                  </div>
                  <div className="flex gap-2">
                    <button onClick={() => viewUpdates(inc.id)}
                      className="px-3 py-1 bg-accent-500/10 text-accent-400 rounded-md text-xs font-medium font-mono hover:bg-accent-500/20">
                      Updates
                    </button>
                    <button onClick={() => setPendingDelete({ type: "incident", id: inc.id, label: `Delete incident "${inc.title}"?` })}
                      className="px-3 py-1 bg-danger-500/10 text-danger-400 rounded-md text-xs font-medium font-mono hover:bg-danger-500/20">
                      Delete
                    </button>
                  </div>
                </div>
              ))}
            </div>
          )}
        </div>
      )}

      {tab === "components" && (
        <div className="space-y-4">
          <div className="flex justify-end">
            <button onClick={() => setShowCompForm(!showCompForm)}
              className="px-4 py-2 bg-rust-500 text-white rounded-lg text-sm font-medium font-mono hover:bg-rust-600">
              {showCompForm ? "Cancel" : "Add Component"}
            </button>
          </div>

          {showCompForm && (
            <div className="bg-dark-800 rounded-lg border border-dark-500 p-5 space-y-3">
              <div className="grid grid-cols-2 gap-3">
                <div>
                  <label className="block text-xs font-medium text-dark-100 mb-1 font-mono">Name</label>
                  <input type="text" value={compForm.name} onChange={e => setCompForm({ ...compForm, name: e.target.value })}
                    placeholder="e.g., API, Website, Database"
                    className="w-full px-3 py-2 bg-dark-900 border border-dark-500 rounded-lg text-sm font-mono text-dark-50 outline-none" />
                </div>
                <div>
                  <label className="block text-xs font-medium text-dark-100 mb-1 font-mono">Group</label>
                  <input type="text" value={compForm.group_name} onChange={e => setCompForm({ ...compForm, group_name: e.target.value })}
                    placeholder="e.g., Core Services"
                    className="w-full px-3 py-2 bg-dark-900 border border-dark-500 rounded-lg text-sm font-mono text-dark-50 outline-none" />
                </div>
              </div>
              <div>
                <label className="block text-xs font-medium text-dark-100 mb-1 font-mono">Linked Monitors</label>
                <div className="flex flex-wrap gap-2">
                  {monitors.map(m => (
                    <label key={m.id} className="flex items-center gap-1 text-xs font-mono text-dark-100">
                      <input type="checkbox"
                        checked={compForm.monitor_ids.includes(m.id)}
                        onChange={e => {
                          setCompForm({
                            ...compForm,
                            monitor_ids: e.target.checked
                              ? [...compForm.monitor_ids, m.id]
                              : compForm.monitor_ids.filter(id => id !== m.id)
                          });
                        }} />
                      {m.name}
                    </label>
                  ))}
                </div>
              </div>
              <div className="flex justify-end">
                <button onClick={createComponent}
                  className="px-4 py-2 bg-rust-500 text-white rounded-lg text-sm font-medium font-mono hover:bg-rust-600">Create</button>
              </div>
            </div>
          )}

          {components.length === 0 ? (
            <div className="p-12 text-center">
              <p className="text-dark-200 text-sm font-mono">No components configured</p>
              <p className="text-dark-300 text-xs mt-1 font-mono">Add components to organize your status page</p>
            </div>
          ) : (
            <div className="bg-dark-800 rounded-lg border border-dark-500 overflow-x-auto">
              <table className="w-full">
                <thead>
                  <tr className="bg-dark-900 border-b border-dark-500">
                    {["Name", "Group", "Monitors", ""].map(h => (
                      <th key={h} className="text-left text-xs font-medium text-dark-200 uppercase font-mono tracking-widest px-5 py-3">{h}</th>
                    ))}
                  </tr>
                </thead>
                <tbody className="divide-y divide-dark-600">
                  {components.map(c => (
                    <tr key={c.id} className="hover:bg-dark-700/30 transition-colors">
                      <td className="px-5 py-4 text-sm text-dark-50 font-mono">{c.name}</td>
                      <td className="px-5 py-4 text-sm text-dark-200 font-mono">{c.group_name || "-"}</td>
                      <td className="px-5 py-4 text-sm text-dark-200 font-mono">{c.monitor_ids?.length || 0} linked</td>
                      <td className="px-5 py-4">
                        <button onClick={() => setPendingDelete({ type: "component", id: c.id, label: `Delete component "${c.name}"?` })}
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

      {tab === "settings" && !config && (
        <div className="bg-dark-800 rounded-lg border border-dark-500 p-8 text-center">
          <p className="text-dark-300 text-sm">Unable to load status page settings. Try refreshing.</p>
        </div>
      )}
      {tab === "settings" && config && (
        <div className="bg-dark-800 rounded-lg border border-dark-500 p-5 space-y-4">
          <div className="grid grid-cols-2 gap-3">
            <div>
              <label className="block text-xs font-medium text-dark-100 mb-1 font-mono">Page Title</label>
              <input type="text" value={config.title} onChange={e => setConfig({ ...config, title: e.target.value })}
                className="w-full px-3 py-2 bg-dark-900 border border-dark-500 rounded-lg text-sm font-mono text-dark-50 outline-none" />
            </div>
            <div>
              <label className="block text-xs font-medium text-dark-100 mb-1 font-mono">Accent Color</label>
              <input type="text" value={config.accent_color} onChange={e => setConfig({ ...config, accent_color: e.target.value })}
                className="w-full px-3 py-2 bg-dark-900 border border-dark-500 rounded-lg text-sm font-mono text-dark-50 outline-none" />
            </div>
          </div>
          <div>
            <label className="block text-xs font-medium text-dark-100 mb-1 font-mono">Description</label>
            <textarea value={config.description} onChange={e => setConfig({ ...config, description: e.target.value })} rows={2}
              className="w-full px-3 py-2 bg-dark-900 border border-dark-500 rounded-lg text-sm font-mono text-dark-50 outline-none resize-none" />
          </div>
          <div>
            <label className="block text-xs font-medium text-dark-100 mb-1 font-mono">Logo URL</label>
            <input type="text" value={config.logo_url || ""} onChange={e => setConfig({ ...config, logo_url: e.target.value || null })}
              className="w-full px-3 py-2 bg-dark-900 border border-dark-500 rounded-lg text-sm font-mono text-dark-50 outline-none" />
          </div>
          <div className="flex gap-6 text-sm font-mono">
            <label className="flex items-center gap-2 text-dark-100">
              <input type="checkbox" checked={config.enabled} onChange={e => setConfig({ ...config, enabled: e.target.checked })} /> Enabled
            </label>
            <label className="flex items-center gap-2 text-dark-100">
              <input type="checkbox" checked={config.show_subscribe} onChange={e => setConfig({ ...config, show_subscribe: e.target.checked })} /> Show Subscribe
            </label>
            <label className="flex items-center gap-2 text-dark-100">
              <input type="checkbox" checked={config.show_incident_history} onChange={e => setConfig({ ...config, show_incident_history: e.target.checked })} /> Show History
            </label>
          </div>
          <div className="flex justify-end">
            <button onClick={saveConfig}
              className="px-4 py-2 bg-rust-500 text-white rounded-lg text-sm font-medium font-mono hover:bg-rust-600">Save Settings</button>
          </div>
        </div>
      )}
    </div>
  );
}
