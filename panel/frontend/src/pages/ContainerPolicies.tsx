import { useState, useEffect } from "react";
import { Navigate } from "react-router-dom";
import { useAuth } from "../context/AuthContext";
import { api } from "../api";

interface Policy {
  id: string;
  user_id: string;
  user_email: string | null;
  max_containers: number;
  max_memory_mb: number;
  max_cpu_percent: number;
  allowed_images: string | null;
  created_at: string;
  updated_at: string;
  used: number;
}

interface User {
  id: string;
  email: string;
  role: string;
}

export default function ContainerPolicies() {
  const { user: authUser } = useAuth();
  // Ahead of the loader effect: a redirect is an effect too, so a guard below it
  // lets the mount spend its requests on a 403 first.
  if (authUser?.role !== "admin") return <Navigate to="/" replace />;
  const [policies, setPolicies] = useState<Policy[]>([]);
  const [users, setUsers] = useState<User[]>([]);
  const [loading, setLoading] = useState(true);
  const [message, setMessage] = useState({ text: "", type: "" });
  const [unreachableServers, setUnreachableServers] = useState(0);

  // Form state
  const [showForm, setShowForm] = useState(false);
  const [editUserId, setEditUserId] = useState("");
  const [formUserId, setFormUserId] = useState("");
  const [maxContainers, setMaxContainers] = useState(10);
  const [maxMemory, setMaxMemory] = useState(4096);
  const [maxCpu, setMaxCpu] = useState(400);
  const [allowedImages, setAllowedImages] = useState("");
  const [saving, setSaving] = useState(false);

  const loadPolicies = async () => {
    try {
      const data = await api.get<{ policies: Policy[]; servers_unreachable: number }>("/container-policies");
      setPolicies(data.policies || []);
      setUnreachableServers(data.servers_unreachable || 0);
    } catch (e) {
      setMessage({ text: e instanceof Error ? e.message : "Failed to load policies", type: "error" });
    } finally {
      setLoading(false);
    }
  };

  const usagePct = (used: number, max: number) => max ? Math.min(100, Math.round((used / max) * 100)) : 0;
  const usageBarColor = (used: number, max: number) => {
    const p = max ? (used / max) * 100 : 0;
    return p >= 90 ? "bg-danger-500" : p >= 70 ? "bg-warn-500" : "bg-rust-500";
  };

  const loadUsers = async () => {
    try {
      const data = await api.get<User[]>("/users");
      setUsers(data);
    } catch { /* ignore */ }
  };

  useEffect(() => {
    loadPolicies();
    loadUsers();
  }, []);

  const resetForm = () => {
    setEditUserId("");
    setFormUserId("");
    setMaxContainers(10);
    setMaxMemory(4096);
    setMaxCpu(400);
    setAllowedImages("");
    setShowForm(false);
  };

  const openEdit = (p: Policy) => {
    setEditUserId(p.user_id);
    setFormUserId(p.user_id);
    setMaxContainers(p.max_containers);
    setMaxMemory(p.max_memory_mb);
    setMaxCpu(p.max_cpu_percent);
    setAllowedImages(p.allowed_images || "");
    setShowForm(true);
  };

  const handleSave = async () => {
    setSaving(true);
    setMessage({ text: "", type: "" });
    try {
      if (editUserId) {
        await api.put(`/container-policies/${editUserId}`, {
          max_containers: maxContainers,
          max_memory_mb: maxMemory,
          max_cpu_percent: maxCpu,
          // Empty clears the allow-list (back to "any image"); null would
          // have meant "leave it", which is why it could never be undone.
          allowed_images: allowedImages,
        });
        setMessage({ text: "Policy updated", type: "success" });
      } else {
        await api.post("/container-policies", {
          user_id: formUserId,
          max_containers: maxContainers,
          max_memory_mb: maxMemory,
          max_cpu_percent: maxCpu,
          allowed_images: allowedImages || null,
        });
        setMessage({ text: "Policy created", type: "success" });
      }
      resetForm();
      loadPolicies();
    } catch (e) {
      setMessage({ text: e instanceof Error ? e.message : "Failed to save", type: "error" });
    } finally {
      setSaving(false);
    }
  };

  const handleDelete = async (userId: string) => {
    try {
      await api.delete(`/container-policies/${userId}`);
      setPolicies(prev => prev.filter(p => p.user_id !== userId));
      setMessage({ text: "Policy removed", type: "success" });
    } catch (e) {
      setMessage({ text: e instanceof Error ? e.message : "Failed", type: "error" });
    }
  };

  // Users without policies
  const usersWithoutPolicy = users.filter(u => !policies.some(p => p.user_id === u.id));

  return (
    <div className="max-w-5xl mx-auto space-y-6">
      <div className="flex items-center justify-between">
        <div>
          <h1 className="text-xl font-bold text-dark-50">Container Policies</h1>
          <p className="text-sm text-dark-300 mt-1">Manage per-user container limits and resource quotas</p>
        </div>
        <button
          onClick={() => { resetForm(); setShowForm(true); }}
          className="flex items-center gap-2 px-4 py-2 bg-rust-500 text-white rounded-lg text-sm font-medium hover:bg-rust-600 transition-colors"
        >
          <svg className="w-4 h-4" fill="none" stroke="currentColor" viewBox="0 0 24 24"><path strokeLinecap="round" strokeLinejoin="round" strokeWidth={2} d="M12 4v16m8-8H4" /></svg>
          Add Policy
        </button>
      </div>

      {message.text && (
        <div className={`px-4 py-3 rounded-lg text-sm border ${message.type === "success" ? "bg-rust-500/10 text-rust-400 border-rust-500/20" : "bg-danger-500/10 text-danger-400 border-danger-500/20"}`}>
          {message.text}
        </div>
      )}

      {/* Policy Form Modal */}
      {showForm && (
        <div className="fixed inset-0 bg-black/60 flex items-center justify-center z-50 p-4" onClick={resetForm}>
          <div className="bg-dark-800 rounded-lg border border-dark-500 p-6 w-full max-w-lg" onClick={e => e.stopPropagation()}>
            <h2 className="text-lg font-semibold text-dark-50 mb-4">
              {editUserId ? "Edit Policy" : "Create Policy"}
            </h2>

            <div className="space-y-4">
              {!editUserId && (
                <div>
                  <label className="block text-sm text-dark-200 mb-1">User</label>
                  <select
                    value={formUserId}
                    onChange={e => setFormUserId(e.target.value)}
                    className="w-full px-3 py-2 border border-dark-500 rounded-lg text-sm"
                  >
                    <option value="">Select a user...</option>
                    {usersWithoutPolicy.map(u => (
                      <option key={u.id} value={u.id}>{u.email} ({u.role})</option>
                    ))}
                  </select>
                </div>
              )}

              <div className="grid grid-cols-2 gap-4">
                <div>
                  <label className="block text-sm text-dark-200 mb-1">Max Containers</label>
                  <input
                    type="number" min={1} max={1000}
                    value={maxContainers}
                    onChange={e => setMaxContainers(parseInt(e.target.value) || 1)}
                    className="w-full px-3 py-2 border border-dark-500 rounded-lg text-sm"
                  />
                </div>
                <div>
                  <label className="block text-sm text-dark-200 mb-1">Max Memory (MB)</label>
                  <input
                    type="number" min={128} max={1048576}
                    value={maxMemory}
                    onChange={e => setMaxMemory(parseInt(e.target.value) || 128)}
                    className="w-full px-3 py-2 border border-dark-500 rounded-lg text-sm"
                  />
                </div>
              </div>

              <div>
                <label className="block text-sm text-dark-200 mb-1">Max CPU (%)</label>
                <input
                  type="number" min={10} max={10000}
                  value={maxCpu}
                  onChange={e => setMaxCpu(parseInt(e.target.value) || 10)}
                  className="w-full px-3 py-2 border border-dark-500 rounded-lg text-sm"
                />
                <p className="text-xs text-dark-400 mt-1">100% = 1 CPU core. 400% = 4 cores.</p>
              </div>

              <div>
                <label className="block text-sm text-dark-200 mb-1">Allowed Images (comma-separated, optional)</label>
                <input
                  type="text"
                  value={allowedImages}
                  onChange={e => setAllowedImages(e.target.value)}
                  className="w-full px-3 py-2 border border-dark-500 rounded-lg text-sm"
                  placeholder="postgres, redis, nginx (or * for all)"
                />
              </div>
            </div>

            <div className="flex justify-end gap-2 mt-6">
              <button onClick={resetForm} className="px-4 py-2 text-sm text-dark-300 border border-dark-600 rounded-lg hover:text-dark-100 hover:border-dark-400 transition-colors">
                Cancel
              </button>
              <button
                onClick={handleSave}
                disabled={saving || (!editUserId && !formUserId)}
                className="px-4 py-2 bg-rust-500 text-white rounded-lg text-sm font-medium hover:bg-rust-600 disabled:opacity-50 transition-colors"
              >
                {saving ? "Saving..." : editUserId ? "Update" : "Create"}
              </button>
            </div>
          </div>
        </div>
      )}

      {unreachableServers > 0 && (
        <div className="px-4 py-2 text-xs text-warn-400 bg-warn-500/10 border border-warn-500/20 rounded-lg">
          {unreachableServers} server{unreachableServers === 1 ? "" : "s"} did not respond — the usage counts below are a floor, not exact.
        </div>
      )}

      {/* Policies Table */}
      <div className="bg-dark-800 rounded-lg border border-dark-500 overflow-hidden">
        <table className="w-full text-sm">
          <thead>
            <tr className="border-b border-dark-600 bg-dark-700/50">
              <th className="text-left px-4 py-3 text-xs font-medium text-dark-300 uppercase">User</th>
              <th className="text-center px-4 py-3 text-xs font-medium text-dark-300 uppercase">Containers</th>
              <th className="text-center px-4 py-3 text-xs font-medium text-dark-300 uppercase">Memory</th>
              <th className="text-center px-4 py-3 text-xs font-medium text-dark-300 uppercase">CPU</th>
              <th className="text-right px-4 py-3 text-xs font-medium text-dark-300 uppercase">Actions</th>
            </tr>
          </thead>
          <tbody>
            {loading ? (
              <tr><td colSpan={5} className="text-center py-8 text-dark-400">Loading...</td></tr>
            ) : policies.length === 0 ? (
              <tr><td colSpan={5} className="text-center py-8 text-dark-400">No policies configured. Users have unlimited container access by default.</td></tr>
            ) : (
              policies.map(p => (
                <tr key={p.id} className="border-b border-dark-700 hover:bg-dark-700/30">
                  <td className="px-4 py-3">
                    <p className="text-dark-100">{p.user_email || "Unknown"}</p>
                    <p className="text-xs text-dark-400 font-mono">{p.user_id.slice(0, 8)}...</p>
                  </td>
                  <td className="text-center px-4 py-3 text-dark-200">
                    <div className="inline-flex flex-col items-center gap-1 w-16">
                      <span className="font-mono">{p.used}/{p.max_containers}</span>
                      <div className="w-full h-1.5 bg-dark-700 rounded-full overflow-hidden">
                        <div
                          className={`h-full ${usageBarColor(p.used, p.max_containers)} rounded-full transition-all`}
                          style={{ width: `${usagePct(p.used, p.max_containers)}%` }}
                        />
                      </div>
                    </div>
                  </td>
                  <td className="text-center px-4 py-3 text-dark-200">{p.max_memory_mb >= 1024 ? `${(p.max_memory_mb / 1024).toFixed(1)}GB` : `${p.max_memory_mb}MB`}</td>
                  <td className="text-center px-4 py-3 text-dark-200">{p.max_cpu_percent}%</td>
                  <td className="text-right px-4 py-3">
                    <button onClick={() => openEdit(p)} className="text-xs text-accent-400 hover:text-accent-300 mr-3">Edit</button>
                    <button onClick={() => handleDelete(p.user_id)} className="text-xs text-danger-400 hover:text-danger-300">Remove</button>
                  </td>
                </tr>
              ))
            )}
          </tbody>
        </table>
      </div>

      {/* Info */}
      <div className="bg-dark-800 rounded-lg border border-dark-500 p-5">
        <h3 className="text-xs font-medium text-dark-300 uppercase font-mono tracking-widest mb-3">How It Works</h3>
        <div className="space-y-2 text-sm text-dark-200">
          <p><strong className="text-dark-100">Container limits</strong> — Maximum number of Docker containers a user can deploy, counted across every server they can reach, not just one. Counted when deploying from a template; a Compose file or a stack is not counted against it.</p>
          <p><strong className="text-dark-100">Memory &amp; CPU</strong> — Maximum resources per container deployment, applied to template deploys.</p>
          <p><strong className="text-dark-100">Allowed images</strong> — Comma-separated. Checked wherever an image is chosen: template deploys, changing an app&rsquo;s image, Compose and stacks. Leave empty for no restriction.</p>
          <p className="text-xs text-dark-400">
            An entry matches the whole reference (<span className="font-mono">nginx:1.27</span>), or the repository it
            tags (<span className="font-mono">nginx</span> allows <span className="font-mono">nginx:1.27</span> but not
            <span className="font-mono"> evil/nginx-backdoor</span>), or a repository prefix ending in
            <span className="font-mono"> /*</span> (<span className="font-mono">ghcr.io/myorg/*</span>). A template&rsquo;s
            own id is also accepted, exactly. <span className="font-mono">*</span> allows everything.
          </p>
          <p className="text-xs text-dark-400 mt-2">Users without a policy have no container limits applied. Only administrators can deploy containers, so a policy on any other account has nothing to restrict.</p>
        </div>
      </div>
    </div>
  );
}
