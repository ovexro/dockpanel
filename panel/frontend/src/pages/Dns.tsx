import { useState, useEffect } from "react";
import { api } from "../api";

interface DnsZone {
  id: string;
  domain: string;
  provider: string;
  cf_zone_id?: string;
  created_at: string;
}

interface DnsRecord {
  id: string;
  type: string;
  name: string;
  content: string;
  ttl: number;
  proxied?: boolean;
  priority?: number;
}

const RECORD_TYPES = ["A", "AAAA", "CNAME", "MX", "TXT", "NS", "SRV", "CAA"];

export default function Dns() {
  const [zones, setZones] = useState<DnsZone[]>([]);
  const [selectedZone, setSelectedZone] = useState<DnsZone | null>(null);
  const [records, setRecords] = useState<DnsRecord[]>([]);
  const [loading, setLoading] = useState(true);
  const [loadingRecords, setLoadingRecords] = useState(false);
  const [message, setMessage] = useState({ text: "", type: "" });
  const [zoneCounts, setZoneCounts] = useState<Record<string, number>>({});

  // Add zone form
  const [showAddZone, setShowAddZone] = useState(false);
  const [zoneProvider, setZoneProvider] = useState<"cloudflare" | "powerdns">("cloudflare");
  const [zoneDomain, setZoneDomain] = useState("");
  const [zoneId, setZoneId] = useState("");
  const [zoneToken, setZoneToken] = useState("");
  const [zoneEmail, setZoneEmail] = useState("");
  const [authMethod, setAuthMethod] = useState<"token" | "key">("token");
  const [savingZone, setSavingZone] = useState(false);

  // Add/edit record form
  const [showRecordForm, setShowRecordForm] = useState(false);
  const [editingRecord, setEditingRecord] = useState<DnsRecord | null>(null);
  const [recType, setRecType] = useState("A");
  const [recName, setRecName] = useState("");
  const [recContent, setRecContent] = useState("");
  const [recTtl, setRecTtl] = useState("1");
  const [recProxied, setRecProxied] = useState(false);
  const [recPriority, setRecPriority] = useState("10");
  const [savingRecord, setSavingRecord] = useState(false);

  // Search/filter
  const [recordSearch, setRecordSearch] = useState("");
  const [typeFilter, setTypeFilter] = useState("");

  // Import/Export
  const [showImport, setShowImport] = useState(false);
  const [importText, setImportText] = useState("");
  const [importing, setImporting] = useState(false);

  // Propagation checker
  const [propagation, setPropagation] = useState<any>(null);
  const [checkingProp, setCheckingProp] = useState<string | null>(null);

  // Zone templates
  const [showTemplates, setShowTemplates] = useState(false);

  // Custom TTL
  const [customTtl, setCustomTtl] = useState("");

  const loadZones = async () => {
    try {
      const data = await api.get<DnsZone[]>("/dns/zones");
      setZones(data);
      if (data.length > 0 && !selectedZone) {
        selectZone(data[0]);
      }
    } catch {
      // no zones yet
    } finally {
      setLoading(false);
    }
  };

  const selectZone = async (zone: DnsZone) => {
    setSelectedZone(zone);
    setLoadingRecords(true);
    try {
      const data = await api.get<{ records: DnsRecord[] }>(`/dns/zones/${zone.id}/records`);
      const recs = data.records || [];
      setRecords(recs);
      setZoneCounts(prev => ({ ...prev, [zone.id]: recs.length }));
    } catch (e) {
      setMessage({ text: e instanceof Error ? e.message : "Failed to load records", type: "error" });
      setRecords([]);
    } finally {
      setLoadingRecords(false);
    }
  };

  useEffect(() => { loadZones(); }, []);

  const handleAddZone = async () => {
    setSavingZone(true);
    setMessage({ text: "", type: "" });
    try {
      const body: Record<string, unknown> = {
        domain: zoneDomain,
        provider: zoneProvider,
      };
      if (zoneProvider === "cloudflare") {
        body.cf_zone_id = zoneId;
        body.cf_api_token = zoneToken;
        if (authMethod === "key" && zoneEmail) {
          body.cf_api_email = zoneEmail;
        }
      }
      const zone = await api.post<DnsZone>("/dns/zones", body);
      setShowAddZone(false);
      setZoneDomain("");
      setZoneId("");
      setZoneToken("");
      setZoneEmail("");
      await loadZones();
      selectZone(zone);
      setMessage({ text: "Zone added successfully", type: "success" });
    } catch (e) {
      setMessage({ text: e instanceof Error ? e.message : "Failed to add zone", type: "error" });
    } finally {
      setSavingZone(false);
    }
  };

  const handleDeleteZone = async (zone: DnsZone) => {
    const providerLabel = zone.provider === "powerdns" ? "PowerDNS" : "Cloudflare";
    if (!confirm(`Remove "${zone.domain}" from DNS management?${zone.provider === "powerdns" ? " This will also delete the zone from PowerDNS." : " This won't delete your Cloudflare zone."}`)) return;
    try {
      await api.delete(`/dns/zones/${zone.id}`);
      if (selectedZone?.id === zone.id) {
        setSelectedZone(null);
        setRecords([]);
      }
      loadZones();
    } catch (e) {
      setMessage({ text: e instanceof Error ? e.message : "Delete failed", type: "error" });
    }
  };

  const openRecordForm = (record?: DnsRecord) => {
    if (record) {
      setEditingRecord(record);
      setRecType(record.type);
      setRecName(record.name);
      setRecContent(record.content);
      setRecTtl(String(record.ttl));
      setRecProxied(record.proxied || false);
      setRecPriority(String(record.priority || 10));
    } else {
      setEditingRecord(null);
      setRecType("A");
      setRecName("");
      setRecContent("");
      setRecTtl(selectedZone?.provider === "powerdns" ? "3600" : "1");
      setRecProxied(false);
      setRecPriority("10");
    }
    setShowRecordForm(true);
  };

  const handleSaveRecord = async () => {
    if (!selectedZone) return;
    setSavingRecord(true);
    setMessage({ text: "", type: "" });

    const ttlValue = recTtl === "custom" ? (parseInt(customTtl) || 3600) : (parseInt(recTtl) || (selectedZone.provider === "powerdns" ? 3600 : 1));
    const body: Record<string, unknown> = {
      type: recType,
      name: recName,
      content: recContent,
      ttl: ttlValue,
    };

    if (selectedZone.provider === "cloudflare" && ["A", "AAAA", "CNAME"].includes(recType)) {
      body.proxied = recProxied;
    }
    if (recType === "MX") {
      body.priority = parseInt(recPriority) || 10;
    }

    try {
      if (editingRecord) {
        await api.put(`/dns/zones/${selectedZone.id}/records/${editingRecord.id}`, body);
        setMessage({ text: "Record updated", type: "success" });
      } else {
        await api.post(`/dns/zones/${selectedZone.id}/records`, body);
        setMessage({ text: "Record created", type: "success" });
      }
      setShowRecordForm(false);
      selectZone(selectedZone);
    } catch (e) {
      setMessage({ text: e instanceof Error ? e.message : "Save failed", type: "error" });
    } finally {
      setSavingRecord(false);
    }
  };

  const handleDeleteRecord = async (record: DnsRecord) => {
    if (!selectedZone || !confirm(`Delete ${record.type} record for ${record.name}?`)) return;
    try {
      await api.delete(`/dns/zones/${selectedZone.id}/records/${record.id}`);
      selectZone(selectedZone);
      setMessage({ text: "Record deleted", type: "success" });
    } catch (e) {
      setMessage({ text: e instanceof Error ? e.message : "Delete failed", type: "error" });
    }
  };

  // ── Export zone file ──────────────────────────────────────────────────
  const handleExport = () => {
    if (!selectedZone || records.length === 0) return;
    let output = `; Zone file for ${selectedZone.domain}\n; Exported from DockPanel\n\n$ORIGIN ${selectedZone.domain}.\n$TTL 3600\n\n`;
    records.forEach(r => {
      const shortName = r.name.replace(new RegExp(`\\.?${selectedZone.domain.replace(/\./g, '\\.')}\\.?$`), '') || '@';
      const pri = r.priority != null ? `\t${r.priority}` : '';
      output += `${shortName}\t${r.ttl}\tIN\t${r.type}${pri}\t${r.content}\n`;
    });
    const blob = new Blob([output], { type: 'text/plain' });
    const url = URL.createObjectURL(blob);
    const a = document.createElement('a');
    a.href = url; a.download = `${selectedZone.domain}.zone`; a.click();
    URL.revokeObjectURL(url);
  };

  // ── Import zone file ──────────────────────────────────────────────────
  const handleImport = async () => {
    if (!selectedZone || !importText.trim()) return;
    setImporting(true);
    const validTypes = ['A', 'AAAA', 'CNAME', 'MX', 'TXT', 'NS', 'SRV', 'CAA'];
    let created = 0;
    const errors: string[] = [];

    for (const line of importText.split('\n')) {
      const trimmed = line.trim();
      if (!trimmed || trimmed.startsWith(';') || trimmed.startsWith('$')) continue;
      const parts = trimmed.split(/\s+/);
      if (parts.length < 3) continue;

      let name = parts[0], ttl = 3600, rtype = '', contentIdx = 0;
      for (let i = 0; i < parts.length; i++) {
        if (validTypes.includes(parts[i])) { rtype = parts[i]; contentIdx = i + 1; break; }
        if (parts[i] === 'IN') continue;
        if (i > 0 && /^\d+$/.test(parts[i])) ttl = parseInt(parts[i]);
      }
      if (!rtype || contentIdx >= parts.length) continue;

      const fullName = name === '@' ? selectedZone.domain : name.endsWith('.') ? name.slice(0, -1) : `${name}.${selectedZone.domain}`;
      let content = parts.slice(contentIdx).join(' ');
      let priority: number | undefined;

      if (rtype === 'MX' && contentIdx + 1 < parts.length && /^\d+$/.test(parts[contentIdx])) {
        priority = parseInt(parts[contentIdx]);
        content = parts.slice(contentIdx + 1).join(' ');
      }

      try {
        const body: Record<string, unknown> = { type: rtype, name: fullName, content, ttl };
        if (priority !== undefined) body.priority = priority;
        await api.post(`/dns/zones/${selectedZone.id}/records`, body);
        created++;
      } catch (e) { errors.push(`${fullName}: ${e instanceof Error ? e.message : 'failed'}`); }
    }

    setMessage({ text: `Imported ${created} record${created !== 1 ? 's' : ''}${errors.length ? `. ${errors.length} error${errors.length !== 1 ? 's' : ''}` : ''}`, type: errors.length ? 'error' : 'success' });
    setImporting(false); setShowImport(false); setImportText('');
    selectZone(selectedZone);
  };

  // ── Propagation checker ────────────────────────────────────────────────
  const checkPropagation = async (name: string, type: string) => {
    setCheckingProp(name);
    try {
      const data = await api.post<any>('/dns/propagation', { name, type });
      setPropagation(data);
    } catch {
      setMessage({ text: 'Propagation check failed', type: 'error' });
    } finally { setCheckingProp(null); }
  };

  // ── Zone templates ─────────────────────────────────────────────────────
  const applyTemplate = async (template: string) => {
    if (!selectedZone) return;
    const domain = selectedZone.domain;
    let serverIp = '';
    try { const resp = await fetch('https://api.ipify.org'); serverIp = await resp.text(); } catch {}

    const templates: Record<string, Record<string, unknown>[]> = {
      website: [
        { type: 'A', name: domain, content: serverIp || '1.2.3.4', ttl: 3600 },
        { type: 'CNAME', name: `www.${domain}`, content: domain, ttl: 3600 },
      ],
      email: [
        { type: 'MX', name: domain, content: `mail.${domain}`, ttl: 3600, priority: 10 },
        { type: 'TXT', name: domain, content: 'v=spf1 mx ~all', ttl: 3600 },
        { type: 'TXT', name: `_dmarc.${domain}`, content: `v=DMARC1; p=quarantine; rua=mailto:dmarc@${domain}`, ttl: 3600 },
      ],
      full: [
        { type: 'A', name: domain, content: serverIp || '1.2.3.4', ttl: 3600 },
        { type: 'CNAME', name: `www.${domain}`, content: domain, ttl: 3600 },
        { type: 'MX', name: domain, content: `mail.${domain}`, ttl: 3600, priority: 10 },
        { type: 'TXT', name: domain, content: 'v=spf1 mx ~all', ttl: 3600 },
        { type: 'TXT', name: `_dmarc.${domain}`, content: `v=DMARC1; p=quarantine; rua=mailto:dmarc@${domain}`, ttl: 3600 },
      ],
    };

    const recs = templates[template] || [];
    let created = 0;
    for (const r of recs) {
      try { await api.post(`/dns/zones/${selectedZone.id}/records`, r); created++; } catch {}
    }
    setMessage({ text: `Created ${created} record${created !== 1 ? 's' : ''} from template`, type: 'success' });
    setShowTemplates(false);
    selectZone(selectedZone);
  };

  // ── Filtered records ───────────────────────────────────────────────────
  const filteredRecords = records.filter(r => {
    if (recordSearch && !r.name.toLowerCase().includes(recordSearch.toLowerCase()) && !r.content.toLowerCase().includes(recordSearch.toLowerCase())) return false;
    if (typeFilter && r.type !== typeFilter) return false;
    return true;
  });

  const ttlLabel = (ttl: number) => {
    if (ttl === 1) return "Auto";
    if (ttl < 60) return `${ttl}s`;
    if (ttl < 3600) return `${Math.round(ttl / 60)}m`;
    if (ttl < 86400) return `${Math.round(ttl / 3600)}h`;
    return `${Math.round(ttl / 86400)}d`;
  };

  const typeColor: Record<string, string> = {
    A: "bg-blue-500/15 text-blue-400",
    AAAA: "bg-rust-500/15 text-rust-400",
    CNAME: "bg-rust-500/15 text-rust-400",
    MX: "bg-warn-500/15 text-warn-400",
    TXT: "bg-dark-700 text-dark-100",
    NS: "bg-purple-500/15 text-purple-400",
    SRV: "bg-pink-500/10 text-pink-400",
    CAA: "bg-red-500/15 text-danger-400",
  };

  const providerBadge = (provider: string) => {
    if (provider === "powerdns") {
      return <span className="ml-2 px-1.5 py-0.5 text-[9px] font-bold uppercase tracking-wider bg-blue-500/15 text-blue-400 border border-blue-500/20">PDNS</span>;
    }
    return <span className="ml-2 px-1.5 py-0.5 text-[9px] font-bold uppercase tracking-wider bg-orange-500/15 text-orange-400 border border-orange-500/20">CF</span>;
  };

  const isCloudflare = selectedZone?.provider !== "powerdns";

  if (loading) {
    return (
      <div className="p-6 lg:p-8">
        <h1 className="text-sm font-medium text-dark-300 uppercase font-mono tracking-widest mb-6">DNS Management</h1>
        <div className="bg-dark-800 rounded-lg border border-dark-500 p-6 animate-pulse">
          <div className="h-6 bg-dark-700 rounded w-48 mb-4" />
          <div className="h-4 bg-dark-700 rounded w-32" />
        </div>
      </div>
    );
  }

  return (
    <div className="p-6 lg:p-8">
      <div className="flex flex-col sm:flex-row sm:items-center justify-between gap-3 mb-6 pb-4 border-b border-dark-600">
        <div>
          <h1 className="text-sm font-medium text-dark-300 uppercase font-mono tracking-widest">DNS Management</h1>
          <p className="text-sm text-dark-200 font-mono mt-1 hidden sm:block">Manage DNS records via Cloudflare or PowerDNS</p>
        </div>
        {showAddZone ? (
          <button
            onClick={() => setShowAddZone(false)}
            className="px-4 py-2 text-dark-300 border border-dark-600 rounded-lg text-sm font-medium hover:text-dark-100 hover:border-dark-400 transition-colors"
          >
            Cancel
          </button>
        ) : (
          <button
            onClick={() => setShowAddZone(true)}
            className="px-4 py-2 bg-rust-500 text-white rounded-lg text-sm font-medium hover:bg-rust-600 transition-colors"
          >
            Add Zone
          </button>
        )}
      </div>

      {message.text && (
        <div className={`mb-4 px-4 py-3 rounded-lg text-sm border ${
          message.type === "success"
            ? "bg-rust-500/10 text-rust-400 border-rust-500/20"
            : "bg-red-500/10 text-danger-400 border-red-500/20"
        }`}>
          {message.text}
        </div>
      )}

      {/* Add Zone Form */}
      {showAddZone && (
        <div className="bg-dark-800 rounded-lg border border-dark-500 p-5 mb-6 space-y-4">
          {/* Provider selector */}
          <div>
            <label className="block text-xs font-medium text-dark-100 mb-2">DNS Provider</label>
            <div className="flex gap-3">
              <button
                onClick={() => setZoneProvider("cloudflare")}
                className={`flex-1 px-4 py-3 border text-sm font-medium transition-colors text-left ${
                  zoneProvider === "cloudflare"
                    ? "border-orange-500/50 bg-orange-500/5 text-orange-400"
                    : "border-dark-500 bg-dark-900 text-dark-200 hover:border-dark-400"
                }`}
              >
                <span className="block font-bold">Cloudflare</span>
                <span className="block text-xs text-dark-300 mt-0.5">Proxy to Cloudflare DNS API</span>
              </button>
              <button
                onClick={() => setZoneProvider("powerdns")}
                className={`flex-1 px-4 py-3 border text-sm font-medium transition-colors text-left ${
                  zoneProvider === "powerdns"
                    ? "border-blue-500/50 bg-blue-500/5 text-blue-400"
                    : "border-dark-500 bg-dark-900 text-dark-200 hover:border-dark-400"
                }`}
              >
                <span className="block font-bold">PowerDNS</span>
                <span className="block text-xs text-dark-300 mt-0.5">Self-hosted authoritative DNS</span>
              </button>
            </div>
          </div>

          {zoneProvider === "cloudflare" ? (
            <>
              <h3 className="text-xs font-medium text-dark-300 uppercase font-mono tracking-widest">Connect Cloudflare Zone</h3>
              <div className="grid grid-cols-1 md:grid-cols-2 gap-4">
                <div>
                  <label className="block text-xs font-medium text-dark-100 mb-1">Domain</label>
                  <input type="text" value={zoneDomain} onChange={(e) => setZoneDomain(e.target.value)} placeholder="example.com" className="w-full px-3 py-2 border border-dark-500 rounded-lg text-sm focus:ring-2 focus:ring-accent-500 focus:border-accent-500 outline-none" />
                  <p className="text-[11px] text-dark-300 mt-1">Your domain name, e.g., example.com</p>
                </div>
                <div>
                  <label className="block text-xs font-medium text-dark-100 mb-1">Cloudflare Zone ID</label>
                  <input type="text" value={zoneId} onChange={(e) => setZoneId(e.target.value)} placeholder="abc123..." className="w-full px-3 py-2 border border-dark-500 rounded-lg text-sm focus:ring-2 focus:ring-accent-500 focus:border-accent-500 outline-none font-mono" />
                </div>
              </div>
              <div>
                <label className="block text-xs font-medium text-dark-100 mb-2">Authentication Method</label>
                <div className="flex gap-4">
                  <label className="flex items-center gap-2 cursor-pointer">
                    <input type="radio" name="authMethod" checked={authMethod === "token"} onChange={() => setAuthMethod("token")} className="w-4 h-4 text-rust-500 border-dark-500 focus:ring-accent-500" />
                    <span className="text-sm text-dark-100">API Token</span>
                  </label>
                  <label className="flex items-center gap-2 cursor-pointer">
                    <input type="radio" name="authMethod" checked={authMethod === "key"} onChange={() => setAuthMethod("key")} className="w-4 h-4 text-rust-500 border-dark-500 focus:ring-accent-500" />
                    <span className="text-sm text-dark-100">Global API Key</span>
                  </label>
                </div>
              </div>
              <div className={`grid grid-cols-1 ${authMethod === "key" ? "md:grid-cols-2" : ""} gap-4`}>
                {authMethod === "key" && (
                  <div>
                    <label className="block text-xs font-medium text-dark-100 mb-1">Cloudflare Email</label>
                    <input type="email" value={zoneEmail} onChange={(e) => setZoneEmail(e.target.value)} placeholder="you@example.com" className="w-full px-3 py-2 border border-dark-500 rounded-lg text-sm focus:ring-2 focus:ring-accent-500 focus:border-accent-500 outline-none" />
                  </div>
                )}
                <div>
                  <label className="block text-xs font-medium text-dark-100 mb-1">{authMethod === "token" ? "API Token" : "Global API Key"}</label>
                  <input type="password" value={zoneToken} onChange={(e) => setZoneToken(e.target.value)} placeholder={authMethod === "token" ? "Scoped token with DNS edit permission" : "Global API Key from CF dashboard"} className="w-full px-3 py-2 border border-dark-500 rounded-lg text-sm focus:ring-2 focus:ring-accent-500 focus:border-accent-500 outline-none" />
                </div>
              </div>
              <p className="text-xs text-dark-300">
                {authMethod === "token"
                  ? "Create an API token at Cloudflare Dashboard \u2192 My Profile \u2192 API Tokens \u2192 Create Token \u2192 \"Edit zone DNS\" template."
                  : "Find your Global API Key at Cloudflare Dashboard \u2192 My Profile \u2192 API Tokens \u2192 Global API Key \u2192 View."}
              </p>
              <div className="flex justify-end">
                <button
                  onClick={handleAddZone}
                  disabled={savingZone || !zoneDomain || !zoneId || !zoneToken || (authMethod === "key" && !zoneEmail)}
                  className="px-4 py-2 bg-rust-500 text-white rounded-lg text-sm font-medium hover:bg-rust-600 disabled:opacity-50 flex items-center gap-2"
                >
                  {savingZone && (
                    <svg className="w-4 h-4 animate-spin" fill="none" viewBox="0 0 24 24">
                      <circle className="opacity-25" cx="12" cy="12" r="10" stroke="currentColor" strokeWidth="4" />
                      <path className="opacity-75" fill="currentColor" d="M4 12a8 8 0 018-8V0C5.373 0 0 5.373 0 12h4z" />
                    </svg>
                  )}
                  {savingZone ? "Verifying..." : "Connect Zone"}
                </button>
              </div>
            </>
          ) : (
            <>
              <h3 className="text-xs font-medium text-dark-300 uppercase font-mono tracking-widest">Create PowerDNS Zone</h3>
              <div>
                <label className="block text-xs font-medium text-dark-100 mb-1">Domain</label>
                <input type="text" value={zoneDomain} onChange={(e) => setZoneDomain(e.target.value)} placeholder="example.com" className="w-full px-3 py-2 border border-dark-500 rounded-lg text-sm focus:ring-2 focus:ring-accent-500 focus:border-accent-500 outline-none" />
                <p className="text-[11px] text-dark-300 mt-1">Your domain name, e.g., example.com</p>
              </div>
              <div className="flex items-start gap-2 bg-blue-500/5 border border-blue-500/20 px-3 py-2.5">
                <svg className="w-4 h-4 text-blue-400 shrink-0 mt-0.5" fill="none" viewBox="0 0 24 24" stroke="currentColor" strokeWidth={2}>
                  <path strokeLinecap="round" strokeLinejoin="round" d="M11.25 11.25l.041-.02a.75.75 0 011.063.852l-.708 2.836a.75.75 0 001.063.853l.041-.021M21 12a9 9 0 11-18 0 9 9 0 0118 0zm-9-3.75h.008v.008H12V8.25z" />
                </svg>
                <p className="text-xs text-dark-200">
                  PowerDNS must be installed and its API URL + key configured in{" "}
                  <a href="/settings" className="text-blue-400 hover:text-blue-300 underline underline-offset-2">Settings</a>{" "}
                  before creating zones. See the setup guide there for installation steps.
                </p>
              </div>
              <div className="flex justify-end">
                <button
                  onClick={handleAddZone}
                  disabled={savingZone || !zoneDomain}
                  className="px-4 py-2 bg-rust-500 text-white rounded-lg text-sm font-medium hover:bg-rust-600 disabled:opacity-50 flex items-center gap-2"
                >
                  {savingZone && (
                    <svg className="w-4 h-4 animate-spin" fill="none" viewBox="0 0 24 24">
                      <circle className="opacity-25" cx="12" cy="12" r="10" stroke="currentColor" strokeWidth="4" />
                      <path className="opacity-75" fill="currentColor" d="M4 12a8 8 0 018-8V0C5.373 0 0 5.373 0 12h4z" />
                    </svg>
                  )}
                  {savingZone ? "Creating..." : "Create Zone"}
                </button>
              </div>
            </>
          )}
        </div>
      )}

      <div className="flex flex-col md:flex-row gap-6">
        {/* Zone List (sidebar) */}
        {zones.length > 0 && (
          <div className="md:w-60 shrink-0">
            <div className="bg-dark-800 rounded-lg border border-dark-500 overflow-hidden">
              <div className="px-4 py-3 border-b border-dark-600">
                <h3 className="text-xs font-medium text-dark-300 uppercase font-mono tracking-widest">Zones</h3>
              </div>
              <div className="divide-y divide-dark-600">
                {zones.map((z) => (
                  <div
                    key={z.id}
                    className={`px-4 py-3 cursor-pointer hover:bg-dark-800 flex items-center justify-between transition-colors ${
                      selectedZone?.id === z.id ? "bg-rust-500/10 border-l-2 border-rust-500" : ""
                    }`}
                    onClick={() => selectZone(z)}
                  >
                    <div className="min-w-0">
                      <div className="flex items-center">
                        <span className="text-sm font-medium text-dark-50 truncate font-mono">{z.domain}</span>
                        {providerBadge(z.provider)}
                      </div>
                      {zoneCounts[z.id] !== undefined && (
                        <span className="text-[10px] text-dark-300 mt-0.5 block">{zoneCounts[z.id]} record{zoneCounts[z.id] !== 1 ? "s" : ""}</span>
                      )}
                    </div>
                    <button
                      onClick={(e) => { e.stopPropagation(); handleDeleteZone(z); }}
                      className="text-dark-300 hover:text-danger-500 shrink-0 ml-2"
                    >
                      <svg className="w-4 h-4" fill="none" viewBox="0 0 24 24" stroke="currentColor" strokeWidth={1.5}>
                        <path strokeLinecap="round" strokeLinejoin="round" d="m14.74 9-.346 9m-4.788 0L9.26 9m9.968-3.21c.342.052.682.107 1.022.166m-1.022-.165L18.16 19.673a2.25 2.25 0 0 1-2.244 2.077H8.084a2.25 2.25 0 0 1-2.244-2.077L4.772 5.79m14.456 0a48.108 48.108 0 0 0-3.478-.397m-12 .562c.34-.059.68-.114 1.022-.165m0 0a48.11 48.11 0 0 1 3.478-.397m7.5 0v-.916c0-1.18-.91-2.164-2.09-2.201a51.964 51.964 0 0 0-3.32 0c-1.18.037-2.09 1.022-2.09 2.201v.916m7.5 0a48.667 48.667 0 0 0-7.5 0" />
                      </svg>
                    </button>
                  </div>
                ))}
              </div>
            </div>
          </div>
        )}

        {/* Records Table */}
        <div className="flex-1 min-w-0">
          {!selectedZone ? (
            !showAddZone && (
              <div className="bg-dark-800 rounded-lg border border-dark-500 p-12 text-center">
                <p className="text-dark-300">{zones.length === 0 ? "Add a zone to get started" : "Select a zone"}</p>
              </div>
            )
          ) : loadingRecords ? (
            <div className="bg-dark-800 rounded-lg border border-dark-500 p-6 animate-pulse">
              <div className="h-6 bg-dark-700 rounded w-48 mb-4" />
              {[...Array(5)].map((_, i) => <div key={i} className="h-10 bg-dark-700 rounded w-full mb-2" />)}
            </div>
          ) : (
            <div className="bg-dark-800 rounded-lg border border-dark-500">
              <div className="px-4 sm:px-5 py-4 border-b border-dark-600">
                <div className="flex items-center justify-between gap-3 flex-wrap">
                  <div className="min-w-0">
                    <div className="flex items-center gap-2">
                      <h2 className="text-base sm:text-lg font-semibold text-dark-50 font-mono truncate">{selectedZone.domain}</h2>
                      {providerBadge(selectedZone.provider)}
                    </div>
                    <p className="text-xs text-dark-200">{records.length} record{records.length !== 1 ? "s" : ""}</p>
                  </div>
                  <div className="flex items-center gap-2 flex-wrap">
                    <button
                      onClick={() => selectZone(selectedZone)}
                      className="px-3 py-1.5 bg-dark-700 text-dark-100 rounded-lg text-xs font-medium hover:bg-dark-600"
                    >
                      Refresh
                    </button>
                    <button onClick={handleExport} disabled={records.length === 0}
                      className="px-3 py-1.5 bg-dark-700 text-dark-100 rounded-lg text-xs font-medium hover:bg-dark-600 disabled:opacity-50">
                      Export
                    </button>
                    <button onClick={() => setShowImport(!showImport)}
                      className="px-3 py-1.5 bg-dark-700 text-dark-100 rounded-lg text-xs font-medium hover:bg-dark-600">
                      {showImport ? "Cancel Import" : "Import"}
                    </button>
                    <button onClick={() => setShowTemplates(!showTemplates)}
                      className="px-3 py-1.5 bg-dark-700 text-dark-100 rounded-lg text-xs font-medium hover:bg-dark-600">
                      Templates
                    </button>
                    <button
                      onClick={() => openRecordForm()}
                      className="px-3 py-1.5 bg-rust-500 text-white rounded-lg text-xs font-medium hover:bg-rust-600 flex items-center gap-1"
                    >
                      <svg className="w-3.5 h-3.5" fill="none" viewBox="0 0 24 24" stroke="currentColor" strokeWidth={2}><path strokeLinecap="round" strokeLinejoin="round" d="M12 4.5v15m7.5-7.5h-15" /></svg>
                      <span className="hidden sm:inline">Add Record</span>
                      <span className="sm:hidden">Record</span>
                    </button>
                  </div>
                </div>
              </div>

              {/* Record Form */}
              {showRecordForm && (
                <div className="px-5 py-4 bg-dark-900 border-b border-dark-500 space-y-3">
                  <div className="grid grid-cols-2 sm:grid-cols-5 gap-3">
                    <div>
                      <label className="block text-xs font-medium text-dark-200 mb-1">Type</label>
                      <select
                        value={recType}
                        onChange={(e) => setRecType(e.target.value)}
                        disabled={!!editingRecord}
                        className="w-full px-2 py-1.5 border border-dark-500 rounded-md text-sm bg-dark-800 focus:ring-2 focus:ring-accent-500 outline-none disabled:bg-dark-700"
                      >
                        {RECORD_TYPES.map((t) => <option key={t} value={t}>{t}</option>)}
                      </select>
                    </div>
                    <div>
                      <label className="block text-xs font-medium text-dark-200 mb-1">Name</label>
                      <input type="text" value={recName} onChange={(e) => setRecName(e.target.value)} placeholder="@" className="w-full px-2 py-1.5 border border-dark-500 rounded-md text-sm focus:ring-2 focus:ring-accent-500 outline-none" />
                      <p className="text-[10px] text-dark-300 mt-0.5">Subdomain or @ for root</p>
                    </div>
                    <div className="col-span-2 sm:col-span-1">
                      <label className="block text-xs font-medium text-dark-200 mb-1">Content</label>
                      <input type="text" value={recContent} onChange={(e) => setRecContent(e.target.value)} placeholder={recType === "A" ? "1.2.3.4" : recType === "CNAME" ? "target.com" : ""} className="w-full px-2 py-1.5 border border-dark-500 rounded-md text-sm focus:ring-2 focus:ring-accent-500 outline-none" />
                      <p className="text-[10px] text-dark-300 mt-0.5">IP address or target</p>
                    </div>
                    <div>
                      <label className="block text-xs font-medium text-dark-200 mb-1">TTL</label>
                      <select value={recTtl} onChange={(e) => setRecTtl(e.target.value)} className="w-full px-2 py-1.5 border border-dark-500 rounded-md text-sm bg-dark-800 focus:ring-2 focus:ring-accent-500 outline-none">
                        {isCloudflare && <option value="1">Auto</option>}
                        <option value="60">1 min</option>
                        <option value="300">5 min</option>
                        <option value="3600">1 hour</option>
                        <option value="86400">1 day</option>
                        <option value="custom">Custom...</option>
                      </select>
                      {recTtl === "custom" && (
                        <input type="number" value={customTtl} onChange={e => setCustomTtl(e.target.value)}
                          placeholder="TTL in seconds" min="60"
                          className="w-full mt-1 px-2 py-1.5 border border-dark-500 rounded-md text-sm focus:ring-2 focus:ring-accent-500 outline-none" />
                      )}
                    </div>
                    {recType === "MX" && (
                      <div>
                        <label className="block text-xs font-medium text-dark-200 mb-1">Priority</label>
                        <input type="number" value={recPriority} onChange={(e) => setRecPriority(e.target.value)} min="0" max="65535" className="w-full px-2 py-1.5 border border-dark-500 rounded-md text-sm focus:ring-2 focus:ring-accent-500 outline-none" />
                      </div>
                    )}
                  </div>
                  {isCloudflare && ["A", "AAAA", "CNAME"].includes(recType) && (
                    <label className="flex items-center gap-2 cursor-pointer">
                      <input type="checkbox" checked={recProxied} onChange={(e) => setRecProxied(e.target.checked)} className="w-4 h-4 text-orange-500 border-dark-500 rounded focus:ring-orange-500" />
                      <span className="text-sm text-dark-100">Proxied through Cloudflare (orange cloud)</span>
                    </label>
                  )}
                  <div className="flex items-center gap-2">
                    <button
                      onClick={handleSaveRecord}
                      disabled={savingRecord || !recName || !recContent}
                      className="px-4 py-1.5 bg-rust-500 text-white rounded-lg text-sm font-medium hover:bg-rust-600 disabled:opacity-50 flex items-center gap-2"
                    >
                      {savingRecord && (
                        <svg className="w-3.5 h-3.5 animate-spin" fill="none" viewBox="0 0 24 24">
                          <circle className="opacity-25" cx="12" cy="12" r="10" stroke="currentColor" strokeWidth="4" />
                          <path className="opacity-75" fill="currentColor" d="M4 12a8 8 0 018-8V0C5.373 0 0 5.373 0 12h4z" />
                        </svg>
                      )}
                      {savingRecord ? "Saving..." : editingRecord ? "Update" : "Create"}
                    </button>
                    <button
                      onClick={() => setShowRecordForm(false)}
                      className="px-4 py-1.5 text-dark-300 border border-dark-600 rounded-lg text-sm font-medium hover:text-dark-100 hover:border-dark-400"
                    >
                      Cancel
                    </button>
                  </div>
                </div>
              )}

              {/* Import Zone File */}
              {showImport && (
                <div className="px-5 py-4 bg-dark-900 border-b border-dark-500 space-y-3">
                  <p className="text-xs text-dark-300">Paste BIND zone file content (one record per line):</p>
                  <textarea value={importText} onChange={e => setImportText(e.target.value)}
                    rows={8} placeholder={"@ 3600 IN A 1.2.3.4\nwww 3600 IN CNAME example.com.\n@ 3600 IN MX 10 mail.example.com."}
                    className="w-full px-3 py-2 border border-dark-500 rounded-lg text-xs font-mono focus:ring-2 focus:ring-accent-500 outline-none" />
                  <button disabled={importing || !importText.trim()} onClick={handleImport}
                    className="px-4 py-2 bg-rust-500 text-white rounded-lg text-sm font-medium hover:bg-rust-600 disabled:opacity-50">
                    {importing ? "Importing..." : "Import Records"}
                  </button>
                </div>
              )}

              {/* Zone Templates */}
              {showTemplates && (
                <div className="px-5 py-4 bg-dark-900 border-b border-dark-500">
                  <p className="text-xs text-dark-300 mb-3">Apply a record template:</p>
                  <div className="flex gap-2 flex-wrap">
                    <button onClick={() => applyTemplate('website')} className="px-3 py-1.5 bg-dark-700 text-dark-100 rounded text-xs hover:bg-dark-600">
                      Website (A + www)
                    </button>
                    <button onClick={() => applyTemplate('email')} className="px-3 py-1.5 bg-dark-700 text-dark-100 rounded text-xs hover:bg-dark-600">
                      Email (MX + SPF + DMARC)
                    </button>
                    <button onClick={() => applyTemplate('full')} className="px-3 py-1.5 bg-dark-700 text-dark-100 rounded text-xs hover:bg-dark-600">
                      Full Setup (All)
                    </button>
                  </div>
                </div>
              )}

              {/* Search & Filter */}
              <div className="px-4 sm:px-5 py-3 border-b border-dark-600 flex gap-2">
                <input value={recordSearch} onChange={e => setRecordSearch(e.target.value)}
                  placeholder="Search records..." className="flex-1 px-3 py-1.5 border border-dark-500 rounded text-sm bg-dark-900 text-dark-100 focus:ring-2 focus:ring-accent-500 outline-none" />
                <select value={typeFilter} onChange={e => setTypeFilter(e.target.value)}
                  className="px-2 py-1.5 border border-dark-500 rounded text-sm bg-dark-900 text-dark-100 focus:ring-2 focus:ring-accent-500 outline-none">
                  <option value="">All Types</option>
                  {RECORD_TYPES.map(t => <option key={t} value={t}>{t}</option>)}
                </select>
              </div>

              {/* Records — Desktop Table */}
              <div className="hidden md:block overflow-x-auto">
                <table className="w-full text-sm">
                  <thead>
                    <tr className="bg-dark-900 text-left">
                      <th className="px-4 py-2.5 text-xs font-medium text-dark-200 uppercase font-mono tracking-widest w-20">Type</th>
                      <th className="px-4 py-2.5 text-xs font-medium text-dark-200 uppercase font-mono tracking-widest">Name</th>
                      <th className="px-4 py-2.5 text-xs font-medium text-dark-200 uppercase font-mono tracking-widest">Content</th>
                      <th className="px-4 py-2.5 text-xs font-medium text-dark-200 uppercase font-mono tracking-widest w-16">TTL</th>
                      {isCloudflare && (
                        <th className="px-4 py-2.5 text-xs font-medium text-dark-200 uppercase font-mono tracking-widest w-16">Proxy</th>
                      )}
                      <th className="px-4 py-2.5 text-xs font-medium text-dark-200 uppercase font-mono tracking-widest w-20">Actions</th>
                    </tr>
                  </thead>
                  <tbody className="divide-y divide-dark-600">
                    {filteredRecords.map((r) => (
                      <tr key={r.id} className="hover:bg-dark-700/30 transition-colors">
                        <td className="px-4 py-2.5">
                          <span className={`inline-flex px-2 py-0.5 rounded text-xs font-medium ${typeColor[r.type] || "bg-dark-700 text-dark-200"}`}>
                            {r.type}
                          </span>
                        </td>
                        <td className="px-4 py-2.5 font-mono text-xs text-dark-50 truncate max-w-48">{r.name}</td>
                        <td className="px-4 py-2.5 font-mono text-xs text-dark-200 truncate max-w-64">{r.content}</td>
                        <td className="px-4 py-2.5 text-xs text-dark-200 font-mono">{ttlLabel(r.ttl)}</td>
                        {isCloudflare && (
                          <td className="px-4 py-2.5">
                            {r.proxied !== undefined && (
                              <span className={`inline-block w-3 h-3 rounded-full ${r.proxied ? "bg-orange-400" : "bg-dark-500"}`} title={r.proxied ? "Proxied" : "DNS only"} />
                            )}
                          </td>
                        )}
                        <td className="px-4 py-2.5">
                          <div className="flex items-center gap-1">
                            <button onClick={() => checkPropagation(r.name, r.type)} disabled={checkingProp === r.name}
                              title="Check propagation" className="p-1 text-dark-300 hover:text-dark-100 disabled:opacity-50">
                              <svg className="w-4 h-4" fill="none" viewBox="0 0 24 24" stroke="currentColor" strokeWidth={1.5}>
                                <path strokeLinecap="round" strokeLinejoin="round" d="M12 21a9.004 9.004 0 008.716-6.747M12 21a9.004 9.004 0 01-8.716-6.747M12 21c2.485 0 4.5-4.03 4.5-9S14.485 3 12 3m0 18c-2.485 0-4.5-4.03-4.5-9S9.515 3 12 3m0 0a8.997 8.997 0 017.843 4.582M12 3a8.997 8.997 0 00-7.843 4.582m15.686 0A11.953 11.953 0 0112 10.5c-2.998 0-5.74-1.1-7.843-2.918m15.686 0A8.959 8.959 0 0121 12c0 .778-.099 1.533-.284 2.253m0 0A17.919 17.919 0 0112 16.5c-3.162 0-6.133-.815-8.716-2.247m0 0A9.015 9.015 0 013 12c0-1.605.42-3.113 1.157-4.418" />
                              </svg>
                            </button>
                            <button onClick={() => openRecordForm(r)} className="p-1 text-dark-300 hover:text-indigo-600" title="Edit">
                              <svg className="w-4 h-4" fill="none" viewBox="0 0 24 24" stroke="currentColor" strokeWidth={1.5}>
                                <path strokeLinecap="round" strokeLinejoin="round" d="m16.862 4.487 1.687-1.688a1.875 1.875 0 1 1 2.652 2.652L10.582 16.07a4.5 4.5 0 0 1-1.897 1.13L6 18l.8-2.685a4.5 4.5 0 0 1 1.13-1.897l8.932-8.931Zm0 0L19.5 7.125" />
                              </svg>
                            </button>
                            <button onClick={() => handleDeleteRecord(r)} className="p-1 text-dark-300 hover:text-danger-500" title="Delete">
                              <svg className="w-4 h-4" fill="none" viewBox="0 0 24 24" stroke="currentColor" strokeWidth={1.5}>
                                <path strokeLinecap="round" strokeLinejoin="round" d="m14.74 9-.346 9m-4.788 0L9.26 9m9.968-3.21c.342.052.682.107 1.022.166m-1.022-.165L18.16 19.673a2.25 2.25 0 0 1-2.244 2.077H8.084a2.25 2.25 0 0 1-2.244-2.077L4.772 5.79m14.456 0a48.108 48.108 0 0 0-3.478-.397m-12 .562c.34-.059.68-.114 1.022-.165m0 0a48.11 48.11 0 0 1 3.478-.397m7.5 0v-.916c0-1.18-.91-2.164-2.09-2.201a51.964 51.964 0 0 0-3.32 0c-1.18.037-2.09 1.022-2.09 2.201v.916m7.5 0a48.667 48.667 0 0 0-7.5 0" />
                              </svg>
                            </button>
                          </div>
                        </td>
                      </tr>
                    ))}
                    {filteredRecords.length === 0 && (
                      <tr>
                        <td colSpan={isCloudflare ? 6 : 5} className="px-4 py-8 text-center text-dark-300 text-sm">
                          {records.length === 0 ? "No DNS records found" : "No records match your search"}
                        </td>
                      </tr>
                    )}
                  </tbody>
                </table>
              </div>

              {/* Records — Mobile Cards */}
              <div className="md:hidden divide-y divide-dark-600">
                {filteredRecords.map((r) => (
                  <div key={r.id} className="px-4 py-3 space-y-1.5">
                    <div className="flex items-center justify-between">
                      <div className="flex items-center gap-2 min-w-0">
                        <span className={`inline-flex px-2 py-0.5 rounded text-xs font-medium shrink-0 ${typeColor[r.type] || "bg-dark-700 text-dark-200"}`}>
                          {r.type}
                        </span>
                        <span className="font-mono text-xs text-dark-50 truncate">{r.name}</span>
                      </div>
                      <div className="flex items-center gap-0.5 shrink-0 ml-2">
                        {isCloudflare && r.proxied !== undefined && (
                          <span className={`inline-block w-2.5 h-2.5 rounded-full mr-1 ${r.proxied ? "bg-orange-400" : "bg-dark-500"}`} />
                        )}
                        <button onClick={() => checkPropagation(r.name, r.type)} disabled={checkingProp === r.name}
                          className="p-1.5 text-dark-300 hover:text-dark-100 disabled:opacity-50" title="Check propagation">
                          <svg className="w-4 h-4" fill="none" viewBox="0 0 24 24" stroke="currentColor" strokeWidth={1.5}>
                            <path strokeLinecap="round" strokeLinejoin="round" d="M12 21a9.004 9.004 0 008.716-6.747M12 21a9.004 9.004 0 01-8.716-6.747M12 21c2.485 0 4.5-4.03 4.5-9S14.485 3 12 3m0 18c-2.485 0-4.5-4.03-4.5-9S9.515 3 12 3m0 0a8.997 8.997 0 017.843 4.582M12 3a8.997 8.997 0 00-7.843 4.582m15.686 0A11.953 11.953 0 0112 10.5c-2.998 0-5.74-1.1-7.843-2.918m15.686 0A8.959 8.959 0 0121 12c0 .778-.099 1.533-.284 2.253m0 0A17.919 17.919 0 0112 16.5c-3.162 0-6.133-.815-8.716-2.247m0 0A9.015 9.015 0 013 12c0-1.605.42-3.113 1.157-4.418" />
                          </svg>
                        </button>
                        <button onClick={() => openRecordForm(r)} className="p-1.5 text-dark-300 hover:text-indigo-600">
                          <svg className="w-4 h-4" fill="none" viewBox="0 0 24 24" stroke="currentColor" strokeWidth={1.5}>
                            <path strokeLinecap="round" strokeLinejoin="round" d="m16.862 4.487 1.687-1.688a1.875 1.875 0 1 1 2.652 2.652L10.582 16.07a4.5 4.5 0 0 1-1.897 1.13L6 18l.8-2.685a4.5 4.5 0 0 1 1.13-1.897l8.932-8.931Zm0 0L19.5 7.125" />
                          </svg>
                        </button>
                        <button onClick={() => handleDeleteRecord(r)} className="p-1.5 text-dark-300 hover:text-danger-500">
                          <svg className="w-4 h-4" fill="none" viewBox="0 0 24 24" stroke="currentColor" strokeWidth={1.5}>
                            <path strokeLinecap="round" strokeLinejoin="round" d="m14.74 9-.346 9m-4.788 0L9.26 9m9.968-3.21c.342.052.682.107 1.022.166m-1.022-.165L18.16 19.673a2.25 2.25 0 0 1-2.244 2.077H8.084a2.25 2.25 0 0 1-2.244-2.077L4.772 5.79m14.456 0a48.108 48.108 0 0 0-3.478-.397m-12 .562c.34-.059.68-.114 1.022-.165m0 0a48.11 48.11 0 0 1 3.478-.397m7.5 0v-.916c0-1.18-.91-2.164-2.09-2.201a51.964 51.964 0 0 0-3.32 0c-1.18.037-2.09 1.022-2.09 2.201v.916m7.5 0a48.667 48.667 0 0 0-7.5 0" />
                          </svg>
                        </button>
                      </div>
                    </div>
                    <div className="flex items-center gap-3 text-xs">
                      <span className="font-mono text-dark-300 truncate">{r.content}</span>
                      <span className="text-dark-500 shrink-0">{ttlLabel(r.ttl)}</span>
                    </div>
                  </div>
                ))}
                {filteredRecords.length === 0 && (
                  <div className="px-4 py-8 text-center text-dark-300 text-sm">
                    {records.length === 0 ? "No DNS records found" : "No records match your search"}
                  </div>
                )}
              </div>

              {/* Propagation Results */}
              {propagation && (
                <div className="px-4 sm:px-5 py-4 border-t border-dark-600">
                  <div className="bg-dark-900 rounded-lg border border-dark-500 p-4">
                    <div className="flex items-center justify-between mb-3">
                      <h4 className="text-xs font-medium text-dark-300 uppercase font-mono tracking-widest">
                        Propagation: {propagation.name} ({propagation.type})
                      </h4>
                      <button onClick={() => setPropagation(null)} className="text-xs text-dark-300 hover:text-dark-100">Close</button>
                    </div>
                    <div className="space-y-2">
                      {propagation.results.map((r: any, i: number) => (
                        <div key={i} className="flex items-center gap-3 text-xs">
                          <div className={`w-2.5 h-2.5 rounded-full shrink-0 ${r.found ? "bg-rust-500" : "bg-danger-400"}`} />
                          <span className="text-dark-200 w-24">{r.label}</span>
                          <span className="text-dark-300 font-mono w-32">{r.resolver}</span>
                          <span className={`font-mono truncate ${r.found ? "text-dark-100" : "text-danger-400"}`}>{r.value}</span>
                        </div>
                      ))}
                    </div>
                    <p className={`text-xs mt-3 font-medium ${propagation.fully_propagated ? "text-rust-400" : "text-warn-400"}`}>
                      {propagation.propagated}/{propagation.total} resolvers responding
                    </p>
                  </div>
                </div>
              )}
            </div>
          )}
        </div>
      </div>
    </div>
  );
}
