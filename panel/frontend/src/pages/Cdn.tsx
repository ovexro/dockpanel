import { useState, useEffect } from "react";
import { api } from "../api";
import { formatDate } from "../utils/format";

interface CdnZone {
  id: string;
  domain: string;
  provider: string;
  pull_zone_id: string | null;
  origin_url: string | null;
  cdn_hostname: string | null;
  enabled: boolean;
  cache_ttl: number;
  created_at: string;
  updated_at: string;
}

interface CdnStats {
  provider: string;
  period: string;
  total_bandwidth: number;
  total_requests: number;
  cache_hit_rate?: number;
  cached_bandwidth?: number;
  threats?: number;
  page_views?: number;
}

function formatBytes(bytes: number): string {
  if (bytes === 0) return "0 B";
  const k = 1024;
  const sizes = ["B", "KB", "MB", "GB", "TB"];
  const i = Math.floor(Math.log(bytes) / Math.log(k));
  return `${(bytes / Math.pow(k, i)).toFixed(1)} ${sizes[i]}`;
}

export default function Cdn() {
  const [zones, setZones] = useState<CdnZone[]>([]);
  const [selectedZone, setSelectedZone] = useState<CdnZone | null>(null);
  const [stats, setStats] = useState<CdnStats | null>(null);
  const [loading, setLoading] = useState(true);
  const [message, setMessage] = useState({ text: "", type: "" });
  const [pendingDelete, setPendingDelete] = useState(false);

  // Add zone form
  const [showAddZone, setShowAddZone] = useState(false);
  const [provider, setProvider] = useState<"bunnycdn" | "cloudflare">("bunnycdn");
  const [domain, setDomain] = useState("");
  const [apiKey, setApiKey] = useState("");
  const [pullZoneId, setPullZoneId] = useState("");
  const [originUrl, setOriginUrl] = useState("");
  const [cdnHostname, setCdnHostname] = useState("");
  const [saving, setSaving] = useState(false);

  // Actions
  const [purging, setPurging] = useState(false);
  const [testing, setTesting] = useState(false);
  const [loadingStats, setLoadingStats] = useState(false);

  // Edit settings
  const [editCacheTtl, setEditCacheTtl] = useState(86400);
  const [editEnabled, setEditEnabled] = useState(true);
  const [updatingSettings, setUpdatingSettings] = useState(false);

  const load = async () => {
    try {
      const data = await api.get<CdnZone[]>("/cdn/zones");
      setZones(data);
      if (data.length > 0 && !selectedZone) {
        setSelectedZone(data[0]);
      } else if (selectedZone) {
        const updated = data.find(z => z.id === selectedZone.id);
        if (updated) setSelectedZone(updated);
      }
    } catch {
      // empty
    } finally {
      setLoading(false);
    }
  };

  useEffect(() => { load(); }, []);

  useEffect(() => {
    if (selectedZone) {
      setEditCacheTtl(selectedZone.cache_ttl);
      setEditEnabled(selectedZone.enabled);
      loadStats(selectedZone.id);
    }
  }, [selectedZone?.id]);

  const loadStats = async (zoneId: string) => {
    setLoadingStats(true);
    setStats(null);
    try {
      const data = await api.get<CdnStats>(`/cdn/zones/${zoneId}/stats`);
      setStats(data);
    } catch {
      // Stats may not be available yet
    } finally {
      setLoadingStats(false);
    }
  };

  const handleCreate = async () => {
    if (!domain.trim() || !apiKey.trim()) return;
    setSaving(true);
    setMessage({ text: "", type: "" });
    try {
      const zone = await api.post<CdnZone>("/cdn/zones", {
        domain: domain.trim(),
        provider,
        api_key: apiKey.trim(),
        pull_zone_id: pullZoneId.trim() || undefined,
        origin_url: originUrl.trim() || undefined,
        cdn_hostname: cdnHostname.trim() || undefined,
      });
      setShowAddZone(false);
      setDomain("");
      setApiKey("");
      setPullZoneId("");
      setOriginUrl("");
      setCdnHostname("");
      setMessage({ text: "CDN zone added.", type: "success" });
      await load();
      setSelectedZone({ ...zone, id: zone.id });
    } catch (err) {
      setMessage({ text: err instanceof Error ? err.message : "Failed to add CDN zone", type: "error" });
    } finally {
      setSaving(false);
    }
  };

  const handleDelete = () => {
    if (!selectedZone) return;
    setPendingDelete(true);
  };

  const executeDelete = async () => {
    if (!selectedZone) return;
    setPendingDelete(false);
    try {
      await api.delete(`/cdn/zones/${selectedZone.id}`);
      setSelectedZone(null);
      setStats(null);
      setMessage({ text: "CDN zone removed.", type: "success" });
      await load();
    } catch (err) {
      setMessage({ text: err instanceof Error ? err.message : "Delete failed", type: "error" });
    }
  };

  const handlePurge = async () => {
    if (!selectedZone) return;
    setPurging(true);
    setMessage({ text: "", type: "" });
    try {
      await api.post(`/cdn/zones/${selectedZone.id}/purge`, {});
      setMessage({ text: "Cache purged successfully.", type: "success" });
    } catch (err) {
      setMessage({ text: err instanceof Error ? err.message : "Purge failed", type: "error" });
    } finally {
      setPurging(false);
    }
  };

  const handleTest = async () => {
    if (!selectedZone) return;
    setTesting(true);
    setMessage({ text: "", type: "" });
    try {
      const result = await api.post<{ ok: boolean; message: string }>(`/cdn/zones/${selectedZone.id}/test`);
      setMessage({ text: result.message, type: result.ok ? "success" : "error" });
    } catch (err) {
      setMessage({ text: err instanceof Error ? err.message : "Test failed", type: "error" });
    } finally {
      setTesting(false);
    }
  };

  const handleUpdateSettings = async () => {
    if (!selectedZone) return;
    setUpdatingSettings(true);
    setMessage({ text: "", type: "" });
    try {
      const updated = await api.put<CdnZone>(`/cdn/zones/${selectedZone.id}`, {
        enabled: editEnabled,
        cache_ttl: editCacheTtl,
      });
      setSelectedZone(updated);
      setMessage({ text: "Settings updated.", type: "success" });
      await load();
    } catch (err) {
      setMessage({ text: err instanceof Error ? err.message : "Update failed", type: "error" });
    } finally {
      setUpdatingSettings(false);
    }
  };

  const ttlOptions = [
    { label: "No Cache", value: 0 },
    { label: "1 hour", value: 3600 },
    { label: "12 hours", value: 43200 },
    { label: "1 day", value: 86400 },
    { label: "7 days", value: 604800 },
    { label: "30 days", value: 2592000 },
    { label: "1 year", value: 31536000 },
  ];

  if (loading) {
    return (
      <div className="p-6 lg:p-8">
        <div className="bg-dark-800 rounded-lg border border-dark-500 p-6 animate-pulse">
          <div className="h-6 bg-dark-700 rounded w-48 mb-4" />
          <div className="h-4 bg-dark-700 rounded w-32" />
        </div>
      </div>
    );
  }

  return (
    <div className="p-6 lg:p-8 space-y-6">
      <div className="flex flex-col sm:flex-row sm:items-center justify-between gap-3">
        <div>
          <h1 className="text-sm font-medium text-dark-300 uppercase font-mono tracking-widest">CDN</h1>
          <p className="text-sm text-dark-200 mt-1">Manage CDN zones, purge cache, and view bandwidth stats</p>
        </div>
        <button
          onClick={() => setShowAddZone(true)}
          className="px-4 py-2 bg-rust-500 text-white rounded-lg text-sm font-medium hover:bg-rust-600 transition-colors"
        >
          Add CDN Zone
        </button>
      </div>

      {/* Message */}
      {message.text && (
        <div className={`px-4 py-3 rounded-lg text-sm border ${
          message.type === "success"
            ? "bg-rust-500/10 text-rust-400 border-rust-500/20"
            : "bg-danger-500/10 text-danger-400 border-danger-500/20"
        }`}>
          {message.text}
        </div>
      )}

      {/* Confirm delete bar */}
      {pendingDelete && selectedZone && (
        <div className="border border-danger-500/30 bg-danger-500/5 rounded-lg px-4 py-3 flex items-center justify-between">
          <span className="text-xs text-danger-400 font-mono">Remove CDN zone for {selectedZone.domain}?</span>
          <div className="flex items-center gap-2 shrink-0 ml-4">
            <button onClick={executeDelete} className="px-3 py-1.5 bg-danger-500 text-white text-xs font-bold uppercase tracking-wider hover:bg-danger-400 transition-colors">Confirm</button>
            <button onClick={() => setPendingDelete(false)} className="px-3 py-1.5 bg-dark-600 text-dark-200 text-xs font-bold uppercase tracking-wider hover:bg-dark-500 transition-colors">Cancel</button>
          </div>
        </div>
      )}

      {/* Add Zone Form */}
      {showAddZone && (
        <div className="bg-dark-800 rounded-lg border border-dark-500 overflow-hidden">
          <div className="px-5 py-4 border-b border-dark-600 flex items-center justify-between">
            <h2 className="text-xs font-medium text-dark-300 uppercase font-mono tracking-widest">Add CDN Zone</h2>
            <button onClick={() => setShowAddZone(false)} className="text-dark-300 hover:text-dark-100">
              <svg className="w-5 h-5" fill="none" viewBox="0 0 24 24" stroke="currentColor" strokeWidth={2}>
                <path strokeLinecap="round" strokeLinejoin="round" d="M6 18L18 6M6 6l12 12" />
              </svg>
            </button>
          </div>
          <div className="p-5 space-y-4">
            <div className="grid grid-cols-2 gap-4">
              <div>
                <label className="block text-sm font-medium text-dark-100 mb-1">Provider</label>
                <select
                  value={provider}
                  onChange={(e) => setProvider(e.target.value as "bunnycdn" | "cloudflare")}
                  className="w-full px-3 py-2 border border-dark-500 rounded-lg text-sm focus:ring-2 focus:ring-accent-500 outline-none"
                >
                  <option value="bunnycdn">BunnyCDN</option>
                  <option value="cloudflare">Cloudflare</option>
                </select>
              </div>
              <div>
                <label className="block text-sm font-medium text-dark-100 mb-1">Domain</label>
                <input
                  type="text"
                  value={domain}
                  onChange={(e) => setDomain(e.target.value)}
                  placeholder="example.com"
                  className="w-full px-3 py-2 border border-dark-500 rounded-lg text-sm focus:ring-2 focus:ring-accent-500 outline-none"
                />
              </div>
            </div>
            <div>
              <label className="block text-sm font-medium text-dark-100 mb-1">API Key</label>
              <input
                type="password"
                value={apiKey}
                onChange={(e) => setApiKey(e.target.value)}
                placeholder={provider === "bunnycdn" ? "BunnyCDN Account API Key" : "Cloudflare API Token"}
                className="w-full px-3 py-2 border border-dark-500 rounded-lg text-sm focus:ring-2 focus:ring-accent-500 outline-none"
              />
            </div>
            <div className="grid grid-cols-2 gap-4">
              <div>
                <label className="block text-sm font-medium text-dark-100 mb-1">
                  {provider === "bunnycdn" ? "Pull Zone ID" : "Zone ID"}
                </label>
                <input
                  type="text"
                  value={pullZoneId}
                  onChange={(e) => setPullZoneId(e.target.value)}
                  placeholder={provider === "bunnycdn" ? "123456" : "zone_id from Cloudflare"}
                  className="w-full px-3 py-2 border border-dark-500 rounded-lg text-sm focus:ring-2 focus:ring-accent-500 outline-none"
                />
              </div>
              <div>
                <label className="block text-sm font-medium text-dark-100 mb-1">Origin URL</label>
                <input
                  type="text"
                  value={originUrl}
                  onChange={(e) => setOriginUrl(e.target.value)}
                  placeholder="https://origin.example.com"
                  className="w-full px-3 py-2 border border-dark-500 rounded-lg text-sm focus:ring-2 focus:ring-accent-500 outline-none"
                />
              </div>
            </div>
            {provider === "bunnycdn" && (
              <div>
                <label className="block text-sm font-medium text-dark-100 mb-1">CDN Hostname</label>
                <input
                  type="text"
                  value={cdnHostname}
                  onChange={(e) => setCdnHostname(e.target.value)}
                  placeholder="cdn.example.com or example.b-cdn.net"
                  className="w-full px-3 py-2 border border-dark-500 rounded-lg text-sm focus:ring-2 focus:ring-accent-500 outline-none"
                />
              </div>
            )}
            <div className="flex items-center gap-3">
              <button
                onClick={handleCreate}
                disabled={saving || !domain.trim() || !apiKey.trim()}
                className="px-4 py-2 bg-rust-500 text-white rounded-lg text-sm font-medium hover:bg-rust-600 disabled:opacity-50 transition-colors"
              >
                {saving ? "Adding..." : "Add Zone"}
              </button>
              <button
                onClick={() => setShowAddZone(false)}
                className="px-4 py-2 bg-dark-700 text-dark-100 rounded-lg text-sm font-medium hover:bg-dark-600 transition-colors"
              >
                Cancel
              </button>
            </div>
          </div>
        </div>
      )}

      <div className="grid grid-cols-1 lg:grid-cols-4 gap-6">
        {/* Zone List */}
        <div className="lg:col-span-1">
          <div className="bg-dark-800 rounded-lg border border-dark-500 overflow-hidden">
            <div className="px-4 py-3 border-b border-dark-600">
              <h2 className="text-xs font-medium text-dark-300 uppercase font-mono tracking-widest">Zones</h2>
            </div>
            {zones.length === 0 ? (
              <div className="p-4 text-center text-sm text-dark-300">
                No CDN zones configured.
              </div>
            ) : (
              <div className="divide-y divide-dark-600">
                {zones.map((zone) => (
                  <button
                    key={zone.id}
                    onClick={() => setSelectedZone(zone)}
                    className={`w-full px-4 py-3 text-left hover:bg-dark-700/50 transition-colors ${
                      selectedZone?.id === zone.id ? "bg-dark-700/50 border-l-2 border-rust-500" : ""
                    }`}
                  >
                    <div className="text-sm text-dark-50 font-medium">{zone.domain}</div>
                    <div className="flex items-center gap-2 mt-1">
                      <span className={`inline-flex px-1.5 py-0.5 rounded text-xs font-medium ${
                        zone.provider === "bunnycdn"
                          ? "bg-amber-500/15 text-amber-400"
                          : "bg-orange-500/15 text-orange-400"
                      }`}>
                        {zone.provider === "bunnycdn" ? "Bunny" : "CF"}
                      </span>
                      <span className={`w-1.5 h-1.5 rounded-full ${zone.enabled ? "bg-green-400" : "bg-dark-400"}`} />
                    </div>
                  </button>
                ))}
              </div>
            )}
          </div>
        </div>

        {/* Zone Detail */}
        <div className="lg:col-span-3 space-y-6">
          {selectedZone ? (
            <>
              {/* Overview */}
              <div className="bg-dark-800 rounded-lg border border-dark-500 overflow-hidden">
                <div className="px-5 py-4 border-b border-dark-600 flex items-center justify-between">
                  <div>
                    <h2 className="text-sm font-medium text-dark-50">{selectedZone.domain}</h2>
                    <p className="text-xs text-dark-300 mt-0.5">
                      {selectedZone.provider === "bunnycdn" ? "BunnyCDN" : "Cloudflare"} &middot;
                      {selectedZone.pull_zone_id ? ` Zone ${selectedZone.pull_zone_id}` : " No zone ID"} &middot;
                      {selectedZone.enabled ? " Active" : " Disabled"}
                    </p>
                  </div>
                  <div className="flex items-center gap-2">
                    <button
                      onClick={handleTest}
                      disabled={testing}
                      className="px-3 py-1.5 bg-dark-700 text-dark-100 rounded-lg text-xs font-medium hover:bg-dark-600 disabled:opacity-50 transition-colors"
                    >
                      {testing ? "Testing..." : "Test API"}
                    </button>
                    <button
                      onClick={handlePurge}
                      disabled={purging}
                      className="px-3 py-1.5 bg-rust-500 text-white rounded-lg text-xs font-medium hover:bg-rust-600 disabled:opacity-50 transition-colors"
                    >
                      {purging ? "Purging..." : "Purge Cache"}
                    </button>
                    <button
                      onClick={handleDelete}
                      className="px-3 py-1.5 bg-danger-500/10 text-danger-400 rounded-lg text-xs font-medium hover:bg-danger-500/20 transition-colors"
                    >
                      Remove
                    </button>
                  </div>
                </div>

                {/* Stats */}
                <div className="p-5">
                  {loadingStats ? (
                    <div className="grid grid-cols-2 sm:grid-cols-4 gap-4">
                      {[1,2,3,4].map(i => (
                        <div key={i} className="animate-pulse">
                          <div className="h-3 bg-dark-700 rounded w-20 mb-2" />
                          <div className="h-6 bg-dark-700 rounded w-16" />
                        </div>
                      ))}
                    </div>
                  ) : stats ? (
                    <div className="grid grid-cols-2 sm:grid-cols-4 gap-4">
                      <div>
                        <p className="text-xs text-dark-300 uppercase font-mono">Bandwidth</p>
                        <p className="text-lg font-medium text-dark-50">{formatBytes(stats.total_bandwidth || 0)}</p>
                      </div>
                      <div>
                        <p className="text-xs text-dark-300 uppercase font-mono">Requests</p>
                        <p className="text-lg font-medium text-dark-50">{(stats.total_requests || 0).toLocaleString()}</p>
                      </div>
                      {stats.cache_hit_rate !== undefined && (
                        <div>
                          <p className="text-xs text-dark-300 uppercase font-mono">Cache Hit Rate</p>
                          <p className="text-lg font-medium text-dark-50">{(stats.cache_hit_rate * 100).toFixed(1)}%</p>
                        </div>
                      )}
                      {stats.cached_bandwidth !== undefined && (
                        <div>
                          <p className="text-xs text-dark-300 uppercase font-mono">Cached</p>
                          <p className="text-lg font-medium text-dark-50">{formatBytes(stats.cached_bandwidth)}</p>
                        </div>
                      )}
                      {stats.page_views !== undefined && (
                        <div>
                          <p className="text-xs text-dark-300 uppercase font-mono">Page Views</p>
                          <p className="text-lg font-medium text-dark-50">{(stats.page_views || 0).toLocaleString()}</p>
                        </div>
                      )}
                      {stats.threats !== undefined && stats.threats > 0 && (
                        <div>
                          <p className="text-xs text-dark-300 uppercase font-mono">Threats Blocked</p>
                          <p className="text-lg font-medium text-danger-400">{stats.threats.toLocaleString()}</p>
                        </div>
                      )}
                    </div>
                  ) : (
                    <p className="text-sm text-dark-300">
                      {selectedZone.pull_zone_id ? "No stats available yet." : "Configure a Pull Zone ID to view statistics."}
                    </p>
                  )}
                  <p className="text-xs text-dark-400 mt-3">Last 30 days</p>
                </div>
              </div>

              {/* Settings */}
              <div className="bg-dark-800 rounded-lg border border-dark-500 overflow-hidden">
                <div className="px-5 py-4 border-b border-dark-600">
                  <h2 className="text-xs font-medium text-dark-300 uppercase font-mono tracking-widest">Settings</h2>
                </div>
                <div className="p-5 space-y-4">
                  <div className="flex items-center justify-between">
                    <div>
                      <h3 className="text-sm font-medium text-dark-100">Enabled</h3>
                      <p className="text-xs text-dark-300">Toggle CDN for this zone</p>
                    </div>
                    <label className="relative inline-flex items-center cursor-pointer">
                      <input
                        type="checkbox"
                        checked={editEnabled}
                        onChange={(e) => setEditEnabled(e.target.checked)}
                        className="sr-only peer"
                      />
                      <div className="w-11 h-6 bg-dark-600 peer-focus:ring-2 peer-focus:ring-accent-500 rounded-full peer peer-checked:after:translate-x-full peer-checked:after:border-white after:content-[''] after:absolute after:top-[2px] after:left-[2px] after:bg-white after:border-gray-300 after:border after:rounded-full after:h-5 after:w-5 after:transition-all peer-checked:bg-rust-500" />
                    </label>
                  </div>
                  <div>
                    <label className="block text-sm font-medium text-dark-100 mb-1">Cache TTL</label>
                    <select
                      value={editCacheTtl}
                      onChange={(e) => setEditCacheTtl(parseInt(e.target.value))}
                      className="w-full sm:w-48 px-3 py-2 border border-dark-500 rounded-lg text-sm focus:ring-2 focus:ring-accent-500 outline-none"
                    >
                      {ttlOptions.map((opt) => (
                        <option key={opt.value} value={opt.value}>{opt.label}</option>
                      ))}
                    </select>
                  </div>
                  <div className="grid grid-cols-2 gap-4">
                    <div>
                      <label className="block text-sm font-medium text-dark-100 mb-1">Origin URL</label>
                      <p className="text-sm text-dark-200 font-mono">{selectedZone.origin_url || "—"}</p>
                    </div>
                    <div>
                      <label className="block text-sm font-medium text-dark-100 mb-1">CDN Hostname</label>
                      <p className="text-sm text-dark-200 font-mono">{selectedZone.cdn_hostname || "—"}</p>
                    </div>
                  </div>
                  <button
                    onClick={handleUpdateSettings}
                    disabled={updatingSettings}
                    className="px-4 py-2 bg-rust-500 text-white rounded-lg text-sm font-medium hover:bg-rust-600 disabled:opacity-50 transition-colors"
                  >
                    {updatingSettings ? "Saving..." : "Save Settings"}
                  </button>
                </div>
              </div>

              {/* Info */}
              <div className="bg-dark-800 rounded-lg border border-dark-500 overflow-hidden">
                <div className="px-5 py-4 border-b border-dark-600">
                  <h2 className="text-xs font-medium text-dark-300 uppercase font-mono tracking-widest">Zone Details</h2>
                </div>
                <div className="p-5">
                  <dl className="grid grid-cols-2 gap-3 text-sm">
                    <div>
                      <dt className="text-dark-300">Provider</dt>
                      <dd className="text-dark-50">{selectedZone.provider === "bunnycdn" ? "BunnyCDN" : "Cloudflare"}</dd>
                    </div>
                    <div>
                      <dt className="text-dark-300">Zone ID</dt>
                      <dd className="text-dark-50 font-mono">{selectedZone.pull_zone_id || "—"}</dd>
                    </div>
                    <div>
                      <dt className="text-dark-300">Added</dt>
                      <dd className="text-dark-50">{formatDate(selectedZone.created_at)}</dd>
                    </div>
                    <div>
                      <dt className="text-dark-300">Updated</dt>
                      <dd className="text-dark-50">{formatDate(selectedZone.updated_at)}</dd>
                    </div>
                  </dl>
                </div>
              </div>
            </>
          ) : (
            <div className="bg-dark-800 rounded-lg border border-dark-500 p-12 text-center">
              <svg className="w-12 h-12 text-dark-400 mx-auto mb-4" fill="none" viewBox="0 0 24 24" stroke="currentColor" strokeWidth={1.5}>
                <path strokeLinecap="round" strokeLinejoin="round" d="M12 21a9.004 9.004 0 008.716-6.747M12 21a9.004 9.004 0 01-8.716-6.747M12 21c2.485 0 4.5-4.03 4.5-9S14.485 3 12 3m0 18c-2.485 0-4.5-4.03-4.5-9S9.515 3 12 3m0 0a8.997 8.997 0 017.843 4.582M12 3a8.997 8.997 0 00-7.843 4.582m15.686 0A11.953 11.953 0 0112 10.5c-2.998 0-5.74-1.1-7.843-2.918m15.686 0A8.959 8.959 0 0121 12c0 .778-.099 1.533-.284 2.253m0 0A17.919 17.919 0 0112 16.5c-3.162 0-6.133-.815-8.716-2.247m0 0A9.015 9.015 0 013 12c0-1.605.42-3.113 1.157-4.418" />
              </svg>
              <h3 className="text-dark-200 text-sm font-medium">Select a CDN zone or add a new one</h3>
              <p className="text-dark-300 text-xs mt-1">Supports BunnyCDN and Cloudflare CDN</p>
            </div>
          )}
        </div>
      </div>
    </div>
  );
}
