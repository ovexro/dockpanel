import { useAuth } from "../context/AuthContext";
import { Navigate } from "react-router-dom";
import { useState, useEffect } from "react";
import { api } from "../api";
import { formatSize, formatDate, timeAgo } from "../utils/format";

interface BackupHealth {
  total_site_backups: number;
  total_db_backups: number;
  total_volume_backups: number;
  total_storage_bytes: number;
  last_24h_success: number;
  last_24h_failed: number;
  policies_active: number;
  policies_total: number;
  verifications_passed: number;
  verifications_failed: number;
  stale_backups: { resource_type: string; resource_name: string; last_backup: string; days_since: number }[];
}

interface BackupPolicy {
  id: string;
  name: string;
  backup_sites: boolean;
  backup_databases: boolean;
  backup_volumes: boolean;
  schedule: string;
  destination_id: string | null;
  retention_count: number;
  encrypt: boolean;
  verify_after_backup: boolean;
  enabled: boolean;
  last_run: string | null;
  last_status: string | null;
  created_at: string;
}

interface DatabaseBackup {
  id: string;
  database_id: string;
  filename: string;
  size_bytes: number;
  db_type: string;
  db_name: string;
  encrypted: boolean;
  uploaded: boolean;
  created_at: string;
}

interface VolumeBackup {
  id: string;
  container_name: string;
  volume_name: string;
  filename: string;
  size_bytes: number;
  encrypted: boolean;
  created_at: string;
}

interface Verification {
  id: string;
  backup_type: string;
  backup_id: string;
  status: string;
  checks_run: number;
  checks_passed: number;
  error_message: string | null;
  duration_ms: number | null;
  created_at: string;
}

interface Destination {
  id: string;
  name: string;
  dtype: string;
}

interface Database {
  id: string;
  name: string;
  engine: string;
}

type Tab = "overview" | "policies" | "databases" | "volumes" | "verifications";

export default function BackupOrchestrator() {
  const { user } = useAuth();
  if (!user || user.role !== "admin") return <Navigate to="/" replace />;
  const [tab, setTab] = useState<Tab>("overview");
  const [health, setHealth] = useState<BackupHealth | null>(null);
  const [policies, setPolicies] = useState<BackupPolicy[]>([]);
  const [dbBackups, setDbBackups] = useState<DatabaseBackup[]>([]);
  const [volBackups, setVolBackups] = useState<VolumeBackup[]>([]);
  const [verifications, setVerifications] = useState<Verification[]>([]);
  const [destinations, setDestinations] = useState<Destination[]>([]);
  const [databases, setDatabases] = useState<Database[]>([]);
  const [loading, setLoading] = useState(true);
  const [message, setMessage] = useState({ text: "", type: "" });
  const [showPolicyForm, setShowPolicyForm] = useState(false);
  const [policyForm, setPolicyForm] = useState({
    name: "", schedule: "0 2 * * *", backup_sites: true, backup_databases: true,
    backup_volumes: false, destination_id: "", retention_count: 7,
    encrypt: false, verify_after_backup: false,
  });

  useEffect(() => { loadAll(); }, []);

  const loadAll = async () => {
    setLoading(true);
    try {
      const [h, p, db, vol, ver, dest, dbs] = await Promise.all([
        api.get<BackupHealth>("/backup-orchestrator/health").catch(() => null),
        api.get<BackupPolicy[]>("/backup-orchestrator/policies").catch(() => []),
        api.get<DatabaseBackup[]>("/backup-orchestrator/db-backups").catch(() => []),
        api.get<VolumeBackup[]>("/backup-orchestrator/volume-backups").catch(() => []),
        api.get<Verification[]>("/backup-orchestrator/verifications").catch(() => []),
        api.get<Destination[]>("/backup-destinations").catch(() => []),
        api.get<Database[]>("/databases").catch(() => []),
      ]);
      setHealth(h);
      setPolicies(p);
      setDbBackups(db);
      setVolBackups(vol);
      setVerifications(ver);
      setDestinations(dest);
      setDatabases(dbs);
    } catch (e) {
      setMessage({ text: e instanceof Error ? e.message : "Failed to load", type: "error" });
    } finally {
      setLoading(false);
    }
  };

  const createPolicy = async () => {
    try {
      await api.post("/backup-orchestrator/policies", {
        ...policyForm,
        destination_id: policyForm.destination_id || null,
      });
      setMessage({ text: "Policy created", type: "success" });
      setShowPolicyForm(false);
      loadAll();
    } catch (e) {
      setMessage({ text: e instanceof Error ? e.message : "Failed", type: "error" });
    }
  };

  const deletePolicy = async (id: string) => {
    try {
      await api.delete(`/backup-orchestrator/policies/${id}`);
      setPolicies(policies.filter(p => p.id !== id));
      setMessage({ text: "Policy deleted", type: "success" });
    } catch (e) {
      setMessage({ text: e instanceof Error ? e.message : "Failed", type: "error" });
    }
  };

  const createDbBackup = async (databaseId: string) => {
    try {
      setMessage({ text: "Creating database backup...", type: "success" });
      await api.post("/backup-orchestrator/db-backup", { database_id: databaseId });
      setMessage({ text: "Database backup created", type: "success" });
      loadAll();
    } catch (e) {
      setMessage({ text: e instanceof Error ? e.message : "Failed", type: "error" });
    }
  };

  const triggerVerify = async (backupType: string, backupId: string) => {
    try {
      await api.post("/backup-orchestrator/verify", { backup_type: backupType, backup_id: backupId });
      setMessage({ text: "Verification started", type: "success" });
      setTimeout(loadAll, 5000);
    } catch (e) {
      setMessage({ text: e instanceof Error ? e.message : "Failed", type: "error" });
    }
  };

  const tabs: { key: Tab; label: string }[] = [
    { key: "overview", label: "Overview" },
    { key: "policies", label: "Policies" },
    { key: "databases", label: "DB Backups" },
    { key: "volumes", label: "Volume Backups" },
    { key: "verifications", label: "Verifications" },
  ];

  if (loading) {
    return <div className="p-8 text-center text-dark-300 font-mono">Loading backup orchestrator...</div>;
  }

  return (
    <div className="p-6 lg:p-8">
      {/* Header */}
      <div className="flex flex-col sm:flex-row sm:items-center justify-between gap-3 mb-6 pb-4 border-b border-dark-600">
        <div>
          <h1 className="text-sm font-medium text-dark-300 uppercase font-mono tracking-widest">Backup Orchestrator</h1>
          <p className="text-sm text-dark-200 mt-1 font-mono">Database, volume & site backups with verification</p>
        </div>
      </div>

      {/* Message */}
      {message.text && (
        <div className={`mb-4 px-4 py-3 rounded-lg text-sm border font-mono ${
          message.type === "success" ? "bg-rust-500/10 text-rust-400 border-rust-500/20" : "bg-danger-500/10 text-danger-400 border-danger-500/20"
        }`} role="alert">{message.text}</div>
      )}

      {/* Tabs */}
      <div className="flex gap-1 mb-6 border-b border-dark-600">
        {tabs.map(t => (
          <button key={t.key} onClick={() => setTab(t.key)}
            className={`px-4 py-2 text-xs font-mono uppercase tracking-widest transition-colors ${
              tab === t.key ? "text-rust-400 border-b-2 border-rust-400" : "text-dark-300 hover:text-dark-100"
            }`}>{t.label}</button>
        ))}
      </div>

      {/* Tab Content */}
      {tab === "overview" && health && <OverviewTab health={health} />}
      {tab === "policies" && (
        <PoliciesTab
          policies={policies} destinations={destinations}
          showForm={showPolicyForm} setShowForm={setShowPolicyForm}
          form={policyForm} setForm={setPolicyForm}
          onCreate={createPolicy} onDelete={deletePolicy}
        />
      )}
      {tab === "databases" && (
        <DatabasesTab backups={dbBackups} databases={databases}
          onCreateBackup={createDbBackup} onVerify={triggerVerify} />
      )}
      {tab === "volumes" && <VolumesTab backups={volBackups} onVerify={triggerVerify} />}
      {tab === "verifications" && <VerificationsTab verifications={verifications} />}
    </div>
  );
}

// ── Overview Tab ──────────────────────────────────────────────────────────

function OverviewTab({ health }: { health: BackupHealth }) {
  const cards = [
    { label: "Site Backups", value: health.total_site_backups, color: "text-rust-400" },
    { label: "DB Backups", value: health.total_db_backups, color: "text-accent-400" },
    { label: "Volume Backups", value: health.total_volume_backups, color: "text-warn-400" },
    { label: "Total Storage", value: formatSize(health.total_storage_bytes), color: "text-dark-50" },
    { label: "24h Success", value: health.last_24h_success, color: "text-rust-400" },
    { label: "24h Failed", value: health.last_24h_failed, color: health.last_24h_failed > 0 ? "text-danger-400" : "text-dark-200" },
    { label: "Active Policies", value: `${health.policies_active}/${health.policies_total}`, color: "text-dark-50" },
    { label: "Verifications", value: `${health.verifications_passed} passed / ${health.verifications_failed} failed`, color: "text-dark-50" },
  ];

  return (
    <div className="space-y-6">
      {/* Stats Grid */}
      <div className="grid grid-cols-2 md:grid-cols-4 gap-3">
        {cards.map(c => (
          <div key={c.label} className="bg-dark-800 rounded-lg border border-dark-500 p-4">
            <p className="text-xs text-dark-300 uppercase font-mono tracking-widest mb-1">{c.label}</p>
            <p className={`text-lg font-mono font-medium ${c.color}`}>{c.value}</p>
          </div>
        ))}
      </div>

      {/* Stale Backups Warning */}
      {health.stale_backups.length > 0 && (
        <div className="bg-dark-800 rounded-lg border border-warn-500/30 overflow-hidden">
          <div className="px-5 py-3 border-b border-dark-600 bg-warn-500/5">
            <h3 className="text-xs font-medium text-warn-400 uppercase font-mono tracking-widest">Stale Backups Warning</h3>
          </div>
          <div className="divide-y divide-dark-600">
            {health.stale_backups.map((s, i) => (
              <div key={i} className="px-5 py-3 flex items-center justify-between">
                <div>
                  <span className="text-sm text-dark-50 font-mono">{s.resource_name}</span>
                  <span className="text-xs text-dark-300 ml-2">({s.resource_type})</span>
                </div>
                <span className="text-xs text-warn-400 font-mono">{s.days_since}d since last backup</span>
              </div>
            ))}
          </div>
        </div>
      )}
    </div>
  );
}

// ── Policies Tab ──────────────────────────────────────────────────────────

function PoliciesTab({
  policies, destinations, showForm, setShowForm, form, setForm, onCreate, onDelete
}: {
  policies: BackupPolicy[]; destinations: Destination[];
  showForm: boolean; setShowForm: (v: boolean) => void;
  form: typeof import("./BackupOrchestrator").default extends never ? never : any;
  setForm: (v: any) => void;
  onCreate: () => void; onDelete: (id: string) => void;
}) {
  return (
    <div className="space-y-4">
      <div className="flex justify-end">
        <button onClick={() => setShowForm(!showForm)}
          className="px-4 py-2 bg-rust-500 text-white rounded-lg text-sm font-medium font-mono hover:bg-rust-600 transition-colors">
          {showForm ? "Cancel" : "Create Policy"}
        </button>
      </div>

      {showForm && (
        <div className="bg-dark-800 rounded-lg border border-dark-500 p-5 space-y-3">
          <div className="grid grid-cols-2 gap-3">
            <div>
              <label className="block text-xs font-medium text-dark-100 mb-1 font-mono">Name</label>
              <input type="text" value={form.name} onChange={e => setForm({ ...form, name: e.target.value })}
                className="w-full px-3 py-2 bg-dark-900 border border-dark-500 rounded-lg text-sm font-mono text-dark-50 focus:ring-2 focus:ring-accent-500 outline-none" />
            </div>
            <div>
              <label className="block text-xs font-medium text-dark-100 mb-1 font-mono">Schedule (Cron)</label>
              <select value={form.schedule} onChange={e => setForm({ ...form, schedule: e.target.value })}
                className="w-full px-3 py-2 bg-dark-900 border border-dark-500 rounded-lg text-sm font-mono text-dark-50 focus:ring-2 focus:ring-accent-500 outline-none">
                <option value="0 2 * * *">Daily 2 AM</option>
                <option value="0 4 * * *">Daily 4 AM</option>
                <option value="0 */12 * * *">Every 12 hours</option>
                <option value="0 3 * * 0">Weekly (Sun 3 AM)</option>
                <option value="0 3 1 * *">Monthly (1st, 3 AM)</option>
              </select>
            </div>
          </div>
          <div className="grid grid-cols-2 gap-3">
            <div>
              <label className="block text-xs font-medium text-dark-100 mb-1 font-mono">Destination</label>
              <select value={form.destination_id} onChange={e => setForm({ ...form, destination_id: e.target.value })}
                className="w-full px-3 py-2 bg-dark-900 border border-dark-500 rounded-lg text-sm font-mono text-dark-50 focus:ring-2 focus:ring-accent-500 outline-none">
                <option value="">Local only</option>
                {destinations.map(d => <option key={d.id} value={d.id}>{d.name} ({d.dtype})</option>)}
              </select>
            </div>
            <div>
              <label className="block text-xs font-medium text-dark-100 mb-1 font-mono">Retention (backups)</label>
              <input type="number" value={form.retention_count} min={1} max={365}
                onChange={e => setForm({ ...form, retention_count: parseInt(e.target.value) || 7 })}
                className="w-full px-3 py-2 bg-dark-900 border border-dark-500 rounded-lg text-sm font-mono text-dark-50 focus:ring-2 focus:ring-accent-500 outline-none" />
            </div>
          </div>
          <div className="flex gap-4 text-sm font-mono">
            <label className="flex items-center gap-2 text-dark-100">
              <input type="checkbox" checked={form.backup_sites} onChange={e => setForm({ ...form, backup_sites: e.target.checked })} /> Sites
            </label>
            <label className="flex items-center gap-2 text-dark-100">
              <input type="checkbox" checked={form.backup_databases} onChange={e => setForm({ ...form, backup_databases: e.target.checked })} /> Databases
            </label>
            <label className="flex items-center gap-2 text-dark-100">
              <input type="checkbox" checked={form.backup_volumes} onChange={e => setForm({ ...form, backup_volumes: e.target.checked })} /> Volumes
            </label>
            <label className="flex items-center gap-2 text-dark-100">
              <input type="checkbox" checked={form.encrypt} onChange={e => setForm({ ...form, encrypt: e.target.checked })} /> Encrypt
            </label>
            <label className="flex items-center gap-2 text-dark-100">
              <input type="checkbox" checked={form.verify_after_backup} onChange={e => setForm({ ...form, verify_after_backup: e.target.checked })} /> Auto-verify
            </label>
          </div>
          <div className="flex justify-end">
            <button onClick={onCreate}
              className="px-4 py-2 bg-rust-500 text-white rounded-lg text-sm font-medium font-mono hover:bg-rust-600">Create</button>
          </div>
        </div>
      )}

      {policies.length === 0 ? (
        <div className="p-12 text-center">
          <p className="text-dark-200 text-sm font-mono">No backup policies yet</p>
          <p className="text-dark-300 text-xs mt-1 font-mono">Create a policy to automate backups across sites, databases, and volumes</p>
        </div>
      ) : (
        <div className="bg-dark-800 rounded-lg border border-dark-500 overflow-x-auto">
          <table className="w-full">
            <thead>
              <tr className="bg-dark-900 border-b border-dark-500">
                {["Name", "Schedule", "Scope", "Retention", "Status", "Last Run", ""].map(h => (
                  <th key={h} className="text-left text-xs font-medium text-dark-200 uppercase font-mono tracking-widest px-5 py-3">{h}</th>
                ))}
              </tr>
            </thead>
            <tbody className="divide-y divide-dark-600">
              {policies.map(p => (
                <tr key={p.id} className="hover:bg-dark-700/30 transition-colors">
                  <td className="px-5 py-4 text-sm text-dark-50 font-mono">{p.name}</td>
                  <td className="px-5 py-4 text-sm text-dark-200 font-mono">{p.schedule}</td>
                  <td className="px-5 py-4 text-xs font-mono">
                    {p.backup_sites && <span className="inline-flex px-2 py-0.5 rounded-full bg-rust-500/15 text-rust-400 mr-1">Sites</span>}
                    {p.backup_databases && <span className="inline-flex px-2 py-0.5 rounded-full bg-accent-500/15 text-accent-400 mr-1">DBs</span>}
                    {p.backup_volumes && <span className="inline-flex px-2 py-0.5 rounded-full bg-warn-500/15 text-warn-400">Vols</span>}
                  </td>
                  <td className="px-5 py-4 text-sm text-dark-200 font-mono">{p.retention_count}</td>
                  <td className="px-5 py-4">
                    <span className={`inline-flex px-2 py-0.5 rounded-full text-xs font-medium font-mono ${
                      p.enabled ? "bg-rust-500/15 text-rust-400" : "bg-dark-700 text-dark-200"
                    }`}>{p.enabled ? "Active" : "Paused"}</span>
                  </td>
                  <td className="px-5 py-4 text-xs text-dark-300 font-mono">
                    {p.last_run ? timeAgo(p.last_run) : "Never"}
                    {p.last_status && (
                      <span className={`ml-1 ${p.last_status === "success" ? "text-rust-400" : "text-danger-400"}`}>
                        ({p.last_status})
                      </span>
                    )}
                  </td>
                  <td className="px-5 py-4">
                    <button onClick={() => onDelete(p.id)}
                      className="px-3 py-1 bg-danger-500/10 text-danger-400 rounded-md text-xs font-medium font-mono hover:bg-danger-500/20 transition-colors">
                      Delete
                    </button>
                  </td>
                </tr>
              ))}
            </tbody>
          </table>
        </div>
      )}
    </div>
  );
}

// ── Database Backups Tab ──────────────────────────────────────────────────

function DatabasesTab({
  backups, databases, onCreateBackup, onVerify
}: {
  backups: DatabaseBackup[]; databases: Database[];
  onCreateBackup: (id: string) => void; onVerify: (type: string, id: string) => void;
}) {
  return (
    <div className="space-y-4">
      {/* Quick backup buttons */}
      {databases.length > 0 && (
        <div className="bg-dark-800 rounded-lg border border-dark-500 p-4">
          <p className="text-xs text-dark-300 uppercase font-mono tracking-widest mb-3">Quick Backup</p>
          <div className="flex flex-wrap gap-2">
            {databases.map(db => (
              <button key={db.id} onClick={() => onCreateBackup(db.id)}
                className="px-3 py-1.5 bg-dark-700 text-dark-100 rounded-lg text-xs font-mono hover:bg-dark-600 transition-colors border border-dark-500">
                {db.name} <span className="text-dark-300">({db.engine})</span>
              </button>
            ))}
          </div>
        </div>
      )}

      {backups.length === 0 ? (
        <div className="p-12 text-center">
          <p className="text-dark-200 text-sm font-mono">No database backups yet</p>
          <p className="text-dark-300 text-xs mt-1 font-mono">Click a database above to create its first backup</p>
        </div>
      ) : (
        <div className="bg-dark-800 rounded-lg border border-dark-500 overflow-x-auto">
          <table className="w-full">
            <thead>
              <tr className="bg-dark-900 border-b border-dark-500">
                {["Database", "Type", "Filename", "Size", "Encrypted", "Created", ""].map(h => (
                  <th key={h} className="text-left text-xs font-medium text-dark-200 uppercase font-mono tracking-widest px-5 py-3">{h}</th>
                ))}
              </tr>
            </thead>
            <tbody className="divide-y divide-dark-600">
              {backups.map(b => (
                <tr key={b.id} className="hover:bg-dark-700/30 transition-colors">
                  <td className="px-5 py-4 text-sm text-dark-50 font-mono">{b.db_name}</td>
                  <td className="px-5 py-4 text-xs text-dark-200 font-mono uppercase">{b.db_type}</td>
                  <td className="px-5 py-4 text-xs text-dark-200 font-mono truncate max-w-[200px]">{b.filename}</td>
                  <td className="px-5 py-4 text-sm text-dark-200 font-mono">{formatSize(b.size_bytes)}</td>
                  <td className="px-5 py-4">
                    {b.encrypted && <span className="inline-flex px-2 py-0.5 rounded-full text-xs font-medium bg-accent-500/15 text-accent-400 font-mono">Encrypted</span>}
                  </td>
                  <td className="px-5 py-4 text-xs text-dark-300 font-mono">{formatDate(b.created_at)}</td>
                  <td className="px-5 py-4">
                    <button onClick={() => onVerify("database", b.id)}
                      className="px-3 py-1 bg-accent-500/10 text-accent-400 rounded-md text-xs font-medium font-mono hover:bg-accent-500/20 transition-colors">
                      Verify
                    </button>
                  </td>
                </tr>
              ))}
            </tbody>
          </table>
        </div>
      )}
    </div>
  );
}

// ── Volume Backups Tab ────────────────────────────────────────────────────

function VolumesTab({ backups, onVerify }: { backups: VolumeBackup[]; onVerify: (type: string, id: string) => void }) {
  return backups.length === 0 ? (
    <div className="p-12 text-center">
      <p className="text-dark-200 text-sm font-mono">No volume backups yet</p>
      <p className="text-dark-300 text-xs mt-1 font-mono">Volume backups will appear here when created via policies or API</p>
    </div>
  ) : (
    <div className="bg-dark-800 rounded-lg border border-dark-500 overflow-x-auto">
      <table className="w-full">
        <thead>
          <tr className="bg-dark-900 border-b border-dark-500">
            {["Container", "Volume", "Filename", "Size", "Created", ""].map(h => (
              <th key={h} className="text-left text-xs font-medium text-dark-200 uppercase font-mono tracking-widest px-5 py-3">{h}</th>
            ))}
          </tr>
        </thead>
        <tbody className="divide-y divide-dark-600">
          {backups.map(b => (
            <tr key={b.id} className="hover:bg-dark-700/30 transition-colors">
              <td className="px-5 py-4 text-sm text-dark-50 font-mono">{b.container_name}</td>
              <td className="px-5 py-4 text-sm text-dark-200 font-mono">{b.volume_name}</td>
              <td className="px-5 py-4 text-xs text-dark-200 font-mono truncate max-w-[200px]">{b.filename}</td>
              <td className="px-5 py-4 text-sm text-dark-200 font-mono">{formatSize(b.size_bytes)}</td>
              <td className="px-5 py-4 text-xs text-dark-300 font-mono">{formatDate(b.created_at)}</td>
              <td className="px-5 py-4">
                <button onClick={() => onVerify("volume", b.id)}
                  className="px-3 py-1 bg-accent-500/10 text-accent-400 rounded-md text-xs font-medium font-mono hover:bg-accent-500/20 transition-colors">
                  Verify
                </button>
              </td>
            </tr>
          ))}
        </tbody>
      </table>
    </div>
  );
}

// ── Verifications Tab ─────────────────────────────────────────────────────

function VerificationsTab({ verifications }: { verifications: Verification[] }) {
  return verifications.length === 0 ? (
    <div className="p-12 text-center">
      <p className="text-dark-200 text-sm font-mono">No verifications yet</p>
      <p className="text-dark-300 text-xs mt-1 font-mono">Trigger a verification from any backup or enable auto-verify in policies</p>
    </div>
  ) : (
    <div className="bg-dark-800 rounded-lg border border-dark-500 overflow-x-auto">
      <table className="w-full">
        <thead>
          <tr className="bg-dark-900 border-b border-dark-500">
            {["Type", "Status", "Checks", "Duration", "Error", "Created"].map(h => (
              <th key={h} className="text-left text-xs font-medium text-dark-200 uppercase font-mono tracking-widest px-5 py-3">{h}</th>
            ))}
          </tr>
        </thead>
        <tbody className="divide-y divide-dark-600">
          {verifications.map(v => (
            <tr key={v.id} className="hover:bg-dark-700/30 transition-colors">
              <td className="px-5 py-4 text-sm text-dark-50 font-mono capitalize">{v.backup_type}</td>
              <td className="px-5 py-4">
                <span className={`inline-flex px-2 py-0.5 rounded-full text-xs font-medium font-mono ${
                  v.status === "passed" ? "bg-rust-500/15 text-rust-400"
                  : v.status === "failed" ? "bg-danger-500/15 text-danger-400"
                  : v.status === "running" ? "bg-warn-500/15 text-warn-400"
                  : "bg-dark-700 text-dark-200"
                }`}>{v.status}</span>
              </td>
              <td className="px-5 py-4 text-sm text-dark-200 font-mono">{v.checks_passed}/{v.checks_run}</td>
              <td className="px-5 py-4 text-sm text-dark-200 font-mono">{v.duration_ms ? `${v.duration_ms}ms` : "-"}</td>
              <td className="px-5 py-4 text-xs text-danger-400 font-mono truncate max-w-[200px]">{v.error_message || "-"}</td>
              <td className="px-5 py-4 text-xs text-dark-300 font-mono">{formatDate(v.created_at)}</td>
            </tr>
          ))}
        </tbody>
      </table>
    </div>
  );
}
