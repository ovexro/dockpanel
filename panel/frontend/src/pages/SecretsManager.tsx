import { useAuth } from "../context/AuthContext";
import { Navigate } from "react-router-dom";
import { useState, useEffect } from "react";
import { api } from "../api";
import { formatDate } from "../utils/format";

interface Vault {
  id: string;
  name: string;
  description: string | null;
  site_id: string | null;
  created_at: string;
}

interface Secret {
  id: string;
  vault_id: string;
  key: string;
  value: string;
  description: string | null;
  secret_type: string;
  auto_inject: boolean;
  version: number;
  updated_by: string | null;
  updated_at: string;
}

interface SecretVersion {
  id: string;
  version: number;
  changed_by: string | null;
  change_type: string;
  created_at: string;
}

const SECRET_TYPES = ["env", "api_key", "password", "certificate", "custom"];

const typeColors: Record<string, string> = {
  env: "bg-rust-500/15 text-rust-400",
  api_key: "bg-accent-500/15 text-accent-400",
  password: "bg-danger-500/15 text-danger-400",
  certificate: "bg-warn-500/15 text-warn-400",
  custom: "bg-dark-700 text-dark-200",
};

export default function SecretsManager() {
  const { user } = useAuth();
  if (!user || user.role !== "admin") return <Navigate to="/" replace />;
  const [vaults, setVaults] = useState<Vault[]>([]);
  const [selectedVault, setSelectedVault] = useState<string | null>(null);
  const [secrets, setSecrets] = useState<Secret[]>([]);
  const [versions, setVersions] = useState<SecretVersion[]>([]);
  const [viewingVersions, setViewingVersions] = useState<string | null>(null);
  const [loading, setLoading] = useState(true);
  const [message, setMessage] = useState({ text: "", type: "" });
  const [reveal, setReveal] = useState(false);

  // Vault form
  const [showVaultForm, setShowVaultForm] = useState(false);
  const [vaultForm, setVaultForm] = useState({ name: "", description: "" });

  // Secret form
  const [showSecretForm, setShowSecretForm] = useState(false);
  const [secretForm, setSecretForm] = useState({ key: "", value: "", description: "", secret_type: "env", auto_inject: false });

  // Edit form
  const [editingSecret, setEditingSecret] = useState<string | null>(null);
  const [editForm, setEditForm] = useState({ value: "", description: "" });

  // Vault edit form
  const [editingVault, setEditingVault] = useState<string | null>(null);
  const [editVaultForm, setEditVaultForm] = useState({ name: "", description: "" });

  useEffect(() => { loadVaults(); }, []);
  useEffect(() => { if (selectedVault) loadSecrets(); }, [selectedVault, reveal]);

  const loadVaults = async () => {
    setLoading(true);
    try {
      const v = await api.get<Vault[]>("/secrets/vaults");
      setVaults(v);
      if (v.length > 0 && !selectedVault) setSelectedVault(v[0].id);
    } catch (e) {
      setMessage({ text: e instanceof Error ? e.message : "Failed", type: "error" });
    } finally {
      setLoading(false);
    }
  };

  const loadSecrets = async () => {
    if (!selectedVault) return;
    try {
      const s = await api.get<Secret[]>(`/secrets/vaults/${selectedVault}/secrets?reveal=${reveal}`);
      setSecrets(s);
    } catch (e) {
      setMessage({ text: e instanceof Error ? e.message : "Failed", type: "error" });
    }
  };

  const createVault = async () => {
    try {
      const v = await api.post<Vault>("/secrets/vaults", vaultForm);
      setVaults([v, ...vaults]);
      setSelectedVault(v.id);
      setShowVaultForm(false);
      setVaultForm({ name: "", description: "" });
      setMessage({ text: "Vault created", type: "success" });
    } catch (e) {
      setMessage({ text: e instanceof Error ? e.message : "Failed", type: "error" });
    }
  };

  const deleteVault = async (id: string) => {
    try {
      await api.delete(`/secrets/vaults/${id}`);
      setVaults(vaults.filter(v => v.id !== id));
      if (selectedVault === id) setSelectedVault(vaults.find(v => v.id !== id)?.id || null);
    } catch (e) {
      setMessage({ text: e instanceof Error ? e.message : "Failed", type: "error" });
    }
  };

  const updateVault = async () => {
    if (!editingVault) return;
    try {
      await api.put(`/secrets/vaults/${editingVault}`, {
        name: editVaultForm.name || undefined,
        description: editVaultForm.description || undefined,
      });
      setEditingVault(null);
      setMessage({ text: "Vault updated", type: "success" });
      await loadVaults();
    } catch (e) {
      setMessage({ text: e instanceof Error ? e.message : "Failed", type: "error" });
    }
  };

  const createSecret = async () => {
    if (!selectedVault) return;
    try {
      await api.post(`/secrets/vaults/${selectedVault}/secrets`, secretForm);
      setShowSecretForm(false);
      setSecretForm({ key: "", value: "", description: "", secret_type: "env", auto_inject: false });
      setMessage({ text: "Secret created (encrypted)", type: "success" });
      loadSecrets();
    } catch (e) {
      setMessage({ text: e instanceof Error ? e.message : "Failed", type: "error" });
    }
  };

  const updateSecret = async () => {
    if (!selectedVault || !editingSecret) return;
    try {
      await api.put(`/secrets/vaults/${selectedVault}/secrets/${editingSecret}`, {
        value: editForm.value || undefined,
        description: editForm.description || undefined,
      });
      setEditingSecret(null);
      setMessage({ text: "Secret updated", type: "success" });
      loadSecrets();
    } catch (e) {
      setMessage({ text: e instanceof Error ? e.message : "Failed", type: "error" });
    }
  };

  const deleteSecret = async (id: string) => {
    if (!selectedVault) return;
    try {
      await api.delete(`/secrets/vaults/${selectedVault}/secrets/${id}`);
      setSecrets(secrets.filter(s => s.id !== id));
    } catch (e) {
      setMessage({ text: e instanceof Error ? e.message : "Failed", type: "error" });
    }
  };

  const viewVersionHistory = async (secretId: string) => {
    if (!selectedVault) return;
    setViewingVersions(secretId);
    try {
      const v = await api.get<SecretVersion[]>(`/secrets/vaults/${selectedVault}/secrets/${secretId}/versions`);
      setVersions(v);
    } catch { setVersions([]); }
  };

  if (loading) return <div className="p-8 text-center text-dark-300 font-mono">Loading secrets...</div>;

  return (
    <div className="p-6 lg:p-8">
      <div className="flex flex-col sm:flex-row sm:items-center justify-between gap-3 mb-6 pb-4 border-b border-dark-600">
        <div>
          <h1 className="text-sm font-medium text-dark-300 uppercase font-mono tracking-widest">Secrets Manager</h1>
          <p className="text-sm text-dark-200 mt-1 font-mono">AES-256-GCM encrypted secrets with version history</p>
        </div>
        <button onClick={() => setShowVaultForm(!showVaultForm)}
          className="px-4 py-2 bg-rust-500 text-white rounded-lg text-sm font-medium font-mono hover:bg-rust-600">
          {showVaultForm ? "Cancel" : "New Vault"}
        </button>
      </div>

      {message.text && (
        <div className={`mb-4 px-4 py-3 rounded-lg text-sm border font-mono ${
          message.type === "success" ? "bg-rust-500/10 text-rust-400 border-rust-500/20" : "bg-danger-500/10 text-danger-400 border-danger-500/20"
        }`}>{message.text}</div>
      )}

      {/* New vault form */}
      {showVaultForm && (
        <div className="bg-dark-800 rounded-lg border border-dark-500 p-5 mb-4 space-y-3">
          <div className="grid grid-cols-2 gap-3">
            <div>
              <label className="block text-xs font-medium text-dark-100 mb-1 font-mono">Vault Name</label>
              <input type="text" value={vaultForm.name} onChange={e => setVaultForm({ ...vaultForm, name: e.target.value })}
                placeholder="e.g., Production API Keys"
                className="w-full px-3 py-2 bg-dark-900 border border-dark-500 rounded-lg text-sm font-mono text-dark-50 outline-none" />
            </div>
            <div>
              <label className="block text-xs font-medium text-dark-100 mb-1 font-mono">Description</label>
              <input type="text" value={vaultForm.description} onChange={e => setVaultForm({ ...vaultForm, description: e.target.value })}
                className="w-full px-3 py-2 bg-dark-900 border border-dark-500 rounded-lg text-sm font-mono text-dark-50 outline-none" />
            </div>
          </div>
          <div className="flex justify-end">
            <button onClick={createVault} className="px-4 py-2 bg-rust-500 text-white rounded-lg text-sm font-medium font-mono hover:bg-rust-600">Create Vault</button>
          </div>
        </div>
      )}

      <div className="flex gap-6">
        {/* Vault sidebar */}
        <div className="w-56 space-y-1 shrink-0">
          {vaults.map(v => (
            <div key={v.id}>
              {editingVault === v.id ? (
                <div className="px-3 py-2 rounded-lg bg-dark-700 border border-accent-500/30 space-y-2">
                  <div>
                    <label className="block text-xs font-medium text-dark-100 mb-1 font-mono">Name</label>
                    <input type="text" value={editVaultForm.name} onChange={e => setEditVaultForm({ ...editVaultForm, name: e.target.value })}
                      className="w-full px-2 py-1 bg-dark-900 border border-dark-500 rounded text-sm font-mono text-dark-50 outline-none" />
                  </div>
                  <div>
                    <label className="block text-xs font-medium text-dark-100 mb-1 font-mono">Description</label>
                    <input type="text" value={editVaultForm.description} onChange={e => setEditVaultForm({ ...editVaultForm, description: e.target.value })}
                      className="w-full px-2 py-1 bg-dark-900 border border-dark-500 rounded text-sm font-mono text-dark-50 outline-none" />
                  </div>
                  <div className="flex justify-end gap-1">
                    <button onClick={() => setEditingVault(null)} className="px-2 py-1 text-dark-200 text-xs font-mono">Cancel</button>
                    <button onClick={updateVault} className="px-2 py-1 bg-accent-500 text-white rounded text-xs font-medium font-mono">Save</button>
                  </div>
                </div>
              ) : (
                <div
                  onClick={() => { setSelectedVault(v.id); setReveal(false); }}
                  className={`px-3 py-2 rounded-lg cursor-pointer flex items-center justify-between group transition-colors ${
                    selectedVault === v.id ? "bg-dark-700 border border-dark-500" : "hover:bg-dark-800"
                  }`}>
                  <div>
                    <p className="text-sm text-dark-50 font-mono">{v.name}</p>
                    {v.description && <p className="text-xs text-dark-300 font-mono">{v.description}</p>}
                  </div>
                  <div className="flex gap-1 opacity-0 group-hover:opacity-100">
                    <button onClick={e => { e.stopPropagation(); setEditingVault(v.id); setEditVaultForm({ name: v.name, description: v.description || "" }); }}
                      className="text-dark-400 hover:text-accent-400 text-xs">
                      Edit
                    </button>
                    <button onClick={e => { e.stopPropagation(); deleteVault(v.id); }}
                      className="text-dark-400 hover:text-danger-400 text-xs">
                      Del
                    </button>
                  </div>
                </div>
              )}
            </div>
          ))}
          {vaults.length === 0 && <p className="text-xs text-dark-300 font-mono px-3 py-2">No vaults yet</p>}
        </div>

        {/* Secrets area */}
        <div className="flex-1 space-y-4">
          {selectedVault && (
            <>
              <div className="flex items-center justify-between">
                <div className="flex items-center gap-3">
                  <button onClick={() => setShowSecretForm(!showSecretForm)}
                    className="px-3 py-1.5 bg-rust-500 text-white rounded-lg text-xs font-medium font-mono hover:bg-rust-600">
                    {showSecretForm ? "Cancel" : "Add Secret"}
                  </button>
                  <label className="flex items-center gap-1.5 text-xs font-mono text-dark-200 cursor-pointer">
                    <input type="checkbox" checked={reveal} onChange={e => setReveal(e.target.checked)} />
                    Reveal values
                  </label>
                </div>
              </div>

              {/* Add secret form */}
              {showSecretForm && (
                <div className="bg-dark-800 rounded-lg border border-dark-500 p-4 space-y-3">
                  <div className="grid grid-cols-3 gap-3">
                    <div>
                      <label className="block text-xs font-medium text-dark-100 mb-1 font-mono">Key</label>
                      <input type="text" value={secretForm.key} onChange={e => setSecretForm({ ...secretForm, key: e.target.value })}
                        placeholder="API_KEY"
                        className="w-full px-3 py-2 bg-dark-900 border border-dark-500 rounded-lg text-sm font-mono text-dark-50 outline-none" />
                    </div>
                    <div>
                      <label className="block text-xs font-medium text-dark-100 mb-1 font-mono">Value</label>
                      <input type="password" value={secretForm.value} onChange={e => setSecretForm({ ...secretForm, value: e.target.value })}
                        className="w-full px-3 py-2 bg-dark-900 border border-dark-500 rounded-lg text-sm font-mono text-dark-50 outline-none" />
                    </div>
                    <div>
                      <label className="block text-xs font-medium text-dark-100 mb-1 font-mono">Type</label>
                      <select value={secretForm.secret_type} onChange={e => setSecretForm({ ...secretForm, secret_type: e.target.value })}
                        className="w-full px-3 py-2 bg-dark-900 border border-dark-500 rounded-lg text-sm font-mono text-dark-50 outline-none">
                        {SECRET_TYPES.map(t => <option key={t} value={t}>{t}</option>)}
                      </select>
                    </div>
                  </div>
                  <div className="flex items-center gap-4">
                    <label className="flex items-center gap-1.5 text-xs font-mono text-dark-100">
                      <input type="checkbox" checked={secretForm.auto_inject} onChange={e => setSecretForm({ ...secretForm, auto_inject: e.target.checked })} />
                      Auto-inject into .env on deploy
                    </label>
                    <button onClick={createSecret} className="ml-auto px-3 py-1.5 bg-rust-500 text-white rounded-lg text-xs font-medium font-mono hover:bg-rust-600">Save</button>
                  </div>
                </div>
              )}

              {/* Edit form */}
              {editingSecret && (
                <div className="bg-dark-800 rounded-lg border border-accent-500/30 p-4 space-y-3">
                  <h3 className="text-xs font-medium text-accent-400 uppercase font-mono tracking-widest">Update Secret</h3>
                  <div className="grid grid-cols-2 gap-3">
                    <div>
                      <label className="block text-xs font-medium text-dark-100 mb-1 font-mono">New Value (leave empty to keep current)</label>
                      <input type="password" value={editForm.value} onChange={e => setEditForm({ ...editForm, value: e.target.value })}
                        className="w-full px-3 py-2 bg-dark-900 border border-dark-500 rounded-lg text-sm font-mono text-dark-50 outline-none" />
                    </div>
                    <div>
                      <label className="block text-xs font-medium text-dark-100 mb-1 font-mono">Description</label>
                      <input type="text" value={editForm.description} onChange={e => setEditForm({ ...editForm, description: e.target.value })}
                        className="w-full px-3 py-2 bg-dark-900 border border-dark-500 rounded-lg text-sm font-mono text-dark-50 outline-none" />
                    </div>
                  </div>
                  <div className="flex justify-end gap-2">
                    <button onClick={() => setEditingSecret(null)} className="px-3 py-1.5 text-dark-200 text-xs font-mono">Cancel</button>
                    <button onClick={updateSecret} className="px-3 py-1.5 bg-accent-500 text-white rounded-lg text-xs font-medium font-mono">Update</button>
                  </div>
                </div>
              )}

              {/* Version history panel */}
              {viewingVersions && (
                <div className="bg-dark-800 rounded-lg border border-dark-500 p-4">
                  <div className="flex items-center justify-between mb-3">
                    <h3 className="text-xs font-medium text-dark-300 uppercase font-mono tracking-widest">Version History</h3>
                    <button onClick={() => setViewingVersions(null)} className="text-dark-300 hover:text-dark-100 text-xs font-mono">Close</button>
                  </div>
                  {versions.length === 0 ? (
                    <p className="text-xs text-dark-300 font-mono">No version history</p>
                  ) : (
                    <div className="space-y-2">
                      {versions.map(v => (
                        <div key={v.id} className="flex items-center gap-3 text-xs font-mono">
                          <span className="text-dark-50">v{v.version}</span>
                          <span className={`px-2 py-0.5 rounded-full ${v.change_type === "create" ? "bg-rust-500/15 text-rust-400" : "bg-accent-500/15 text-accent-400"}`}>{v.change_type}</span>
                          <span className="text-dark-300">{v.changed_by || "system"}</span>
                          <span className="text-dark-400 ml-auto">{formatDate(v.created_at)}</span>
                        </div>
                      ))}
                    </div>
                  )}
                </div>
              )}

              {/* Secrets table */}
              {secrets.length === 0 ? (
                <div className="p-12 text-center">
                  <p className="text-dark-200 text-sm font-mono">No secrets in this vault</p>
                  <p className="text-dark-300 text-xs mt-1 font-mono">Add your first secret above</p>
                </div>
              ) : (
                <div className="bg-dark-800 rounded-lg border border-dark-500 overflow-x-auto">
                  <table className="w-full">
                    <thead>
                      <tr className="bg-dark-900 border-b border-dark-500">
                        {["Key", "Value", "Type", "Inject", "Ver", "Updated", ""].map(h => (
                          <th key={h} className="text-left text-xs font-medium text-dark-200 uppercase font-mono tracking-widest px-4 py-3">{h}</th>
                        ))}
                      </tr>
                    </thead>
                    <tbody className="divide-y divide-dark-600">
                      {secrets.map(s => (
                        <tr key={s.id} className="hover:bg-dark-700/30 transition-colors">
                          <td className="px-4 py-3 text-sm text-dark-50 font-mono font-medium">{s.key}</td>
                          <td className="px-4 py-3 text-sm text-dark-200 font-mono">{s.value}</td>
                          <td className="px-4 py-3">
                            <span className={`inline-flex px-2 py-0.5 rounded-full text-xs font-medium font-mono ${typeColors[s.secret_type] || "bg-dark-700 text-dark-200"}`}>{s.secret_type}</span>
                          </td>
                          <td className="px-4 py-3 text-xs text-dark-300 font-mono">{s.auto_inject ? "Yes" : "-"}</td>
                          <td className="px-4 py-3 text-xs text-dark-300 font-mono">v{s.version}</td>
                          <td className="px-4 py-3 text-xs text-dark-300 font-mono">{formatDate(s.updated_at)}</td>
                          <td className="px-4 py-3">
                            <div className="flex gap-1">
                              <button onClick={() => { setEditingSecret(s.id); setEditForm({ value: "", description: s.description || "" }); }}
                                className="px-2 py-1 bg-accent-500/10 text-accent-400 rounded text-xs font-mono hover:bg-accent-500/20">Edit</button>
                              <button onClick={() => viewVersionHistory(s.id)}
                                className="px-2 py-1 bg-dark-700 text-dark-200 rounded text-xs font-mono hover:bg-dark-600">History</button>
                              <button onClick={() => deleteSecret(s.id)}
                                className="px-2 py-1 bg-danger-500/10 text-danger-400 rounded text-xs font-mono hover:bg-danger-500/20">Del</button>
                            </div>
                          </td>
                        </tr>
                      ))}
                    </tbody>
                  </table>
                </div>
              )}
            </>
          )}

          {!selectedVault && (
            <div className="p-12 text-center">
              <p className="text-dark-200 text-sm font-mono">Select or create a vault to manage secrets</p>
            </div>
          )}
        </div>
      </div>
    </div>
  );
}
