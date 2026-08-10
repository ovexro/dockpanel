import { useState, useEffect, FormEvent } from "react";
import { Link } from "react-router-dom";
import { api } from "../api";
import { formatDate } from "../utils/format";
import { statusColors, runtimeLabels } from "../constants";
import ProvisionLog from "../components/ProvisionLog";
import { PrereqCallout, useDnsPrereq } from "../components/Prerequisite";
import { FieldHelp, InfoTip } from "../components/FieldHelp";
import PhpVersionPicker from "../components/PhpVersionPicker";
import { useAuth } from "../context/AuthContext";

interface Site {
  id: string;
  domain: string;
  runtime: string;
  status: string;
  ssl_enabled: boolean;
  enabled: boolean;
  parent_site_id: string | null;
  created_at: string;
  // Present only on the admin all-sites view (GET /api/admin/sites). The
  // owner-scoped list cannot carry them, and would not need to: every row it
  // returns belongs to the person reading it.
  user_id?: string;
  owner_email?: string | null;
  owner_role?: string | null;
}

interface PanelUser {
  id: string;
  email: string;
  role: string;
}

export default function Sites() {
  const { user } = useAuth();
  const [sites, setSites] = useState<Site[]>([]);
  const [loading, setLoading] = useState(true);
  const [showForm, setShowForm] = useState(false);
  const [error, setError] = useState("");
  const [provisioningSiteId, setProvisioningSiteId] = useState<string | null>(null);
  const [search, setSearch] = useState("");
  const [displayCount, setDisplayCount] = useState(25);

  // Admin all-sites view. Ownership is exclusive, so a site handed to a client
  // leaves the admin's own list entirely — this is how they find it again, and
  // the only place from which a transfer can be undone.
  const isAdmin = user?.role === "admin";
  /** The one role that may hold sites but never claim a new domain. */
  const isClient = user?.role === "client";
  const [allSites, setAllSites] = useState(false);
  const [users, setUsers] = useState<PanelUser[]>([]);
  const [transferFor, setTransferFor] = useState<Site | null>(null);
  const [transferEmail, setTransferEmail] = useState("");
  const [transferring, setTransferring] = useState(false);
  const [notice, setNotice] = useState("");

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

  // Preflight the domain as it is typed. Nothing here BLOCKS creation — a site
  // is perfectly valid before its DNS exists, and the brief is explicit that a
  // not-yet-propagated record must not be presented as an error. The point is
  // that the user learns about the prerequisite here, rather than discovering it
  // later when SSL silently fails to appear.
  const { prereq: dnsPrereq, checking: dnsChecking, recheck: recheckDns } =
    useDnsPrereq(showForm ? domain : "");

  const fetchSites = () => {
    api
      .get<Site[]>(allSites ? "/admin/sites" : "/sites")
      .then(setSites)
      .catch((e) => setError(e.message))
      .finally(() => setLoading(false));
  };

  useEffect(fetchSites, [allSites]);

  // The transfer recipient is picked, not typed. A free-text email could only
  // ever fail late — `transfer` answers 404 "No account with that email" — and
  // an operator does not carry their clients' addresses in their head.
  useEffect(() => {
    if (!isAdmin) return;
    api.get<PanelUser[]>("/users").then(setUsers).catch(() => setUsers([]));
  }, [isAdmin]);

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
          {isAdmin && (
            <label className="flex items-center gap-2 text-sm text-dark-200 select-none cursor-pointer">
              <input
                type="checkbox"
                checked={allSites}
                onChange={(e) => { setLoading(true); setAllSites(e.target.checked); }}
                className="rounded border-dark-500 bg-dark-800 text-rust-500 focus:ring-rust-500"
              />
              All sites on this server
            </label>
          )}
          {sites.length >= 2 && (
            <input
              type="text"
              value={search}
              onChange={(e) => setSearch(e.target.value)}
              placeholder="Search sites..."
              className="px-3 py-1.5 bg-dark-800 border border-dark-600 rounded-lg text-sm text-dark-100 placeholder-dark-400 focus:outline-none focus:border-dark-400"
            />
          )}
          {/* A `client` holds sites and cannot bring a new domain into service —
              that single refusal is what the role IS
              (`services::domain_claim::ensure_claimable`). Offering the control
              anyway meant the client filled in a domain, a runtime, a PHP version
              and, for a WordPress choice, an admin username and password, pressed
              Create, and only then met the refusal. Same predicate as
              `Dashboard.tsx:680`, which already hides its create tile. */}
          {!isClient && (
            <button
              onClick={() => setShowForm(!showForm)}
              className="px-4 py-2 bg-rust-500 text-white rounded-lg text-sm font-medium hover:bg-rust-600 transition-colors"
            >
              {showForm ? "Cancel" : "Create Site"}
            </button>
          )}
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
              <FieldHelp id="sites.create.domain" />
              <PrereqCallout
                prereq={dnsPrereq}
                onRecheck={recheckDns}
                checking={dnsChecking}
                showSatisfied
                className="mt-2"
              />
            </div>
            {!cms ? (
              <div>
                {/* The (i) sits beside the label, never inside it: a button
                    inside a <label> activates the labelled control, so opening
                    the tooltip would also drop the select's list over it. */}
                <div className="flex items-center mb-1">
                  <label htmlFor="site-runtime" className="block text-sm font-medium text-dark-100">Runtime</label>
                  <InfoTip id="sites.create.runtime" />
                </div>
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
                <FieldHelp id="sites.create.admin_password" />
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
              <FieldHelp id="sites.create.proxy_port" />
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

          {/* Shown for a CMS too. `handleCreate` sends `php_version` for every
              CMS install — a one-click WordPress is a PHP site — so gating this
              on `!cms` left the most common route to a PHP site submitting the
              form's untouched default with nothing on screen to say which
              versions the server has. That is the same failure the picker exists
              to prevent, on the path most people take. */}
          {(runtime === "php" || cms) && (
            <div className="grid grid-cols-1 md:grid-cols-2 gap-4">
              <div>
                <label htmlFor="site-php-version" className="block text-sm font-medium text-dark-100 mb-1">PHP Version</label>
                {/* The agent refuses to write a PHP vhost when the version's FPM
                    socket is missing, so an uninstalled version failed the whole
                    site creation with a message pointing somewhere that could not
                    fix it. The picker now says which versions this server has and
                    offers to install one that it does not. */}
                <PhpVersionPicker
                  id="site-php-version"
                  value={phpVersion}
                  onChange={(v) => setPhpVersion(v)}
                  canInstall={user?.role === "admin"}
                />
              </div>
              <div className={cms ? "hidden" : undefined}>
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
          {/* The empty state has to say something different to the one role that
              cannot act on it. Inviting a client to "create your first site" is
              the same broken promise as the button above, and it is worse here:
              this is the screen they reach when a site they DO own has not been
              transferred yet, so the panel answered "you have nothing, make one"
              to someone whose only route is to ask their administrator. */}
          {isClient ? (
            <p className="text-dark-300 text-sm mt-2 max-w-md mx-auto">
              No sites have been assigned to your account yet. Sites are created and handed
              over by your administrator — once one is, it appears here and you can manage it
              fully.
            </p>
          ) : (
            <>
              <p className="text-dark-300 text-sm mt-2 max-w-md mx-auto">Deploy static, PHP, Node.js, or Python sites with automatic SSL certificates, nginx configuration, and one-click CMS installs.</p>
              <button onClick={() => setShowForm(true)} className="mt-3 px-4 py-2 bg-rust-500 text-white rounded-lg text-sm font-medium hover:bg-rust-600 transition-colors">
                Create your first site
              </button>
            </>
          )}
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
                {allSites && (
                  <>
                    <th scope="col" className="text-left text-xs font-medium text-dark-200 uppercase tracking-widest font-mono px-5 py-3">Owner</th>
                    <th scope="col" className="text-right text-xs font-medium text-dark-200 uppercase tracking-widest font-mono px-5 py-3"><span className="sr-only">Actions</span></th>
                  </>
                )}
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
                    {/* Every row links to its own page, including a site this
                        admin does not own. That used to be rendered as inert
                        text, correctly, because a per-site read answered 404 for
                        anybody but the owner — so the operator who had handed a
                        site to a client could see it here and go no further.
                        An administrator now reaches any site on a machine they
                        run, so the link goes somewhere. */}
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
                      site.enabled === false ? "bg-warn-500/10 text-warn-400" : statusColors[site.status] || "bg-dark-700 text-dark-200"
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
                  {allSites && (
                    <>
                      <td className="px-5 py-4 text-sm">
                        <span className="text-dark-100 font-mono">{site.owner_email ?? "—"}</span>
                        {site.owner_role && site.owner_role !== "admin" && (
                          <span className="ml-2 inline-flex px-2 py-0.5 rounded-full text-[10px] font-medium bg-dark-700 text-dark-200 uppercase tracking-wide">
                            {site.owner_role}
                          </span>
                        )}
                      </td>
                      <td className="px-5 py-4 text-right">
                        <button
                          onClick={() => { setTransferFor(site); setTransferEmail(""); setNotice(""); }}
                          className="px-2.5 py-1 rounded text-xs font-medium bg-dark-700 text-dark-200 hover:bg-dark-600 transition-colors"
                        >
                          Transfer
                        </button>
                      </td>
                    </>
                  )}
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

      {notice && (
        <div role="status" className="mt-4 bg-dark-700 text-dark-100 text-sm px-4 py-3 rounded-lg border border-dark-500">
          {notice}
        </div>
      )}

      {/* Transfer, from the all-sites list rather than the site's own page.
          It has to live here: once a site belongs to somebody else its detail
          page answers 404 to the admin, so a control rendered only there can
          hand a site away and never take it back. */}
      {transferFor && (
        <div className="fixed inset-0 z-50 flex items-center justify-center bg-black/60 p-4" role="dialog" aria-modal="true" aria-labelledby="transfer-title">
          <div className="bg-dark-800 border border-dark-500 rounded-lg elevation-2 w-full max-w-md p-5">
            <h2 id="transfer-title" className="text-base font-medium text-dark-100">Transfer {transferFor.domain}</h2>
            <p className="text-xs text-dark-300 mt-1">
              Ownership is exclusive. The account you choose becomes the owner and the current
              one — {transferFor.owner_email ?? "the current owner"} — stops being it.
            </p>
            <label htmlFor="transfer-to" className="block text-sm font-medium text-dark-100 mt-4 mb-1">Transfer to</label>
            <select
              id="transfer-to"
              value={transferEmail}
              onChange={(e) => setTransferEmail(e.target.value)}
              className="w-full px-3 py-2.5 border border-dark-500 rounded-lg focus:ring-2 focus:ring-accent-500 outline-none text-sm bg-dark-800"
            >
              <option value="">Choose an account…</option>
              {users
                /* `suspended` is refused by the endpoint with a 409 — leaving it
                   out of the list is the same rule stated earlier. Every other
                   role is a legitimate destination, including admin, which is
                   how a transfer is undone. */
                .filter((u) => u.role !== "suspended" && u.email !== transferFor.owner_email)
                .map((u) => (
                  <option key={u.id} value={u.email}>{u.email} ({u.role})</option>
                ))}
            </select>
            <div className="flex items-center gap-2 mt-5">
              <button
                disabled={!transferEmail || transferring}
                onClick={() => {
                  setTransferring(true);
                  api.post(`/sites/${transferFor.id}/transfer`, { email: transferEmail })
                    .then(() => {
                      setNotice(`${transferFor.domain} transferred to ${transferEmail}.`);
                      setTransferFor(null);
                      fetchSites();
                    })
                    .catch((e) => setError(e instanceof Error ? e.message : "Transfer failed"))
                    .finally(() => setTransferring(false));
                }}
                className="px-4 py-2 bg-rust-500 text-white rounded-lg text-sm font-medium hover:bg-rust-600 disabled:opacity-50 transition-colors"
              >
                {transferring ? "Transferring…" : "Transfer"}
              </button>
              <button
                onClick={() => setTransferFor(null)}
                className="px-4 py-2 text-sm text-dark-300 border border-dark-600 rounded-lg hover:text-dark-100 hover:border-dark-400 transition-colors"
              >
                Cancel
              </button>
            </div>
          </div>
        </div>
      )}
      </div>
    </div>
  );
}
