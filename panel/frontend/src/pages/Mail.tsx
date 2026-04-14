import { useAuth } from "../context/AuthContext";
import { Navigate } from "react-router-dom";
import { useState, useEffect } from "react";
import { api } from "../api";
import ProvisionLog from "../components/ProvisionLog";

interface MailDomain {
  id: string;
  domain: string;
  dkim_selector: string;
  dkim_public_key: string | null;
  catch_all: string | null;
  enabled: boolean;
  created_at: string;
}

interface MailAccount {
  id: string;
  domain_id: string;
  email: string;
  display_name: string | null;
  quota_mb: number;
  enabled: boolean;
  forward_to: string | null;
  autoresponder_enabled: boolean;
  autoresponder_subject: string | null;
  autoresponder_body: string | null;
  created_at: string;
}

interface MailAlias {
  id: string;
  source_email: string;
  destination_email: string;
  created_at: string;
}

interface DnsRecord {
  type: string;
  name: string;
  content: string;
  description: string;
}

interface QueueItem {
  id: string;
  sender: string;
  recipients: string;
  size: string;
  arrival_time: string;
  status: string;
}

interface MailBackup {
  file: string;
  email: string;
  size: number;
}

interface DnsCheckItem {
  status: string;
  type: string;
  expected?: string;
  actual?: string;
}

interface MailLogStats {
  sent: number;
  received: number;
  bounced: number;
  rejected: number;
}

interface MailLogEntry {
  time: string;
  message: string;
  level?: string;
  from?: string;
  to?: string;
  status?: string;
  message_id?: string;
}

interface MailLogs {
  stats: MailLogStats;
  recent: MailLogEntry[];
}

interface StorageAccount {
  email: string;
  bytes: number;
  mb: number;
}

interface TlsStatus {
  inbound_tls: string;
  outbound_tls: string;
  inbound_enforced: boolean;
  outbound_enforced: boolean;
}

export default function Mail() {
  const { user } = useAuth();
  if (!user || user.role !== "admin") return <Navigate to="/" replace />;
  const [domains, setDomains] = useState<MailDomain[]>([]);
  const [selectedDomain, setSelectedDomain] = useState<MailDomain | null>(null);
  const [tab, setTab] = useState<"accounts" | "aliases" | "dns" | "queue" | "logs">("accounts");
  const [accounts, setAccounts] = useState<MailAccount[]>([]);
  const [aliases, setAliases] = useState<MailAlias[]>([]);
  const [dnsRecords, setDnsRecords] = useState<DnsRecord[]>([]);
  const [queue, setQueue] = useState<QueueItem[]>([]);
  const [loading, setLoading] = useState(true);
  const [message, setMessage] = useState({ text: "", type: "" });

  // Forms
  const [showAddDomain, setShowAddDomain] = useState(false);
  const [newDomain, setNewDomain] = useState("");
  const [savingDomain, setSavingDomain] = useState(false);

  const [showAddAccount, setShowAddAccount] = useState(false);
  const [accEmail, setAccEmail] = useState("");
  const [accPassword, setAccPassword] = useState("");
  const [accName, setAccName] = useState("");
  const [accQuota, setAccQuota] = useState("1024");
  const [savingAccount, setSavingAccount] = useState(false);

  const [showAddAlias, setShowAddAlias] = useState(false);
  const [aliasSource, setAliasSource] = useState("");
  const [aliasDest, setAliasDest] = useState("");
  const [savingAlias, setSavingAlias] = useState(false);

  const [editAccount, setEditAccount] = useState<MailAccount | null>(null);
  const [editPassword, setEditPassword] = useState("");
  const [editQuota, setEditQuota] = useState("");
  const [editForward, setEditForward] = useState("");
  const [editAutoEnabled, setEditAutoEnabled] = useState(false);
  const [editAutoSubject, setEditAutoSubject] = useState("");
  const [editAutoBody, setEditAutoBody] = useState("");

  // Mail server status
  const [mailStatus, setMailStatus] = useState<{ installed: boolean; running: boolean } | null>(null);
  const [installing, setInstalling] = useState(false);
  const [installId, setInstallId] = useState<string | null>(null);
  const [showSetupGuide, setShowSetupGuide] = useState(false);

  // Mail services
  const [rspamd, setRspamd] = useState<{ installed: boolean; running: boolean } | null>(null);
  const [webmail, setWebmail] = useState<{ installed: boolean; running: boolean; port: number } | null>(null);
  const [relay, setRelay] = useState<{ configured: boolean; relayhost: string } | null>(null);
  const [relayHost, setRelayHost] = useState("");
  const [relayPort, setRelayPort] = useState("587");
  const [relayUser, setRelayUser] = useState("");
  const [relayPass, setRelayPass] = useState("");
  const [showRelayForm, setShowRelayForm] = useState(false);
  const [blacklist, setBlacklist] = useState<{ ip: string; results: { rbl: string; name: string; listed: boolean }[]; clean: boolean } | null>(null);
  const [checkingBl, setCheckingBl] = useState(false);

  // Rate limiting
  const [rateLimit, setRateLimit] = useState<{ configured: boolean; rate: string } | null>(null);
  const [rateLimitValue, setRateLimitValue] = useState("100/hour");

  // TLS enforcement
  const [tls, setTls] = useState<{ inbound_tls: string; outbound_tls: string; inbound_enforced: boolean; outbound_enforced: boolean } | null>(null);

  // Mailbox backups
  const [backups, setBackups] = useState<MailBackup[]>([]);
  const [backingUp, setBackingUp] = useState<string | null>(null);

  // DNS verification
  const [dnsCheck, setDnsCheck] = useState<{ checks: DnsCheckItem[]; all_pass: boolean } | null>(null);
  const [checkingDns, setCheckingDns] = useState(false);

  // Mail logs
  const [mailLogs, setMailLogs] = useState<MailLogs | null>(null);

  // Storage usage
  const [storage, setStorage] = useState<Record<string, { bytes: number; mb: number }>>({});

  // Bulk import
  const [showBulkImport, setShowBulkImport] = useState(false);
  const [bulkImportText, setBulkImportText] = useState("");
  const [importing, setImporting] = useState(false);
  const [pendingConfirm, setPendingConfirm] = useState<{ type: string; label: string; data?: Record<string, unknown> } | null>(null);

  const loadMailStatus = async () => {
    try {
      const data = await api.get<{ installed: boolean; running: boolean }>("/mail/status");
      setMailStatus(data);
    } catch { setMailStatus({ installed: false, running: false }); }
  };

  const handleInstall = async () => {
    setInstalling(true);
    setMessage({ text: "", type: "" });
    try {
      const result = await api.post<{ install_id?: string }>("/mail/install", {});
      if (result.install_id) {
        setInstallId(result.install_id);
      } else {
        setMessage({ text: "Mail server installed and configured", type: "success" });
        setInstalling(false);
        loadMailStatus();
      }
    } catch (e) {
      setMessage({ text: e instanceof Error ? e.message : "Installation failed", type: "error" });
      setInstalling(false);
    }
  };

  const loadDomains = async () => {
    try {
      const data = await api.get<MailDomain[]>("/mail/domains");
      setDomains(data);
      if (data.length > 0 && !selectedDomain) selectDomain(data[0]);
    } catch { /* no domains yet */ }
    finally { setLoading(false); }
  };

  const selectDomain = async (domain: MailDomain) => {
    setSelectedDomain(domain);
    loadDomainData(domain.id);
  };

  const loadDomainData = async (domainId: string) => {
    api.get<MailAccount[]>(`/mail/domains/${domainId}/accounts`).then(setAccounts).catch(() => setAccounts([]));
    api.get<MailAlias[]>(`/mail/domains/${domainId}/aliases`).then(setAliases).catch(() => setAliases([]));
    api.get<{ records: DnsRecord[] }>(`/mail/domains/${domainId}/dns`).then(d => setDnsRecords(d.records || [])).catch(() => setDnsRecords([]));
  };

  const loadQueue = async () => {
    try {
      const data = await api.get<{ queue: QueueItem[] }>("/mail/queue");
      setQueue(data.queue || []);
    } catch { setQueue([]); }
  };

  const loadMailLogs = async () => {
    try { const data = await api.get<MailLogs>("/mail/logs"); setMailLogs(data); }
    catch { setMailLogs(null); }
  };

  const loadStorage = () => {
    api.get<{ accounts: StorageAccount[] }>("/mail/storage").then(d => {
      const map: Record<string, { bytes: number; mb: number }> = {};
      d.accounts.forEach((a) => { map[a.email] = { bytes: a.bytes, mb: a.mb }; });
      setStorage(map);
    }).catch(() => {});
  };

  const loadBackups = async () => {
    try { const data = await api.get<{ backups: MailBackup[] }>("/mail/backups"); setBackups(data.backups); }
    catch { setBackups([]); }
  };

  useEffect(() => {
    loadMailStatus();
    loadDomains();
    loadStorage();
    api.get<{ installed: boolean; running: boolean }>("/mail/rspamd/status").then(setRspamd).catch(() => {});
    api.get<{ installed: boolean; running: boolean; port: number }>("/mail/webmail/status").then(setWebmail).catch(() => {});
    api.get<{ configured: boolean; relayhost: string }>("/mail/relay/status").then(setRelay).catch(() => {});
    api.get<{ configured: boolean; rate: string }>("/mail/rate-limit/status").then(setRateLimit).catch(() => {});
    api.get<TlsStatus>("/mail/tls/status").then(setTls).catch(() => {});
    loadBackups();
  }, []);
  useEffect(() => { if (tab === "queue") loadQueue(); }, [tab]);
  useEffect(() => { if (tab === "logs") loadMailLogs(); }, [tab]);

  const handleAddDomain = async () => {
    setSavingDomain(true);
    setMessage({ text: "", type: "" });
    try {
      const domain = await api.post<MailDomain>("/mail/domains", { domain: newDomain });
      setNewDomain("");
      setShowAddDomain(false);
      await loadDomains();
      selectDomain(domain);
      setMessage({ text: "Domain added with DKIM keys generated", type: "success" });
    } catch (e) {
      setMessage({ text: e instanceof Error ? e.message : "Failed", type: "error" });
    } finally { setSavingDomain(false); }
  };

  const handleDeleteDomain = (domain: MailDomain) => {
    setPendingConfirm({
      type: "delete_domain",
      label: `Delete mail domain "${domain.domain}"? All accounts and aliases will be removed.`,
      data: { id: domain.id }
    });
  };

  const handleAddAccount = async () => {
    if (!selectedDomain) return;
    setSavingAccount(true);
    try {
      await api.post(`/mail/domains/${selectedDomain.id}/accounts`, {
        email: accEmail.includes("@") ? accEmail : `${accEmail}@${selectedDomain.domain}`,
        password: accPassword,
        display_name: accName || null,
        quota_mb: parseInt(accQuota) || 1024,
      });
      setAccEmail(""); setAccPassword(""); setAccName(""); setAccQuota("1024");
      setShowAddAccount(false);
      loadDomainData(selectedDomain.id);
      setMessage({ text: "Mailbox created", type: "success" });
    } catch (e) {
      setMessage({ text: e instanceof Error ? e.message : "Failed", type: "error" });
    } finally { setSavingAccount(false); }
  };

  const handleDeleteAccount = (account: MailAccount) => {
    if (!selectedDomain) return;
    setPendingConfirm({
      type: "delete_account",
      label: `Delete mailbox "${account.email}"?`,
      data: { domainId: selectedDomain.id, accountId: account.id }
    });
  };

  const handleUpdateAccount = async () => {
    if (!selectedDomain || !editAccount) return;
    try {
      const body: Record<string, unknown> = {};
      if (editPassword) body.password = editPassword;
      if (editQuota) body.quota_mb = parseInt(editQuota);
      body.forward_to = editForward || null;
      body.autoresponder_enabled = editAutoEnabled;
      if (editAutoSubject) body.autoresponder_subject = editAutoSubject;
      if (editAutoBody) body.autoresponder_body = editAutoBody;
      await api.put(`/mail/domains/${selectedDomain.id}/accounts/${editAccount.id}`, body);
      setEditAccount(null);
      loadDomainData(selectedDomain.id);
      setMessage({ text: "Account updated", type: "success" });
    } catch (e) {
      setMessage({ text: e instanceof Error ? e.message : "Failed", type: "error" });
    }
  };

  const handleAddAlias = async () => {
    if (!selectedDomain) return;
    setSavingAlias(true);
    try {
      await api.post(`/mail/domains/${selectedDomain.id}/aliases`, {
        source_email: aliasSource.includes("@") ? aliasSource : `${aliasSource}@${selectedDomain.domain}`,
        destination_email: aliasDest,
      });
      setAliasSource(""); setAliasDest("");
      setShowAddAlias(false);
      loadDomainData(selectedDomain.id);
      setMessage({ text: "Alias created", type: "success" });
    } catch (e) {
      setMessage({ text: e instanceof Error ? e.message : "Failed", type: "error" });
    } finally { setSavingAlias(false); }
  };

  const handleDeleteAlias = (alias: MailAlias) => {
    if (!selectedDomain) return;
    setPendingConfirm({
      type: "delete_alias",
      label: `Delete alias "${alias.source_email}"?`,
      data: { domainId: selectedDomain.id, aliasId: alias.id }
    });
  };

  const openEditAccount = (acc: MailAccount) => {
    setEditAccount(acc);
    setEditPassword("");
    setEditQuota(String(acc.quota_mb));
    setEditForward(acc.forward_to || "");
    setEditAutoEnabled(acc.autoresponder_enabled);
    setEditAutoSubject(acc.autoresponder_subject || "");
    setEditAutoBody(acc.autoresponder_body || "");
  };

  const executeConfirm = async () => {
    if (!pendingConfirm) return;
    const { type, data } = pendingConfirm;
    setPendingConfirm(null);
    try {
      switch (type) {
        case "delete_domain": {
          const id = data?.id as string;
          await api.delete(`/mail/domains/${id}`);
          if (selectedDomain?.id === id) { setSelectedDomain(null); setAccounts([]); setAliases([]); }
          loadDomains();
          setMessage({ text: "Domain removed", type: "success" });
          break;
        }
        case "delete_account": {
          const domainId = data?.domainId as string;
          const accountId = data?.accountId as string;
          await api.delete(`/mail/domains/${domainId}/accounts/${accountId}`);
          loadDomainData(domainId);
          setMessage({ text: "Mailbox deleted", type: "success" });
          break;
        }
        case "delete_alias": {
          const domainId = data?.domainId as string;
          const aliasId = data?.aliasId as string;
          await api.delete(`/mail/domains/${domainId}/aliases/${aliasId}`);
          loadDomainData(domainId);
          setMessage({ text: "Alias deleted", type: "success" });
          break;
        }
        case "remove_webmail": {
          await api.post("/mail/webmail/remove", {});
          setWebmail({ installed: false, running: false, port: 0 });
          setMessage({ text: "Roundcube removed", type: "success" });
          break;
        }
        case "restore_backup": {
          await api.post("/mail/restore", { email: data?.email as string, file: data?.file as string });
          setMessage({ text: "Mailbox restored", type: "success" });
          break;
        }
        case "delete_backup": {
          await api.post("/mail/backups/delete", { file: data?.file as string });
          loadBackups();
          setMessage({ text: "Backup deleted", type: "success" });
          break;
        }
      }
    } catch (e) {
      setMessage({ text: e instanceof Error ? e.message : "Action failed", type: "error" });
    }
  };

  if (loading) {
    return (
      <div className="p-6 lg:p-8 animate-fade-up">
        <h1 className="text-sm font-medium text-dark-300 uppercase font-mono tracking-widest mb-6">Mail</h1>
        <div className="bg-dark-800 border border-dark-500 p-6 animate-pulse">
          <div className="h-6 bg-dark-700 rounded w-48 mb-4" />
          <div className="h-4 bg-dark-700 rounded w-32" />
        </div>
      </div>
    );
  }

  return (
    <div className="animate-fade-up">
      <div className="page-header">
        <div>
          <h1 className="page-header-title">Mail</h1>
          <p className="page-header-subtitle">Manage email domains, mailboxes, and aliases</p>
        </div>
        <div className="flex items-center gap-2">
          {showAddDomain ? (
            <button onClick={() => setShowAddDomain(false)} className="px-4 py-2 text-dark-300 border border-dark-600 rounded-lg text-sm font-medium hover:text-dark-100 hover:border-dark-400 transition-colors">
              Cancel
            </button>
          ) : (
            <button
              onClick={() => setShowAddDomain(true)}
              disabled={!mailStatus?.installed}
              title={!mailStatus?.installed ? "Install Mail Server in Settings → Services first" : ""}
              className={`px-4 py-2 rounded-lg text-sm font-medium transition-colors ${mailStatus?.installed ? "bg-rust-500 text-white hover:bg-rust-600" : "bg-dark-700 text-dark-400 cursor-not-allowed"}`}
            >
              Add Domain
            </button>
          )}
        </div>
      </div>

      <div className="p-6 lg:p-8">

      {message.text && (
        <div className={`mb-4 px-4 py-3 rounded-lg text-sm border ${message.type === "success" ? "bg-rust-500/10 text-rust-400 border-rust-500/20" : "bg-danger-500/10 text-danger-400 border-danger-500/20"}`}>
          {message.text}
        </div>
      )}

      {/* Inline confirmation bar */}
      {pendingConfirm && (
        <div className={`mb-4 px-4 py-3 rounded-lg border flex items-center justify-between ${
          ["delete_domain", "delete_account", "delete_backup"].includes(pendingConfirm.type) ? "border-danger-500/30 bg-danger-500/5" : "border-warn-500/30 bg-warn-500/5"
        }`}>
          <span className={`text-xs font-mono ${["delete_domain", "delete_account", "delete_backup"].includes(pendingConfirm.type) ? "text-danger-400" : "text-warn-400"}`}>
            {pendingConfirm.label}
          </span>
          <div className="flex items-center gap-2 shrink-0 ml-4">
            <button onClick={executeConfirm} className="px-3 py-1.5 bg-danger-500 text-white text-xs font-bold uppercase tracking-wider hover:bg-danger-400 transition-colors">
              Confirm
            </button>
            <button onClick={() => setPendingConfirm(null)} className="px-3 py-1.5 bg-dark-600 text-dark-200 text-xs font-bold uppercase tracking-wider hover:bg-dark-500 transition-colors">
              Cancel
            </button>
          </div>
        </div>
      )}

      {/* Mail Server Not Installed — redirect to Services */}
      {mailStatus && !mailStatus.installed && (
        <div className="mb-6 bg-dark-800 border border-dark-500 p-5">
          <div className="flex items-center justify-between">
            <div>
              <h3 className="text-sm font-medium text-dark-50">Mail Server Not Installed</h3>
              <p className="text-xs text-dark-300 mt-0.5">Postfix + Dovecot + OpenDKIM need to be installed to manage email.</p>
            </div>
            <a
              href="/settings"
              className="px-4 py-2 bg-rust-500 text-white rounded-lg text-sm font-medium hover:bg-rust-600 shrink-0 ml-4"
            >
              Go to Settings &rarr; Services
            </a>
          </div>
        </div>
      )}

      {mailStatus && mailStatus.installed && !mailStatus.running && (
        <div className="mb-4 px-4 py-3 rounded-lg text-sm border bg-warn-500/10 text-warn-400 border-warn-500/20">
          Mail server is installed but not running. Check Postfix and Dovecot services.
        </div>
      )}

      {/* Mail Services */}
      {mailStatus?.installed && (
        <div className="grid grid-cols-1 sm:grid-cols-2 lg:grid-cols-3 gap-4 mb-6">
          {/* Spam Filter (Rspamd) */}
          {rspamd && (
            <div className="bg-dark-800 rounded-lg border border-dark-500 p-4 elevation-1">
              <div className="flex items-center justify-between mb-2">
                <h3 className="text-xs font-medium text-dark-300 uppercase font-mono tracking-widest">Spam Filter</h3>
                <div className={`w-2.5 h-2.5 rounded-full ${rspamd.running ? "bg-rust-500" : rspamd.installed ? "bg-warn-500" : "bg-dark-500"}`} />
              </div>
              <p className="text-sm text-dark-100 mb-3">
                {rspamd.running ? "Active" : rspamd.installed ? "Stopped" : "Not installed"}
              </p>
              {!rspamd.installed ? (
                <button onClick={async () => {
                  try { await api.post("/mail/rspamd/install", {}); setRspamd({ installed: true, running: true }); setMessage({ text: "Rspamd installed", type: "success" }); }
                  catch (e) { setMessage({ text: e instanceof Error ? e.message : "Failed", type: "error" }); }
                }} className="w-full px-3 py-1.5 bg-rust-500 text-white rounded text-xs font-medium hover:bg-rust-600 transition-colors">Install Rspamd</button>
              ) : (
                <button onClick={async () => {
                  try { await api.post("/mail/rspamd/toggle", { enable: !rspamd.running }); setRspamd({ ...rspamd, running: !rspamd.running }); }
                  catch (e) { setMessage({ text: e instanceof Error ? e.message : "Failed", type: "error" }); }
                }} className={`w-full px-3 py-1.5 rounded text-xs font-medium transition-colors ${rspamd.running ? "bg-dark-700 text-dark-100 hover:bg-dark-600" : "bg-rust-500 text-white hover:bg-rust-600"}`}>
                  {rspamd.running ? "Disable" : "Enable"}
                </button>
              )}
            </div>
          )}

          {/* Webmail (Roundcube) */}
          {webmail && (
            <div className="bg-dark-800 rounded-lg border border-dark-500 p-4 elevation-1">
              <div className="flex items-center justify-between mb-2">
                <h3 className="text-xs font-medium text-dark-300 uppercase font-mono tracking-widest">Webmail</h3>
                <div className={`w-2.5 h-2.5 rounded-full ${webmail.running ? "bg-rust-500" : webmail.installed ? "bg-warn-500" : "bg-dark-500"}`} />
              </div>
              <p className="text-sm text-dark-100 mb-3">
                {webmail.running ? `Running (:${webmail.port})` : webmail.installed ? "Stopped" : "Not installed"}
              </p>
              {!webmail.installed ? (
                <button onClick={async () => {
                  const domain = selectedDomain?.domain || "localhost";
                  try { const r = await api.post<{ port: number }>("/mail/webmail/install", { domain, port: 8888 }); setWebmail({ installed: true, running: true, port: r.port || 8888 }); setMessage({ text: "Roundcube deployed", type: "success" }); }
                  catch (e) { setMessage({ text: e instanceof Error ? e.message : "Failed", type: "error" }); }
                }} className="w-full px-3 py-1.5 bg-rust-500 text-white rounded text-xs font-medium hover:bg-rust-600 transition-colors">Install Roundcube</button>
              ) : (
                <div className="flex gap-2">
                  {webmail.running && (
                    <a href={`http://${window.location.hostname}:${webmail.port}`} target="_blank" rel="noopener noreferrer" className="flex-1 px-3 py-1.5 bg-dark-700 text-dark-100 rounded text-xs font-medium hover:bg-dark-600 transition-colors text-center">Open</a>
                  )}
                  <button onClick={() => setPendingConfirm({ type: "remove_webmail", label: "Remove Roundcube webmail?" })}
                    className="flex-1 px-3 py-1.5 bg-dark-700 text-dark-100 rounded text-xs font-medium hover:bg-dark-600 transition-colors">Remove</button>
                </div>
              )}
            </div>
          )}

          {/* SMTP Relay */}
          <div className="bg-dark-800 rounded-lg border border-dark-500 p-4 elevation-1">
            <div className="flex items-center justify-between mb-2">
              <h3 className="text-xs font-medium text-dark-300 uppercase font-mono tracking-widest">SMTP Relay</h3>
              <div className={`w-2.5 h-2.5 rounded-full ${relay?.configured ? "bg-rust-500" : "bg-dark-500"}`} />
            </div>
            <p className="text-sm text-dark-100 mb-3 truncate" title={relay?.relayhost || ""}>
              {relay?.configured ? relay.relayhost : "Direct delivery"}
            </p>
            {relay?.configured ? (
              <button onClick={async () => {
                try { await api.post("/mail/relay/remove", {}); setRelay({ configured: false, relayhost: "" }); setMessage({ text: "Relay removed", type: "success" }); }
                catch (e) { setMessage({ text: e instanceof Error ? e.message : "Failed", type: "error" }); }
              }} className="w-full px-3 py-1.5 bg-dark-700 text-dark-200 hover:bg-dark-600 border border-dark-600 rounded text-xs font-medium transition-colors">Remove Relay</button>
            ) : (
              <button onClick={() => setShowRelayForm(!showRelayForm)} className="w-full px-3 py-1.5 bg-rust-500 text-white rounded text-xs font-medium hover:bg-rust-600 transition-colors">Configure</button>
            )}
          </div>

          {/* Email Reputation */}
          <div className="bg-dark-800 rounded-lg border border-dark-500 p-4 elevation-1">
            <div className="flex items-center justify-between mb-2">
              <h3 className="text-xs font-medium text-dark-300 uppercase font-mono tracking-widest">Reputation</h3>
              {blacklist && <div className={`w-2.5 h-2.5 rounded-full ${blacklist.clean ? "bg-rust-500" : "bg-danger-400"}`} />}
            </div>
            {blacklist ? (
              <p className={`text-sm mb-3 ${blacklist.clean ? "text-dark-100" : "text-danger-400"}`}>
                {blacklist.clean ? "Clean" : `${blacklist.results.filter(r => r.listed).length} listed`}
              </p>
            ) : (
              <p className="text-sm text-dark-100 mb-3">Not checked</p>
            )}
            <button disabled={checkingBl} onClick={async () => {
              setCheckingBl(true);
              try { const data = await api.get<{ ip: string; results: { rbl: string; name: string; listed: boolean }[]; clean: boolean }>("/mail/blacklist-check"); setBlacklist(data); }
              catch (e) { setMessage({ text: e instanceof Error ? e.message : "Check failed", type: "error" }); }
              finally { setCheckingBl(false); }
            }} className="w-full px-3 py-1.5 bg-dark-700 text-dark-200 hover:bg-dark-600 border border-dark-600 rounded text-xs font-medium disabled:opacity-50 transition-colors">
              {checkingBl ? "Checking..." : "Check"}
            </button>
          </div>

          {/* Rate Limiting */}
          <div className="bg-dark-800 rounded-lg border border-dark-500 p-4 elevation-1">
            <h3 className="text-xs font-medium text-dark-300 uppercase font-mono tracking-widest">Rate Limiting</h3>
            <div className="flex items-center gap-2 mt-2">
              <div className={`w-2.5 h-2.5 rounded-full ${rateLimit?.configured ? "bg-rust-500" : "bg-dark-500"}`} />
              <span className="text-sm text-dark-100">{rateLimit?.configured ? rateLimit.rate : "No limit"}</span>
            </div>
            <div className="flex items-center gap-2 mt-3">
              <select value={rateLimitValue} onChange={e => setRateLimitValue(e.target.value)} className="px-2 py-1.5 border border-dark-500 rounded text-xs bg-transparent text-dark-100 focus:ring-2 focus:ring-accent-500 outline-none">
                <option value="50/hour">50/hour</option>
                <option value="100/hour">100/hour</option>
                <option value="500/hour">500/hour</option>
                <option value="1000/day">1000/day</option>
                <option value="5000/day">5000/day</option>
              </select>
              <button onClick={async () => {
                try { await api.post("/mail/rate-limit/set", { rate: rateLimitValue }); setRateLimit({ configured: true, rate: rateLimitValue }); setMessage({ text: "Rate limit set", type: "success" }); }
                catch (e) { setMessage({ text: e instanceof Error ? e.message : "Failed", type: "error" }); }
              }} className="px-2 py-1.5 bg-dark-700 text-dark-200 hover:bg-dark-600 border border-dark-600 rounded text-xs font-medium transition-colors">Set</button>
              {rateLimit?.configured && (
                <button onClick={async () => {
                  try { await api.post("/mail/rate-limit/remove"); setRateLimit({ configured: false, rate: "" }); setMessage({ text: "Rate limit removed", type: "success" }); }
                  catch (e) { setMessage({ text: e instanceof Error ? e.message : "Failed", type: "error" }); }
                }} className="px-2 py-1.5 bg-dark-700 text-dark-200 hover:bg-dark-600 border border-dark-600 rounded text-xs font-medium transition-colors">Remove</button>
              )}
            </div>
          </div>

          {/* TLS Encryption */}
          <div className="bg-dark-800 rounded-lg border border-dark-500 p-4 elevation-1">
            <h3 className="text-xs font-medium text-dark-300 uppercase font-mono tracking-widest">TLS Encryption</h3>
            {tls && (
              <>
                <div className="mt-2 space-y-1">
                  <div className="flex items-center justify-between text-xs">
                    <span className="text-dark-200">Inbound (SMTP)</span>
                    <span className={tls.inbound_enforced ? "text-rust-400 font-medium" : "text-dark-300"}>{tls.inbound_tls || "not set"}</span>
                  </div>
                  <div className="flex items-center justify-between text-xs">
                    <span className="text-dark-200">Outbound (relay)</span>
                    <span className={tls.outbound_enforced ? "text-rust-400 font-medium" : "text-dark-300"}>{tls.outbound_tls || "not set"}</span>
                  </div>
                </div>
                <div className="flex gap-2 mt-3">
                  <button onClick={async () => {
                    try { await api.post("/mail/tls/enforce", { inbound: "encrypt", outbound: "encrypt" }); setTls({ ...tls, inbound_tls: "encrypt", outbound_tls: "encrypt", inbound_enforced: true, outbound_enforced: true }); setMessage({ text: "TLS enforced", type: "success" }); }
                    catch (e) { setMessage({ text: e instanceof Error ? e.message : "Failed", type: "error" }); }
                  }} className="px-2 py-1.5 bg-dark-700 text-dark-200 hover:bg-dark-600 border border-dark-600 rounded text-xs font-medium transition-colors">
                    {tls.inbound_enforced && tls.outbound_enforced ? "Enforced" : "Enforce TLS"}
                  </button>
                  {(tls.inbound_enforced || tls.outbound_enforced) && (
                    <button onClick={async () => {
                      try { await api.post("/mail/tls/enforce", { inbound: "may", outbound: "may" }); setTls({ ...tls, inbound_tls: "may", outbound_tls: "may", inbound_enforced: false, outbound_enforced: false }); setMessage({ text: "TLS set to opportunistic", type: "success" }); }
                      catch (e) { setMessage({ text: e instanceof Error ? e.message : "Failed", type: "error" }); }
                    }} className="px-2 py-1.5 bg-dark-700 text-dark-200 hover:bg-dark-600 border border-dark-600 rounded text-xs font-medium transition-colors">Opportunistic</button>
                  )}
                </div>
              </>
            )}
          </div>
        </div>
      )}

      {/* Blacklist Detail */}
      {blacklist && !blacklist.clean && (
        <div className="mb-4 bg-dark-800 rounded-lg border border-dark-500 p-4">
          <div className="flex items-center justify-between mb-2">
            <h3 className="text-xs font-medium text-dark-300 uppercase font-mono tracking-widest">Blacklist Results</h3>
            <span className="text-xs text-dark-300 font-mono">{blacklist.ip}</span>
          </div>
          <div className="grid grid-cols-2 sm:grid-cols-4 gap-1">
            {blacklist.results.map((r) => (
              <div key={r.rbl} className="flex items-center gap-2 text-xs py-1">
                <div className={`w-2 h-2 rounded-full shrink-0 ${r.listed ? "bg-danger-400" : "bg-rust-500"}`} />
                <span className={r.listed ? "text-danger-400" : "text-dark-300"}>{r.name}</span>
              </div>
            ))}
          </div>
        </div>
      )}

      {/* SMTP Relay Configuration Form */}
      {showRelayForm && (
        <div className="mb-6 bg-dark-800 border border-dark-500 p-5 space-y-4">
          <h3 className="text-xs font-medium text-dark-300 uppercase font-mono tracking-widest">Configure SMTP Relay</h3>
          <p className="text-xs text-dark-200">Route outbound mail through an external SMTP relay (SendGrid, SES, Mailgun, etc.).</p>
          <div className="grid grid-cols-1 sm:grid-cols-2 gap-3">
            <div>
              <label className="block text-xs text-dark-200 mb-1">SMTP Host</label>
              <input type="text" value={relayHost} onChange={(e) => setRelayHost(e.target.value)} placeholder="smtp.sendgrid.net" className="w-full px-3 py-1.5 border border-dark-500 rounded-lg text-sm focus:ring-2 focus:ring-accent-500 outline-none" />
            </div>
            <div>
              <label className="block text-xs text-dark-200 mb-1">Port</label>
              <input type="text" value={relayPort} onChange={(e) => setRelayPort(e.target.value)} placeholder="587" className="w-full px-3 py-1.5 border border-dark-500 rounded-lg text-sm focus:ring-2 focus:ring-accent-500 outline-none" />
            </div>
            <div>
              <label className="block text-xs text-dark-200 mb-1">Username</label>
              <input type="text" value={relayUser} onChange={(e) => setRelayUser(e.target.value)} placeholder="apikey" className="w-full px-3 py-1.5 border border-dark-500 rounded-lg text-sm focus:ring-2 focus:ring-accent-500 outline-none" />
            </div>
            <div>
              <label className="block text-xs text-dark-200 mb-1">Password</label>
              <input type="password" value={relayPass} onChange={(e) => setRelayPass(e.target.value)} placeholder="API key or password" className="w-full px-3 py-1.5 border border-dark-500 rounded-lg text-sm focus:ring-2 focus:ring-accent-500 outline-none" />
            </div>
          </div>
          <div className="flex justify-end gap-2">
            <button onClick={() => setShowRelayForm(false)} className="px-4 py-1.5 text-dark-300 border border-dark-600 rounded-lg text-sm font-medium hover:text-dark-100 hover:border-dark-400">Cancel</button>
            <button onClick={async () => {
              try {
                await api.post("/mail/relay/configure", { host: relayHost, port: parseInt(relayPort), username: relayUser, password: relayPass });
                setRelay({ configured: true, relayhost: `[${relayHost}]:${relayPort}` });
                setShowRelayForm(false);
                setRelayHost(""); setRelayPort("587"); setRelayUser(""); setRelayPass("");
                setMessage({ text: "SMTP relay configured", type: "success" });
              } catch (e) { setMessage({ text: e instanceof Error ? e.message : "Failed", type: "error" }); }
            }} disabled={!relayHost || !relayUser || !relayPass} className="px-4 py-1.5 bg-rust-500 text-white rounded-lg text-sm font-medium hover:bg-rust-600 disabled:opacity-50">Save Relay</button>
          </div>
        </div>
      )}

      {/* Add Domain Form */}
      {showAddDomain && (
        <div className="bg-dark-800 border border-dark-500 p-5 mb-6 space-y-4">
          <h3 className="text-xs font-medium text-dark-300 uppercase font-mono tracking-widest">Add Mail Domain</h3>
          <div>
            <label className="block text-xs font-medium text-dark-100 mb-1">Domain</label>
            <input type="text" value={newDomain} onChange={(e) => setNewDomain(e.target.value)} placeholder="example.com" className="w-full px-3 py-2 border border-dark-500 rounded-lg text-sm focus:ring-2 focus:ring-accent-500 outline-none" />
            <p className="text-[11px] text-dark-300 mt-1">Your domain name, e.g., example.com</p>
          </div>
          <p className="text-xs text-dark-300">DKIM keys and DNS records (MX, SPF, DKIM, DMARC) will be created automatically.</p>
          <div className="flex justify-end">
            <button onClick={handleAddDomain} disabled={savingDomain || !newDomain} className="px-4 py-2 bg-rust-500 text-white rounded-lg text-sm font-medium hover:bg-rust-600 disabled:opacity-50 flex items-center gap-2">
              {savingDomain && (
                <svg className="w-4 h-4 animate-spin" fill="none" viewBox="0 0 24 24">
                  <circle className="opacity-25" cx="12" cy="12" r="10" stroke="currentColor" strokeWidth="4" />
                  <path className="opacity-75" fill="currentColor" d="M4 12a8 8 0 018-8V0C5.373 0 0 5.373 0 12h4z" />
                </svg>
              )}
              {savingDomain ? "Creating..." : "Add Domain"}
            </button>
          </div>
        </div>
      )}

      <div className="flex flex-col md:flex-row gap-6">
        {/* Domain Sidebar */}
        {domains.length > 0 && (
          <div className="md:w-60 shrink-0">
            <div className="bg-dark-800 border border-dark-500 overflow-hidden">
              <div className="px-4 py-3 border-b border-dark-600">
                <h3 className="text-xs font-medium text-dark-300 uppercase font-mono tracking-widest">Domains</h3>
              </div>
              <div className="divide-y divide-dark-600">
                {domains.map((d) => (
                  <div key={d.id} className={`px-4 py-3 cursor-pointer hover:bg-dark-800 flex items-center justify-between transition-colors ${selectedDomain?.id === d.id ? "bg-dark-50/5 border-l-2 border-dark-50" : ""}`} onClick={() => selectDomain(d)}>
                    <div className="min-w-0">
                      <span className="text-sm font-medium text-dark-50 truncate font-mono block">{d.domain}</span>
                      <span className="text-[10px] text-dark-300">{d.dkim_public_key ? "DKIM ready" : "No DKIM"}</span>
                    </div>
                    <button onClick={(e) => { e.stopPropagation(); handleDeleteDomain(d); }} className="text-dark-300 hover:text-danger-400 shrink-0 ml-2">
                      <svg className="w-4 h-4" fill="none" viewBox="0 0 24 24" stroke="currentColor" strokeWidth={1.5}><path strokeLinecap="round" strokeLinejoin="round" d="m14.74 9-.346 9m-4.788 0L9.26 9m9.968-3.21c.342.052.682.107 1.022.166m-1.022-.165L18.16 19.673a2.25 2.25 0 0 1-2.244 2.077H8.084a2.25 2.25 0 0 1-2.244-2.077L4.772 5.79m14.456 0a48.108 48.108 0 0 0-3.478-.397m-12 .562c.34-.059.68-.114 1.022-.165m0 0a48.11 48.11 0 0 1 3.478-.397m7.5 0v-.916c0-1.18-.91-2.164-2.09-2.201a51.964 51.964 0 0 0-3.32 0c-1.18.037-2.09 1.022-2.09 2.201v.916m7.5 0a48.667 48.667 0 0 0-7.5 0" /></svg>
                    </button>
                  </div>
                ))}
              </div>
            </div>
          </div>
        )}

        {/* Content */}
        <div className="flex-1 min-w-0">
          {!selectedDomain ? (
            !showAddDomain && (
              <div className="bg-dark-800 border border-dark-500 p-12 text-center">
                <svg className="w-12 h-12 text-dark-300 mx-auto mb-3" fill="none" viewBox="0 0 24 24" stroke="currentColor" strokeWidth={1}><path strokeLinecap="round" strokeLinejoin="round" d="M21.75 6.75v10.5a2.25 2.25 0 0 1-2.25 2.25h-15a2.25 2.25 0 0 1-2.25-2.25V6.75m19.5 0A2.25 2.25 0 0 0 19.5 4.5h-15a2.25 2.25 0 0 0-2.25 2.25m19.5 0v.243a2.25 2.25 0 0 1-1.07 1.916l-7.5 4.615a2.25 2.25 0 0 1-2.36 0L3.32 8.91a2.25 2.25 0 0 1-1.07-1.916V6.75" /></svg>
                <p className="text-dark-300">{domains.length === 0 ? "Add a mail domain to get started" : "Select a domain"}</p>
              </div>
            )
          ) : (
            <div className="bg-dark-800 border border-dark-500">
              {/* Domain header + tabs */}
              <div className="px-5 py-4 border-b border-dark-600">
                <h2 className="text-lg font-semibold text-dark-50 font-mono">{selectedDomain.domain}</h2>
                <p className="text-xs text-dark-200">{accounts.length} mailbox{accounts.length !== 1 ? "es" : ""} · {aliases.length} alias{aliases.length !== 1 ? "es" : ""}</p>
              </div>
              <div className="flex border-b border-dark-600 px-5">
                {(["accounts", "aliases", "dns", "queue", "logs"] as const).map((t) => (
                  <button key={t} onClick={() => setTab(t)} className={`px-4 py-2.5 text-xs font-medium uppercase tracking-wider transition-colors ${tab === t ? "text-dark-50 border-b-2 border-dark-50" : "text-dark-300 hover:text-dark-100"}`}>
                    {t === "dns" ? "DNS Records" : t === "queue" ? "Queue" : t === "logs" ? "Logs" : t.charAt(0).toUpperCase() + t.slice(1)}
                  </button>
                ))}
              </div>

              <div className="p-5">
                {/* Accounts Tab */}
                {tab === "accounts" && (
                  <div>
                    <div className="flex items-center justify-between mb-4">
                      <h3 className="text-xs text-dark-300 uppercase tracking-widest">Mailboxes</h3>
                      <div className="flex gap-2">
                        <button onClick={() => setShowBulkImport(!showBulkImport)} className="px-3 py-1.5 bg-dark-700 text-dark-100 rounded text-xs font-medium hover:bg-dark-600">
                          {showBulkImport ? "Cancel Import" : "Bulk Import"}
                        </button>
                        {showAddAccount ? (
                          <button onClick={() => setShowAddAccount(false)} className="px-3 py-1.5 text-dark-300 border border-dark-600 rounded-lg text-xs font-medium hover:text-dark-100 hover:border-dark-400">
                            Cancel
                          </button>
                        ) : (
                          <button onClick={() => setShowAddAccount(true)} className="px-3 py-1.5 bg-rust-500 text-white rounded-lg text-xs font-medium hover:bg-rust-600">
                            Add Mailbox
                          </button>
                        )}
                      </div>
                    </div>

                    {showAddAccount && (
                      <div className="bg-dark-900 border border-dark-500 p-4 mb-4 space-y-3">
                        <div className="grid grid-cols-1 sm:grid-cols-2 gap-3">
                          <div>
                            <label className="block text-xs text-dark-200 mb-1">Email</label>
                            <div className="flex">
                              <input type="text" value={accEmail} onChange={(e) => setAccEmail(e.target.value)} placeholder="user" className="flex-1 px-3 py-1.5 border border-dark-500 rounded-l-lg text-sm focus:ring-2 focus:ring-accent-500 outline-none" />
                              <span className="px-3 py-1.5 bg-dark-700 border border-l-0 border-dark-500 rounded-r-lg text-sm text-dark-300">@{selectedDomain.domain}</span>
                            </div>
                            <p className="text-[10px] text-dark-300 mt-0.5">Full email address, e.g., user@example.com</p>
                          </div>
                          <div>
                            <label className="block text-xs text-dark-200 mb-1">Password</label>
                            <input type="password" value={accPassword} onChange={(e) => setAccPassword(e.target.value)} placeholder="Min 8 characters" className="w-full px-3 py-1.5 border border-dark-500 rounded-lg text-sm focus:ring-2 focus:ring-accent-500 outline-none" />
                          </div>
                          <div>
                            <label className="block text-xs text-dark-200 mb-1">Display Name</label>
                            <input type="text" value={accName} onChange={(e) => setAccName(e.target.value)} placeholder="John Doe" className="w-full px-3 py-1.5 border border-dark-500 rounded-lg text-sm focus:ring-2 focus:ring-accent-500 outline-none" />
                          </div>
                          <div>
                            <label className="block text-xs text-dark-200 mb-1">Quota (MB)</label>
                            <div className="flex items-center">
                              <input type="number" value={accQuota} onChange={(e) => setAccQuota(e.target.value)} className="w-full px-3 py-1.5 border border-dark-500 rounded-lg text-sm focus:ring-2 focus:ring-accent-500 outline-none" />
                              <span className="text-xs text-dark-300 ml-2 shrink-0">
                                {parseInt(accQuota) >= 1024 ? `${(parseInt(accQuota) / 1024).toFixed(1)} GB` : `${accQuota} MB`}
                              </span>
                            </div>
                            <p className="text-[10px] text-dark-300 mt-0.5">Storage limit in MB, 0 for unlimited</p>
                          </div>
                        </div>
                        <div className="flex justify-end">
                          <button onClick={handleAddAccount} disabled={savingAccount || !accEmail || !accPassword} className="px-4 py-1.5 bg-rust-500 text-white rounded-lg text-xs font-medium hover:bg-rust-600 disabled:opacity-50 flex items-center gap-2">
                            {savingAccount && (
                              <svg className="w-3.5 h-3.5 animate-spin" fill="none" viewBox="0 0 24 24">
                                <circle className="opacity-25" cx="12" cy="12" r="10" stroke="currentColor" strokeWidth="4" />
                                <path className="opacity-75" fill="currentColor" d="M4 12a8 8 0 018-8V0C5.373 0 0 5.373 0 12h4z" />
                              </svg>
                            )}
                            {savingAccount ? "Creating..." : "Create Mailbox"}
                          </button>
                        </div>
                      </div>
                    )}

                    {showBulkImport && (
                      <div className="bg-dark-900 border border-dark-500 p-4 mb-4">
                        <h4 className="text-xs font-medium text-dark-300 uppercase font-mono tracking-widest mb-2">Bulk Import Accounts</h4>
                        <p className="text-xs text-dark-200 mb-2">One account per line: <code className="bg-dark-700 px-1 rounded">email:password</code> or <code className="bg-dark-700 px-1 rounded">email:password:quota_mb</code></p>
                        <textarea
                          value={bulkImportText}
                          onChange={(e) => setBulkImportText(e.target.value)}
                          placeholder={"user1@domain.com:StrongPass123\nuser2@domain.com:AnotherPass:2048\ninfo@domain.com:Secret456:512"}
                          rows={6}
                          className="w-full px-3 py-2 border border-dark-500 rounded-lg text-sm font-mono focus:ring-2 focus:ring-accent-500 outline-none mb-3"
                        />
                        <button
                          disabled={importing || !bulkImportText.trim()}
                          onClick={async () => {
                            if (!selectedDomain) return;
                            setImporting(true);
                            const lines = bulkImportText.split("\n").filter(l => l.trim() && !l.startsWith("#"));
                            let created = 0;
                            const errors: string[] = [];
                            for (const line of lines) {
                              const parts = line.trim().split(":");
                              if (parts.length < 2) { errors.push(`Invalid: ${line}`); continue; }
                              const [email, password, quotaStr] = parts;
                              const quota = quotaStr ? parseInt(quotaStr) : 1024;
                              try {
                                await api.post(`/mail/domains/${selectedDomain.id}/accounts`, { email: email.trim(), password, quota_mb: quota });
                                created++;
                              } catch (e) {
                                errors.push(`${email}: ${e instanceof Error ? e.message : "failed"}`);
                              }
                            }
                            setMessage({
                              text: `Imported ${created} account(s)${errors.length > 0 ? `. Errors: ${errors.join("; ")}` : ""}`,
                              type: errors.length > 0 ? "error" : "success",
                            });
                            setBulkImportText("");
                            setShowBulkImport(false);
                            setImporting(false);
                            loadDomainData(selectedDomain.id);
                            loadStorage();
                          }}
                          className="px-4 py-2 bg-rust-500 text-white rounded-lg text-sm font-medium hover:bg-rust-600 disabled:opacity-50"
                        >
                          {importing ? "Importing..." : `Import ${bulkImportText.split("\n").filter(l => l.trim() && !l.startsWith("#")).length} Accounts`}
                        </button>
                      </div>
                    )}

                    {accounts.length === 0 ? (
                      <p className="text-dark-300 text-sm text-center py-8">No mailboxes yet</p>
                    ) : (
                      <div className="divide-y divide-dark-600">
                        {accounts.map((acc) => (
                          <div key={acc.id} className="py-3 flex items-center justify-between">
                            <div className="min-w-0">
                              <div className="flex items-center gap-2">
                                <span className="text-sm text-dark-50 font-mono truncate">{acc.email}</span>
                                {!acc.enabled && <span className="px-1.5 py-0.5 text-[9px] bg-dark-600 text-dark-300 uppercase">Disabled</span>}
                                {acc.forward_to && <span className="px-1.5 py-0.5 text-[9px] bg-accent-500/15 text-accent-400 uppercase">Fwd</span>}
                                {acc.autoresponder_enabled && <span className="px-1.5 py-0.5 text-[9px] bg-accent-600/15 text-accent-400 uppercase">Auto</span>}
                              </div>
                              <div className="flex items-center gap-2 flex-wrap">
                                <span className="text-xs text-dark-300">{acc.display_name || ""} · {acc.quota_mb} MB quota</span>
                                {storage[acc.email] && (
                                  <div className="flex items-center gap-2">
                                    <div className="w-20 h-1.5 bg-dark-600 rounded-full overflow-hidden">
                                      <div className="h-full bg-rust-500 rounded-full" style={{ width: `${Math.min(100, (storage[acc.email].mb / acc.quota_mb) * 100)}%` }} />
                                    </div>
                                    <span className="text-xs text-dark-300 font-mono">{storage[acc.email].mb} / {acc.quota_mb} MB</span>
                                  </div>
                                )}
                              </div>
                            </div>
                            <div className="flex items-center gap-1 shrink-0 ml-2">
                              <button disabled={backingUp === acc.email} onClick={async () => {
                                setBackingUp(acc.email);
                                try { await api.post("/mail/backup", { email: acc.email }); setMessage({ text: `Backup created for ${acc.email}`, type: "success" }); loadBackups(); }
                                catch (e) { setMessage({ text: e instanceof Error ? e.message : "Failed", type: "error" }); }
                                finally { setBackingUp(null); }
                              }} className="p-1.5 text-dark-300 hover:text-rust-400 disabled:opacity-50" title="Backup">
                                {backingUp === acc.email ? (
                                  <svg className="w-4 h-4 animate-spin" fill="none" viewBox="0 0 24 24"><circle className="opacity-25" cx="12" cy="12" r="10" stroke="currentColor" strokeWidth="4" /><path className="opacity-75" fill="currentColor" d="M4 12a8 8 0 018-8V0C5.373 0 0 5.373 0 12h4z" /></svg>
                                ) : (
                                  <svg className="w-4 h-4" fill="none" viewBox="0 0 24 24" stroke="currentColor" strokeWidth={1.5}><path strokeLinecap="round" strokeLinejoin="round" d="M20.25 7.5l-.625 10.632a2.25 2.25 0 01-2.247 2.118H6.622a2.25 2.25 0 01-2.247-2.118L3.75 7.5m8.25 3v6.75m0 0l-3-3m3 3l3-3M3.375 7.5h17.25c.621 0 1.125-.504 1.125-1.125v-1.5c0-.621-.504-1.125-1.125-1.125H3.375c-.621 0-1.125.504-1.125 1.125v1.5c0 .621.504 1.125 1.125 1.125z" /></svg>
                                )}
                              </button>
                              <button onClick={() => openEditAccount(acc)} className="p-1.5 text-dark-300 hover:text-accent-400" title="Edit">
                                <svg className="w-4 h-4" fill="none" viewBox="0 0 24 24" stroke="currentColor" strokeWidth={1.5}><path strokeLinecap="round" strokeLinejoin="round" d="m16.862 4.487 1.687-1.688a1.875 1.875 0 1 1 2.652 2.652L10.582 16.07a4.5 4.5 0 0 1-1.897 1.13L6 18l.8-2.685a4.5 4.5 0 0 1 1.13-1.897l8.932-8.931Zm0 0L19.5 7.125" /></svg>
                              </button>
                              <button onClick={() => handleDeleteAccount(acc)} className="p-1.5 text-dark-300 hover:text-danger-400" title="Delete">
                                <svg className="w-4 h-4" fill="none" viewBox="0 0 24 24" stroke="currentColor" strokeWidth={1.5}><path strokeLinecap="round" strokeLinejoin="round" d="m14.74 9-.346 9m-4.788 0L9.26 9m9.968-3.21c.342.052.682.107 1.022.166m-1.022-.165L18.16 19.673a2.25 2.25 0 0 1-2.244 2.077H8.084a2.25 2.25 0 0 1-2.244-2.077L4.772 5.79m14.456 0a48.108 48.108 0 0 0-3.478-.397m-12 .562c.34-.059.68-.114 1.022-.165m0 0a48.11 48.11 0 0 1 3.478-.397m7.5 0v-.916c0-1.18-.91-2.164-2.09-2.201a51.964 51.964 0 0 0-3.32 0c-1.18.037-2.09 1.022-2.09 2.201v.916m7.5 0a48.667 48.667 0 0 0-7.5 0" /></svg>
                              </button>
                            </div>
                          </div>
                        ))}
                      </div>
                    )}

                    {/* Edit Account Modal */}
                    {editAccount && (
                      <div className="fixed inset-0 bg-black/30 flex items-center justify-center z-50 p-4 dp-modal-overlay" onClick={() => setEditAccount(null)}>
                        <div className="bg-dark-800 border border-dark-500 p-6 w-full max-w-lg dp-modal" onClick={(e) => e.stopPropagation()}>
                          <h3 className="text-sm font-medium text-dark-50 mb-4">Edit {editAccount.email}</h3>
                          <div className="space-y-3">
                            <div>
                              <label className="block text-xs text-dark-200 mb-1">New Password (leave blank to keep)</label>
                              <input type="password" value={editPassword} onChange={(e) => setEditPassword(e.target.value)} className="w-full px-3 py-1.5 border border-dark-500 rounded-lg text-sm focus:ring-2 focus:ring-accent-500 outline-none" />
                            </div>
                            <div>
                              <label className="block text-xs text-dark-200 mb-1">Quota (MB)</label>
                              <div className="flex items-center">
                                <input type="number" value={editQuota} onChange={(e) => setEditQuota(e.target.value)} className="w-full px-3 py-1.5 border border-dark-500 rounded-lg text-sm focus:ring-2 focus:ring-accent-500 outline-none" />
                                <span className="text-xs text-dark-300 ml-2 shrink-0">
                                  {parseInt(editQuota) >= 1024 ? `${(parseInt(editQuota) / 1024).toFixed(1)} GB` : `${editQuota} MB`}
                                </span>
                              </div>
                            </div>
                            <div>
                              <label className="block text-xs text-dark-200 mb-1">Forward To (optional)</label>
                              <input type="email" value={editForward} onChange={(e) => setEditForward(e.target.value)} placeholder="other@example.com" className="w-full px-3 py-1.5 border border-dark-500 rounded-lg text-sm focus:ring-2 focus:ring-accent-500 outline-none" />
                            </div>
                            <div className="border-t border-dark-600 pt-3">
                              <label className="flex items-center gap-2 cursor-pointer mb-2">
                                <input type="checkbox" checked={editAutoEnabled} onChange={(e) => setEditAutoEnabled(e.target.checked)} className="rounded border-dark-500" />
                                <span className="text-sm text-dark-100">Autoresponder</span>
                              </label>
                              {editAutoEnabled && (
                                <div className="space-y-2 ml-6">
                                  <input type="text" value={editAutoSubject} onChange={(e) => setEditAutoSubject(e.target.value)} placeholder="Subject" className="w-full px-3 py-1.5 border border-dark-500 rounded-lg text-sm focus:ring-2 focus:ring-accent-500 outline-none" />
                                  <textarea value={editAutoBody} onChange={(e) => setEditAutoBody(e.target.value)} placeholder="Auto-reply message..." rows={3} className="w-full px-3 py-1.5 border border-dark-500 rounded-lg text-sm focus:ring-2 focus:ring-accent-500 outline-none" />
                                </div>
                              )}
                            </div>
                          </div>
                          <div className="flex justify-end gap-2 mt-4">
                            <button onClick={() => setEditAccount(null)} className="px-4 py-1.5 text-dark-300 border border-dark-600 rounded-lg text-sm font-medium hover:text-dark-100 hover:border-dark-400">Cancel</button>
                            <button onClick={handleUpdateAccount} className="px-4 py-1.5 bg-rust-500 text-white rounded-lg text-sm font-medium hover:bg-rust-600">Save</button>
                          </div>
                        </div>
                      </div>
                    )}

                    {/* Mailbox Backups */}
                    {backups.length > 0 && (
                      <div className="mt-4 bg-dark-900 border border-dark-500 rounded-lg overflow-hidden">
                        <div className="px-4 py-2 border-b border-dark-600">
                          <h4 className="text-xs font-medium text-dark-300 uppercase font-mono tracking-widest">Mailbox Backups</h4>
                        </div>
                        <div className="divide-y divide-dark-600">
                          {backups.map((b) => (
                            <div key={b.file} className="px-4 py-2 flex items-center justify-between text-xs">
                              <div>
                                <span className="text-dark-100 font-mono">{b.email}</span>
                                <span className="text-dark-300 ml-2">{(b.size / 1024 / 1024).toFixed(1)} MB</span>
                              </div>
                              <div className="flex gap-2">
                                <button onClick={() => setPendingConfirm({
                                  type: "restore_backup",
                                  label: `Restore ${b.email} from this backup? Current mail data will be overwritten.`,
                                  data: { email: b.email, file: b.file }
                                })} className="text-rust-400 hover:text-rust-300">Restore</button>
                                <button onClick={() => setPendingConfirm({
                                  type: "delete_backup",
                                  label: "Delete this backup?",
                                  data: { file: b.file }
                                })} className="text-danger-400 hover:text-danger-300">Delete</button>
                              </div>
                            </div>
                          ))}
                        </div>
                      </div>
                    )}
                  </div>
                )}

                {/* Aliases Tab */}
                {tab === "aliases" && (
                  <div>
                    <div className="flex items-center justify-between mb-4">
                      <h3 className="text-xs text-dark-300 uppercase tracking-widest">Aliases & Forwarding</h3>
                      {showAddAlias ? (
                        <button onClick={() => setShowAddAlias(false)} className="px-3 py-1.5 text-dark-300 border border-dark-600 rounded-lg text-xs font-medium hover:text-dark-100 hover:border-dark-400">
                          Cancel
                        </button>
                      ) : (
                        <button onClick={() => setShowAddAlias(true)} className="px-3 py-1.5 bg-rust-500 text-white rounded-lg text-xs font-medium hover:bg-rust-600">
                          Add Alias
                        </button>
                      )}
                    </div>

                    {showAddAlias && (
                      <div className="bg-dark-900 border border-dark-500 p-4 mb-4 space-y-3">
                        <div className="grid grid-cols-1 sm:grid-cols-2 gap-3">
                          <div>
                            <label className="block text-xs text-dark-200 mb-1">From</label>
                            <input type="text" value={aliasSource} onChange={(e) => setAliasSource(e.target.value)} placeholder={`alias@${selectedDomain.domain}`} className="w-full px-3 py-1.5 border border-dark-500 rounded-lg text-sm focus:ring-2 focus:ring-accent-500 outline-none" />
                            <p className="text-[10px] text-dark-300 mt-0.5">Source address that will be redirected</p>
                          </div>
                          <div>
                            <label className="block text-xs text-dark-200 mb-1">Deliver To</label>
                            <input type="email" value={aliasDest} onChange={(e) => setAliasDest(e.target.value)} placeholder="user@example.com" className="w-full px-3 py-1.5 border border-dark-500 rounded-lg text-sm focus:ring-2 focus:ring-accent-500 outline-none" />
                            <p className="text-[10px] text-dark-300 mt-0.5">Destination mailbox for forwarded mail</p>
                          </div>
                        </div>
                        <div className="flex justify-end">
                          <button onClick={handleAddAlias} disabled={savingAlias || !aliasSource || !aliasDest} className="px-4 py-1.5 bg-rust-500 text-white rounded-lg text-xs font-medium hover:bg-rust-600 disabled:opacity-50 flex items-center gap-2">
                            {savingAlias && (
                              <svg className="w-3.5 h-3.5 animate-spin" fill="none" viewBox="0 0 24 24">
                                <circle className="opacity-25" cx="12" cy="12" r="10" stroke="currentColor" strokeWidth="4" />
                                <path className="opacity-75" fill="currentColor" d="M4 12a8 8 0 018-8V0C5.373 0 0 5.373 0 12h4z" />
                              </svg>
                            )}
                            {savingAlias ? "Creating..." : "Create Alias"}
                          </button>
                        </div>
                      </div>
                    )}

                    {aliases.length === 0 ? (
                      <p className="text-dark-300 text-sm text-center py-8">No aliases yet</p>
                    ) : (
                      <div className="divide-y divide-dark-600">
                        {aliases.map((a) => (
                          <div key={a.id} className="py-3 flex items-center justify-between">
                            <div className="flex items-center gap-2 text-sm font-mono min-w-0">
                              <span className="text-dark-50 truncate">{a.source_email}</span>
                              <svg className="w-4 h-4 text-dark-300 shrink-0" fill="none" viewBox="0 0 24 24" stroke="currentColor" strokeWidth={2}><path strokeLinecap="round" strokeLinejoin="round" d="M13.5 4.5 21 12m0 0-7.5 7.5M21 12H3" /></svg>
                              <span className="text-dark-200 truncate">{a.destination_email}</span>
                            </div>
                            <button onClick={() => handleDeleteAlias(a)} className="p-1.5 text-dark-300 hover:text-danger-400 shrink-0 ml-2">
                              <svg className="w-4 h-4" fill="none" viewBox="0 0 24 24" stroke="currentColor" strokeWidth={1.5}><path strokeLinecap="round" strokeLinejoin="round" d="m14.74 9-.346 9m-4.788 0L9.26 9m9.968-3.21c.342.052.682.107 1.022.166m-1.022-.165L18.16 19.673a2.25 2.25 0 0 1-2.244 2.077H8.084a2.25 2.25 0 0 1-2.244-2.077L4.772 5.79m14.456 0a48.108 48.108 0 0 0-3.478-.397m-12 .562c.34-.059.68-.114 1.022-.165m0 0a48.11 48.11 0 0 1 3.478-.397m7.5 0v-.916c0-1.18-.91-2.164-2.09-2.201a51.964 51.964 0 0 0-3.32 0c-1.18.037-2.09 1.022-2.09 2.201v.916m7.5 0a48.667 48.667 0 0 0-7.5 0" /></svg>
                            </button>
                          </div>
                        ))}
                      </div>
                    )}
                  </div>
                )}

                {/* DNS Records Tab */}
                {tab === "dns" && (
                  <div>
                    <div className="flex items-center justify-between mb-4">
                      <h3 className="text-xs text-dark-300 uppercase tracking-widest">Required DNS Records</h3>
                      <button disabled={checkingDns} onClick={async () => {
                        if (!selectedDomain) return;
                        setCheckingDns(true);
                        try {
                          const data = await api.get<{ checks: DnsCheckItem[]; all_pass: boolean }>(`/mail/domains/${selectedDomain.id}/dns-check`);
                          setDnsCheck(data);
                        } catch (e) { setMessage({ text: e instanceof Error ? e.message : "Check failed", type: "error" }); }
                        finally { setCheckingDns(false); }
                      }} className="px-3 py-1.5 bg-rust-500 text-white rounded text-xs font-medium hover:bg-rust-600 disabled:opacity-50">
                        {checkingDns ? "Checking..." : "Verify DNS"}
                      </button>
                    </div>

                    {dnsCheck && (
                      <div className="mb-4 bg-dark-900 border border-dark-500 p-4 space-y-2">
                        <p className={`text-sm font-medium ${dnsCheck.all_pass ? "text-rust-400" : "text-warn-400"}`}>
                          {dnsCheck.all_pass ? "All DNS records verified" : `${dnsCheck.checks.filter((c) => c.status === "pass").length}/${dnsCheck.checks.length} records verified`}
                        </p>
                        {dnsCheck.checks.map((c, i) => (
                          <div key={i} className="flex items-center gap-3 text-sm">
                            <div className={`w-2.5 h-2.5 rounded-full ${c.status === "pass" ? "bg-rust-500" : "bg-danger-400"}`} />
                            <span className="font-mono text-dark-100 w-16">{c.type}</span>
                            <span className={c.status === "pass" ? "text-dark-200" : "text-danger-400"}>
                              {c.status === "pass" ? "Verified" : "Not found"}
                            </span>
                          </div>
                        ))}
                      </div>
                    )}

                    <p className="text-xs text-dark-200 mb-4">Add these records to your DNS provider for {selectedDomain.domain} to send and receive email properly.</p>
                    {dnsRecords.length === 0 ? (
                      <p className="text-dark-300 text-sm text-center py-8">No DNS records generated yet</p>
                    ) : (
                      <div className="space-y-3">
                        {dnsRecords.map((rec, i) => (
                          <div key={i} className="bg-dark-900 border border-dark-500 p-4">
                            <div className="flex items-center gap-2 mb-2">
                              <span className="px-2 py-0.5 text-xs font-medium bg-accent-500/15 text-accent-400">{rec.type}</span>
                              <span className="text-xs text-dark-200">{rec.description}</span>
                            </div>
                            <div className="grid grid-cols-1 sm:grid-cols-2 gap-2 text-xs font-mono">
                              <div>
                                <span className="text-dark-300">Name: </span>
                                <span className="text-dark-50">{rec.name}</span>
                              </div>
                              <div>
                                <span className="text-dark-300">Value: </span>
                                <span className="text-dark-50 break-all">{rec.content}</span>
                              </div>
                            </div>
                          </div>
                        ))}
                      </div>
                    )}
                  </div>
                )}

                {/* Queue Tab */}
                {tab === "queue" && (
                  <div>
                    <div className="flex items-center justify-between mb-4">
                      <h3 className="text-xs text-dark-300 uppercase tracking-widest">Mail Queue</h3>
                      <div className="flex gap-2">
                        <button onClick={loadQueue} className="px-3 py-1.5 bg-dark-700 text-dark-100 rounded-lg text-xs font-medium hover:bg-dark-600">Refresh</button>
                        <button onClick={async () => {
                          try { await api.post("/mail/queue/flush", {}); loadQueue(); setMessage({ text: "Queue flushed", type: "success" }); }
                          catch (e) { setMessage({ text: e instanceof Error ? e.message : "Flush failed", type: "error" }); }
                        }} className="px-3 py-1.5 bg-rust-500 text-white rounded-lg text-xs font-medium hover:bg-rust-600">Flush All</button>
                      </div>
                    </div>
                    {queue.length === 0 ? (
                      <div className="text-center py-8">
                        <svg className="w-10 h-10 text-dark-300 mx-auto mb-2" fill="none" viewBox="0 0 24 24" stroke="currentColor" strokeWidth={1}><path strokeLinecap="round" strokeLinejoin="round" d="M9 12.75 11.25 15 15 9.75M21 12a9 9 0 1 1-18 0 9 9 0 0 1 18 0Z" /></svg>
                        <p className="text-dark-300 text-sm">Mail queue is empty</p>
                      </div>
                    ) : (
                      <div className="divide-y divide-dark-600">
                        {queue.map((item) => (
                          <div key={item.id} className="py-3 flex items-center justify-between">
                            <div className="min-w-0">
                              <span className="text-sm text-dark-50 font-mono block truncate">{item.sender} → {item.recipients}</span>
                              <span className="text-xs text-dark-300">{item.size} · {item.arrival_time} · {item.status}</span>
                            </div>
                            <button onClick={async () => {
                              try { await api.delete(`/mail/queue/${item.id}`); loadQueue(); setMessage({ text: "Message removed from queue", type: "success" }); }
                              catch (e) { setMessage({ text: e instanceof Error ? e.message : "Failed to remove", type: "error" }); }
                            }} className="p-1.5 text-dark-300 hover:text-danger-400 shrink-0 ml-2">
                              <svg className="w-4 h-4" fill="none" viewBox="0 0 24 24" stroke="currentColor" strokeWidth={1.5}><path strokeLinecap="round" strokeLinejoin="round" d="m14.74 9-.346 9m-4.788 0L9.26 9m9.968-3.21c.342.052.682.107 1.022.166m-1.022-.165L18.16 19.673a2.25 2.25 0 0 1-2.244 2.077H8.084a2.25 2.25 0 0 1-2.244-2.077L4.772 5.79m14.456 0a48.108 48.108 0 0 0-3.478-.397m-12 .562c.34-.059.68-.114 1.022-.165m0 0a48.11 48.11 0 0 1 3.478-.397m7.5 0v-.916c0-1.18-.91-2.164-2.09-2.201a51.964 51.964 0 0 0-3.32 0c-1.18.037-2.09 1.022-2.09 2.201v.916m7.5 0a48.667 48.667 0 0 0-7.5 0" /></svg>
                            </button>
                          </div>
                        ))}
                      </div>
                    )}
                  </div>
                )}

                {/* Logs Tab */}
                {tab === "logs" && (
                  <div className="space-y-4">
                    <div className="flex justify-end">
                      <button onClick={loadMailLogs} className="px-3 py-1.5 bg-dark-700 text-dark-100 rounded text-xs font-medium hover:bg-dark-600">Refresh</button>
                    </div>
                    {!mailLogs && (
                      <div className="space-y-3 animate-pulse">
                        <div className="grid grid-cols-4 gap-3">
                          {[1,2,3,4].map(i => (
                            <div key={i} className="bg-dark-800 rounded-lg border border-dark-500 p-4">
                              <div className="h-3 bg-dark-700 rounded w-16 mx-auto mb-2" />
                              <div className="h-7 bg-dark-700 rounded w-12 mx-auto" />
                            </div>
                          ))}
                        </div>
                        <div className="bg-dark-800 rounded-lg border border-dark-500 p-4">
                          <div className="h-3 bg-dark-700 rounded w-32 mb-3" />
                          <div className="space-y-2">
                            {[1,2,3].map(i => <div key={i} className="h-4 bg-dark-700 rounded w-full" />)}
                          </div>
                        </div>
                      </div>
                    )}
                    {mailLogs && (
                      <>
                        <div className="grid grid-cols-4 gap-3">
                          {[
                            { label: "Sent", value: mailLogs.stats.sent, color: "text-rust-400" },
                            { label: "Received", value: mailLogs.stats.received, color: "text-accent-400" },
                            { label: "Bounced", value: mailLogs.stats.bounced, color: "text-warn-400" },
                            { label: "Rejected", value: mailLogs.stats.rejected, color: "text-danger-400" },
                          ].map(s => (
                            <div key={s.label} className="bg-dark-800 rounded-lg border border-dark-500 p-4 text-center">
                              <p className="text-xs text-dark-300 uppercase font-mono">{s.label}</p>
                              <p className={`text-2xl font-bold mt-1 ${s.color}`}>{s.value}</p>
                            </div>
                          ))}
                        </div>
                        <div className="bg-dark-800 rounded-lg border border-dark-500 overflow-hidden">
                          <div className="px-5 py-3 border-b border-dark-600">
                            <h3 className="text-xs font-medium text-dark-300 uppercase font-mono tracking-widest">Recent Activity</h3>
                          </div>
                          <div className="divide-y divide-dark-600 max-h-96 overflow-y-auto">
                            {mailLogs.recent.map((entry, i) => (
                              <div key={i} className="px-5 py-2 flex items-start gap-3">
                                <span className="text-xs text-dark-300 font-mono shrink-0 mt-0.5">{entry.time}</span>
                                <span className={`text-xs font-mono break-all ${entry.level === "error" ? "text-danger-400" : "text-dark-200"}`}>{entry.message}</span>
                              </div>
                            ))}
                            {mailLogs.recent.length === 0 && <p className="px-5 py-4 text-sm text-dark-300">No recent mail activity</p>}
                          </div>
                        </div>
                      </>
                    )}
                  </div>
                )}
              </div>
            </div>
          )}
        </div>
      </div>
      </div>
    </div>
  );
}
