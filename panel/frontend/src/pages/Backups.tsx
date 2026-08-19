import { useState, useEffect } from "react";
import { useParams, Link } from "react-router-dom";
import { api } from "../api";
import { useAuth } from "../context/AuthContext";
import { formatSize, formatDate } from "../utils/format";
import ProvisionLog from "../components/ProvisionLog";

interface Backup {
  id: string;
  site_id: string;
  filename: string;
  size_bytes: number;
  created_at: string;
  /** Databases actually inside this archive. 0 for any backup made before v2.34.0. */
  databases_included: number;
  /** Databases the site had when the backup ran. */
  databases_expected: number;
}

/** What a given archive can actually give back. Per-row, because the answer
 *  differs between archives: older ones hold files only, and a restore that
 *  silently returns a site without its content is the failure mode this whole
 *  column exists to prevent. */
function backupContents(b: Backup): { label: string; complete: boolean; title: string } {
  if (b.databases_expected === 0 && b.databases_included === 0) {
    return {
      label: "Files",
      complete: true,
      title:
        "This archive holds the site's files. No database was attached to the site when it was made.",
    };
  }
  if (b.databases_included === 0) {
    return {
      label: "Files only",
      complete: false,
      title:
        "This archive holds the site's files but NO database — restoring it will not bring back posts, pages, users or settings.",
    };
  }
  if (b.databases_included < b.databases_expected) {
    return {
      label: `Files + ${b.databases_included}/${b.databases_expected} databases`,
      complete: false,
      title: `Only ${b.databases_included} of the site's ${b.databases_expected} databases are in this archive.`,
    };
  }
  return {
    label: `Files + ${b.databases_included} database${b.databases_included === 1 ? "" : "s"}`,
    complete: true,
    title: "This archive holds the site's files and its database content.",
  };
}

/** The sentence the operator reads BEFORE the restore is spawned.
 *
 *  `backupContents(b).title` is deliberately NOT reused here. Its reassuring
 *  branch keys on `databases_expected` — what the site had *when the backup
 *  ran* — while the backend decides on a live `SELECT ... FROM databases WHERE
 *  site_id = $1`. A site that gained a database since would be told "no
 *  database was attached" over exactly the case the restore then reports as an
 *  error. `.label` is safe because it states a fact about the archive alone. */
function restoreWarning(b: Backup): string {
  return `Unpacks ${backupContents(b).label.toLowerCase()} over the live site. Anything not in it stays as it is now.`;
}

interface Site {
  id: string;
  domain: string;
}

interface BackupSchedule {
  id: string;
  destination_id: string | null;
  schedule: string;
  retention_count: number;
  enabled: boolean;
  last_run: string | null;
  last_status: string | null;
}

interface BackupDestination {
  id: string;
  name: string;
  dtype: string;
}

const SCHEDULE_PRESETS = [
  { label: "Daily at 2 AM", value: "0 2 * * *" },
  { label: "Daily at 4 AM", value: "0 4 * * *" },
  { label: "Every 12 hours", value: "0 */12 * * *" },
  { label: "Weekly (Sun 3 AM)", value: "0 3 * * 0" },
  { label: "Monthly (1st, 3 AM)", value: "0 3 1 * *" },
];

export default function Backups() {
  const { id } = useParams<{ id: string }>();
  const [site, setSite] = useState<Site | null>(null);
  const [backups, setBackups] = useState<Backup[]>([]);
  const [loading, setLoading] = useState(true);
  const [creating, setCreating] = useState(false);
  const [restoring, setRestoring] = useState<string | null>(null);
  const [deleteTarget, setDeleteTarget] = useState<string | null>(null);
  // A restore drops and recreates every database in the archive over the live
  // one and replaces the site's files, with no automatic safety net — and the
  // guide documents a "Confirm the restore" step. Its own row already arms the
  // strictly smaller Delete; this is the same two-step, on the bigger control.
  const [restoreTarget, setRestoreTarget] = useState<string | null>(null);
  const [message, setMessage] = useState({ text: "", type: "" });
  const [backupSseId, setBackupSseId] = useState<string | null>(null);
  const [restoreSseId, setRestoreSseId] = useState<string | null>(null);

  // Schedule state
  const [schedule, setSchedule] = useState<BackupSchedule | null>(null);
  const [destinations, setDestinations] = useState<BackupDestination[]>([]);
  const [showScheduleForm, setShowScheduleForm] = useState(false);
  const [schedCron, setSchedCron] = useState("0 2 * * *");
  const [schedDestId, setSchedDestId] = useState("");
  const [schedRetention, setSchedRetention] = useState("7");
  const [savingSchedule, setSavingSchedule] = useState(false);
  // Whether the destination list came back at all. An empty picker and a picker
  // that failed to load render the same way unless something remembers which
  // happened, and the honest sentence to show is different for each.
  const [destinationsFailed, setDestinationsFailed] = useState(false);
  const { user } = useAuth();
  const isAdmin = user?.role === "admin";

  useEffect(() => {
    api.get<Site>(`/sites/${id}`).then(setSite).catch(() => {});
    loadBackups();
    loadSchedule();
    api.get<BackupDestination[]>("/backup-destinations/selectable")
      .then((d) => { setDestinations(d); setDestinationsFailed(false); })
      .catch(() => setDestinationsFailed(true));
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [id]);

  const loadSchedule = async () => {
    try {
      const data = await api.get<BackupSchedule | null>(`/sites/${id}/backup-schedule`);
      setSchedule(data);
      if (data) {
        setSchedCron(data.schedule);
        setSchedDestId(data.destination_id || "");
        setSchedRetention(String(data.retention_count));
      }
    } catch {
      // no schedule
    }
  };

  const loadBackups = async () => {
    setLoading(true);
    try {
      const data = await api.get<Backup[]>(`/sites/${id}/backups`);
      setBackups(data);
    } catch (e) {
      setMessage({
        text: e instanceof Error ? e.message : "Failed to load backups",
        type: "error",
      });
    } finally {
      setLoading(false);
    }
  };

  const handleCreate = async () => {
    setCreating(true);
    setMessage({ text: "", type: "" });
    try {
      const result = await api.post<{ backup_id?: string }>(`/sites/${id}/backups`);
      if (result.backup_id) {
        setBackupSseId(result.backup_id);
      } else {
        setMessage({ text: "Backup created successfully", type: "success" });
        setCreating(false);
        loadBackups();
      }
    } catch (e) {
      setMessage({
        text: e instanceof Error ? e.message : "Backup failed",
        type: "error",
      });
      setCreating(false);
    }
  };

  const handleRestore = async (backupId: string) => {
    setRestoring(backupId);
    setMessage({ text: "", type: "" });
    try {
      const result = await api.post<{ restore_id?: string }>(`/sites/${id}/backups/${backupId}/restore`);
      if (result.restore_id) {
        setRestoreSseId(result.restore_id);
      } else {
        setMessage({ text: "Backup restored successfully", type: "success" });
        setRestoring(null);
      }
    } catch (e) {
      setMessage({
        text: e instanceof Error ? e.message : "Restore failed",
        type: "error",
      });
      setRestoring(null);
    }
  };

  const handleDelete = async (backupId: string) => {
    try {
      await api.delete(`/sites/${id}/backups/${backupId}`);
      setDeleteTarget(null);
      setMessage({ text: "Backup deleted", type: "success" });
      loadBackups();
    } catch (e) {
      setMessage({
        text: e instanceof Error ? e.message : "Delete failed",
        type: "error",
      });
    }
  };

  return (
    <div className="p-6 lg:p-8">
      {/* Breadcrumb */}
      <div className="mb-6">
        <Link
          to={`/sites/${id}`}
          className="text-sm text-dark-200 hover:text-dark-50"
        >
          {site?.domain || "Site"}
        </Link>
        <span className="text-sm text-dark-300 mx-2">/</span>
        <span className="text-sm text-dark-50 font-medium">Backups</span>
      </div>

      {/* Header */}
      <div className="flex flex-col sm:flex-row sm:items-center justify-between gap-3 mb-6 pb-4 border-b border-dark-600">
        <div>
          <h1 className="text-sm font-medium text-dark-300 uppercase font-mono tracking-widest">Backups</h1>
          <p className="text-sm text-dark-200 mt-1 font-mono">{site?.domain}</p>
          {/* What an archive holds differs per archive: anything made before
              v2.34.0 is files only. Stating the general rule here and the actual
              contents per row is deliberate — someone who reads "Backup" as "my
              site is safe" otherwise finds out at the worst possible moment. */}
          <p className="text-xs text-dark-300 mt-2 max-w-xl">
            A backup contains this site&apos;s <strong className="text-dark-200">files and its
            databases</strong>, so restoring one brings back its content too. The Contains column
            shows what each archive actually holds &mdash; backups made before DockPanel included
            databases are marked <strong className="text-dark-200">Files only</strong>.
          </p>
        </div>
        <button
          onClick={handleCreate}
          disabled={creating}
          className="px-4 py-2 bg-rust-500 text-white rounded-lg text-sm font-medium hover:bg-rust-600 disabled:opacity-50 transition-colors flex items-center gap-2"
        >
          {creating ? (
            <>
              <div className="w-4 h-4 border-2 border-white/30 border-t-white rounded-full animate-spin" />
              Creating...
            </>
          ) : (
            <>
              <svg className="w-4 h-4" fill="none" viewBox="0 0 24 24" stroke="currentColor" strokeWidth={2}>
                <path strokeLinecap="round" strokeLinejoin="round" d="M12 4.5v15m7.5-7.5h-15" />
              </svg>
              Create Backup
            </>
          )}
        </button>
      </div>

      {message.text && (
        <div
          className={`mb-4 px-4 py-3 rounded-lg text-sm border ${
            message.type === "success"
              ? "bg-rust-500/10 text-rust-400 border-rust-500/20"
              : "bg-danger-500/10 text-danger-400 border-danger-500/20"
          }`}
          role="alert"
        >
          {message.text}
        </div>
      )}

      {/* Backup/Restore provisioning logs */}
      {backupSseId && (
        <ProvisionLog
          sseUrl={`/api/services/install/${backupSseId}/log`}
          onComplete={() => {
            setBackupSseId(null);
            setCreating(false);
            loadBackups();
          }}
        />
      )}
      {restoreSseId && (
        <ProvisionLog
          sseUrl={`/api/services/install/${restoreSseId}/log`}
          onComplete={() => {
            setRestoreSseId(null);
            setRestoring(null);
          }}
        />
      )}

      {/* Scheduled Backups */}
      <div className="bg-dark-800 rounded-lg border border-dark-500 overflow-hidden mb-6">
        <div className="px-5 py-3 border-b border-dark-600 flex items-center justify-between">
          <h3 className="text-xs font-medium text-dark-300 uppercase font-mono tracking-widest">Scheduled Backup</h3>
          {!showScheduleForm && (
            <button
              onClick={() => setShowScheduleForm(true)}
              className="px-3 py-1 bg-rust-500 text-white rounded-md text-xs font-medium hover:bg-rust-600"
            >
              {schedule ? "Edit" : "Set Up"}
            </button>
          )}
        </div>
        <div className="p-5">
          {showScheduleForm ? (
            <div className="space-y-3">
              <div className="grid grid-cols-2 gap-3">
                <div>
                  <label className="block text-xs font-medium text-dark-100 mb-1">Schedule</label>
                  <select value={SCHEDULE_PRESETS.some(p => p.value === schedCron) ? schedCron : "custom"} onChange={(e) => { if (e.target.value !== "custom") setSchedCron(e.target.value); }} className="w-full px-3 py-2 border border-dark-500 rounded-lg text-sm bg-dark-800 focus:ring-2 focus:ring-accent-500 outline-none">
                    {SCHEDULE_PRESETS.map((p) => (
                      <option key={p.value} value={p.value}>{p.label}</option>
                    ))}
                    <option value="custom">Custom</option>
                  </select>
                  {!SCHEDULE_PRESETS.some(p => p.value === schedCron) && (
                    <input type="text" value={schedCron} onChange={(e) => setSchedCron(e.target.value)} placeholder="0 2 * * *" className="w-full mt-2 px-3 py-2 border border-dark-500 rounded-lg text-sm focus:ring-2 focus:ring-accent-500 outline-none" />
                  )}
                </div>
                <div>
                  <label className="block text-xs font-medium text-dark-100 mb-1">Destination</label>
                  <select value={schedDestId} onChange={(e) => setSchedDestId(e.target.value)} className="w-full px-3 py-2 border border-dark-500 rounded-lg text-sm bg-dark-800 focus:ring-2 focus:ring-accent-500 outline-none">
                    <option value="">Select destination...</option>
                    {destinations.map((d) => (
                      <option key={d.id} value={d.id}>{d.name} ({d.dtype})</option>
                    ))}
                  </select>
                  {destinationsFailed ? (
                    <p className="text-xs text-danger-400 mt-1">
                      Could not load destinations. Reload the page, or contact your
                      administrator if it keeps happening.
                    </p>
                  ) : destinations.length === 0 && (
                    <p className="text-xs text-warn-500 mt-1">
                      {/* Backup Manager is administrator-only, so a client is told who
                          can act rather than handed a door that bounces them back. */}
                      No destinations yet —{" "}
                      {isAdmin ? (
                        <Link to="/backup-orchestrator?tab=destinations" className="underline hover:text-warn-400">
                          add one in Backup Manager
                        </Link>
                      ) : (
                        "ask your administrator to add one"
                      )}
                    </p>
                  )}
                </div>
              </div>
              <div>
                <label className="block text-xs font-medium text-dark-100 mb-1">Keep last N backups</label>
                <input type="number" value={schedRetention} onChange={(e) => setSchedRetention(e.target.value)} min="1" max="365" className="w-32 px-3 py-2 border border-dark-500 rounded-lg text-sm focus:ring-2 focus:ring-accent-500 outline-none" />
              </div>
              <div className="flex justify-end gap-2">
                <button onClick={() => setShowScheduleForm(false)} className="px-3 py-1.5 text-sm text-dark-100 bg-dark-700 rounded-lg hover:bg-dark-600">Cancel</button>
                {schedule && (
                  <button
                    onClick={async () => {
                      try {
                        await api.delete(`/sites/${id}/backup-schedule`);
                        setSchedule(null);
                        setShowScheduleForm(false);
                        setMessage({ text: "Schedule removed", type: "success" });
                      } catch (e) {
                        setMessage({ text: e instanceof Error ? e.message : "Failed", type: "error" });
                      }
                    }}
                    className="px-3 py-1.5 text-sm text-danger-400 bg-danger-500/10 rounded-lg hover:bg-danger-500/20"
                  >
                    Remove
                  </button>
                )}
                <button
                  disabled={savingSchedule || !schedDestId}
                  onClick={async () => {
                    setSavingSchedule(true);
                    setMessage({ text: "", type: "" });
                    try {
                      await api.put(`/sites/${id}/backup-schedule`, {
                        destination_id: schedDestId,
                        schedule: schedCron,
                        retention_count: parseInt(schedRetention) || 7,
                        enabled: true,
                      });
                      loadSchedule();
                      setShowScheduleForm(false);
                      setMessage({ text: "Schedule saved", type: "success" });
                    } catch (e) {
                      setMessage({ text: e instanceof Error ? e.message : "Failed", type: "error" });
                    } finally {
                      setSavingSchedule(false);
                    }
                  }}
                  className="px-4 py-1.5 bg-rust-500 text-white rounded-lg text-sm font-medium hover:bg-rust-600 disabled:opacity-50"
                >
                  {savingSchedule ? "Saving..." : "Save"}
                </button>
              </div>
            </div>
          ) : schedule ? (
            <div className="flex items-center justify-between">
              <div>
                <p className="text-sm text-dark-50">
                  <span className="font-mono bg-dark-700 px-1.5 py-0.5 rounded text-xs">{schedule.schedule}</span>
                  <span className="mx-2 text-dark-300">|</span>
                  Keep {schedule.retention_count} backups
                  <span className={`ml-2 inline-flex px-2 py-0.5 rounded-full text-xs font-medium ${schedule.enabled ? "bg-rust-500/15 text-rust-400" : "bg-dark-700 text-dark-200"}`}>
                    {schedule.enabled ? "Active" : "Paused"}
                  </span>
                </p>
                {schedule.last_run && (
                  <p className="text-xs text-dark-200 mt-1 font-mono">
                    Last run: {new Date(schedule.last_run).toLocaleString()}
                    {schedule.last_status && (
                      <span className={`ml-1 ${schedule.last_status === "success" ? "text-rust-400" : "text-danger-400"}`}>
                        ({schedule.last_status})
                      </span>
                    )}
                  </p>
                )}
              </div>
            </div>
          ) : (
            <p className="text-sm text-dark-300 text-center">No scheduled backup configured</p>
          )}
        </div>
      </div>

      {/* Backups list */}
      <div className="bg-dark-800 rounded-lg border border-dark-500 overflow-x-auto">
        {loading ? (
          <div className="p-8 text-center text-dark-300">Loading...</div>
        ) : backups.length === 0 ? (
          <div className="p-12 text-center">
            <svg className="w-16 h-16 mx-auto text-dark-300 mb-4" fill="none" viewBox="0 0 24 24" stroke="currentColor" strokeWidth={1}>
              <path strokeLinecap="round" strokeLinejoin="round" d="M20.25 7.5l-.625 10.632a2.25 2.25 0 0 1-2.247 2.118H6.622a2.25 2.25 0 0 1-2.247-2.118L3.75 7.5m8.25 3v6.75m0 0-3-3m3 3 3-3M3.375 7.5h17.25c.621 0 1.125-.504 1.125-1.125v-1.5c0-.621-.504-1.125-1.125-1.125H3.375c-.621 0-1.125.504-1.125 1.125v1.5c0 .621.504 1.125 1.125 1.125Z" />
            </svg>
            <p className="text-dark-200 text-sm">No backups yet</p>
            <p className="text-dark-300 text-xs mt-1">
              Create your first backup to protect this site&apos;s files and database content
            </p>
          </div>
        ) : (
          <table className="w-full">
            <thead>
              <tr className="bg-dark-900 border-b border-dark-500">
                <th scope="col" className="text-left text-xs font-medium text-dark-200 uppercase font-mono tracking-widest px-5 py-3">
                  Filename
                </th>
                <th scope="col" className="text-left text-xs font-medium text-dark-200 uppercase font-mono tracking-widest px-5 py-3 w-48">
                  Contains
                </th>
                <th scope="col" className="text-left text-xs font-medium text-dark-200 uppercase font-mono tracking-widest px-5 py-3 w-24">
                  Size
                </th>
                <th scope="col" className="text-left text-xs font-medium text-dark-200 uppercase font-mono tracking-widest px-5 py-3 w-40">
                  Created
                </th>
                <th scope="col" className="text-right text-xs font-medium text-dark-200 uppercase font-mono tracking-widest px-5 py-3 w-40">
                  Actions
                </th>
              </tr>
            </thead>
            <tbody className="divide-y divide-dark-600">
              {backups.map((backup) => (
                <tr key={backup.id} className="hover:bg-dark-700/30 transition-colors">
                  <td className="px-5 py-4">
                    <div className="flex items-center gap-2">
                      <svg className="w-4 h-4 text-dark-300 shrink-0" fill="none" viewBox="0 0 24 24" stroke="currentColor" strokeWidth={1.5}>
                        <path strokeLinecap="round" strokeLinejoin="round" d="M20.25 7.5l-.625 10.632a2.25 2.25 0 0 1-2.247 2.118H6.622a2.25 2.25 0 0 1-2.247-2.118L3.75 7.5M10 11.25h4M3.375 7.5h17.25c.621 0 1.125-.504 1.125-1.125v-1.5c0-.621-.504-1.125-1.125-1.125H3.375c-.621 0-1.125.504-1.125 1.125v1.5c0 .621.504 1.125 1.125 1.125Z" />
                      </svg>
                      <span className="text-sm text-dark-50 font-mono">
                        {backup.filename}
                      </span>
                    </div>
                  </td>
                  <td className="px-5 py-4">
                    {(() => {
                      const c = backupContents(backup);
                      return (
                        <span
                          title={c.title}
                          className={`inline-flex items-center gap-1.5 px-2 py-0.5 rounded-md text-xs font-medium border ${
                            c.complete
                              ? "bg-dark-700 text-dark-200 border-dark-500"
                              : "bg-warn-500/10 text-warn-400 border-warn-500/20"
                          }`}
                        >
                          {!c.complete && (
                            <svg className="w-3 h-3 shrink-0" fill="none" viewBox="0 0 24 24" stroke="currentColor" strokeWidth={2}>
                              <path strokeLinecap="round" strokeLinejoin="round" d="M12 9v3.75m-9.303 3.376c-.866 1.5.217 3.374 1.948 3.374h14.71c1.73 0 2.813-1.874 1.948-3.374L13.949 3.378c-.866-1.5-3.032-1.5-3.898 0L2.697 16.126ZM12 15.75h.007v.008H12v-.008Z" />
                            </svg>
                          )}
                          {c.label}
                        </span>
                      );
                    })()}
                  </td>
                  <td className="px-5 py-4 text-sm text-dark-200 font-mono">
                    {formatSize(backup.size_bytes)}
                  </td>
                  <td className="px-5 py-4 text-sm text-dark-200 font-mono">
                    {formatDate(backup.created_at)}
                  </td>
                  <td className="px-5 py-4 text-right">
                    <div className="flex items-center justify-end gap-2 flex-wrap">
                      {restoreTarget === backup.id ? (
                        <div className="inline-flex items-center gap-1.5">
                          <span className="text-[10px] text-danger-400 font-mono mr-1">
                            {restoreWarning(backup)}
                          </span>
                          <button
                            type="button"
                            onClick={() => { setRestoreTarget(null); handleRestore(backup.id); }}
                            className="px-2.5 py-1 text-[10px] font-mono uppercase tracking-wider bg-danger-500/10 hover:bg-danger-500/20 text-danger-400 border border-danger-500/40 rounded"
                          >
                            Restore
                          </button>
                          <button
                            type="button"
                            onClick={() => setRestoreTarget(null)}
                            className="px-2.5 py-1 text-[10px] font-mono uppercase tracking-wider bg-dark-700 hover:bg-dark-600 text-dark-200 rounded"
                          >
                            Cancel
                          </button>
                        </div>
                      ) : (
                        <button
                          type="button"
                          onClick={() => setRestoreTarget(backup.id)}
                          disabled={restoring === backup.id}
                          title="Unpack this archive over the live site"
                          className="px-3 py-1 bg-warn-500/10 text-warn-400 rounded-md text-xs font-medium hover:bg-warn-400/15 disabled:opacity-50 transition-colors"
                        >
                          {restoring === backup.id ? "Restoring..." : "Restore"}
                        </button>
                      )}
                      {deleteTarget === backup.id ? (
                        <div className="flex items-center gap-1">
                          <button
                            onClick={() => handleDelete(backup.id)}
                            className="px-2 py-1 bg-danger-600 text-white rounded-md text-xs"
                          >
                            Confirm
                          </button>
                          <button
                            onClick={() => setDeleteTarget(null)}
                            className="px-2 py-1 bg-dark-600 text-dark-200 rounded-md text-xs"
                          >
                            Cancel
                          </button>
                        </div>
                      ) : (
                        <button
                          onClick={() => setDeleteTarget(backup.id)}
                          className="px-3 py-1 bg-danger-500/10 text-danger-400 rounded-md text-xs font-medium hover:bg-danger-500/20 transition-colors"
                        >
                          Delete
                        </button>
                      )}
                    </div>
                  </td>
                </tr>
              ))}
            </tbody>
          </table>
        )}
      </div>
    </div>
  );
}
