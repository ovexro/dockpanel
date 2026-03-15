import { useState, useEffect } from "react";
import { api } from "../api";

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

export default function Mail() {
  const [domains, setDomains] = useState<MailDomain[]>([]);
  const [selectedDomain, setSelectedDomain] = useState<MailDomain | null>(null);
  const [tab, setTab] = useState<"accounts" | "aliases" | "dns" | "queue">("accounts");
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
  const [showSetupGuide, setShowSetupGuide] = useState(false);

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
      await api.post("/mail/install", {});
      setMessage({ text: "Mail server installed and configured", type: "success" });
      loadMailStatus();
    } catch (e) {
      setMessage({ text: e instanceof Error ? e.message : "Installation failed", type: "error" });
    } finally { setInstalling(false); }
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

  useEffect(() => { loadMailStatus(); loadDomains(); }, []);
  useEffect(() => { if (tab === "queue") loadQueue(); }, [tab]);

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

  const handleDeleteDomain = async (domain: MailDomain) => {
    if (!confirm(`Delete mail domain "${domain.domain}"? All accounts and aliases will be removed.`)) return;
    try {
      await api.delete(`/mail/domains/${domain.id}`);
      if (selectedDomain?.id === domain.id) { setSelectedDomain(null); setAccounts([]); setAliases([]); }
      loadDomains();
      setMessage({ text: "Domain removed", type: "success" });
    } catch (e) {
      setMessage({ text: e instanceof Error ? e.message : "Failed", type: "error" });
    }
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

  const handleDeleteAccount = async (account: MailAccount) => {
    if (!selectedDomain || !confirm(`Delete mailbox "${account.email}"?`)) return;
    try {
      await api.delete(`/mail/domains/${selectedDomain.id}/accounts/${account.id}`);
      loadDomainData(selectedDomain.id);
      setMessage({ text: "Mailbox deleted", type: "success" });
    } catch (e) {
      setMessage({ text: e instanceof Error ? e.message : "Failed", type: "error" });
    }
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

  const handleDeleteAlias = async (alias: MailAlias) => {
    if (!selectedDomain || !confirm(`Delete alias "${alias.source_email}"?`)) return;
    try {
      await api.delete(`/mail/domains/${selectedDomain.id}/aliases/${alias.id}`);
      loadDomainData(selectedDomain.id);
    } catch (e) {
      setMessage({ text: e instanceof Error ? e.message : "Failed", type: "error" });
    }
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
    <div className="p-6 lg:p-8 animate-fade-up">
      <div className="flex flex-col sm:flex-row sm:items-center justify-between gap-3 mb-6 pb-4 border-b border-dark-600">
        <div>
          <h1 className="text-sm font-medium text-dark-300 uppercase font-mono tracking-widest">Mail</h1>
          <p className="text-sm text-dark-200 font-mono mt-1 hidden sm:block">Manage email domains, mailboxes, and aliases</p>
        </div>
        <button onClick={() => setShowAddDomain(!showAddDomain)} className="px-4 py-2 bg-rust-500 text-white rounded-lg text-sm font-medium hover:bg-rust-600 transition-colors">
          {showAddDomain ? "Cancel" : "Add Domain"}
        </button>
      </div>

      {message.text && (
        <div className={`mb-4 px-4 py-3 rounded-lg text-sm border ${message.type === "success" ? "bg-rust-500/10 text-rust-400 border-rust-500/20" : "bg-red-500/10 text-danger-400 border-red-500/20"}`}>
          {message.text}
        </div>
      )}

      {/* Mail Server Setup */}
      {mailStatus && !mailStatus.installed && (
        <div className="mb-6 bg-dark-800 border border-dark-500 p-5 space-y-4">
          <div className="flex items-center justify-between">
            <div>
              <h3 className="text-sm font-medium text-dark-50">Mail Server Not Installed</h3>
              <p className="text-xs text-dark-300 mt-0.5">Postfix + Dovecot + OpenDKIM need to be installed to manage email.</p>
            </div>
            <button
              onClick={handleInstall}
              disabled={installing}
              className="px-4 py-2 bg-rust-500 text-white rounded-lg text-sm font-medium hover:bg-rust-600 disabled:opacity-50 shrink-0 ml-4"
            >
              {installing ? "Installing..." : "Install Mail Server"}
            </button>
          </div>

          <button
            onClick={() => setShowSetupGuide(!showSetupGuide)}
            className="flex items-center gap-2 text-sm text-blue-400 hover:text-blue-300 transition-colors"
          >
            <svg className="w-4 h-4" fill="none" viewBox="0 0 24 24" stroke="currentColor" strokeWidth={2}>
              <path strokeLinecap="round" strokeLinejoin="round" d="M11.25 11.25l.041-.02a.75.75 0 011.063.852l-.708 2.836a.75.75 0 001.063.853l.041-.021M21 12a9 9 0 11-18 0 9 9 0 0118 0zm-9-3.75h.008v.008H12V8.25z" />
            </svg>
            {showSetupGuide ? "Hide setup details" : "What does this install?"}
            <svg className={`w-3 h-3 transition-transform ${showSetupGuide ? "rotate-180" : ""}`} fill="none" viewBox="0 0 24 24" stroke="currentColor" strokeWidth={2.5}>
              <path strokeLinecap="round" strokeLinejoin="round" d="M19.5 8.25l-7.5 7.5-7.5-7.5" />
            </svg>
          </button>

          {showSetupGuide && (
            <div className="bg-dark-900 border border-dark-500 p-4 space-y-3 text-sm">
              <p className="text-dark-200 font-medium">The "Install Mail Server" button will automatically:</p>
              <div className="space-y-2 text-xs text-dark-200">
                <div className="flex items-start gap-2">
                  <span className="text-rust-400 shrink-0 mt-0.5">1.</span>
                  <span>Install <span className="text-dark-100 font-mono">postfix</span>, <span className="text-dark-100 font-mono">dovecot-imapd</span>, <span className="text-dark-100 font-mono">dovecot-pop3d</span>, <span className="text-dark-100 font-mono">opendkim</span></span>
                </div>
                <div className="flex items-start gap-2">
                  <span className="text-rust-400 shrink-0 mt-0.5">2.</span>
                  <span>Create a <span className="text-dark-100 font-mono">vmail</span> system user (uid 5000) for virtual mailbox storage</span>
                </div>
                <div className="flex items-start gap-2">
                  <span className="text-rust-400 shrink-0 mt-0.5">3.</span>
                  <span>Configure Postfix for virtual mailbox hosting with SASL auth via Dovecot</span>
                </div>
                <div className="flex items-start gap-2">
                  <span className="text-rust-400 shrink-0 mt-0.5">4.</span>
                  <span>Configure Dovecot with file-based user auth and LMTP delivery</span>
                </div>
                <div className="flex items-start gap-2">
                  <span className="text-rust-400 shrink-0 mt-0.5">5.</span>
                  <span>Set up OpenDKIM milter for DKIM signing of outgoing mail</span>
                </div>
                <div className="flex items-start gap-2">
                  <span className="text-rust-400 shrink-0 mt-0.5">6.</span>
                  <span>Enable SMTP submission port (587) with TLS + authentication</span>
                </div>
                <div className="flex items-start gap-2">
                  <span className="text-rust-400 shrink-0 mt-0.5">7.</span>
                  <span>Start and enable all services</span>
                </div>
              </div>
              <p className="text-xs text-dark-300 border-t border-dark-600 pt-3">After installation, add domains and mailboxes from this page. For each domain, you'll get DNS records (MX, SPF, DKIM, DMARC) to add at your DNS provider.</p>

              <p className="text-dark-200 font-medium mt-2">Or install manually:</p>
              <pre className="bg-dark-950 border border-dark-600 p-3 text-xs text-dark-100 font-mono overflow-x-auto whitespace-pre">{`apt install postfix dovecot-imapd dovecot-pop3d dovecot-lmtpd opendkim opendkim-tools
groupadd -g 5000 vmail
useradd -g 5000 -u 5000 -d /var/vmail -s /usr/sbin/nologin -m vmail`}</pre>
            </div>
          )}
        </div>
      )}

      {mailStatus && mailStatus.installed && !mailStatus.running && (
        <div className="mb-4 px-4 py-3 rounded-lg text-sm border bg-warn-500/10 text-warn-400 border-warn-500/20">
          Mail server is installed but not running. Check Postfix and Dovecot services.
        </div>
      )}

      {/* Add Domain Form */}
      {showAddDomain && (
        <div className="bg-dark-800 border border-dark-500 p-5 mb-6 space-y-4">
          <h3 className="text-xs font-medium text-dark-300 uppercase font-mono tracking-widest">Add Mail Domain</h3>
          <div>
            <label className="block text-xs font-medium text-dark-100 mb-1">Domain</label>
            <input type="text" value={newDomain} onChange={(e) => setNewDomain(e.target.value)} placeholder="example.com" className="w-full px-3 py-2 border border-dark-500 rounded-lg text-sm focus:ring-2 focus:ring-accent-500 outline-none" />
          </div>
          <p className="text-xs text-dark-300">DKIM keys will be generated automatically. You'll need to add DNS records after creation.</p>
          <div className="flex justify-end">
            <button onClick={handleAddDomain} disabled={savingDomain || !newDomain} className="px-4 py-2 bg-rust-500 text-white rounded-lg text-sm font-medium hover:bg-rust-600 disabled:opacity-50">
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
            <div className="bg-dark-800 border border-dark-500 p-12 text-center">
              <svg className="w-12 h-12 text-dark-300 mx-auto mb-3" fill="none" viewBox="0 0 24 24" stroke="currentColor" strokeWidth={1}><path strokeLinecap="round" strokeLinejoin="round" d="M21.75 6.75v10.5a2.25 2.25 0 0 1-2.25 2.25h-15a2.25 2.25 0 0 1-2.25-2.25V6.75m19.5 0A2.25 2.25 0 0 0 19.5 4.5h-15a2.25 2.25 0 0 0-2.25 2.25m19.5 0v.243a2.25 2.25 0 0 1-1.07 1.916l-7.5 4.615a2.25 2.25 0 0 1-2.36 0L3.32 8.91a2.25 2.25 0 0 1-1.07-1.916V6.75" /></svg>
              <p className="text-dark-300">{domains.length === 0 ? "Add a mail domain to get started" : "Select a domain"}</p>
            </div>
          ) : (
            <div className="bg-dark-800 border border-dark-500">
              {/* Domain header + tabs */}
              <div className="px-5 py-4 border-b border-dark-600">
                <h2 className="text-lg font-semibold text-dark-50 font-mono">{selectedDomain.domain}</h2>
                <p className="text-xs text-dark-200">{accounts.length} mailbox{accounts.length !== 1 ? "es" : ""} · {aliases.length} alias{aliases.length !== 1 ? "es" : ""}</p>
              </div>
              <div className="flex border-b border-dark-600 px-5">
                {(["accounts", "aliases", "dns", "queue"] as const).map((t) => (
                  <button key={t} onClick={() => setTab(t)} className={`px-4 py-2.5 text-xs font-medium uppercase tracking-wider transition-colors ${tab === t ? "text-dark-50 border-b-2 border-dark-50" : "text-dark-300 hover:text-dark-100"}`}>
                    {t === "dns" ? "DNS Records" : t === "queue" ? "Queue" : t.charAt(0).toUpperCase() + t.slice(1)}
                  </button>
                ))}
              </div>

              <div className="p-5">
                {/* Accounts Tab */}
                {tab === "accounts" && (
                  <div>
                    <div className="flex items-center justify-between mb-4">
                      <h3 className="text-xs text-dark-300 uppercase tracking-widest">Mailboxes</h3>
                      <button onClick={() => setShowAddAccount(!showAddAccount)} className="px-3 py-1.5 bg-rust-500 text-white rounded-lg text-xs font-medium hover:bg-rust-600">
                        {showAddAccount ? "Cancel" : "Add Mailbox"}
                      </button>
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
                            <input type="number" value={accQuota} onChange={(e) => setAccQuota(e.target.value)} className="w-full px-3 py-1.5 border border-dark-500 rounded-lg text-sm focus:ring-2 focus:ring-accent-500 outline-none" />
                          </div>
                        </div>
                        <div className="flex justify-end">
                          <button onClick={handleAddAccount} disabled={savingAccount || !accEmail || !accPassword} className="px-4 py-1.5 bg-rust-500 text-white rounded-lg text-xs font-medium hover:bg-rust-600 disabled:opacity-50">
                            {savingAccount ? "Creating..." : "Create Mailbox"}
                          </button>
                        </div>
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
                                {acc.forward_to && <span className="px-1.5 py-0.5 text-[9px] bg-blue-500/15 text-blue-400 uppercase">Fwd</span>}
                                {acc.autoresponder_enabled && <span className="px-1.5 py-0.5 text-[9px] bg-purple-500/15 text-purple-400 uppercase">Auto</span>}
                              </div>
                              <span className="text-xs text-dark-300">{acc.display_name || ""} · {acc.quota_mb} MB quota</span>
                            </div>
                            <div className="flex items-center gap-1 shrink-0 ml-2">
                              <button onClick={() => openEditAccount(acc)} className="p-1.5 text-dark-300 hover:text-blue-400" title="Edit">
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
                      <div className="fixed inset-0 bg-black/30 flex items-center justify-center z-50 p-4" onClick={() => setEditAccount(null)}>
                        <div className="bg-dark-800 border border-dark-500 p-6 w-full max-w-lg" onClick={(e) => e.stopPropagation()}>
                          <h3 className="text-sm font-medium text-dark-50 mb-4">Edit {editAccount.email}</h3>
                          <div className="space-y-3">
                            <div>
                              <label className="block text-xs text-dark-200 mb-1">New Password (leave blank to keep)</label>
                              <input type="password" value={editPassword} onChange={(e) => setEditPassword(e.target.value)} className="w-full px-3 py-1.5 border border-dark-500 rounded-lg text-sm focus:ring-2 focus:ring-accent-500 outline-none" />
                            </div>
                            <div>
                              <label className="block text-xs text-dark-200 mb-1">Quota (MB)</label>
                              <input type="number" value={editQuota} onChange={(e) => setEditQuota(e.target.value)} className="w-full px-3 py-1.5 border border-dark-500 rounded-lg text-sm focus:ring-2 focus:ring-accent-500 outline-none" />
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
                            <button onClick={() => setEditAccount(null)} className="px-4 py-1.5 text-dark-300 text-sm">Cancel</button>
                            <button onClick={handleUpdateAccount} className="px-4 py-1.5 bg-rust-500 text-white rounded-lg text-sm font-medium hover:bg-rust-600">Save</button>
                          </div>
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
                      <button onClick={() => setShowAddAlias(!showAddAlias)} className="px-3 py-1.5 bg-rust-500 text-white rounded-lg text-xs font-medium hover:bg-rust-600">
                        {showAddAlias ? "Cancel" : "Add Alias"}
                      </button>
                    </div>

                    {showAddAlias && (
                      <div className="bg-dark-900 border border-dark-500 p-4 mb-4 space-y-3">
                        <div className="grid grid-cols-1 sm:grid-cols-2 gap-3">
                          <div>
                            <label className="block text-xs text-dark-200 mb-1">From</label>
                            <input type="text" value={aliasSource} onChange={(e) => setAliasSource(e.target.value)} placeholder={`alias@${selectedDomain.domain}`} className="w-full px-3 py-1.5 border border-dark-500 rounded-lg text-sm focus:ring-2 focus:ring-accent-500 outline-none" />
                          </div>
                          <div>
                            <label className="block text-xs text-dark-200 mb-1">Deliver To</label>
                            <input type="email" value={aliasDest} onChange={(e) => setAliasDest(e.target.value)} placeholder="user@example.com" className="w-full px-3 py-1.5 border border-dark-500 rounded-lg text-sm focus:ring-2 focus:ring-accent-500 outline-none" />
                          </div>
                        </div>
                        <div className="flex justify-end">
                          <button onClick={handleAddAlias} disabled={savingAlias || !aliasSource || !aliasDest} className="px-4 py-1.5 bg-rust-500 text-white rounded-lg text-xs font-medium hover:bg-rust-600 disabled:opacity-50">
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
                    <h3 className="text-xs text-dark-300 uppercase tracking-widest mb-4">Required DNS Records</h3>
                    <p className="text-xs text-dark-200 mb-4">Add these records to your DNS provider for {selectedDomain.domain} to send and receive email properly.</p>
                    {dnsRecords.length === 0 ? (
                      <p className="text-dark-300 text-sm text-center py-8">Loading DNS records...</p>
                    ) : (
                      <div className="space-y-3">
                        {dnsRecords.map((rec, i) => (
                          <div key={i} className="bg-dark-900 border border-dark-500 p-4">
                            <div className="flex items-center gap-2 mb-2">
                              <span className="px-2 py-0.5 text-xs font-medium bg-blue-500/15 text-blue-400">{rec.type}</span>
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
                        <button onClick={async () => { await api.post("/mail/queue/flush", {}); loadQueue(); setMessage({ text: "Queue flushed", type: "success" }); }} className="px-3 py-1.5 bg-rust-500 text-white rounded-lg text-xs font-medium hover:bg-rust-600">Flush All</button>
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
                            <button onClick={async () => { await api.delete(`/mail/queue/${item.id}`); loadQueue(); }} className="p-1.5 text-dark-300 hover:text-danger-400 shrink-0 ml-2">
                              <svg className="w-4 h-4" fill="none" viewBox="0 0 24 24" stroke="currentColor" strokeWidth={1.5}><path strokeLinecap="round" strokeLinejoin="round" d="m14.74 9-.346 9m-4.788 0L9.26 9m9.968-3.21c.342.052.682.107 1.022.166m-1.022-.165L18.16 19.673a2.25 2.25 0 0 1-2.244 2.077H8.084a2.25 2.25 0 0 1-2.244-2.077L4.772 5.79m14.456 0a48.108 48.108 0 0 0-3.478-.397m-12 .562c.34-.059.68-.114 1.022-.165m0 0a48.11 48.11 0 0 1 3.478-.397m7.5 0v-.916c0-1.18-.91-2.164-2.09-2.201a51.964 51.964 0 0 0-3.32 0c-1.18.037-2.09 1.022-2.09 2.201v.916m7.5 0a48.667 48.667 0 0 0-7.5 0" /></svg>
                            </button>
                          </div>
                        ))}
                      </div>
                    )}
                  </div>
                )}
              </div>
            </div>
          )}
        </div>
      </div>
    </div>
  );
}
