import { useState, useEffect } from "react";
import { Navigate } from "react-router-dom";
import { useAuth } from "../context/AuthContext";
import { api } from "../api";
import WebhookGatewayContent from "./WebhookGateway";
import ExtensionsContent from "./Extensions";

interface WhmcsConfig {
  configured: boolean;
  api_url: string;
  api_identifier: string;
  webhook_secret?: string;
}

interface WhmcsService {
  whmcs_service_id: number;
  plan: string;
  status: string;
}

interface MigrationItem {
  id: string;
  container_name: string;
  status: string;
  progress_pct: number;
}

function WhmcsContent() {
  const [config, setConfig] = useState<WhmcsConfig | null>(null);
  const [services, setServices] = useState<WhmcsService[]>([]);
  const [loading, setLoading] = useState(true);
  const [msg, setMsg] = useState({ text: "", type: "" });
  const [apiUrl, setApiUrl] = useState("");
  const [apiIdent, setApiIdent] = useState("");
  const [apiSecret, setApiSecret] = useState("");
  const [saving, setSaving] = useState(false);

  useEffect(() => {
    (async () => {
      try {
        const [cfg, svc] = await Promise.all([
          api.get<WhmcsConfig>("/whmcs/config"),
          api.get<{ services: WhmcsService[] }>("/whmcs/services").catch(() => ({ services: [] as WhmcsService[] })),
        ]);
        setConfig(cfg);
        setServices(svc.services || []);
        if (cfg.configured) {
          setApiUrl(cfg.api_url);
          setApiIdent(cfg.api_identifier);
        }
      } catch { /* ignore */ }
      setLoading(false);
    })();
  }, []);

  const handleSave = async () => {
    setSaving(true);
    try {
      await api.put("/whmcs/config", { api_url: apiUrl, api_identifier: apiIdent, api_secret: apiSecret });
      setMsg({ text: "WHMCS configured", type: "success" });
      const cfg = await api.get<WhmcsConfig>("/whmcs/config");
      setConfig(cfg);
    } catch (e) { setMsg({ text: e instanceof Error ? e.message : "Failed", type: "error" }); }
    setSaving(false);
  };

  if (loading) return (
    <div className="space-y-4 animate-pulse">
      <div className="bg-dark-800 rounded-lg border border-dark-500 p-5">
        <div className="h-4 bg-dark-700 rounded w-40 mb-4" />
        <div className="space-y-3">
          <div className="h-9 bg-dark-700 rounded" />
          <div className="grid grid-cols-2 gap-3">
            <div className="h-9 bg-dark-700 rounded" />
            <div className="h-9 bg-dark-700 rounded" />
          </div>
        </div>
      </div>
    </div>
  );

  return (
    <div className="space-y-4">
      {msg.text && (
        <div className={`px-4 py-3 rounded-lg text-sm border ${msg.type === "success" ? "bg-rust-500/10 text-rust-400 border-rust-500/20" : "bg-danger-500/10 text-danger-400 border-danger-500/20"}`}>
          {msg.text}
        </div>
      )}

      <div className="bg-dark-800 rounded-lg border border-dark-500 p-5">
        <h3 className="text-xs font-medium text-dark-300 uppercase font-mono tracking-widest mb-4">WHMCS Configuration</h3>
        <div className="space-y-3">
          <div>
            <label className="block text-sm text-dark-200 mb-1">API URL</label>
            <input type="url" value={apiUrl} onChange={e => setApiUrl(e.target.value)} placeholder="https://billing.example.com/includes/api.php" className="w-full px-3 py-2 text-sm border border-dark-500 rounded-lg" />
          </div>
          <div className="grid grid-cols-2 gap-3">
            <div>
              <label className="block text-sm text-dark-200 mb-1">API Identifier</label>
              <input type="text" value={apiIdent} onChange={e => setApiIdent(e.target.value)} className="w-full px-3 py-2 text-sm border border-dark-500 rounded-lg" />
            </div>
            <div>
              <label className="block text-sm text-dark-200 mb-1">API Secret</label>
              <input type="password" value={apiSecret} onChange={e => setApiSecret(e.target.value)} placeholder="Enter to update" className="w-full px-3 py-2 text-sm border border-dark-500 rounded-lg" />
            </div>
          </div>
          <div className="flex items-center justify-between pt-2">
            <div>
              {config?.configured && config?.webhook_secret && (
                <p className="text-xs text-dark-400">Webhook URL: <code className="text-dark-200">/api/whmcs/webhook?secret={config.webhook_secret}</code></p>
              )}
            </div>
            <button onClick={handleSave} disabled={saving || !apiUrl || !apiIdent || !apiSecret} className="px-4 py-2 bg-rust-500 text-white rounded-lg text-sm hover:bg-rust-600 disabled:opacity-50">
              {saving ? "Saving..." : "Save"}
            </button>
          </div>
        </div>
      </div>

      {services.length > 0 && (
        <div className="bg-dark-800 rounded-lg border border-dark-500 overflow-hidden">
          <div className="px-4 py-3 border-b border-dark-600">
            <h3 className="text-xs font-medium text-dark-300 uppercase font-mono tracking-widest">Service Mappings ({services.length})</h3>
          </div>
          <table className="w-full text-sm">
            <thead><tr className="border-b border-dark-600 bg-dark-700/50">
              <th className="text-left px-4 py-2 text-xs text-dark-300 uppercase">WHMCS ID</th>
              <th className="text-left px-4 py-2 text-xs text-dark-300 uppercase">Plan</th>
              <th className="text-left px-4 py-2 text-xs text-dark-300 uppercase">Status</th>
            </tr></thead>
            <tbody>
              {services.map((s) => (
                <tr key={s.whmcs_service_id} className="border-b border-dark-700">
                  <td className="px-4 py-2 text-dark-200">#{s.whmcs_service_id}</td>
                  <td className="px-4 py-2 text-dark-200">{s.plan}</td>
                  <td className="px-4 py-2"><span className={`px-2 py-0.5 rounded text-xs ${s.status === "active" ? "bg-rust-500/20 text-rust-400" : "bg-warn-500/20 text-warn-400"}`}>{s.status}</span></td>
                </tr>
              ))}
            </tbody>
          </table>
        </div>
      )}
    </div>
  );
}

function MigrationsContent() {
  const [migrations, setMigrations] = useState<MigrationItem[]>([]);
  const [loading, setLoading] = useState(true);

  useEffect(() => {
    api.get<{ migrations: MigrationItem[] }>("/migrations/apps")
      .then(d => setMigrations(d.migrations || []))
      .catch(() => {})
      .finally(() => setLoading(false));
  }, []);

  return (
    <div className="space-y-4">
      <div className="bg-dark-800 rounded-lg border border-dark-500 p-5">
        <h3 className="text-xs font-medium text-dark-300 uppercase font-mono tracking-widest mb-3">App Migrations</h3>
        <p className="text-sm text-dark-200 mb-4">Migrate Docker containers between servers. Start migrations from the Apps page on a specific server.</p>
        {loading ? (
          <div className="animate-pulse space-y-2">
            <div className="h-8 bg-dark-700 rounded" />
            <div className="h-8 bg-dark-700 rounded" />
          </div>
        ) : migrations.length === 0 ? (
          <p className="text-dark-400 text-sm">No migrations yet.</p>
        ) : (
          <table className="w-full text-sm">
            <thead><tr className="border-b border-dark-600 bg-dark-700/50">
              <th className="text-left px-4 py-2 text-xs text-dark-300 uppercase">Container</th>
              <th className="text-left px-4 py-2 text-xs text-dark-300 uppercase">Status</th>
              <th className="text-left px-4 py-2 text-xs text-dark-300 uppercase">Progress</th>
            </tr></thead>
            <tbody>
              {migrations.map((m) => (
                <tr key={m.id} className="border-b border-dark-700">
                  <td className="px-4 py-2 text-dark-200">{m.container_name}</td>
                  <td className="px-4 py-2"><span className={`px-2 py-0.5 rounded text-xs ${m.status === "completed" ? "bg-rust-500/20 text-rust-400" : m.status === "failed" ? "bg-danger-500/20 text-danger-400" : "bg-accent-500/20 text-accent-400"}`}>{m.status}</span></td>
                  <td className="px-4 py-2 text-dark-300">{m.progress_pct}%</td>
                </tr>
              ))}
            </tbody>
          </table>
        )}
      </div>
    </div>
  );
}

export default function Integrations() {
  const { user } = useAuth();
  const [tab, setTab] = useState<"webhooks" | "extensions" | "whmcs" | "migrations">("webhooks");

  if (!user || user.role !== "admin") return <Navigate to="/" replace />;

  return (
    <div className="p-6 lg:p-8">
      <div className="flex flex-col sm:flex-row sm:items-center justify-between gap-3 mb-6 pb-4 border-b border-dark-600">
        <div>
          <h1 className="text-sm font-medium text-dark-300 uppercase font-mono tracking-widest">Integrations</h1>
          <p className="text-sm text-dark-200 mt-1">Webhooks, extensions, billing, and migrations</p>
        </div>
      </div>
      <div className="flex gap-6 mb-6 text-sm font-mono overflow-x-auto">
        <button onClick={() => setTab("webhooks")} className={`whitespace-nowrap ${tab === "webhooks" ? "border-b-2 border-rust-500 text-dark-50 pb-2" : "text-dark-300 hover:text-dark-100 pb-2"}`}>Webhooks</button>
        <button onClick={() => setTab("extensions")} className={`whitespace-nowrap ${tab === "extensions" ? "border-b-2 border-rust-500 text-dark-50 pb-2" : "text-dark-300 hover:text-dark-100 pb-2"}`}>Extensions</button>
        <button onClick={() => setTab("whmcs")} className={`whitespace-nowrap ${tab === "whmcs" ? "border-b-2 border-rust-500 text-dark-50 pb-2" : "text-dark-300 hover:text-dark-100 pb-2"}`}>WHMCS</button>
        <button onClick={() => setTab("migrations")} className={`whitespace-nowrap ${tab === "migrations" ? "border-b-2 border-rust-500 text-dark-50 pb-2" : "text-dark-300 hover:text-dark-100 pb-2"}`}>Migrations</button>
      </div>
      {tab === "webhooks" && <WebhookGatewayContent />}
      {tab === "extensions" && <ExtensionsContent />}
      {tab === "whmcs" && <WhmcsContent />}
      {tab === "migrations" && <MigrationsContent />}
    </div>
  );
}
