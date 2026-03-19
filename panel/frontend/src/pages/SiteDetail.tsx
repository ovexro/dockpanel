import { useState, useEffect } from "react";
import { useParams, useNavigate, Link } from "react-router-dom";
import { api } from "../api";
import { formatDate } from "../utils/format";
import { statusColors, runtimeLabelsDetailed as runtimeLabels } from "../constants";

interface Site {
  id: string;
  domain: string;
  runtime: string;
  status: string;
  proxy_port: number | null;
  php_version: string | null;
  ssl_enabled: boolean;
  ssl_expiry: string | null;
  rate_limit: number | null;
  max_upload_mb: number;
  php_memory_mb: number;
  php_max_workers: number;
  custom_nginx: string | null;
  php_preset: string | null;
  parent_site_id: string | null;
  synced_at: string | null;
  created_at: string;
  updated_at: string;
}

interface StagingInfo {
  exists: boolean;
  site?: Site;
  disk_usage_bytes?: number;
}

export default function SiteDetail() {
  const { id } = useParams<{ id: string }>();
  const navigate = useNavigate();
  const [site, setSite] = useState<Site | null>(null);
  const [loading, setLoading] = useState(true);
  const [error, setError] = useState("");
  const [deleting, setDeleting] = useState(false);
  const [confirmDelete, setConfirmDelete] = useState(false);
  const [provisioning, setProvisioning] = useState(false);
  const [sslMessage, setSslMessage] = useState("");
  const [switchingPhp, setSwitchingPhp] = useState(false);
  const [phpMessage, setPhpMessage] = useState("");
  const [savingLimits, setSavingLimits] = useState(false);
  const [limitsMessage, setLimitsMessage] = useState("");
  const [rateLimit, setRateLimit] = useState<string>("");
  const [maxUpload, setMaxUpload] = useState("64");
  const [phpMemory, setPhpMemory] = useState("256");
  const [phpWorkers, setPhpWorkers] = useState("5");
  const [customNginx, setCustomNginx] = useState("");
  const [staging, setStaging] = useState<StagingInfo | null>(null);
  const [stagingLoading, setStagingLoading] = useState(false);
  const [stagingMessage, setStagingMessage] = useState("");
  const [stagingDomain, setStagingDomain] = useState("");
  const [showStagingForm, setShowStagingForm] = useState(false);

  // Redirects
  const [redirects, setRedirects] = useState<{ source: string; target: string; type: string }[]>([]);
  const [redirectSource, setRedirectSource] = useState("");
  const [redirectTarget, setRedirectTarget] = useState("");
  const [redirectType, setRedirectType] = useState("301");
  const [redirectMsg, setRedirectMsg] = useState("");
  const [showRedirects, setShowRedirects] = useState(false);

  // Password Protection
  const [protectedPaths, setProtectedPaths] = useState<string[]>([]);
  const [protectedUsers, setProtectedUsers] = useState<string[]>([]);
  const [protectPath, setProtectPath] = useState("/");
  const [protectUser, setProtectUser] = useState("");
  const [protectPass, setProtectPass] = useState("");
  const [protectMsg, setProtectMsg] = useState("");
  const [showProtect, setShowProtect] = useState(false);

  // Domain Aliases
  const [aliases, setAliases] = useState<string[]>([]);
  const [newAlias, setNewAlias] = useState("");
  const [aliasMsg, setAliasMsg] = useState("");
  const [showAliases, setShowAliases] = useState(false);

  // Access Logs
  const [accessLogs, setAccessLogs] = useState("");
  const [errorLogs, setErrorLogs] = useState("");
  const [phpErrors, setPhpErrors] = useState("");
  const [showAccessLogs, setShowAccessLogs] = useState(false);
  const [logType, setLogType] = useState<"access" | "error" | "php">("access");

  // Traffic Stats
  const [stats, setStats] = useState<any>(null);
  const [showStats, setShowStats] = useState(false);

  // Health Check
  const [health, setHealth] = useState<{ healthy: boolean; status: number; response_time_ms: number } | null>(null);
  const [checkingHealth, setCheckingHealth] = useState(false);

  // Site Cloning
  const [cloning, setCloning] = useState(false);
  const [cloneMsg, setCloneMsg] = useState("");

  // Custom SSL Upload
  const [showSslUpload, setShowSslUpload] = useState(false);
  const [sslCert, setSslCert] = useState("");
  const [sslKey, setSslKey] = useState("");
  const [uploadingSsl, setUploadingSsl] = useState(false);

  // PHP Extensions
  const [phpExts, setPhpExts] = useState<{ installed: string[]; available: string[] } | null>(null);
  const [showPhpExts, setShowPhpExts] = useState(false);
  const [installingExt, setInstallingExt] = useState<string | null>(null);

  // Environment Variables
  const [envVars, setEnvVars] = useState<{ key: string; value: string }[]>([]);
  const [showEnvVars, setShowEnvVars] = useState(false);
  const [savingEnv, setSavingEnv] = useState(false);
  const [envMsg, setEnvMsg] = useState("");

  const fetchStaging = () => {
    api.get<StagingInfo>(`/sites/${id}/staging`).then(setStaging).catch(() => { /* no staging */ });
  };

  const loadRedirects = () => {
    api.get<{ redirects: { source: string; target: string; type: string }[] }>(`/sites/${id}/redirects`)
      .then((d) => setRedirects(d.redirects || []))
      .catch(() => {});
  };

  const loadProtected = () => {
    api.get<{ paths: string[]; users: string[] }>(`/sites/${id}/password-protect`)
      .then((d) => { setProtectedPaths(d.paths || []); setProtectedUsers(d.users || []); })
      .catch(() => {});
  };

  const loadAliases = () => {
    api.get<{ aliases: string[] }>(`/sites/${id}/aliases`)
      .then((d) => setAliases(d.aliases || []))
      .catch(() => {});
  };

  const loadLogs = async (type: "access" | "error") => {
    try {
      const data = await api.get<{ logs: string }>(`/sites/${id}/access-logs?type=${type}&lines=200`);
      if (type === "access") setAccessLogs(data.logs);
      else setErrorLogs(data.logs);
    } catch {}
  };

  const loadPhpErrors = async () => {
    try {
      const data = await api.get<{ logs: string }>(`/sites/${id}/php-errors`);
      setPhpErrors(data.logs);
    } catch {
      setPhpErrors("");
    }
  };

  const loadStats = async () => {
    try {
      const data = await api.get<any>(`/sites/${id}/stats`);
      setStats(data);
    } catch {}
  };

  const loadPhpExtensions = async () => {
    if (!site?.php_version) return;
    try {
      const data = await api.get<any>(`/php/extensions/${site.php_version}`);
      setPhpExts(data);
    } catch {}
  };

  const loadEnvVars = async () => {
    try {
      const data = await api.get<{ vars: { key: string; value: string }[] }>(`/sites/${id}/env`);
      setEnvVars(data.vars);
    } catch { setEnvVars([]); }
  };

  useEffect(() => {
    api
      .get<Site>(`/sites/${id}`)
      .then((s) => {
        setSite(s);
        setRateLimit(s.rate_limit != null ? String(s.rate_limit) : "");
        setMaxUpload(String(s.max_upload_mb));
        setPhpMemory(String(s.php_memory_mb));
        setPhpWorkers(String(s.php_max_workers));
        setCustomNginx(s.custom_nginx || "");
      })
      .catch((e) => setError(e.message))
      .finally(() => setLoading(false));
    fetchStaging();
    loadRedirects();
    loadProtected();
    loadAliases();
  }, [id]);

  const handleDelete = async () => {
    if (!confirmDelete) {
      setConfirmDelete(true);
      return;
    }
    setDeleting(true);
    try {
      await api.delete(`/sites/${id}`);
      navigate("/sites");
    } catch (err) {
      setError(err instanceof Error ? err.message : "Delete failed");
      setDeleting(false);
    }
  };

  const handleProvisionSSL = async () => {
    if (!site) return;
    setProvisioning(true);
    setSslMessage("");
    try {
      await api.post(`/sites/${id}/ssl`);
      // Refresh site data
      const updated = await api.get<Site>(`/sites/${id}`);
      setSite(updated);
      setSslMessage("SSL certificate provisioned successfully!");
    } catch (err) {
      setSslMessage(
        err instanceof Error ? err.message : "SSL provisioning failed"
      );
    } finally {
      setProvisioning(false);
    }
  };

  if (loading) {
    return (
      <div className="p-6 lg:p-8">
        <div className="bg-dark-800 rounded-lg border border-dark-500 p-6 animate-pulse">
          <div className="h-6 bg-dark-700 rounded w-64 mb-4" />
          <div className="h-4 bg-dark-700 rounded w-32" />
        </div>
      </div>
    );
  }

  if (error || !site) {
    return (
      <div className="p-6 lg:p-8">
        <div className="bg-danger-500/10 text-danger-400 px-4 py-3 rounded-lg border border-danger-500/20">
          {error || "Site not found"}
        </div>
        <Link to="/sites" className="text-sm text-rust-400 hover:text-rust-300 mt-4 inline-block">
          &larr; Back to sites
        </Link>
      </div>
    );
  }

  return (
    <div className="p-6 lg:p-8">
      {/* Breadcrumb */}
      <div className="mb-6">
        <Link to="/sites" className="text-sm text-dark-200 hover:text-dark-100">
          Sites
        </Link>
        <span className="text-sm text-dark-300 mx-2">/</span>
        <span className="text-sm text-dark-50 font-medium font-mono">{site.domain}</span>
      </div>

      {/* Header */}
      <div className="flex items-start justify-between mb-6">
        <div>
          <h1 className="text-sm font-medium text-dark-300 uppercase font-mono tracking-widest">{site.domain}</h1>
          <div className="flex items-center gap-3 mt-2">
            <span className={`inline-flex px-2.5 py-0.5 rounded-full text-xs font-medium ${statusColors[site.status] || "bg-dark-700 text-dark-200"}`}>
              {site.status}
            </span>
            <span className="text-sm text-dark-200">
              {runtimeLabels[site.runtime] || site.runtime}
            </span>
            <button disabled={checkingHealth} onClick={async () => {
              setCheckingHealth(true);
              try { const data = await api.get<any>(`/sites/${id}/health`); setHealth(data); }
              catch { setHealth({ healthy: false, status: 0, response_time_ms: 0 }); }
              finally { setCheckingHealth(false); }
            }} className="px-3 py-1.5 bg-dark-700 text-dark-100 rounded text-xs font-medium hover:bg-dark-600 disabled:opacity-50 transition-colors">
              {checkingHealth ? "Checking..." : "Health Check"}
            </button>
            {health && (
              <span className={`text-xs font-mono ${health.healthy ? "text-rust-400" : "text-danger-400"}`}>
                {health.status > 0 ? `${health.status}` : "Down"} · {health.response_time_ms}ms
              </span>
            )}
          </div>
        </div>
        <div>
          {confirmDelete ? (
            <div className="flex items-center gap-2">
              <span className="text-sm text-danger-400">Are you sure?</span>
              <button
                onClick={handleDelete}
                disabled={deleting}
                className="px-3 py-1.5 bg-danger-500 text-white rounded-lg text-sm hover:bg-danger-600 disabled:opacity-50"
              >
                {deleting ? "Deleting..." : "Yes, delete"}
              </button>
              <button
                onClick={() => setConfirmDelete(false)}
                className="px-3 py-1.5 bg-dark-600 text-dark-100 rounded-lg text-sm hover:bg-dark-500"
              >
                Cancel
              </button>
            </div>
          ) : (
            <div className="flex items-center gap-2">
              <button disabled={cloning} onClick={async () => {
                const newDomain = prompt("Clone site to domain:", `clone-${site?.domain}`);
                if (!newDomain) return;
                setCloning(true);
                setCloneMsg("");
                try {
                  await api.post(`/sites/${id}/clone`, { domain: newDomain });
                  setCloneMsg(`Site cloned to ${newDomain}`);
                } catch (e) { setCloneMsg(e instanceof Error ? e.message : "Clone failed"); }
                finally { setCloning(false); }
              }} className="px-4 py-2 bg-dark-700 text-dark-100 rounded-lg text-sm font-medium hover:bg-dark-600 disabled:opacity-50 transition-colors">
                {cloning ? "Cloning..." : "Clone Site"}
              </button>
              <button
                onClick={handleDelete}
                className="px-4 py-2 bg-danger-500/10 text-danger-400 rounded-lg text-sm font-medium hover:bg-danger-500/20 transition-colors"
              >
                Delete Site
              </button>
            </div>
          )}
        </div>
      </div>
      {cloneMsg && (
        <div className={`mb-4 px-4 py-3 rounded-lg text-sm ${
          cloneMsg.includes("cloned") ? "bg-rust-500/10 text-rust-400 border border-rust-500/20" : "bg-danger-500/10 text-danger-400 border border-danger-500/20"
        }`}>{cloneMsg}</div>
      )}

      {/* Details */}
      <div className="bg-dark-800 rounded-lg border border-dark-500 overflow-hidden">
        <dl className="divide-y divide-dark-600">
          <div className="px-5 py-4 grid grid-cols-3">
            <dt className="text-sm font-medium text-dark-200">Domain</dt>
            <dd className="text-sm text-dark-50 col-span-2 font-mono">{site.domain}</dd>
          </div>
          <div className="px-5 py-4 grid grid-cols-3">
            <dt className="text-sm font-medium text-dark-200">Runtime</dt>
            <dd className="text-sm text-dark-50 col-span-2">
              {runtimeLabels[site.runtime] || site.runtime}
            </dd>
          </div>
          {site.proxy_port && (
            <div className="px-5 py-4 grid grid-cols-3">
              <dt className="text-sm font-medium text-dark-200">Proxy Port</dt>
              <dd className="text-sm text-dark-50 col-span-2 font-mono">{site.proxy_port}</dd>
            </div>
          )}
          {site.runtime === "php" && (
            <div className="px-5 py-4 grid grid-cols-3">
              <dt className="text-sm font-medium text-dark-200">PHP Version</dt>
              <dd className="text-sm col-span-2">
                <div className="flex items-center gap-3">
                  <select
                    value={site.php_version || "8.3"}
                    onChange={async (e) => {
                      const newVersion = e.target.value;
                      if (newVersion === site.php_version) return;
                      setSwitchingPhp(true);
                      setPhpMessage("");
                      try {
                        const updated = await api.put<Site>(`/sites/${id}/php`, { version: newVersion });
                        setSite(updated);
                        setPhpMessage(`Switched to PHP ${newVersion}`);
                      } catch (err) {
                        setPhpMessage(err instanceof Error ? err.message : "Switch failed");
                      } finally {
                        setSwitchingPhp(false);
                      }
                    }}
                    disabled={switchingPhp}
                    className="px-2 py-1 border border-dark-500 rounded-md text-sm bg-dark-800 focus:ring-2 focus:ring-accent-500 focus:border-accent-500 outline-none disabled:opacity-50"
                  >
                    <option value="8.4">PHP 8.4</option>
                    <option value="8.3">PHP 8.3</option>
                    <option value="8.2">PHP 8.2</option>
                    <option value="8.1">PHP 8.1</option>
                  </select>
                  {switchingPhp && (
                    <span className="text-xs text-dark-200">Switching...</span>
                  )}
                  {phpMessage && (
                    <span className={`text-xs ${phpMessage.includes("Switched") ? "text-rust-400" : "text-danger-400"}`}>
                      {phpMessage}
                    </span>
                  )}
                </div>
              </dd>
            </div>
          )}
          {site.runtime === "php" && site.php_preset && site.php_preset !== "generic" && (
            <div className="px-5 py-4 grid grid-cols-3">
              <dt className="text-sm font-medium text-dark-200">Framework</dt>
              <dd className="text-sm text-dark-50 col-span-2 capitalize">{site.php_preset}</dd>
            </div>
          )}
          <div className="px-5 py-4 grid grid-cols-3">
            <dt className="text-sm font-medium text-dark-200">SSL</dt>
            <dd className="text-sm col-span-2">
              {site.ssl_enabled ? (
                <div className="space-y-1">
                  <span className="inline-flex items-center gap-1 text-rust-400">
                    <svg className="w-4 h-4" fill="currentColor" viewBox="0 0 20 20">
                      <path fillRule="evenodd" d="M10 1a4.5 4.5 0 0 0-4.5 4.5V9H5a2 2 0 0 0-2 2v6a2 2 0 0 0 2 2h10a2 2 0 0 0 2-2v-6a2 2 0 0 0-2-2h-.5V5.5A4.5 4.5 0 0 0 10 1Zm3 8V5.5a3 3 0 1 0-6 0V9h6Z" clipRule="evenodd" />
                    </svg>
                    Enabled (Let's Encrypt)
                  </span>
                  {site.ssl_expiry && (
                    <p className="text-xs text-dark-200">
                      Expires: {new Date(site.ssl_expiry).toLocaleDateString()}
                    </p>
                  )}
                </div>
              ) : (
                <div>
                  <div className="flex items-center gap-3">
                    <span className="text-dark-300">Not configured</span>
                    {site.status === "active" && (
                      <>
                        <button
                          onClick={handleProvisionSSL}
                          disabled={provisioning}
                          className="px-3 py-1 bg-rust-500 text-white rounded-md text-xs font-medium hover:bg-rust-600 disabled:opacity-50 transition-colors"
                        >
                          {provisioning ? "Provisioning..." : "Let's Encrypt"}
                        </button>
                        <button
                          onClick={() => setShowSslUpload(!showSslUpload)}
                          className="px-3 py-1 bg-dark-700 text-dark-100 rounded-md text-xs font-medium hover:bg-dark-600 transition-colors"
                        >
                          Upload Custom
                        </button>
                      </>
                    )}
                  </div>
                  {showSslUpload && (
                    <div className="mt-3 space-y-3">
                      <textarea value={sslCert} onChange={e => setSslCert(e.target.value)}
                        placeholder={"-----BEGIN CERTIFICATE-----\n...\n-----END CERTIFICATE-----"}
                        rows={4} className="w-full px-3 py-2 bg-dark-900 border border-dark-500 rounded-lg text-xs font-mono text-dark-100 focus:ring-2 focus:ring-accent-500 outline-none" />
                      <textarea value={sslKey} onChange={e => setSslKey(e.target.value)}
                        placeholder={"-----BEGIN PRIVATE KEY-----\n...\n-----END PRIVATE KEY-----"}
                        rows={4} className="w-full px-3 py-2 bg-dark-900 border border-dark-500 rounded-lg text-xs font-mono text-dark-100 focus:ring-2 focus:ring-accent-500 outline-none" />
                      <button disabled={uploadingSsl || !sslCert || !sslKey} onClick={async () => {
                        setUploadingSsl(true);
                        setSslMessage("");
                        try {
                          await api.post(`/sites/${id}/ssl/upload`, { certificate: sslCert, private_key: sslKey });
                          setSslMessage("Custom SSL certificate installed successfully!");
                          setSslCert(""); setSslKey(""); setShowSslUpload(false);
                          const updated = await api.get<Site>(`/sites/${id}`);
                          setSite(updated);
                        } catch (e) { setSslMessage(e instanceof Error ? e.message : "Upload failed"); }
                        finally { setUploadingSsl(false); }
                      }} className="px-4 py-2 bg-rust-500 text-white rounded-lg text-sm font-medium hover:bg-rust-600 disabled:opacity-50">
                        {uploadingSsl ? "Installing..." : "Install Certificate"}
                      </button>
                    </div>
                  )}
                </div>
              )}
            </dd>
          </div>
          <div className="px-5 py-4 grid grid-cols-3">
            <dt className="text-sm font-medium text-dark-200">Status</dt>
            <dd className="text-sm col-span-2">
              <span className={`inline-flex px-2.5 py-0.5 rounded-full text-xs font-medium ${statusColors[site.status] || "bg-dark-700 text-dark-200"}`}>
                {site.status}
              </span>
            </dd>
          </div>
          <div className="px-5 py-4 grid grid-cols-3">
            <dt className="text-sm font-medium text-dark-200">Created</dt>
            <dd className="text-sm text-dark-50 col-span-2">
              {formatDate(site.created_at)}
            </dd>
          </div>
          <div className="px-5 py-4 grid grid-cols-3">
            <dt className="text-sm font-medium text-dark-200">Last Updated</dt>
            <dd className="text-sm text-dark-50 col-span-2">
              {formatDate(site.updated_at)}
            </dd>
          </div>
        </dl>
      </div>

      {/* Quick actions */}
      <div className="grid grid-cols-2 sm:grid-cols-3 lg:grid-cols-6 gap-4 mt-6">
        <Link
          to={`/sites/${id}/files`}
          className="bg-dark-800 rounded-lg border border-dark-500 p-5 hover:border-accent-400 hover:shadow-sm transition-all group"
        >
          <div className="flex items-center gap-3">
            <div className="p-2 bg-warn-500/10 rounded-lg text-warn-400 group-hover:bg-warn-500/20 transition-colors">
              <svg className="w-5 h-5" fill="none" viewBox="0 0 24 24" stroke="currentColor" strokeWidth={1.5}>
                <path strokeLinecap="round" strokeLinejoin="round" d="M2.25 12.75V12A2.25 2.25 0 0 1 4.5 9.75h15A2.25 2.25 0 0 1 21.75 12v.75m-8.69-6.44-2.12-2.12a1.5 1.5 0 0 0-1.061-.44H4.5A2.25 2.25 0 0 0 2.25 6v12a2.25 2.25 0 0 0 2.25 2.25h15A2.25 2.25 0 0 0 21.75 18V9a2.25 2.25 0 0 0-2.25-2.25h-5.379a1.5 1.5 0 0 1-1.06-.44Z" />
              </svg>
            </div>
            <div>
              <p className="text-sm font-medium text-dark-50">File Manager</p>
              <p className="text-xs text-dark-200">Browse & edit files</p>
            </div>
          </div>
        </Link>
        <Link
          to={`/terminal?site=${id}`}
          className="bg-dark-800 rounded-lg border border-dark-500 p-5 hover:border-accent-400 hover:shadow-sm transition-all group"
        >
          <div className="flex items-center gap-3">
            <div className="p-2 bg-dark-500/10 rounded-lg text-dark-400 group-hover:bg-dark-500/20 transition-colors">
              <svg className="w-5 h-5" fill="none" viewBox="0 0 24 24" stroke="currentColor" strokeWidth={1.5}>
                <path strokeLinecap="round" strokeLinejoin="round" d="m6.75 7.5 3 2.25-3 2.25m4.5 0h3m-9 8.25h13.5A2.25 2.25 0 0 0 21 18V6a2.25 2.25 0 0 0-2.25-2.25H5.25A2.25 2.25 0 0 0 3 6v12a2.25 2.25 0 0 0 2.25 2.25Z" />
              </svg>
            </div>
            <div>
              <p className="text-sm font-medium text-dark-50">Terminal</p>
              <p className="text-xs text-dark-200">SSH-like access</p>
            </div>
          </div>
        </Link>
        <Link
          to={`/sites/${id}/backups`}
          className="bg-dark-800 rounded-lg border border-dark-500 p-5 hover:border-accent-400 hover:shadow-sm transition-all group"
        >
          <div className="flex items-center gap-3">
            <div className="p-2 bg-rust-500/10 rounded-lg text-rust-500 group-hover:bg-accent-500/20 transition-colors">
              <svg className="w-5 h-5" fill="none" viewBox="0 0 24 24" stroke="currentColor" strokeWidth={1.5}>
                <path strokeLinecap="round" strokeLinejoin="round" d="M20.25 7.5l-.625 10.632a2.25 2.25 0 0 1-2.247 2.118H6.622a2.25 2.25 0 0 1-2.247-2.118L3.75 7.5m8.25 3v6.75m0 0-3-3m3 3 3-3M3.375 7.5h17.25c.621 0 1.125-.504 1.125-1.125v-1.5c0-.621-.504-1.125-1.125-1.125H3.375c-.621 0-1.125.504-1.125 1.125v1.5c0 .621.504 1.125 1.125 1.125Z" />
              </svg>
            </div>
            <div>
              <p className="text-sm font-medium text-dark-50">Backups</p>
              <p className="text-xs text-dark-200">Create & restore</p>
            </div>
          </div>
        </Link>
        <Link
          to={`/sites/${id}/crons`}
          className="bg-dark-800 rounded-lg border border-dark-500 p-5 hover:border-accent-400 hover:shadow-sm transition-all group"
        >
          <div className="flex items-center gap-3">
            <div className="p-2 bg-accent-600/10 rounded-lg text-accent-400 group-hover:bg-accent-600/20 transition-colors">
              <svg className="w-5 h-5" fill="none" viewBox="0 0 24 24" stroke="currentColor" strokeWidth={1.5}>
                <path strokeLinecap="round" strokeLinejoin="round" d="M12 6v6h4.5m4.5 0a9 9 0 1 1-18 0 9 9 0 0 1 18 0Z" />
              </svg>
            </div>
            <div>
              <p className="text-sm font-medium text-dark-50">Cron Jobs</p>
              <p className="text-xs text-dark-200">Scheduled tasks</p>
            </div>
          </div>
        </Link>
        <Link
          to={`/sites/${id}/deploy`}
          className="bg-dark-800 rounded-lg border border-dark-500 p-5 hover:border-accent-400 hover:shadow-sm transition-all group"
        >
          <div className="flex items-center gap-3">
            <div className="p-2 bg-rust-500/10 rounded-lg text-rust-400 group-hover:bg-rust-500/20 transition-colors">
              <svg className="w-5 h-5" fill="none" viewBox="0 0 24 24" stroke="currentColor" strokeWidth={1.5}>
                <path strokeLinecap="round" strokeLinejoin="round" d="M17.25 6.75 22.5 12l-5.25 5.25m-10.5 0L1.5 12l5.25-5.25m7.5-3-4.5 16.5" />
              </svg>
            </div>
            <div>
              <p className="text-sm font-medium text-dark-50">Git Deploy</p>
              <p className="text-xs text-dark-200">Push to deploy</p>
            </div>
          </div>
        </Link>
        {site.php_preset === "wordpress" && (
          <Link
            to={`/sites/${id}/wordpress`}
            className="bg-dark-800 rounded-lg border border-dark-500 p-5 hover:border-accent-400 hover:shadow-sm transition-all group"
          >
            <div className="flex items-center gap-3">
              <div className="p-2 bg-accent-500/10 rounded-lg text-accent-400 group-hover:bg-accent-500/20 transition-colors">
                <svg className="w-5 h-5" viewBox="0 0 24 24" fill="currentColor">
                  <path d="M12 2C6.486 2 2 6.486 2 12s4.486 10 10 10 10-4.486 10-10S17.514 2 12 2zm0 2c1.67 0 3.214.52 4.488 1.401L5.401 16.488A7.957 7.957 0 0 1 4 12c0-4.411 3.589-8 8-8zm0 16c-1.67 0-3.214-.52-4.488-1.401L18.599 7.512A7.957 7.957 0 0 1 20 12c0 4.411-3.589 8-8 8z" />
                </svg>
              </div>
              <div>
                <p className="text-sm font-medium text-dark-50">WordPress</p>
                <p className="text-xs text-dark-200">Manage WP</p>
              </div>
            </div>
          </Link>
        )}
      </div>

      {/* Resource Limits */}
      {site.status === "active" && (
        <div className="bg-dark-800 rounded-lg border border-dark-500 overflow-hidden mt-6">
          <div className="px-5 py-4 border-b border-dark-600">
            <h2 className="text-xs font-medium text-dark-300 uppercase font-mono tracking-widest">Resource Limits</h2>
          </div>
          <div className="p-5 space-y-4">
            <div className="grid grid-cols-2 sm:grid-cols-4 gap-4">
              <div>
                <label className="block text-xs font-medium text-dark-200 mb-1">Rate Limit (req/s per IP)</label>
                <input
                  type="number"
                  value={rateLimit}
                  onChange={(e) => setRateLimit(e.target.value)}
                  placeholder="Unlimited"
                  min="1"
                  max="10000"
                  className="w-full px-3 py-2 border border-dark-500 rounded-lg text-sm focus:ring-2 focus:ring-accent-500 focus:border-accent-500 outline-none"
                />
              </div>
              <div>
                <label className="block text-xs font-medium text-dark-200 mb-1">Max Upload (MB)</label>
                <input
                  type="number"
                  value={maxUpload}
                  onChange={(e) => setMaxUpload(e.target.value)}
                  min="1"
                  max="10240"
                  className="w-full px-3 py-2 border border-dark-500 rounded-lg text-sm focus:ring-2 focus:ring-accent-500 focus:border-accent-500 outline-none"
                />
              </div>
              {site.runtime === "php" && (
                <>
                  <div>
                    <label className="block text-xs font-medium text-dark-200 mb-1">PHP Memory (MB)</label>
                    <input
                      type="number"
                      value={phpMemory}
                      onChange={(e) => setPhpMemory(e.target.value)}
                      min="32"
                      max="4096"
                      className="w-full px-3 py-2 border border-dark-500 rounded-lg text-sm focus:ring-2 focus:ring-accent-500 focus:border-accent-500 outline-none"
                    />
                  </div>
                  <div>
                    <label className="block text-xs font-medium text-dark-200 mb-1">PHP Workers</label>
                    <input
                      type="number"
                      value={phpWorkers}
                      onChange={(e) => setPhpWorkers(e.target.value)}
                      min="1"
                      max="100"
                      className="w-full px-3 py-2 border border-dark-500 rounded-lg text-sm focus:ring-2 focus:ring-accent-500 focus:border-accent-500 outline-none"
                    />
                  </div>
                </>
              )}
            </div>
            {/* Custom Nginx Directives */}
            <div>
              <label className="block text-xs font-medium text-dark-200 mb-1">
                Custom Nginx Directives <span className="text-dark-300 font-normal">(injected into server block)</span>
              </label>
              <textarea
                value={customNginx}
                onChange={(e) => setCustomNginx(e.target.value)}
                rows={4}
                placeholder={"# Example:\n# add_header X-Custom-Header \"value\";\n# location /api { proxy_pass http://localhost:3000; }"}
                spellCheck={false}
                className="w-full px-3 py-2 border border-dark-500 rounded-lg text-sm font-mono focus:ring-2 focus:ring-accent-500 focus:border-accent-500 outline-none resize-y"
              />
              <p className="text-[10px] text-dark-300 mt-1">
                Config is validated before applying. Invalid directives will be rejected.
              </p>
            </div>

            <div className="flex items-center justify-between">
              <p className="text-xs text-dark-300">
                {rateLimit ? `${rateLimit} req/s per IP` : "No rate limit"} · {maxUpload} MB uploads
                {site.runtime === "php" ? ` · ${phpMemory} MB memory · ${phpWorkers} workers` : ""}
              </p>
              <div className="flex items-center gap-3">
                {limitsMessage && (
                  <span className={`text-xs ${limitsMessage.includes("saved") ? "text-rust-400" : "text-danger-400"}`}>
                    {limitsMessage}
                  </span>
                )}
                <button
                  disabled={savingLimits}
                  onClick={async () => {
                    setSavingLimits(true);
                    setLimitsMessage("");
                    try {
                      const updated = await api.put<Site>(`/sites/${id}/limits`, {
                        rate_limit: rateLimit ? parseInt(rateLimit) : null,
                        max_upload_mb: parseInt(maxUpload) || 64,
                        php_memory_mb: parseInt(phpMemory) || 256,
                        php_max_workers: parseInt(phpWorkers) || 5,
                        custom_nginx: customNginx || null,
                      });
                      setSite(updated);
                      setLimitsMessage("Limits saved");
                    } catch (err) {
                      setLimitsMessage(err instanceof Error ? err.message : "Save failed");
                    } finally {
                      setSavingLimits(false);
                    }
                  }}
                  className="px-4 py-2 bg-rust-500 text-white rounded-lg text-sm font-medium hover:bg-rust-600 disabled:opacity-50 transition-colors"
                >
                  {savingLimits ? "Saving..." : "Apply Limits"}
                </button>
              </div>
            </div>
          </div>
        </div>
      )}

      {/* Staging Environment */}
      {site.status === "active" && !site.parent_site_id && (
        <div className="bg-dark-800 rounded-lg border border-dark-500 overflow-hidden mt-6">
          <div className="px-5 py-4 border-b border-dark-600 flex items-center justify-between">
            <h2 className="text-xs font-medium text-dark-300 uppercase font-mono tracking-widest">Staging Environment</h2>
            {staging?.exists && staging.site && (
              <span className={`inline-flex px-2.5 py-0.5 rounded-full text-xs font-medium ${statusColors[staging.site.status] || "bg-dark-700 text-dark-200"}`}>
                {staging.site.status}
              </span>
            )}
          </div>
          <div className="p-5">
            {staging?.exists && staging.site ? (
              <div className="space-y-4">
                {/* Staging info */}
                <div className="grid grid-cols-2 sm:grid-cols-4 gap-4">
                  <div>
                    <p className="text-xs font-medium text-dark-200 mb-1">Domain</p>
                    <a
                      href={`http://${staging.site.domain}`}
                      target="_blank"
                      rel="noopener noreferrer"
                      className="text-sm text-rust-400 hover:text-rust-300 font-mono"
                    >
                      {staging.site.domain}
                    </a>
                  </div>
                  <div>
                    <p className="text-xs font-medium text-dark-200 mb-1">Last Synced</p>
                    <p className="text-sm text-dark-50">
                      {staging.site.synced_at ? formatDate(staging.site.synced_at) : "Never"}
                    </p>
                  </div>
                  <div>
                    <p className="text-xs font-medium text-dark-200 mb-1">Disk Usage</p>
                    <p className="text-sm text-dark-50">
                      {staging.disk_usage_bytes != null
                        ? staging.disk_usage_bytes < 1048576
                          ? `${Math.round(staging.disk_usage_bytes / 1024)} KB`
                          : `${(staging.disk_usage_bytes / 1048576).toFixed(1)} MB`
                        : "—"}
                    </p>
                  </div>
                  <div>
                    <p className="text-xs font-medium text-dark-200 mb-1">Created</p>
                    <p className="text-sm text-dark-50">{formatDate(staging.site.created_at)}</p>
                  </div>
                </div>

                {/* Staging actions */}
                <div className="flex items-center gap-2 pt-2 border-t border-dark-600">
                  <button
                    disabled={stagingLoading}
                    onClick={async () => {
                      setStagingLoading(true);
                      setStagingMessage("");
                      try {
                        await api.post(`/sites/${id}/staging/sync`);
                        setStagingMessage("Files synced from production");
                        fetchStaging();
                      } catch (e) {
                        setStagingMessage(e instanceof Error ? e.message : "Sync failed");
                      } finally {
                        setStagingLoading(false);
                      }
                    }}
                    className="px-3 py-1.5 bg-rust-500 text-white rounded-lg text-xs font-medium hover:bg-rust-600 disabled:opacity-50 transition-colors"
                  >
                    {stagingLoading ? "Working..." : "Sync from Prod"}
                  </button>
                  <button
                    disabled={stagingLoading}
                    onClick={async () => {
                      if (!confirm("This will overwrite production files with staging files. Continue?")) return;
                      setStagingLoading(true);
                      setStagingMessage("");
                      try {
                        await api.post(`/sites/${id}/staging/push`);
                        setStagingMessage("Staging pushed to production");
                      } catch (e) {
                        setStagingMessage(e instanceof Error ? e.message : "Push failed");
                      } finally {
                        setStagingLoading(false);
                      }
                    }}
                    className="px-3 py-1.5 bg-warn-600 text-white rounded-lg text-xs font-medium hover:bg-warn-600 disabled:opacity-50 transition-colors"
                  >
                    Push to Prod
                  </button>
                  <Link
                    to={`/sites/${staging.site.id}/files`}
                    className="px-3 py-1.5 bg-dark-700 text-dark-100 rounded-lg text-xs font-medium hover:bg-dark-600 transition-colors"
                  >
                    Files
                  </Link>
                  <Link
                    to={`/terminal?site=${staging.site.id}`}
                    className="px-3 py-1.5 bg-dark-700 text-dark-100 rounded-lg text-xs font-medium hover:bg-dark-600 transition-colors"
                  >
                    Terminal
                  </Link>
                  <div className="flex-1" />
                  <button
                    disabled={stagingLoading}
                    onClick={async () => {
                      if (!confirm("Delete this staging environment? This cannot be undone.")) return;
                      setStagingLoading(true);
                      setStagingMessage("");
                      try {
                        await api.delete(`/sites/${id}/staging`);
                        setStaging({ exists: false });
                        setStagingMessage("Staging deleted");
                      } catch (e) {
                        setStagingMessage(e instanceof Error ? e.message : "Delete failed");
                      } finally {
                        setStagingLoading(false);
                      }
                    }}
                    className="px-3 py-1.5 bg-danger-500/10 text-danger-400 rounded-lg text-xs font-medium hover:bg-danger-500/20 transition-colors"
                  >
                    Delete Staging
                  </button>
                </div>

                {stagingMessage && (
                  <p className={`text-xs ${stagingMessage.includes("failed") || stagingMessage.includes("Failed") ? "text-danger-400" : "text-rust-400"}`}>
                    {stagingMessage}
                  </p>
                )}
              </div>
            ) : (
              <div className="space-y-3">
                <p className="text-sm text-dark-200">
                  Create a staging copy of this site to test changes before going live.
                </p>
                {showStagingForm ? (
                  <div className="flex items-end gap-3">
                    <div className="flex-1">
                      <label className="block text-xs font-medium text-dark-200 mb-1">
                        Staging Domain
                      </label>
                      <input
                        type="text"
                        value={stagingDomain}
                        onChange={(e) => setStagingDomain(e.target.value)}
                        placeholder={`staging.${site.domain}`}
                        className="w-full px-3 py-2 border border-dark-500 rounded-lg text-sm focus:ring-2 focus:ring-accent-500 focus:border-accent-500 outline-none"
                      />
                    </div>
                    <button
                      disabled={stagingLoading}
                      onClick={async () => {
                        setStagingLoading(true);
                        setStagingMessage("");
                        try {
                          await api.post(`/sites/${id}/staging`, {
                            domain: stagingDomain || undefined,
                          });
                          fetchStaging();
                          setShowStagingForm(false);
                          setStagingMessage("Staging environment created");
                        } catch (e) {
                          setStagingMessage(e instanceof Error ? e.message : "Creation failed");
                        } finally {
                          setStagingLoading(false);
                        }
                      }}
                      className="px-4 py-2 bg-rust-500 text-white rounded-lg text-sm font-medium hover:bg-rust-600 disabled:opacity-50 transition-colors"
                    >
                      {stagingLoading ? "Creating..." : "Create"}
                    </button>
                    <button
                      onClick={() => setShowStagingForm(false)}
                      className="px-4 py-2 bg-dark-700 text-dark-100 rounded-lg text-sm font-medium hover:bg-dark-600 transition-colors"
                    >
                      Cancel
                    </button>
                  </div>
                ) : (
                  <button
                    onClick={() => setShowStagingForm(true)}
                    className="px-4 py-2 bg-rust-500 text-white rounded-lg text-sm font-medium hover:bg-rust-600 transition-colors"
                  >
                    Create Staging
                  </button>
                )}
                {stagingMessage && (
                  <p className={`text-xs ${stagingMessage.includes("failed") || stagingMessage.includes("Failed") ? "text-danger-400" : "text-rust-400"}`}>
                    {stagingMessage}
                  </p>
                )}
              </div>
            )}
          </div>
        </div>
      )}

      {/* Redirect Rules */}
      {site.status === "active" && (
        <div className="bg-dark-800 rounded-lg border border-dark-500 overflow-hidden mt-6">
          <button
            onClick={() => setShowRedirects(!showRedirects)}
            className="w-full px-5 py-4 border-b border-dark-600 flex items-center justify-between text-left"
          >
            <h2 className="text-xs font-medium text-dark-300 uppercase font-mono tracking-widest">Redirect Rules</h2>
            <div className="flex items-center gap-2">
              {redirects.length > 0 && (
                <span className="text-xs text-dark-200">{redirects.length} rule{redirects.length !== 1 ? "s" : ""}</span>
              )}
              <svg className={`w-4 h-4 text-dark-300 transition-transform ${showRedirects ? "rotate-180" : ""}`} fill="none" viewBox="0 0 24 24" stroke="currentColor" strokeWidth={2}>
                <path strokeLinecap="round" strokeLinejoin="round" d="M19.5 8.25l-7.5 7.5-7.5-7.5" />
              </svg>
            </div>
          </button>
          {showRedirects && (
            <div className="p-5 space-y-4">
              {/* Existing redirects */}
              {redirects.length > 0 && (
                <div className="space-y-2">
                  {redirects.map((r, i) => (
                    <div key={i} className="flex items-center justify-between bg-dark-900 rounded-lg px-4 py-3">
                      <div className="flex items-center gap-3 text-sm">
                        <span className={`px-2 py-0.5 rounded text-xs font-medium ${r.type === "301" ? "bg-rust-500/10 text-rust-400" : "bg-warn-500/10 text-warn-400"}`}>
                          {r.type}
                        </span>
                        <span className="text-dark-100 font-mono">{r.source}</span>
                        <svg className="w-4 h-4 text-dark-300" fill="none" viewBox="0 0 24 24" stroke="currentColor" strokeWidth={1.5}>
                          <path strokeLinecap="round" strokeLinejoin="round" d="M13.5 4.5L21 12m0 0l-7.5 7.5M21 12H3" />
                        </svg>
                        <span className="text-dark-200 font-mono truncate max-w-[300px]">{r.target}</span>
                      </div>
                      <button
                        onClick={async () => {
                          try {
                            await api.post(`/sites/${id}/redirects/remove`, { source: r.source });
                            loadRedirects();
                            setRedirectMsg("Redirect removed");
                          } catch (e) {
                            setRedirectMsg(e instanceof Error ? e.message : "Failed");
                          }
                        }}
                        className="p-1 text-dark-300 hover:text-danger-500 transition-colors"
                        title="Remove"
                      >
                        <svg className="w-4 h-4" fill="none" viewBox="0 0 24 24" stroke="currentColor" strokeWidth={1.5}>
                          <path strokeLinecap="round" strokeLinejoin="round" d="M6 18L18 6M6 6l12 12" />
                        </svg>
                      </button>
                    </div>
                  ))}
                </div>
              )}

              {/* Add redirect form */}
              <div className="flex items-end gap-3">
                <div className="flex-1">
                  <label className="block text-xs font-medium text-dark-200 mb-1">Source Path</label>
                  <input
                    type="text"
                    value={redirectSource}
                    onChange={(e) => setRedirectSource(e.target.value)}
                    placeholder="/old-page"
                    className="w-full px-3 py-2 border border-dark-500 rounded-lg text-sm focus:ring-2 focus:ring-accent-500 focus:border-accent-500 outline-none"
                  />
                </div>
                <div className="flex-1">
                  <label className="block text-xs font-medium text-dark-200 mb-1">Target URL</label>
                  <input
                    type="text"
                    value={redirectTarget}
                    onChange={(e) => setRedirectTarget(e.target.value)}
                    placeholder="https://example.com/new-page"
                    className="w-full px-3 py-2 border border-dark-500 rounded-lg text-sm focus:ring-2 focus:ring-accent-500 focus:border-accent-500 outline-none"
                  />
                </div>
                <div className="w-24">
                  <label className="block text-xs font-medium text-dark-200 mb-1">Type</label>
                  <select
                    value={redirectType}
                    onChange={(e) => setRedirectType(e.target.value)}
                    className="w-full px-3 py-2 border border-dark-500 rounded-lg text-sm bg-dark-800 focus:ring-2 focus:ring-accent-500 focus:border-accent-500 outline-none"
                  >
                    <option value="301">301</option>
                    <option value="302">302</option>
                  </select>
                </div>
                <button
                  onClick={async () => {
                    if (!redirectSource || !redirectTarget) return;
                    setRedirectMsg("");
                    try {
                      await api.post(`/sites/${id}/redirects`, {
                        source: redirectSource,
                        target: redirectTarget,
                        redirect_type: redirectType,
                      });
                      setRedirectSource("");
                      setRedirectTarget("");
                      loadRedirects();
                      setRedirectMsg("Redirect added");
                    } catch (e) {
                      setRedirectMsg(e instanceof Error ? e.message : "Failed");
                    }
                  }}
                  className="px-4 py-2 bg-rust-500 text-white rounded-lg text-sm font-medium hover:bg-rust-600 transition-colors"
                >
                  Add
                </button>
              </div>

              {redirectMsg && (
                <p className={`text-xs ${redirectMsg.includes("Failed") || redirectMsg.includes("failed") ? "text-danger-400" : "text-rust-400"}`}>
                  {redirectMsg}
                </p>
              )}
            </div>
          )}
        </div>
      )}

      {/* Password Protection */}
      {site.status === "active" && (
        <div className="bg-dark-800 rounded-lg border border-dark-500 overflow-hidden mt-6">
          <button
            onClick={() => setShowProtect(!showProtect)}
            className="w-full px-5 py-4 border-b border-dark-600 flex items-center justify-between text-left"
          >
            <h2 className="text-xs font-medium text-dark-300 uppercase font-mono tracking-widest">Password Protection</h2>
            <div className="flex items-center gap-2">
              {protectedPaths.length > 0 && (
                <span className="text-xs text-dark-200">{protectedPaths.length} path{protectedPaths.length !== 1 ? "s" : ""}</span>
              )}
              <svg className={`w-4 h-4 text-dark-300 transition-transform ${showProtect ? "rotate-180" : ""}`} fill="none" viewBox="0 0 24 24" stroke="currentColor" strokeWidth={2}>
                <path strokeLinecap="round" strokeLinejoin="round" d="M19.5 8.25l-7.5 7.5-7.5-7.5" />
              </svg>
            </div>
          </button>
          {showProtect && (
            <div className="p-5 space-y-4">
              {/* Current protection */}
              {protectedPaths.length > 0 && (
                <div className="space-y-2">
                  <div className="flex items-center gap-4 text-xs text-dark-200 mb-2">
                    <span>Protected paths: {protectedPaths.join(", ")}</span>
                    <span>Users: {protectedUsers.join(", ")}</span>
                  </div>
                  {protectedPaths.map((p, i) => (
                    <div key={i} className="flex items-center justify-between bg-dark-900 rounded-lg px-4 py-3">
                      <div className="flex items-center gap-2 text-sm">
                        <svg className="w-4 h-4 text-warn-400" fill="none" viewBox="0 0 24 24" stroke="currentColor" strokeWidth={1.5}>
                          <path strokeLinecap="round" strokeLinejoin="round" d="M16.5 10.5V6.75a4.5 4.5 0 1 0-9 0v3.75m-.75 11.25h10.5a2.25 2.25 0 0 0 2.25-2.25v-6.75a2.25 2.25 0 0 0-2.25-2.25H6.75a2.25 2.25 0 0 0-2.25 2.25v6.75a2.25 2.25 0 0 0 2.25 2.25Z" />
                        </svg>
                        <span className="text-dark-100 font-mono">{p}</span>
                      </div>
                      <button
                        onClick={async () => {
                          try {
                            await api.post(`/sites/${id}/password-protect/remove`, { path: p });
                            loadProtected();
                            setProtectMsg("Protection removed");
                          } catch (e) {
                            setProtectMsg(e instanceof Error ? e.message : "Failed");
                          }
                        }}
                        className="p-1 text-dark-300 hover:text-danger-500 transition-colors"
                        title="Remove"
                      >
                        <svg className="w-4 h-4" fill="none" viewBox="0 0 24 24" stroke="currentColor" strokeWidth={1.5}>
                          <path strokeLinecap="round" strokeLinejoin="round" d="M6 18L18 6M6 6l12 12" />
                        </svg>
                      </button>
                    </div>
                  ))}
                </div>
              )}

              {/* Add protection form */}
              <div className="flex items-end gap-3">
                <div>
                  <label className="block text-xs font-medium text-dark-200 mb-1">Path</label>
                  <input
                    type="text"
                    value={protectPath}
                    onChange={(e) => setProtectPath(e.target.value)}
                    placeholder="/"
                    className="w-32 px-3 py-2 border border-dark-500 rounded-lg text-sm focus:ring-2 focus:ring-accent-500 focus:border-accent-500 outline-none"
                  />
                </div>
                <div>
                  <label className="block text-xs font-medium text-dark-200 mb-1">Username</label>
                  <input
                    type="text"
                    value={protectUser}
                    onChange={(e) => setProtectUser(e.target.value)}
                    placeholder="admin"
                    className="w-36 px-3 py-2 border border-dark-500 rounded-lg text-sm focus:ring-2 focus:ring-accent-500 focus:border-accent-500 outline-none"
                  />
                </div>
                <div>
                  <label className="block text-xs font-medium text-dark-200 mb-1">Password</label>
                  <input
                    type="password"
                    value={protectPass}
                    onChange={(e) => setProtectPass(e.target.value)}
                    placeholder="password"
                    className="w-36 px-3 py-2 border border-dark-500 rounded-lg text-sm focus:ring-2 focus:ring-accent-500 focus:border-accent-500 outline-none"
                  />
                </div>
                <button
                  onClick={async () => {
                    if (!protectUser || !protectPass) return;
                    setProtectMsg("");
                    try {
                      await api.post(`/sites/${id}/password-protect`, {
                        path: protectPath || "/",
                        username: protectUser,
                        password: protectPass,
                      });
                      setProtectUser("");
                      setProtectPass("");
                      loadProtected();
                      setProtectMsg("Protection enabled");
                    } catch (e) {
                      setProtectMsg(e instanceof Error ? e.message : "Failed");
                    }
                  }}
                  className="px-4 py-2 bg-rust-500 text-white rounded-lg text-sm font-medium hover:bg-rust-600 transition-colors"
                >
                  Protect
                </button>
              </div>

              {protectMsg && (
                <p className={`text-xs ${protectMsg.includes("Failed") || protectMsg.includes("failed") ? "text-danger-400" : "text-rust-400"}`}>
                  {protectMsg}
                </p>
              )}
            </div>
          )}
        </div>
      )}

      {/* Domain Aliases */}
      {site.status === "active" && (
        <div className="bg-dark-800 rounded-lg border border-dark-500 overflow-hidden mt-6">
          <button
            onClick={() => setShowAliases(!showAliases)}
            className="w-full px-5 py-4 border-b border-dark-600 flex items-center justify-between text-left"
          >
            <h2 className="text-xs font-medium text-dark-300 uppercase font-mono tracking-widest">Domain Aliases</h2>
            <div className="flex items-center gap-2">
              {aliases.length > 0 && (
                <span className="text-xs text-dark-200">{aliases.length} alias{aliases.length !== 1 ? "es" : ""}</span>
              )}
              <svg className={`w-4 h-4 text-dark-300 transition-transform ${showAliases ? "rotate-180" : ""}`} fill="none" viewBox="0 0 24 24" stroke="currentColor" strokeWidth={2}>
                <path strokeLinecap="round" strokeLinejoin="round" d="M19.5 8.25l-7.5 7.5-7.5-7.5" />
              </svg>
            </div>
          </button>
          {showAliases && (
            <div className="p-5 space-y-4">
              <p className="text-xs text-dark-300">
                Additional domains that serve the same site content. DNS must point to this server.
              </p>

              {/* Current aliases */}
              {aliases.length > 0 && (
                <div className="space-y-2">
                  {aliases.map((alias, i) => (
                    <div key={i} className="flex items-center justify-between bg-dark-900 rounded-lg px-4 py-3">
                      <span className="text-sm text-dark-100 font-mono">{alias}</span>
                      <button
                        onClick={async () => {
                          try {
                            await api.post(`/sites/${id}/aliases/remove`, { alias });
                            loadAliases();
                            setAliasMsg("Alias removed");
                          } catch (e) {
                            setAliasMsg(e instanceof Error ? e.message : "Failed");
                          }
                        }}
                        className="p-1 text-dark-300 hover:text-danger-500 transition-colors"
                        title="Remove"
                      >
                        <svg className="w-4 h-4" fill="none" viewBox="0 0 24 24" stroke="currentColor" strokeWidth={1.5}>
                          <path strokeLinecap="round" strokeLinejoin="round" d="M6 18L18 6M6 6l12 12" />
                        </svg>
                      </button>
                    </div>
                  ))}
                </div>
              )}

              {/* Add alias form */}
              <div className="flex items-end gap-3">
                <div className="flex-1">
                  <label className="block text-xs font-medium text-dark-200 mb-1">Domain Alias</label>
                  <input
                    type="text"
                    value={newAlias}
                    onChange={(e) => setNewAlias(e.target.value)}
                    placeholder="www.example.com"
                    className="w-full px-3 py-2 border border-dark-500 rounded-lg text-sm focus:ring-2 focus:ring-accent-500 focus:border-accent-500 outline-none"
                  />
                </div>
                <button
                  onClick={async () => {
                    if (!newAlias.trim()) return;
                    setAliasMsg("");
                    try {
                      await api.post(`/sites/${id}/aliases`, { alias: newAlias.trim() });
                      setNewAlias("");
                      loadAliases();
                      setAliasMsg("Alias added");
                    } catch (e) {
                      setAliasMsg(e instanceof Error ? e.message : "Failed");
                    }
                  }}
                  className="px-4 py-2 bg-rust-500 text-white rounded-lg text-sm font-medium hover:bg-rust-600 transition-colors"
                >
                  Add Alias
                </button>
              </div>

              {aliasMsg && (
                <p className={`text-xs ${aliasMsg.includes("Failed") || aliasMsg.includes("failed") ? "text-danger-400" : "text-rust-400"}`}>
                  {aliasMsg}
                </p>
              )}
            </div>
          )}
        </div>
      )}

      {/* Access Logs */}
      {site.status === "active" && (
        <div className="bg-dark-800 rounded-lg border border-dark-500 overflow-hidden mt-6">
          <button onClick={() => { setShowAccessLogs(!showAccessLogs); if (!showAccessLogs) loadLogs(logType === "php" ? "access" : logType as "access" | "error"); }}
            className="w-full px-5 py-4 flex items-center justify-between hover:bg-dark-700/30 transition-colors">
            <h2 className="text-xs font-medium text-dark-300 uppercase font-mono tracking-widest">Access Logs</h2>
            <svg className={`w-4 h-4 text-dark-300 transition-transform ${showAccessLogs ? "rotate-180" : ""}`} fill="none" viewBox="0 0 24 24" stroke="currentColor" strokeWidth={2}>
              <path strokeLinecap="round" strokeLinejoin="round" d="m19.5 8.25-7.5 7.5-7.5-7.5" />
            </svg>
          </button>
          {showAccessLogs && (
            <div className="border-t border-dark-600">
              <div className="flex items-center justify-between px-5 py-2 bg-dark-900">
                <div className="flex gap-2">
                  <button onClick={() => { setLogType("access"); loadLogs("access"); }}
                    className={`px-2 py-1 rounded text-xs font-medium ${logType === "access" ? "bg-rust-500/15 text-rust-400" : "text-dark-300 hover:text-dark-100"}`}>Access</button>
                  <button onClick={() => { setLogType("error"); loadLogs("error"); }}
                    className={`px-2 py-1 rounded text-xs font-medium ${logType === "error" ? "bg-danger-400/15 text-danger-400" : "text-dark-300 hover:text-dark-100"}`}>Error</button>
                  {site.runtime === "php" && (
                    <button onClick={() => { setLogType("php"); loadPhpErrors(); }}
                      className={`px-2 py-1 rounded text-xs font-medium ${logType === "php" ? "bg-warn-500/15 text-warn-400" : "text-dark-300 hover:text-dark-100"}`}>PHP Errors</button>
                  )}
                </div>
                <button onClick={() => logType === "php" ? loadPhpErrors() : loadLogs(logType as "access" | "error")} className="text-xs text-rust-400 hover:text-rust-300">Refresh</button>
              </div>
              <pre className="p-4 text-[11px] font-mono text-dark-200 bg-dark-950 max-h-80 overflow-y-auto overflow-x-auto whitespace-pre-wrap">
                {(logType === "access" ? accessLogs : logType === "error" ? errorLogs : phpErrors) || "No logs available"}
              </pre>
            </div>
          )}
        </div>
      )}

      {/* Traffic Stats */}
      {site.status === "active" && (
        <div className="bg-dark-800 rounded-lg border border-dark-500 overflow-hidden mt-6">
          <button onClick={() => { setShowStats(!showStats); if (!showStats) loadStats(); }}
            className="w-full px-5 py-4 flex items-center justify-between hover:bg-dark-700/30 transition-colors">
            <h2 className="text-xs font-medium text-dark-300 uppercase font-mono tracking-widest">Traffic Stats</h2>
            <svg className={`w-4 h-4 text-dark-300 transition-transform ${showStats ? "rotate-180" : ""}`} fill="none" viewBox="0 0 24 24" stroke="currentColor" strokeWidth={2}>
              <path strokeLinecap="round" strokeLinejoin="round" d="m19.5 8.25-7.5 7.5-7.5-7.5" />
            </svg>
          </button>
          {showStats && stats && (
            <div className="border-t border-dark-600 p-5">
              <div className="grid grid-cols-3 gap-4 mb-4">
                <div className="text-center">
                  <p className="text-2xl font-bold text-dark-50">{stats.requests.toLocaleString()}</p>
                  <p className="text-xs text-dark-300">Requests</p>
                </div>
                <div className="text-center">
                  <p className="text-2xl font-bold text-dark-50">{stats.unique_ips.toLocaleString()}</p>
                  <p className="text-xs text-dark-300">Unique IPs</p>
                </div>
                <div className="text-center">
                  <p className="text-2xl font-bold text-dark-50">{stats.bandwidth_mb} MB</p>
                  <p className="text-xs text-dark-300">Bandwidth</p>
                </div>
              </div>
              {stats.top_pages && stats.top_pages.length > 0 && (
                <div>
                  <h4 className="text-xs font-medium text-dark-300 uppercase font-mono tracking-widest mb-2">Top Pages</h4>
                  <div className="space-y-1">
                    {stats.top_pages.map((p: any, i: number) => (
                      <div key={i} className="flex items-center justify-between text-xs">
                        <span className="text-dark-100 font-mono truncate flex-1">{p.path}</span>
                        <span className="text-dark-300 ml-2 font-mono">{p.count}</span>
                      </div>
                    ))}
                  </div>
                </div>
              )}
              {stats.status_codes && Object.keys(stats.status_codes).length > 0 && (
                <div className="mt-4">
                  <h4 className="text-xs font-medium text-dark-300 uppercase font-mono tracking-widest mb-2">Status Codes</h4>
                  <div className="flex gap-3 flex-wrap">
                    {Object.entries(stats.status_codes).sort().map(([code, count]: [string, any]) => (
                      <div key={code} className={`px-2 py-1 rounded text-xs font-mono ${code.startsWith("2") ? "bg-rust-500/15 text-rust-400" : code.startsWith("3") ? "bg-accent-500/15 text-accent-400" : code.startsWith("4") ? "bg-warn-500/15 text-warn-400" : "bg-danger-500/15 text-danger-400"}`}>
                        {code}: {count}
                      </div>
                    ))}
                  </div>
                </div>
              )}
            </div>
          )}
        </div>
      )}

      {/* PHP Extensions */}
      {site?.runtime === "php" && (
        <div className="bg-dark-800 rounded-lg border border-dark-500 overflow-hidden mt-6">
          <button onClick={() => { setShowPhpExts(!showPhpExts); if (!showPhpExts) loadPhpExtensions(); }}
            className="w-full px-5 py-4 flex items-center justify-between hover:bg-dark-700/30 transition-colors">
            <h2 className="text-xs font-medium text-dark-300 uppercase font-mono tracking-widest">PHP Extensions</h2>
            <svg className={`w-4 h-4 text-dark-300 transition-transform ${showPhpExts ? "rotate-180" : ""}`} fill="none" viewBox="0 0 24 24" stroke="currentColor" strokeWidth={2}>
              <path strokeLinecap="round" strokeLinejoin="round" d="m19.5 8.25-7.5 7.5-7.5-7.5" />
            </svg>
          </button>
          {showPhpExts && phpExts && (
            <div className="border-t border-dark-600 p-5">
              <div className="mb-4">
                <h4 className="text-xs text-dark-300 uppercase font-mono mb-2">Installed ({phpExts.installed.length})</h4>
                <div className="flex flex-wrap gap-1.5">
                  {phpExts.installed.map(ext => (
                    <span key={ext} className="px-2 py-0.5 bg-rust-500/10 text-rust-400 rounded text-xs font-mono">{ext}</span>
                  ))}
                </div>
              </div>
              <div>
                <h4 className="text-xs text-dark-300 uppercase font-mono mb-2">Available to install</h4>
                <div className="flex flex-wrap gap-1.5">
                  {phpExts.available.filter(e => !phpExts.installed.includes(e)).map(ext => (
                    <button key={ext} disabled={installingExt === ext} onClick={async () => {
                      setInstallingExt(ext);
                      try {
                        await api.post("/php/extensions/install", { version: site?.php_version, extension: ext });
                        loadPhpExtensions();
                      } catch {}
                      finally { setInstallingExt(null); }
                    }} className="px-2 py-0.5 bg-dark-700 text-dark-200 rounded text-xs font-mono hover:bg-dark-600 disabled:opacity-50">
                      {installingExt === ext ? "..." : ext}
                    </button>
                  ))}
                </div>
              </div>
            </div>
          )}
        </div>
      )}

      {/* Environment Variables */}
      <div className="bg-dark-800 rounded-lg border border-dark-500 overflow-hidden mt-6">
        <button onClick={() => { setShowEnvVars(!showEnvVars); if (!showEnvVars) loadEnvVars(); }}
          className="w-full px-5 py-4 flex items-center justify-between hover:bg-dark-700/30 transition-colors">
          <h2 className="text-xs font-medium text-dark-300 uppercase font-mono tracking-widest">Environment Variables</h2>
          <svg className={`w-4 h-4 text-dark-300 transition-transform ${showEnvVars ? "rotate-180" : ""}`} fill="none" viewBox="0 0 24 24" stroke="currentColor" strokeWidth={2}>
            <path strokeLinecap="round" strokeLinejoin="round" d="m19.5 8.25-7.5 7.5-7.5-7.5" />
          </svg>
        </button>
        {showEnvVars && (
          <div className="border-t border-dark-600 p-5 space-y-2">
            {envVars.map((v, i) => (
              <div key={i} className="flex gap-2">
                <input value={v.key} onChange={e => { const n = [...envVars]; n[i] = { ...n[i], key: e.target.value }; setEnvVars(n); }}
                  placeholder="KEY" className="w-1/3 px-2 py-1.5 bg-dark-900 border border-dark-500 rounded text-xs font-mono text-dark-100 focus:ring-2 focus:ring-accent-500 outline-none" />
                <input value={v.value} onChange={e => { const n = [...envVars]; n[i] = { ...n[i], value: e.target.value }; setEnvVars(n); }}
                  placeholder="value" className="flex-1 px-2 py-1.5 bg-dark-900 border border-dark-500 rounded text-xs font-mono text-dark-100 focus:ring-2 focus:ring-accent-500 outline-none" />
                <button onClick={() => setEnvVars(envVars.filter((_, j) => j !== i))} className="text-danger-400 hover:text-danger-300 text-sm px-1">x</button>
              </div>
            ))}
            <div className="flex gap-2 items-center">
              <button onClick={() => setEnvVars([...envVars, { key: "", value: "" }])} className="text-xs text-rust-400 hover:text-rust-300">+ Add variable</button>
              <button disabled={savingEnv} onClick={async () => {
                setSavingEnv(true);
                setEnvMsg("");
                try {
                  await api.put(`/sites/${id}/env`, { vars: envVars.filter(v => v.key) });
                  setEnvMsg("Environment variables saved");
                } catch (e) { setEnvMsg(e instanceof Error ? e.message : "Save failed"); }
                finally { setSavingEnv(false); }
              }} className="px-3 py-1 bg-rust-500 text-white rounded text-xs font-medium hover:bg-rust-600 disabled:opacity-50">
                {savingEnv ? "Saving..." : "Save"}
              </button>
              {envMsg && (
                <span className={`text-xs ${envMsg.includes("saved") ? "text-rust-400" : "text-danger-400"}`}>{envMsg}</span>
              )}
            </div>
            <p className="text-xs text-dark-300">Saves to .env file in the site root. Node.js/Python apps are auto-restarted.</p>
          </div>
        )}
      </div>

      {/* SSL message */}
      {sslMessage && (
        <div className={`mt-4 px-4 py-3 rounded-lg text-sm ${
          sslMessage.includes("success")
            ? "bg-rust-500/10 text-rust-400 border border-rust-500/20"
            : "bg-danger-500/10 text-danger-400 border border-danger-500/20"
        }`}>
          {sslMessage}
        </div>
      )}
    </div>
  );
}
