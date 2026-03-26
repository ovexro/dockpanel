import { useState, useEffect, FormEvent } from "react";
import { Link } from "react-router-dom";
import { api } from "../api";
import { formatDate } from "../utils/format";
import { statusColors, runtimeLabels } from "../constants";
import ProvisionLog from "../components/ProvisionLog";

interface Site {
  id: string;
  domain: string;
  runtime: string;
  status: string;
  ssl_enabled: boolean;
  enabled: boolean;
  parent_site_id: string | null;
  created_at: string;
}

export default function Sites() {
  const [sites, setSites] = useState<Site[]>([]);
  const [loading, setLoading] = useState(true);
  const [showForm, setShowForm] = useState(false);
  const [error, setError] = useState("");
  const [provisioningSiteId, setProvisioningSiteId] = useState<string | null>(null);
  const [search, setSearch] = useState("");
  const [displayCount, setDisplayCount] = useState(25);

  // Form state
  const [domain, setDomain] = useState("");
  const [runtime, setRuntime] = useState("static");
  const [proxyPort, setProxyPort] = useState("");
  const [phpVersion, setPhpVersion] = useState("8.3");
  const [phpPreset, setPhpPreset] = useState("generic");
  const [appCommand, setAppCommand] = useState("");
  const [submitting, setSubmitting] = useState(false);
  const [cms, setCms] = useState("");
  const [siteTitle, setSiteTitle] = useState("");
  const [adminEmail, setAdminEmail] = useState("");
  const [adminUser, setAdminUser] = useState("admin");
  const [adminPassword, setAdminPassword] = useState("");

  const fetchSites = () => {
    api
      .get<Site[]>("/sites")
      .then(setSites)
      .catch((e) => setError(e.message))
      .finally(() => setLoading(false));
  };

  useEffect(fetchSites, []);

  const handleCreate = async (e: FormEvent) => {
    e.preventDefault();
    setError("");
    setSubmitting(true);
    try {
      const effectiveRuntime = cms ? "php" : runtime;
      const effectivePreset = cms || phpPreset;
      const body: Record<string, unknown> = { domain, runtime: effectiveRuntime };
      if (effectiveRuntime === "proxy") body.proxy_port = parseInt(proxyPort);
      if (effectiveRuntime === "node" || effectiveRuntime === "python") {
        body.app_command = appCommand;
      }
      if (effectiveRuntime === "php") {
        body.php_version = phpVersion;
        body.php_preset = effectivePreset;
      }
      if (cms) {
        body.cms = cms;
        if (siteTitle) body.site_title = siteTitle;
        if (adminEmail) body.admin_email = adminEmail;
        if (adminUser) body.admin_user = adminUser;
        if (adminPassword) body.admin_password = adminPassword;
      }

      const created = await api.post<Site>("/sites", body);
      setShowForm(false);
      setProvisioningSiteId(created.id);
      setDomain("");
      setRuntime("static");
      setProxyPort("");
      setCms("");
      setSiteTitle("");
      setAdminEmail("");
      setAdminUser("admin");
      setAdminPassword("");
      fetchSites();
    } catch (err) {
      setError(err instanceof Error ? err.message : "Failed to create site");
    } finally {
      setSubmitting(false);
    }
  };

  const handleProvisionComplete = () => {
    setProvisioningSiteId(null);
    fetchSites();
  };

  return (
    <div className="animate-fade-up">
      <div className="page-header">
        <div>
          <h1 className="page-header-title">Sites</h1>
          <p className="page-header-subtitle">Manage your websites and applications</p>
        </div>
        <div className="flex items-center gap-2">
          {sites.length >= 2 && (
            <input
              type="text"
              value={search}
              onChange={(e) => setSearch(e.target.value)}
              placeholder="Search sites..."
              className="px-3 py-1.5 bg-dark-800 border border-dark-600 rounded-lg text-sm text-dark-100 placeholder-dark-400 focus:outline-none focus:border-dark-400"
            />
          )}
          <button
            onClick={() => setShowForm(!showForm)}
            className="px-4 py-2 bg-rust-500 text-white rounded-lg text-sm font-medium hover:bg-rust-600 transition-colors"
          >
            {showForm ? "Cancel" : "Create Site"}
          </button>
        </div>
      </div>

      <div className="p-6 lg:p-8">

      {error && (
        <div role="alert" className="bg-danger-500/10 text-danger-400 text-sm px-4 py-3 rounded-lg border border-danger-500/20 mb-4">
          {error}
          <button onClick={() => setError("")} className="float-right font-bold" aria-label="Close error">&times;</button>
        </div>
      )}

      {/* Provisioning log */}
      {provisioningSiteId && (
        <ProvisionLog siteId={provisioningSiteId} onComplete={handleProvisionComplete} />
      )}

      {/* Create form */}
      {showForm && (
        <div className="mb-6">
        <form
          onSubmit={handleCreate}
          className="bg-dark-800 rounded-lg border border-dark-500 p-5 space-y-4"
        >
          {/* Quick CMS Install */}
          <div>
            <label className="block text-xs font-medium text-dark-200 mb-2">Quick Install</label>
            <div className="flex gap-2 overflow-x-auto pb-2 -mx-1 px-1">
              {[
                { id: "", label: "Custom Site", desc: "" },
                { id: "wordpress", label: "WordPress", desc: "Blog & CMS" },
                { id: "laravel", label: "Laravel", desc: "PHP Framework" },
                { id: "drupal", label: "Drupal", desc: "Enterprise CMS" },
                { id: "joomla", label: "Joomla", desc: "CMS" },
                { id: "symfony", label: "Symfony", desc: "PHP Framework" },
                { id: "codeigniter", label: "CodeIgniter", desc: "PHP Framework" },
              ].map((c) => (
                <button key={c.id} type="button" onClick={() => { setCms(c.id); if (c.id) { setRuntime("php"); setPhpPreset(c.id || "generic"); } else { setRuntime("static"); } }}
                  className={`flex-shrink-0 px-3 py-2 border text-sm transition-colors ${cms === c.id ? "border-dark-50/30 bg-dark-50/5 text-dark-50" : "border-dark-500 bg-dark-900/50 text-dark-300 hover:border-dark-400"}`}
                >
                  {c.label}
                </button>
              ))}
            </div>
          </div>

          <div className="grid grid-cols-1 md:grid-cols-2 gap-4">
            <div>
              <label htmlFor="site-domain" className="block text-sm font-medium text-dark-100 mb-1">Domain</label>
              <input
                id="site-domain"
                type="text"
                value={domain}
                onChange={(e) => setDomain(e.target.value)}
                required
                placeholder="example.com"
                className="w-full px-3 py-2.5 border border-dark-500 rounded-lg focus:ring-2 focus:ring-accent-500 focus:border-accent-500 outline-none text-sm"
              />
              <p className="text-xs text-dark-400 mt-1.5">Your site's public domain name (e.g., example.com)</p>
            </div>
            {!cms ? (
              <div>
                <label htmlFor="site-runtime" className="block text-sm font-medium text-dark-100 mb-1">Runtime</label>
                <select
                  id="site-runtime"
                  value={runtime}
                  onChange={(e) => setRuntime(e.target.value)}
                  className="w-full px-3 py-2.5 border border-dark-500 rounded-lg focus:ring-2 focus:ring-accent-500 focus:border-accent-500 outline-none text-sm bg-dark-800"
                >
                  <option value="static">Static (HTML/CSS/JS)</option>
                  <option value="php">PHP</option>
                  <option value="node">Node.js</option>
                  <option value="python">Python</option>
                  <option value="proxy">Reverse Proxy</option>
                </select>
                <p className="text-xs text-dark-400 mt-1.5">
                  {runtime === "node" ? "Node.js app with managed process (systemd + nginx reverse proxy)" :
                   runtime === "python" ? "Python app with managed process (systemd + nginx reverse proxy)" :
                   runtime === "proxy" ? "Reverse proxy to a port (Docker container or external service)" :
                   runtime === "php" ? "PHP with PHP-FPM — WordPress, Laravel, Drupal, etc." :
                   "Static HTML/CSS/JS files served by nginx"}
                </p>
              </div>
            ) : (
              <div>
                <label className="block text-sm font-medium text-dark-100 mb-1">Site Title</label>
                <input type="text" value={siteTitle} onChange={(e) => setSiteTitle(e.target.value)} placeholder={`My ${cms.charAt(0).toUpperCase() + cms.slice(1)} Site`} className="w-full px-3 py-2.5 border border-dark-500 rounded-lg focus:ring-2 focus:ring-accent-500 outline-none text-sm" />
                <p className="text-xs text-dark-400 mt-1.5">The title for your {cms.charAt(0).toUpperCase() + cms.slice(1)} site</p>
              </div>
            )}
          </div>

          {/* CMS Admin Fields */}
          {cms && (
            <>
            <div className="col-span-2 border-t border-dark-700 pt-3 mt-1">
              <span className="text-xs font-medium text-dark-400 uppercase tracking-wider">{cms.charAt(0).toUpperCase() + cms.slice(1)} Configuration</span>
            </div>
            <div className="grid grid-cols-1 md:grid-cols-3 gap-4">
              <div>
                <label className="block text-sm font-medium text-dark-100 mb-1">Admin Email</label>
                <input type="email" value={adminEmail} onChange={(e) => setAdminEmail(e.target.value)} placeholder="you@example.com" className="w-full px-3 py-2.5 border border-dark-500 rounded-lg focus:ring-2 focus:ring-accent-500 outline-none text-sm" />
                <p className="text-xs text-dark-400 mt-1.5">{cms.charAt(0).toUpperCase() + cms.slice(1)} admin email address</p>
              </div>
              <div>
                <label className="block text-sm font-medium text-dark-100 mb-1">Admin Username</label>
                <input type="text" value={adminUser} onChange={(e) => setAdminUser(e.target.value)} placeholder="admin" className="w-full px-3 py-2.5 border border-dark-500 rounded-lg focus:ring-2 focus:ring-accent-500 outline-none text-sm" />
              </div>
              <div>
                <label className="block text-sm font-medium text-dark-100 mb-1">Admin Password</label>
                <input type="password" value={adminPassword} onChange={(e) => setAdminPassword(e.target.value)} placeholder="Auto-generated if blank" className="w-full px-3 py-2.5 border border-dark-500 rounded-lg focus:ring-2 focus:ring-accent-500 outline-none text-sm" />
              </div>
            </div>
            </>
          )}

          {runtime === "proxy" && (
            <div>
              <label htmlFor="site-proxy-port" className="block text-sm font-medium text-dark-100 mb-1">Proxy Port</label>
              <input
                id="site-proxy-port"
                type="number"
                value={proxyPort}
                onChange={(e) => setProxyPort(e.target.value)}
                required
                placeholder="3000"
                min="1"
                max="65535"
                className="w-full px-3 py-2.5 border border-dark-500 rounded-lg focus:ring-2 focus:ring-accent-500 focus:border-accent-500 outline-none text-sm max-w-xs"
              />
              <p className="text-xs text-dark-400 mt-1.5">The local port your application listens on</p>
            </div>
          )}

          {(runtime === "node" || runtime === "python") && (
            <div>
              <label htmlFor="site-app-command" className="block text-sm font-medium text-dark-100 mb-1">Start Command</label>
              <input
                id="site-app-command"
                type="text"
                value={appCommand}
                onChange={(e) => setAppCommand(e.target.value)}
                required
                placeholder={runtime === "node" ? "npm start" : "gunicorn app:app"}
                className="w-full px-3 py-2.5 border border-dark-500 rounded-lg focus:ring-2 focus:ring-accent-500 focus:border-accent-500 outline-none text-sm font-mono"
              />
              <p className="text-xs text-dark-400 mt-1.5">
                {runtime === "node"
                  ? "e.g., npm start, node server.js, npx next start"
                  : "e.g., gunicorn app:app, uvicorn main:app, flask run"}
                {" "}— port auto-allocated via $PORT env var
              </p>
            </div>
          )}

          {runtime === "php" && !cms && (
            <div className="grid grid-cols-1 md:grid-cols-2 gap-4">
              <div>
                <label htmlFor="site-php-version" className="block text-sm font-medium text-dark-100 mb-1">PHP Version</label>
                <select
                  id="site-php-version"
                  value={phpVersion}
                  onChange={(e) => setPhpVersion(e.target.value)}
                  className="w-full px-3 py-2.5 border border-dark-500 rounded-lg focus:ring-2 focus:ring-accent-500 focus:border-accent-500 outline-none text-sm bg-dark-800"
                >
                  <option value="8.4">PHP 8.4</option>
                  <option value="8.3">PHP 8.3</option>
                  <option value="8.2">PHP 8.2</option>
                  <option value="8.1">PHP 8.1</option>
                </select>
              </div>
              <div>
                <label htmlFor="site-php-preset" className="block text-sm font-medium text-dark-100 mb-1">Framework</label>
                <select
                  id="site-php-preset"
                  value={phpPreset}
                  onChange={(e) => setPhpPreset(e.target.value)}
                  className="w-full px-3 py-2.5 border border-dark-500 rounded-lg focus:ring-2 focus:ring-accent-500 focus:border-accent-500 outline-none text-sm bg-dark-800"
                >
                  <option value="generic">Generic PHP</option>
                  <option value="laravel">Laravel</option>
                  <option value="wordpress">WordPress</option>
                  <option value="drupal">Drupal</option>
                  <option value="joomla">Joomla</option>
                  <option value="symfony">Symfony</option>
                  <option value="codeigniter">CodeIgniter</option>
                  <option value="magento">Magento</option>
                </select>
                <p className="text-xs text-dark-400 mt-1.5">Nginx configuration preset for your PHP framework</p>
              </div>
            </div>
          )}

          <div className="flex items-center gap-3 pt-2">
            <button
              type="submit"
              disabled={submitting}
              className="inline-flex items-center gap-2 px-6 py-2.5 bg-rust-500 text-white rounded-lg text-sm font-medium hover:bg-rust-600 disabled:opacity-50 transition-colors"
            >
              {submitting ? (
                <>
                  <span className="w-4 h-4 border-2 border-white/30 border-t-white rounded-full animate-spin" />
                  Creating...
                </>
              ) : "Create Site"}
            </button>
            <button
              type="button"
              onClick={() => setShowForm(false)}
              className="px-4 py-2 text-sm text-dark-300 border border-dark-600 rounded-lg hover:text-dark-100 hover:border-dark-400 transition-colors"
            >
              Cancel
            </button>
          </div>
        </form>
        </div>
      )}

      {/* Sites list */}
      {loading ? (
        <div className="bg-dark-800 rounded-lg border border-dark-500 animate-pulse">
          {[...Array(3)].map((_, i) => (
            <div key={i} className="px-5 py-4 border-b border-dark-600 last:border-0">
              <div className="h-5 bg-dark-700 rounded w-48" />
            </div>
          ))}
        </div>
      ) : !showForm && sites.length === 0 ? (
        <div className="bg-dark-800 rounded-lg border border-dark-500 p-12 text-center">
          <svg className="w-12 h-12 mx-auto text-dark-300 mb-4" fill="none" viewBox="0 0 24 24" stroke="currentColor" strokeWidth={1}>
            <path strokeLinecap="round" strokeLinejoin="round" d="M12 21a9.004 9.004 0 0 0 8.716-6.747M12 21a9.004 9.004 0 0 1-8.716-6.747M12 21c2.485 0 4.5-4.03 4.5-9S14.485 3 12 3m0 18c-2.485 0-4.5-4.03-4.5-9S9.515 3 12 3m0 0a8.997 8.997 0 0 1 7.843 4.582M12 3a8.997 8.997 0 0 0-7.843 4.582m15.686 0A11.953 11.953 0 0 1 12 10.5c-2.998 0-5.74-1.1-7.843-2.918m15.686 0A8.959 8.959 0 0 1 21 12c0 .778-.099 1.533-.284 2.253m0 0A17.919 17.919 0 0 1 12 16.5a17.92 17.92 0 0 1-8.716-2.247m0 0A9 9 0 0 1 3 12c0-1.47.353-2.856.978-4.082" />
          </svg>
          <p className="text-dark-200 font-medium text-lg">No sites yet</p>
          <p className="text-dark-300 text-sm mt-2 max-w-md mx-auto">Deploy static, PHP, Node.js, or Python sites with automatic SSL certificates, nginx configuration, and one-click CMS installs.</p>
          <button onClick={() => setShowForm(true)} className="mt-3 px-4 py-2 bg-rust-500 text-white rounded-lg text-sm font-medium hover:bg-rust-600 transition-colors">
            Create your first site
          </button>
        </div>
      ) : sites.length > 0 ? (
        <div className="bg-dark-800 rounded-lg border border-dark-500 overflow-x-auto elevation-1">
          <table className="w-full">
            <thead>
              <tr className="border-b border-dark-500 bg-dark-900">
                <th scope="col" className="text-left text-xs font-medium text-dark-200 uppercase tracking-widest font-mono px-5 py-3">Domain</th>
                <th scope="col" className="text-left text-xs font-medium text-dark-200 uppercase tracking-widest font-mono px-5 py-3 hidden sm:table-cell">Runtime</th>
                <th scope="col" className="text-left text-xs font-medium text-dark-200 uppercase tracking-widest font-mono px-5 py-3">Status</th>
                <th scope="col" className="text-left text-xs font-medium text-dark-200 uppercase tracking-widest font-mono px-5 py-3 hidden md:table-cell">SSL</th>
                <th scope="col" className="text-left text-xs font-medium text-dark-200 uppercase tracking-widest font-mono px-5 py-3 hidden lg:table-cell">Created</th>
              </tr>
            </thead>
            <tbody className="divide-y divide-dark-600">
              {(() => {
                const filtered = sites.filter((s) => !s.parent_site_id && s.domain.toLowerCase().includes(search.toLowerCase()));
                const displayed = filtered.slice(0, displayCount);
                const remaining = filtered.length - displayed.length;
                return (
                  <>
                  {displayed.map((site) => (
                <tr key={site.id} className="hover:bg-dark-700/30 transition-colors">
                  <td className="px-5 py-4">
                    <Link
                      to={`/sites/${site.id}`}
                      className="text-sm font-medium text-rust-400 hover:text-rust-300 font-mono"
                    >
                      {site.domain}
                    </Link>
                  </td>
                  <td className="px-5 py-4 text-sm text-dark-200 hidden sm:table-cell">
                    {runtimeLabels[site.runtime] || site.runtime}
                  </td>
                  <td className="px-5 py-4">
                    <span className={`inline-flex px-2.5 py-0.5 rounded-full text-xs font-medium ${
                      site.enabled === false ? "bg-amber-500/10 text-amber-400" : statusColors[site.status] || "bg-dark-700 text-dark-200"
                    }`}>
                      {site.enabled === false ? "disabled" : site.status}
                    </span>
                  </td>
                  <td className="px-5 py-4 hidden md:table-cell">
                    {site.ssl_enabled ? (
                      <span className="inline-flex items-center gap-1 text-xs text-rust-400">
                        <svg className="w-3.5 h-3.5" fill="currentColor" viewBox="0 0 20 20">
                          <path fillRule="evenodd" d="M10 1a4.5 4.5 0 0 0-4.5 4.5V9H5a2 2 0 0 0-2 2v6a2 2 0 0 0 2 2h10a2 2 0 0 0 2-2v-6a2 2 0 0 0-2-2h-.5V5.5A4.5 4.5 0 0 0 10 1Zm3 8V5.5a3 3 0 1 0-6 0V9h6Z" clipRule="evenodd" />
                        </svg>
                        Secure
                      </span>
                    ) : (
                      <span className="text-xs text-dark-300">None</span>
                    )}
                  </td>
                  <td className="px-5 py-4 text-sm text-dark-200 hidden lg:table-cell">
                    {formatDate(site.created_at)}
                  </td>
                </tr>
              ))}
              </>
                );
              })()}
            </tbody>
          </table>
          {(() => {
            const filtered = sites.filter((s) => !s.parent_site_id && s.domain.toLowerCase().includes(search.toLowerCase()));
            const remaining = filtered.length - displayCount;
            return remaining > 0 ? (
              <button
                onClick={() => setDisplayCount((c) => c + 25)}
                className="w-full py-2 text-sm text-dark-300 hover:text-dark-100 border-t border-dark-600 hover:bg-dark-700/30 transition-colors"
              >
                Show more ({remaining} remaining)
              </button>
            ) : null;
          })()}
        </div>
      ) : null}
      </div>
    </div>
  );
}
