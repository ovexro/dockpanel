import { useAuth } from "../context/AuthContext";
import { Navigate } from "react-router-dom";
import { useState, useEffect, useCallback } from "react";
import { api, ApiError } from "../api";
import { Link } from "react-router-dom";

interface WPSite {
  site_id: string;
  domain: string;
  wp_version: string;
  update_available: boolean;
  vulns: number;
  critical_vulns: number;
}

interface SecurityItem {
  name: string;
  label: string;
  description?: string;
  status: string;
  auto_fixable: boolean;
  severity?: string;
}

interface VulnItem {
  severity: string;
  title?: string;
  name?: string;
}

interface VulnScanResult {
  total_vulns: number;
  critical_count: number;
  high_count: number;
  vulnerabilities: VulnItem[];
}

interface SecurityCheckResponse {
  checks?: SecurityItem[];
}

export default function WordPressToolkit() {
  const { user } = useAuth();
  if (!user || user.role !== "admin") return <Navigate to="/" replace />;
  const [tab, setTab] = useState<"overview" | "security">("overview");
  const [sites, setSites] = useState<WPSite[]>([]);
  const [loading, setLoading] = useState(true);
  const [selected, setSelected] = useState<Set<string>>(new Set());
  const [updating, setUpdating] = useState(false);
  const [scanning, setScanning] = useState<string | null>(null);
  const [scanResults, setScanResults] = useState<Record<string, VulnScanResult>>({});
  const [securityChecks, setSecurityChecks] = useState<Record<string, SecurityItem[]>>({});
  const [checkingAll, setCheckingAll] = useState(false);
  const [hardening, setHardening] = useState<string | null>(null);
  const [error, setError] = useState("");
  const [success, setSuccess] = useState("");

  // Fetch WP sites
  const fetchSites = useCallback(async () => {
    try {
      const data = await api.get<WPSite[]>("/wordpress/sites");
      setSites(data);
    } catch (e) {
      setError(e instanceof ApiError ? e.message : "Failed to load WordPress sites");
    } finally {
      setLoading(false);
    }
  }, []);

  useEffect(() => {
    fetchSites();
  }, [fetchSites]);

  // Bulk update
  const handleBulkUpdate = async (target: string) => {
    if (selected.size === 0) return;
    setUpdating(true);
    setError("");
    try {
      await api.post("/wordpress/bulk-update", {
        site_ids: Array.from(selected),
        target,
      });
      await fetchSites();
      setError("");
      setSuccess(`Updated ${selected.size} site(s) successfully`);
      setSelected(new Set());
    } catch (e) {
      setError(e instanceof ApiError ? e.message : "Bulk update failed");
    } finally {
      setUpdating(false);
    }
  };

  // Vuln scan
  const handleScan = async (siteId: string) => {
    setScanning(siteId);
    setError("");
    try {
      const result = await api.post<VulnScanResult>(`/sites/${siteId}/wordpress/vuln-scan`);
      setScanResults((prev) => ({ ...prev, [siteId]: result }));
      await fetchSites();
    } catch (e) {
      setError(e instanceof ApiError ? e.message : "Vulnerability scan failed");
    } finally {
      setScanning(null);
    }
  };

  // Security check for one site
  const handleSecurityCheck = async (siteId: string) => {
    try {
      const result = await api.get<SecurityCheckResponse>(`/sites/${siteId}/wordpress/security-check`);
      const checks: SecurityItem[] = Array.isArray(result?.checks) ? result.checks : [];
      setSecurityChecks((prev) => ({ ...prev, [siteId]: checks }));
    } catch (e) {
      setError(e instanceof ApiError ? e.message : "Security check failed");
    }
  };

  // Check all sites
  const handleCheckAll = async () => {
    setCheckingAll(true);
    for (const site of sites) {
      await handleSecurityCheck(site.site_id);
    }
    setCheckingAll(false);
  };

  // Apply hardening
  const handleHarden = async (siteId: string, fixes: string[]) => {
    setHardening(siteId);
    setError("");
    try {
      await api.post(`/sites/${siteId}/wordpress/harden`, { fixes });
      setSuccess("Hardening applied successfully");
      // Re-check after hardening
      await handleSecurityCheck(siteId);
    } catch (e) {
      setError(e instanceof ApiError ? e.message : "Hardening failed");
    } finally {
      setHardening(null);
    }
  };

  // Toggle selection
  const toggleSelect = (siteId: string) => {
    setSelected((prev) => {
      const next = new Set(prev);
      if (next.has(siteId)) next.delete(siteId);
      else next.add(siteId);
      return next;
    });
  };

  const toggleSelectAll = () => {
    if (selected.size === sites.length) {
      setSelected(new Set());
    } else {
      setSelected(new Set(sites.map((s) => s.site_id)));
    }
  };

  if (loading) {
    return (
      <div className="p-6 lg:p-8">
        <h1 className="text-sm font-medium text-dark-300 uppercase font-mono tracking-widest mb-6">
          WordPress Toolkit
        </h1>
        <div className="bg-dark-800 rounded-lg border border-dark-500 p-6 animate-pulse">
          <div className="h-6 bg-dark-700 rounded w-48 mb-4" />
          <div className="h-4 bg-dark-700 rounded w-32" />
        </div>
      </div>
    );
  }

  const totalVulns = sites.reduce((acc, s) => acc + s.vulns, 0);
  const criticalVulns = sites.reduce((acc, s) => acc + s.critical_vulns, 0);
  const updatesAvailable = sites.filter((s) => s.update_available).length;

  return (
    <div className="p-6 lg:p-8">
      {/* Header */}
      <div className="flex items-center justify-between gap-3 mb-6 pb-4 border-b border-dark-600">
        <div>
          <h1 className="text-sm font-medium text-dark-300 uppercase font-mono tracking-widest">
            WordPress Toolkit
          </h1>
          <p className="text-sm text-dark-200 font-mono mt-0.5">
            Manage all WordPress sites from one place
          </p>
        </div>
        <div className="flex items-center gap-3 text-xs font-mono">
          <span className="text-dark-200">
            {sites.length} site{sites.length !== 1 ? "s" : ""}
          </span>
          {updatesAvailable > 0 && (
            <span className="px-2 py-0.5 bg-warn-500/15 text-warn-400 rounded">
              {updatesAvailable} update{updatesAvailable !== 1 ? "s" : ""}
            </span>
          )}
          {criticalVulns > 0 && (
            <span className="px-2 py-0.5 bg-danger-500/15 text-danger-400 rounded">
              {criticalVulns} critical
            </span>
          )}
        </div>
      </div>

      {/* Success */}
      {success && (
        <div className="mb-4 px-4 py-3 rounded-lg text-sm border bg-rust-500/10 text-rust-400 border-rust-500/20 flex items-center justify-between">
          <span>{success}</span>
          <button onClick={() => setSuccess("")} className="text-rust-400 hover:text-rust-300 ml-4">
            <svg className="w-4 h-4" fill="none" viewBox="0 0 24 24" stroke="currentColor" strokeWidth={2}>
              <path strokeLinecap="round" strokeLinejoin="round" d="M6 18 18 6M6 6l12 12" />
            </svg>
          </button>
        </div>
      )}
      {/* Error */}
      {error && (
        <div className="mb-4 px-4 py-3 rounded-lg text-sm border bg-danger-500/10 text-danger-400 border-danger-500/20 flex items-center justify-between">
          <span>{error}</span>
          <button onClick={() => setError("")} className="text-danger-400 hover:text-danger-300 ml-4">
            <svg className="w-4 h-4" fill="none" viewBox="0 0 24 24" stroke="currentColor" strokeWidth={2}>
              <path strokeLinecap="round" strokeLinejoin="round" d="M6 18 18 6M6 6l12 12" />
            </svg>
          </button>
        </div>
      )}

      {/* Tabs */}
      <div className="flex gap-1 mb-6">
        <button
          onClick={() => setTab("overview")}
          className={`px-4 py-2 rounded-lg text-sm font-medium transition-colors ${
            tab === "overview"
              ? "bg-rust-500 text-white"
              : "bg-dark-700 text-dark-200 hover:bg-dark-600"
          }`}
        >
          Overview
        </button>
        <button
          onClick={() => setTab("security")}
          className={`px-4 py-2 rounded-lg text-sm font-medium transition-colors ${
            tab === "security"
              ? "bg-rust-500 text-white"
              : "bg-dark-700 text-dark-200 hover:bg-dark-600"
          }`}
        >
          Security
        </button>
      </div>

      {/* No sites */}
      {sites.length === 0 && (
        <div className="bg-dark-800 rounded-lg border border-dark-500 p-12 text-center">
          <div className="w-16 h-16 bg-dark-700 rounded-2xl flex items-center justify-center mx-auto mb-4">
            <svg className="w-8 h-8 text-dark-300" fill="none" viewBox="0 0 24 24" stroke="currentColor" strokeWidth={1.5}>
              <path strokeLinecap="round" strokeLinejoin="round" d="M12 21a9.004 9.004 0 0 0 8.716-6.747M12 21a9.004 9.004 0 0 1-8.716-6.747M12 21c2.485 0 4.5-4.03 4.5-9S14.485 3 12 3m0 18c-2.485 0-4.5-4.03-4.5-9S9.515 3 12 3m0 0a8.997 8.997 0 0 1 7.843 4.582M12 3a8.997 8.997 0 0 0-7.843 4.582m15.686 0A11.953 11.953 0 0 1 12 10.5c-2.998 0-5.74-1.1-7.843-2.918m15.686 0A8.959 8.959 0 0 1 21 12c0 .778-.099 1.533-.284 2.253m0 0A17.919 17.919 0 0 1 12 16.5a17.92 17.92 0 0 1-8.716-2.247m0 0A8.966 8.966 0 0 1 3 12c0-1.264.26-2.466.732-3.558" />
            </svg>
          </div>
          <h2 className="text-lg font-semibold text-dark-50 mb-2">No WordPress Sites</h2>
          <p className="text-sm text-dark-200 mb-4">
            No WordPress installations detected on your sites. Install WordPress on a site to see it here.
          </p>
          <Link
            to="/sites"
            className="inline-flex px-4 py-2 bg-rust-500 text-white rounded-lg text-sm font-medium hover:bg-rust-600"
          >
            Go to Sites
          </Link>
        </div>
      )}

      {/* Overview Tab */}
      {tab === "overview" && sites.length > 0 && (
        <>
          {/* Summary cards */}
          <div className="grid grid-cols-1 sm:grid-cols-3 gap-4 mb-6">
            <div className="bg-dark-800 rounded-lg border border-dark-500 p-4">
              <div className="text-xs text-dark-300 uppercase font-mono tracking-widest mb-1">
                WordPress Sites
              </div>
              <div className="text-2xl font-bold text-dark-50 font-mono">{sites.length}</div>
            </div>
            <div className="bg-dark-800 rounded-lg border border-dark-500 p-4">
              <div className="text-xs text-dark-300 uppercase font-mono tracking-widest mb-1">
                Updates Available
              </div>
              <div className={`text-2xl font-bold font-mono ${updatesAvailable > 0 ? "text-warn-400" : "text-dark-50"}`}>
                {updatesAvailable}
              </div>
            </div>
            <div className="bg-dark-800 rounded-lg border border-dark-500 p-4">
              <div className="text-xs text-dark-300 uppercase font-mono tracking-widest mb-1">
                Vulnerabilities
              </div>
              <div className={`text-2xl font-bold font-mono ${criticalVulns > 0 ? "text-danger-400" : totalVulns > 0 ? "text-warn-400" : "text-dark-50"}`}>
                {totalVulns}
                {criticalVulns > 0 && (
                  <span className="text-sm text-danger-400 ml-2">({criticalVulns} critical)</span>
                )}
              </div>
            </div>
          </div>

          {/* Bulk actions */}
          {selected.size > 0 && (
            <div className="bg-dark-800 rounded-lg border border-rust-500/30 p-3 mb-4 flex items-center gap-3 flex-wrap">
              <span className="text-sm text-dark-200 font-mono">
                {selected.size} selected
              </span>
              <div className="flex items-center gap-2">
                <button
                  onClick={() => handleBulkUpdate("plugins")}
                  disabled={updating}
                  className="px-3 py-1.5 bg-rust-500 text-white rounded-lg text-xs font-medium hover:bg-rust-600 disabled:opacity-50"
                >
                  {updating ? "Updating..." : "Update Plugins"}
                </button>
                <button
                  onClick={() => handleBulkUpdate("themes")}
                  disabled={updating}
                  className="px-3 py-1.5 bg-rust-500 text-white rounded-lg text-xs font-medium hover:bg-rust-600 disabled:opacity-50"
                >
                  Update Themes
                </button>
                <button
                  onClick={() => handleBulkUpdate("core")}
                  disabled={updating}
                  className="px-3 py-1.5 bg-warn-500 text-white rounded-lg text-xs font-medium hover:bg-warn-600 disabled:opacity-50"
                >
                  Update Core
                </button>
                <button
                  onClick={() => handleBulkUpdate("all")}
                  disabled={updating}
                  className="px-3 py-1.5 bg-accent-600 text-white rounded-lg text-xs font-medium hover:bg-accent-700 disabled:opacity-50"
                >
                  Update All
                </button>
              </div>
              <button
                onClick={() => setSelected(new Set())}
                className="ml-auto text-xs text-dark-300 hover:text-dark-100"
              >
                Clear
              </button>
            </div>
          )}

          {/* Site grid */}
          <div className="grid grid-cols-1 md:grid-cols-2 xl:grid-cols-3 gap-4">
            {/* Select all header */}
            <div className="col-span-full flex items-center gap-2 mb-1">
              <label className="flex items-center gap-2 cursor-pointer">
                <input
                  type="checkbox"
                  checked={selected.size === sites.length && sites.length > 0}
                  onChange={toggleSelectAll}
                  className="w-4 h-4 rounded border-dark-500 bg-dark-700 text-rust-500 focus:ring-rust-500 focus:ring-offset-0"
                />
                <span className="text-xs text-dark-300 font-mono">Select all</span>
              </label>
            </div>

            {sites.map((site) => (
              <div
                key={site.site_id}
                className={`bg-dark-800 rounded-lg border p-4 transition-colors ${
                  selected.has(site.site_id)
                    ? "border-rust-500/50"
                    : "border-dark-500 hover:border-dark-400"
                }`}
              >
                <div className="flex items-start gap-3">
                  <input
                    type="checkbox"
                    checked={selected.has(site.site_id)}
                    onChange={() => toggleSelect(site.site_id)}
                    className="mt-1 w-4 h-4 rounded border-dark-500 bg-dark-700 text-rust-500 focus:ring-rust-500 focus:ring-offset-0"
                  />
                  <div className="flex-1 min-w-0">
                    <div className="flex items-center gap-2 mb-1">
                      <h3 className="text-sm font-semibold text-dark-50 truncate">
                        {site.domain}
                      </h3>
                      {site.update_available && (
                        <span className="shrink-0 px-1.5 py-0.5 bg-warn-500/15 text-warn-400 rounded text-[10px] font-medium">
                          UPDATE
                        </span>
                      )}
                    </div>
                    <div className="flex items-center gap-3 text-xs text-dark-200 font-mono mb-3">
                      <span>WP {site.wp_version}</span>
                      {site.vulns > 0 && (
                        <span className={site.critical_vulns > 0 ? "text-danger-400" : "text-warn-400"}>
                          {site.vulns} vuln{site.vulns !== 1 ? "s" : ""}
                        </span>
                      )}
                      {site.vulns === 0 && (
                        <span className="text-dark-400">No vulns</span>
                      )}
                    </div>
                    <div className="flex items-center gap-2">
                      <Link
                        to={`/sites/${site.site_id}/wordpress`}
                        className="px-2.5 py-1 bg-dark-700 text-dark-100 rounded text-xs font-medium hover:bg-dark-600"
                      >
                        Manage
                      </Link>
                      <button
                        onClick={() => handleScan(site.site_id)}
                        disabled={scanning === site.site_id}
                        className="px-2.5 py-1 bg-dark-700 text-dark-100 rounded text-xs font-medium hover:bg-dark-600 disabled:opacity-50"
                      >
                        {scanning === site.site_id ? (
                          <span className="flex items-center gap-1">
                            <svg className="w-3 h-3 animate-spin" fill="none" viewBox="0 0 24 24">
                              <circle className="opacity-25" cx="12" cy="12" r="10" stroke="currentColor" strokeWidth="4" />
                              <path className="opacity-75" fill="currentColor" d="M4 12a8 8 0 018-8v4a4 4 0 00-4 4H4z" />
                            </svg>
                            Scanning
                          </span>
                        ) : (
                          "Scan"
                        )}
                      </button>
                    </div>
                  </div>
                </div>

                {/* Scan results inline */}
                {scanResults[site.site_id] && (
                  <div className="mt-3 pt-3 border-t border-dark-600">
                    <div className="text-xs font-mono text-dark-300 mb-2">Last Scan Results</div>
                    <div className="grid grid-cols-3 gap-2 text-xs font-mono">
                      <div>
                        <span className="text-dark-400">Total: </span>
                        <span className="text-dark-100">{scanResults[site.site_id].total_vulns ?? 0}</span>
                      </div>
                      <div>
                        <span className="text-dark-400">Critical: </span>
                        <span className={scanResults[site.site_id].critical_count > 0 ? "text-danger-400" : "text-dark-100"}>
                          {scanResults[site.site_id].critical_count ?? 0}
                        </span>
                      </div>
                      <div>
                        <span className="text-dark-400">High: </span>
                        <span className={scanResults[site.site_id].high_count > 0 ? "text-warn-400" : "text-dark-100"}>
                          {scanResults[site.site_id].high_count ?? 0}
                        </span>
                      </div>
                    </div>
                    {Array.isArray(scanResults[site.site_id].vulnerabilities) &&
                      scanResults[site.site_id].vulnerabilities.length > 0 && (
                        <div className="mt-2 space-y-1 max-h-32 overflow-y-auto">
                          {scanResults[site.site_id].vulnerabilities.map((v, i) => (
                            <div
                              key={i}
                              className="flex items-center gap-2 text-xs px-2 py-1 bg-dark-700 rounded"
                            >
                              <span
                                className={`shrink-0 w-1.5 h-1.5 rounded-full ${
                                  v.severity === "critical"
                                    ? "bg-danger-500"
                                    : v.severity === "high"
                                    ? "bg-warn-500"
                                    : "bg-dark-400"
                                }`}
                              />
                              <span className="text-dark-100 truncate">{v.title || v.name || "Unknown"}</span>
                              <span className="text-dark-400 ml-auto shrink-0">{v.severity}</span>
                            </div>
                          ))}
                        </div>
                      )}
                  </div>
                )}
              </div>
            ))}
          </div>
        </>
      )}

      {/* Security Tab */}
      {tab === "security" && sites.length > 0 && (
        <>
          <div className="flex items-center justify-between mb-4">
            <p className="text-sm text-dark-200">
              Run security checks against your WordPress sites and apply hardening fixes.
            </p>
            <button
              onClick={handleCheckAll}
              disabled={checkingAll}
              className="px-4 py-2 bg-rust-500 text-white rounded-lg text-sm font-medium hover:bg-rust-600 disabled:opacity-50"
            >
              {checkingAll ? (
                <span className="flex items-center gap-2">
                  <svg className="w-4 h-4 animate-spin" fill="none" viewBox="0 0 24 24">
                    <circle className="opacity-25" cx="12" cy="12" r="10" stroke="currentColor" strokeWidth="4" />
                    <path className="opacity-75" fill="currentColor" d="M4 12a8 8 0 018-8v4a4 4 0 00-4 4H4z" />
                  </svg>
                  Checking...
                </span>
              ) : (
                "Check All Sites"
              )}
            </button>
          </div>

          <div className="space-y-4">
            {sites.map((site) => {
              const checks = securityChecks[site.site_id];
              const failedChecks = checks?.filter((c) => c.status === "fail") || [];
              const fixableIds = failedChecks.filter((c) => c.auto_fixable).map((c) => c.name);

              return (
                <div
                  key={site.site_id}
                  className="bg-dark-800 rounded-lg border border-dark-500"
                >
                  <div className="flex items-center justify-between p-4 border-b border-dark-600">
                    <div className="flex items-center gap-3">
                      <h3 className="text-sm font-semibold text-dark-50">{site.domain}</h3>
                      <span className="text-xs text-dark-300 font-mono">WP {site.wp_version}</span>
                      {checks && (
                        <span
                          className={`px-2 py-0.5 rounded text-[10px] font-medium ${
                            failedChecks.length === 0
                              ? "bg-rust-500/15 text-rust-400"
                              : "bg-danger-500/15 text-danger-400"
                          }`}
                        >
                          {failedChecks.length === 0
                            ? "All Passed"
                            : `${failedChecks.length} Issue${failedChecks.length !== 1 ? "s" : ""}`}
                        </span>
                      )}
                    </div>
                    <div className="flex items-center gap-2">
                      {fixableIds.length > 0 && (
                        <button
                          onClick={() => handleHarden(site.site_id, fixableIds)}
                          disabled={hardening === site.site_id}
                          className="px-3 py-1.5 bg-accent-600 text-white rounded-lg text-xs font-medium hover:bg-accent-700 disabled:opacity-50"
                        >
                          {hardening === site.site_id ? "Fixing..." : `Fix ${fixableIds.length} Issue${fixableIds.length !== 1 ? "s" : ""}`}
                        </button>
                      )}
                      <button
                        onClick={() => handleSecurityCheck(site.site_id)}
                        className="px-3 py-1.5 bg-dark-700 text-dark-100 rounded-lg text-xs font-medium hover:bg-dark-600"
                      >
                        {checks ? "Re-check" : "Check"}
                      </button>
                    </div>
                  </div>

                  {checks && checks.length > 0 && (
                    <div className="divide-y divide-dark-600">
                      {checks.map((check) => (
                        <div
                          key={check.name}
                          className="flex items-center gap-3 px-4 py-2.5 hover:bg-dark-700/30 transition-colors"
                        >
                          {/* Status icon */}
                          {check.status === "pass" ? (
                            <svg className="w-4 h-4 text-rust-400 shrink-0" fill="none" viewBox="0 0 24 24" stroke="currentColor" strokeWidth={2}>
                              <path strokeLinecap="round" strokeLinejoin="round" d="m4.5 12.75 6 6 9-13.5" />
                            </svg>
                          ) : check.status === "warning" ? (
                            <svg className="w-4 h-4 text-warn-400 shrink-0" fill="none" viewBox="0 0 24 24" stroke="currentColor" strokeWidth={2}>
                              <path strokeLinecap="round" strokeLinejoin="round" d="M12 9v3.75m-9.303 3.376c-.866 1.5.217 3.374 1.948 3.374h14.71c1.73 0 2.813-1.874 1.948-3.374L13.949 3.378c-.866-1.5-3.032-1.5-3.898 0L2.697 16.126ZM12 15.75h.007v.008H12v-.008Z" />
                            </svg>
                          ) : (
                            <svg className="w-4 h-4 text-danger-400 shrink-0" fill="none" viewBox="0 0 24 24" stroke="currentColor" strokeWidth={2}>
                              <path strokeLinecap="round" strokeLinejoin="round" d="M6 18 18 6M6 6l12 12" />
                            </svg>
                          )}

                          <div className="flex-1 min-w-0">
                            <div className="text-sm text-dark-100">{check.label}</div>
                            {check.description && (
                              <div className="text-xs text-dark-400 mt-0.5">{check.description}</div>
                            )}
                          </div>

                          {check.status === "fail" && check.auto_fixable && (
                            <button
                              onClick={() => handleHarden(site.site_id, [check.name])}
                              disabled={hardening === site.site_id}
                              className="shrink-0 px-2.5 py-1 bg-accent-600 text-white rounded text-xs font-medium hover:bg-accent-700 disabled:opacity-50"
                            >
                              Fix
                            </button>
                          )}
                        </div>
                      ))}
                    </div>
                  )}

                  {checks && checks.length === 0 && (
                    <div className="px-4 py-6 text-center text-sm text-dark-300">
                      No security checks returned for this site.
                    </div>
                  )}

                  {!checks && (
                    <div className="px-4 py-6 text-center text-sm text-dark-400">
                      Click "Check" to run a security audit on this site.
                    </div>
                  )}
                </div>
              );
            })}
          </div>
        </>
      )}
    </div>
  );
}
