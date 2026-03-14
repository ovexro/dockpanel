import { useState, useEffect } from "react";
import { api } from "../api";

interface DnsZone {
  id: string;
  domain: string;
  provider: string;
  cf_zone_id: string;
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

  // Add zone form
  const [showAddZone, setShowAddZone] = useState(false);
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
      setRecords(data.records || []);
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
      const zone = await api.post<DnsZone>("/dns/zones", {
        domain: zoneDomain,
        cf_zone_id: zoneId,
        cf_api_token: zoneToken,
        ...(authMethod === "key" && zoneEmail ? { cf_api_email: zoneEmail } : {}),
      });
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
    if (!confirm(`Remove "${zone.domain}" from DNS management? This won't delete your Cloudflare zone.`)) return;
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
      setRecTtl("1");
      setRecProxied(false);
      setRecPriority("10");
    }
    setShowRecordForm(true);
  };

  const handleSaveRecord = async () => {
    if (!selectedZone) return;
    setSavingRecord(true);
    setMessage({ text: "", type: "" });

    const body: Record<string, unknown> = {
      type: recType,
      name: recName,
      content: recContent,
      ttl: parseInt(recTtl) || 1,
    };

    if (["A", "AAAA", "CNAME"].includes(recType)) {
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

  const ttlLabel = (ttl: number) => {
    if (ttl === 1) return "Auto";
    if (ttl < 60) return `${ttl}s`;
    if (ttl < 3600) return `${Math.round(ttl / 60)}m`;
    if (ttl < 86400) return `${Math.round(ttl / 3600)}h`;
    return `${Math.round(ttl / 86400)}d`;
  };

  const typeColor: Record<string, string> = {
    A: "bg-blue-500/15 text-blue-400",
    AAAA: "bg-rust-500/15 text-rust-600",
    CNAME: "bg-emerald-500/15 text-emerald-400",
    MX: "bg-amber-500/15 text-amber-400",
    TXT: "bg-dark-700 text-dark-100",
    NS: "bg-purple-500/15 text-purple-400",
    SRV: "bg-pink-500/10 text-pink-400",
    CAA: "bg-red-500/15 text-red-400",
  };

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
      <div className="flex items-center justify-between mb-6 pb-4 border-b border-dark-600">
        <div>
          <h1 className="text-sm font-medium text-dark-300 uppercase font-mono tracking-widest">DNS Management</h1>
          <p className="text-sm text-dark-200 font-mono mt-1">Manage DNS records via Cloudflare</p>
        </div>
        <button
          onClick={() => setShowAddZone(!showAddZone)}
          className="px-4 py-2 bg-rust-500 text-white rounded-lg text-sm font-medium hover:bg-rust-600 transition-colors"
        >
          {showAddZone ? "Cancel" : "Add Zone"}
        </button>
      </div>

      {message.text && (
        <div className={`mb-4 px-4 py-3 rounded-lg text-sm border ${
          message.type === "success"
            ? "bg-emerald-500/10 text-emerald-400 border-emerald-500/20"
            : "bg-red-500/10 text-red-400 border-red-500/20"
        }`}>
          {message.text}
        </div>
      )}

      {/* Add Zone Form */}
      {showAddZone && (
        <div className="bg-dark-800 rounded-lg border border-dark-500 p-5 mb-6 space-y-4">
          <h3 className="text-xs font-medium text-dark-300 uppercase font-mono tracking-widest">Connect Cloudflare Zone</h3>
          <div className="grid grid-cols-1 md:grid-cols-2 gap-4">
            <div>
              <label className="block text-xs font-medium text-dark-100 mb-1">Domain</label>
              <input type="text" value={zoneDomain} onChange={(e) => setZoneDomain(e.target.value)} placeholder="example.com" className="w-full px-3 py-2 border border-dark-500 rounded-lg text-sm focus:ring-2 focus:ring-rust-500 focus:border-rust-500 outline-none" />
            </div>
            <div>
              <label className="block text-xs font-medium text-dark-100 mb-1">Cloudflare Zone ID</label>
              <input type="text" value={zoneId} onChange={(e) => setZoneId(e.target.value)} placeholder="abc123..." className="w-full px-3 py-2 border border-dark-500 rounded-lg text-sm focus:ring-2 focus:ring-rust-500 focus:border-rust-500 outline-none font-mono" />
            </div>
          </div>
          <div>
            <label className="block text-xs font-medium text-dark-100 mb-2">Authentication Method</label>
            <div className="flex gap-4">
              <label className="flex items-center gap-2 cursor-pointer">
                <input type="radio" name="authMethod" checked={authMethod === "token"} onChange={() => setAuthMethod("token")} className="w-4 h-4 text-rust-500 border-dark-500 focus:ring-rust-500" />
                <span className="text-sm text-dark-100">API Token</span>
              </label>
              <label className="flex items-center gap-2 cursor-pointer">
                <input type="radio" name="authMethod" checked={authMethod === "key"} onChange={() => setAuthMethod("key")} className="w-4 h-4 text-rust-500 border-dark-500 focus:ring-rust-500" />
                <span className="text-sm text-dark-100">Global API Key</span>
              </label>
            </div>
          </div>
          <div className={`grid grid-cols-1 ${authMethod === "key" ? "md:grid-cols-2" : ""} gap-4`}>
            {authMethod === "key" && (
              <div>
                <label className="block text-xs font-medium text-dark-100 mb-1">Cloudflare Email</label>
                <input type="email" value={zoneEmail} onChange={(e) => setZoneEmail(e.target.value)} placeholder="you@example.com" className="w-full px-3 py-2 border border-dark-500 rounded-lg text-sm focus:ring-2 focus:ring-rust-500 focus:border-rust-500 outline-none" />
              </div>
            )}
            <div>
              <label className="block text-xs font-medium text-dark-100 mb-1">{authMethod === "token" ? "API Token" : "Global API Key"}</label>
              <input type="password" value={zoneToken} onChange={(e) => setZoneToken(e.target.value)} placeholder={authMethod === "token" ? "Scoped token with DNS edit permission" : "Global API Key from CF dashboard"} className="w-full px-3 py-2 border border-dark-500 rounded-lg text-sm focus:ring-2 focus:ring-rust-500 focus:border-rust-500 outline-none" />
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
              className="px-4 py-2 bg-rust-500 text-white rounded-lg text-sm font-medium hover:bg-rust-600 disabled:opacity-50"
            >
              {savingZone ? "Verifying..." : "Connect Zone"}
            </button>
          </div>
        </div>
      )}

      <div className="flex flex-col md:flex-row gap-6">
        {/* Zone List (sidebar) */}
        {zones.length > 0 && (
          <div className="md:w-56 shrink-0">
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
                    <span className="text-sm font-medium text-dark-50 truncate font-mono">{z.domain}</span>
                    <button
                      onClick={(e) => { e.stopPropagation(); handleDeleteZone(z); }}
                      className="text-dark-300 hover:text-red-500 shrink-0"
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
            <div className="bg-dark-800 rounded-lg border border-dark-500 p-12 text-center">
              <p className="text-dark-300">{zones.length === 0 ? "Add a Cloudflare zone to get started" : "Select a zone"}</p>
            </div>
          ) : loadingRecords ? (
            <div className="bg-dark-800 rounded-lg border border-dark-500 p-6 animate-pulse">
              <div className="h-6 bg-dark-700 rounded w-48 mb-4" />
              {[...Array(5)].map((_, i) => <div key={i} className="h-10 bg-dark-700 rounded w-full mb-2" />)}
            </div>
          ) : (
            <div className="bg-dark-800 rounded-lg border border-dark-500 overflow-hidden">
              <div className="px-5 py-4 border-b border-dark-600 flex items-center justify-between">
                <div>
                  <h2 className="text-lg font-semibold text-dark-50 font-mono">{selectedZone.domain}</h2>
                  <p className="text-xs text-dark-200">{records.length} record{records.length !== 1 ? "s" : ""}</p>
                </div>
                <div className="flex items-center gap-2">
                  <button
                    onClick={() => selectZone(selectedZone)}
                    className="px-3 py-1.5 bg-dark-700 text-dark-100 rounded-lg text-xs font-medium hover:bg-dark-600"
                  >
                    Refresh
                  </button>
                  <button
                    onClick={() => openRecordForm()}
                    className="px-3 py-1.5 bg-rust-500 text-white rounded-lg text-xs font-medium hover:bg-rust-600"
                  >
                    Add Record
                  </button>
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
                        className="w-full px-2 py-1.5 border border-dark-500 rounded-md text-sm bg-dark-800 focus:ring-2 focus:ring-rust-500 outline-none disabled:bg-dark-700"
                      >
                        {RECORD_TYPES.map((t) => <option key={t} value={t}>{t}</option>)}
                      </select>
                    </div>
                    <div>
                      <label className="block text-xs font-medium text-dark-200 mb-1">Name</label>
                      <input type="text" value={recName} onChange={(e) => setRecName(e.target.value)} placeholder="@" className="w-full px-2 py-1.5 border border-dark-500 rounded-md text-sm focus:ring-2 focus:ring-rust-500 outline-none" />
                    </div>
                    <div className="col-span-2 sm:col-span-1">
                      <label className="block text-xs font-medium text-dark-200 mb-1">Content</label>
                      <input type="text" value={recContent} onChange={(e) => setRecContent(e.target.value)} placeholder={recType === "A" ? "1.2.3.4" : recType === "CNAME" ? "target.com" : ""} className="w-full px-2 py-1.5 border border-dark-500 rounded-md text-sm focus:ring-2 focus:ring-rust-500 outline-none" />
                    </div>
                    <div>
                      <label className="block text-xs font-medium text-dark-200 mb-1">TTL</label>
                      <select value={recTtl} onChange={(e) => setRecTtl(e.target.value)} className="w-full px-2 py-1.5 border border-dark-500 rounded-md text-sm bg-dark-800 focus:ring-2 focus:ring-rust-500 outline-none">
                        <option value="1">Auto</option>
                        <option value="60">1 min</option>
                        <option value="300">5 min</option>
                        <option value="3600">1 hour</option>
                        <option value="86400">1 day</option>
                      </select>
                    </div>
                    {recType === "MX" && (
                      <div>
                        <label className="block text-xs font-medium text-dark-200 mb-1">Priority</label>
                        <input type="number" value={recPriority} onChange={(e) => setRecPriority(e.target.value)} min="0" max="65535" className="w-full px-2 py-1.5 border border-dark-500 rounded-md text-sm focus:ring-2 focus:ring-rust-500 outline-none" />
                      </div>
                    )}
                  </div>
                  {["A", "AAAA", "CNAME"].includes(recType) && (
                    <label className="flex items-center gap-2 cursor-pointer">
                      <input type="checkbox" checked={recProxied} onChange={(e) => setRecProxied(e.target.checked)} className="w-4 h-4 text-orange-500 border-dark-500 rounded focus:ring-orange-500" />
                      <span className="text-sm text-dark-100">Proxied through Cloudflare (orange cloud)</span>
                    </label>
                  )}
                  <div className="flex items-center gap-2">
                    <button
                      onClick={handleSaveRecord}
                      disabled={savingRecord || !recName || !recContent}
                      className="px-4 py-1.5 bg-rust-500 text-white rounded-lg text-sm font-medium hover:bg-rust-600 disabled:opacity-50"
                    >
                      {savingRecord ? "Saving..." : editingRecord ? "Update" : "Create"}
                    </button>
                    <button
                      onClick={() => setShowRecordForm(false)}
                      className="px-4 py-1.5 bg-dark-600 text-dark-100 rounded-lg text-sm font-medium hover:bg-dark-500"
                    >
                      Cancel
                    </button>
                  </div>
                </div>
              )}

              {/* Records Table */}
              <div className="overflow-x-auto">
                <table className="w-full text-sm">
                  <thead>
                    <tr className="bg-dark-900 text-left">
                      <th className="px-4 py-2.5 text-xs font-medium text-dark-200 uppercase font-mono tracking-widest w-20">Type</th>
                      <th className="px-4 py-2.5 text-xs font-medium text-dark-200 uppercase font-mono tracking-widest">Name</th>
                      <th className="px-4 py-2.5 text-xs font-medium text-dark-200 uppercase font-mono tracking-widest">Content</th>
                      <th className="px-4 py-2.5 text-xs font-medium text-dark-200 uppercase font-mono tracking-widest w-16">TTL</th>
                      <th className="px-4 py-2.5 text-xs font-medium text-dark-200 uppercase font-mono tracking-widest w-16">Proxy</th>
                      <th className="px-4 py-2.5 text-xs font-medium text-dark-200 uppercase font-mono tracking-widest w-20">Actions</th>
                    </tr>
                  </thead>
                  <tbody className="divide-y divide-dark-600">
                    {records.map((r) => (
                      <tr key={r.id} className="hover:bg-dark-700/30 transition-colors">
                        <td className="px-4 py-2.5">
                          <span className={`inline-flex px-2 py-0.5 rounded text-xs font-medium ${typeColor[r.type] || "bg-dark-700 text-dark-200"}`}>
                            {r.type}
                          </span>
                        </td>
                        <td className="px-4 py-2.5 font-mono text-xs text-dark-50 truncate max-w-48">{r.name}</td>
                        <td className="px-4 py-2.5 font-mono text-xs text-dark-200 truncate max-w-64">{r.content}</td>
                        <td className="px-4 py-2.5 text-xs text-dark-200 font-mono">{ttlLabel(r.ttl)}</td>
                        <td className="px-4 py-2.5">
                          {r.proxied !== undefined && (
                            <span className={`inline-block w-3 h-3 rounded-full ${r.proxied ? "bg-orange-400" : "bg-dark-500"}`} title={r.proxied ? "Proxied" : "DNS only"} />
                          )}
                        </td>
                        <td className="px-4 py-2.5">
                          <div className="flex items-center gap-1">
                            <button onClick={() => openRecordForm(r)} className="p-1 text-dark-300 hover:text-indigo-600" title="Edit">
                              <svg className="w-4 h-4" fill="none" viewBox="0 0 24 24" stroke="currentColor" strokeWidth={1.5}>
                                <path strokeLinecap="round" strokeLinejoin="round" d="m16.862 4.487 1.687-1.688a1.875 1.875 0 1 1 2.652 2.652L10.582 16.07a4.5 4.5 0 0 1-1.897 1.13L6 18l.8-2.685a4.5 4.5 0 0 1 1.13-1.897l8.932-8.931Zm0 0L19.5 7.125" />
                              </svg>
                            </button>
                            <button onClick={() => handleDeleteRecord(r)} className="p-1 text-dark-300 hover:text-red-600" title="Delete">
                              <svg className="w-4 h-4" fill="none" viewBox="0 0 24 24" stroke="currentColor" strokeWidth={1.5}>
                                <path strokeLinecap="round" strokeLinejoin="round" d="m14.74 9-.346 9m-4.788 0L9.26 9m9.968-3.21c.342.052.682.107 1.022.166m-1.022-.165L18.16 19.673a2.25 2.25 0 0 1-2.244 2.077H8.084a2.25 2.25 0 0 1-2.244-2.077L4.772 5.79m14.456 0a48.108 48.108 0 0 0-3.478-.397m-12 .562c.34-.059.68-.114 1.022-.165m0 0a48.11 48.11 0 0 1 3.478-.397m7.5 0v-.916c0-1.18-.91-2.164-2.09-2.201a51.964 51.964 0 0 0-3.32 0c-1.18.037-2.09 1.022-2.09 2.201v.916m7.5 0a48.667 48.667 0 0 0-7.5 0" />
                              </svg>
                            </button>
                          </div>
                        </td>
                      </tr>
                    ))}
                    {records.length === 0 && (
                      <tr>
                        <td colSpan={6} className="px-4 py-8 text-center text-dark-300 text-sm">No DNS records found</td>
                      </tr>
                    )}
                  </tbody>
                </table>
              </div>
            </div>
          )}
        </div>
      </div>
    </div>
  );
}
