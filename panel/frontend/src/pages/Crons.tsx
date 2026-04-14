import { useState, useEffect, FormEvent } from "react";
import { useParams, Link } from "react-router-dom";
import { api } from "../api";
import { formatDate } from "../utils/format";

interface Cron {
  id: string;
  site_id: string;
  label: string;
  command: string;
  schedule: string;
  enabled: boolean;
  last_run: string | null;
  last_status: string | null;
  last_output: string | null;
  created_at: string;
}

interface Site {
  id: string;
  domain: string;
}

const PRESETS = [
  { label: "Every minute", value: "* * * * *" },
  { label: "Every 5 minutes", value: "*/5 * * * *" },
  { label: "Every 15 minutes", value: "*/15 * * * *" },
  { label: "Every hour", value: "0 * * * *" },
  { label: "Every 6 hours", value: "0 */6 * * *" },
  { label: "Every day at midnight", value: "0 0 * * *" },
  { label: "Every day at 3 AM", value: "0 3 * * *" },
  { label: "Every Sunday at midnight", value: "0 0 * * 0" },
  { label: "Every Monday at 6 AM", value: "0 6 * * 1" },
  { label: "1st of every month", value: "0 0 1 * *" },
];

export default function Crons() {
  const { id } = useParams<{ id: string }>();
  const [site, setSite] = useState<Site | null>(null);
  const [crons, setCrons] = useState<Cron[]>([]);
  const [loading, setLoading] = useState(true);
  const [showForm, setShowForm] = useState(false);
  const [message, setMessage] = useState({ text: "", type: "" });

  // Form state
  const [label, setLabel] = useState("");
  const [command, setCommand] = useState("");
  const [schedule, setSchedule] = useState("0 * * * *");
  const [submitting, setSubmitting] = useState(false);

  // Run state
  const [running, setRunning] = useState<string | null>(null);
  const [runOutput, setRunOutput] = useState<{ id: string; output: string; success: boolean } | null>(null);

  // Edit state
  const [editId, setEditId] = useState<string | null>(null);

  // Delete confirm
  const [deleteTarget, setDeleteTarget] = useState<string | null>(null);

  useEffect(() => {
    api.get<Site>(`/sites/${id}`).then(setSite).catch(() => {});
    loadCrons();
  }, [id]);

  const loadCrons = async () => {
    setLoading(true);
    try {
      const data = await api.get<Cron[]>(`/sites/${id}/crons`);
      setCrons(data);
    } catch (e) {
      setMessage({ text: e instanceof Error ? e.message : "Failed to load crons", type: "error" });
    } finally {
      setLoading(false);
    }
  };

  const handleCreate = async (e: FormEvent) => {
    e.preventDefault();
    setSubmitting(true);
    setMessage({ text: "", type: "" });
    try {
      await api.post(`/sites/${id}/crons`, { label, command, schedule });
      setShowForm(false);
      setLabel("");
      setCommand("");
      setSchedule("0 * * * *");
      setMessage({ text: "Cron job created", type: "success" });
      loadCrons();
    } catch (e) {
      setMessage({ text: e instanceof Error ? e.message : "Failed to create cron", type: "error" });
    } finally {
      setSubmitting(false);
    }
  };

  const handleToggle = async (cron: Cron) => {
    try {
      await api.put(`/sites/${id}/crons/${cron.id}`, { enabled: !cron.enabled });
      loadCrons();
    } catch (e) {
      setMessage({ text: e instanceof Error ? e.message : "Failed to update cron", type: "error" });
    }
  };

  const handleDelete = async (cronId: string) => {
    try {
      await api.delete(`/sites/${id}/crons/${cronId}`);
      setDeleteTarget(null);
      setMessage({ text: "Cron job deleted", type: "success" });
      loadCrons();
    } catch (e) {
      setMessage({ text: e instanceof Error ? e.message : "Delete failed", type: "error" });
    }
  };

  const handleRun = async (cron: Cron) => {
    setRunning(cron.id);
    setRunOutput(null);
    try {
      const result = await api.post<{ success: boolean; output: string }>(`/sites/${id}/crons/${cron.id}/run`);
      setRunOutput({ id: cron.id, output: result.output || "", success: result.success });
      loadCrons();
    } catch (e) {
      setMessage({ text: e instanceof Error ? e.message : "Run failed", type: "error" });
    } finally {
      setRunning(null);
    }
  };

  const handleEdit = async (cron: Cron) => {
    if (editId === cron.id) {
      // Save
      try {
        await api.put(`/sites/${id}/crons/${cron.id}`, { label, command, schedule });
        setEditId(null);
        setMessage({ text: "Cron job updated", type: "success" });
        loadCrons();
      } catch (e) {
        setMessage({ text: e instanceof Error ? e.message : "Update failed", type: "error" });
      }
    } else {
      // Enter edit mode
      setEditId(cron.id);
      setLabel(cron.label);
      setCommand(cron.command);
      setSchedule(cron.schedule);
    }
  };

  return (
    <div className="p-6 lg:p-8">
      {/* Breadcrumb */}
      <div className="mb-6">
        <Link to={`/sites/${id}`} className="text-sm text-dark-200 hover:text-dark-50">
          {site?.domain || "Site"}
        </Link>
        <span className="text-sm text-dark-300 mx-2">/</span>
        <span className="text-sm text-dark-50 font-medium">Cron Jobs</span>
      </div>

      {/* Header */}
      <div className="flex flex-col sm:flex-row sm:items-center justify-between gap-3 mb-6 pb-4 border-b border-dark-600">
        <div>
          <h1 className="text-sm font-medium text-dark-300 uppercase font-mono tracking-widest">Cron Jobs</h1>
          <p className="text-sm text-dark-200 font-mono mt-1">{site?.domain}</p>
        </div>
        <button
          onClick={() => { setShowForm(!showForm); setEditId(null); setLabel(""); setCommand(""); setSchedule("0 * * * *"); }}
          className="px-4 py-2 bg-rust-500 text-white rounded-lg text-sm font-medium hover:bg-rust-600 transition-colors"
        >
          {showForm ? "Cancel" : "Add Cron Job"}
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
          <button onClick={() => setMessage({ text: "", type: "" })} className="float-right font-bold" aria-label="Close">&times;</button>
        </div>
      )}

      {/* Create form */}
      {showForm && (
        <form onSubmit={handleCreate} className="bg-dark-800 rounded-lg border border-dark-500 p-5 mb-6 space-y-4">
          {/* Preset Templates */}
          <div>
            <label className="block text-sm font-medium text-dark-100 mb-1">Template</label>
            <select
              value=""
              onChange={(e) => {
                const domain = site?.domain || "DOMAIN";
                const templates: Record<string, { label: string; command: string; schedule: string }> = {
                  "wp-cron": { label: "WordPress Cron", command: `cd /var/www/${domain}/public && php wp-cron.php > /dev/null 2>&1`, schedule: "*/15 * * * *" },
                  "laravel-schedule": { label: "Laravel Scheduler", command: `cd /var/www/${domain} && php artisan schedule:run > /dev/null 2>&1`, schedule: "* * * * *" },
                  "log-cleanup": { label: "Log Cleanup (30 days)", command: `find /var/log/nginx -name '*.log' -mtime +30 -delete`, schedule: "0 3 * * 0" },
                  "disk-check": { label: "Disk Usage Alert", command: `df -h / | awk 'NR==2{if($5+0>90)print "DISK WARNING: "$5}'`, schedule: "0 */6 * * *" },
                };
                const tmpl = templates[e.target.value];
                if (tmpl) {
                  setLabel(tmpl.label);
                  setCommand(tmpl.command);
                  setSchedule(tmpl.schedule);
                }
              }}
              className="w-full px-3 py-2.5 border border-dark-500 rounded-lg focus:ring-2 focus:ring-accent-500 outline-none text-sm bg-dark-800"
            >
              <option value="">Custom (no template)</option>
              <option value="wp-cron">WordPress Cron</option>
              <option value="laravel-schedule">Laravel Scheduler</option>
              <option value="log-cleanup">Log Cleanup (30 days)</option>
              <option value="disk-check">Disk Usage Alert</option>
            </select>
          </div>
          <div>
            <label htmlFor="cron-label" className="block text-sm font-medium text-dark-100 mb-1">Label (optional)</label>
            <input
              id="cron-label"
              type="text"
              value={label}
              onChange={(e) => setLabel(e.target.value)}
              placeholder="Cleanup temp files"
              className="w-full px-3 py-2.5 border border-dark-500 rounded-lg focus:ring-2 focus:ring-accent-500 focus:border-accent-500 outline-none text-sm"
            />
          </div>
          <div>
            <label htmlFor="cron-command" className="block text-sm font-medium text-dark-100 mb-1">Command</label>
            <input
              id="cron-command"
              type="text"
              value={command}
              onChange={(e) => setCommand(e.target.value)}
              required
              placeholder="/usr/bin/php /var/www/example.com/artisan schedule:run"
              className="w-full px-3 py-2.5 border border-dark-500 rounded-lg focus:ring-2 focus:ring-accent-500 focus:border-accent-500 outline-none text-sm font-mono"
            />
            <p className="text-xs text-dark-300 mt-1">Shell command to execute</p>
          </div>
          <div className="grid grid-cols-1 md:grid-cols-2 gap-4">
            <div>
              <label htmlFor="cron-preset" className="block text-sm font-medium text-dark-100 mb-1">Schedule Preset</label>
              <select
                id="cron-preset"
                value={PRESETS.find((p) => p.value === schedule) ? schedule : "custom"}
                onChange={(e) => { if (e.target.value !== "custom") setSchedule(e.target.value); }}
                className="w-full px-3 py-2.5 border border-dark-500 rounded-lg focus:ring-2 focus:ring-accent-500 focus:border-accent-500 outline-none text-sm bg-dark-800"
              >
                {PRESETS.map((p) => (
                  <option key={p.value} value={p.value}>{p.label} ({p.value})</option>
                ))}
                <option value="custom">Custom</option>
              </select>
            </div>
            <div>
              <label htmlFor="cron-schedule" className="block text-sm font-medium text-dark-100 mb-1">Cron Expression</label>
              <input
                id="cron-schedule"
                type="text"
                value={schedule}
                onChange={(e) => setSchedule(e.target.value)}
                required
                placeholder="* * * * *"
                className="w-full px-3 py-2.5 border border-dark-500 rounded-lg focus:ring-2 focus:ring-accent-500 focus:border-accent-500 outline-none text-sm font-mono"
              />
              <p className="text-xs text-dark-300 mt-1">Cron expression, e.g., */5 * * * * for every 5 minutes</p>
            </div>
          </div>
          <div className="flex gap-3">
            <button
              type="submit"
              disabled={submitting}
              className="flex items-center gap-2 px-6 py-2.5 bg-rust-500 text-white rounded-lg text-sm font-medium hover:bg-rust-600 disabled:opacity-50 transition-colors"
            >
              {submitting && <span className="w-4 h-4 border-2 border-white/30 border-t-white rounded-full animate-spin" />}
              {submitting ? "Adding..." : "Create Cron Job"}
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
      )}

      {/* Crons list */}
      <div className="bg-dark-800 rounded-lg border border-dark-500 overflow-hidden">
        {loading ? (
          <div className="flex items-center justify-center py-12">
            <div className="w-6 h-6 border-2 border-dark-600 border-t-rust-500 rounded-full animate-spin" />
          </div>
        ) : !showForm && crons.length === 0 ? (
          <div className="p-12 text-center">
            <svg className="w-16 h-16 mx-auto text-dark-300 mb-4" fill="none" viewBox="0 0 24 24" stroke="currentColor" strokeWidth={1}>
              <path strokeLinecap="round" strokeLinejoin="round" d="M12 6v6h4.5m4.5 0a9 9 0 1 1-18 0 9 9 0 0 1 18 0Z" />
            </svg>
            <p className="text-dark-200 font-medium">No cron jobs yet</p>
            <p className="text-dark-300 text-sm mt-2 max-w-md mx-auto">Schedule recurring tasks with cron expressions. Run scripts, backups, or maintenance commands on any interval.</p>
          </div>
        ) : (
          <div className="divide-y divide-dark-600">
            {crons.map((cron) => (
              <div key={cron.id} className="px-5 py-4 hover:bg-dark-700/30 transition-colors">
                <div className="flex items-start justify-between gap-4">
                  <div className="flex-1 min-w-0">
                    <div className="flex items-center gap-3">
                      {/* Toggle */}
                      <button
                        onClick={() => handleToggle(cron)}
                        className={`relative inline-flex h-5 w-9 shrink-0 cursor-pointer rounded-full border-2 border-transparent transition-colors ${
                          cron.enabled ? "bg-rust-500" : "bg-dark-600"
                        }`}
                        aria-label={cron.enabled ? "Disable cron" : "Enable cron"}
                      >
                        <span className={`pointer-events-none inline-block h-4 w-4 transform rounded-full bg-dark-800 shadow transition-transform ${
                          cron.enabled ? "translate-x-4" : "translate-x-0"
                        }`} />
                      </button>
                      <span className="text-sm font-medium text-dark-50 truncate">
                        {cron.label || cron.command}
                      </span>
                    </div>
                    {cron.label && (
                      <p className="text-xs font-mono text-dark-200 mt-1 ml-12 truncate">{cron.command}</p>
                    )}
                    <div className="flex items-center gap-4 mt-2 ml-12">
                      <span className="inline-flex items-center gap-1 text-xs text-dark-200">
                        <svg className="w-3.5 h-3.5" fill="none" viewBox="0 0 24 24" stroke="currentColor" strokeWidth={1.5}>
                          <path strokeLinecap="round" strokeLinejoin="round" d="M12 6v6h4.5m4.5 0a9 9 0 1 1-18 0 9 9 0 0 1 18 0Z" />
                        </svg>
                        <code className="font-mono">{cron.schedule}</code>
                      </span>
                      {cron.last_run && (
                        <span className="text-xs text-dark-300">
                          Last run: {formatDate(cron.last_run)}
                          {cron.last_status && (
                            <span className={`ml-1 ${cron.last_status === "success" ? "text-rust-400" : "text-danger-500"}`}>
                              ({cron.last_status})
                            </span>
                          )}
                        </span>
                      )}
                    </div>
                  </div>
                  <div className="flex items-center gap-2 shrink-0 flex-wrap">
                    <button
                      onClick={() => handleRun(cron)}
                      disabled={running === cron.id}
                      className="px-3 py-1 bg-warn-500/10 text-warn-400 rounded-md text-xs font-medium hover:bg-warn-400/15 disabled:opacity-50 transition-colors"
                      aria-label="Run now"
                    >
                      {running === cron.id ? "Running..." : "Run Now"}
                    </button>
                    <button
                      onClick={() => handleEdit(cron)}
                      className="px-3 py-1 bg-dark-700 text-dark-100 rounded-md text-xs font-medium hover:bg-dark-600 transition-colors"
                      aria-label="Edit cron"
                    >
                      Edit
                    </button>
                    {deleteTarget === cron.id ? (
                      <div className="flex items-center gap-1">
                        <button onClick={() => handleDelete(cron.id)} className="px-2 py-1 bg-danger-600 text-white rounded-md text-xs">Confirm</button>
                        <button onClick={() => setDeleteTarget(null)} className="px-2 py-1 bg-dark-600 text-dark-200 rounded-md text-xs">Cancel</button>
                      </div>
                    ) : (
                      <button
                        onClick={() => setDeleteTarget(cron.id)}
                        className="px-3 py-1 bg-danger-500/10 text-danger-400 rounded-md text-xs font-medium hover:bg-danger-500/20 transition-colors"
                        aria-label="Delete cron"
                      >
                        Delete
                      </button>
                    )}
                  </div>
                </div>

                {/* Run output */}
                {runOutput && runOutput.id === cron.id && (
                  <div className={`mt-3 ml-12 p-3 rounded-lg text-xs font-mono whitespace-pre-wrap max-h-40 overflow-auto ${
                    runOutput.success ? "bg-dark-900 text-dark-100 border border-dark-500" : "bg-danger-500/10 text-danger-400 border border-danger-500/20"
                  }`}>
                    {runOutput.output || "(no output)"}
                    <button
                      onClick={() => setRunOutput(null)}
                      className="block mt-2 text-dark-300 hover:text-dark-200 text-xs"
                    >
                      Dismiss
                    </button>
                  </div>
                )}

                {/* Inline edit form */}
                {editId === cron.id && (
                  <div className="mt-3 ml-12 p-4 bg-dark-900 rounded-lg border border-dark-500 space-y-3">
                    <div>
                      <label className="block text-xs font-medium text-dark-200 mb-1">Label</label>
                      <input
                        type="text"
                        value={label}
                        onChange={(e) => setLabel(e.target.value)}
                        className="w-full px-3 py-2 border border-dark-500 rounded-lg text-sm"
                      />
                    </div>
                    <div>
                      <label className="block text-xs font-medium text-dark-200 mb-1">Command</label>
                      <input
                        type="text"
                        value={command}
                        onChange={(e) => setCommand(e.target.value)}
                        className="w-full px-3 py-2 border border-dark-500 rounded-lg text-sm font-mono"
                      />
                    </div>
                    <div>
                      <label className="block text-xs font-medium text-dark-200 mb-1">Schedule</label>
                      <input
                        type="text"
                        value={schedule}
                        onChange={(e) => setSchedule(e.target.value)}
                        className="w-full px-3 py-2 border border-dark-500 rounded-lg text-sm font-mono"
                      />
                    </div>
                    <div className="flex gap-2">
                      <button
                        onClick={() => handleEdit(cron)}
                        className="px-4 py-2 bg-rust-500 text-white rounded-lg text-xs font-medium hover:bg-rust-600"
                      >
                        Save
                      </button>
                      <button
                        onClick={() => setEditId(null)}
                        className="px-4 py-2 bg-dark-600 text-dark-100 rounded-lg text-xs font-medium hover:bg-dark-500"
                      >
                        Cancel
                      </button>
                    </div>
                  </div>
                )}
              </div>
            ))}
          </div>
        )}
      </div>
    </div>
  );
}
