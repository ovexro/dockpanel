import { useState, useEffect, FormEvent } from "react";
import { api } from "../api";

interface MaintenanceWindow {
  id: string;
  name: string;
  starts_at: string;
  ends_at: string;
  active: boolean;
}

export default function Maintenance() {
  const [windows, setWindows] = useState<MaintenanceWindow[]>([]);
  const [loading, setLoading] = useState(true);
  const [error, setError] = useState("");
  const [showForm, setShowForm] = useState(false);
  const [submitting, setSubmitting] = useState(false);
  const [message, setMessage] = useState<{ text: string; type: string } | null>(null);
  const [silenced, setSilenced] = useState(false);

  // Form state
  const [formName, setFormName] = useState("");
  const [formStartsAt, setFormStartsAt] = useState("");
  const [formEndsAt, setFormEndsAt] = useState("");

  const fetchWindows = () => {
    api.get<{ windows: MaintenanceWindow[] }>("/monitors/maintenance")
      .then((data) => {
        setWindows(data.windows || []);
        // Check if any window is active (silenced)
        const hasActive = (data.windows || []).some((w) => w.active);
        setSilenced(hasActive);
      })
      .catch((e) => setError(e.message))
      .finally(() => setLoading(false));
  };

  useEffect(() => {
    fetchWindows();
    const id = setInterval(fetchWindows, 30000);
    return () => clearInterval(id);
  }, []);

  const handleCreate = async (e: FormEvent) => {
    e.preventDefault();
    setError("");
    setSubmitting(true);
    try {
      await api.post("/monitors/maintenance", {
        name: formName || "Maintenance",
        starts_at: new Date(formStartsAt).toISOString(),
        ends_at: new Date(formEndsAt).toISOString(),
      });
      setShowForm(false);
      setFormName("");
      setFormStartsAt("");
      setFormEndsAt("");
      setMessage({ text: "Maintenance window created", type: "success" });
      setTimeout(() => setMessage(null), 3000);
      fetchWindows();
    } catch (err) {
      setError(err instanceof Error ? err.message : "Failed to create");
    } finally {
      setSubmitting(false);
    }
  };

  const handleDelete = async (id: string) => {
    try {
      await api.delete(`/monitors/maintenance/${id}`);
      setMessage({ text: "Maintenance window deleted", type: "success" });
      setTimeout(() => setMessage(null), 3000);
      fetchWindows();
    } catch (err) {
      setError(err instanceof Error ? err.message : "Delete failed");
    }
  };

  const handleSilence = async () => {
    try {
      const now = new Date();
      const end = new Date(now.getTime() + 30 * 60 * 1000);
      await api.post("/monitors/maintenance", {
        name: "Alert silence (30 min)",
        starts_at: now.toISOString(),
        ends_at: end.toISOString(),
      });
      setSilenced(true);
      setMessage({ text: "Alerts silenced for 30 minutes", type: "success" });
      setTimeout(() => setMessage(null), 5000);
      fetchWindows();
    } catch (err) {
      setError(err instanceof Error ? err.message : "Failed to silence");
    }
  };

  if (loading) {
    return (
      <div className="animate-fade-up">
        <div className="bg-dark-800 rounded-lg border border-dark-500 p-6 animate-pulse">
          <div className="h-6 bg-dark-700 rounded w-48 mb-4" />
          <div className="h-4 bg-dark-700 rounded w-32" />
        </div>
      </div>
    );
  }

  return (
    <div>
      <div className="flex items-center justify-between mb-4">
        <p className="text-sm text-dark-200 font-mono">
          {windows.length > 0
            ? `${windows.length} maintenance window${windows.length > 1 ? "s" : ""}`
            : "Schedule maintenance to suppress alerts"}
        </p>
        <div className="flex gap-2">
          <button
            onClick={handleSilence}
            className="px-3 py-1.5 bg-warn-500/15 text-warn-400 rounded text-xs font-medium hover:bg-warn-500/25"
          >
            {silenced ? "Silenced (30m)" : "Silence Alerts"}
          </button>
          <button
            onClick={() => setShowForm(!showForm)}
            className="px-4 py-2 bg-rust-500 text-white rounded-lg text-sm font-medium hover:bg-rust-600 transition-colors"
          >
            Schedule Maintenance
          </button>
        </div>
      </div>

      {error && (
        <div className="bg-danger-500/10 text-danger-400 text-sm px-4 py-3 rounded-lg border border-danger-500/20 mb-4">
          {error}
          <button onClick={() => setError("")} className="ml-2 font-medium hover:underline">Dismiss</button>
        </div>
      )}

      {message && (
        <div className={`mb-4 px-4 py-3 rounded-lg text-sm border ${
          message.type === "success"
            ? "bg-rust-500/10 text-rust-400 border-rust-500/20"
            : "bg-danger-500/10 text-danger-400 border-danger-500/20"
        }`}>
          {message.text}
        </div>
      )}

      {/* Create form */}
      {showForm && (
        <form onSubmit={handleCreate} className="bg-dark-800 rounded-lg border border-dark-500 p-5 mb-6">
          <h3 className="text-xs font-medium text-dark-300 uppercase font-mono tracking-widest mb-3">New Maintenance Window</h3>
          <div className="grid grid-cols-1 sm:grid-cols-3 gap-4 mb-4">
            <div>
              <label className="block text-xs font-medium text-dark-200 mb-1">Name</label>
              <input type="text" value={formName} onChange={(e) => setFormName(e.target.value)} placeholder="Server update" className="w-full px-3 py-2 border border-dark-500 rounded-lg text-sm focus:ring-2 focus:ring-accent-500 focus:border-accent-500 outline-none" />
            </div>
            <div>
              <label className="block text-xs font-medium text-dark-200 mb-1">Start</label>
              <input type="datetime-local" value={formStartsAt} onChange={(e) => setFormStartsAt(e.target.value)} required className="w-full px-3 py-2 border border-dark-500 rounded-lg text-sm focus:ring-2 focus:ring-accent-500 focus:border-accent-500 outline-none" />
            </div>
            <div>
              <label className="block text-xs font-medium text-dark-200 mb-1">End</label>
              <input type="datetime-local" value={formEndsAt} onChange={(e) => setFormEndsAt(e.target.value)} required className="w-full px-3 py-2 border border-dark-500 rounded-lg text-sm focus:ring-2 focus:ring-accent-500 focus:border-accent-500 outline-none" />
            </div>
          </div>
          <div className="flex gap-3">
            <button type="submit" disabled={submitting} className="flex items-center gap-2 px-4 py-2 bg-rust-500 text-white rounded-lg text-sm font-medium hover:bg-rust-600 disabled:opacity-50">
              {submitting && <span className="w-4 h-4 border-2 border-white/30 border-t-white rounded-full animate-spin" />}
              {submitting ? "Creating..." : "Create Window"}
            </button>
            <button type="button" onClick={() => setShowForm(false)} className="px-4 py-2 text-sm text-dark-300 border border-dark-600 rounded-lg hover:text-dark-100 hover:border-dark-400 transition-colors">
              Cancel
            </button>
          </div>
        </form>
      )}

      {/* Window list */}
      {windows.length === 0 && !showForm ? (
        <div className="bg-dark-800 rounded-lg border border-dark-500 p-12 text-center">
          <svg className="w-12 h-12 text-dark-300 mx-auto mb-4" fill="none" viewBox="0 0 24 24" stroke="currentColor" strokeWidth={1}>
            <path strokeLinecap="round" strokeLinejoin="round" d="M11.42 15.17 17.25 21A2.652 2.652 0 0 0 21 17.25l-5.877-5.877M11.42 15.17l2.496-3.03c.317-.384.74-.626 1.208-.766M11.42 15.17l-4.655 5.653a2.548 2.548 0 1 1-3.586-3.586l6.837-5.63m5.108-.233c.55-.164 1.163-.188 1.743-.14a4.5 4.5 0 0 0 4.486-6.336l-3.276 3.277a3.004 3.004 0 0 1-2.25-2.25l3.276-3.276a4.5 4.5 0 0 0-6.336 4.486c.091 1.076-.071 2.264-.904 2.95l-.102.085" />
          </svg>
          <p className="text-dark-200 text-sm">No maintenance windows scheduled.</p>
          <p className="text-dark-300 text-xs mt-1">Alerts are suppressed during maintenance windows.</p>
        </div>
      ) : (
        <div className="space-y-3">
          {windows.map((w) => (
            <div key={w.id} className={`bg-dark-800 rounded-lg border p-4 ${w.active ? "border-warn-500/40" : "border-dark-500"}`}>
              <div className="flex items-center justify-between">
                <div>
                  <div className="flex items-center gap-2">
                    <p className="text-sm font-medium text-dark-50">{w.name}</p>
                    {w.active && (
                      <span className="inline-flex items-center gap-1 px-2 py-0.5 rounded-full text-xs font-medium bg-warn-500/15 text-warn-400">
                        <span className="w-1.5 h-1.5 rounded-full bg-warn-500 animate-pulse" />
                        Active
                      </span>
                    )}
                    {!w.active && new Date(w.ends_at) < new Date() && (
                      <span className="px-2 py-0.5 rounded-full text-xs font-medium bg-dark-700 text-dark-300">Ended</span>
                    )}
                    {!w.active && new Date(w.starts_at) > new Date() && (
                      <span className="px-2 py-0.5 rounded-full text-xs font-medium bg-accent-500/15 text-accent-400">Scheduled</span>
                    )}
                  </div>
                  <p className="text-xs text-dark-300 font-mono mt-1">
                    {new Date(w.starts_at).toLocaleString()} — {new Date(w.ends_at).toLocaleString()}
                  </p>
                </div>
                <button
                  onClick={() => handleDelete(w.id)}
                  className="p-1 text-dark-300 hover:text-danger-500 transition-colors"
                  title="Delete"
                >
                  <svg className="w-4 h-4" fill="none" viewBox="0 0 24 24" stroke="currentColor" strokeWidth={2}>
                    <path strokeLinecap="round" strokeLinejoin="round" d="m14.74 9-.346 9m-4.788 0L9.26 9m9.968-3.21c.342.052.682.107 1.022.166m-1.022-.165L18.16 19.673a2.25 2.25 0 0 1-2.244 2.077H8.084a2.25 2.25 0 0 1-2.244-2.077L4.772 5.79m14.456 0a48.108 48.108 0 0 0-3.478-.397m-12 .562c.34-.059.68-.114 1.022-.165m0 0a48.11 48.11 0 0 1 3.478-.397m7.5 0v-.916c0-1.18-.91-2.164-2.09-2.201a51.964 51.964 0 0 0-3.32 0c-1.18.037-2.09 1.022-2.09 2.201v.916m7.5 0a48.667 48.667 0 0 0-7.5 0" />
                  </svg>
                </button>
              </div>
            </div>
          ))}
        </div>
      )}
    </div>
  );
}
