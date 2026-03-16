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

  const fetchStaging = () => {
    api.get<StagingInfo>(`/sites/${id}/staging`).then(setStaging).catch(() => { /* no staging */ });
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
        <div className="bg-red-500/10 text-danger-400 px-4 py-3 rounded-lg border border-red-500/20">
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
          </div>
        </div>
        <div>
          {confirmDelete ? (
            <div className="flex items-center gap-2">
              <span className="text-sm text-danger-400">Are you sure?</span>
              <button
                onClick={handleDelete}
                disabled={deleting}
                className="px-3 py-1.5 bg-red-600 text-white rounded-lg text-sm hover:bg-red-700 disabled:opacity-50"
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
            <button
              onClick={handleDelete}
              className="px-4 py-2 bg-red-500/10 text-danger-400 rounded-lg text-sm font-medium hover:bg-red-500/20 transition-colors"
            >
              Delete Site
            </button>
          )}
        </div>
      </div>

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
                <div className="flex items-center gap-3">
                  <span className="text-dark-300">Not configured</span>
                  {site.status === "active" && (
                    <button
                      onClick={handleProvisionSSL}
                      disabled={provisioning}
                      className="px-3 py-1 bg-rust-500 text-white rounded-md text-xs font-medium hover:bg-rust-600 disabled:opacity-50 transition-colors"
                    >
                      {provisioning ? "Provisioning..." : "Enable SSL"}
                    </button>
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
          className="bg-dark-800 rounded-lg border border-dark-500 p-5 hover:border-indigo-300 hover:shadow-sm transition-all group"
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
          className="bg-dark-800 rounded-lg border border-dark-500 p-5 hover:border-indigo-300 hover:shadow-sm transition-all group"
        >
          <div className="flex items-center gap-3">
            <div className="p-2 bg-slate-500/10 rounded-lg text-slate-400 group-hover:bg-slate-500/20 transition-colors">
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
          className="bg-dark-800 rounded-lg border border-dark-500 p-5 hover:border-indigo-300 hover:shadow-sm transition-all group"
        >
          <div className="flex items-center gap-3">
            <div className="p-2 bg-rust-500/10 rounded-lg text-rust-500 group-hover:bg-indigo-100 transition-colors">
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
          className="bg-dark-800 rounded-lg border border-dark-500 p-5 hover:border-indigo-300 hover:shadow-sm transition-all group"
        >
          <div className="flex items-center gap-3">
            <div className="p-2 bg-violet-500/10 rounded-lg text-violet-400 group-hover:bg-violet-500/20 transition-colors">
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
          className="bg-dark-800 rounded-lg border border-dark-500 p-5 hover:border-indigo-300 hover:shadow-sm transition-all group"
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
            className="bg-dark-800 rounded-lg border border-dark-500 p-5 hover:border-indigo-300 hover:shadow-sm transition-all group"
          >
            <div className="flex items-center gap-3">
              <div className="p-2 bg-blue-500/10 rounded-lg text-blue-400 group-hover:bg-blue-500/20 transition-colors">
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
                    className="px-3 py-1.5 bg-red-500/10 text-danger-400 rounded-lg text-xs font-medium hover:bg-red-500/20 transition-colors"
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

      {/* SSL message */}
      {sslMessage && (
        <div className={`mt-4 px-4 py-3 rounded-lg text-sm ${
          sslMessage.includes("success")
            ? "bg-rust-500/10 text-rust-400 border border-rust-500/20"
            : "bg-red-500/10 text-danger-400 border border-red-500/20"
        }`}>
          {sslMessage}
        </div>
      )}
    </div>
  );
}
