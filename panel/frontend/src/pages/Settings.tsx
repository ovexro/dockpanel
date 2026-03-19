import { useState, useEffect, useRef } from "react";
import { api } from "../api";
import ProvisionLog from "../components/ProvisionLog";
import UpdatesContent from "./Updates";
import UsersContent from "./Users";

interface HealthStatus {
  db: string;
  agent: string;
  uptime: string;
  database: boolean; // computed
  agentOk: boolean;  // computed
}

interface BackupDestination {
  id: string;
  name: string;
  dtype: string;
  config: Record<string, string>;
  created_at: string;
}

export default function Settings() {
  const [settings, setSettings] = useState<Record<string, string>>({});
  const [loading, setLoading] = useState(true);
  const [saving, setSaving] = useState<string | null>(null);
  const [health, setHealth] = useState<HealthStatus | null>(null);
  const [healthLoading, setHealthLoading] = useState(true);
  const [message, setMessage] = useState({ text: "", type: "" });
  const healthTimer = useRef<ReturnType<typeof setInterval>>(undefined);
  const qrRef = useRef<HTMLDivElement>(null);

  // Form state
  const [panelName, setPanelName] = useState("");
  const [smtpProvider, setSmtpProvider] = useState("custom");
  const [smtpHost, setSmtpHost] = useState("");
  const [smtpPort, setSmtpPort] = useState("");
  const [smtpUser, setSmtpUser] = useState("");
  const [smtpPass, setSmtpPass] = useState("");
  const [smtpFrom, setSmtpFrom] = useState("");
  const [smtpFromName, setSmtpFromName] = useState("");
  const [smtpEncryption, setSmtpEncryption] = useState("starttls");
  const [testingEmail, setTestingEmail] = useState(false);

  // Update count for tab badge
  const [updateCount, setUpdateCount] = useState(0);

  // 2FA state
  const [twoFaEnabled, setTwoFaEnabled] = useState(false);
  const [twoFaSetup, setTwoFaSetup] = useState<{ secret: string; qr_svg: string } | null>(null);
  const [twoFaCode, setTwoFaCode] = useState("");
  const [twoFaDisableCode, setTwoFaDisableCode] = useState("");
  const [recoveryCodes, setRecoveryCodes] = useState<string[]>([]);
  const [twoFaLoading, setTwoFaLoading] = useState(false);

  // Auto-healing
  const [autoHealEnabled, setAutoHealEnabled] = useState(false);

  // PowerDNS
  const [pdnsApiUrl, setPdnsApiUrl] = useState("");
  const [pdnsApiKey, setPdnsApiKey] = useState("");
  const [showPdnsGuide, setShowPdnsGuide] = useState(false);

  // Notification channels
  const [notifySlackUrl, setNotifySlackUrl] = useState("");
  const [notifyDiscordUrl, setNotifyDiscordUrl] = useState("");
  const [notifyPagerdutyKey, setNotifyPagerdutyKey] = useState("");
  const [notifyEmail, setNotifyEmail] = useState(true);
  const [testingWebhook, setTestingWebhook] = useState<string | null>(null);
  const [webhookResult, setWebhookResult] = useState<{ type: string; msg: string }>({ type: "", msg: "" });

  // Password change
  const [currentPass, setCurrentPass] = useState("");
  const [newPass, setNewPass] = useState("");
  const [confirmPass, setConfirmPass] = useState("");

  // API Keys
  const [apiKeys, setApiKeys] = useState<any[]>([]);
  const [showNewKey, setShowNewKey] = useState(false);
  const [newKeyName, setNewKeyName] = useState("");
  const [newKeyResult, setNewKeyResult] = useState<string | null>(null);

  // Hostname
  const [hostname, setHostname] = useState("");

  // Theme
  const [currentTheme, setCurrentTheme] = useState(() => localStorage.getItem("dp-theme") || "terminal");

  // Backup destinations
  const [destinations, setDestinations] = useState<BackupDestination[]>([]);
  const [showDestForm, setShowDestForm] = useState(false);
  const [destName, setDestName] = useState("");
  const [destType, setDestType] = useState("s3");
  const [destBucket, setDestBucket] = useState("");
  const [destRegion, setDestRegion] = useState("us-east-1");
  const [destEndpoint, setDestEndpoint] = useState("https://s3.amazonaws.com");
  const [destAccessKey, setDestAccessKey] = useState("");
  const [destSecretKey, setDestSecretKey] = useState("");
  const [destPathPrefix, setDestPathPrefix] = useState("backups");
  const [destSftpHost, setDestSftpHost] = useState("");
  const [destSftpPort, setDestSftpPort] = useState("22");
  const [destSftpUser, setDestSftpUser] = useState("");
  const [destSftpPass, setDestSftpPass] = useState("");
  const [destSftpPath, setDestSftpPath] = useState("/backups");
  const [savingDest, setSavingDest] = useState(false);
  const [testingDest, setTestingDest] = useState<string | null>(null);

  const loadSettings = async () => {
    try {
      const data = await api.get<Record<string, string>>("/settings");
      setSettings(data);
      setPanelName(data.panel_name || "");
      setSmtpHost(data.smtp_host || "");
      setSmtpPort(data.smtp_port || "");
      setSmtpUser(data.smtp_username || "");
      setSmtpPass(data.smtp_password || "");
      setSmtpFrom(data.smtp_from || "");
      setSmtpFromName(data.smtp_from_name || "");
      setSmtpEncryption(data.smtp_encryption || "starttls");
      setAutoHealEnabled(data.auto_heal_enabled === "true");
      setPdnsApiUrl(data.pdns_api_url || "");
      setPdnsApiKey(data.pdns_api_key || "");
    } catch (e) {
      setMessage({
        text: e instanceof Error ? e.message : "Failed to load settings",
        type: "error",
      });
    } finally {
      setLoading(false);
    }
  };

  const loadHealth = async () => {
    try {
      const raw = await api.get<{ db: string; agent: string; uptime: string }>("/settings/health");
      setHealth({ ...raw, database: raw.db === "ok", agentOk: raw.agent === "ok" });
    } catch {
      setHealth(null);
    } finally {
      setHealthLoading(false);
    }
  };

  const loadDestinations = async () => {
    try {
      const data = await api.get<BackupDestination[]>("/backup-destinations");
      setDestinations(data);
    } catch {
      setMessage({ text: "Failed to load backup destinations", type: "error" });
    }
  };

  const load2faStatus = async () => {
    try {
      const data = await api.get<{ enabled: boolean }>("/auth/2fa/status");
      setTwoFaEnabled(data.enabled);
    } catch { /* ignore */ }
  };

  const loadNotifyChannels = async () => {
    try {
      const rules = await api.get<{ notify_email?: boolean; notify_slack_url?: string; notify_discord_url?: string; notify_pagerduty_key?: string }[]>("/alert-rules");
      if (rules.length > 0) {
        const r = rules[0];
        setNotifyEmail(r.notify_email !== false);
        setNotifySlackUrl(r.notify_slack_url || "");
        setNotifyDiscordUrl(r.notify_discord_url || "");
        setNotifyPagerdutyKey(r.notify_pagerduty_key || "");
      }
    } catch { /* ignore */ }
  };

  useEffect(() => {
    loadSettings();
    loadHealth();
    loadDestinations();
    load2faStatus();
    loadNotifyChannels();
    api.get<{ count: number }>("/system/updates/count")
      .then((d) => setUpdateCount(d.count))
      .catch(() => {});
    api.get<any[]>("/api-keys").then(setApiKeys).catch(() => {});
    api.get<{ hostname?: string }>("/system/info")
      .then((d) => { if (d.hostname) setHostname(d.hostname); })
      .catch(() => {});
    healthTimer.current = setInterval(loadHealth, 30000);
    return () => clearInterval(healthTimer.current);
  }, []);

  // Safely render QR SVG without dangerouslySetInnerHTML
  useEffect(() => {
    if (qrRef.current && twoFaSetup?.qr_svg) {
      const parser = new DOMParser();
      const doc = parser.parseFromString(twoFaSetup.qr_svg, "image/svg+xml");
      const svg = doc.querySelector("svg");
      if (svg) {
        qrRef.current.innerHTML = "";
        qrRef.current.appendChild(svg);
      }
    }
  }, [twoFaSetup?.qr_svg]);

  const saveGeneral = async () => {
    setSaving("general");
    setMessage({ text: "", type: "" });
    try {
      await api.put("/settings", { panel_name: panelName });
      setMessage({ text: "General settings saved", type: "success" });
    } catch (e) {
      setMessage({
        text: e instanceof Error ? e.message : "Failed to save settings",
        type: "error",
      });
    } finally {
      setSaving(null);
    }
  };

  const saveSMTP = async () => {
    setSaving("smtp");
    setMessage({ text: "", type: "" });
    try {
      await api.put("/settings", {
        smtp_host: smtpHost,
        smtp_port: smtpPort,
        smtp_username: smtpUser,
        smtp_password: smtpPass,
        smtp_from: smtpFrom,
        smtp_from_name: smtpFromName,
        smtp_encryption: smtpEncryption,
      });
      setMessage({ text: "SMTP settings saved", type: "success" });
    } catch (e) {
      setMessage({
        text: e instanceof Error ? e.message : "Failed to save SMTP settings",
        type: "error",
      });
    } finally {
      setSaving(null);
    }
  };

  const handleTestEmail = async () => {
    setTestingEmail(true);
    setMessage({ text: "", type: "" });
    try {
      const result = await api.post<{ message: string }>("/settings/smtp/test", {});
      setMessage({ text: result.message || "Test email sent!", type: "success" });
    } catch (e) {
      setMessage({ text: e instanceof Error ? e.message : "Test email failed", type: "error" });
    } finally {
      setTestingEmail(false);
    }
  };

  const SMTP_PRESETS: Record<string, { host: string; port: string; encryption: string }> = {
    custom: { host: "", port: "", encryption: "starttls" },
    mailgun: { host: "smtp.mailgun.org", port: "587", encryption: "starttls" },
    ses: { host: "email-smtp.us-east-1.amazonaws.com", port: "587", encryption: "starttls" },
    sendgrid: { host: "smtp.sendgrid.net", port: "587", encryption: "starttls" },
    resend: { host: "smtp.resend.com", port: "465", encryption: "tls" },
    gmail: { host: "smtp.gmail.com", port: "587", encryption: "starttls" },
    outlook: { host: "smtp-mail.outlook.com", port: "587", encryption: "starttls" },
    zoho: { host: "smtp.zoho.com", port: "465", encryption: "tls" },
  };

  const applyPreset = (provider: string) => {
    setSmtpProvider(provider);
    const preset = SMTP_PRESETS[provider];
    if (preset && provider !== "custom") {
      setSmtpHost(preset.host);
      setSmtpPort(preset.port);
      setSmtpEncryption(preset.encryption);
    }
  };

  const [tab, setTab] = useState("general");

  if (loading) {
    return (
      <div className="p-6 lg:p-8">
        <h1 className="text-sm font-medium text-dark-300 uppercase font-mono tracking-widest mb-6">Settings</h1>
        <div className="space-y-4">
          {[...Array(3)].map((_, i) => (
            <div key={i} className="bg-dark-800 rounded-lg border border-dark-500 p-6 animate-pulse">
              <div className="h-5 bg-dark-700 rounded w-40 mb-4" />
              <div className="h-10 bg-dark-700 rounded w-full" />
            </div>
          ))}
        </div>
      </div>
    );
  }

  return (
    <div className="p-6 lg:p-8">
      <div className="mb-6 pb-4 border-b border-dark-600">
        <h1 className="text-sm font-medium text-dark-300 uppercase font-mono tracking-widest">Settings</h1>
        <p className="text-sm text-dark-200 font-mono mt-1">Manage panel configuration</p>
      </div>

      <div className="flex gap-1 mb-6 border-b border-dark-600 pb-1 pt-3 overflow-x-auto">
        {[
          { id: "general", label: "General" },
          { id: "email", label: "Email" },
          { id: "security", label: "Security" },
          { id: "notifications", label: "Notifications" },
          { id: "destinations", label: "Backup Destinations" },
          { id: "services", label: "Services" },
          { id: "updates", label: "Updates" },
          { id: "users", label: "Users" },
        ].map(t => (
          <button key={t.id} onClick={() => setTab(t.id)}
            className={`relative px-3 py-2 text-sm font-medium rounded-t-lg transition-colors whitespace-nowrap shrink-0 ${
              tab === t.id ? "text-rust-400 border-b-2 border-rust-400" : "text-dark-300 hover:text-dark-100"
            }`}>
            {t.label}
            {t.id === "updates" && updateCount > 0 && (
              <span className="absolute -top-1 -right-1 px-1.5 py-0.5 text-[10px] font-bold bg-red-500 text-white rounded-full min-w-[18px] text-center leading-none">
                {updateCount}
              </span>
            )}
          </button>
        ))}
      </div>

      {message.text && (
        <div
          className={`mb-4 px-4 py-3 rounded-lg text-sm border ${
            message.type === "success"
              ? "bg-rust-500/10 text-rust-400 border-rust-500/20"
              : "bg-red-500/10 text-danger-400 border-red-500/20"
          }`}
        >
          {message.text}
        </div>
      )}

      <div className="space-y-6">
        {/* General Settings */}
        {tab === "general" && (<>
        <div className="bg-dark-800 rounded-lg border border-dark-500 overflow-hidden">
          <div className="px-5 py-3 border-b border-dark-600">
            <h3 className="text-xs font-medium text-dark-300 uppercase font-mono tracking-widest">General Settings</h3>
          </div>
          <div className="p-5 space-y-4">
            <div>
              <label htmlFor="panel_name" className="block text-sm font-medium text-dark-100 mb-1">Panel Name</label>
              <input
                id="panel_name"
                type="text"
                value={panelName}
                onChange={(e) => setPanelName(e.target.value)}
                className="w-full px-3 py-2 border border-dark-500 rounded-lg text-sm focus:ring-2 focus:ring-accent-500 focus:border-accent-500"
                placeholder="DockPanel"
              />
            </div>
            <div className="flex justify-end">
              <button
                onClick={saveGeneral}
                disabled={saving === "general"}
                className="px-4 py-2 bg-rust-500 text-white rounded-lg text-sm font-medium hover:bg-rust-600 disabled:opacity-50"
              >
                {saving === "general" ? "Saving..." : "Save"}
              </button>
            </div>
          </div>
        </div>

        {/* Auto-Healing — part of General tab */}
        <div className="bg-dark-800 rounded-lg border border-dark-500 overflow-hidden">
          <div className="px-5 py-3 border-b border-dark-600">
            <h3 className="text-xs font-medium text-dark-300 uppercase font-mono tracking-widest">Auto-Healing</h3>
            <p className="text-xs text-dark-200 mt-0.5">Automatically fix common issues when detected</p>
          </div>
          <div className="p-5 space-y-4">
            <div className="flex items-center justify-between">
              <div>
                <p className="text-sm text-dark-100">Enable auto-healing</p>
                <p className="text-xs text-dark-300 mt-0.5">
                  Auto-restarts crashed services, cleans logs when disk is full, renews expiring SSL certs
                </p>
              </div>
              <button
                onClick={async () => {
                  const newVal = !autoHealEnabled;
                  try {
                    await api.put("/settings", { auto_heal_enabled: newVal ? "true" : "false" });
                    setAutoHealEnabled(newVal);
                    setMessage({ text: `Auto-healing ${newVal ? "enabled" : "disabled"}`, type: "success" });
                  } catch (e) {
                    setMessage({ text: e instanceof Error ? e.message : "Failed", type: "error" });
                  }
                }}
                role="switch"
                aria-checked={autoHealEnabled}
                aria-label="Enable auto-healing"
                className={`relative inline-flex h-6 w-11 items-center rounded-full transition-colors ${autoHealEnabled ? "bg-rust-500" : "bg-dark-600"}`}
              >
                <span className={`inline-block h-4 w-4 transform rounded-full bg-white transition-transform ${autoHealEnabled ? "translate-x-6" : "translate-x-1"}`} />
              </button>
            </div>
            {autoHealEnabled && (
              <div className="text-xs text-dark-300 space-y-1 pl-4 border-l-2 border-dark-600">
                <p>Crashed services are restarted (max once per 10 minutes)</p>
                <p>Logs are cleaned when disk exceeds 90% (max once per hour)</p>
                <p>SSL certs are renewed when expiring within 3 days (max once per 6 hours)</p>
                <p>All actions are logged in the Audit Log page</p>
              </div>
            )}
          </div>
        </div>

        {/* Public Status Page — part of General tab */}
        <div className="bg-dark-800 rounded-lg border border-dark-500 overflow-hidden">
          <div className="px-5 py-3 border-b border-dark-600">
            <h3 className="text-xs font-medium text-dark-300 uppercase font-mono tracking-widest">Public Status Page</h3>
            <p className="text-xs text-dark-200 mt-0.5">Share monitor status publicly at /api/status-page</p>
          </div>
          <div className="p-5">
            <div className="flex items-center justify-between">
              <div>
                <p className="text-sm text-dark-100">Enable public status page</p>
                <p className="text-xs text-dark-300 mt-0.5">
                  Exposes all enabled monitor statuses (name + status only, no URLs) at a public JSON endpoint
                </p>
              </div>
              <button
                onClick={async () => {
                  const newVal = settings.status_page_enabled !== "true";
                  try {
                    await api.put("/settings", { status_page_enabled: newVal ? "true" : "false" });
                    setSettings({ ...settings, status_page_enabled: newVal ? "true" : "false" });
                    setMessage({ text: `Status page ${newVal ? "enabled" : "disabled"}`, type: "success" });
                  } catch (e) {
                    setMessage({ text: e instanceof Error ? e.message : "Failed", type: "error" });
                  }
                }}
                role="switch"
                aria-checked={settings.status_page_enabled === "true"}
                aria-label="Enable public status page"
                className={`relative inline-flex h-6 w-11 items-center rounded-full transition-colors ${settings.status_page_enabled === "true" ? "bg-rust-500" : "bg-dark-600"}`}
              >
                <span className={`inline-block h-4 w-4 transform rounded-full bg-white transition-transform ${settings.status_page_enabled === "true" ? "translate-x-6" : "translate-x-1"}`} />
              </button>
            </div>
          </div>
        </div>

        {/* Feature #1: Timezone */}
        <div className="bg-dark-800 rounded-lg border border-dark-500 overflow-hidden">
          <div className="px-5 py-3 border-b border-dark-600">
            <h3 className="text-xs font-medium text-dark-300 uppercase font-mono tracking-widest">Timezone</h3>
          </div>
          <div className="p-5">
            <select value={settings.timezone || "UTC"} onChange={async (e) => {
              try {
                await api.put("/settings", { timezone: e.target.value });
                setSettings({ ...settings, timezone: e.target.value });
                setMessage({ text: "Timezone updated", type: "success" });
              } catch (err) { setMessage({ text: err instanceof Error ? err.message : "Failed", type: "error" }); }
            }} className="w-full px-3 py-2 border border-dark-500 rounded-lg text-sm focus:ring-2 focus:ring-accent-500 outline-none">
              {["UTC", "America/New_York", "America/Chicago", "America/Denver", "America/Los_Angeles",
                "Europe/London", "Europe/Paris", "Europe/Berlin", "Europe/Bucharest", "Europe/Moscow",
                "Asia/Tokyo", "Asia/Shanghai", "Asia/Kolkata", "Asia/Dubai",
                "Australia/Sydney", "Pacific/Auckland"].map(tz => (
                <option key={tz} value={tz}>{tz}</option>
              ))}
            </select>
            <p className="text-xs text-dark-300 mt-1">Affects displayed timestamps throughout the panel</p>
          </div>
        </div>

        {/* Feature #2: Branding */}
        <div className="bg-dark-800 rounded-lg border border-dark-500 overflow-hidden">
          <div className="px-5 py-3 border-b border-dark-600">
            <h3 className="text-xs font-medium text-dark-300 uppercase font-mono tracking-widest">Branding</h3>
          </div>
          <div className="p-5 space-y-3">
            <div>
              <label className="block text-sm font-medium text-dark-100 mb-1">Logo URL</label>
              <input type="url" value={settings.logo_url || ""} onChange={e => setSettings({ ...settings, logo_url: e.target.value })}
                placeholder="https://example.com/logo.png" className="w-full px-3 py-2 border border-dark-500 rounded-lg text-sm focus:ring-2 focus:ring-accent-500 outline-none" />
            </div>
            <div>
              <label className="block text-sm font-medium text-dark-100 mb-1">Accent Color</label>
              <div className="flex gap-2">
                {["#22c55e", "#3b82f6", "#8b5cf6", "#ec4899", "#f59e0b", "#ef4444"].map(color => (
                  <button key={color} onClick={() => setSettings({ ...settings, accent_color: color })}
                    className={`w-8 h-8 rounded-full border-2 ${settings.accent_color === color ? "border-white" : "border-transparent"}`}
                    style={{ backgroundColor: color }} />
                ))}
              </div>
            </div>
            <button onClick={async () => {
              try {
                await api.put("/settings", { logo_url: settings.logo_url || "", accent_color: settings.accent_color || "" });
                setMessage({ text: "Branding saved", type: "success" });
              } catch (err) { setMessage({ text: err instanceof Error ? err.message : "Failed", type: "error" }); }
            }} className="px-4 py-2 bg-rust-500 text-white rounded-lg text-sm font-medium hover:bg-rust-600 disabled:opacity-50">Save Branding</button>
          </div>
        </div>

        {/* Feature #5: Configuration Backup */}
        <div className="bg-dark-800 rounded-lg border border-dark-500 overflow-hidden">
          <div className="px-5 py-3 border-b border-dark-600">
            <h3 className="text-xs font-medium text-dark-300 uppercase font-mono tracking-widest">Configuration Backup</h3>
          </div>
          <div className="p-5 flex gap-3">
            <button onClick={async () => {
              try {
                const data = await api.get<any>("/settings/export");
                const blob = new Blob([JSON.stringify(data, null, 2)], { type: "application/json" });
                const url = URL.createObjectURL(blob);
                const a = document.createElement("a"); a.href = url; a.download = "dockpanel-config.json"; a.click();
                URL.revokeObjectURL(url);
              } catch (err) { setMessage({ text: err instanceof Error ? err.message : "Export failed", type: "error" }); }
            }} className="px-4 py-2 bg-dark-700 text-dark-100 rounded-lg text-sm font-medium hover:bg-dark-600">Export Config</button>
            <label className="px-4 py-2 bg-dark-700 text-dark-100 rounded-lg text-sm font-medium hover:bg-dark-600 cursor-pointer">
              Import Config
              <input type="file" accept=".json" className="hidden" onChange={async (e) => {
                const file = e.target.files?.[0];
                if (!file) return;
                const text = await file.text();
                try {
                  const data = JSON.parse(text);
                  if (!confirm(`Import ${Object.keys(data.settings || {}).length} settings? This will overwrite existing values.`)) return;
                  await api.post("/settings/import", data);
                  setMessage({ text: "Config imported", type: "success" });
                  window.location.reload();
                } catch { setMessage({ text: "Invalid config file", type: "error" }); }
                e.target.value = "";
              }} />
            </label>
          </div>
        </div>

        {/* Feature #9: Disk Cleanup */}
        <div className="bg-dark-800 rounded-lg border border-dark-500 overflow-hidden">
          <div className="px-5 py-3 border-b border-dark-600">
            <h3 className="text-xs font-medium text-dark-300 uppercase font-mono tracking-widest">Disk Cleanup</h3>
          </div>
          <div className="p-5 flex items-center justify-between">
            <div>
              <p className="text-sm text-dark-100">Free disk space</p>
              <p className="text-xs text-dark-300 mt-0.5">Clears apt cache, old logs, temp files, dangling Docker images</p>
            </div>
            <button onClick={async () => {
              try {
                const result = await api.post<any>("/system/cleanup");
                setMessage({ text: `Cleaned: ${result.cleaned?.join(", ") || "done"}`, type: "success" });
              } catch (e) { setMessage({ text: e instanceof Error ? e.message : "Failed", type: "error" }); }
            }} className="px-4 py-2 bg-rust-500 text-white rounded-lg text-sm font-medium hover:bg-rust-600">Clean Up</button>
          </div>
        </div>

        {/* Feature #10: Hostname */}
        <div className="bg-dark-800 rounded-lg border border-dark-500 overflow-hidden">
          <div className="px-5 py-3 border-b border-dark-600">
            <h3 className="text-xs font-medium text-dark-300 uppercase font-mono tracking-widest">Server Hostname</h3>
          </div>
          <div className="p-5">
            <div className="flex gap-2">
              <input type="text" value={hostname} onChange={e => setHostname(e.target.value)}
                placeholder="server.example.com" className="flex-1 px-3 py-2 border border-dark-500 rounded-lg text-sm font-mono focus:ring-2 focus:ring-accent-500 outline-none" />
              <button onClick={async () => {
                try {
                  await api.post("/system/hostname", { hostname });
                  setMessage({ text: "Hostname updated", type: "success" });
                } catch (e) { setMessage({ text: e instanceof Error ? e.message : "Failed", type: "error" }); }
              }} className="px-4 py-2 bg-rust-500 text-white rounded-lg text-sm font-medium hover:bg-rust-600 shrink-0">Save</button>
            </div>
            <p className="text-xs text-dark-300 mt-1">Only alphanumeric characters, hyphens, and dots allowed</p>
          </div>
        </div>

        {/* Feature #11: Theme Picker */}
        <div className="bg-dark-800 rounded-lg border border-dark-500 overflow-hidden">
          <div className="px-5 py-3 border-b border-dark-600">
            <h3 className="text-xs font-medium text-dark-300 uppercase font-mono tracking-widest">Appearance</h3>
          </div>
          <div className="p-5 space-y-6">
            {/* Layout selector */}
            <div>
              <p className="text-sm text-dark-100 mb-3">Layout</p>
              <div className="grid grid-cols-3 gap-3">
                {([
                  { id: "command", name: "Command", desc: "Full sidebar, terminal feel",
                    preview: (c: { bg: string; bar: string; accent: string; text: string }) => (
                      <div className="flex gap-1" style={{ height: "44px" }}>
                        <div style={{ background: c.bar, width: "22%", borderRadius: "2px" }} className="flex flex-col gap-0.5 p-1">
                          <div style={{ background: c.accent, height: "2px", width: "80%" }} />
                          <div style={{ background: c.text, height: "1.5px", opacity: 0.4 }} />
                          <div style={{ background: c.text, height: "1.5px", width: "70%", opacity: 0.4 }} />
                          <div style={{ background: c.text, height: "1.5px", width: "85%", opacity: 0.4 }} />
                        </div>
                        <div style={{ width: "78%" }} className="flex flex-col gap-0.5 p-0.5">
                          <div style={{ background: c.bar, height: "14px", borderRadius: "1px" }} />
                          <div style={{ background: c.bar, flex: 1, borderRadius: "1px" }} />
                        </div>
                      </div>
                    )},
                  { id: "glass", name: "Glass", desc: "Minimal icon sidebar, modern",
                    preview: (c: { bg: string; bar: string; accent: string; text: string }) => (
                      <div className="flex gap-1" style={{ height: "44px" }}>
                        <div style={{ background: c.bar, width: "10%", borderRadius: "2px", opacity: 0.6 }} className="flex flex-col items-center gap-1.5 pt-1.5">
                          <div style={{ background: c.accent, width: "5px", height: "5px", borderRadius: "1px" }} />
                          <div style={{ background: c.text, width: "4px", height: "4px", borderRadius: "1px", opacity: 0.4 }} />
                          <div style={{ background: c.text, width: "4px", height: "4px", borderRadius: "1px", opacity: 0.4 }} />
                        </div>
                        <div style={{ width: "90%" }} className="flex flex-col gap-0.5 p-0.5">
                          <div style={{ background: c.bar, height: "14px", borderRadius: "1px" }} />
                          <div style={{ background: c.bar, flex: 1, borderRadius: "1px" }} />
                        </div>
                      </div>
                    )},
                  { id: "atlas", name: "Atlas", desc: "Top navbar, enterprise",
                    preview: (c: { bg: string; bar: string; accent: string; text: string }) => (
                      <div className="flex flex-col gap-0.5" style={{ height: "44px" }}>
                        <div style={{ background: c.bar, height: "10px", borderRadius: "1px" }} className="flex items-center gap-1 px-1">
                          <div style={{ background: c.accent, width: "8px", height: "3px", borderRadius: "1px" }} />
                          <div style={{ background: c.text, width: "6px", height: "2px", opacity: 0.4 }} />
                          <div style={{ background: c.text, width: "6px", height: "2px", opacity: 0.4 }} />
                          <div style={{ background: c.text, width: "6px", height: "2px", opacity: 0.4 }} />
                        </div>
                        <div style={{ background: c.bar, height: "5px", borderRadius: "1px", opacity: 0.5 }} />
                        <div style={{ background: c.bar, flex: 1, borderRadius: "1px" }} />
                      </div>
                    )},
                ] as const).map(l => {
                  const currentLayout = localStorage.getItem("dp-layout") || "command";
                  const isActive = currentLayout === l.id;
                  const accent = currentTheme === "midnight" ? "#3b82f6" : currentTheme === "arctic" ? "#0d9488" : currentTheme === "ember" ? "#f97316" : "#22c55e";
                  const bg = currentTheme === "arctic" ? "#f7f9fc" : currentTheme === "midnight" ? "#0a1628" : currentTheme === "ember" ? "#1a1614" : "#111113";
                  const bar = currentTheme === "arctic" ? "#dce3ed" : currentTheme === "midnight" ? "#182d50" : currentTheme === "ember" ? "#332b26" : "#27272a";
                  const text = currentTheme === "arctic" ? "#8d9bb0" : currentTheme === "midnight" ? "#6280a8" : currentTheme === "ember" ? "#8a7968" : "#71717a";
                  return (
                    <button key={l.id} onClick={() => {
                      localStorage.setItem("dp-layout", l.id);
                      document.documentElement.setAttribute("data-layout", l.id);
                      window.dispatchEvent(new Event("dp-layout-change"));
                    }}
                      className="text-left transition-all duration-150"
                      style={{
                        borderRadius: "8px",
                        border: isActive ? `2px solid ${accent}` : "2px solid transparent",
                        boxShadow: isActive ? `0 0 12px ${accent}33` : "none",
                      }}
                    >
                      <div style={{ background: bg, borderRadius: "6px 6px 0 0", overflow: "hidden", padding: "6px" }}>
                        {l.preview({ bg, bar, accent, text })}
                      </div>
                      <div style={{ background: bar, borderRadius: "0 0 6px 6px", padding: "6px 10px" }}>
                        <div style={{ color: isActive ? accent : text, fontSize: "12px", fontWeight: 600, fontFamily: "'Inter', sans-serif" }}>{l.name}</div>
                        <div style={{ color: text, fontSize: "10px", fontFamily: "'Inter', sans-serif", opacity: 0.7 }}>{l.desc}</div>
                      </div>
                    </button>
                  );
                })}
              </div>
            </div>

            {/* Color flavor selector */}
            <div>
              <p className="text-sm text-dark-100 mb-3">Color</p>
              <div className="grid grid-cols-2 sm:grid-cols-4 gap-3">
                {([
                  { id: "terminal", name: "Terminal", desc: "Hacker aesthetic", bg: "#111113", sidebar: "#09090b", accent: "#22c55e", card: "#18181b", text: "#71717a", bar: "#27272a" },
                  { id: "midnight", name: "Midnight", desc: "Deep navy, modern", bg: "#0a1628", sidebar: "#050a18", accent: "#3b82f6", card: "#0f1f3a", text: "#6280a8", bar: "#182d50" },
                  { id: "arctic", name: "Arctic", desc: "Clean & light", bg: "#f7f9fc", sidebar: "#ffffff", accent: "#0d9488", card: "#edf1f7", text: "#8d9bb0", bar: "#dce3ed" },
                  { id: "ember", name: "Ember", desc: "Warm & premium", bg: "#1a1614", sidebar: "#0c0a09", accent: "#f97316", card: "#241f1c", text: "#8a7968", bar: "#332b26" },
                ] as const).map(t => {
                  const active = currentTheme === t.id;
                  return (
                    <button key={t.id} onClick={() => {
                      localStorage.setItem("dp-theme", t.id);
                      document.documentElement.setAttribute("data-theme", t.id);
                      setCurrentTheme(t.id);
                    }}
                      className="group text-left transition-all duration-150"
                      style={{
                        borderRadius: "8px",
                        border: active ? `2px solid ${t.accent}` : "2px solid transparent",
                        boxShadow: active ? `0 0 12px ${t.accent}33` : "none",
                      }}
                    >
                      {/* Mini preview */}
                      <div style={{ background: t.bg, borderRadius: "6px 6px 0 0", overflow: "hidden" }} className="p-1.5">
                        <div className="flex gap-1" style={{ height: "52px" }}>
                          {/* Mini sidebar */}
                          <div style={{ background: t.sidebar, width: "20%", borderRadius: "3px" }} className="flex flex-col gap-1 p-1">
                            <div style={{ background: t.accent, height: "3px", borderRadius: "1px", width: "80%" }} />
                            <div style={{ background: t.bar, height: "2px", borderRadius: "1px" }} />
                            <div style={{ background: t.bar, height: "2px", borderRadius: "1px", width: "70%" }} />
                            <div style={{ background: t.bar, height: "2px", borderRadius: "1px", width: "85%" }} />
                          </div>
                          {/* Mini content */}
                          <div style={{ width: "80%" }} className="flex flex-col gap-1 p-1">
                            <div className="flex gap-1">
                              <div style={{ background: t.card, height: "16px", borderRadius: "2px", flex: 1 }}>
                                <div style={{ background: t.accent, height: "2px", borderRadius: "1px", width: "40%", margin: "4px" }} />
                              </div>
                              <div style={{ background: t.card, height: "16px", borderRadius: "2px", flex: 1 }}>
                                <div style={{ background: t.text, height: "2px", borderRadius: "1px", width: "60%", margin: "4px" }} />
                              </div>
                            </div>
                            <div style={{ background: t.card, flex: 1, borderRadius: "2px" }}>
                              <div style={{ background: t.text, height: "2px", borderRadius: "1px", width: "70%", margin: "4px" }} />
                              <div style={{ background: t.text, height: "2px", borderRadius: "1px", width: "50%", margin: "2px 4px" }} />
                            </div>
                          </div>
                        </div>
                      </div>
                      {/* Label */}
                      <div style={{ background: t.sidebar, borderRadius: "0 0 6px 6px", padding: "6px 10px" }}>
                        <div style={{ color: active ? t.accent : t.text, fontSize: "12px", fontWeight: 600, fontFamily: "'Inter', sans-serif" }}>{t.name}</div>
                        <div style={{ color: t.text, fontSize: "10px", fontFamily: "'Inter', sans-serif", opacity: 0.7 }}>{t.desc}</div>
                      </div>
                    </button>
                  );
                })}
              </div>
            </div>
            {/* Feature #12: Locale Selector */}
            <div className="flex items-center justify-between">
              <div>
                <p className="text-sm text-dark-100">Language</p>
                <p className="text-xs text-dark-300">More languages coming soon</p>
              </div>
              <select disabled className="px-2 py-1.5 border border-dark-500 rounded text-sm opacity-50">
                <option>English</option>
              </select>
            </div>
          </div>
        </div>
        </>)}

        {/* SMTP Configuration */}
        {tab === "email" && (
        <div className="bg-dark-800 rounded-lg border border-dark-500 overflow-hidden">
          <div className="px-5 py-3 border-b border-dark-600">
            <h3 className="text-xs font-medium text-dark-300 uppercase font-mono tracking-widest">SMTP Configuration</h3>
            <p className="text-xs text-dark-200 mt-0.5">Configure outgoing email for all sites on this server</p>
          </div>
          <div className="p-5 space-y-4">
            {/* Provider Preset */}
            <div>
              <label htmlFor="smtp-provider" className="block text-sm font-medium text-dark-100 mb-1">Provider</label>
              <select
                id="smtp-provider"
                value={smtpProvider}
                onChange={(e) => applyPreset(e.target.value)}
                className="w-full px-3 py-2 border border-dark-500 rounded-lg text-sm bg-dark-800 focus:ring-2 focus:ring-accent-500 focus:border-accent-500 outline-none"
              >
                <option value="custom">Custom SMTP</option>
                <option value="mailgun">Mailgun</option>
                <option value="ses">Amazon SES</option>
                <option value="sendgrid">SendGrid</option>
                <option value="resend">Resend</option>
                <option value="gmail">Gmail</option>
                <option value="outlook">Outlook / Microsoft 365</option>
                <option value="zoho">Zoho Mail</option>
              </select>
            </div>
            <div className="grid grid-cols-1 md:grid-cols-3 gap-4">
              <div>
                <label htmlFor="smtp_host" className="block text-sm font-medium text-dark-100 mb-1">Host</label>
                <input
                  id="smtp_host"
                  type="text"
                  value={smtpHost}
                  onChange={(e) => setSmtpHost(e.target.value)}
                  className="w-full px-3 py-2 border border-dark-500 rounded-lg text-sm focus:ring-2 focus:ring-accent-500 focus:border-accent-500 outline-none font-mono"
                  placeholder="smtp.example.com"
                />
              </div>
              <div>
                <label htmlFor="smtp_port" className="block text-sm font-medium text-dark-100 mb-1">Port</label>
                <input
                  id="smtp_port"
                  type="text"
                  value={smtpPort}
                  onChange={(e) => setSmtpPort(e.target.value)}
                  className="w-full px-3 py-2 border border-dark-500 rounded-lg text-sm focus:ring-2 focus:ring-accent-500 focus:border-accent-500 outline-none font-mono"
                  placeholder="587"
                />
              </div>
              <div>
                <label htmlFor="smtp-encryption" className="block text-sm font-medium text-dark-100 mb-1">Encryption</label>
                <select
                  id="smtp-encryption"
                  value={smtpEncryption}
                  onChange={(e) => setSmtpEncryption(e.target.value)}
                  className="w-full px-3 py-2 border border-dark-500 rounded-lg text-sm bg-dark-800 focus:ring-2 focus:ring-accent-500 focus:border-accent-500 outline-none"
                >
                  <option value="starttls">STARTTLS (port 587)</option>
                  <option value="tls">TLS/SSL (port 465)</option>
                  <option value="none">None (port 25)</option>
                </select>
              </div>
            </div>
            <div className="grid grid-cols-1 md:grid-cols-2 gap-4">
              <div>
                <label htmlFor="smtp_user" className="block text-sm font-medium text-dark-100 mb-1">Username</label>
                <input
                  id="smtp_user"
                  type="text"
                  value={smtpUser}
                  onChange={(e) => setSmtpUser(e.target.value)}
                  className="w-full px-3 py-2 border border-dark-500 rounded-lg text-sm focus:ring-2 focus:ring-accent-500 focus:border-accent-500 outline-none"
                  placeholder="user@example.com"
                />
              </div>
              <div>
                <label htmlFor="smtp_pass" className="block text-sm font-medium text-dark-100 mb-1">Password</label>
                <input
                  id="smtp_pass"
                  type="password"
                  value={smtpPass}
                  onChange={(e) => setSmtpPass(e.target.value)}
                  className="w-full px-3 py-2 border border-dark-500 rounded-lg text-sm focus:ring-2 focus:ring-accent-500 focus:border-accent-500 outline-none"
                  placeholder="Enter password or API key"
                />
              </div>
            </div>
            <div className="grid grid-cols-1 md:grid-cols-2 gap-4">
              <div>
                <label htmlFor="smtp_from" className="block text-sm font-medium text-dark-100 mb-1">From Address</label>
                <input
                  id="smtp_from"
                  type="text"
                  value={smtpFrom}
                  onChange={(e) => setSmtpFrom(e.target.value)}
                  className="w-full px-3 py-2 border border-dark-500 rounded-lg text-sm focus:ring-2 focus:ring-accent-500 focus:border-accent-500 outline-none"
                  placeholder="noreply@example.com"
                />
              </div>
              <div>
                <label htmlFor="smtp_from_name" className="block text-sm font-medium text-dark-100 mb-1">From Name</label>
                <input
                  id="smtp_from_name"
                  type="text"
                  value={smtpFromName}
                  onChange={(e) => setSmtpFromName(e.target.value)}
                  className="w-full px-3 py-2 border border-dark-500 rounded-lg text-sm focus:ring-2 focus:ring-accent-500 focus:border-accent-500 outline-none"
                  placeholder="DockPanel"
                />
              </div>
            </div>
            <div className="flex justify-end gap-2">
              <button
                onClick={handleTestEmail}
                disabled={testingEmail || !smtpHost}
                className="px-4 py-2 text-sm font-medium text-dark-100 bg-dark-700 rounded-lg hover:bg-dark-600 disabled:opacity-50"
              >
                {testingEmail ? "Sending..." : "Send Test Email"}
              </button>
              <button
                onClick={saveSMTP}
                disabled={saving === "smtp"}
                className="px-4 py-2 bg-rust-500 text-white rounded-lg text-sm font-medium hover:bg-rust-600 disabled:opacity-50"
              >
                {saving === "smtp" ? "Saving..." : "Save"}
              </button>
            </div>
          </div>
        </div>

        )}

        {/* Backup Destinations */}
        {tab === "destinations" && (
        <div className="bg-dark-800 rounded-lg border border-dark-500 overflow-hidden">
          <div className="px-5 py-3 border-b border-dark-600 flex items-center justify-between">
            <h3 className="text-xs font-medium text-dark-300 uppercase font-mono tracking-widest">Backup Destinations</h3>
            {showDestForm ? (
              <button
                onClick={() => setShowDestForm(false)}
                className="px-3 py-1 text-dark-300 border border-dark-600 rounded-md text-xs font-medium hover:text-dark-100 hover:border-dark-400"
              >
                Cancel
              </button>
            ) : (
              <button
                onClick={() => setShowDestForm(true)}
                className="px-3 py-1 bg-rust-500 text-white rounded-md text-xs font-medium hover:bg-rust-600"
              >
                Add Destination
              </button>
            )}
          </div>
          <div className="p-5">
            {showDestForm && (
              <div className="mb-4 p-4 bg-dark-900 rounded-lg space-y-3">
                <div className="grid grid-cols-2 gap-3">
                  <div>
                    <label className="block text-xs font-medium text-dark-100 mb-1">Name</label>
                    <input type="text" value={destName} onChange={(e) => setDestName(e.target.value)} placeholder="My S3 Backup" className="w-full px-3 py-2 border border-dark-500 rounded-lg text-sm focus:ring-2 focus:ring-accent-500 focus:border-accent-500 outline-none" />
                  </div>
                  <div>
                    <label className="block text-xs font-medium text-dark-100 mb-1">Type</label>
                    <select value={destType} onChange={(e) => setDestType(e.target.value)} className="w-full px-3 py-2 border border-dark-500 rounded-lg text-sm bg-dark-800 focus:ring-2 focus:ring-accent-500 outline-none">
                      <option value="s3">S3 / R2 / MinIO</option>
                      <option value="sftp">SFTP</option>
                    </select>
                  </div>
                </div>
                {destType === "s3" ? (
                  <>
                    <div className="grid grid-cols-2 gap-3">
                      <div>
                        <label className="block text-xs font-medium text-dark-100 mb-1">Bucket</label>
                        <input type="text" value={destBucket} onChange={(e) => setDestBucket(e.target.value)} placeholder="my-backups" className="w-full px-3 py-2 border border-dark-500 rounded-lg text-sm focus:ring-2 focus:ring-accent-500 focus:border-accent-500 outline-none" />
                      </div>
                      <div>
                        <label className="block text-xs font-medium text-dark-100 mb-1">Region</label>
                        <input type="text" value={destRegion} onChange={(e) => setDestRegion(e.target.value)} className="w-full px-3 py-2 border border-dark-500 rounded-lg text-sm focus:ring-2 focus:ring-accent-500 focus:border-accent-500 outline-none" />
                      </div>
                    </div>
                    <div>
                      <label className="block text-xs font-medium text-dark-100 mb-1">Endpoint URL</label>
                      <input type="text" value={destEndpoint} onChange={(e) => setDestEndpoint(e.target.value)} placeholder="https://s3.amazonaws.com" className="w-full px-3 py-2 border border-dark-500 rounded-lg text-sm focus:ring-2 focus:ring-accent-500 focus:border-accent-500 outline-none font-mono" />
                      <p className="text-xs text-dark-300 mt-1">For R2: https://ACCOUNT_ID.r2.cloudflarestorage.com</p>
                    </div>
                    <div className="grid grid-cols-2 gap-3">
                      <div>
                        <label className="block text-xs font-medium text-dark-100 mb-1">Access Key</label>
                        <input type="text" value={destAccessKey} onChange={(e) => setDestAccessKey(e.target.value)} className="w-full px-3 py-2 border border-dark-500 rounded-lg text-sm focus:ring-2 focus:ring-accent-500 focus:border-accent-500 outline-none" />
                      </div>
                      <div>
                        <label className="block text-xs font-medium text-dark-100 mb-1">Secret Key</label>
                        <input type="password" value={destSecretKey} onChange={(e) => setDestSecretKey(e.target.value)} className="w-full px-3 py-2 border border-dark-500 rounded-lg text-sm focus:ring-2 focus:ring-accent-500 focus:border-accent-500 outline-none" />
                      </div>
                    </div>
                    <div>
                      <label className="block text-xs font-medium text-dark-100 mb-1">Path Prefix</label>
                      <input type="text" value={destPathPrefix} onChange={(e) => setDestPathPrefix(e.target.value)} placeholder="backups" className="w-full px-3 py-2 border border-dark-500 rounded-lg text-sm focus:ring-2 focus:ring-accent-500 focus:border-accent-500 outline-none" />
                    </div>
                  </>
                ) : (
                  <>
                    <div className="grid grid-cols-2 gap-3">
                      <div>
                        <label className="block text-xs font-medium text-dark-100 mb-1">Host</label>
                        <input type="text" value={destSftpHost} onChange={(e) => setDestSftpHost(e.target.value)} placeholder="backup.example.com" className="w-full px-3 py-2 border border-dark-500 rounded-lg text-sm focus:ring-2 focus:ring-accent-500 focus:border-accent-500 outline-none font-mono" />
                      </div>
                      <div>
                        <label className="block text-xs font-medium text-dark-100 mb-1">Port</label>
                        <input type="text" value={destSftpPort} onChange={(e) => setDestSftpPort(e.target.value)} className="w-full px-3 py-2 border border-dark-500 rounded-lg text-sm focus:ring-2 focus:ring-accent-500 focus:border-accent-500 outline-none font-mono" />
                      </div>
                    </div>
                    <div className="grid grid-cols-2 gap-3">
                      <div>
                        <label className="block text-xs font-medium text-dark-100 mb-1">Username</label>
                        <input type="text" value={destSftpUser} onChange={(e) => setDestSftpUser(e.target.value)} className="w-full px-3 py-2 border border-dark-500 rounded-lg text-sm focus:ring-2 focus:ring-accent-500 focus:border-accent-500 outline-none" />
                      </div>
                      <div>
                        <label className="block text-xs font-medium text-dark-100 mb-1">Password</label>
                        <input type="password" value={destSftpPass} onChange={(e) => setDestSftpPass(e.target.value)} className="w-full px-3 py-2 border border-dark-500 rounded-lg text-sm focus:ring-2 focus:ring-accent-500 focus:border-accent-500 outline-none" />
                      </div>
                    </div>
                    <div>
                      <label className="block text-xs font-medium text-dark-100 mb-1">Remote Path</label>
                      <input type="text" value={destSftpPath} onChange={(e) => setDestSftpPath(e.target.value)} placeholder="/backups" className="w-full px-3 py-2 border border-dark-500 rounded-lg text-sm focus:ring-2 focus:ring-accent-500 focus:border-accent-500 outline-none font-mono" />
                    </div>
                  </>
                )}
                <div className="flex justify-end">
                  <button
                    disabled={savingDest}
                    onClick={async () => {
                      setSavingDest(true);
                      setMessage({ text: "", type: "" });
                      try {
                        const config = destType === "s3"
                          ? { bucket: destBucket, region: destRegion, endpoint: destEndpoint, access_key: destAccessKey, secret_key: destSecretKey, path_prefix: destPathPrefix }
                          : { host: destSftpHost, port: parseInt(destSftpPort), username: destSftpUser, password: destSftpPass, remote_path: destSftpPath };
                        await api.post("/backup-destinations", { name: destName, dtype: destType, config });
                        setShowDestForm(false);
                        setDestName(""); setDestBucket(""); setDestAccessKey(""); setDestSecretKey("");
                        setDestSftpHost(""); setDestSftpUser(""); setDestSftpPass("");
                        loadDestinations();
                        setMessage({ text: "Destination created", type: "success" });
                      } catch (e) {
                        setMessage({ text: e instanceof Error ? e.message : "Failed", type: "error" });
                      } finally {
                        setSavingDest(false);
                      }
                    }}
                    className="px-4 py-2 bg-rust-500 text-white rounded-lg text-sm font-medium hover:bg-rust-600 disabled:opacity-50"
                  >
                    {savingDest ? "Saving..." : "Save Destination"}
                  </button>
                </div>
              </div>
            )}
            {destinations.length === 0 && !showDestForm ? (
              <p className="text-sm text-dark-300 text-center py-4">No backup destinations configured</p>
            ) : (
              <div className="space-y-2">
                {destinations.map((d) => (
                  <div key={d.id} className="flex items-center justify-between p-3 bg-dark-900 rounded-lg">
                    <div>
                      <p className="text-sm font-medium text-dark-50">{d.name}</p>
                      <p className="text-xs text-dark-200 font-mono">
                        {d.dtype === "s3" ? `S3: ${d.config.bucket || ""}` : `SFTP: ${d.config.host || ""}`}
                      </p>
                    </div>
                    <div className="flex items-center gap-2">
                      <button
                        onClick={async () => {
                          setTestingDest(d.id);
                          try {
                            await api.post(`/backup-destinations/${d.id}/test`);
                            setMessage({ text: `${d.name}: Connection successful`, type: "success" });
                          } catch (e) {
                            setMessage({ text: e instanceof Error ? e.message : "Test failed", type: "error" });
                          } finally {
                            setTestingDest(null);
                          }
                        }}
                        disabled={testingDest === d.id}
                        className="px-2 py-1 bg-blue-500/10 text-blue-400 rounded text-xs font-medium hover:bg-blue-500/20 disabled:opacity-50"
                      >
                        {testingDest === d.id ? "Testing..." : "Test"}
                      </button>
                      <button
                        onClick={async () => {
                          if (!confirm(`Delete "${d.name}"?`)) return;
                          try {
                            await api.delete(`/backup-destinations/${d.id}`);
                            loadDestinations();
                          } catch (e) {
                            setMessage({ text: e instanceof Error ? e.message : "Delete failed", type: "error" });
                          }
                        }}
                        className="px-2 py-1 bg-red-500/10 text-danger-400 rounded text-xs font-medium hover:bg-red-500/20"
                      >
                        Delete
                      </button>
                    </div>
                  </div>
                ))}
              </div>
            )}
          </div>
        </div>

        )}

        {/* Two-Factor Authentication */}
        {tab === "security" && (<>
        <div className="bg-dark-800 rounded-lg border border-dark-500 overflow-hidden">
          <div className="px-5 py-3 border-b border-dark-600">
            <h3 className="text-xs font-medium text-dark-300 uppercase font-mono tracking-widest">Two-Factor Authentication</h3>
            <p className="text-xs text-dark-200 mt-0.5">Add an extra layer of security to your account</p>
          </div>
          <div className="p-5">
            {twoFaEnabled ? (
              <div className="space-y-4">
                <div className="flex items-center gap-2">
                  <div className="w-3 h-3 rounded-full bg-rust-500" />
                  <span className="text-sm text-rust-400 font-medium">2FA is enabled</span>
                </div>
                <div className="flex items-center gap-2">
                  <input
                    type="text"
                    inputMode="numeric"
                    value={twoFaDisableCode}
                    onChange={(e) => setTwoFaDisableCode(e.target.value.replace(/\D/g, "").slice(0, 6))}
                    placeholder="Enter TOTP code to disable"
                    aria-label="TOTP code to disable 2FA"
                    className="px-3 py-2 border border-dark-500 rounded-lg text-sm w-48 focus:ring-2 focus:ring-accent-500 outline-none font-mono"
                  />
                  <button
                    disabled={twoFaLoading || twoFaDisableCode.length < 6}
                    onClick={async () => {
                      setTwoFaLoading(true);
                      setMessage({ text: "", type: "" });
                      try {
                        await api.post("/auth/2fa/disable", { code: twoFaDisableCode });
                        setTwoFaEnabled(false);
                        setTwoFaDisableCode("");
                        setMessage({ text: "2FA disabled", type: "success" });
                      } catch (e) {
                        setMessage({ text: e instanceof Error ? e.message : "Failed", type: "error" });
                      } finally {
                        setTwoFaLoading(false);
                      }
                    }}
                    className="px-4 py-2 bg-red-500/20 text-danger-400 rounded-lg text-sm font-medium hover:bg-red-500/30 disabled:opacity-50"
                  >
                    Disable 2FA
                  </button>
                </div>
              </div>
            ) : twoFaSetup ? (
              <div className="space-y-4">
                <p className="text-sm text-dark-100">Scan this QR code with your authenticator app:</p>
                <div ref={qrRef} className="flex justify-center bg-white rounded-lg p-4 w-fit mx-auto" />
                <p className="text-xs text-dark-300 text-center font-mono break-all">
                  Manual entry: {twoFaSetup.secret}
                </p>
                <div className="flex items-center gap-2">
                  <input
                    type="text"
                    inputMode="numeric"
                    value={twoFaCode}
                    onChange={(e) => setTwoFaCode(e.target.value.replace(/\D/g, "").slice(0, 6))}
                    placeholder="Enter 6-digit code"
                    aria-label="TOTP verification code"
                    className="px-3 py-2 border border-dark-500 rounded-lg text-sm w-48 focus:ring-2 focus:ring-accent-500 outline-none font-mono"
                    autoFocus
                  />
                  <button
                    disabled={twoFaLoading || twoFaCode.length < 6}
                    onClick={async () => {
                      setTwoFaLoading(true);
                      setMessage({ text: "", type: "" });
                      try {
                        const res = await api.post<{ recovery_codes: string[] }>("/auth/2fa/enable", { code: twoFaCode });
                        setTwoFaEnabled(true);
                        setTwoFaSetup(null);
                        setTwoFaCode("");
                        setRecoveryCodes(res.recovery_codes);
                        setMessage({ text: "2FA enabled! Save your recovery codes.", type: "success" });
                      } catch (e) {
                        setMessage({ text: e instanceof Error ? e.message : "Invalid code", type: "error" });
                      } finally {
                        setTwoFaLoading(false);
                      }
                    }}
                    className="px-4 py-2 bg-rust-500 text-white rounded-lg text-sm font-medium hover:bg-rust-600 disabled:opacity-50"
                  >
                    Verify & Enable
                  </button>
                  <button
                    onClick={() => { setTwoFaSetup(null); setTwoFaCode(""); }}
                    className="px-4 py-2 text-dark-300 border border-dark-600 rounded-lg text-sm font-medium hover:text-dark-100 hover:border-dark-400"
                  >
                    Cancel
                  </button>
                </div>
                {recoveryCodes.length > 0 && (
                  <div className="mt-4 p-4 bg-warn-500/10 rounded-lg border border-warn-500/20">
                    <p className="text-sm font-medium text-warn-400 mb-2">Recovery Codes (save these!)</p>
                    <div className="grid grid-cols-2 gap-1">
                      {recoveryCodes.map((code, i) => (
                        <code key={i} className="text-xs text-dark-100 font-mono bg-dark-900 px-2 py-1 rounded">{code}</code>
                      ))}
                    </div>
                    <p className="text-xs text-dark-300 mt-2">Each code can only be used once. Store them securely.</p>
                  </div>
                )}
              </div>
            ) : (
              <div className="space-y-3">
                <p className="text-sm text-dark-200">
                  Protect your account with time-based one-time passwords (TOTP).
                  Works with Google Authenticator, Authy, 1Password, etc.
                </p>
                <button
                  disabled={twoFaLoading}
                  onClick={async () => {
                    setTwoFaLoading(true);
                    setMessage({ text: "", type: "" });
                    try {
                      const res = await api.post<{ secret: string; qr_svg: string }>("/auth/2fa/setup");
                      setTwoFaSetup(res);
                    } catch (e) {
                      setMessage({ text: e instanceof Error ? e.message : "Failed", type: "error" });
                    } finally {
                      setTwoFaLoading(false);
                    }
                  }}
                  className="px-4 py-2 bg-rust-500 text-white rounded-lg text-sm font-medium hover:bg-rust-600 disabled:opacity-50"
                >
                  {twoFaLoading ? "Setting up..." : "Enable 2FA"}
                </button>
              </div>
            )}
          </div>
        </div>

        {/* 2FA Enforcement (admin only) */}
        <div className="bg-dark-800 rounded-lg border border-dark-500 overflow-hidden mt-4">
          <div className="px-5 py-3 border-b border-dark-600">
            <h3 className="text-xs font-medium text-dark-300 uppercase font-mono tracking-widest">2FA Enforcement</h3>
          </div>
          <div className="p-5 flex items-center justify-between">
            <div>
              <p className="text-sm text-dark-100">Require 2FA for all users</p>
              <p className="text-xs text-dark-300 mt-0.5">Users without 2FA will see a warning banner on every page</p>
            </div>
            <button
              onClick={async () => {
                const newVal = settings.enforce_2fa !== "true";
                try {
                  await api.put("/settings", { enforce_2fa: newVal ? "true" : "false" });
                  setSettings({ ...settings, enforce_2fa: newVal ? "true" : "false" });
                  setMessage({ text: `2FA enforcement ${newVal ? "enabled" : "disabled"}`, type: "success" });
                } catch (e) { setMessage({ text: e instanceof Error ? e.message : "Failed", type: "error" }); }
              }}
              className={`relative w-11 h-6 rounded-full transition-colors ${settings.enforce_2fa === "true" ? "bg-rust-500" : "bg-dark-600"}`}
            >
              <span className={`absolute top-0.5 left-0.5 w-5 h-5 bg-white rounded-full transition-transform ${settings.enforce_2fa === "true" ? "translate-x-5" : ""}`} />
            </button>
          </div>
        </div>

        {/* SSH Keys — Security tab */}
        <SSHKeys />

        {/* Auto-Updates — Security tab */}
        <AutoUpdates />

        {/* IP Whitelist — Security tab */}
        <IPWhitelist />

        {/* Feature #6: Password Change */}
        <div className="bg-dark-800 rounded-lg border border-dark-500 overflow-hidden mt-4">
          <div className="px-5 py-3 border-b border-dark-600">
            <h3 className="text-xs font-medium text-dark-300 uppercase font-mono tracking-widest">Change Password</h3>
          </div>
          <div className="p-5 space-y-3">
            <input type="password" value={currentPass} onChange={e => setCurrentPass(e.target.value)} placeholder="Current password"
              className="w-full px-3 py-2 border border-dark-500 rounded-lg text-sm focus:ring-2 focus:ring-accent-500 outline-none" />
            <input type="password" value={newPass} onChange={e => setNewPass(e.target.value)} placeholder="New password (min 8 chars)"
              className="w-full px-3 py-2 border border-dark-500 rounded-lg text-sm focus:ring-2 focus:ring-accent-500 outline-none" />
            <input type="password" value={confirmPass} onChange={e => setConfirmPass(e.target.value)} placeholder="Confirm new password"
              className="w-full px-3 py-2 border border-dark-500 rounded-lg text-sm focus:ring-2 focus:ring-accent-500 outline-none" />
            <button disabled={!currentPass || !newPass || newPass !== confirmPass || newPass.length < 8} onClick={async () => {
              try {
                await api.post("/auth/change-password", { current_password: currentPass, new_password: newPass });
                setMessage({ text: "Password changed successfully", type: "success" });
                setCurrentPass(""); setNewPass(""); setConfirmPass("");
              } catch (e) { setMessage({ text: e instanceof Error ? e.message : "Failed", type: "error" }); }
            }} className="px-4 py-2 bg-rust-500 text-white rounded-lg text-sm font-medium hover:bg-rust-600 disabled:opacity-50">
              Change Password
            </button>
          </div>
        </div>

        {/* Feature #3: Session Management */}
        <div className="bg-dark-800 rounded-lg border border-dark-500 overflow-hidden mt-4">
          <div className="px-5 py-3 border-b border-dark-600">
            <h3 className="text-xs font-medium text-dark-300 uppercase font-mono tracking-widest">Sessions</h3>
          </div>
          <div className="p-5 flex items-center justify-between">
            <div>
              <p className="text-sm text-dark-100">Active Sessions</p>
              <p className="text-xs text-dark-300 mt-0.5">Force all users to re-login</p>
            </div>
            <button onClick={async () => {
              if (!confirm("Revoke all sessions? All users (including you) will be logged out.")) return;
              try {
                await api.post("/auth/revoke-all");
                setMessage({ text: "All sessions revoked", type: "success" });
                setTimeout(() => { window.location.href = "/login"; }, 2000);
              } catch (e) { setMessage({ text: e instanceof Error ? e.message : "Failed", type: "error" }); }
            }} className="px-3 py-1.5 bg-red-500/10 text-danger-400 rounded text-xs font-medium hover:bg-red-500/20">Revoke All Sessions</button>
          </div>
        </div>

        {/* Feature #4: API Key Management */}
        <div className="bg-dark-800 rounded-lg border border-dark-500 overflow-hidden mt-4">
          <div className="px-5 py-3 border-b border-dark-600 flex justify-between items-center">
            <h3 className="text-xs font-medium text-dark-300 uppercase font-mono tracking-widest">API Keys</h3>
            <button onClick={() => setShowNewKey(!showNewKey)} className="text-xs text-rust-400 hover:text-rust-300">
              {showNewKey ? "Cancel" : "+ Create Key"}
            </button>
          </div>
          {showNewKey && (
            <div className="px-5 py-3 border-b border-dark-600 flex gap-2">
              <input value={newKeyName} onChange={e => setNewKeyName(e.target.value)} placeholder="Key name"
                className="flex-1 px-3 py-1.5 border border-dark-500 rounded text-sm focus:ring-2 focus:ring-accent-500 outline-none" />
              <button onClick={async () => {
                try {
                  const result = await api.post<{ key: string }>("/api-keys", { name: newKeyName || "API Key" });
                  setNewKeyResult(result.key);
                  setNewKeyName("");
                  setShowNewKey(false);
                  api.get<any[]>("/api-keys").then(setApiKeys).catch(() => {});
                } catch (e) { setMessage({ text: e instanceof Error ? e.message : "Failed", type: "error" }); }
              }} className="px-3 py-1.5 bg-rust-500 text-white rounded text-xs font-medium hover:bg-rust-600">Create</button>
            </div>
          )}
          {newKeyResult && (
            <div className="px-5 py-3 border-b border-dark-600 bg-rust-500/5">
              <p className="text-xs text-rust-400 mb-1">Copy this key now — it won't be shown again:</p>
              <div className="flex gap-2">
                <code className="flex-1 px-2 py-1 bg-dark-900 rounded text-xs font-mono text-dark-100 break-all">{newKeyResult}</code>
                <button onClick={() => { navigator.clipboard.writeText(newKeyResult); setNewKeyResult(null); }} className="px-2 py-1 bg-dark-700 rounded text-xs text-dark-200 shrink-0">Copy</button>
              </div>
            </div>
          )}
          <div className="divide-y divide-dark-600">
            {apiKeys.map((k: any) => (
              <div key={k.id} className="px-5 py-2.5 flex items-center justify-between text-xs">
                <div>
                  <span className="text-dark-100">{k.name}</span>
                  <span className="text-dark-400 ml-2">Created {new Date(k.created_at).toLocaleDateString()}</span>
                </div>
                <div className="flex gap-2">
                  <button onClick={async () => {
                    try {
                      const r = await api.post<{ key: string }>(`/api-keys/${k.id}/rotate`);
                      setNewKeyResult(r.key);
                      api.get<any[]>("/api-keys").then(setApiKeys).catch(() => {});
                    } catch (e) { setMessage({ text: e instanceof Error ? e.message : "Failed", type: "error" }); }
                  }} className="text-dark-300 hover:text-dark-100">Rotate</button>
                  <button onClick={async () => {
                    if (!confirm("Revoke this API key?")) return;
                    try {
                      await api.delete(`/api-keys/${k.id}`);
                      setApiKeys(apiKeys.filter((a: any) => a.id !== k.id));
                    } catch (e) { setMessage({ text: e instanceof Error ? e.message : "Failed", type: "error" }); }
                  }} className="text-danger-400 hover:text-danger-300">Revoke</button>
                </div>
              </div>
            ))}
            {apiKeys.length === 0 && <p className="px-5 py-3 text-xs text-dark-300">No API keys created</p>}
          </div>
        </div>
        </>)}

        {/* Notification Channels */}
        {tab === "notifications" && (<>
        <div className="bg-dark-800 rounded-lg border border-dark-500 overflow-hidden">
          <div className="px-5 py-3 border-b border-dark-600">
            <h3 className="text-xs font-medium text-dark-300 uppercase font-mono tracking-widest">Notification Channels</h3>
            <p className="text-xs text-dark-200 mt-0.5">Where to send alert notifications</p>
          </div>
          <div className="p-5 space-y-4">
            <div className="flex items-center gap-3">
              <input
                type="checkbox"
                id="notify-email"
                checked={notifyEmail}
                onChange={(e) => setNotifyEmail(e.target.checked)}
                className="rounded border-dark-500 text-rust-500 focus:ring-accent-500"
              />
              <label htmlFor="notify-email" className="text-sm text-dark-100">Email notifications</label>
            </div>
            <div>
              <label htmlFor="notify-slack" className="block text-sm font-medium text-dark-100 mb-1">Slack Webhook URL</label>
              <div className="flex gap-2">
                <input
                  id="notify-slack"
                  type="url"
                  value={notifySlackUrl}
                  onChange={(e) => setNotifySlackUrl(e.target.value)}
                  className="flex-1 px-3 py-2 border border-dark-500 rounded-lg text-sm focus:ring-2 focus:ring-accent-500 outline-none font-mono"
                  placeholder="https://hooks.slack.com/services/..."
                />
                <button
                  disabled={!notifySlackUrl || testingWebhook === "slack"}
                  onClick={async () => {
                    setTestingWebhook("slack");
                    setWebhookResult({ type: "", msg: "" });
                    try {
                      await api.post("/settings/test-webhook", { url: notifySlackUrl, service: "slack" });
                      setWebhookResult({ type: "slack-ok", msg: "Sent!" });
                    } catch (e) {
                      setWebhookResult({ type: "slack-err", msg: e instanceof Error ? e.message : "Failed" });
                    } finally {
                      setTestingWebhook(null);
                    }
                  }}
                  className="px-3 py-2 bg-dark-700 text-dark-100 rounded-lg text-xs font-medium hover:bg-dark-600 disabled:opacity-50 shrink-0"
                >
                  {testingWebhook === "slack" ? "Testing..." : "Test"}
                </button>
              </div>
              {webhookResult.type === "slack-ok" && <p className="text-xs text-rust-400 mt-1">{webhookResult.msg}</p>}
              {webhookResult.type === "slack-err" && <p className="text-xs text-danger-400 mt-1">{webhookResult.msg}</p>}
            </div>
            <div>
              <label htmlFor="notify-discord" className="block text-sm font-medium text-dark-100 mb-1">Discord Webhook URL</label>
              <div className="flex gap-2">
                <input
                  id="notify-discord"
                  type="url"
                  value={notifyDiscordUrl}
                  onChange={(e) => setNotifyDiscordUrl(e.target.value)}
                  className="flex-1 px-3 py-2 border border-dark-500 rounded-lg text-sm focus:ring-2 focus:ring-accent-500 outline-none font-mono"
                  placeholder="https://discord.com/api/webhooks/..."
                />
                <button
                  disabled={!notifyDiscordUrl || testingWebhook === "discord"}
                  onClick={async () => {
                    setTestingWebhook("discord");
                    setWebhookResult({ type: "", msg: "" });
                    try {
                      await api.post("/settings/test-webhook", { url: notifyDiscordUrl, service: "discord" });
                      setWebhookResult({ type: "discord-ok", msg: "Sent!" });
                    } catch (e) {
                      setWebhookResult({ type: "discord-err", msg: e instanceof Error ? e.message : "Failed" });
                    } finally {
                      setTestingWebhook(null);
                    }
                  }}
                  className="px-3 py-2 bg-dark-700 text-dark-100 rounded-lg text-xs font-medium hover:bg-dark-600 disabled:opacity-50 shrink-0"
                >
                  {testingWebhook === "discord" ? "Testing..." : "Test"}
                </button>
              </div>
              {webhookResult.type === "discord-ok" && <p className="text-xs text-rust-400 mt-1">{webhookResult.msg}</p>}
              {webhookResult.type === "discord-err" && <p className="text-xs text-danger-400 mt-1">{webhookResult.msg}</p>}
            </div>
            <div>
              <label htmlFor="notify-pagerduty" className="block text-sm font-medium text-dark-100 mb-1">PagerDuty Integration Key</label>
              <input
                id="notify-pagerduty"
                type="text"
                value={notifyPagerdutyKey}
                onChange={(e) => setNotifyPagerdutyKey(e.target.value)}
                className="w-full px-3 py-2 border border-dark-500 rounded-lg text-sm focus:ring-2 focus:ring-accent-500 outline-none font-mono"
                placeholder="Integration key from PagerDuty service"
              />
              <p className="text-xs text-dark-300 mt-1">Events API v2 integration key. Get it from PagerDuty &gt; Services &gt; Integrations.</p>
            </div>
            <div className="flex justify-end">
              <button
                onClick={async () => {
                  setSaving("notify");
                  setMessage({ text: "", type: "" });
                  try {
                    await api.put("/alert-rules", {
                      notify_email: notifyEmail,
                      notify_slack_url: notifySlackUrl || null,
                      notify_discord_url: notifyDiscordUrl || null,
                      notify_pagerduty_key: notifyPagerdutyKey || null,
                    });
                    setMessage({ text: "Notification channels saved", type: "success" });
                  } catch (e) {
                    setMessage({ text: e instanceof Error ? e.message : "Failed", type: "error" });
                  } finally {
                    setSaving(null);
                  }
                }}
                disabled={saving === "notify"}
                className="px-4 py-2 bg-rust-500 text-white rounded-lg text-sm font-medium hover:bg-rust-600 disabled:opacity-50"
              >
                {saving === "notify" ? "Saving..." : "Save"}
              </button>
            </div>
          </div>
        </div>

        {/* Feature #8: Email Footer + Feature #13: Events Webhook */}
        <div className="bg-dark-800 rounded-lg border border-dark-500 overflow-hidden mt-4">
          <div className="px-5 py-3 border-b border-dark-600">
            <h3 className="text-xs font-medium text-dark-300 uppercase font-mono tracking-widest">Additional Settings</h3>
          </div>
          <div className="p-5 space-y-4">
            <div>
              <label className="block text-sm font-medium text-dark-100 mb-1">Email Footer Text</label>
              <input type="text" value={settings.email_footer || ""} onChange={e => setSettings({ ...settings, email_footer: e.target.value })}
                placeholder="Sent by DockPanel" className="w-full px-3 py-2 border border-dark-500 rounded-lg text-sm focus:ring-2 focus:ring-accent-500 outline-none" />
              <p className="text-xs text-dark-300 mt-1">Custom footer text appended to notification emails</p>
            </div>
            <div>
              <label className="block text-sm font-medium text-dark-100 mb-1">Panel Events Webhook</label>
              <input type="url" value={settings.events_webhook_url || ""} onChange={e => setSettings({ ...settings, events_webhook_url: e.target.value })}
                placeholder="https://example.com/webhook" className="w-full px-3 py-2 border border-dark-500 rounded-lg text-sm font-mono focus:ring-2 focus:ring-accent-500 outline-none" />
              <p className="text-xs text-dark-300 mt-1">Receives POST for site.create, app.deploy, security.scan events</p>
            </div>
            <div className="flex justify-end">
              <button onClick={async () => {
                setSaving("notifyExtra");
                setMessage({ text: "", type: "" });
                try {
                  await api.put("/settings", {
                    email_footer: settings.email_footer || "",
                    events_webhook_url: settings.events_webhook_url || "",
                  });
                  setMessage({ text: "Settings saved", type: "success" });
                } catch (e) { setMessage({ text: e instanceof Error ? e.message : "Failed", type: "error" }); }
                finally { setSaving(null); }
              }} disabled={saving === "notifyExtra"} className="px-4 py-2 bg-rust-500 text-white rounded-lg text-sm font-medium hover:bg-rust-600 disabled:opacity-50">
                {saving === "notifyExtra" ? "Saving..." : "Save"}
              </button>
            </div>
          </div>
        </div>
        </>)}

        {/* Services tab: Service Installers (incl. PowerDNS config), System Health */}
        {tab === "services" && (<>
        {/* Service Installers with integrated PowerDNS config */}
        <ServiceInstallers
          pdnsApiUrl={pdnsApiUrl}
          setPdnsApiUrl={setPdnsApiUrl}
          pdnsApiKey={pdnsApiKey}
          setPdnsApiKey={setPdnsApiKey}
          showPdnsGuide={showPdnsGuide}
          setShowPdnsGuide={setShowPdnsGuide}
          saving={saving}
          setSaving={setSaving}
          setMessage={setMessage}
        />

        {/* System Health */}
        <div className="bg-dark-800 rounded-lg border border-dark-500 overflow-hidden">
          <div className="px-5 py-3 border-b border-dark-600 flex items-center justify-between">
            <h3 className="text-xs font-medium text-dark-300 uppercase font-mono tracking-widest">System Health</h3>
            <button
              onClick={() => {
                setHealthLoading(true);
                loadHealth();
              }}
              className="px-3 py-1 bg-rust-500 text-white rounded-md text-xs font-medium hover:bg-rust-600"
            >
              Check Now
            </button>
          </div>
          <div className="p-5">
            {healthLoading && !health ? (
              <div className="text-center text-sm text-dark-300 py-4">Checking health...</div>
            ) : !health ? (
              <div className="text-center text-sm text-danger-500 py-4">Could not reach health endpoint</div>
            ) : (
              <div className="space-y-4">
                <div className="flex items-center justify-between">
                  <div className="flex items-center gap-3">
                    <div className={`w-3 h-3 rounded-full ${health.database ? "bg-rust-500" : "bg-red-500"}`} />
                    <span className="text-sm text-dark-50">Database</span>
                  </div>
                  <span className={`text-sm font-medium ${health.database ? "text-rust-400" : "text-danger-400"}`}>
                    {health.database ? "Connected" : "Error"}
                  </span>
                </div>
                <div className="flex items-center justify-between">
                  <div className="flex items-center gap-3">
                    <div className={`w-3 h-3 rounded-full ${health.agentOk ? "bg-rust-500" : "bg-red-500"}`} />
                    <span className="text-sm text-dark-50">Agent</span>
                  </div>
                  <span className={`text-sm font-medium ${health.agentOk ? "text-rust-400" : "text-danger-400"}`}>
                    {health.agentOk ? "Connected" : "Error"}
                  </span>
                </div>
                <div className="flex items-center justify-between">
                  <div className="flex items-center gap-3">
                    <div className="w-3 h-3 rounded-full bg-blue-500" />
                    <span className="text-sm text-dark-50">Uptime</span>
                  </div>
                  <span className="text-sm font-medium text-dark-200 font-mono">{health.uptime}</span>
                </div>
              </div>
            )}
          </div>
        </div>
        </>)}

        {tab === "updates" && (
          <UpdatesContent />
        )}

        {tab === "users" && (
          <UsersContent />
        )}
      </div>
    </div>
  );
}

// ── Service Installers Component ────────────────────────────────────────

function ServiceInstallers({ pdnsApiUrl, setPdnsApiUrl, pdnsApiKey, setPdnsApiKey, showPdnsGuide, setShowPdnsGuide, saving, setSaving, setMessage }: {
  pdnsApiUrl: string;
  setPdnsApiUrl: (v: string) => void;
  pdnsApiKey: string;
  setPdnsApiKey: (v: string) => void;
  showPdnsGuide: boolean;
  setShowPdnsGuide: (v: boolean) => void;
  saving: string | null;
  setSaving: (v: string | null) => void;
  setMessage: (v: { text: string; type: string }) => void;
}) {
  const [status, setStatus] = useState<Record<string, { installed?: boolean; running?: boolean; active?: boolean; version?: string | null }> | null>(null);
  const [mailStatus, setMailStatus] = useState<{ installed: boolean; running: boolean } | null>(null);
  const [installing, setInstalling] = useState<string | null>(null);
  const [installId, setInstallId] = useState<string | null>(null);
  const [msg, setMsg] = useState({ text: "", type: "" });
  const [showGuide, setShowGuide] = useState(false);

  const refreshStatus = () => {
    api.get<Record<string, unknown>>("/services/install-status")
      .then((d) => setStatus(d as any))
      .catch(() => {});
    api.get<{ installed: boolean; running: boolean }>("/mail/status")
      .then((d) => setMailStatus(d))
      .catch(() => setMailStatus({ installed: false, running: false }));
  };

  useEffect(refreshStatus, []);

  const install = async (service: string, _label: string) => {
    setInstalling(service);
    setInstallId(null);
    setMsg({ text: "", type: "" });
    try {
      const endpoint = service === "mail" ? "/mail/install" : `/services/install/${service}`;
      const result = await api.post<{ install_id?: string }>(endpoint, {});
      if (result.install_id) {
        setInstallId(result.install_id);
      } else {
        setMsg({ text: `${_label} installed successfully`, type: "success" });
        refreshStatus();
        setInstalling(null);
      }
    } catch (e) {
      setMsg({ text: e instanceof Error ? e.message : "Installation failed", type: "error" });
      setInstalling(null);
    }
  };

  const services = [
    { id: "php", label: "PHP", desc: "PHP-FPM for dynamic websites (WordPress, Laravel, etc.)", field: "php", checkInstalled: (s: any) => s?.php?.installed, checkRunning: (s: any) => s?.php?.running, extra: (s: any) => s?.php?.version ? `v${s.php.version}` : null },
    { id: "certbot", label: "Certbot", desc: "Let's Encrypt SSL certificates with auto-renewal", field: "certbot", checkInstalled: (s: any) => s?.certbot?.installed, checkRunning: () => true, extra: () => null },
    { id: "ufw", label: "UFW Firewall", desc: "Firewall with default rules (SSH, HTTP, HTTPS, mail ports)", field: "ufw", checkInstalled: (s: any) => s?.ufw?.installed, checkRunning: (s: any) => s?.ufw?.active, extra: () => null },
    { id: "fail2ban", label: "Fail2Ban", desc: "Intrusion prevention with SSH, Nginx, Postfix jails", field: "fail2ban", checkInstalled: (s: any) => s?.fail2ban?.installed, checkRunning: (s: any) => s?.fail2ban?.running, extra: () => null },
    { id: "powerdns", label: "PowerDNS", desc: "Self-hosted authoritative DNS server with HTTP API", field: "powerdns", checkInstalled: (s: any) => s?.powerdns?.installed, checkRunning: (s: any) => s?.powerdns?.running, extra: () => null },
    { id: "mail", label: "Mail Server", desc: "Postfix + Dovecot + OpenDKIM for email hosting", field: "mail", checkInstalled: () => mailStatus?.installed ?? null, checkRunning: () => mailStatus?.running ?? false, extra: () => null },
    { id: "redis", label: "Redis", desc: "In-memory cache and data store for PHP applications", field: "redis", checkInstalled: (s: any) => s?.redis?.installed, checkRunning: (s: any) => s?.redis?.running, extra: () => null },
    { id: "nodejs", label: "Node.js", desc: "JavaScript runtime for builds, SSR, and npm packages", field: "nodejs", checkInstalled: (s: any) => s?.nodejs?.installed, checkRunning: () => null, extra: () => null },
    { id: "composer", label: "Composer", desc: "PHP dependency manager for Laravel, Symfony, Drupal", field: "composer", checkInstalled: (s: any) => s?.composer?.installed, checkRunning: () => null, extra: () => null },
  ];

  return (
    <div className="bg-dark-800 rounded-lg border border-dark-500 overflow-hidden">
      <div className="px-5 py-3 border-b border-dark-600">
        <h3 className="text-xs font-medium text-dark-300 uppercase font-mono tracking-widest">Services</h3>
        <p className="text-xs text-dark-200 mt-0.5">One-click install for common server software</p>
      </div>
      <div className="p-5 space-y-4">
        {installId && (
          <ProvisionLog
            sseUrl={`/api/services/install/${installId}/log`}
            onComplete={() => {
              setInstallId(null);
              setInstalling(null);
              refreshStatus();
            }}
          />
        )}

        {msg.text && (
          <div className={`px-4 py-2 rounded-lg text-sm border ${msg.type === "success" ? "bg-rust-500/10 text-rust-400 border-rust-500/20" : "bg-red-500/10 text-danger-400 border-red-500/20"}`}>
            {msg.text}
          </div>
        )}

        <button
          onClick={() => setShowGuide(!showGuide)}
          className="flex items-center gap-2 text-sm text-blue-400 hover:text-blue-300 transition-colors"
        >
          <svg className="w-4 h-4" fill="none" viewBox="0 0 24 24" stroke="currentColor" strokeWidth={2}>
            <path strokeLinecap="round" strokeLinejoin="round" d="M11.25 11.25l.041-.02a.75.75 0 011.063.852l-.708 2.836a.75.75 0 001.063.853l.041-.021M21 12a9 9 0 11-18 0 9 9 0 0118 0zm-9-3.75h.008v.008H12V8.25z" />
          </svg>
          {showGuide ? "Hide details" : "What do these install?"}
          <svg className={`w-3 h-3 transition-transform ${showGuide ? "rotate-180" : ""}`} fill="none" viewBox="0 0 24 24" stroke="currentColor" strokeWidth={2.5}>
            <path strokeLinecap="round" strokeLinejoin="round" d="M19.5 8.25l-7.5 7.5-7.5-7.5" />
          </svg>
        </button>

        {showGuide && (
          <div className="bg-dark-900 border border-dark-500 p-4 text-xs text-dark-200 space-y-2">
            <p><span className="text-dark-100 font-medium">PHP</span> — Installs PHP-FPM + common extensions (mysql, curl, gd, mbstring, xml, zip, opcache). Required for WordPress and PHP sites.</p>
            <p><span className="text-dark-100 font-medium">Certbot</span> — Installs Let's Encrypt certbot with nginx plugin. Enables auto-renewal timer. Required for free SSL certificates.</p>
            <p><span className="text-dark-100 font-medium">UFW</span> — Installs firewall, opens SSH/HTTP/HTTPS/SMTP/IMAPS ports, enables with deny-by-default policy.</p>
            <p><span className="text-dark-100 font-medium">Fail2Ban</span> — Installs intrusion prevention. Creates jails for SSH brute-force, nginx auth failures, Postfix, and Dovecot.</p>
            <p><span className="text-dark-100 font-medium">PowerDNS</span> — Installs authoritative DNS server with PostgreSQL backend. Auto-configures HTTP API and saves credentials to Settings.</p>
            <p><span className="text-dark-100 font-medium">Mail Server</span> — Installs Postfix (SMTP), Dovecot (IMAP/POP3), and OpenDKIM (DKIM signing). Creates vmail user, configures virtual mailbox hosting with SASL auth and submission port (587). Manage domains and mailboxes from the Mail page.</p>
            <p><span className="text-dark-100 font-medium">Redis</span> — Installs Redis in-memory data store. Used as cache backend for PHP applications (WordPress object cache, Laravel, Drupal). Runs as a systemd service on port 6379.</p>
            <p><span className="text-dark-100 font-medium">Node.js</span> — Installs Node.js 22 LTS and npm via NodeSource. Used for build tools, SSR frameworks (Next.js, Nuxt), and running JavaScript/TypeScript applications.</p>
            <p><span className="text-dark-100 font-medium">Composer</span> — Installs Composer globally at /usr/local/bin. The standard PHP dependency manager used by Laravel, Symfony, Drupal, and most PHP frameworks.</p>
          </div>
        )}

        <div className="grid grid-cols-1 sm:grid-cols-2 gap-3">
          {services.map((svc) => {
            const installed = status ? svc.checkInstalled(status) : null;
            const running = status ? svc.checkRunning(status) : null;
            const extra = status ? svc.extra(status) : null;

            return (
              <div key={svc.id} className="border border-dark-500 bg-dark-900/50 p-4 flex items-center justify-between">
                <div>
                  <div className="flex items-center gap-2">
                    <span className="text-sm font-medium text-dark-50">{svc.label}</span>
                    {extra && <span className="text-[10px] text-dark-300">{extra}</span>}
                    {installed === true && running !== null && (
                      <span className={`w-2 h-2 rounded-full ${running ? "bg-rust-400" : "bg-warn-400"}`} title={running ? "Running" : "Installed but not running"} />
                    )}
                  </div>
                  <p className="text-[10px] text-dark-300 mt-0.5">{svc.desc}</p>
                </div>
                {installed ? (
                  <span className="text-[10px] text-dark-300 uppercase tracking-wider shrink-0 ml-3">Installed</span>
                ) : (
                  <button
                    onClick={() => install(svc.id, svc.label)}
                    disabled={installing !== null}
                    className="px-3 py-1.5 bg-rust-500 text-white rounded-lg text-xs font-medium hover:bg-rust-600 disabled:opacity-50 shrink-0 ml-3"
                  >
                    {installing === svc.id ? "Installing..." : "Install"}
                  </button>
                )}
              </div>
            );
          })}
        </div>

        {/* PowerDNS API Configuration */}
        <div className="border-t border-dark-600 pt-4 mt-2 space-y-3">
          <div className="flex items-center justify-between">
            <h4 className="text-xs font-medium text-dark-200 uppercase font-mono tracking-widest">PowerDNS API Configuration</h4>
            <button
              onClick={() => setShowPdnsGuide(!showPdnsGuide)}
              className="flex items-center gap-1.5 text-xs text-blue-400 hover:text-blue-300 transition-colors"
            >
              <svg className="w-3.5 h-3.5" fill="none" viewBox="0 0 24 24" stroke="currentColor" strokeWidth={2}>
                <path strokeLinecap="round" strokeLinejoin="round" d="M11.25 11.25l.041-.02a.75.75 0 011.063.852l-.708 2.836a.75.75 0 001.063.853l.041-.021M21 12a9 9 0 11-18 0 9 9 0 0118 0zm-9-3.75h.008v.008H12V8.25z" />
              </svg>
              {showPdnsGuide ? "Hide guide" : "Setup guide"}
            </button>
          </div>

          {showPdnsGuide && (
            <div className="bg-dark-900 border border-dark-500 p-4 space-y-3 text-sm">
              <p className="text-dark-200 font-medium">Install PowerDNS with PostgreSQL backend:</p>
              <pre className="bg-dark-950 border border-dark-600 p-3 text-xs text-dark-100 font-mono overflow-x-auto whitespace-pre">{`# Install PowerDNS
apt install pdns-server pdns-backend-pgsql

# Create a database for PowerDNS
sudo -u postgres createdb pdns
sudo -u postgres psql pdns < /usr/share/doc/pdns-backend-pgsql/schema.pgsql.sql`}</pre>
              <p className="text-dark-200 font-medium">Configure <span className="text-dark-100 font-mono">/etc/powerdns/pdns.conf</span>:</p>
              <pre className="bg-dark-950 border border-dark-600 p-3 text-xs text-dark-100 font-mono overflow-x-auto whitespace-pre">{`launch=gpgsql
gpgsql-host=127.0.0.1
gpgsql-dbname=pdns
gpgsql-user=postgres

# Enable HTTP API
api=yes
api-key=your-secret-key-here
webserver=yes
webserver-address=127.0.0.1
webserver-port=8081
webserver-allow-from=127.0.0.1`}</pre>
              <p className="text-dark-200 font-medium">Then restart and verify:</p>
              <pre className="bg-dark-950 border border-dark-600 p-3 text-xs text-dark-100 font-mono overflow-x-auto whitespace-pre">{`systemctl restart pdns
curl -s -H "X-API-Key: your-secret-key-here" \\
  http://127.0.0.1:8081/api/v1/servers/localhost | jq .`}</pre>
              <p className="text-xs text-dark-300">After setup, enter the API URL and key below, then create zones from the DNS page.</p>
            </div>
          )}

          <div className="grid grid-cols-1 md:grid-cols-2 gap-3">
            <div>
              <label htmlFor="pdns-url" className="block text-xs font-medium text-dark-100 mb-1">API URL</label>
              <input
                id="pdns-url"
                type="url"
                value={pdnsApiUrl}
                onChange={(e) => setPdnsApiUrl(e.target.value)}
                className="w-full px-3 py-2 border border-dark-500 rounded-lg text-sm focus:ring-2 focus:ring-accent-500 outline-none font-mono"
                placeholder="http://127.0.0.1:8081"
              />
            </div>
            <div>
              <label htmlFor="pdns-key" className="block text-xs font-medium text-dark-100 mb-1">API Key</label>
              <input
                id="pdns-key"
                type="password"
                value={pdnsApiKey}
                onChange={(e) => setPdnsApiKey(e.target.value)}
                className="w-full px-3 py-2 border border-dark-500 rounded-lg text-sm focus:ring-2 focus:ring-accent-500 outline-none font-mono"
                placeholder="PowerDNS API key"
              />
              <p className="text-xs text-dark-300 mt-1">The api-key value from /etc/powerdns/pdns.conf</p>
            </div>
          </div>
          <div className="flex justify-end">
            <button
              onClick={async () => {
                setSaving("pdns");
                setMessage({ text: "", type: "" });
                try {
                  const body: Record<string, string> = { pdns_api_url: pdnsApiUrl };
                  if (pdnsApiKey && pdnsApiKey !== "********") {
                    body.pdns_api_key = pdnsApiKey;
                  }
                  await api.put("/settings", body);
                  setMessage({ text: "PowerDNS settings saved", type: "success" });
                } catch (e) {
                  setMessage({ text: e instanceof Error ? e.message : "Failed", type: "error" });
                } finally {
                  setSaving(null);
                }
              }}
              disabled={saving === "pdns"}
              className="px-4 py-2 bg-rust-500 text-white rounded-lg text-sm font-medium hover:bg-rust-600 disabled:opacity-50"
            >
              {saving === "pdns" ? "Saving..." : "Save PowerDNS Config"}
            </button>
          </div>
        </div>
      </div>
    </div>
  );
}

// ── SSH Keys Component ──────────────────────────────────────────────────

function SSHKeys() {
  const [keys, setKeys] = useState<{ type: string; fingerprint: string; comment: string; key: string }[]>([]);
  const [newKey, setNewKey] = useState("");
  const [adding, setAdding] = useState(false);
  const [msg, setMsg] = useState({ text: "", type: "" });

  useEffect(() => {
    api.get<{ keys: typeof keys }>("/ssh-keys").then(d => setKeys(d.keys || [])).catch(() => {});
  }, []);

  return (
    <div className="bg-dark-800 rounded-lg border border-dark-500 overflow-hidden">
      <div className="px-5 py-3 border-b border-dark-600">
        <h3 className="text-xs font-medium text-dark-300 uppercase font-mono tracking-widest">SSH Keys</h3>
        <p className="text-xs text-dark-200 mt-0.5">Manage authorized SSH keys for root access</p>
      </div>
      <div className="p-5 space-y-3">
        {msg.text && <div className={`px-3 py-2 rounded text-xs ${msg.type === "success" ? "bg-rust-500/10 text-rust-400" : "bg-red-500/10 text-danger-400"}`}>{msg.text}</div>}
        {keys.map((k) => (
          <div key={k.fingerprint} className="flex items-center justify-between bg-dark-900 border border-dark-500 px-4 py-2">
            <div className="min-w-0">
              <span className="text-xs text-dark-50 font-mono block truncate">{k.comment || k.key}</span>
              <span className="text-[10px] text-dark-300 font-mono">{k.fingerprint}</span>
            </div>
            <button onClick={async () => { await api.delete(`/ssh-keys/${encodeURIComponent(k.fingerprint)}`); setKeys(keys.filter(x => x.fingerprint !== k.fingerprint)); }} className="p-1 text-dark-300 hover:text-danger-400 shrink-0 ml-2">
              <svg className="w-4 h-4" fill="none" viewBox="0 0 24 24" stroke="currentColor" strokeWidth={1.5}><path strokeLinecap="round" strokeLinejoin="round" d="M6 18L18 6M6 6l12 12" /></svg>
            </button>
          </div>
        ))}
        <div className="flex gap-2">
          <input type="text" value={newKey} onChange={(e) => setNewKey(e.target.value)} placeholder="ssh-ed25519 AAAA... user@host" className="flex-1 px-3 py-2 border border-dark-500 rounded-lg text-xs font-mono focus:ring-2 focus:ring-accent-500 outline-none" />
          <button disabled={adding || !newKey.startsWith("ssh-")} onClick={async () => {
            setAdding(true); setMsg({ text: "", type: "" });
            try { await api.post("/ssh-keys", { key: newKey }); setNewKey(""); const d = await api.get<{ keys: typeof keys }>("/ssh-keys"); setKeys(d.keys || []); setMsg({ text: "Key added", type: "success" }); }
            catch (e: any) { setMsg({ text: e?.message || "Failed", type: "error" }); }
            finally { setAdding(false); }
          }} className="px-3 py-2 bg-rust-500 text-white rounded-lg text-xs font-medium hover:bg-rust-600 disabled:opacity-50 shrink-0">
            {adding ? "Adding..." : "Add Key"}
          </button>
        </div>
      </div>
    </div>
  );
}

// ── Auto-Updates Component ──────────────────────────────────────────────

function AutoUpdates() {
  const [status, setStatus] = useState<{ installed: boolean; enabled: boolean } | null>(null);
  const [toggling, setToggling] = useState(false);

  useEffect(() => {
    api.get<{ installed: boolean; enabled: boolean }>("/auto-updates/status").then(setStatus).catch(() => {});
  }, []);

  const toggle = async () => {
    if (!status) return;
    setToggling(true);
    try {
      await api.post(status.enabled ? "/auto-updates/disable" : "/auto-updates/enable", {});
      setStatus({ ...status, installed: true, enabled: !status.enabled });
    } catch { /* ignore */ }
    finally { setToggling(false); }
  };

  return (
    <div className="bg-dark-800 rounded-lg border border-dark-500 overflow-hidden">
      <div className="px-5 py-3 border-b border-dark-600">
        <h3 className="text-xs font-medium text-dark-300 uppercase font-mono tracking-widest">Auto-Updates</h3>
        <p className="text-xs text-dark-200 mt-0.5">Automatically install security patches</p>
      </div>
      <div className="p-5">
        <div className="flex items-center justify-between">
          <div>
            <p className="text-sm text-dark-100">Automatic security updates</p>
            <p className="text-xs text-dark-300 mt-0.5">Uses unattended-upgrades to apply security patches automatically</p>
          </div>
          <button onClick={toggle} disabled={toggling} className={`relative inline-flex h-6 w-11 items-center rounded-full transition-colors shrink-0 ${status?.enabled ? "bg-rust-500" : "bg-dark-600"}`}>
            <span className={`inline-block h-4 w-4 transform rounded-full bg-white transition-transform ${status?.enabled ? "translate-x-6" : "translate-x-1"}`} />
          </button>
        </div>
      </div>
    </div>
  );
}

// ── IP Whitelist Component ──────────────────────────────────────────────

function IPWhitelist() {
  const [ips, setIps] = useState<string[]>([]);
  const [newIp, setNewIp] = useState("");
  const [saving, setSaving] = useState(false);
  const [msg, setMsg] = useState({ text: "", type: "" });

  useEffect(() => {
    api.get<{ ips: string[] }>("/panel-whitelist").then(d => setIps(d.ips || [])).catch(() => {});
  }, []);

  const save = async (list: string[]) => {
    setSaving(true); setMsg({ text: "", type: "" });
    try {
      await api.post("/panel-whitelist", { ips: list });
      setIps(list);
      setMsg({ text: list.length > 0 ? `Whitelist saved (${list.length} IPs)` : "Whitelist cleared", type: "success" });
    } catch (e: any) { setMsg({ text: e?.message || "Failed", type: "error" }); }
    finally { setSaving(false); }
  };

  return (
    <div className="bg-dark-800 rounded-lg border border-dark-500 overflow-hidden">
      <div className="px-5 py-3 border-b border-dark-600">
        <h3 className="text-xs font-medium text-dark-300 uppercase font-mono tracking-widest">Panel IP Whitelist</h3>
        <p className="text-xs text-dark-200 mt-0.5">Restrict panel access to specific IPs</p>
      </div>
      <div className="p-5 space-y-3">
        {msg.text && <div className={`px-3 py-2 rounded text-xs ${msg.type === "success" ? "bg-rust-500/10 text-rust-400" : "bg-red-500/10 text-danger-400"}`}>{msg.text}</div>}
        {ips.map((ip, i) => (
          <div key={i} className="flex items-center justify-between bg-dark-900 border border-dark-500 px-3 py-1.5">
            <span className="text-xs text-dark-50 font-mono">{ip}</span>
            <button onClick={() => save(ips.filter((_, j) => j !== i))} className="text-dark-300 hover:text-danger-400">
              <svg className="w-3.5 h-3.5" fill="none" viewBox="0 0 24 24" stroke="currentColor" strokeWidth={2}><path strokeLinecap="round" strokeLinejoin="round" d="M6 18L18 6M6 6l12 12" /></svg>
            </button>
          </div>
        ))}
        <div className="flex gap-2">
          <input type="text" value={newIp} onChange={(e) => setNewIp(e.target.value)} placeholder="e.g. 203.0.113.50" className="flex-1 px-3 py-2 border border-dark-500 rounded-lg text-xs font-mono focus:ring-2 focus:ring-accent-500 outline-none" />
          <button disabled={!newIp || saving} onClick={() => { save([...ips, newIp.trim()]); setNewIp(""); }} className="px-3 py-2 bg-rust-500 text-white rounded-lg text-xs font-medium hover:bg-rust-600 disabled:opacity-50 shrink-0">Add IP</button>
        </div>
        {ips.length > 0 && <button onClick={() => save([])} className="text-xs text-dark-300 hover:text-dark-100">Clear whitelist (allow all)</button>}
      </div>
    </div>
  );
}
