import { useState, useEffect, useRef } from "react";
import { useSearchParams } from "react-router-dom";
import { useAuth } from "../context/AuthContext";
import { api } from "../api";
import ProvisionLog from "../components/ProvisionLog";
import UpdatesContent from "./Updates";
import { timeAgo } from "../utils/format";
import { applyTheme, readStoredTheme } from "../hooks/useLayoutState";
import ConfirmDialog from "../components/ConfirmDialog";
import {
  TwoFactorCard,
  PasskeysCard,
  ChangePasswordCard,
  SessionsCard,
  ApiKeysCard,
  ExportMyDataCard,
} from "../components/AccountSecurity";

// Every alert type the panel can suppress from external channels, in the order
// the grid renders them. This list is the frontend half of a pair: the backend
// rejects a stored value naming anything outside it, so the two must agree
// exactly, and a pin arm compares them.
//
// It was ten entries while the panel had grown to twenty producers, so half the
// alert types that page an operator had no control here at all — including
// every certificate-renewal failure. Adding a producer means adding it here.
//
// No exemptions: a pin arm compares this list against the alert types the
// backend actually raises, reading the call sites rather than either list.
const SUPPRESSIBLE_ALERT_TYPES: { key: string; label: string }[] = [
  { key: "cpu", label: "CPU" },
  { key: "memory", label: "Memory" },
  { key: "disk", label: "Disk" },
  { key: "disk_forecast", label: "Disk forecast" },
  { key: "memory_leak", label: "Memory leak" },
  { key: "offline", label: "Offline" },
  { key: "service_down", label: "Service" },
  { key: "container_down", label: "Container down" },
  { key: "container_crashloop", label: "Container crashloop" },
  { key: "container_unhealthy", label: "Container unhealthy" },
  { key: "gpu_utilization", label: "GPU Util" },
  { key: "gpu_temperature", label: "GPU Temp" },
  { key: "gpu_vram", label: "GPU VRAM" },
  { key: "backup_failure", label: "Backup" },
  { key: "backup_verification_failed", label: "Backup verification" },
  { key: "cron_failure", label: "Cron" },
  { key: "ssl_expiry", label: "SSL expiry" },
  { key: "ssl_renewal_failure", label: "SSL renewal" },
  { key: "security", label: "Security scan" },
  { key: "image_scan", label: "Image scan" },
  { key: "slow_response", label: "Slow response" },
];

/** Which `alert_rules` column governs whether a type is RECORDED at all.
 *
 * Distinct from muting, and the distinction is the whole point of the two
 * columns in the grid below. `muted_types` suppresses the external send while
 * the `alerts` row and the admin bell still happen; these columns are read by
 * `is_alert_enabled` BEFORE the INSERT, so the event never exists — no row, no
 * bell, no status-page incident. A type absent from this map has no such
 * column and is always recorded.
 *
 * The three GPU types deliberately share ONE column: the backend maps
 * gpu_utilization / gpu_temperature / gpu_vram all to `alert_gpu`, so those
 * three checkboxes move together and the GPU Alert Thresholds card further
 * down edits the same value.
 */
const RECORD_COLUMN_BY_TYPE: Record<string, string> = {
  cpu: "alert_cpu",
  memory: "alert_memory",
  disk: "alert_disk",
  offline: "alert_offline",
  service_down: "alert_service_health",
  backup_failure: "alert_backup_failure",
  ssl_expiry: "alert_ssl_expiry",
  gpu_utilization: "alert_gpu",
  gpu_temperature: "alert_gpu",
  gpu_vram: "alert_gpu",
};


/** Preview colours for the theme picker and the layout thumbnails.
 *
 *  Hand-copied out of index.css, field for field:
 *    bg → --color-dark-900 · sidebar → --color-dark-950 · card → --color-dark-800
 *    bar → --color-dark-700 · text → --color-dark-500
 *    accent → --color-rust-400 on the dark themes, --color-rust-500 on the two
 *             light ones (arctic, clean) — rust-400 is a light tint there and
 *             would not clear the 3.0 non-text floor against the swatch card.
 *
 *  It is a copy on purpose: a getComputedStyle probe cannot work while terminal's
 *  ramp lives in @theme on :root with no [data-theme="terminal"] block, because a
 *  probe element inherits the CURRENT theme and would report ember's colours for
 *  Terminal while you are on ember. A pin asserts these stay in step with
 *  index.css — update both together, or the pin goes red.
 *
 *  Previously drifted: every one of ember's six fields was invented (none of the
 *  values appeared in index.css at all), terminal's and midnight's `text` had been
 *  hand-brightened, arctic's accent still held the pre-retune #0d9488, and the
 *  layout thumbnails below had no branch for clean or clean-dark and so rendered
 *  a black terminal preview inside a white card on both. */
const THEME_SWATCHES = [
  { id: "terminal", name: "Terminal", desc: "Hacker aesthetic", bg: "#111113", sidebar: "#09090b", accent: "#22c55e", card: "#18181b", text: "#52525b", bar: "#27272a" },
  { id: "midnight", name: "Midnight", desc: "Deep navy, modern", bg: "#0a1628", sidebar: "#050a18", accent: "#3b82f6", card: "#0f1f3a", text: "#3a5785", bar: "#182d50" },
  { id: "ember", name: "Ember", desc: "Warm & premium", bg: "#1c1917", sidebar: "#100e0d", accent: "#fb923c", card: "#292524", text: "#78716c", bar: "#3b3533" },
  { id: "clean-dark", name: "Clean Dark", desc: "GitHub-dark, rounded", bg: "#161b22", sidebar: "#0d1117", accent: "#3b82f6", card: "#1c2333", text: "#636e7b", bar: "#2d333b" },
  { id: "arctic", name: "Arctic", desc: "Teal & light", bg: "#f7f9fc", sidebar: "#ffffff", accent: "#0d7970", card: "#edf1f7", text: "#8d9bb0", bar: "#dce3ed" },
  { id: "clean", name: "Clean Light", desc: "Modern SaaS, blue", bg: "#f8fafc", sidebar: "#ffffff", accent: "#2563eb", card: "#f1f5f9", text: "#94a3b8", bar: "#e2e8f0" },
] as const;

const DEFAULT_SWATCH = THEME_SWATCHES[1]; // midnight — the default theme

interface HealthStatus {
  db: string;
  agent: string;
  uptime: string;
  database: boolean; // computed
  agentOk: boolean;  // computed
}

interface ExportConfig {
  settings: Record<string, string>;
  [key: string]: unknown;
}

interface CleanupResult {
  cleaned?: string[];
}

/**
 * `PUT /settings` when SMTP keys were among them.
 *
 * The setting is panel-global, so the push is fleet-wide: `smtp` says where the
 * config actually landed, host by host, and `warning` is the API's own sentence
 * for the partial case. Everything below `ok` is absent when the save touched no
 * SMTP key or when no host was configured — hence every field optional.
 */
interface SmtpSaveResult {
  ok: boolean;
  /** Present only on a partial push. Complete sentence, safe to render as-is. */
  warning?: string;
  smtp?: {
    /** Hosts that took the configuration. */
    configured: string[];
    /** Hosts that were asked and refused. */
    failed: { server: string; error: string }[];
    /** Registered hosts no agent could be resolved for — never asked at all. */
    not_asked: { server: string; status: string }[];
  };
}

/** WebAuthn PublicKeyCredentialCreationOptions as returned by the server (base64url-encoded) */
type ServiceStatus = Record<string, { installed?: boolean; running?: boolean; active?: boolean; version?: string | null }>;

interface OAuthRedirects {
  base_url: string;
  base_url_configured: boolean;
  redirect_uris: Record<string, string>;
}

/** Must stay the set routes/oauth.rs::OAUTH_PROVIDERS accepts — a card for a
 *  provider `get_provider` does not know is a control that cannot work. */
const OAUTH_PROVIDERS = [
  { id: "google", label: "Google", console: "https://console.cloud.google.com/apis/credentials" },
  { id: "github", label: "GitHub", console: "https://github.com/settings/developers" },
  { id: "gitlab", label: "GitLab", console: "https://gitlab.com/-/user_settings/applications" },
] as const;

/** The four channels services/notifications.rs looks up a template for. */
const NOTIF_CHANNELS = [
  { id: "email", label: "Email", hint: "HTML body. Replaces the default alert email entirely.", rows: 5 },
  { id: "slack", label: "Slack", hint: "Sent as the `text` field. Default: *{{title}}* then the message.", rows: 3 },
  { id: "discord", label: "Discord", hint: "Sent as `content`. Default: **{{title}}** then the message.", rows: 3 },
  { id: "webhook", label: "Generic webhook", hint: "Becomes the `message` field of the JSON payload.", rows: 3 },
] as const;

/** Placeholders services/notifications.rs::format_message substitutes. Anything
 *  else is passed through verbatim — an operator typing {{host}} gets "{{host}}". */
const NOTIF_PLACEHOLDERS = ["{{title}}", "{{message}}", "{{severity}}", "{{timestamp}}"] as const;

/** The plans routes/billing.rs builds `stripe_price_{plan}` from. */
const STRIPE_PLANS = [
  { id: "starter", label: "Starter" },
  { id: "pro", label: "Pro" },
  { id: "agency", label: "Agency" },
] as const;

export default function Settings() {
  const { user } = useAuth();
  const [settings, setSettings] = useState<Record<string, string>>({});
  const [loading, setLoading] = useState(true);
  const [saving, setSaving] = useState<string | null>(null);
  const [health, setHealth] = useState<HealthStatus | null>(null);
  const [healthLoading, setHealthLoading] = useState(true);
  const [message, setMessage] = useState({ text: "", type: "" });
  // Servers whose agent predates the recording gate and so keep recording no
  // matter what the toggle says. Empty on a single-server install.
  const [recordingLagging, setRecordingLagging] = useState<{ name: string; agent_version: string }[]>([]);
  const healthTimer = useRef<ReturnType<typeof setInterval>>(undefined);

  // Form state
  const [panelName, setPanelName] = useState("");
  const [baseUrl, setBaseUrl] = useState("");
  const [panelIps, setPanelIps] = useState("");
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

  // Auto-healing
  const [autoHealEnabled, setAutoHealEnabled] = useState(false);
  const [autoHealReclaim, setAutoHealReclaim] = useState(false);
  const [reverseProxy, setReverseProxy] = useState("nginx");
  const [traefikInstalling, setTraefikInstalling] = useState(false);
  const [showTraefikEmail, setShowTraefikEmail] = useState(false);
  const [traefikEmail, setTraefikEmail] = useState("admin@example.com");

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
  const [mutedTypes, setMutedTypes] = useState<string[]>([]);
  // Defaults mirror the schema, which declares every one of these NOT NULL
  // DEFAULT TRUE — so an unsaved page shows what the backend would do.
  const [recordTypes, setRecordTypes] = useState<Record<string, boolean>>({
    alert_cpu: true,
    alert_memory: true,
    alert_disk: true,
    alert_offline: true,
    alert_backup_failure: true,
    alert_ssl_expiry: true,
    alert_service_health: true,
  });
  // The Alerts page builds escalation policies and then tells the operator, in its own
  // words, to "attach a policy to an alert rule from the alert-rules editor". This is
  // that editor, and until now it had no such control: the attach endpoint had zero
  // callers, so every rule kept escalation_policy_id NULL and every alert fell back to
  // the hardcoded cadence no matter how many chains had been built.
  const [alertRuleId, setAlertRuleId] = useState<string | null>(null);
  const [escalationPolicyId, setEscalationPolicyId] = useState("");
  const [escalationPolicies, setEscalationPolicies] = useState<{ id: string; name: string }[]>([]);
  // GPU alert thresholds
  const [gpuAlertEnabled, setGpuAlertEnabled] = useState(true);
  const [gpuUtilThreshold, setGpuUtilThreshold] = useState(95);
  const [gpuUtilDuration, setGpuUtilDuration] = useState(5);
  const [gpuTempThreshold, setGpuTempThreshold] = useState(85);
  const [gpuVramThreshold, setGpuVramThreshold] = useState(95);

  // Hostname
  const [hostname, setHostname] = useState("");

  // Theme. Read through readStoredTheme so a legacy stored id ("nexus", "dark")
  // resolves to the theme actually on screen — this used to be the one reader
  // that skipped the migration, so such a user saw NO swatch marked active.
  const [currentTheme, setCurrentTheme] = useState(readStoredTheme);
  const [showHeader, setShowHeader] = useState(() => localStorage.getItem("dp-show-header") === "true");
  const [flatNav, setFlatNav] = useState(() => localStorage.getItem("dp-flat-nav") === "true");

  const [pendingConfirm, setPendingConfirm] = useState<{ type: string; label: string; data?: Record<string, unknown> } | null>(null);

  // OAuth sign-in providers. The keys behind these were writable through the
  // settings API and read by routes/oauth.rs since the feature shipped, with no
  // control anywhere — so the panel's only unconfigurable door was an
  // authentication one. Same shape as the s275 note on OAuth Auto-Registration
  // below: the switch existed, the operator could not see it.
  const [oauthCreds, setOauthCreds] = useState<Record<string, { id: string; secret: string }>>(
    Object.fromEntries(OAUTH_PROVIDERS.map(p => [p.id, { id: "", secret: "" }]))
  );
  const [oauthRedirects, setOauthRedirects] = useState<OAuthRedirects | null>(null);
  const [copiedRedirect, setCopiedRedirect] = useState<string | null>(null);

  // Notification templates (Gap #70) — read by services/notifications.rs.
  const [notifTemplates, setNotifTemplates] = useState<Record<string, string>>(
    Object.fromEntries(NOTIF_CHANNELS.map(c => [c.id, ""]))
  );

  // Stripe plan price IDs — routes/billing.rs looks each one up by
  // `stripe_price_{plan}` and its comment already said "admin configures via
  // Settings page", which was not true of any page.
  const [stripePrices, setStripePrices] = useState<Record<string, string>>(
    Object.fromEntries(STRIPE_PLANS.map(p => [p.id, ""]))
  );
  // Whether STRIPE_SECRET_KEY is present in api.env. The price IDs below are
  // settings; the secret key is not, so the two halves of billing are configured
  // in different places and only one of them is on this page.
  const [billingEnabled, setBillingEnabled] = useState<boolean | null>(null);
  const [canary, setCanary] = useState<{ enabled: boolean; watching: number; total: number; armable: number; paths: { path: string; state: string; plantable: boolean; detail: string | null }[] } | null>(null);
  const [canaryErr, setCanaryErr] = useState<string | null>(null);
  const [arming, setArming] = useState(false);

  const loadSettings = async () => {
    try {
      const data = await api.get<Record<string, string>>("/settings");
      setSettings(data);
      setPanelName(data.panel_name || "");
      setBaseUrl(data.base_url || "");
      setPanelIps(data.allowed_panel_ips || "");
      setSmtpHost(data.smtp_host || "");
      setSmtpPort(data.smtp_port || "");
      setSmtpUser(data.smtp_username || "");
      setSmtpPass(data.smtp_password || "");
      setSmtpFrom(data.smtp_from || "");
      setSmtpFromName(data.smtp_from_name || "");
      setSmtpEncryption(data.smtp_encryption || "starttls");
      setAutoHealEnabled(data.auto_heal_enabled === "true");
      setAutoHealReclaim(data.auto_heal_docker_reclaim === "true");
      setReverseProxy(data.reverse_proxy || "nginx");
      setPdnsApiUrl(data.pdns_api_url || "");
      setPdnsApiKey(data.pdns_api_key || "");
      setOauthCreds(Object.fromEntries(OAUTH_PROVIDERS.map(p => [p.id, {
        id: data[`oauth_${p.id}_client_id`] || "",
        // Arrives as the mask when stored; the backend skips that sentinel on
        // write, so an untouched field cannot blank a configured secret.
        secret: data[`oauth_${p.id}_client_secret`] || "",
      }])));
      setNotifTemplates(Object.fromEntries(
        NOTIF_CHANNELS.map(c => [c.id, data[`notif_template_${c.id}`] || ""])
      ));
      setStripePrices(Object.fromEntries(
        STRIPE_PLANS.map(p => [p.id, data[`stripe_price_${p.id}`] || ""])
      ));
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

  const executeConfirm = async () => {
    if (!pendingConfirm) return;
    const { type, data } = pendingConfirm;
    setPendingConfirm(null);
    try {
      switch (type) {
        case "traefik_uninstall": {
          setTraefikInstalling(true);
          await api.post("/traefik/uninstall");
          setReverseProxy("nginx");
          setMessage({ text: "Switched to nginx", type: "success" });
          setTraefikInstalling(false);
          break;
        }
        case "import_config": {
          if (!data) break;
          await api.post("/settings/import", data.config);
          setMessage({ text: "Config imported", type: "success" });
          window.location.reload();
          break;
        }
        case "revoke_sessions": {
          await api.post("/auth/revoke-all");
          setMessage({ text: "All sessions revoked", type: "success" });
          setTimeout(() => { window.location.href = "/login"; }, 2000);
          break;
        }
      }
    } catch (e) {
      setMessage({ text: e instanceof Error ? e.message : "Action failed", type: "error" });
      if (type === "traefik_uninstall") setTraefikInstalling(false);
    }
  };

  // What the canary tripwire is ACTUALLY watching, as opposed to whether its
  // setting is on. The row below rendered "on" from the setting alone, and the
  // setting is seeded true, so a panel watching zero files looked identical to a
  // panel watching all of them.
  type CanaryPath = { path: string; state: string; plantable: boolean; detail: string | null };
  type CanaryStatus = { enabled: boolean; watching: number; total: number; armable: number; paths: CanaryPath[] };
  const loadCanary = async () => {
    try {
      setCanary(await api.get<CanaryStatus>("/security/canary-status"));
      setCanaryErr(null);
    } catch (e) {
      // Deliberately NOT a silent catch: a status this screen cannot read is the
      // one state the row must never paint as protected.
      setCanary(null);
      setCanaryErr(e instanceof Error ? e.message : "unavailable");
    }
  };

  const loadNotifyChannels = async () => {
    try {
      const rules = await api.get<{ id?: string; escalation_policy_id?: string | null; notify_email?: boolean; notify_slack_url?: string; notify_discord_url?: string; notify_pagerduty_key?: string; muted_types?: string; alert_cpu?: boolean; alert_memory?: boolean; alert_disk?: boolean; alert_offline?: boolean; alert_backup_failure?: boolean; alert_ssl_expiry?: boolean; alert_service_health?: boolean; alert_gpu?: boolean; gpu_util_threshold?: number; gpu_util_duration?: number; gpu_temp_threshold?: number; gpu_vram_threshold?: number }[]>("/alert-rules");
      if (rules.length > 0) {
        const r = rules[0];
        setAlertRuleId(r.id ?? null);
        setEscalationPolicyId(r.escalation_policy_id ?? "");
        setNotifyEmail(r.notify_email !== false);
        setNotifySlackUrl(r.notify_slack_url || "");
        setNotifyDiscordUrl(r.notify_discord_url || "");
        setNotifyPagerdutyKey(r.notify_pagerduty_key || "");
        if (r.muted_types) {
          // Drop tokens this build cannot suppress before they reach the
          // checkboxes. A stale one would otherwise survive every Save — it
          // round-trips through the payload untouched — and the backend now
          // refuses the write, so a settings page carrying one could never be
          // saved again. Filtering here lets the next Save heal the row.
          const known = new Set(SUPPRESSIBLE_ALERT_TYPES.map(t => t.key));
          setMutedTypes(r.muted_types.split(',').map((t: string) => t.trim()).filter((t: string) => known.has(t)));
        }
        // `!== false` so an absent column reads as the schema default (TRUE)
        // rather than as "switched off" — the reassuring direction here is the
        // WRONG one: showing a box ticked for a type the operator had silenced
        // would invite them to save it back on without noticing.
        setRecordTypes({
          alert_cpu: r.alert_cpu !== false,
          alert_memory: r.alert_memory !== false,
          alert_disk: r.alert_disk !== false,
          alert_offline: r.alert_offline !== false,
          alert_backup_failure: r.alert_backup_failure !== false,
          alert_ssl_expiry: r.alert_ssl_expiry !== false,
          alert_service_health: r.alert_service_health !== false,
        });
        if (r.alert_gpu !== undefined) setGpuAlertEnabled(r.alert_gpu);
        if (r.gpu_util_threshold) setGpuUtilThreshold(r.gpu_util_threshold);
        if (r.gpu_util_duration) setGpuUtilDuration(r.gpu_util_duration);
        if (r.gpu_temp_threshold) setGpuTempThreshold(r.gpu_temp_threshold);
        if (r.gpu_vram_threshold) setGpuVramThreshold(r.gpu_vram_threshold);
      }
    } catch { /* ignore */ }
    // A missing policy list must not break the notification tab — the picker just has
    // nothing to offer, which is also the honest state when no policies exist yet.
    try {
      const policies = await api.get<{ id: string; name: string }[]>("/escalation-policies");
      setEscalationPolicies(policies.map(({ id, name }) => ({ id, name })));
    } catch { setEscalationPolicies([]); }
  };

  useEffect(() => {
    loadSettings();
    loadHealth();
    loadNotifyChannels();
    loadCanary();
    api.get<{ count: number }>("/system/updates/count")
      .then((d) => setUpdateCount(d.count))
      .catch(() => {});
    api.get<{ lagging: { name: string; agent_version: string }[] }>("/settings/recording-coverage")
      .then((d) => setRecordingLagging(d.lagging || []))
      .catch(() => {});
    api.get<{ hostname?: string }>("/system/info")
      .then((d) => { if (d.hostname) setHostname(d.hostname); })
      .catch(() => {});
    api.get<OAuthRedirects>("/settings/oauth-redirects")
      .then(setOauthRedirects)
      .catch(() => {});
    api.get<{ billing_enabled?: boolean }>("/billing/plan")
      .then((d) => setBillingEnabled(!!d.billing_enabled))
      .catch(() => {});
    healthTimer.current = setInterval(loadHealth, 30000);
    return () => clearInterval(healthTimer.current);
  }, []);

  const saveGeneral = async () => {
    setSaving("general");
    setMessage({ text: "", type: "" });
    try {
      await api.put("/settings", { panel_name: panelName, base_url: baseUrl.trim() });
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
      // Saving SMTP pushes the config at EVERY member of the fleet, and the
      // write to `settings` succeeding says nothing about whether it landed.
      // `warning` is a complete sentence from the API naming the hosts that
      // rejected it and the hosts that were never asked — the ones whose mail
      // will keep using stale credentials until SMTP is saved again. Same
      // response shape and same handling as BackupOrchestrator's deleteDest.
      const res = await api.put<SmtpSaveResult>("/settings", {
        smtp_host: smtpHost,
        smtp_port: smtpPort,
        smtp_username: smtpUser,
        smtp_password: smtpPass,
        smtp_from: smtpFrom,
        smtp_from_name: smtpFromName,
        smtp_encryption: smtpEncryption,
      });
      const configured = res?.smtp?.configured ?? [];
      setMessage(
        res?.warning
          ? { text: res.warning, type: "warning" }
          : {
              // Name the hosts on the way through: "saved" on its own is the
              // claim that let one box's success stand for the fleet's.
              text: configured.length > 1
                ? `SMTP settings saved and pushed to all ${configured.length} servers (${configured.join(", ")})`
                : "SMTP settings saved",
              type: "success",
            },
      );
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

  // Deep-linkable tab, the same shape Monitoring.tsx and Security.tsx already
  // use. Without it this page always opened on "general", so every control
  // anywhere in the panel that said "configure X in Settings" landed the
  // operator one tab short of X and left them to find it — and there was no way
  // to write a correct link even if you wanted to, because nothing here read the
  // URL. "Configure alert channels →" on the notifications page was the instance
  // that surfaced it; there are several more.
  const SETTINGS_TABS: readonly string[] = ["general", "email", "account", "channels", "services"];
  const [searchParams] = useSearchParams();
  const resolveSettingsTab = (raw: string | null): string =>
    raw && SETTINGS_TABS.includes(raw) ? raw : "general";
  const [tab, setTab] = useState(() => resolveSettingsTab(searchParams.get("tab")));
  useEffect(() => {
    setTab(resolveSettingsTab(searchParams.get("tab")));
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [searchParams]);

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
    <div>
      <div className="page-header">
        <div>
          <h1 className="page-header-title">Settings</h1>
          <p className="page-header-subtitle">Manage panel configuration</p>
        </div>
      </div>

      <div className="p-6 lg:p-8">
      <div className="flex gap-1 mb-6 border-b border-dark-600 pb-1 overflow-x-auto">
        {[
          { id: "general", label: "General" },
          { id: "email", label: "Email" },
          { id: "account", label: "Account" },
          { id: "channels", label: "Alert Channels" },
          { id: "services", label: "Services" },
        ].map(t => (
          <button key={t.id} onClick={() => setTab(t.id)}
            className={`flex items-center gap-1.5 px-3 py-2 text-sm font-medium rounded-t-lg transition-colors whitespace-nowrap shrink-0 ${
              tab === t.id ? "text-rust-400 border-b-2 border-rust-400" : "text-dark-300 hover:text-dark-100"
            }`}>
            {t.label}
          </button>
        ))}
      </div>

      {message.text && (
        <div
          className={`mb-4 px-4 py-3 rounded-lg text-sm border ${
            message.type === "success"
              ? "bg-rust-500/10 text-rust-400 border-rust-500/20"
              : message.type === "warning"
                // A fleet operation that partly landed is neither. Red would say
                // the save failed; green would say every host has the new config.
                ? "bg-warn-500/10 text-warn-400 border-warn-500/20"
                : "bg-danger-500/10 text-danger-400 border-danger-500/20"
          }`}
        >
          {message.text}
        </div>
      )}

      {/* Issue #120: this used to be an inline bar rendered here, at the top of
          the page. The Reverse Proxy control that arms it sits near the bottom,
          so switching back to nginx looked like it did nothing at all. It is a
          portalled dialog now — see components/ConfirmDialog. */}
      {pendingConfirm && (
        <ConfirmDialog
          label={pendingConfirm.label}
          tone={pendingConfirm.type === "revoke_sessions" ? "danger" : "warn"}
          onConfirm={executeConfirm}
          onCancel={() => setPendingConfirm(null)}
        />
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
            <div>
              <label htmlFor="base_url" className="block text-sm font-medium text-dark-100 mb-1">Panel URL</label>
              <input
                id="base_url"
                type="url"
                value={baseUrl}
                onChange={(e) => setBaseUrl(e.target.value)}
                className="w-full px-3 py-2 border border-dark-500 rounded-lg text-sm focus:ring-2 focus:ring-accent-500 focus:border-accent-500"
                placeholder="https://panel.example.com"
              />
              <p className="text-xs text-dark-200 mt-1">
                Where this panel is reachable. Alert notifications use it to link to the
                matching runbook — leave it empty and alerts are still delivered, but
                without the link.
              </p>
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
                  Auto-restarts crashed services, frees disk space on the server whose disk alert is firing, renews expiring SSL certs
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
              <>
                <div className="text-xs text-dark-300 space-y-1 pl-4 border-l-2 border-dark-600">
                  <p>Crashed services are restarted (max once per 10 minutes)</p>
                  <p>Oversized logs and <span className="font-mono">/tmp</span> files older than 7 days are cleaned when a server crosses its disk threshold (default 85%) — on that server, at most once per hour</p>
                  <p>SSL certs are renewed on the CA's ACME Renewal Information (ARI) schedule, or a profile-aware fallback (2d for shortlived, 15d for tlsserver, 30d for classic). Max once per 6 hours per domain.</p>
                  <p>All actions are logged in the Audit Log page</p>
                </div>

                {/* Docker reclamation is a SEPARATE consent. Until v2.56.0 this ran as
                    part of "cleans logs" and was `docker system prune -af --volumes`. */}
                <div className="flex items-center justify-between pt-2">
                  <div className="pr-4">
                    <p className="text-sm text-dark-100">Also reclaim unused Docker resources</p>
                    <p className="text-xs text-dark-300 mt-0.5">
                      When a disk alert triggers cleanup, additionally remove <span className="font-medium">dangling images, build cache and unattached networks that DockPanel does not manage</span>.
                      Containers, volumes and images belonging to your apps are never touched — including apps that are only sleeping.
                    </p>
                  </div>
                  <button
                    onClick={async () => {
                      const newVal = !autoHealReclaim;
                      try {
                        await api.put("/settings", { auto_heal_docker_reclaim: newVal ? "true" : "false" });
                        setAutoHealReclaim(newVal);
                        setMessage({ text: `Docker reclamation ${newVal ? "enabled" : "disabled"}`, type: "success" });
                      } catch (e) {
                        setMessage({ text: e instanceof Error ? e.message : "Failed", type: "error" });
                      }
                    }}
                    role="switch"
                    aria-checked={autoHealReclaim}
                    aria-label="Reclaim unused Docker resources during disk cleanup"
                    className={`relative shrink-0 inline-flex h-6 w-11 items-center rounded-full transition-colors ${autoHealReclaim ? "bg-rust-500" : "bg-dark-600"}`}
                  >
                    <span className={`inline-block h-4 w-4 transform rounded-full bg-white transition-transform ${autoHealReclaim ? "translate-x-6" : "translate-x-1"}`} />
                  </button>
                </div>
              </>
            )}
          </div>
        </div>

        {/* Reverse Proxy — Traefik option */}
        <div className="bg-dark-800 rounded-lg border border-dark-500 overflow-hidden">
          <div className="px-5 py-3 border-b border-dark-600">
            <h3 className="text-xs font-medium text-dark-300 uppercase font-mono tracking-widest">Reverse Proxy</h3>
            <p className="text-xs text-dark-200 mt-0.5">Choose nginx or Traefik for Docker app routing</p>
          </div>
          <div className="p-5 space-y-4">
            <div className="flex items-center gap-4">
              <button
                onClick={async () => {
                  if (reverseProxy === "traefik") {
                    setPendingConfirm({ type: "traefik_uninstall", label: "Switch back to nginx? Existing Docker apps with domains will need redeployment." });
                  }
                }}
                className={`flex-1 px-4 py-3 rounded-lg border text-sm font-mono text-center transition-colors ${reverseProxy === "nginx" ? "border-rust-500 bg-rust-500/10 text-rust-400" : "border-dark-600 bg-dark-900 text-dark-300 hover:border-dark-400 cursor-pointer"}`}
              >
                <div className="font-bold">nginx</div>
                <div className="text-xs text-dark-400 mt-1">Default, serves PHP/static + proxy</div>
              </button>
              <button
                onClick={() => {
                  if (reverseProxy === "nginx") {
                    setShowTraefikEmail(true);
                    setTraefikEmail("admin@example.com");
                  }
                }}
                disabled={traefikInstalling || showTraefikEmail}
                className={`flex-1 px-4 py-3 rounded-lg border text-sm font-mono text-center transition-colors ${reverseProxy === "traefik" ? "border-rust-500 bg-rust-500/10 text-rust-400" : "border-dark-600 bg-dark-900 text-dark-300 hover:border-dark-400 cursor-pointer"} disabled:opacity-50`}
              >
                <div className="font-bold">{traefikInstalling ? "Installing..." : "Traefik"}</div>
                <div className="text-xs text-dark-400 mt-1">Docker-native, auto-SSL, dashboard</div>
              </button>
            </div>
            {showTraefikEmail && (
              <div className="flex items-center gap-2 mt-3 px-1">
                <label className="text-xs text-dark-300">ACME email for Let's Encrypt:</label>
                <input
                  type="email"
                  value={traefikEmail}
                  onChange={(e) => setTraefikEmail(e.target.value)}
                  onKeyDown={async (e) => {
                    if (e.key === "Enter") {
                      if (!traefikEmail.includes("@") || !traefikEmail.includes(".")) { setMessage({ text: "Invalid email address", type: "error" }); return; }
                      setShowTraefikEmail(false);
                      setTraefikInstalling(true);
                      try {
                        await api.post("/traefik/install", { acme_email: traefikEmail });
                        setReverseProxy("traefik");
                        setMessage({ text: "Traefik installed and active", type: "success" });
                      } catch (err) { setMessage({ text: err instanceof Error ? err.message : "Failed", type: "error" }); }
                      finally { setTraefikInstalling(false); }
                    }
                    if (e.key === "Escape") setShowTraefikEmail(false);
                  }}
                  autoFocus
                  className="flex-1 px-3 py-1.5 bg-dark-900 border border-dark-500 rounded text-sm text-dark-100"
                  placeholder="you@example.com"
                />
                <button
                  onClick={async () => {
                    if (!traefikEmail.includes("@") || !traefikEmail.includes(".")) { setMessage({ text: "Invalid email address", type: "error" }); return; }
                    setShowTraefikEmail(false);
                    setTraefikInstalling(true);
                    try {
                      await api.post("/traefik/install", { acme_email: traefikEmail });
                      setReverseProxy("traefik");
                      setMessage({ text: "Traefik installed and active", type: "success" });
                    } catch (err) { setMessage({ text: err instanceof Error ? err.message : "Failed", type: "error" }); }
                    finally { setTraefikInstalling(false); }
                  }}
                  className="px-3 py-1.5 bg-rust-500 text-white rounded text-xs font-medium"
                >Install</button>
                <button onClick={() => setShowTraefikEmail(false)} className="px-3 py-1.5 bg-dark-600 text-dark-200 rounded text-xs font-medium">Cancel</button>
              </div>
            )}
            {reverseProxy === "traefik" && (
              <div className="text-xs text-dark-300 space-y-1 pl-4 border-l-2 border-rust-500/30">
                <p>Traefik handles Docker app routing via container labels</p>
                <p>Auto-SSL via Let's Encrypt (no manual cert provisioning)</p>
                <p>Dashboard: <a href="http://127.0.0.1:8080" target="_blank" rel="noreferrer" className="text-rust-400 hover:text-rust-300">http://127.0.0.1:8080</a></p>
                <p>Sites (PHP/static) still use nginx</p>
              </div>
            )}
          </div>
        </div>

        {/* Public Status Page — part of General tab */}
        <div className="bg-dark-800 rounded-lg border border-dark-500 overflow-hidden">
          <div className="px-5 py-3 border-b border-dark-600">
            <h3 className="text-xs font-medium text-dark-300 uppercase font-mono tracking-widest">Public Status Page</h3>
            <p className="text-xs text-dark-200 mt-0.5">The master switch for everything served publicly at /status — off by default</p>
          </div>
          <div className="p-5">
            <div className="flex items-center justify-between">
              <div>
                <p className="text-sm text-dark-100">Enable public status page</p>
                <p className="text-xs text-dark-300 mt-0.5">
                  Publishes, to anyone on the internet with no login: every enabled monitor's name and
                  status, your status-page components, and any incident marked visible on the status page —
                  including the ones the alert engine files for you, whose titles name your servers and
                  services. Appearance and history are configured under Incidents.
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

        {/* A timezone selector lived here until v2.46.0, claiming to "affect
            displayed timestamps throughout the panel". Nothing read the key —
            not the backend, not this frontend — so every timestamp stayed UTC.
            Removed rather than left lying; re-add it together with the
            formatting it promises, not before. */}

        {/* Feature #2: Branding */}
        <div className="bg-dark-800 rounded-lg border border-dark-500 overflow-hidden">
          <div className="px-5 py-3 border-b border-dark-600">
            <h3 className="text-xs font-medium text-dark-300 uppercase font-mono tracking-widest">Branding</h3>
          </div>
          <div className="p-5 space-y-3">
            <div>
              <label className="block text-sm font-medium text-dark-100 mb-1">Logo URL</label>
              <input type="url" value={settings.logo_url || ""} onChange={e => setSettings({ ...settings, logo_url: e.target.value })}
                placeholder="https://example.com/logo.png — or upload below" className="w-full px-3 py-2 border border-dark-500 rounded-lg text-sm focus:ring-2 focus:ring-accent-500 outline-none" />
              <div className="mt-2 flex items-center gap-3">
                <label className="px-3 py-1.5 bg-dark-700 hover:bg-dark-600 border border-dark-500 rounded-lg text-xs font-medium cursor-pointer">
                  Upload image…
                  <input type="file" accept="image/png,image/jpeg,image/webp" className="hidden" onChange={async (e) => {
                    const file = e.target.files?.[0];
                    if (!file) return;
                    if (file.size > 2 * 1024 * 1024) {
                      setMessage({ text: "Image too large (max 2 MB)", type: "error" });
                      return;
                    }
                    try {
                      const buf = await file.arrayBuffer();
                      const res = await fetch("/api/branding/logo", {
                        method: "POST",
                        credentials: "same-origin",
                        headers: { "Content-Type": file.type, "X-Requested-With": "DockPanel" },
                        body: buf,
                      });
                      const data = await res.json().catch(() => ({}));
                      if (!res.ok) throw new Error(data.error || `Upload failed (${res.status})`);
                      const newUrl = data.logo_url as string;
                      setSettings({ ...settings, logo_url: newUrl });
                      await api.put("/settings", { logo_url: newUrl });
                      setMessage({ text: "Logo uploaded", type: "success" });
                    } catch (err) {
                      setMessage({ text: err instanceof Error ? err.message : "Upload failed", type: "error" });
                    } finally {
                      e.target.value = "";
                    }
                  }} />
                </label>
                <span className="text-xs text-dark-300">PNG, JPEG, or WebP — up to 2 MB</span>
              </div>
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
            {/* hide_branding was the third key in this card's own family with no
                control: GET /api/branding returns it, and NexusLayout,
                CommandLayout and Login all hide the DockPanel mark when it is
                set — a white-label switch every consumer honoured and nobody
                could throw. Default is closed (an absent row reads false), so
                this compares === "true", unlike the default-open toggles in the
                Security Hardening card. */}
            <div className="flex items-center justify-between pt-1">
              <div>
                <p className="text-sm text-dark-100">Hide DockPanel Branding</p>
                <p className="text-xs text-dark-400">Remove the DockPanel name from the sidebar, header and login page</p>
              </div>
              <button onClick={() => setSettings({ ...settings, hide_branding: settings.hide_branding === "true" ? "false" : "true" })}
                className={`relative w-11 h-6 rounded-full transition-colors shrink-0 ${settings.hide_branding === "true" ? "bg-rust-500" : "bg-dark-600"}`}>
                <div className={`absolute top-0.5 w-5 h-5 bg-white rounded-full transition-transform ${settings.hide_branding === "true" ? "translate-x-5.5 left-0.5" : "left-0.5"}`} />
              </button>
            </div>
            <button onClick={async () => {
              try {
                await api.put("/settings", {
                  logo_url: settings.logo_url || "",
                  accent_color: settings.accent_color || "",
                  hide_branding: settings.hide_branding === "true" ? "true" : "false",
                });
                setMessage({ text: "Branding saved", type: "success" });
              } catch (err) { setMessage({ text: err instanceof Error ? err.message : "Failed", type: "error" }); }
            }} className="px-4 py-2 bg-rust-500 text-white rounded-lg text-sm font-medium hover:bg-rust-600 disabled:opacity-50">Save Branding</button>
          </div>
        </div>

        {/* Stripe plan pricing.
            routes/billing.rs builds the key from the plan being checked out, and
            when the row is missing answers 503 "Price not configured for the pro
            plan. Set '…' in settings." — an error naming a setting no page could
            set, next to a comment reading "admin configures via Settings page".
            This is that page.
            The key names are deliberately not spelled out here: §9 of
            tests/settings-controls-pin-e2e.sh greps this tree for them, and a
            key named only in prose would credit a control that does not exist. */}
        {user?.role === "admin" && (
        <div className="bg-dark-800 rounded-lg border border-dark-500 overflow-hidden">
          <div className="px-5 py-3 border-b border-dark-600">
            <h3 className="text-xs font-medium text-dark-300 uppercase font-mono tracking-widest">Stripe Plan Pricing</h3>
            <p className="text-xs text-dark-200 mt-0.5">Stripe Price IDs used at checkout. A plan with no price ID cannot be subscribed to.</p>
          </div>
          {billingEnabled === false && (
            <div className="px-5 py-3 border-b border-dark-600 bg-dark-900/40">
              <p className="text-xs text-dark-300">
                <span className="text-dark-100 font-medium">Billing is off.</span> The price IDs below are stored but unused until
                <code className="font-mono mx-1">STRIPE_SECRET_KEY</code> is set in <code className="font-mono">/etc/dockpanel/api.env</code> —
                that half of the configuration is not a panel setting.
              </p>
            </div>
          )}
          <div className="p-5 space-y-3">
            {STRIPE_PLANS.map(p => (
              <div key={p.id}>
                <label htmlFor={`stripe-price-${p.id}`} className="block text-sm font-medium text-dark-100 mb-1">{p.label}</label>
                <input
                  id={`stripe-price-${p.id}`}
                  type="text"
                  value={stripePrices[p.id] || ""}
                  onChange={e => setStripePrices(prev => ({ ...prev, [p.id]: e.target.value }))}
                  placeholder="price_1AbCdEfGhIjKlMnO"
                  className="w-full px-3 py-2 border border-dark-500 rounded-lg text-sm font-mono focus:ring-2 focus:ring-accent-500 focus:border-accent-500 outline-none"
                />
              </div>
            ))}
            <button
              onClick={async () => {
                setSaving("stripe");
                try {
                  const body = Object.fromEntries(
                    STRIPE_PLANS.map(p => [`stripe_price_${p.id}`, (stripePrices[p.id] || "").trim()])
                  );
                  await api.put("/settings", body);
                  setSettings(prev => ({ ...prev, ...body }));
                  setMessage({ text: "Plan pricing saved", type: "success" });
                } catch (e) {
                  setMessage({ text: e instanceof Error ? e.message : "Failed", type: "error" });
                } finally {
                  setSaving(null);
                }
              }}
              disabled={saving === "stripe"}
              className="px-4 py-2 bg-rust-500 text-white rounded-lg text-sm font-medium hover:bg-rust-600 disabled:opacity-50"
            >{saving === "stripe" ? "Saving..." : "Save Pricing"}</button>
          </div>
        </div>
        )}

        {/* Feature #5: Configuration Backup */}
        <div className="bg-dark-800 rounded-lg border border-dark-500 overflow-hidden">
          <div className="px-5 py-3 border-b border-dark-600">
            <h3 className="text-xs font-medium text-dark-300 uppercase font-mono tracking-widest">Configuration Backup</h3>
          </div>
          <div className="p-5 flex gap-3">
            <button onClick={async () => {
              try {
                const data = await api.get<ExportConfig>("/settings/export");
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
                  setPendingConfirm({
                    type: "import_config",
                    label: `Import ${Object.keys(data.settings || {}).length} settings? This will overwrite existing values.`,
                    data: { config: data }
                  });
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
                const result = await api.post<CleanupResult>("/system/cleanup");
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
                  { id: "command", name: "Sidebar", desc: "Full sidebar, grouped nav",
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
                  { id: "glass", name: "Compact", desc: "Collapsible icon sidebar",
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
                  { id: "atlas", name: "Topbar", desc: "Horizontal navbar, breadcrumbs",
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
                  // Same table as the theme picker below. The ternary chain this
                  // replaces branched on midnight/arctic/ember only, so both Clean
                  // themes fell through to terminal's dark values.
                  const sw = THEME_SWATCHES.find(s => s.id === currentTheme) ?? DEFAULT_SWATCH;
                  const { accent, bg, bar, text } = sw;
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

            {/* Layout options */}
            <div className="flex gap-6 text-sm">
              <label className="flex items-center gap-2 text-dark-100 cursor-pointer">
                <input type="checkbox"
                  checked={showHeader}
                  onChange={e => {
                    const val = e.target.checked;
                    setShowHeader(val);
                    localStorage.setItem("dp-show-header", val ? "true" : "false");
                    window.dispatchEvent(new Event("dp-layout-options-change"));
                  }}
                />
                Show top header bar
              </label>
              <label className="flex items-center gap-2 text-dark-100 cursor-pointer">
                <input type="checkbox"
                  checked={flatNav}
                  onChange={e => {
                    const val = e.target.checked;
                    setFlatNav(val);
                    localStorage.setItem("dp-flat-nav", val ? "true" : "false");
                    window.dispatchEvent(new Event("dp-layout-options-change"));
                  }}
                />
                Flat navigation (no groups)
              </label>
            </div>

            {/* Theme selector */}
            <div>
              <p className="text-sm text-dark-100 mb-3">Theme</p>
              <div className="grid grid-cols-2 sm:grid-cols-4 gap-3">
                {THEME_SWATCHES.map(t => {
                  const active = currentTheme === t.id;
                  return (
                    <button key={t.id} onClick={() => {
                      // Goes through applyTheme so the layout's useLayoutState
                      // re-seeds. Writing the DOM here directly left that hook on
                      // its mount-time value, so the header cycle button computed
                      // its next theme from a stale one and looked dead for a click.
                      applyTheme(t.id);
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

        {/* The self-service cards below are SHARED with `pages/Account.tsx`, the
            non-adminOnly door every role can reach. One implementation, two
            callers — a copy here would drift the moment either side changed. */}
        {tab === "account" && (<>
        <TwoFactorCard />

        {/* 2FA Enforcement. The "(admin only)" in this comment was the ONLY
            thing asserting it — no role check existed, so the toggle rendered
            for every account that reached /settings by URL. `PUT /api/settings`
            is `AdminUser`, so it always refused; a control that refuses is a
            broken promise rather than a leak, which is why it survived. */}
        {user?.role === "admin" && (
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
        )}

        <PasskeysCard />

        {/* SSH Keys — Security tab */}
        <SSHKeys />

        {/* Auto-Updates — Security tab */}
        <AutoUpdates />

        {/* The "Panel IP Whitelist" card that stood here was removed in v2.90.0: it
            wrote a file on the agent host that nothing read, while telling the operator
            "Whitelist saved (N IPs)". The control that actually restricts panel access
            is Panel IP Allowlist, in Security Hardening further down this tab. */}

        <ChangePasswordCard />

        <SessionsCard />

        {/* Revoke every session PANEL-WIDE. Its own copy said "Admin only." and
            nothing enforced it: the row rendered for anyone who reached
            /settings, and only `POST /api/auth/revoke-all` (an `AdminUser`
            route writing a global `sessions_revoked_at`) refused them. A client
            revoking their OWN sessions is `SessionsCard` above; this is the
            different, panel-wide capability, so it stays here and is gated. */}
        {user?.role === "admin" && (
        <div className="bg-dark-800 rounded-lg border border-dark-500 overflow-hidden">
          <div className="px-5 py-3 border-b border-dark-600">
            <h3 className="text-xs font-medium text-dark-300 uppercase font-mono tracking-widest">Panel-Wide Sessions</h3>
          </div>
          <div className="p-5 flex items-center justify-between gap-4">
            <div>
              <p className="text-sm text-dark-100">Revoke every session, panel-wide</p>
              <p className="text-xs text-dark-300 mt-0.5">
                Logs out <span className="text-warn-400">every user of this panel</span>, not just you.
              </p>
            </div>
            <button onClick={() => setPendingConfirm({ type: "revoke_sessions", label: "Revoke all sessions? All users (including you) will be logged out." })}
              className="shrink-0 px-3 py-1.5 bg-danger-500/10 text-danger-400 rounded text-xs font-medium hover:bg-danger-500/20">Revoke All Sessions</button>
          </div>
        </div>
        )}

        <ExportMyDataCard />

        <ApiKeysCard />

        {/* Security Hardening Settings (admin only) */}
        {user?.role === "admin" && (
        <div className="bg-dark-800 rounded-lg border border-dark-500 overflow-hidden mt-4">
          <div className="px-5 py-3 border-b border-dark-600">
            <h3 className="text-xs font-medium text-dark-300 uppercase font-mono tracking-widest">Security Hardening</h3>
            <p className="text-xs text-dark-200 mt-0.5">Post-incident security features</p>
          </div>
          <div className="divide-y divide-dark-700">
            {/* Self-Registration */}
            <div className="px-5 py-3 flex items-center justify-between">
              <div>
                <p className="text-sm text-dark-100">Self-Registration</p>
                <p className="text-xs text-dark-400">Allow new users to register accounts</p>
              </div>
              <button onClick={async () => {
                const current = settings.self_registration_enabled === "true";
                const newVal = !current;
                try {
                  await api.put("/settings", { self_registration_enabled: newVal ? "true" : "false" });
                  setSettings(prev => ({ ...prev, self_registration_enabled: newVal ? "true" : "false" }));
                  setMessage({ text: `Registration ${newVal ? "enabled" : "disabled"}`, type: "success" });
                } catch (e) { setMessage({ text: e instanceof Error ? e.message : "Failed", type: "error" }); }
              }} className={`relative w-11 h-6 rounded-full transition-colors ${settings.self_registration_enabled === "true" ? "bg-rust-500" : "bg-dark-600"}`}>
                <div className={`absolute top-0.5 w-5 h-5 bg-white rounded-full transition-transform ${settings.self_registration_enabled === "true" ? "translate-x-5.5 left-0.5" : "left-0.5"}`} />
              </button>
            </div>
            {/* OAuth Auto-Registration.
                Surfaced at s275. This setting existed and was writable through
                the settings API, but had NO control anywhere in the panel — so
                an operator could turn Self-Registration off above and still have
                new accounts created by anyone who could reach a configured OAuth
                provider, through a switch they could not see.

                Note the comparison: `!== "false"`, not `=== "true"`. The backend
                default for this key is OPEN (an absent row means allowed), which
                is the opposite of the toggle above it. Rendering it the same way
                would show OFF while the server still said yes — a control that
                lies in the reassuring direction. */}
            <div className="px-5 py-3 flex items-center justify-between">
              <div>
                <p className="text-sm text-dark-100">OAuth Auto-Registration</p>
                <p className="text-xs text-dark-400">Create an account on first OAuth sign-in. Turning Self-Registration off also blocks this.</p>
              </div>
              <button onClick={async () => {
                const current = settings.oauth_auto_create !== "false";
                const newVal = !current;
                try {
                  await api.put("/settings", { oauth_auto_create: newVal ? "true" : "false" });
                  setSettings(prev => ({ ...prev, oauth_auto_create: newVal ? "true" : "false" }));
                  setMessage({ text: `OAuth auto-registration ${newVal ? "enabled" : "disabled"}`, type: "success" });
                } catch (e) { setMessage({ text: e instanceof Error ? e.message : "Failed", type: "error" }); }
              }} className={`relative w-11 h-6 rounded-full transition-colors ${settings.oauth_auto_create !== "false" ? "bg-rust-500" : "bg-dark-600"}`}>
                <div className={`absolute top-0.5 w-5 h-5 bg-white rounded-full transition-transform ${settings.oauth_auto_create !== "false" ? "translate-x-5.5 left-0.5" : "left-0.5"}`} />
              </button>
            </div>
            {/* Registration Approval */}
            <div className="px-5 py-3 flex items-center justify-between">
              <div>
                <p className="text-sm text-dark-100">Require Approval for New Users</p>
                <p className="text-xs text-dark-400">New registrations need admin approval before login</p>
              </div>
              <button onClick={async () => {
                const current = settings.security_approval_required === "true";
                const newVal = !current;
                try {
                  await api.put("/settings", { security_approval_required: newVal ? "true" : "false" });
                  setSettings(prev => ({ ...prev, security_approval_required: newVal ? "true" : "false" }));
                  setMessage({ text: `Approval mode ${newVal ? "enabled" : "disabled"}`, type: "success" });
                } catch (e) { setMessage({ text: e instanceof Error ? e.message : "Failed", type: "error" }); }
              }} className={`relative w-11 h-6 rounded-full transition-colors ${settings.security_approval_required === "true" ? "bg-rust-500" : "bg-dark-600"}`}>
                <div className={`absolute top-0.5 w-5 h-5 bg-white rounded-full transition-transform ${settings.security_approval_required === "true" ? "translate-x-5.5 left-0.5" : "left-0.5"}`} />
              </button>
            </div>
            {/* Geo-IP Alerts */}
            <div className="px-5 py-3 flex items-center justify-between">
              <div>
                <p className="text-sm text-dark-100">Geo-IP Login Alerts</p>
                <p className="text-xs text-dark-400">Alert on login from new IPs, VPNs, or datacenters</p>
              </div>
              <button onClick={async () => {
                const current = settings.security_geo_alert_enabled !== "false";
                const newVal = !current;
                try {
                  await api.put("/settings", { security_geo_alert_enabled: newVal ? "true" : "false" });
                  setSettings(prev => ({ ...prev, security_geo_alert_enabled: newVal ? "true" : "false" }));
                  setMessage({ text: `Geo-IP alerts ${newVal ? "enabled" : "disabled"}`, type: "success" });
                } catch (e) { setMessage({ text: e instanceof Error ? e.message : "Failed", type: "error" }); }
              }} className={`relative w-11 h-6 rounded-full transition-colors ${settings.security_geo_alert_enabled !== "false" ? "bg-rust-500" : "bg-dark-600"}`}>
                <div className={`absolute top-0.5 w-5 h-5 bg-white rounded-full transition-transform ${settings.security_geo_alert_enabled !== "false" ? "translate-x-5.5 left-0.5" : "left-0.5"}`} />
              </button>
            </div>
            {/* Session Recording */}
            <div className="px-5 py-3 flex items-center justify-between">
              <div>
                <p className="text-sm text-dark-100">Terminal Session Recording</p>
                <p className="text-xs text-dark-400">Record all terminal sessions for forensic replay</p>
                {/* The toggle is one row in settings, but each server's agent enforces
                    it. An agent older than the gate release ignores the signed claim
                    and records regardless — so without this, switching recording off
                    reports a fleet-wide success that is false for those members. */}
                {recordingLagging.length > 0 && (
                  <p className="text-xs text-warn-400 mt-1">
                    {settings.security_session_recording === "false"
                      ? "Still recording on "
                      : "Not controlled by this toggle on "}
                    {recordingLagging.map((s) => `${s.name} (agent ${s.agent_version})`).join(", ")}
                    {" — update the agent there for this setting to apply."}
                  </p>
                )}
              </div>
              <button onClick={async () => {
                const current = settings.security_session_recording !== "false";
                const newVal = !current;
                try {
                  await api.put("/settings", { security_session_recording: newVal ? "true" : "false" });
                  setSettings(prev => ({ ...prev, security_session_recording: newVal ? "true" : "false" }));
                  setMessage({ text: `Session recording ${newVal ? "enabled" : "disabled"}`, type: "success" });
                } catch (e) { setMessage({ text: e instanceof Error ? e.message : "Failed", type: "error" }); }
              }} className={`relative w-11 h-6 rounded-full transition-colors ${settings.security_session_recording !== "false" ? "bg-rust-500" : "bg-dark-600"}`}>
                <div className={`absolute top-0.5 w-5 h-5 bg-white rounded-full transition-transform ${settings.security_session_recording !== "false" ? "translate-x-5.5 left-0.5" : "left-0.5"}`} />
              </button>
            </div>
            {/* DB Auto-Backup */}
            <div className="px-5 py-3 flex items-center justify-between">
              <div>
                <p className="text-sm text-dark-100">Panel Database Auto-Backup</p>
                <p className="text-xs text-dark-400">Daily PostgreSQL backup with 7-day retention</p>
              </div>
              <button onClick={async () => {
                const current = settings.security_db_backup_enabled !== "false";
                const newVal = !current;
                try {
                  await api.put("/settings", { security_db_backup_enabled: newVal ? "true" : "false" });
                  setSettings(prev => ({ ...prev, security_db_backup_enabled: newVal ? "true" : "false" }));
                  setMessage({ text: `DB auto-backup ${newVal ? "enabled" : "disabled"}`, type: "success" });
                } catch (e) { setMessage({ text: e instanceof Error ? e.message : "Failed", type: "error" }); }
              }} className={`relative w-11 h-6 rounded-full transition-colors ${settings.security_db_backup_enabled !== "false" ? "bg-rust-500" : "bg-dark-600"}`}>
                <div className={`absolute top-0.5 w-5 h-5 bg-white rounded-full transition-transform ${settings.security_db_backup_enabled !== "false" ? "translate-x-5.5 left-0.5" : "left-0.5"}`} />
              </button>
            </div>
            {/* Canary Files */}
            <div className="px-5 py-3">
              <div className="flex items-center justify-between">
                <div>
                  <p className="text-sm text-dark-100">Canary File Monitoring</p>
                  <p className="text-xs text-dark-400">Detect unauthorized filesystem exploration</p>
                </div>
                <button onClick={async () => {
                  const current = settings.security_canary_enabled !== "false";
                  const newVal = !current;
                  try {
                    await api.put("/settings", { security_canary_enabled: newVal ? "true" : "false" });
                    setSettings(prev => ({ ...prev, security_canary_enabled: newVal ? "true" : "false" }));
                    setMessage({ text: `Canary monitoring ${newVal ? "enabled" : "disabled"}`, type: "success" });
                  } catch (e) { setMessage({ text: e instanceof Error ? e.message : "Failed", type: "error" }); }
                }} className={`relative w-11 h-6 rounded-full transition-colors ${settings.security_canary_enabled !== "false" ? "bg-rust-500" : "bg-dark-600"}`}>
                  <div className={`absolute top-0.5 w-5 h-5 bg-white rounded-full transition-transform ${settings.security_canary_enabled !== "false" ? "translate-x-5.5 left-0.5" : "left-0.5"}`} />
                </button>
              </div>
              {/* The switch above reports the SETTING. This reports the TRIPWIRE.
                  They were the same control until v2.132.0, and because nothing in
                  the product planted a canary, "on" meant "watching nothing" on
                  every install — an intrusion detector whose only observable
                  behaviour was silence, which is what a working one looks like. */}
              {settings.security_canary_enabled !== "false" && (
                <div className="mt-2.5">
                  {canaryErr ? (
                    <p className="text-xs text-warn-400">Armed state unavailable ({canaryErr}) — this screen cannot confirm anything is being watched.</p>
                  ) : canary ? (
                    <>
                      <div className="flex items-center justify-between gap-3">
                        <p className={`text-xs ${canary.watching === 0 ? "text-danger-400" : "text-dark-300"}`}>
                          {canary.watching === 0
                            ? "NOT ARMED — no canary file is being watched, so no alert can ever fire."
                            : `Watching ${canary.watching} of ${canary.total} canary paths.`}
                        </p>
                        {canary.armable > 0 && (
                          <button
                            disabled={arming}
                            onClick={async () => {
                              setArming(true);
                              try {
                                const r = await api.post<{ armed: number; total: number; refused: { path: string; reason: string }[] }>("/security/canary/arm", {});
                                await loadCanary();
                                // Report the refusals too. The agent endpoint used to
                                // drop them, which is how three of its four paths went
                                // unplanted without anyone being told.
                                setMessage(
                                  r.refused && r.refused.length > 0
                                    ? { text: `Armed ${r.armed} of ${r.total}. Refused: ${r.refused.map(f => `${f.path} (${f.reason})`).join("; ")}`, type: "error" }
                                    : { text: `Armed ${r.armed} of ${r.total} canary files`, type: "success" }
                                );
                              } catch (e) {
                                setMessage({ text: e instanceof Error ? e.message : "Failed to arm canary files", type: "error" });
                              } finally { setArming(false); }
                            }}
                            className="shrink-0 px-2 py-1 rounded text-xs font-medium bg-rust-500/20 text-rust-400 hover:bg-rust-500/30 disabled:opacity-50"
                          >
                            {arming ? "Arming..." : `Arm ${canary.armable} canary file${canary.armable === 1 ? "" : "s"}`}
                          </button>
                        )}
                      </div>
                      <ul className="mt-1.5 space-y-0.5">
                        {canary.paths.map(p => (
                          <li key={p.path} className="text-xs font-mono flex items-baseline gap-2">
                            <span className={
                              p.state === "watching" ? "text-rust-400"
                                : p.state === "masked" ? "text-dark-400"
                                : "text-warn-400"
                            }>{p.state === "watching" ? "watching" : p.state}</span>
                            <span className="text-dark-300">{p.path}</span>
                            {p.detail && <span className="text-dark-400 font-sans">— {p.detail}</span>}
                          </li>
                        ))}
                      </ul>
                    </>
                  ) : (
                    <p className="text-xs text-dark-400">Checking what is armed...</p>
                  )}
                </div>
              )}
            </div>
            {/* Lockdown Threshold */}
            <div className="px-5 py-3 flex items-center justify-between">
              <div>
                <p className="text-sm text-dark-100">Auto-Lockdown Threshold</p>
                <p className="text-xs text-dark-400">Suspicious events before auto-lockdown triggers (default: 5 in 10 min)</p>
              </div>
              <input type="number" min="2" max="50" value={settings.security_lockdown_threshold || "5"}
                onChange={async (e) => {
                  try {
                    await api.put("/settings", { security_lockdown_threshold: e.target.value });
                    setSettings(prev => ({ ...prev, security_lockdown_threshold: e.target.value }));
                    setMessage({ text: "Threshold updated", type: "success" });
                  } catch (err) { setMessage({ text: err instanceof Error ? err.message : "Failed to save", type: "error" }); }
                }}
                className="w-16 px-2 py-1 border border-dark-500 rounded text-sm text-center focus:ring-2 focus:ring-accent-500 outline-none bg-dark-700"
              />
            </div>
            {/* Lockdown window — the other half of the threshold rule. It had no
                control at all until v2.46.0, so the sentence "5 in 10 min" was
                only half adjustable. Default mirrors the server's own (10). */}
            <div className="px-5 py-3 flex items-center justify-between">
              <div>
                <p className="text-sm text-dark-100">Auto-Lockdown Window</p>
                <p className="text-xs text-dark-400">Minutes those suspicious events must fall within (default: 10)</p>
              </div>
              <input type="number" min="1" max="1440" value={settings.security_lockdown_window_minutes || "10"}
                onChange={async (e) => {
                  try {
                    await api.put("/settings", { security_lockdown_window_minutes: e.target.value });
                    setSettings(prev => ({ ...prev, security_lockdown_window_minutes: e.target.value }));
                    setMessage({ text: "Lockdown window updated", type: "success" });
                  } catch (err) { setMessage({ text: err instanceof Error ? err.message : "Failed to save", type: "error" }); }
                }}
                className="w-16 px-2 py-1 border border-dark-500 rounded text-sm text-center focus:ring-2 focus:ring-accent-500 outline-none bg-dark-700"
              />
            </div>
            <div className="px-5 py-3 flex items-center justify-between">
              <div>
                <p className="text-sm text-dark-100">Site Creation Rate Limit</p>
                <p className="text-xs text-dark-400">Max sites one user may create per hour (default: 3, 0 = no limit)</p>
              </div>
              <input type="number" min="0" max="999" value={settings.security_site_rate_limit || "3"}
                onChange={async (e) => {
                  try {
                    await api.put("/settings", { security_site_rate_limit: e.target.value });
                    setSettings(prev => ({ ...prev, security_site_rate_limit: e.target.value }));
                    setMessage({ text: "Rate limit updated", type: "success" });
                  } catch (err) { setMessage({ text: err instanceof Error ? err.message : "Failed to save", type: "error" }); }
                }}
                className="w-16 px-2 py-1 border border-dark-500 rounded text-sm text-center focus:ring-2 focus:ring-accent-500 outline-none bg-dark-700"
              />
            </div>
            {/* Server terminal kill switch. The stored key is INVERTED
                (`server_terminal_disabled`) and the server treats an absent row as
                "not disabled", so the toggle reads `!== "true"` — writing it the
                other way round would draw this OFF on every fresh install while
                the terminal was in fact open. */}
            <div className="px-5 py-3 flex items-center justify-between">
              <div>
                <p className="text-sm text-dark-100">Server Terminal</p>
                <p className="text-xs text-dark-400">Full-server shell for admins. Off leaves per-site terminals working.</p>
              </div>
              <button onClick={async () => {
                const enabled = settings.server_terminal_disabled !== "true";
                const next = enabled ? "true" : "false";
                try {
                  await api.put("/settings", { server_terminal_disabled: next });
                  setSettings(prev => ({ ...prev, server_terminal_disabled: next }));
                  setMessage({ text: `Server terminal ${enabled ? "disabled" : "enabled"}`, type: "success" });
                } catch (e) { setMessage({ text: e instanceof Error ? e.message : "Failed", type: "error" }); }
              }} className={`relative w-11 h-6 rounded-full transition-colors ${settings.server_terminal_disabled !== "true" ? "bg-rust-500" : "bg-dark-600"}`}>
                <div className={`absolute top-0.5 w-5 h-5 bg-white rounded-full transition-transform ${settings.server_terminal_disabled !== "true" ? "translate-x-5.5 left-0.5" : "left-0.5"}`} />
              </button>
            </div>
            <div className="px-5 py-3">
              <p className="text-sm text-dark-100">Panel IP Allowlist</p>
              <p className="text-xs text-dark-400 mb-2">
                Comma-separated IPs or CIDR ranges that may log in. Empty allows every address.
                Requires your reverse proxy to send <code className="font-mono">X-Real-IP</code>; if it does not,
                a non-empty list locks everyone out and only a database edit restores access.
              </p>
              <div className="flex gap-2">
                <input type="text" value={panelIps} onChange={e => setPanelIps(e.target.value)}
                  placeholder="203.0.113.4, 10.0.0.0/8, 2001:db8::/32"
                  className="flex-1 px-3 py-2 border border-dark-500 rounded-lg text-sm font-mono focus:ring-2 focus:ring-accent-500 outline-none" />
                <button onClick={async () => {
                  try {
                    await api.put("/settings", { allowed_panel_ips: panelIps });
                    setSettings(prev => ({ ...prev, allowed_panel_ips: panelIps }));
                    setMessage({ text: panelIps.trim() ? "IP allowlist saved" : "IP allowlist cleared — all addresses allowed", type: "success" });
                  } catch (e) { setMessage({ text: e instanceof Error ? e.message : "Failed", type: "error" }); }
                }} className="px-4 py-2 bg-rust-500 text-white rounded-lg text-sm font-medium hover:bg-rust-600">Save</button>
              </div>
            </div>
          </div>
        </div>
        )}

        {/* Data retention windows. The reader (auto_healer.rs's run_retention_cleanup,
            GAP 67) has existed for a while but had no ALLOWED_KEYS entry and no
            control here — every install was stuck at the hardcoded defaults with no
            way to change them. Found by the first dockpanel loose-ends audit. */}
        {user?.role === "admin" && (
        <div className="bg-dark-800 rounded-lg border border-dark-500 overflow-hidden mt-4">
          <div className="px-5 py-3 border-b border-dark-600">
            <h3 className="text-xs font-medium text-dark-300 uppercase font-mono tracking-widest">Data Retention</h3>
            <p className="text-xs text-dark-200 mt-0.5">How long each log/history table keeps rows before the nightly cleanup deletes them.</p>
          </div>
          <div className="divide-y divide-dark-700">
            <div className="px-5 py-3 flex items-center justify-between">
              <div>
                <p className="text-sm text-dark-100">Activity Log</p>
                <p className="text-xs text-dark-400">Days to keep (default: 365)</p>
              </div>
              <input type="number" min="1" max="3650" value={settings.retention_activity_days || "365"}
                onChange={async (e) => {
                  try {
                    await api.put("/settings", { retention_activity_days: e.target.value });
                    setSettings(prev => ({ ...prev, retention_activity_days: e.target.value }));
                    setMessage({ text: "Activity Log retention updated", type: "success" });
                  } catch (err) { setMessage({ text: err instanceof Error ? err.message : "Failed to save", type: "error" }); }
                }}
                className="w-20 px-2 py-1 border border-dark-500 rounded text-sm text-center focus:ring-2 focus:ring-accent-500 outline-none bg-dark-700"
              />
            </div>
            <div className="px-5 py-3 flex items-center justify-between">
              <div>
                <p className="text-sm text-dark-100">System Logs</p>
                <p className="text-xs text-dark-400">Days to keep (default: 30)</p>
              </div>
              <input type="number" min="1" max="3650" value={settings.retention_system_log_days || "30"}
                onChange={async (e) => {
                  try {
                    await api.put("/settings", { retention_system_log_days: e.target.value });
                    setSettings(prev => ({ ...prev, retention_system_log_days: e.target.value }));
                    setMessage({ text: "System Logs retention updated", type: "success" });
                  } catch (err) { setMessage({ text: err instanceof Error ? err.message : "Failed to save", type: "error" }); }
                }}
                className="w-20 px-2 py-1 border border-dark-500 rounded text-sm text-center focus:ring-2 focus:ring-accent-500 outline-none bg-dark-700"
              />
            </div>
            <div className="px-5 py-3 flex items-center justify-between">
              <div>
                <p className="text-sm text-dark-100">Alert History</p>
                <p className="text-xs text-dark-400">Days to keep (default: 90)</p>
              </div>
              <input type="number" min="1" max="3650" value={settings.retention_alert_days || "90"}
                onChange={async (e) => {
                  try {
                    await api.put("/settings", { retention_alert_days: e.target.value });
                    setSettings(prev => ({ ...prev, retention_alert_days: e.target.value }));
                    setMessage({ text: "Alert History retention updated", type: "success" });
                  } catch (err) { setMessage({ text: err instanceof Error ? err.message : "Failed to save", type: "error" }); }
                }}
                className="w-20 px-2 py-1 border border-dark-500 rounded text-sm text-center focus:ring-2 focus:ring-accent-500 outline-none bg-dark-700"
              />
            </div>
            <div className="px-5 py-3 flex items-center justify-between">
              <div>
                <p className="text-sm text-dark-100">Security Scan History</p>
                <p className="text-xs text-dark-400">Days to keep (default: 90)</p>
              </div>
              <input type="number" min="1" max="3650" value={settings.retention_scan_days || "90"}
                onChange={async (e) => {
                  try {
                    await api.put("/settings", { retention_scan_days: e.target.value });
                    setSettings(prev => ({ ...prev, retention_scan_days: e.target.value }));
                    setMessage({ text: "Security Scan History retention updated", type: "success" });
                  } catch (err) { setMessage({ text: err instanceof Error ? err.message : "Failed to save", type: "error" }); }
                }}
                className="w-20 px-2 py-1 border border-dark-500 rounded text-sm text-center focus:ring-2 focus:ring-accent-500 outline-none bg-dark-700"
              />
            </div>
            <div className="px-5 py-3 flex items-center justify-between">
              <div>
                <p className="text-sm text-dark-100">Webhook Delivery Log</p>
                <p className="text-xs text-dark-400">Days to keep (default: 7)</p>
              </div>
              <input type="number" min="1" max="3650" value={settings.retention_webhook_days || "7"}
                onChange={async (e) => {
                  try {
                    await api.put("/settings", { retention_webhook_days: e.target.value });
                    setSettings(prev => ({ ...prev, retention_webhook_days: e.target.value }));
                    setMessage({ text: "Webhook Delivery Log retention updated", type: "success" });
                  } catch (err) { setMessage({ text: err instanceof Error ? err.message : "Failed to save", type: "error" }); }
                }}
                className="w-20 px-2 py-1 border border-dark-500 rounded text-sm text-center focus:ring-2 focus:ring-accent-500 outline-none bg-dark-700"
              />
            </div>
            <div className="px-5 py-3 flex items-center justify-between">
              <div>
                <p className="text-sm text-dark-100">Notification History</p>
                <p className="text-xs text-dark-400">Days to keep (default: 30)</p>
              </div>
              <input type="number" min="1" max="3650" value={settings.retention_notification_days || "30"}
                onChange={async (e) => {
                  try {
                    await api.put("/settings", { retention_notification_days: e.target.value });
                    setSettings(prev => ({ ...prev, retention_notification_days: e.target.value }));
                    setMessage({ text: "Notification History retention updated", type: "success" });
                  } catch (err) { setMessage({ text: err instanceof Error ? err.message : "Failed to save", type: "error" }); }
                }}
                className="w-20 px-2 py-1 border border-dark-500 rounded text-sm text-center focus:ring-2 focus:ring-accent-500 outline-none bg-dark-700"
              />
            </div>
            <div className="px-5 py-3 flex items-center justify-between">
              <div>
                <p className="text-sm text-dark-100">Monitor Check History</p>
                <p className="text-xs text-dark-400">Days to keep (default: 7)</p>
              </div>
              <input type="number" min="1" max="3650" value={settings.retention_monitor_days || "7"}
                onChange={async (e) => {
                  try {
                    await api.put("/settings", { retention_monitor_days: e.target.value });
                    setSettings(prev => ({ ...prev, retention_monitor_days: e.target.value }));
                    setMessage({ text: "Monitor Check History retention updated", type: "success" });
                  } catch (err) { setMessage({ text: err instanceof Error ? err.message : "Failed to save", type: "error" }); }
                }}
                className="w-20 px-2 py-1 border border-dark-500 rounded text-sm text-center focus:ring-2 focus:ring-accent-500 outline-none bg-dark-700"
              />
            </div>
          </div>
        </div>
        )}

        {user?.role === "admin" && <CredentialEncryptionCard setMessage={setMessage} />}

        {/* OAuth Sign-In Providers.
            The six oauth_*_client_id/_client_secret keys were in ALLOWED_KEYS,
            masked on read, encrypted on write and consumed by routes/oauth.rs —
            every part of the path built except the screen. So the panel's one
            unconfigurable setting was a login door, which s277 had just finished
            putting the IP allowlist and lockdown gates on.

            The redirect URI below is fetched from the server, not composed from
            window.location: it is built from BASE_URL, and a panel reached at an
            address other than its configured one would otherwise print a URI the
            server never sends. */}
        {user?.role === "admin" && (
        <div className="bg-dark-800 rounded-lg border border-dark-500 overflow-hidden mt-4">
          <div className="px-5 py-3 border-b border-dark-600">
            <h3 className="text-xs font-medium text-dark-300 uppercase font-mono tracking-widest">OAuth Sign-In</h3>
            <p className="text-xs text-dark-200 mt-0.5">Let users sign in with an external provider. Each configured provider adds a button to the login page.</p>
          </div>
          {oauthRedirects && !oauthRedirects.base_url_configured && (
            <div className="px-5 py-3 border-b border-dark-600 bg-danger-500/5">
              <p className="text-xs text-danger-400">
                <span className="font-medium">BASE_URL is not set</span> in <code className="font-mono">/etc/dockpanel/api.env</code>.
                Redirect URIs are relative without it and no provider will accept one, so sign-in fails after the operator
                has already registered the app. Set it and restart <code className="font-mono">dockpanel-api</code> before configuring below.
              </p>
            </div>
          )}
          <div className="divide-y divide-dark-700">
            {OAUTH_PROVIDERS.map(p => {
              const creds = oauthCreds[p.id] || { id: "", secret: "" };
              const idSet = creds.id.trim() !== "";
              const secretSet = creds.secret.trim() !== "";
              const status = idSet && secretSet ? "active" : idSet || secretSet ? "incomplete" : "unset";
              const redirect = oauthRedirects?.redirect_uris?.[p.id] || "";
              return (
                <div key={p.id} className="px-5 py-4 space-y-3">
                  <div className="flex items-center justify-between">
                    <div className="flex items-center gap-2">
                      <p className="text-sm text-dark-100">{p.label}</p>
                      <span className={`px-1.5 py-0.5 rounded text-[10px] font-mono uppercase tracking-wider ${
                        status === "active" ? "bg-rust-500/10 text-rust-400"
                        : status === "incomplete" ? "bg-danger-500/10 text-danger-400"
                        : "bg-dark-700 text-dark-400"
                      }`}>
                        {status === "active" ? "Active" : status === "incomplete" ? "Incomplete" : "Not configured"}
                      </span>
                    </div>
                    <a href={p.console} target="_blank" rel="noreferrer" className="text-xs text-rust-400 hover:text-rust-300">
                      {p.label} console →
                    </a>
                  </div>
                  <div className="grid grid-cols-1 md:grid-cols-2 gap-3">
                    <div>
                      <label htmlFor={`oauth-${p.id}-id`} className="block text-xs font-medium text-dark-300 mb-1">Client ID</label>
                      <input
                        id={`oauth-${p.id}-id`}
                        type="text"
                        value={creds.id}
                        onChange={e => setOauthCreds(prev => ({ ...prev, [p.id]: { ...prev[p.id], id: e.target.value } }))}
                        placeholder="Client ID from the provider"
                        className="w-full px-3 py-2 border border-dark-500 rounded-lg text-sm font-mono focus:ring-2 focus:ring-accent-500 outline-none"
                      />
                    </div>
                    <div>
                      <label htmlFor={`oauth-${p.id}-secret`} className="block text-xs font-medium text-dark-300 mb-1">Client Secret</label>
                      <input
                        id={`oauth-${p.id}-secret`}
                        type="password"
                        value={creds.secret}
                        onChange={e => setOauthCreds(prev => ({ ...prev, [p.id]: { ...prev[p.id], secret: e.target.value } }))}
                        placeholder="Client secret"
                        className="w-full px-3 py-2 border border-dark-500 rounded-lg text-sm font-mono focus:ring-2 focus:ring-accent-500 outline-none"
                      />
                    </div>
                  </div>
                  {redirect && (
                    <div>
                      <p className="text-xs text-dark-400 mb-1">Register this redirect URI at {p.label}:</p>
                      <div className="flex gap-2">
                        <code className="flex-1 px-2 py-1.5 bg-dark-900 rounded text-xs font-mono text-dark-100 break-all">{redirect}</code>
                        <button
                          onClick={() => { navigator.clipboard.writeText(redirect); setCopiedRedirect(p.id); }}
                          className="px-2 py-1 bg-dark-700 rounded text-xs text-dark-200 shrink-0 hover:bg-dark-600"
                        >{copiedRedirect === p.id ? "Copied" : "Copy"}</button>
                      </div>
                    </div>
                  )}
                  <div className="flex justify-end">
                    <button
                      onClick={async () => {
                        const id = creds.id.trim();
                        const secret = creds.secret.trim();
                        // A client ID alone is the trap worth refusing: the public
                        // branding endpoint lists a provider as soon as its
                        // client_id is non-empty, so a half-save puts a working
                        // button on the login page that dead-ends at the callback
                        // with "OAuth not fully configured" — visible to every
                        // logged-out visitor, diagnosable by nobody.
                        if (id && !secret) {
                          setMessage({ text: `${p.label}: a client secret is required — with only a client ID the button appears on the login page and sign-in fails at the callback`, type: "error" });
                          return;
                        }
                        if (!id && secret && secret !== "********") {
                          setMessage({ text: `${p.label}: a client ID is required to use this secret`, type: "error" });
                          return;
                        }
                        const body: Record<string, string> = { [`oauth_${p.id}_client_id`]: id };
                        // The mask means "unchanged" — the backend skips it. Send
                        // the secret only when it is a real new value, and clear
                        // it explicitly when the ID is being removed, so disabling
                        // a provider does not leave its secret behind.
                        if (!id) {
                          body[`oauth_${p.id}_client_secret`] = "";
                        } else if (secret && secret !== "********") {
                          body[`oauth_${p.id}_client_secret`] = secret;
                        }
                        try {
                          await api.put("/settings", body);
                          setSettings(prev => ({ ...prev, ...body }));
                          if (!id) {
                            setOauthCreds(prev => ({ ...prev, [p.id]: { id: "", secret: "" } }));
                            setMessage({ text: `${p.label} sign-in disabled`, type: "success" });
                          } else {
                            setOauthCreds(prev => ({ ...prev, [p.id]: { ...prev[p.id], secret: "********" } }));
                            setMessage({ text: `${p.label} sign-in configured`, type: "success" });
                          }
                        } catch (e) { setMessage({ text: e instanceof Error ? e.message : "Failed", type: "error" }); }
                      }}
                      className="px-4 py-2 bg-rust-500 text-white rounded-lg text-sm font-medium hover:bg-rust-600 disabled:opacity-50"
                    >{idSet ? "Save" : "Save / Disable"}</button>
                  </div>
                </div>
              );
            })}
          </div>
        </div>
        )}
        </>)}

        {/* Notification Channels */}
        {tab === "channels" && (<>
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
            {/* Escalation policy — attached through its own admin-only endpoint, so it
                applies the moment it is picked rather than waiting on Save below. */}
            <div className="bg-dark-800 rounded-lg border border-dark-600 p-5 space-y-2">
              <label htmlFor="escalation-policy" className="block text-sm font-medium text-dark-100">Escalation Policy</label>
              <select
                id="escalation-policy"
                value={escalationPolicyId}
                disabled={!alertRuleId}
                onChange={async (e) => {
                  const next = e.target.value;
                  const previous = escalationPolicyId;
                  setEscalationPolicyId(next);
                  try {
                    await api.put(`/alert-rules/${alertRuleId}/escalation-policy`, { policy_id: next || null });
                    setMessage({ text: next ? "Escalation policy attached" : "Escalation policy removed", type: "success" });
                  } catch (err) {
                    // Put the picker back where it was — leaving it showing a policy the
                    // rule does not carry is the same lie this release is about.
                    setEscalationPolicyId(previous);
                    setMessage({ text: err instanceof Error ? err.message : "Failed to attach policy", type: "error" });
                  }
                }}
                className="w-full px-3 py-2 bg-dark-900 border border-dark-500 rounded-lg text-sm text-dark-50 outline-none disabled:opacity-50"
              >
                <option value="">No escalation (default cadence)</option>
                {escalationPolicies.map(p => <option key={p.id} value={p.id}>{p.name}</option>)}
              </select>
              <p className="text-xs text-dark-300">
                {!alertRuleId
                  ? "Save your notification settings once to create an alert rule, then choose a policy."
                  : escalationPolicies.length === 0
                    ? "No policies yet — build one on the Alerts page under Escalation."
                    : "Unacknowledged alerts follow this chain instead of the built-in reminder cadence, then keep re-paging its last step every 30 minutes until acknowledged."}
              </p>
            </div>
            {/* Alert Behaviour — one row per type, Record and Notify side by side.
                These are two different switches and were previously two different
                shapes: a checkbox grid for muting, and nothing at all for the
                record columns, which were reachable only through Export/Import
                Config. A single row per type is what makes the difference
                legible — the alternative on the table was a second grid beside
                the first, one meaning "don't page me" and the other "don't
                record it".
                Both columns read positively: ticked = it happens. The mute grid
                this replaces was inverted (ticked = suppressed), which could not
                survive sitting next to a Record column without misreading. */}
            <div className="bg-dark-800 rounded-lg border border-dark-600 p-5 space-y-3">
              <h3 className="text-sm font-medium text-dark-100 font-mono">Alert Behaviour</h3>
              <p className="text-xs text-dark-400">
                <span className="text-dark-200">Record</span> keeps the alert inside the panel — the alert row, the
                bell, and any status-page incident. Switched off, the event is never created at all.{" "}
                <span className="text-dark-200">Notify</span> governs only the external send — Slack, Discord,
                PagerDuty, webhooks. Switched off, the alert is still recorded; you just are not paged for it.
              </p>
              <div className="overflow-x-auto">
                <table className="w-full text-sm">
                  <thead>
                    <tr className="text-xs font-mono uppercase tracking-widest text-dark-400">
                      <th className="text-left font-normal py-2">Type</th>
                      <th className="text-center font-normal py-2 w-24">Record</th>
                      <th className="text-center font-normal py-2 w-24">Notify</th>
                    </tr>
                  </thead>
                  <tbody>
                    {SUPPRESSIBLE_ALERT_TYPES.map(({ key, label }) => {
                      const column = RECORD_COLUMN_BY_TYPE[key];
                      const recorded = column === "alert_gpu" ? gpuAlertEnabled : column ? recordTypes[column] : true;
                      return (
                        <tr key={key} className="border-t border-dark-700">
                          <td className="py-2 text-dark-200">{label}</td>
                          <td className="py-2 text-center">
                            {column ? (
                              <input
                                type="checkbox"
                                aria-label={`Record ${label} alerts`}
                                checked={recorded}
                                onChange={e => {
                                  if (column === "alert_gpu") setGpuAlertEnabled(e.target.checked);
                                  else setRecordTypes({ ...recordTypes, [column]: e.target.checked });
                                }}
                                className="rounded border-dark-500 bg-dark-900 text-rust-500 focus:ring-rust-500"
                              />
                            ) : (
                              <span className="text-dark-500" title="Always recorded — this type has no record switch">&mdash;</span>
                            )}
                          </td>
                          <td className="py-2 text-center">
                            <input
                              type="checkbox"
                              aria-label={`Notify externally for ${label} alerts`}
                              checked={!mutedTypes.includes(key)}
                              onChange={e => setMutedTypes(
                                e.target.checked ? mutedTypes.filter(t => t !== key) : [...mutedTypes, key]
                              )}
                              className="rounded border-dark-500 bg-dark-900 text-rust-500 focus:ring-rust-500"
                            />
                          </td>
                        </tr>
                      );
                    })}
                  </tbody>
                </table>
              </div>
              <p className="text-xs text-dark-500">
                &mdash; means the type is always recorded and has no switch. The three GPU rows share a single
                switch, the same one the GPU card below uses.
              </p>
            </div>
            {/* GPU Alert Thresholds */}
            <div className="bg-dark-800 rounded-lg border border-dark-600 p-5 space-y-4">
              <div className="flex items-center justify-between">
                <div>
                  <h3 className="text-sm font-medium text-dark-100 font-mono">GPU Alert Thresholds</h3>
                  <p className="text-xs text-dark-400 mt-0.5">Configure when GPU alerts fire. Only applies to servers with NVIDIA GPUs. The Enabled box is the same switch as the three GPU rows in the grid above &mdash; one value, shown in both places.</p>
                </div>
                <label className="flex items-center gap-2 text-sm text-dark-200 cursor-pointer">
                  <input type="checkbox" checked={gpuAlertEnabled} onChange={e => setGpuAlertEnabled(e.target.checked)}
                    className="rounded border-dark-500 bg-dark-900 text-rust-500 focus:ring-rust-500" />
                  Enabled
                </label>
              </div>
              {gpuAlertEnabled && (
                <div className="grid grid-cols-1 sm:grid-cols-2 gap-4">
                  <div>
                    <label className="block text-xs font-mono text-dark-300 uppercase tracking-widest mb-1">Utilization threshold</label>
                    <div className="flex items-center gap-2">
                      <input type="range" min="50" max="100" value={gpuUtilThreshold} onChange={e => setGpuUtilThreshold(Number(e.target.value))}
                        className="flex-1 accent-rust-500" />
                      <span className="text-sm text-dark-100 font-mono w-12 text-right">{gpuUtilThreshold}%</span>
                    </div>
                  </div>
                  <div>
                    <label className="block text-xs font-mono text-dark-300 uppercase tracking-widest mb-1">Utilization duration</label>
                    <div className="flex items-center gap-2">
                      <input type="range" min="1" max="30" value={gpuUtilDuration} onChange={e => setGpuUtilDuration(Number(e.target.value))}
                        className="flex-1 accent-rust-500" />
                      <span className="text-sm text-dark-100 font-mono w-16 text-right">{gpuUtilDuration} min</span>
                    </div>
                  </div>
                  <div>
                    <label className="block text-xs font-mono text-dark-300 uppercase tracking-widest mb-1">Temperature threshold</label>
                    <div className="flex items-center gap-2">
                      <input type="range" min="60" max="110" value={gpuTempThreshold} onChange={e => setGpuTempThreshold(Number(e.target.value))}
                        className="flex-1 accent-rust-500" />
                      <span className="text-sm text-dark-100 font-mono w-12 text-right">{gpuTempThreshold}°C</span>
                    </div>
                  </div>
                  <div>
                    <label className="block text-xs font-mono text-dark-300 uppercase tracking-widest mb-1">VRAM threshold</label>
                    <div className="flex items-center gap-2">
                      <input type="range" min="50" max="100" value={gpuVramThreshold} onChange={e => setGpuVramThreshold(Number(e.target.value))}
                        className="flex-1 accent-rust-500" />
                      <span className="text-sm text-dark-100 font-mono w-12 text-right">{gpuVramThreshold}%</span>
                    </div>
                  </div>
                </div>
              )}
            </div>
            <div className="flex justify-end">
              <button
                onClick={async () => {
                  setSaving("notify");
                  setMessage({ text: "", type: "" });
                  try {
                    // Send the field exactly as the operator left it, empty included.
                    // Substituting a null for an empty box made clearing a destination
                    // impossible: the handler COALESCEs a null onto the stored value, so
                    // the old webhook survived, alerts kept being posted to it, and the
                    // save still answered "Notification channels saved". An empty string
                    // is the safe sentinel at both ends — the SSRF validators skip a
                    // blank field rather than rejecting it, and every sender guards on
                    // the value being non-empty before it delivers.
                    await api.put("/alert-rules", {
                      notify_email: notifyEmail,
                      notify_slack_url: notifySlackUrl,
                      notify_discord_url: notifyDiscordUrl,
                      notify_pagerduty_key: notifyPagerdutyKey,
                      muted_types: mutedTypes.join(','),
                      // Explicit booleans, never omitted: upsert_rules COALESCEs
                      // a null onto the stored value, so a missing field would
                      // make switching a type OFF silently impossible.
                      alert_cpu: recordTypes.alert_cpu,
                      alert_memory: recordTypes.alert_memory,
                      alert_disk: recordTypes.alert_disk,
                      alert_offline: recordTypes.alert_offline,
                      alert_backup_failure: recordTypes.alert_backup_failure,
                      alert_ssl_expiry: recordTypes.alert_ssl_expiry,
                      alert_service_health: recordTypes.alert_service_health,
                      alert_gpu: gpuAlertEnabled,
                      gpu_util_threshold: gpuUtilThreshold,
                      gpu_util_duration: gpuUtilDuration,
                      gpu_temp_threshold: gpuTempThreshold,
                      gpu_vram_threshold: gpuVramThreshold,
                    });
                    setMessage({ text: "Notification channels saved", type: "success" });
                    // Re-read so the escalation picker learns the rule id on the first
                    // save, when the row is created rather than updated.
                    await loadNotifyChannels();
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

        {/* An "Additional Settings" card lived here until v2.46.0 with two inputs:
            an email footer "appended to notification emails" and an events webhook
            that "receives POST for site.create, app.deploy, security.scan". Both
            saved successfully and neither was ever read — no code appends the
            footer, and nothing POSTs anywhere. Removed rather than left lying;
            they come back with the code that honours them. */}

        {/* Notification Templates (Gap #70).
            The mirror image of the card removed above: these four keys ARE read
            — services/notifications.rs::format_message looks up
            notif_template_{channel} on every alert and substitutes into it — and
            had no control, so the feature existed only for whoever could reach
            the settings API by hand. */}
        {user?.role === "admin" && (
        <div className="bg-dark-800 rounded-lg border border-dark-500 overflow-hidden mt-4">
          <div className="px-5 py-3 border-b border-dark-600">
            <h3 className="text-xs font-medium text-dark-300 uppercase font-mono tracking-widest">Notification Templates</h3>
            <p className="text-xs text-dark-200 mt-0.5">Custom message format per channel. Leave a field empty to use the built-in format.</p>
          </div>
          <div className="p-5 space-y-4">
            <div className="flex items-center gap-2 flex-wrap">
              <span className="text-xs text-dark-400">Placeholders:</span>
              {NOTIF_PLACEHOLDERS.map(ph => (
                <code key={ph} className="px-1.5 py-0.5 bg-dark-900 rounded text-xs font-mono text-rust-400">{ph}</code>
              ))}
              <span className="text-xs text-dark-400">— anything else is sent literally.</span>
            </div>
            {NOTIF_CHANNELS.map(c => (
              <div key={c.id}>
                <label htmlFor={`notif-tmpl-${c.id}`} className="block text-sm font-medium text-dark-100 mb-1">{c.label}</label>
                <p className="text-xs text-dark-400 mb-1">{c.hint}</p>
                <textarea
                  id={`notif-tmpl-${c.id}`}
                  rows={c.rows}
                  value={notifTemplates[c.id] || ""}
                  onChange={e => setNotifTemplates(prev => ({ ...prev, [c.id]: e.target.value }))}
                  placeholder="Leave empty for the default format"
                  className="w-full px-3 py-2 border border-dark-500 rounded-lg text-sm font-mono focus:ring-2 focus:ring-accent-500 focus:border-accent-500 outline-none"
                />
              </div>
            ))}
            <div className="flex justify-end">
              <button
                onClick={async () => {
                  setSaving("templates");
                  try {
                    const body = Object.fromEntries(
                      NOTIF_CHANNELS.map(c => [`notif_template_${c.id}`, notifTemplates[c.id] || ""])
                    );
                    await api.put("/settings", body);
                    setSettings(prev => ({ ...prev, ...body }));
                    setMessage({ text: "Notification templates saved", type: "success" });
                  } catch (e) {
                    setMessage({ text: e instanceof Error ? e.message : "Failed", type: "error" });
                  } finally {
                    setSaving(null);
                  }
                }}
                disabled={saving === "templates"}
                className="px-4 py-2 bg-rust-500 text-white rounded-lg text-sm font-medium hover:bg-rust-600 disabled:opacity-50"
              >{saving === "templates" ? "Saving..." : "Save Templates"}</button>
            </div>
          </div>
        </div>
        )}
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

        {/* Image Vulnerability Scanning */}
        <ImageScanSettings setMessage={setMessage} />

        {/* SBOM (composition; companion to image scanning) */}
        <SbomSettings setMessage={setMessage} />

        {/* Prometheus metrics endpoint */}
        <PrometheusSettings setMessage={setMessage} />

        {/* ACME contact — rendered BEFORE the profile card, and independent of it:
            a bad contact is one reason the profile card's directory call fails. */}
        <AcmeContactSettings setMessage={setMessage} />

        {/* ACME profile selection — 2026-ready Let's Encrypt */}
        <AcmeSettings setMessage={setMessage} />

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
                    <div className={`w-3 h-3 rounded-full ${health.database ? "bg-rust-500" : "bg-danger-500"}`} />
                    <span className="text-sm text-dark-50">Database</span>
                  </div>
                  <span className={`text-sm font-medium ${health.database ? "text-rust-400" : "text-danger-400"}`}>
                    {health.database ? "Connected" : "Error"}
                  </span>
                </div>
                <div className="flex items-center justify-between">
                  <div className="flex items-center gap-3">
                    <div className={`w-3 h-3 rounded-full ${health.agentOk ? "bg-rust-500" : "bg-danger-500"}`} />
                    <span className="text-sm text-dark-50">Agent</span>
                  </div>
                  <span className={`text-sm font-medium ${health.agentOk ? "text-rust-400" : "text-danger-400"}`}>
                    {health.agentOk ? "Connected" : "Error"}
                  </span>
                </div>
                <div className="flex items-center justify-between">
                  <div className="flex items-center gap-3">
                    <div className="w-3 h-3 rounded-full bg-accent-500" />
                    <span className="text-sm text-dark-50">Uptime</span>
                  </div>
                  <span className="text-sm font-medium text-dark-200 font-mono">{health.uptime}</span>
                </div>
              </div>
            )}
          </div>
        </div>
        </>)}

      </div>
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
  const [status, setStatus] = useState<ServiceStatus | null>(null);
  const [mailStatus, setMailStatus] = useState<{ installed: boolean; running: boolean } | null>(null);
  const [installing, setInstalling] = useState<string | null>(null);
  const [installId, setInstallId] = useState<string | null>(null);
  const [uninstalling, setUninstalling] = useState<string | null>(null);
  const [msg, setMsg] = useState({ text: "", type: "" });
  const [showGuide, setShowGuide] = useState(false);
  const [svcPendingConfirm, setSvcPendingConfirm] = useState<{ service: string; label: string } | null>(null);

  const refreshStatus = () => {
    api.get<ServiceStatus>("/services/install-status")
      .then((d) => setStatus(d))
      .catch(() => {});
    api.get<{ installed: boolean; running: boolean }>("/mail/status")
      .then((d) => setMailStatus(d))
      .catch(() => setMailStatus({ installed: false, running: false }));
  };

  useEffect(refreshStatus, []);

  const [pdnsBackend, setPdnsBackend] = useState<"sqlite" | "pgsql">("sqlite");

  const install = async (service: string, _label: string, body?: Record<string, unknown>) => {
    setInstalling(service);
    setInstallId(null);
    setMsg({ text: "", type: "" });
    try {
      const endpoint = service === "mail" ? "/mail/install" : `/services/install/${service}`;
      const result = await api.post<{ install_id?: string }>(endpoint, body ?? {});
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

  const uninstall = (service: string, label: string) => {
    setSvcPendingConfirm({ service, label });
  };

  const executeUninstall = async () => {
    if (!svcPendingConfirm) return;
    const { service, label } = svcPendingConfirm;
    setSvcPendingConfirm(null);
    setUninstalling(service);
    setMsg({ text: "", type: "" });
    try {
      const endpoint = service === "mail" ? "/mail/uninstall" : `/services/uninstall/${service}`;
      await api.post(endpoint, {});
      setMsg({ text: `${label} uninstalled successfully`, type: "success" });
      refreshStatus();
    } catch (e) {
      setMsg({ text: e instanceof Error ? e.message : "Uninstall failed", type: "error" });
    } finally {
      setUninstalling(null);
    }
  };

  const services = [
    { id: "php", label: "PHP", desc: "PHP-FPM for dynamic websites (WordPress, Laravel, etc.)", field: "php", checkInstalled: (s: ServiceStatus) => s?.php?.installed, checkRunning: (s: ServiceStatus) => s?.php?.running, extra: (s: ServiceStatus) => s?.php?.version ? `v${s.php.version}` : null },
    { id: "certbot", label: "Certbot", desc: "Let's Encrypt SSL certificates with auto-renewal", field: "certbot", checkInstalled: (s: ServiceStatus) => s?.certbot?.installed, checkRunning: () => true, extra: () => null },
    { id: "ufw", label: "UFW Firewall", desc: "Firewall with default rules (SSH, HTTP, HTTPS, mail ports)", field: "ufw", checkInstalled: (s: ServiceStatus) => s?.ufw?.installed, checkRunning: (s: ServiceStatus) => s?.ufw?.active, extra: () => null },
    { id: "fail2ban", label: "Fail2Ban", desc: "Intrusion prevention with SSH, Nginx, Postfix jails", field: "fail2ban", checkInstalled: (s: ServiceStatus) => s?.fail2ban?.installed, checkRunning: (s: ServiceStatus) => s?.fail2ban?.running, extra: () => null },
    { id: "powerdns", label: "PowerDNS", desc: "Self-hosted authoritative DNS server with HTTP API", field: "powerdns", checkInstalled: (s: ServiceStatus) => s?.powerdns?.installed, checkRunning: (s: ServiceStatus) => s?.powerdns?.running, extra: () => null },
    { id: "mail", label: "Mail Server", desc: "Postfix + Dovecot + OpenDKIM for email hosting", field: "mail", checkInstalled: () => mailStatus?.installed ?? null, checkRunning: () => mailStatus?.running ?? false, extra: () => null },
    { id: "redis", label: "Redis", desc: "In-memory cache and data store for PHP applications", field: "redis", checkInstalled: (s: ServiceStatus) => s?.redis?.installed, checkRunning: (s: ServiceStatus) => s?.redis?.running, extra: () => null },
    { id: "nodejs", label: "Node.js", desc: "JavaScript runtime for builds, SSR, and npm packages", field: "nodejs", checkInstalled: (s: ServiceStatus) => s?.nodejs?.installed, checkRunning: () => null, extra: () => null },
    { id: "composer", label: "Composer", desc: "PHP dependency manager for Laravel, Symfony, Drupal", field: "composer", checkInstalled: (s: ServiceStatus) => s?.composer?.installed, checkRunning: () => null, extra: () => null },
    { id: "waf", label: "WAF (ModSecurity)", desc: "Web Application Firewall with OWASP CRS — blocks SQL injection, XSS, and OWASP Top 10", field: "waf", checkInstalled: (s: ServiceStatus) => s?.waf?.installed, checkRunning: () => null, extra: () => null },
    { id: "cloudflared", label: "Cloudflare Tunnel", desc: "Expose sites without port forwarding — zero-trust access via Cloudflare's network", field: "cloudflared", checkInstalled: (s: ServiceStatus) => s?.cloudflared?.installed, checkRunning: (s: ServiceStatus) => s?.cloudflared?.running, extra: () => null },
    { id: "sshpass", label: "sshpass", desc: "Required for password-authenticated SFTP backup destinations — not needed for SSH-key or S3 destinations", field: "sshpass", checkInstalled: (s: ServiceStatus) => s?.sshpass?.installed, checkRunning: () => null, extra: () => null },
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
          <div className={`px-4 py-2 rounded-lg text-sm border ${msg.type === "success" ? "bg-rust-500/10 text-rust-400 border-rust-500/20" : "bg-danger-500/10 text-danger-400 border-danger-500/20"}`}>
            {msg.text}
          </div>
        )}

        {/* Issue #120: this detached uninstall bar rendered here, above the
            service list, while the Uninstall buttons that arm it sit in the rows
            below. It is a portalled dialog now — see components/ConfirmDialog. */}
        {svcPendingConfirm && (
          <ConfirmDialog
            label={`Uninstall ${svcPendingConfirm.label}?`}
            tone="danger"
            onConfirm={executeUninstall}
            onCancel={() => setSvcPendingConfirm(null)}
          />
        )}

        <button
          onClick={() => setShowGuide(!showGuide)}
          className="flex items-center gap-2 text-sm text-accent-400 hover:text-accent-300 transition-colors"
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
            <p><span className="text-dark-100 font-medium">PowerDNS</span> — Installs an authoritative DNS server with your choice of SQLite (no database server required) or PostgreSQL backend. Auto-configures the HTTP API and saves credentials to Settings.</p>
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
                <div className="min-w-0">
                  <div className="flex items-center gap-2">
                    <span className="text-sm font-medium text-dark-50">{svc.label}</span>
                    {extra && <span className="text-[10px] text-dark-300">{extra}</span>}
                    {installed === true && running !== null && (
                      <span className={`w-2 h-2 rounded-full ${running ? "bg-rust-400" : "bg-warn-400"}`} title={running ? "Running" : "Installed but not running"} />
                    )}
                  </div>
                  <p className="text-[10px] text-dark-300 mt-0.5">{svc.desc}</p>
                  {svc.id === "powerdns" && !installed && (
                    <div className="flex items-center gap-1.5 mt-2">
                      <span className="text-[10px] text-dark-400 uppercase tracking-wider">Backend</span>
                      <div className="flex items-center rounded-lg border border-dark-600 overflow-hidden text-[10px] font-medium" title="SQLite needs no database server and is recommended for most setups.">
                        <button
                          type="button"
                          onClick={() => setPdnsBackend("sqlite")}
                          className={`px-2 py-1 ${pdnsBackend === "sqlite" ? "bg-dark-600 text-dark-50" : "bg-dark-800 text-dark-400 hover:text-dark-200"}`}
                        >SQLite</button>
                        <button
                          type="button"
                          onClick={() => setPdnsBackend("pgsql")}
                          className={`px-2 py-1 ${pdnsBackend === "pgsql" ? "bg-dark-600 text-dark-50" : "bg-dark-800 text-dark-400 hover:text-dark-200"}`}
                        >PostgreSQL</button>
                      </div>
                    </div>
                  )}
                </div>
                {installed ? (
                  <div className="flex items-center gap-2 shrink-0 ml-3">
                    <span className="text-[10px] text-dark-300 uppercase tracking-wider">Installed</span>
                    {(
                      <button
                        onClick={() => uninstall(svc.id, svc.label)}
                        disabled={uninstalling !== null || installing !== null}
                        className="px-2.5 py-1 bg-danger-500/10 text-danger-400 border border-danger-500/20 rounded-lg text-[10px] font-medium hover:bg-danger-500/20 disabled:opacity-50"
                      >
                        {uninstalling === svc.id ? "Removing..." : "Uninstall"}
                      </button>
                    )}
                  </div>
                ) : (
                  <button
                    onClick={() => install(svc.id, svc.label, svc.id === "powerdns" ? { backend: pdnsBackend } : undefined)}
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
              className="flex items-center gap-1.5 text-xs text-accent-400 hover:text-accent-300 transition-colors"
            >
              <svg className="w-3.5 h-3.5" fill="none" viewBox="0 0 24 24" stroke="currentColor" strokeWidth={2}>
                <path strokeLinecap="round" strokeLinejoin="round" d="M11.25 11.25l.041-.02a.75.75 0 011.063.852l-.708 2.836a.75.75 0 001.063.853l.041-.021M21 12a9 9 0 11-18 0 9 9 0 0118 0zm-9-3.75h.008v.008H12V8.25z" />
              </svg>
              {showPdnsGuide ? "Hide guide" : "Setup guide"}
            </button>
          </div>

          {showPdnsGuide && (
            <div className="bg-dark-900 border border-dark-500 p-4 space-y-3 text-sm">
              <p className="text-dark-200">The one-click <span className="text-dark-100 font-medium">Install</span> button above does all of this for you — choose <span className="text-dark-100 font-mono">SQLite</span> (no database server needed, recommended for most setups) or <span className="text-dark-100 font-mono">PostgreSQL</span>, and the API URL + key are generated and saved automatically.</p>
              <p className="text-dark-200 font-medium">Prefer to set it up by hand? SQLite backend:</p>
              <pre className="bg-dark-950 border border-dark-600 p-3 text-xs text-dark-100 font-mono overflow-x-auto whitespace-pre">{`# Install PowerDNS with the SQLite backend
apt install pdns-server pdns-backend-sqlite3 sqlite3

# Create the database from the bundled schema
mkdir -p /var/lib/powerdns
sqlite3 /var/lib/powerdns/pdns.sqlite3 < /usr/share/doc/pdns-backend-sqlite3/schema.sqlite3.sql
chown -R pdns:pdns /var/lib/powerdns`}</pre>
              <p className="text-dark-200 font-medium">Configure <span className="text-dark-100 font-mono">/etc/powerdns/pdns.conf</span>:</p>
              <pre className="bg-dark-950 border border-dark-600 p-3 text-xs text-dark-100 font-mono overflow-x-auto whitespace-pre">{`launch=gsqlite3
gsqlite3-database=/var/lib/powerdns/pdns.sqlite3

# Enable HTTP API
api=yes
api-key=your-secret-key-here
webserver=yes
webserver-address=127.0.0.1
webserver-port=8081
webserver-allow-from=127.0.0.1`}</pre>
              <p className="text-xs text-dark-300">On Ubuntu (and anywhere <span className="font-mono text-dark-200">systemd-resolved</span> is running) port 53 is already taken by the stub resolver on <span className="font-mono text-dark-200">127.0.0.53</span>, so PowerDNS's default wildcard bind fails with <span className="font-mono text-dark-200">Address already in use</span> and the service restart-loops. Add a <span className="font-mono text-dark-200">local-address</span> line listing your real IPs — the one-click installer detects this and does it for you:</p>
              <pre className="bg-dark-950 border border-dark-600 p-3 text-xs text-dark-100 font-mono overflow-x-auto whitespace-pre">{`local-address=YOUR.PUBLIC.IP.HERE,127.0.0.1`}</pre>
              <p className="text-xs text-dark-300">Prefer PostgreSQL? DockPanel's database runs in the <span className="font-mono text-dark-200">dockpanel-postgres</span> container, not on localhost — so <span className="font-mono text-dark-200">sudo -u postgres createdb pdns</span> won't work. Use the one-click installer's PostgreSQL option (it creates the <span className="font-mono text-dark-200">pdns</span> database inside the container for you).</p>
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
        {msg.text && <div className={`px-3 py-2 rounded text-xs ${msg.type === "success" ? "bg-rust-500/10 text-rust-400" : "bg-danger-500/10 text-danger-400"}`}>{msg.text}</div>}
        {keys.length === 0 && (
          <p className="text-xs text-dark-400 text-center py-2">No SSH keys configured</p>
        )}
        {keys.map((k) => (
          <div key={k.fingerprint} className="flex items-center justify-between bg-dark-900 border border-dark-500 px-4 py-2">
            <div className="min-w-0">
              <span className="text-xs text-dark-50 font-mono block truncate">{k.comment || k.key}</span>
              <span className="text-[10px] text-dark-300 font-mono">{k.fingerprint}</span>
            </div>
            <button onClick={async () => {
              try { await api.delete(`/ssh-keys/${encodeURIComponent(k.fingerprint)}`); setKeys(keys.filter(x => x.fingerprint !== k.fingerprint)); setMsg({ text: "Key removed", type: "success" }); }
              catch (e) { setMsg({ text: e instanceof Error ? e.message : "Failed to remove key", type: "error" }); }
            }} className="p-1 text-dark-300 hover:text-danger-400 shrink-0 ml-2">
              <svg className="w-4 h-4" fill="none" viewBox="0 0 24 24" stroke="currentColor" strokeWidth={1.5}><path strokeLinecap="round" strokeLinejoin="round" d="M6 18L18 6M6 6l12 12" /></svg>
            </button>
          </div>
        ))}
        <div className="flex gap-2">
          <input type="text" value={newKey} onChange={(e) => setNewKey(e.target.value)} placeholder="ssh-ed25519 AAAA... user@host" className="flex-1 px-3 py-2 border border-dark-500 rounded-lg text-xs font-mono focus:ring-2 focus:ring-accent-500 outline-none" />
          <button disabled={adding || !newKey.startsWith("ssh-")} onClick={async () => {
            setAdding(true); setMsg({ text: "", type: "" });
            try { await api.post("/ssh-keys", { key: newKey }); setNewKey(""); const d = await api.get<{ keys: typeof keys }>("/ssh-keys"); setKeys(d.keys || []); setMsg({ text: "Key added", type: "success" }); }
            catch (e) { setMsg({ text: e instanceof Error ? e.message : "Failed", type: "error" }); }
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
  // Local, and rendered under the toggle on purpose. A page-level banner on a
  // page this long is a message the operator never sees next to the control
  // they clicked — the #120 shape. Keep the answer where the question was.
  const [error, setError] = useState("");

  useEffect(() => {
    api.get<{ installed: boolean; enabled: boolean }>("/auto-updates/status").then(setStatus).catch(() => {});
  }, []);

  const toggle = async () => {
    if (!status) return;
    setToggling(true);
    setError("");
    try {
      await api.post(status.enabled ? "/auto-updates/disable" : "/auto-updates/enable", {});
      setStatus({ ...status, installed: true, enabled: !status.enabled });
    } catch (e) {
      // The switch used to slide back with no explanation, which on a host
      // family where unattended-upgrades is not the mechanism is the ONLY
      // outcome it can have. Silence there reads as "done".
      setError(e instanceof Error ? e.message : "Could not change automatic security updates");
    }
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
        {error && <p className="text-xs text-danger-400 mt-3">{error}</p>}
      </div>
    </div>
  );
}

// ── Image Vulnerability Scanning ────────────────────────────────────────

interface ImageScanSettingsState {
  enabled: boolean;
  on_deploy: boolean;
  deploy_gate: string;
  interval_hours: number;
  installed: boolean;
}

function ImageScanSettings({ setMessage }: { setMessage: (m: { text: string; type: string }) => void }) {
  const [s, setS] = useState<ImageScanSettingsState | null>(null);
  const [loadError, setLoadError] = useState<string | null>(null);
  const [saving, setSaving] = useState(false);
  const [installing, setInstalling] = useState(false);
  const [uninstallConfirm, setUninstallConfirm] = useState(false);
  const [lastSweep, setLastSweep] = useState<{ scanned_at: string; image_count: number } | null>(null);

  const load = () => {
    setLoadError(null);
    api.get<ImageScanSettingsState>("/image-scan/settings")
      .then(setS)
      .catch((e: unknown) => {
        setS(null);
        setLoadError(e instanceof Error ? e.message : "Failed to load settings");
      });
    api.get<{ image: string; scanned_at: string }[]>("/image-scan/recent")
      .then(scans => {
        if (!scans || scans.length === 0) { setLastSweep(null); return; }
        const newest = scans.reduce((acc, x) =>
          new Date(x.scanned_at).getTime() > new Date(acc.scanned_at).getTime() ? x : acc
        );
        setLastSweep({ scanned_at: newest.scanned_at, image_count: scans.length });
      })
      .catch(() => setLastSweep(null));
  };

  useEffect(() => { load(); }, []);

  const update = async (patch: Partial<ImageScanSettingsState>) => {
    if (!s) return;
    const next = { ...s, ...patch };
    setS(next);
    setSaving(true);
    try {
      await api.put("/image-scan/settings", {
        enabled: next.enabled,
        on_deploy: next.on_deploy,
        deploy_gate: next.deploy_gate,
        interval_hours: next.interval_hours,
      });
      setMessage({ text: "Image scan settings saved", type: "success" });
    } catch (e) {
      setMessage({ text: `Save failed: ${(e as Error).message || "unknown"}`, type: "error" });
      load();
    } finally {
      setSaving(false);
    }
  };

  const install = async () => {
    setInstalling(true);
    try {
      await api.post("/image-scan/install", {});
      setMessage({ text: "Scanner installed (grype)", type: "success" });
      load();
    } catch (e) {
      setMessage({ text: `Install failed: ${(e as Error).message || "unknown"}`, type: "error" });
    } finally {
      setInstalling(false);
    }
  };

  const uninstall = async () => {
    setInstalling(true);
    try {
      await api.post("/image-scan/uninstall", {});
      setMessage({ text: "Scanner removed", type: "success" });
      load();
    } catch (e) {
      setMessage({ text: `Uninstall failed: ${(e as Error).message || "unknown"}`, type: "error" });
    } finally {
      setInstalling(false);
      setUninstallConfirm(false);
    }
  };

  if (!s) {
    return (
      <div className="bg-dark-800 rounded-lg border border-dark-500 overflow-hidden">
        <div className="px-5 py-3 border-b border-dark-600">
          <h3 className="text-xs font-medium text-dark-300 uppercase font-mono tracking-widest">Image Vulnerability Scanning</h3>
        </div>
        {loadError ? (
          <div className="p-5 flex items-center justify-between gap-3">
            <p className="text-sm text-danger-400">Could not load scanner settings: {loadError}</p>
            <button type="button" onClick={load} className="px-3 py-1.5 bg-dark-600 text-dark-50 rounded-lg text-xs font-medium hover:bg-dark-500 shrink-0">Retry</button>
          </div>
        ) : (
          <div className="p-5 text-sm text-dark-300">Loading...</div>
        )}
      </div>
    );
  }

  return (
    <div className="bg-dark-800 rounded-lg border border-dark-500 overflow-hidden">
      <div className="px-5 py-3 border-b border-dark-600">
        <h3 className="text-xs font-medium text-dark-300 uppercase font-mono tracking-widest">Image Vulnerability Scanning</h3>
        <p className="text-xs text-dark-200 mt-0.5">Scan deployed Docker images for known CVEs (grype). Per-app badges appear on the Apps page.</p>
      </div>
      <div className="p-5 space-y-4">
        <div className="flex items-center justify-between border border-dark-600 bg-dark-900/50 rounded p-4">
          <div>
            <div className="text-sm font-medium text-dark-50 flex items-center gap-2">
              Scanner (grype)
              <span
                className={`w-2 h-2 rounded-full ${s.installed ? "bg-rust-400" : "bg-dark-500"}`}
                role="img"
                aria-label={s.installed ? "Scanner installed" : "Scanner not installed"}
                title={s.installed ? "Installed" : "Not installed"}
              />
            </div>
            <p className="text-[10px] text-dark-300 mt-0.5">~70MB binary + vulnerability database. Required for any scanning.</p>
            {s.installed && (
              <p className="text-[10px] text-dark-300 mt-1">
                {lastSweep
                  ? <>Last scan <span title={new Date(lastSweep.scanned_at).toLocaleString()} className="text-dark-200">{timeAgo(lastSweep.scanned_at)}</span> · {lastSweep.image_count} image{lastSweep.image_count === 1 ? "" : "s"} on file</>
                  : <>No scans recorded yet.</>}
              </p>
            )}
          </div>
          {s.installed ? (
            uninstallConfirm ? (
              <div className="flex gap-2">
                <button type="button" onClick={uninstall} disabled={installing} className="px-2.5 py-1 bg-danger-500 text-white rounded-lg text-[10px] font-medium hover:bg-danger-600 disabled:opacity-50">
                  {installing ? "Removing..." : "Confirm uninstall"}
                </button>
                <button type="button" onClick={() => setUninstallConfirm(false)} className="px-2.5 py-1 bg-dark-600 text-dark-200 rounded-lg text-[10px] font-medium hover:bg-dark-500">
                  Cancel
                </button>
              </div>
            ) : (
              <button type="button" onClick={() => setUninstallConfirm(true)} aria-label="Uninstall image scanner" className="px-2.5 py-1 bg-danger-500/10 text-danger-400 border border-danger-500/20 rounded-lg text-[10px] font-medium hover:bg-danger-500/20">
                Uninstall
              </button>
            )
          ) : (
            <button type="button" onClick={install} disabled={installing} className="px-3 py-1.5 bg-rust-500 text-white rounded-lg text-xs font-medium hover:bg-rust-600 disabled:opacity-50">
              {installing ? "Installing..." : "Install Scanner"}
            </button>
          )}
        </div>

        {!s.installed && (
          <p className="text-[11px] text-dark-300 -mt-2 ml-1">Install the scanner above to enable scheduled scans and the deploy gate.</p>
        )}

        <div className="grid grid-cols-1 sm:grid-cols-2 gap-3">
          <label className="flex items-start gap-3 border border-dark-600 bg-dark-900/50 rounded p-4 cursor-pointer hover:bg-dark-900">
            <input
              type="checkbox"
              checked={s.enabled}
              disabled={!s.installed || saving}
              onChange={e => update({ enabled: e.target.checked })}
              className="mt-1"
            />
            <div>
              <div className="text-sm font-medium text-dark-50">Enable scheduled scans</div>
              <p className="text-[10px] text-dark-300 mt-0.5">Background sweep rescans every running app's image at the interval below.</p>
            </div>
          </label>

          <label className="flex items-start gap-3 border border-dark-600 bg-dark-900/50 rounded p-4 cursor-pointer hover:bg-dark-900">
            <input
              type="checkbox"
              checked={s.on_deploy}
              disabled={!s.installed || saving}
              onChange={e => update({ on_deploy: e.target.checked })}
              className="mt-1"
            />
            <div>
              <div className="text-sm font-medium text-dark-50">Gate deploys on scan results</div>
              <p className="text-[10px] text-dark-300 mt-0.5">Refuse an image whose last scan exceeds the threshold below &mdash; on deploys, image changes, compose and stacks. Updating an app is exempt: it re-pulls the same reference, so the scan on file describes the image being replaced.</p>
            </div>
          </label>
        </div>

        <div className="grid grid-cols-1 sm:grid-cols-2 gap-3">
          <div className="border border-dark-600 bg-dark-900/50 rounded p-4">
            <label className="block text-xs font-mono text-dark-300 uppercase tracking-widest mb-2">Deploy gate threshold</label>
            <select
              value={s.deploy_gate}
              disabled={saving}
              onChange={e => update({ deploy_gate: e.target.value })}
              className="w-full px-3 py-2 bg-dark-800 border border-dark-500 rounded text-sm text-dark-50"
            >
              <option value="none">None — never block</option>
              <option value="critical">Critical only</option>
              <option value="high">High or Critical</option>
              <option value="medium">Medium, High, or Critical</option>
            </select>
            <p className="text-[10px] text-dark-300 mt-2">Only enforced when "Gate deploys" is on.</p>
          </div>

          <div className="border border-dark-600 bg-dark-900/50 rounded p-4">
            <label className="block text-xs font-mono text-dark-300 uppercase tracking-widest mb-2">Rescan interval (hours)</label>
            <input
              type="number"
              min={1}
              max={720}
              value={s.interval_hours}
              disabled={saving}
              onChange={e => {
                const v = parseInt(e.target.value, 10);
                if (!Number.isNaN(v)) update({ interval_hours: v });
              }}
              className="w-full px-3 py-2 bg-dark-800 border border-dark-500 rounded text-sm text-dark-50 font-mono"
            />
            <p className="text-[10px] text-dark-300 mt-2">Background sweep skips images scanned within this window.</p>
          </div>
        </div>
      </div>
    </div>
  );
}

// ── SBOM (syft) ─────────────────────────────────────────────────────────

interface SbomSettingsState {
  installed: boolean;
}

function SbomSettings({ setMessage }: { setMessage: (m: { text: string; type: string }) => void }) {
  const [s, setS] = useState<SbomSettingsState | null>(null);
  const [loadError, setLoadError] = useState<string | null>(null);
  const [installing, setInstalling] = useState(false);
  const [uninstallConfirm, setUninstallConfirm] = useState(false);

  const load = () => {
    setLoadError(null);
    api.get<SbomSettingsState>("/sbom/settings")
      .then(setS)
      .catch((e: unknown) => {
        setS(null);
        setLoadError(e instanceof Error ? e.message : "Failed to load settings");
      });
  };

  useEffect(() => { load(); }, []);

  const install = async () => {
    setInstalling(true);
    try {
      await api.post("/sbom/install", {});
      setMessage({ text: "SBOM generator installed (syft)", type: "success" });
      load();
    } catch (e) {
      setMessage({ text: `Install failed: ${(e as Error).message || "unknown"}`, type: "error" });
    } finally {
      setInstalling(false);
    }
  };

  const uninstall = async () => {
    setInstalling(true);
    try {
      await api.post("/sbom/uninstall", {});
      setMessage({ text: "SBOM generator removed", type: "success" });
      load();
    } catch (e) {
      setMessage({ text: `Uninstall failed: ${(e as Error).message || "unknown"}`, type: "error" });
    } finally {
      setInstalling(false);
      setUninstallConfirm(false);
    }
  };

  if (!s) {
    return (
      <div className="bg-dark-800 rounded-lg border border-dark-500 overflow-hidden">
        <div className="px-5 py-3 border-b border-dark-600">
          <h3 className="text-xs font-medium text-dark-300 uppercase font-mono tracking-widest">SBOM Generation</h3>
        </div>
        {loadError ? (
          <div className="p-5 flex items-center justify-between gap-3">
            <p className="text-sm text-danger-400">Could not load SBOM settings: {loadError}</p>
            <button type="button" onClick={load} className="px-3 py-1.5 bg-dark-600 text-dark-50 rounded-lg text-xs font-medium hover:bg-dark-500 shrink-0">Retry</button>
          </div>
        ) : (
          <div className="p-5 text-sm text-dark-300">Loading...</div>
        )}
      </div>
    );
  }

  return (
    <div className="bg-dark-800 rounded-lg border border-dark-500 overflow-hidden">
      <div className="px-5 py-3 border-b border-dark-600">
        <h3 className="text-xs font-medium text-dark-300 uppercase font-mono tracking-widest">SBOM Generation</h3>
        <p className="text-xs text-dark-200 mt-0.5">Generate SPDX 2.3 SBOMs for deployed images on demand (syft). Use the "Download SBOM" button on any app's scan drawer.</p>
        <p className="text-[10px] text-dark-300 mt-1 italic">On-demand only — no schedule, no deploy gate. Install or uninstall is the only configuration.</p>
      </div>
      <div className="p-5 space-y-4">
        <div className="flex items-center justify-between border border-dark-600 bg-dark-900/50 rounded p-4">
          <div>
            <div className="text-sm font-medium text-dark-50 flex items-center gap-2">
              Generator (syft)
              <span
                className={`w-2 h-2 rounded-full ${s.installed ? "bg-rust-400" : "bg-dark-500"}`}
                role="img"
                aria-label={s.installed ? "Generator installed" : "Generator not installed"}
                title={s.installed ? "Installed" : "Not installed"}
              />
            </div>
            <p className="text-[10px] text-dark-300 mt-0.5">~80MB binary. Required to generate SBOMs from container images.</p>
          </div>
          {s.installed ? (
            uninstallConfirm ? (
              <div className="flex gap-2">
                <button type="button" onClick={uninstall} disabled={installing} className="px-2.5 py-1 bg-danger-500 text-white rounded-lg text-[10px] font-medium hover:bg-danger-600 disabled:opacity-50">
                  {installing ? "Removing..." : "Confirm uninstall"}
                </button>
                <button type="button" onClick={() => setUninstallConfirm(false)} className="px-2.5 py-1 bg-dark-600 text-dark-200 rounded-lg text-[10px] font-medium hover:bg-dark-500">
                  Cancel
                </button>
              </div>
            ) : (
              <button type="button" onClick={() => setUninstallConfirm(true)} aria-label="Uninstall SBOM generator" className="px-2.5 py-1 bg-danger-500/10 text-danger-400 border border-danger-500/20 rounded-lg text-[10px] font-medium hover:bg-danger-500/20">
                Uninstall
              </button>
            )
          ) : (
            <button type="button" onClick={install} disabled={installing} className="px-3 py-1.5 bg-rust-500 text-white rounded-lg text-xs font-medium hover:bg-rust-600 disabled:opacity-50">
              {installing ? "Installing..." : "Install Generator"}
            </button>
          )}
        </div>
      </div>
    </div>
  );
}

// ── Prometheus scrape endpoint ──────────────────────────────────────────

interface PromSettingsState {
  enabled: boolean;
  token_configured: boolean;
  token_prefix: string | null;
}

function PrometheusSettings({ setMessage }: { setMessage: (m: { text: string; type: string }) => void }) {
  const [s, setS] = useState<PromSettingsState | null>(null);
  const [saving, setSaving] = useState(false);
  const [newToken, setNewToken] = useState<string | null>(null);
  const [rotateConfirm, setRotateConfirm] = useState(false);

  const load = () => {
    api.get<PromSettingsState>("/prometheus/settings")
      .then(setS)
      .catch(() => setS(null));
  };

  useEffect(() => { load(); }, []);

  const save = async (enabled: boolean, rotate: boolean) => {
    setSaving(true);
    try {
      const res = await api.post<{ token: string | null; message: string }>("/prometheus/settings", {
        enabled,
        rotate_token: rotate,
      });
      if (res.token) setNewToken(res.token);
      setMessage({ text: res.message, type: "success" });
      load();
    } catch (e) {
      setMessage({ text: `Failed: ${(e as Error).message || "unknown"}`, type: "error" });
    } finally {
      setSaving(false);
      setRotateConfirm(false);
    }
  };

  const copy = (text: string) => {
    navigator.clipboard.writeText(text).then(
      () => setMessage({ text: "Copied to clipboard", type: "success" }),
      () => setMessage({ text: "Copy failed", type: "error" }),
    );
  };

  if (!s) {
    return (
      <div className="bg-dark-800 rounded-lg border border-dark-500 overflow-hidden">
        <div className="px-5 py-3 border-b border-dark-600">
          <h3 className="text-xs font-medium text-dark-300 uppercase font-mono tracking-widest">Prometheus Metrics</h3>
        </div>
        <div className="p-5 text-sm text-dark-300">Loading...</div>
      </div>
    );
  }

  const scrapeUrl = `${window.location.origin}/api/metrics`;
  const scrapeConfig = `scrape_configs:
  - job_name: 'dockpanel'
    metrics_path: /api/metrics
    scheme: ${window.location.protocol.replace(":", "")}
    bearer_token: ${newToken ?? "<your-scrape-token>"}
    static_configs:
      - targets: ['${window.location.host}']`;

  return (
    <div className="bg-dark-800 rounded-lg border border-dark-500 overflow-hidden">
      <div className="px-5 py-3 border-b border-dark-600">
        <h3 className="text-xs font-medium text-dark-300 uppercase font-mono tracking-widest">Prometheus Metrics</h3>
        <p className="text-xs text-dark-200 mt-0.5">
          Expose CPU, memory, disk, GPU, sites, and alerts in Prometheus exposition format at <span className="font-mono text-dark-50">/api/metrics</span> for external monitoring stacks to scrape.
        </p>
      </div>
      <div className="p-5 space-y-4">
        <div className="flex items-center justify-between border border-dark-600 bg-dark-900/50 rounded p-4">
          <div>
            <div className="text-sm font-medium text-dark-50 flex items-center gap-2">
              Scrape endpoint
              <span className={`w-2 h-2 rounded-full ${s.enabled ? "bg-rust-400" : "bg-dark-500"}`} title={s.enabled ? "Enabled" : "Disabled"} />
            </div>
            <p className="text-[10px] text-dark-300 mt-0.5">
              {s.enabled
                ? "Active. Scrapers need the bearer token below."
                : "Disabled. When disabled, /api/metrics returns 404 (hides the endpoint)."}
            </p>
          </div>
          <button
            onClick={() => save(!s.enabled, false)}
            disabled={saving}
            className={`px-3 py-1.5 text-xs font-medium rounded-md disabled:opacity-50 ${
              s.enabled
                ? "bg-danger-500/10 text-danger-400 border border-danger-500/20 hover:bg-danger-500/20"
                : "bg-rust-500 text-white hover:bg-rust-600"
            }`}
          >
            {saving ? "..." : s.enabled ? "Disable" : "Enable"}
          </button>
        </div>

        {s.enabled && (
          <>
            <div className="border border-dark-600 bg-dark-900/50 rounded p-4 space-y-3">
              <div className="flex items-center justify-between">
                <div>
                  <div className="text-sm font-medium text-dark-50">Scrape token</div>
                  <p className="text-[10px] text-dark-300 mt-0.5">
                    {s.token_configured
                      ? <>Active token starts with <span className="font-mono text-dark-50">{s.token_prefix ?? "dpms_…"}</span>. Rotate invalidates the old one immediately.</>
                      : "No token configured."}
                  </p>
                </div>
                {rotateConfirm ? (
                  <div className="flex gap-2">
                    <button onClick={() => save(s.enabled, true)} disabled={saving} className="px-3 py-1.5 bg-danger-500 text-white text-xs font-bold uppercase tracking-wider hover:bg-danger-600 disabled:opacity-50">
                      {saving ? "..." : "Confirm Rotate"}
                    </button>
                    <button onClick={() => setRotateConfirm(false)} className="px-3 py-1.5 bg-dark-600 text-dark-200 text-xs font-bold uppercase tracking-wider hover:bg-dark-500">
                      Cancel
                    </button>
                  </div>
                ) : (
                  <button onClick={() => setRotateConfirm(true)} className="px-2.5 py-1 bg-dark-600 text-dark-100 border border-dark-500 rounded-lg text-[10px] font-medium hover:bg-dark-500">
                    Rotate
                  </button>
                )}
              </div>

              {newToken && (
                <div className="border border-warn-500/30 bg-warn-500/5 rounded p-3">
                  <div className="text-[10px] font-medium text-warn-400 uppercase tracking-wider mb-1">Save this token — it won't be shown again</div>
                  <div className="flex items-center gap-2">
                    <code className="flex-1 text-xs font-mono text-dark-50 break-all">{newToken}</code>
                    <button
                      onClick={() => copy(newToken)}
                      className="px-2 py-1 bg-dark-700 border border-dark-500 text-dark-100 rounded text-[10px] hover:bg-dark-600 shrink-0"
                    >
                      Copy
                    </button>
                  </div>
                </div>
              )}
            </div>

            <div className="border border-dark-600 bg-dark-900/50 rounded p-4 space-y-2">
              <div className="flex items-center justify-between">
                <div className="text-sm font-medium text-dark-50">Endpoint URL</div>
                <button
                  onClick={() => copy(scrapeUrl)}
                  className="px-2 py-1 bg-dark-700 border border-dark-500 text-dark-100 rounded text-[10px] hover:bg-dark-600"
                >
                  Copy URL
                </button>
              </div>
              <code className="block text-xs font-mono text-dark-100 break-all">{scrapeUrl}</code>
            </div>

            <div className="border border-dark-600 bg-dark-900/50 rounded p-4 space-y-2">
              <div className="flex items-center justify-between">
                <div className="text-sm font-medium text-dark-50">Prometheus scrape config</div>
                <button
                  onClick={() => copy(scrapeConfig)}
                  className="px-2 py-1 bg-dark-700 border border-dark-500 text-dark-100 rounded text-[10px] hover:bg-dark-600"
                >
                  Copy YAML
                </button>
              </div>
              <pre className="text-[11px] font-mono text-dark-100 whitespace-pre-wrap break-all">{scrapeConfig}</pre>
              <p className="text-[10px] text-dark-300">
                Drop this block under <span className="font-mono">scrape_configs:</span> in your <span className="font-mono">prometheus.yml</span>.
              </p>
            </div>
          </>
        )}
      </div>
    </div>
  );
}

// ── ACME contact address ──────────────────────────────────────────────
//
// The panel hands an email to Let's Encrypt as the account contact. Until
// v2.28.0 that was silently the operator's panel login address, unvalidated —
// so registering the panel as e.g. admin@box.test made the CA refuse the
// contact and EVERY certificate fail, with no reason in the UI and none in
// journalctl either. This card makes the address visible, says plainly whether
// it works, and lets an admin set a usable one without changing their login.

interface AcmeContactResp {
  contact_email: string | null;
  login_email: string;
  login_email_usable: boolean;
  effective_contact: string | null;
  contact_problem: string | null;
}

function AcmeContactSettings({ setMessage }: { setMessage: (m: { text: string; type: string }) => void }) {
  const [s, setS] = useState<AcmeContactResp | null>(null);
  const [draft, setDraft] = useState("");
  const [saving, setSaving] = useState(false);
  const [loadError, setLoadError] = useState<string | null>(null);

  const load = () => {
    api.get<AcmeContactResp>("/ssl/contact-email")
      .then((r) => { setS(r); setDraft(r.contact_email ?? ""); setLoadError(null); })
      .catch((e) => setLoadError((e as Error).message || "failed to load"));
  };

  useEffect(load, []);

  const save = async (email: string | null) => {
    setSaving(true);
    try {
      await api.post("/ssl/contact-email", { email });
      load();
      setMessage({
        text: email ? `ACME contact address set to ${email}` : "ACME contact address cleared",
        type: "success",
      });
    } catch (e) {
      setMessage({ text: `Failed: ${(e as Error).message || "unknown"}`, type: "error" });
    } finally {
      setSaving(false);
    }
  };

  return (
    <div className="bg-dark-800 rounded-lg border border-dark-500 overflow-hidden">
      <div className="px-5 py-3 border-b border-dark-600">
        <h3 className="text-xs font-medium text-dark-300 uppercase font-mono tracking-widest">ACME Contact</h3>
        <p className="text-xs text-dark-200 mt-0.5">
          The address DockPanel registers with Let's Encrypt. Certificates cannot be issued without one the CA accepts.
        </p>
      </div>
      <div className="p-5 space-y-3">
        {loadError && (
          <div role="alert" className="text-sm text-danger-400">Couldn't load the ACME contact ({loadError}).</div>
        )}
        {!s && !loadError && <div className="text-sm text-dark-300">Loading...</div>}
        {s && (
          <>
            {s.contact_problem ? (
              <div role="alert" className="border border-danger-500/30 bg-danger-500/10 rounded p-4">
                <div className="text-sm font-medium text-danger-400 mb-1">SSL cannot be issued right now</div>
                <p className="text-xs text-danger-400/90">{s.contact_problem}</p>
              </div>
            ) : (
              <div className="border border-dark-600 bg-dark-900/50 rounded p-4">
                <div className="text-sm font-medium text-dark-50 mb-1">Certificates are requested as</div>
                <p className="text-xs font-mono text-rust-400">{s.effective_contact}</p>
                <p className="text-[10px] text-dark-300 mt-2">
                  {s.contact_email
                    ? "From the panel-wide contact address below."
                    : <>Your login address (<span className="font-mono">{s.login_email}</span>). Set an override below to use a different one.</>}
                </p>
              </div>
            )}

            {!s.login_email_usable && (
              <p className="text-xs text-warn-400">
                Your login address <span className="font-mono">{s.login_email}</span> can't be used as a Let's Encrypt
                contact, so the panel-wide address below is required.
              </p>
            )}

            <div className="border border-dark-600 bg-dark-900/50 rounded p-4">
              <label htmlFor="acme-contact" className="block text-sm font-medium text-dark-50 mb-2">
                Panel-wide contact address
              </label>
              <div className="flex items-center gap-2">
                <input
                  id="acme-contact"
                  type="email"
                  value={draft}
                  onChange={(e) => setDraft(e.target.value)}
                  placeholder={s.login_email_usable ? s.login_email : "you@yourdomain.com"}
                  className="flex-1 bg-dark-900 border border-dark-500 text-dark-50 rounded px-3 py-1.5 text-sm disabled:opacity-50"
                  disabled={saving}
                />
                <button
                  type="button"
                  onClick={() => save(draft.trim() || null)}
                  disabled={saving || draft.trim() === (s.contact_email ?? "")}
                  className="px-3 py-1.5 bg-rust-500 text-white rounded text-sm font-medium hover:bg-rust-600 disabled:opacity-50"
                >
                  {saving ? "Saving..." : "Save"}
                </button>
                {s.contact_email && (
                  <button
                    type="button"
                    onClick={() => { setDraft(""); save(null); }}
                    disabled={saving}
                    className="px-3 py-1.5 bg-dark-700 text-dark-100 rounded text-sm font-medium hover:bg-dark-600 disabled:opacity-50"
                  >
                    Clear
                  </button>
                )}
              </div>
              <p className="text-[10px] text-dark-300 mt-2">
                Leave empty to use each user's own login address. Reserved domains
                (<span className="font-mono">.test</span>, <span className="font-mono">.local</span>,
                {" "}<span className="font-mono">.internal</span>, …) are rejected — Let's Encrypt refuses them.
              </p>
            </div>
          </>
        )}
      </div>
    </div>
  );
}

// ── ACME profile selection ────────────────────────────────────────────
//
// Lets the admin pick the default Let's Encrypt profile new certs use.
// Maps to RFC 8555 + RFC 9773 "profiles" extension. LE currently exposes:
//   classic     — 90-day certs, default
//   tlsserver   — 90-day today; flips to 45-day on 2026-05-13 (opt-in)
//   shortlived  — ~6-day certs, for the highest-automation subscribers
//
// When the CA doesn't advertise profiles, the card hides itself.

interface ProfileMeta { name: string; description: string }
interface AcmeProfilesResp { profiles: ProfileMeta[]; default: string | null }

function AcmeSettings({ setMessage }: { setMessage: (m: { text: string; type: string }) => void }) {
  const [s, setS] = useState<AcmeProfilesResp | null>(null);
  const [saving, setSaving] = useState(false);
  const [loadError, setLoadError] = useState<string | null>(null);

  useEffect(() => {
    api.get<AcmeProfilesResp>("/ssl/profiles")
      .then((r) => { setS(r); setLoadError(null); })
      .catch((e) => setLoadError((e as Error).message || "failed to load"));
  }, []);

  const setProfile = async (profile: string | null) => {
    setSaving(true);
    try {
      await api.post("/ssl/default-profile", { profile });
      setS((prev) => prev ? { ...prev, default: profile } : prev);
      setMessage({ text: profile ? `Default ACME profile: ${profile}` : "Default ACME profile cleared", type: "success" });
    } catch (e) {
      setMessage({ text: `Failed: ${(e as Error).message || "unknown"}`, type: "error" });
    } finally {
      setSaving(false);
    }
  };

  if (loadError) {
    return (
      <div className="bg-dark-800 rounded-lg border border-dark-500 overflow-hidden">
        <div className="px-5 py-3 border-b border-dark-600">
          <h3 className="text-xs font-medium text-dark-300 uppercase font-mono tracking-widest">ACME Profile</h3>
        </div>
        <div className="p-5 text-sm text-dark-300">
          Couldn't reach the ACME directory ({loadError}). The CA's profile list is only available once an admin has an ACME account — this loads after your first SSL issuance.
        </div>
      </div>
    );
  }

  if (!s) {
    return (
      <div className="bg-dark-800 rounded-lg border border-dark-500 overflow-hidden">
        <div className="px-5 py-3 border-b border-dark-600">
          <h3 className="text-xs font-medium text-dark-300 uppercase font-mono tracking-widest">ACME Profile</h3>
        </div>
        <div className="p-5 text-sm text-dark-300">Loading...</div>
      </div>
    );
  }

  if (s.profiles.length === 0) {
    return (
      <div className="bg-dark-800 rounded-lg border border-dark-500 overflow-hidden">
        <div className="px-5 py-3 border-b border-dark-600">
          <h3 className="text-xs font-medium text-dark-300 uppercase font-mono tracking-widest">ACME Profile</h3>
        </div>
        <div className="p-5 text-sm text-dark-300">
          The configured CA doesn't advertise the ACME profiles extension — DockPanel will request its default profile.
        </div>
      </div>
    );
  }

  return (
    <div className="bg-dark-800 rounded-lg border border-dark-500 overflow-hidden">
      <div className="px-5 py-3 border-b border-dark-600">
        <h3 className="text-xs font-medium text-dark-300 uppercase font-mono tracking-widest">ACME Profile</h3>
        <p className="text-xs text-dark-200 mt-0.5">
          Default profile for new Let's Encrypt certificates. Renewal uses the CA's ARI (RFC 9773) hints, falling back to a profile-aware threshold.
        </p>
      </div>
      <div className="p-5 space-y-3">
        <div className="border border-dark-600 bg-dark-900/50 rounded p-4">
          <div className="text-sm font-medium text-dark-50 mb-2">Default for new certificates</div>
          <div className="flex items-center gap-2">
            <select
              value={s.default ?? ""}
              onChange={(e) => setProfile(e.target.value || null)}
              disabled={saving}
              className="flex-1 bg-dark-900 border border-dark-500 text-dark-50 rounded px-3 py-1.5 text-sm disabled:opacity-50"
            >
              <option value="">CA default</option>
              {s.profiles.map((p) => (
                <option key={p.name} value={p.name}>{p.name}</option>
              ))}
            </select>
          </div>
          <p className="text-[10px] text-dark-300 mt-2">
            "CA default" lets Let's Encrypt pick — today that's <span className="font-mono">classic</span> (90-day).
            Existing certs keep their current profile until renewal.
          </p>
        </div>
        <div className="border border-dark-600 bg-dark-900/50 rounded p-4">
          <div className="text-sm font-medium text-dark-50 mb-2">Available profiles</div>
          <div className="space-y-2">
            {s.profiles.map((p) => (
              <div key={p.name} className="flex items-start gap-2 text-xs">
                <span className="font-mono text-rust-400 min-w-20 shrink-0">{p.name}</span>
                <span className="text-dark-200">{p.description}</span>
              </div>
            ))}
          </div>
        </div>
      </div>
    </div>
  );
}

interface ReencryptSubject {
  subject: string;
  examined: number;
  rewritten: number;
  already_current: number;
  unreadable: number;
  raced: number;
}

interface ReencryptResult {
  examined: number;
  rewritten: number;
  unreadable: number;
  raced: number;
  subjects: ReencryptSubject[];
  covered_modules: string[];
}

/// The operator half of SECRETS_ENCRYPTION_KEY.
///
/// Shipping the endpoint without this card would have reproduced the exact
/// shape this session spent its verification budget criticising elsewhere in
/// the panel: a complete, authz-scoped backend reachable only by hand-crafted
/// HTTP, with nothing in the UI able to run it.
function CredentialEncryptionCard({ setMessage }: { setMessage: (m: { text: string; type: string }) => void }) {
  const [running, setRunning] = useState(false);
  const [confirming, setConfirming] = useState(false);
  const [result, setResult] = useState<ReencryptResult | null>(null);

  const run = async () => {
    setConfirming(false);
    setRunning(true);
    try {
      const r = await api.post<ReencryptResult>("/settings/credentials/reencrypt", {});
      setResult(r);
      if (r.unreadable > 0) {
        setMessage({
          text: `${r.rewritten} re-encrypted, but ${r.unreadable} value(s) could not be read and were left untouched`,
          type: "error",
        });
      } else if (r.raced > 0) {
        setMessage({
          text: `${r.rewritten} re-encrypted, but ${r.raced} value(s) changed while this ran and were safely skipped — run it again to pick them up`,
          type: "error",
        });
      } else if (r.rewritten === 0) {
        setMessage({ text: "Everything is already under the current key — nothing to do", type: "success" });
      } else {
        setMessage({ text: `${r.rewritten} credential(s) re-encrypted under the current key`, type: "success" });
      }
    } catch (e) {
      setMessage({ text: e instanceof Error ? e.message : "Re-encryption failed", type: "error" });
    } finally {
      setRunning(false);
    }
  };

  return (
    <div className="bg-dark-800 rounded-lg border border-dark-500 overflow-hidden mt-4">
      <div className="px-5 py-3 border-b border-dark-600">
        <h3 className="text-xs font-medium text-dark-300 uppercase font-mono tracking-widest">Credential Encryption</h3>
        <p className="text-xs text-dark-200 mt-0.5">Rewrite stored credentials under the current encryption key</p>
      </div>
      <div className="px-5 py-4 space-y-3">
        <p className="text-xs text-dark-300">
          Stored credentials and Secrets Manager values are encrypted with a key derived from{" "}
          <code className="text-dark-100">JWT_SECRET</code>, or from{" "}
          <code className="text-dark-100">SECRETS_ENCRYPTION_KEY</code> when you set one. After changing
          either, run this once so every value is rewritten under the new key — until you do, the panel
          is reading old values through a fallback and the previous key still has to be derivable.
        </p>
        <p className="text-xs text-dark-400">
          Safe to re-run. A value the panel cannot read is reported and left untouched rather than
          overwritten.
        </p>

        {!confirming ? (
          <button
            type="button"
            onClick={() => setConfirming(true)}
            disabled={running}
            className="px-4 py-2 bg-rust-500 text-white rounded-lg text-sm font-medium hover:bg-rust-600 disabled:opacity-50"
          >
            {running ? "Re-encrypting…" : "Re-encrypt credentials"}
          </button>
        ) : (
          <div className="flex items-center gap-2">
            <span className="text-xs text-dark-200">Rewrite every stored credential now?</span>
            <button
              type="button"
              onClick={run}
              className="px-3 py-1.5 bg-rust-500 text-white rounded-lg text-xs font-medium hover:bg-rust-600"
            >
              Yes, re-encrypt
            </button>
            <button
              type="button"
              onClick={() => setConfirming(false)}
              className="px-3 py-1.5 bg-dark-700 text-dark-200 rounded-lg text-xs font-medium hover:bg-dark-600"
            >
              Cancel
            </button>
          </div>
        )}

        {result && (
          <div className="mt-2 border border-dark-600 rounded-lg overflow-hidden">
            <table className="w-full text-xs">
              <thead className="bg-dark-900 text-dark-400">
                <tr>
                  <th className="text-left px-3 py-2 font-medium">Store</th>
                  <th className="text-right px-3 py-2 font-medium">Examined</th>
                  <th className="text-right px-3 py-2 font-medium">Rewritten</th>
                  <th className="text-right px-3 py-2 font-medium">Already current</th>
                  <th className="text-right px-3 py-2 font-medium">Unreadable</th>
                  <th className="text-right px-3 py-2 font-medium">Raced</th>
                </tr>
              </thead>
              <tbody className="divide-y divide-dark-700">
                {result.subjects.map((s) => (
                  <tr key={s.subject}>
                    <td className="px-3 py-2 font-mono text-dark-200">{s.subject}</td>
                    <td className="px-3 py-2 text-right text-dark-300">{s.examined}</td>
                    <td className="px-3 py-2 text-right text-dark-100">{s.rewritten}</td>
                    <td className="px-3 py-2 text-right text-dark-400">{s.already_current}</td>
                    <td className={`px-3 py-2 text-right ${s.unreadable > 0 ? "text-danger-400" : "text-dark-400"}`}>
                      {s.unreadable}
                    </td>
                    <td className={`px-3 py-2 text-right ${s.raced > 0 ? "text-warn-400" : "text-dark-400"}`}>
                      {s.raced}
                    </td>
                  </tr>
                ))}
              </tbody>
            </table>
          </div>
        )}
      </div>
    </div>
  );
}
