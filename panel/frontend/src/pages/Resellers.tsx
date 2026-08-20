import { useState, useEffect } from "react";
import { Navigate } from "react-router-dom";
import { useAuth } from "../context/AuthContext";
import { api } from "../api";
import ConfirmDialog from "../components/ConfirmDialog";

/**
 * The admin's door onto /api/resellers.
 *
 * Those eight handlers — list, create, get, update, remove, list_servers,
 * allocate_server, deallocate_server — shipped complete, routed and AdminUser-
 * gated, and had ZERO callers in this SPA. So the only reseller-shaped control
 * the panel offered an admin was the role dropdown in Users, which sets
 * `role='reseller'` without inserting a profile: the account it produces gets
 * the Reseller nav group and its own panel answers 404. Meanwhile README,
 * FEATURES, COMPARISON, the multi-server guide and the marketing site all
 * described reseller accounts, quotas, white-label branding and per-reseller
 * server allocation as working. An operator went looking for the creation form
 * those pages promise, found the sidebar's "Reseller Panel", clicked it, and got
 * `403 This endpoint is for resellers only` — which is issue #112.
 *
 * Two contract details worth keeping in view while editing this file:
 *  - The LIST row and the DETAIL row are different shapes. `ResellerListItem`
 *    carries `email` and no branding/disk fields; `ResellerProfile` carries the
 *    branding and disk fields and no `email`. Editing therefore needs BOTH: the
 *    email comes down from the row that was clicked, the rest from a GET.
 *  - `update` distinguishes THREE states on the three text columns since #117:
 *    an absent key keeps the stored value, an empty string clears it to NULL,
 *    and a value writes it. So the branding fields — panel name, logo, accent
 *    colour — can now be taken off again, which they could not before.
 *    The NUMERIC quotas are still COALESCE-only: `null` means KEEP, and a blank
 *    box cannot restore "unlimited", because the empty-string sentinel has no
 *    numeric spelling. The form still says so for those, and only those.
 */

interface ResellerListItem {
  id: string;
  user_id: string;
  email: string;
  panel_name: string | null;
  max_users: number | null;
  max_sites: number | null;
  max_databases: number | null;
  used_users: number;
  used_sites: number;
  used_databases: number;
  created_at: string;
}

interface ResellerProfile {
  id: string;
  user_id: string;
  panel_name: string | null;
  max_users: number | null;
  max_sites: number | null;
  max_databases: number | null;
  max_disk_mb: number | null;
  max_email_accounts: number | null;
  used_users: number;
  used_sites: number;
  used_databases: number;
  logo_url: string | null;
  accent_color: string | null;
  hide_branding: boolean;
  created_at: string;
  updated_at: string;
}

interface ResellerServer {
  id: string;
  reseller_id: string;
  server_id: string;
  server_name: string;
  created_at: string;
}

interface UserRow {
  id: string;
  email: string;
  role: string;
}

interface ServerRow {
  id: string;
  name: string;
}

// Numeric quotas only. The API cannot clear a NUMBER, so an empty box means
// "leave as it is" on edit and "no limit" on create. One helper so both
// directions agree. (The three text columns are clearable since #117 — they
// send the empty string rather than null and do not go through this helper.)
const numOrNull = (s: string): number | null => {
  const t = s.trim();
  if (t === "") return null;
  const n = Number(t);
  return Number.isFinite(n) ? n : null;
};
const numStr = (n: number | null | undefined): string =>
  n === null || n === undefined ? "" : String(n);

const quota = (used: number, max: number | null): string =>
  max === null ? `${used} / ∞` : `${used} / ${max}`;

export default function Resellers() {
  const { user } = useAuth();
  // Ahead of the loader effect: a redirect is an effect too, so a guard below it
  // lets the mount spend its requests on a 403 first.
  if (!user || user.role !== "admin") return <Navigate to="/" replace />;

  const [resellers, setResellers] = useState<ResellerListItem[]>([]);
  const [users, setUsers] = useState<UserRow[]>([]);
  const [servers, setServers] = useState<ServerRow[]>([]);
  const [loading, setLoading] = useState(true);
  const [message, setMessage] = useState({ text: "", type: "" });

  // Create / edit form
  const [showForm, setShowForm] = useState(false);
  const [editId, setEditId] = useState<string | null>(null);
  const [editEmail, setEditEmail] = useState("");
  const [formUserId, setFormUserId] = useState("");
  const [panelName, setPanelName] = useState("");
  const [maxUsers, setMaxUsers] = useState("");
  const [maxSites, setMaxSites] = useState("");
  const [maxDatabases, setMaxDatabases] = useState("");
  const [maxDiskMb, setMaxDiskMb] = useState("");
  const [maxEmailAccounts, setMaxEmailAccounts] = useState("");
  const [logoUrl, setLogoUrl] = useState("");
  const [accentColor, setAccentColor] = useState("");
  const [hideBranding, setHideBranding] = useState(false);
  const [saving, setSaving] = useState(false);

  // Per-row server allocation
  const [expanded, setExpanded] = useState<string | null>(null);
  const [allocations, setAllocations] = useState<ResellerServer[]>([]);
  const [allocLoading, setAllocLoading] = useState(false);
  const [allocPick, setAllocPick] = useState("");
  const [pendingDealloc, setPendingDealloc] = useState<ResellerServer | null>(null);

  // Demote confirmation — a banner rather than a row swap, because the
  // consequences are worth spelling out before the click.
  const [pendingDelete, setPendingDelete] = useState<ResellerListItem | null>(null);

  const loadResellers = async () => {
    try {
      setResellers(await api.get<ResellerListItem[]>("/resellers"));
    } catch (e) {
      setMessage({ text: e instanceof Error ? e.message : "Failed to load resellers", type: "error" });
    } finally {
      setLoading(false);
    }
  };

  useEffect(() => {
    loadResellers();
    // `create` refuses an admin (400) and 409s a user who already has a profile,
    // so the picker offers neither. Failing quietly is right: the list is the
    // page, the picker is only needed to add.
    api.get<UserRow[]>("/users").then(setUsers).catch(() => {});
    api.get<ServerRow[]>("/servers").then(setServers).catch(() => {});
  }, []);

  const resetForm = () => {
    setEditId(null);
    setEditEmail("");
    setFormUserId("");
    setPanelName("");
    setMaxUsers("");
    setMaxSites("");
    setMaxDatabases("");
    setMaxDiskMb("");
    setMaxEmailAccounts("");
    setLogoUrl("");
    setAccentColor("");
    setHideBranding(false);
    setShowForm(false);
  };

  const openEdit = async (r: ResellerListItem) => {
    setMessage({ text: "", type: "" });
    try {
      // The list row does not carry the branding or disk fields — only the
      // detail response does — so opening the editor from the list alone would
      // silently blank half the form on save.
      const p = await api.get<ResellerProfile>(`/resellers/${r.id}`);
      setEditId(p.id);
      setEditEmail(r.email);
      setPanelName(p.panel_name || "");
      setMaxUsers(numStr(p.max_users));
      setMaxSites(numStr(p.max_sites));
      setMaxDatabases(numStr(p.max_databases));
      setMaxDiskMb(numStr(p.max_disk_mb));
      setMaxEmailAccounts(numStr(p.max_email_accounts));
      setLogoUrl(p.logo_url || "");
      setAccentColor(p.accent_color || "");
      setHideBranding(p.hide_branding);
      setShowForm(true);
    } catch (e) {
      setMessage({ text: e instanceof Error ? e.message : "Failed to load reseller", type: "error" });
    }
  };

  const handleSave = async () => {
    setSaving(true);
    setMessage({ text: "", type: "" });
    try {
      if (editId) {
        await api.put(`/resellers/${editId}`, {
          // On an UPDATE the empty string means "clear it"; null means "leave it
          // alone" (see the three-state contract on the handler). Sending null
          // for a blank box made the white-label fields one-way — an operator
          // could set a panel name, a logo and an accent colour and never take
          // any of them off again. The create payload below keeps `|| null`,
          // where a blank box genuinely does mean "never set".
          panel_name: panelName,
          max_users: numOrNull(maxUsers),
          max_sites: numOrNull(maxSites),
          max_databases: numOrNull(maxDatabases),
          max_disk_mb: numOrNull(maxDiskMb),
          max_email_accounts: numOrNull(maxEmailAccounts),
          logo_url: logoUrl,
          accent_color: accentColor,
          hide_branding: hideBranding,
        });
        setMessage({ text: "Reseller updated", type: "success" });
      } else {
        await api.post("/resellers", {
          user_id: formUserId,
          panel_name: panelName || null,
          max_users: numOrNull(maxUsers),
          max_sites: numOrNull(maxSites),
          max_databases: numOrNull(maxDatabases),
          logo_url: logoUrl || null,
          accent_color: accentColor || null,
          hide_branding: hideBranding,
        });
        setMessage({ text: "Reseller created", type: "success" });
      }
      resetForm();
      loadResellers();
    } catch (e) {
      setMessage({ text: e instanceof Error ? e.message : "Failed to save", type: "error" });
    } finally {
      setSaving(false);
    }
  };

  const executeDelete = async () => {
    if (!pendingDelete) return;
    const target = pendingDelete;
    setPendingDelete(null);
    try {
      await api.delete(`/resellers/${target.id}`);
      setResellers((prev) => prev.filter((r) => r.id !== target.id));
      if (expanded === target.id) setExpanded(null);
      setMessage({ text: `${target.email} is no longer a reseller`, type: "success" });
    } catch (e) {
      setMessage({ text: e instanceof Error ? e.message : "Failed to remove", type: "error" });
    }
  };

  const toggleServers = async (r: ResellerListItem) => {
    if (expanded === r.id) {
      setExpanded(null);
      return;
    }
    setExpanded(r.id);
    setAllocations([]);
    setAllocPick("");
    setPendingDealloc(null);
    setAllocLoading(true);
    try {
      setAllocations(await api.get<ResellerServer[]>(`/resellers/${r.id}/servers`));
    } catch (e) {
      setMessage({ text: e instanceof Error ? e.message : "Failed to load allocations", type: "error" });
    } finally {
      setAllocLoading(false);
    }
  };

  const handleAllocate = async () => {
    if (!expanded || !allocPick) return;
    setMessage({ text: "", type: "" });
    try {
      await api.post(`/resellers/${expanded}/servers`, { server_id: allocPick });
      setAllocations(await api.get<ResellerServer[]>(`/resellers/${expanded}/servers`));
      setAllocPick("");
      setMessage({ text: "Server allocated", type: "success" });
    } catch (e) {
      setMessage({ text: e instanceof Error ? e.message : "Failed to allocate", type: "error" });
    }
  };

  const executeDealloc = async () => {
    if (!expanded || !pendingDealloc) return;
    const target = pendingDealloc;
    setPendingDealloc(null);
    try {
      // The path takes the SERVER id, not the allocation row id — and note the
      // `reseller_id` on this row is a USER id (reseller_servers references
      // users), so it must not be round-tripped as the profile id in the URL.
      await api.delete(`/resellers/${expanded}/servers/${target.server_id}`);
      setAllocations((prev) => prev.filter((a) => a.server_id !== target.server_id));
      setMessage({ text: "Server deallocated", type: "success" });
    } catch (e) {
      setMessage({ text: e instanceof Error ? e.message : "Failed to deallocate", type: "error" });
    }
  };

  // `create` 400s on an admin and 409s on a user who already has a profile, so
  // offering either is offering a door that refuses.
  const eligibleUsers = users.filter(
    (u) => u.role !== "admin" && !resellers.some((r) => r.user_id === u.id),
  );
  // Allocating a server twice violates a unique index and surfaces as an opaque
  // 500, so the picker only offers what is not already allocated.
  const availableServers = servers.filter(
    (s) => !allocations.some((a) => a.server_id === s.id),
  );

  return (
    <div>
      <div className="page-header">
        <div>
          <h1 className="page-header-title">Resellers</h1>
          <p className="page-header-subtitle">Promote accounts to resellers, set their quotas and branding, and allocate servers</p>
        </div>
        <div className="flex items-center gap-2">
          <button
            onClick={() => { resetForm(); setShowForm(true); }}
            className="px-4 py-2 bg-rust-500 text-white rounded-lg text-sm font-medium hover:bg-rust-600 transition-colors flex items-center gap-2"
          >
            <svg className="w-4 h-4" fill="none" viewBox="0 0 24 24" stroke="currentColor" strokeWidth={2}>
              <path strokeLinecap="round" strokeLinejoin="round" d="M12 4.5v15m7.5-7.5h-15" />
            </svg>
            Add Reseller
          </button>
        </div>
      </div>

      <div className="p-6 lg:p-8">
        {message.text && (
          <div
            className={`mb-4 px-4 py-3 rounded-lg text-sm border ${
              message.type === "success"
                ? "bg-rust-500/10 text-rust-400 border-rust-500/20"
                : "bg-danger-500/10 text-danger-400 border-danger-500/20"
            }`}
          >
            {message.text}
          </div>
        )}

        {/* Issue #120: this was a banner at the top of the page while the Remove
            button lives on a reseller row further down. The blast radius below is
            stated ONLY here and nowhere on the row, so an operator on a long list
            was confirming a demotion whose consequences were off-screen. */}
        {pendingDelete && (
          <ConfirmDialog
            label={`Remove reseller "${pendingDelete.email}"? They become an ordinary user, are signed out everywhere, lose every allocated server, and their ${pendingDelete.used_users} sub-account(s) stop being theirs. Nothing is deleted.`}
            confirmLabel="Remove"
            onConfirm={executeDelete}
            onCancel={() => setPendingDelete(null)}
          />
        )}

        <div className="bg-dark-800 rounded-lg border border-dark-500 overflow-x-auto">
          {loading ? (
            <div className="p-6 space-y-3 animate-pulse">
              {[...Array(4)].map((_, i) => (
                <div key={i} className="h-12 bg-dark-700 rounded w-full" />
              ))}
            </div>
          ) : resellers.length === 0 ? (
            <div className="p-12 text-center">
              <p className="text-dark-200 font-medium">No resellers</p>
              <p className="text-dark-400 text-sm mt-2">
                A reseller is an existing account promoted here. Promoting one gives it its own
                panel, quotas for the users, sites and databases it may create, and optional
                white-label branding.
              </p>
            </div>
          ) : (
            <table className="w-full">
              <thead>
                <tr className="bg-dark-900 border-b border-dark-500 text-left text-xs font-mono text-dark-400 uppercase">
                  <th className="px-5 py-3">Account</th>
                  <th className="px-5 py-3">Panel name</th>
                  <th className="px-5 py-3">Users</th>
                  <th className="px-5 py-3">Sites</th>
                  <th className="px-5 py-3">Databases</th>
                  <th className="px-5 py-3">Created</th>
                  <th className="px-5 py-3 text-right">Actions</th>
                </tr>
              </thead>
              <tbody>
                {resellers.map((r) => (
                  <>
                    <tr key={r.id} className="border-b border-dark-700 hover:bg-dark-700/50">
                      <td className="px-5 py-3 text-sm text-dark-50 font-mono">{r.email}</td>
                      <td className="px-5 py-3 text-sm text-dark-200">{r.panel_name || <span className="text-dark-400">-</span>}</td>
                      <td className="px-5 py-3 text-sm text-dark-200 font-mono">{quota(r.used_users, r.max_users)}</td>
                      <td className="px-5 py-3 text-sm text-dark-200 font-mono">{quota(r.used_sites, r.max_sites)}</td>
                      <td className="px-5 py-3 text-sm text-dark-200 font-mono">{quota(r.used_databases, r.max_databases)}</td>
                      <td className="px-5 py-3 text-sm text-dark-400 text-xs whitespace-nowrap">{new Date(r.created_at).toLocaleDateString()}</td>
                      <td className="px-5 py-3">
                        <div className="flex items-center justify-end gap-2">
                          <button onClick={() => toggleServers(r)} className="px-2 py-1 bg-dark-600 text-dark-100 rounded text-xs hover:bg-dark-500 transition-colors">
                            {expanded === r.id ? "Hide servers" : "Servers"}
                          </button>
                          <button onClick={() => openEdit(r)} className="px-2 py-1 bg-dark-600 text-dark-100 rounded text-xs hover:bg-dark-500 transition-colors">
                            Edit
                          </button>
                          <button onClick={() => setPendingDelete(r)} className="px-2 py-1 bg-dark-600 text-danger-400 rounded text-xs hover:bg-dark-500 transition-colors">
                            Remove
                          </button>
                        </div>
                      </td>
                    </tr>
                    {expanded === r.id && (
                      <tr key={`${r.id}-servers`} className="border-b border-dark-700 bg-dark-900/40">
                        <td colSpan={7} className="px-5 py-4">
                          <h4 className="text-xs font-medium text-dark-300 uppercase font-mono tracking-widest mb-3">
                            Servers allocated to {r.email}
                          </h4>
                          {allocLoading ? (
                            <div className="h-8 bg-dark-700 rounded w-64 animate-pulse" />
                          ) : (
                            <>
                              {allocations.length === 0 ? (
                                <p className="text-sm text-dark-400 mb-3">
                                  No servers allocated. A reseller can only place its users' sites on
                                  a server allocated here.
                                </p>
                              ) : (
                                <ul className="mb-3 space-y-1">
                                  {allocations.map((a) => (
                                    <li key={a.id} className="flex items-center justify-between max-w-xl">
                                      <span className="text-sm text-dark-100 font-mono">{a.server_name}</span>
                                      {pendingDealloc?.server_id === a.server_id ? (
                                        <span className="flex items-center gap-1">
                                          <button onClick={executeDealloc} className="px-2 py-1 bg-danger-500 text-white rounded text-xs">Confirm</button>
                                          <button onClick={() => setPendingDealloc(null)} className="px-2 py-1 bg-dark-600 text-dark-200 rounded text-xs">Cancel</button>
                                        </span>
                                      ) : (
                                        <button onClick={() => setPendingDealloc(a)} className="px-2 py-1 bg-dark-600 text-danger-400 rounded text-xs hover:bg-dark-500 transition-colors">
                                          Remove
                                        </button>
                                      )}
                                    </li>
                                  ))}
                                </ul>
                              )}
                              <div className="flex items-center gap-2">
                                <select
                                  value={allocPick}
                                  onChange={(e) => setAllocPick(e.target.value)}
                                  className="px-3 py-1.5 bg-dark-800 border border-dark-600 rounded-lg text-sm text-dark-100"
                                >
                                  <option value="">Select a server…</option>
                                  {availableServers.map((s) => (
                                    <option key={s.id} value={s.id}>{s.name}</option>
                                  ))}
                                </select>
                                <button
                                  onClick={handleAllocate}
                                  disabled={!allocPick}
                                  className="px-3 py-1.5 bg-rust-500 text-white rounded-lg text-sm font-medium hover:bg-rust-600 disabled:opacity-40 disabled:cursor-not-allowed transition-colors"
                                >
                                  Allocate
                                </button>
                              </div>
                            </>
                          )}
                        </td>
                      </tr>
                    )}
                  </>
                ))}
              </tbody>
            </table>
          )}
        </div>
      </div>

      {showForm && (
        <div
          className="fixed inset-0 bg-black/30 flex items-center justify-center z-50 dp-modal-overlay p-4"
          role="dialog"
          aria-labelledby="reseller-form-title"
          onKeyDown={(e) => { if (e.key === "Escape") resetForm(); }}
        >
          <div className="bg-dark-800 rounded-lg shadow-xl p-6 w-full max-w-lg max-h-[90vh] overflow-y-auto dp-modal">
            <h3 id="reseller-form-title" className="text-xs font-medium text-dark-300 uppercase font-mono tracking-widest mb-4">
              {editId ? `Edit ${editEmail}` : "Add Reseller"}
            </h3>
            <div className="space-y-4">
              {!editId && (
                <div>
                  <label htmlFor="reseller-user" className="block text-sm font-medium text-dark-100 mb-1">Account</label>
                  <select
                    id="reseller-user"
                    value={formUserId}
                    onChange={(e) => setFormUserId(e.target.value)}
                    className="w-full px-3 py-2 bg-dark-800 border border-dark-500 rounded-lg text-sm text-dark-100"
                    autoFocus
                  >
                    <option value="">Select an account…</option>
                    {eligibleUsers.map((u) => (
                      <option key={u.id} value={u.id}>{u.email} ({u.role})</option>
                    ))}
                  </select>
                  <p className="text-xs text-dark-400 mt-1">
                    An existing account is promoted. Administrators cannot be promoted, and an
                    account that is already a reseller is not listed.
                  </p>
                </div>
              )}

              <div>
                <label htmlFor="reseller-panel-name" className="block text-sm font-medium text-dark-100 mb-1">Panel name</label>
                <input
                  id="reseller-panel-name"
                  type="text"
                  maxLength={100}
                  value={panelName}
                  onChange={(e) => setPanelName(e.target.value)}
                  className="w-full px-3 py-2 border border-dark-500 rounded-lg text-sm focus:ring-2 focus:ring-accent-500 focus:border-accent-500"
                  placeholder="Acme Hosting"
                />
                <p className="text-xs text-dark-400 mt-1">Shown to this reseller in place of the DockPanel name. Up to 100 characters.</p>
              </div>

              <div className="grid grid-cols-3 gap-3">
                <div>
                  <label htmlFor="reseller-max-users" className="block text-sm font-medium text-dark-100 mb-1">Max users</label>
                  <input id="reseller-max-users" type="number" min={0} value={maxUsers} onChange={(e) => setMaxUsers(e.target.value)}
                    className="w-full px-3 py-2 border border-dark-500 rounded-lg text-sm" placeholder="∞" />
                </div>
                <div>
                  <label htmlFor="reseller-max-sites" className="block text-sm font-medium text-dark-100 mb-1">Max sites</label>
                  <input id="reseller-max-sites" type="number" min={0} value={maxSites} onChange={(e) => setMaxSites(e.target.value)}
                    className="w-full px-3 py-2 border border-dark-500 rounded-lg text-sm" placeholder="∞" />
                </div>
                <div>
                  <label htmlFor="reseller-max-databases" className="block text-sm font-medium text-dark-100 mb-1">Max databases</label>
                  <input id="reseller-max-databases" type="number" min={0} value={maxDatabases} onChange={(e) => setMaxDatabases(e.target.value)}
                    className="w-full px-3 py-2 border border-dark-500 rounded-lg text-sm" placeholder="∞" />
                </div>
              </div>

              {editId && (
                <div className="grid grid-cols-2 gap-3">
                  <div>
                    <label htmlFor="reseller-max-disk" className="block text-sm font-medium text-dark-100 mb-1">Max disk (MB)</label>
                    <input id="reseller-max-disk" type="number" min={0} value={maxDiskMb} onChange={(e) => setMaxDiskMb(e.target.value)}
                      className="w-full px-3 py-2 border border-dark-500 rounded-lg text-sm" placeholder="∞" />
                  </div>
                  <div>
                    <label htmlFor="reseller-max-email" className="block text-sm font-medium text-dark-100 mb-1">Max email accounts</label>
                    <input id="reseller-max-email" type="number" min={0} value={maxEmailAccounts} onChange={(e) => setMaxEmailAccounts(e.target.value)}
                      className="w-full px-3 py-2 border border-dark-500 rounded-lg text-sm" placeholder="∞" />
                  </div>
                  <p className="col-span-2 text-xs text-dark-400 -mt-1">
                    These two are recorded but not yet enforced anywhere. Disk and email quotas
                    have no consumer in the panel today.
                  </p>
                </div>
              )}

              <div className="grid grid-cols-2 gap-3">
                <div>
                  <label htmlFor="reseller-logo" className="block text-sm font-medium text-dark-100 mb-1">Logo URL</label>
                  <input id="reseller-logo" type="url" value={logoUrl} onChange={(e) => setLogoUrl(e.target.value)}
                    className="w-full px-3 py-2 border border-dark-500 rounded-lg text-sm" placeholder="https://…/logo.svg" />
                </div>
                <div>
                  <label htmlFor="reseller-accent" className="block text-sm font-medium text-dark-100 mb-1">Accent colour</label>
                  <input id="reseller-accent" type="text" value={accentColor} onChange={(e) => setAccentColor(e.target.value)}
                    className="w-full px-3 py-2 border border-dark-500 rounded-lg text-sm" placeholder="#ea580c" />
                </div>
              </div>

              <label className="flex items-center gap-2 text-sm text-dark-100">
                <input type="checkbox" checked={hideBranding} onChange={(e) => setHideBranding(e.target.checked)} className="rounded border-dark-500" />
                Hide DockPanel branding for this reseller
              </label>

              {editId && (
                <p className="text-xs text-dark-400">
                  Clearing the panel name, logo or accent colour removes it. A blank
                  quota leaves the stored number unchanged — those cannot be reset to
                  unlimited here.
                </p>
              )}
            </div>

            <div className="flex items-center justify-end gap-2 mt-6">
              <button onClick={resetForm} className="px-4 py-2 bg-dark-600 text-dark-100 rounded-lg text-sm hover:bg-dark-500 transition-colors">Cancel</button>
              <button
                onClick={handleSave}
                disabled={saving || (!editId && !formUserId)}
                className="px-4 py-2 bg-rust-500 text-white rounded-lg text-sm font-medium hover:bg-rust-600 disabled:opacity-40 disabled:cursor-not-allowed transition-colors"
              >
                {saving ? "Saving…" : editId ? "Save changes" : "Create reseller"}
              </button>
            </div>
          </div>
        </div>
      )}
    </div>
  );
}
