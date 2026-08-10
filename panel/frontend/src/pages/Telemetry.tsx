import { useState, useEffect, Fragment } from "react";
import { api } from "../api";
import { formatDate } from "../utils/format";

interface TelemetryEvent {
  id: string;
  event_type: string;
  category: string;
  message: string;
  context: Record<string, unknown>;
  sent_at: string | null;
  created_at: string;
}

interface TelemetryStats {
  total: number;
  unsent: number;
  last_24h: number;
  by_category: { category: string; count: number }[];
  by_type: { type: string; count: number }[];
}

interface TelemetryConfig {
  telemetry_enabled?: string;
  telemetry_endpoint?: string;
  telemetry_installation_id?: string;
  current_version?: string;
  /// Derived server-side: true only while `update_available_version` names a
  /// release strictly newer than `current_version`. The raw key survives a
  /// panel that has overtaken it (an operator who upgraded from the shell, a
  /// poll that has not run yet), so its presence alone never means "update".
  update_available?: boolean;
  update_available_version?: string;
  update_release_notes?: string;
  update_release_url?: string;
  update_checked_at?: string;
}

// Phase 4 W4: panel self-update.
interface PanelSnapshotRow {
  id: string;
  file_path: string;
  from_version: string;
  to_version: string | null;
  trigger: string;
  operator: string | null;
  size_bytes: number;
  sha256: string;
  rolled_back_at: string | null;
  created_at: string;
}

type UpdateStateName = "idle" | "in_flight" | "succeeded" | "rolled_back" | "failed";

interface UpdateStateView {
  state: UpdateStateName;
  target_version?: string;
  snapshot_id?: string;
  started_at?: string;
  last_log_line?: string | null;
  from_version?: string;
  to_version?: string;
  attempted_version?: string;
  completed_at?: string;
  reason?: string;
  at?: string;
  current_version: string;
  available_version?: string | null;
  channel: string;
  /// Verdict of the last snapshot restore. A restore stops and restarts the
  /// panel, so its outcome cannot come back through the request that began it —
  /// the backend reads it from a file the restore writes on every exit path.
  last_restore?: {
    snapshot_id: string;
    ok: boolean;
    stage: string;
    detail: string;
    finished_at: string;
  } | null;
  /// Verdict of the last `update.sh` run. Same reason as `last_restore`, plus
  /// one the restore does not have: an update that fails BEFORE the api is
  /// stopped never restarts anything, so this file is the ONLY place it is
  /// reported. Without it such a run showed as an in-flight update frozen on
  /// its first log line until the 15-minute window lapsed.
  last_panel_update?: {
    target_version: string;
    ok: boolean;
    stage: string;
    detail: string;
    finished_at: string;
  } | null;
}

interface FleetRunRow {
  id: string;
  target_version: string;
  plan: { server_id: string; name: string; agent_version?: string | null }[];
  progress: {
    server_id: string;
    status: string;
    duration_ms?: number | null;
    error?: string | null;
  }[];
  halt_on_failure: boolean;
  include_panel: boolean;
  started_at: string;
  finished_at?: string | null;
  outcome?: string | null;
}

// Phase 4 W5: fleet configuration drift.
interface DriftServerOpt {
  id: string;
  name: string;
  is_local: boolean;
  status: string;
  last_seen_at: string | null;
  agent_version: string | null;
}
interface FieldDiff {
  field: string;
  reference: unknown;
  current: unknown;
  sensitive: boolean;
}
interface EntityValueDiff {
  identity: string;
  fields: FieldDiff[];
}
interface CoverageMetrics {
  total_sites: number;
  backed_up: number;
  unprotected: number;
  destinations: number;
}
interface ServerEntityDrift {
  server_id: string;
  name: string;
  status: string;
  last_seen_at: string | null;
  in_sync: boolean;
  note?: string;
  field_diffs?: FieldDiff[];
  only_here?: string[];
  only_reference?: string[];
  value_diffs?: EntityValueDiff[];
  coverage?: CoverageMetrics;
  reference_coverage?: CoverageMetrics;
}
interface EntityDrift {
  entity: string;
  label: string;
  mode: string;
  drift_count: number;
  servers: ServerEntityDrift[];
}
interface DriftReport {
  reference: { server_id: string; name: string; is_local: boolean; status: string; last_seen_at: string | null };
  generated_at: string;
  servers_compared: number;
  total_drifted_servers: number;
  entities: EntityDrift[];
  note?: string;
}

function fmtDriftVal(v: unknown): string {
  if (v === null || v === undefined) return "—";
  if (typeof v === "boolean") return v ? "on" : "off";
  if (typeof v === "object") return JSON.stringify(v);
  return String(v);
}

function FieldDiffTable({ fields }: { fields: FieldDiff[] }) {
  return (
    <div className="overflow-x-auto rounded border border-dark-700">
      <table className="w-full text-xs">
        <thead>
          <tr className="text-dark-400 bg-dark-800/50">
            <th className="text-left font-normal px-2 py-1">Field</th>
            <th className="text-left font-normal px-2 py-1">Reference</th>
            <th className="text-left font-normal px-2 py-1">This server</th>
          </tr>
        </thead>
        <tbody>
          {fields.map(f => (
            <tr key={f.field} className="border-t border-dark-700 align-top">
              <td className="px-2 py-1 font-mono text-dark-300">
                {f.field}
                {f.sensitive && (
                  <span className="text-dark-600 ml-1" title="secret — compared by presence only">🔒</span>
                )}
              </td>
              <td className="px-2 py-1 text-dark-400 break-all">{fmtDriftVal(f.reference)}</td>
              <td className="px-2 py-1 text-rust-300 break-all">{fmtDriftVal(f.current)}</td>
            </tr>
          ))}
        </tbody>
      </table>
    </div>
  );
}

function safeHttpUrl(url: string | undefined): string | null {
  if (!url) return null;
  return /^https:\/\/[a-z0-9.-]+\//i.test(url) ? url : null;
}

const EVENT_TYPE_COLORS: Record<string, string> = {
  panic: "bg-danger-500/20 text-danger-400 border-danger-500/30",
  error: "bg-danger-500/10 text-danger-400 border-danger-500/20",
  warning: "bg-warn-500/10 text-warn-400 border-warn-500/20",
  info: "bg-accent-500/10 text-accent-400 border-accent-500/20",
};

const CATEGORY_COLORS: Record<string, string> = {
  agent: "text-dark-300",
  api: "text-dark-300",
  database: "text-dark-300",
  ssl: "text-dark-300",
  docker: "text-dark-300",
  mail: "text-dark-300",
  security: "text-dark-300",
  nginx: "text-dark-300",
  backup: "text-dark-300",
  general: "text-dark-300",
};

export default function Telemetry() {
  const [tab, setTab] = useState<"events" | "updates" | "config">("events");
  const [events, setEvents] = useState<TelemetryEvent[]>([]);
  const [stats, setStats] = useState<TelemetryStats | null>(null);
  const [config, setConfig] = useState<TelemetryConfig>({});
  const [loading, setLoading] = useState(true);
  const [total, setTotal] = useState(0);
  const [page, setPage] = useState(0);
  const [message, setMessage] = useState({ text: "", type: "" });
  const [categoryFilter, setCategoryFilter] = useState("");
  const [typeFilter, setTypeFilter] = useState("");
  const [previewData, setPreviewData] = useState<Record<string, unknown> | null>(null);
  const [showPreview, setShowPreview] = useState(false);
  const [expandedEvent, setExpandedEvent] = useState<string | null>(null);

  // Config form
  const [enabled, setEnabled] = useState(false);
  const [endpoint, setEndpoint] = useState("");
  const [saving, setSaving] = useState(false);
  const [sending, setSending] = useState(false);
  const [checking, setChecking] = useState(false);
  const [clearing, setClearing] = useState(false);

  // Phase 4 W4: panel self-update state.
  const [updateState, setUpdateState] = useState<UpdateStateView | null>(null);
  const [snapshots, setSnapshots] = useState<PanelSnapshotRow[]>([]);
  const [applyConfirm, setApplyConfirm] = useState(false);
  const [applying, setApplying] = useState(false);
  const [showApplyProgress, setShowApplyProgress] = useState(false);
  const [rollbackConfirm, setRollbackConfirm] = useState<string | null>(null);
  const [rollingBack, setRollingBack] = useState(false);
  const [savingChannel, setSavingChannel] = useState(false);
  const [creatingSnapshot, setCreatingSnapshot] = useState(false);
  const [fleetVersion, setFleetVersion] = useState("");
  const [fleetHalt, setFleetHalt] = useState(true);
  const [fleetIncludePanel, setFleetIncludePanel] = useState(false);
  const [fleetSubmitting, setFleetSubmitting] = useState(false);
  const [fleetRuns, setFleetRuns] = useState<FleetRunRow[]>([]);
  const [agentAutoUpdate, setAgentAutoUpdate] = useState(false);
  const [savingAgentAutoUpdate, setSavingAgentAutoUpdate] = useState(false);

  // Phase 4 W5: fleet configuration drift.
  const [driftServers, setDriftServers] = useState<DriftServerOpt[]>([]);
  const [driftReference, setDriftReference] = useState("");
  const [driftReport, setDriftReport] = useState<DriftReport | null>(null);
  const [driftLoading, setDriftLoading] = useState(false);
  const [driftEntityOpen, setDriftEntityOpen] = useState<string | null>(null);

  const limit = 25;

  const loadAgentAutoUpdate = async () => {
    try {
      const data = await api.get<Record<string, string>>("/settings");
      setAgentAutoUpdate(data?.agent_auto_update_enabled === "true");
    } catch {
      // Non-fatal: the card renders as off, and the panel is the enforcer
      // anyway — an agent only updates when this endpoint says so.
    }
  };

  const toggleAgentAutoUpdate = async () => {
    const next = !agentAutoUpdate;
    setSavingAgentAutoUpdate(true);
    try {
      await api.put("/settings", { agent_auto_update_enabled: next ? "true" : "false" });
      setAgentAutoUpdate(next);
      flash(
        next
          ? "Agents will now update themselves to this panel's release."
          : "Agent auto-update disabled.",
        "success"
      );
    } catch (e: unknown) {
      flash(e instanceof Error ? e.message : "Could not save the setting", "error");
    } finally {
      setSavingAgentAutoUpdate(false);
    }
  };

  const loadUpdateState = async () => {
    try {
      const data = await api.get<UpdateStateView>("/update/status");
      setUpdateState(data);
    } catch {
      // empty — telemetry tab still works if status is unreachable
    }
  };
  const loadSnapshots = async () => {
    try {
      const data = await api.get<PanelSnapshotRow[]>("/snapshots");
      setSnapshots(data || []);
    } catch {
      // empty
    }
  };
  const loadFleetRuns = async () => {
    try {
      const data = await api.get<FleetRunRow[]>("/update/fleet");
      setFleetRuns(data || []);
    } catch {
      // empty
    }
  };
  const loadDriftServers = async () => {
    try {
      const data = await api.get<DriftServerOpt[]>("/drift/servers");
      setDriftServers(data || []);
      // Default the reference to the local server (or the first available).
      setDriftReference(prev => prev || (data?.find(s => s.is_local) || data?.[0])?.id || "");
    } catch {
      // empty — the card shows the single-server hint if this never loads
    }
  };
  const runDrift = async () => {
    if (!driftReference) return;
    setDriftLoading(true);
    try {
      const data = await api.get<DriftReport>(`/drift?reference=${encodeURIComponent(driftReference)}`);
      setDriftReport(data);
      setDriftEntityOpen(null);
    } catch (e: unknown) {
      flash(e instanceof Error ? e.message : "Could not compute drift", "error");
    } finally {
      setDriftLoading(false);
    }
  };

  const loadEvents = async () => {
    try {
      let url = `/telemetry/events?limit=${limit}&offset=${page * limit}`;
      if (categoryFilter) url += `&category=${categoryFilter}`;
      if (typeFilter) url += `&event_type=${typeFilter}`;
      const data = await api.get<{ events: TelemetryEvent[]; total: number }>(url);
      setEvents(data.events);
      setTotal(data.total);
    } catch {
      // empty
    }
  };

  const loadStats = async () => {
    try {
      const data = await api.get<TelemetryStats>("/telemetry/stats");
      setStats(data);
    } catch {
      // empty
    }
  };

  const loadConfig = async () => {
    try {
      const data = await api.get<TelemetryConfig>("/telemetry/config");
      setConfig(data);
      setEnabled(data.telemetry_enabled === "true");
      setEndpoint(data.telemetry_endpoint || "");
    } catch {
      // empty
    }
  };

  useEffect(() => {
    Promise.all([
      loadEvents(),
      loadStats(),
      loadConfig(),
      loadUpdateState(),
      loadSnapshots(),
      loadFleetRuns(),
      loadAgentAutoUpdate(),
      loadDriftServers(),
    ]).finally(() => setLoading(false));
  }, []);

  useEffect(() => { loadEvents(); }, [page, categoryFilter, typeFilter]);

  // A fleet run takes minutes and the table was only ever loaded on mount, so a
  // run sat at "running..." until the operator reloaded the page by hand. Poll
  // while at least one run has no outcome yet, and stop as soon as none do.
  const hasRunningFleet = fleetRuns.some(r => !r.outcome);
  useEffect(() => {
    if (!hasRunningFleet) return;
    const t = setInterval(() => { loadFleetRuns(); }, 10000);
    return () => clearInterval(t);
  }, [hasRunningFleet]);

  // Phase 4 W4: poll /update/status while applying so the modal reflects
  // live progress. Stops when the state transitions out of in_flight.
  useEffect(() => {
    if (!showApplyProgress) return;
    const tick = async () => {
      await loadUpdateState();
      await loadSnapshots();
    };
    const interval = setInterval(tick, 2000);
    return () => clearInterval(interval);
  }, [showApplyProgress]);

  useEffect(() => {
    if (showApplyProgress && updateState && updateState.state !== "in_flight") {
      // Update flow reached a terminal state — leave the modal open so
      // the operator can read the outcome, but stop polling.
    }
  }, [updateState, showApplyProgress]);

  const flash = (text: string, type: string) => {
    setMessage({ text, type });
    setTimeout(() => setMessage({ text: "", type: "" }), 4000);
  };

  const saveConfig = async () => {
    setSaving(true);
    try {
      await api.put("/telemetry/config", {
        telemetry_enabled: enabled ? "true" : "false",
        telemetry_endpoint: endpoint,
      });
      flash("Configuration saved", "success");
      loadConfig();
    } catch (e: unknown) {
      flash(e instanceof Error ? e.message : "Failed to save", "error");
    } finally {
      setSaving(false);
    }
  };

  // Phase 4 W4: apply update through the panel itself.
  const applyUpdate = async () => {
    if (!config.update_available || !config.update_available_version) return;
    setApplyConfirm(false);
    setApplying(true);
    setShowApplyProgress(true);
    try {
      const target = `v${config.update_available_version}`;
      await api.post("/update/apply", { target_version: target });
      flash("Update started — services may briefly become unavailable.", "success");
      // First status poll fires immediately (the useEffect interval kicks
      // in once showApplyProgress is true).
      loadUpdateState();
    } catch (e: unknown) {
      flash(e instanceof Error ? e.message : "Apply failed", "error");
      setShowApplyProgress(false);
    } finally {
      setApplying(false);
    }
  };

  const triggerManualCheck = async () => {
    setChecking(true);
    try {
      const resp = await api.post<{ checked_at: string; available_version: string | null }>(
        "/update/manual-check",
        {}
      );
      flash(
        resp.available_version
          ? `Available: v${resp.available_version}`
          : "No update available — you are on the latest version.",
        "success"
      );
      await Promise.all([loadConfig(), loadUpdateState()]);
    } catch (e: unknown) {
      flash(e instanceof Error ? e.message : "Check failed", "error");
    } finally {
      setChecking(false);
    }
  };

  const changeChannel = async (next: string) => {
    setSavingChannel(true);
    try {
      await api.put("/update/channel", { channel: next });
      flash(`Channel set to ${next}.`, "success");
      await loadUpdateState();
    } catch (e: unknown) {
      flash(e instanceof Error ? e.message : "Channel change failed", "error");
    } finally {
      setSavingChannel(false);
    }
  };

  const createManualSnapshot = async () => {
    setCreatingSnapshot(true);
    try {
      await api.post("/snapshots", {});
      flash("Snapshot created.", "success");
      await loadSnapshots();
    } catch (e: unknown) {
      flash(e instanceof Error ? e.message : "Snapshot failed", "error");
    } finally {
      setCreatingSnapshot(false);
    }
  };

  const rollbackToSnapshot = async (snapshotId: string) => {
    setRollbackConfirm(null);
    setRollingBack(true);
    try {
      await api.post("/update/rollback", { snapshot_id: snapshotId });
      flash("Rollback started — services restarting.", "success");
      // The api will die mid-restore; rely on next page load to recover.
    } catch (e: unknown) {
      flash(e instanceof Error ? e.message : "Rollback failed", "error");
    } finally {
      setRollingBack(false);
    }
  };

  const deleteSnapshot = async (snapshotId: string) => {
    try {
      await api.delete(`/snapshots/${snapshotId}`);
      flash("Snapshot deleted.", "success");
      await loadSnapshots();
    } catch (e: unknown) {
      flash(e instanceof Error ? e.message : "Delete failed", "error");
    }
  };

  const applyToFleet = async () => {
    if (!fleetVersion.trim()) {
      flash("Pick a target version first.", "error");
      return;
    }
    setFleetSubmitting(true);
    try {
      const resp = await api.post<{ run_id: string; plan_size: number }>(
        "/update/fleet",
        {
          target_version: fleetVersion,
          halt_on_failure: fleetHalt,
          include_panel: fleetIncludePanel,
        }
      );
      flash(
        `Fleet update started: ${resp.plan_size} server(s) queued (run ${resp.run_id.slice(0, 8)}).`,
        "success"
      );
      await loadFleetRuns();
    } catch (e: unknown) {
      flash(e instanceof Error ? e.message : "Fleet apply failed", "error");
    } finally {
      setFleetSubmitting(false);
    }
  };

  const sendNow = async () => {
    setSending(true);
    try {
      await api.post("/telemetry/send");
      flash("Telemetry events being sent", "success");
      setTimeout(loadEvents, 3000);
    } catch (e: unknown) {
      flash(e instanceof Error ? e.message : "Failed to send", "error");
    } finally {
      setSending(false);
    }
  };

  const previewReport = async () => {
    try {
      const data = await api.get<Record<string, unknown>>("/telemetry/preview");
      setPreviewData(data);
      setShowPreview(true);
    } catch (e: unknown) {
      flash(e instanceof Error ? e.message : "Failed to generate preview", "error");
    }
  };

  const exportReport = async () => {
    try {
      const data = await api.get("/telemetry/export");
      const blob = new Blob([JSON.stringify(data, null, 2)], { type: "application/json" });
      const url = URL.createObjectURL(blob);
      const a = document.createElement("a");
      a.href = url;
      a.download = `dockpanel-telemetry-${new Date().toISOString().slice(0, 10)}.json`;
      a.click();
      URL.revokeObjectURL(url);
      flash("Report exported", "success");
    } catch (e: unknown) {
      flash(e instanceof Error ? e.message : "Failed to export", "error");
    }
  };

  const [pendingClear, setPendingClear] = useState<number | "all" | null>(null);

  const clearEvents = async (days?: number) => {
    setPendingClear(days ?? "all");
  };

  const executeClear = async () => {
    const days = pendingClear === "all" ? undefined : (pendingClear ?? undefined);
    setPendingClear(null);
    setClearing(true);
    try {
      const url = days ? `/telemetry/events?before_days=${days}` : "/telemetry/events";
      const data = await api.delete<{ deleted: number }>(url);
      flash(`${data.deleted} events cleared`, "success");
      loadEvents();
      loadStats();
    } catch (e: unknown) {
      flash(e instanceof Error ? e.message : "Failed to clear", "error");
    } finally {
      setClearing(false);
    }
  };

  // Same endpoint as before, but it now runs the poll inline and answers with
  // what it found, so the flash reports the check instead of announcing one.
  // Reloads the config afterwards rather than guessing at a 5s delay.
  const checkUpdates = async () => {
    setChecking(true);
    try {
      const resp = await api.post<{ message: string; available_version: string | null }>(
        "/telemetry/check-updates"
      );
      flash(resp.message, "success");
      await Promise.all([loadConfig(), loadUpdateState()]);
    } catch (e: unknown) {
      flash(e instanceof Error ? e.message : "Failed to check", "error");
    } finally {
      setChecking(false);
    }
  };

  if (loading) {
    return (
      <div className="flex items-center justify-center h-64">
        <div className="w-6 h-6 border-2 border-dark-600 border-t-rust-500 rounded-full animate-spin" />
      </div>
    );
  }

  const totalPages = Math.ceil(total / limit);

  // Every "is there an update" question on this page answers from the backend's
  // derived flag, never from the mere presence of the stored version — that row
  // outlives the release it names on any panel upgraded from the shell (#98).
  const updateAvailable = Boolean(config.update_available && config.update_available_version);

  // The two update-path verdicts, newest first. Built here rather than inline so
  // the Updates tab can order them against each other; each is still rendered
  // exactly once.
  const updateVerdicts = [
    updateState?.last_panel_update ? {
      at: updateState.last_panel_update.finished_at,
      el: (
        <div key="panel-update" className={`rounded-lg border px-3 py-2 text-xs ${
          updateState.last_panel_update.ok
            ? "bg-rust-500/10 border-rust-500/30 text-rust-400"
            : "bg-danger-500/10 border-danger-500/30 text-danger-400"
        }`}>
          <div className="font-medium">
            {updateState.last_panel_update.ok
              ? `Last update completed${updateState.last_panel_update.target_version ? ` (${updateState.last_panel_update.target_version})` : ""}`
              : `Last update FAILED at stage "${updateState.last_panel_update.stage}"`}
          </div>
          <div className="mt-1 text-dark-300 break-words">{updateState.last_panel_update.detail}</div>
          <div className="mt-1 font-mono text-[10px] text-dark-400">
            {new Date(updateState.last_panel_update.finished_at).toLocaleString()}
          </div>
        </div>
      ),
    } : null,
    updateState?.last_restore ? {
      at: updateState.last_restore.finished_at,
      el: (
        <div key="restore" className={`rounded-lg border px-3 py-2 text-xs ${
          updateState.last_restore.ok
            ? "bg-rust-500/10 border-rust-500/30 text-rust-400"
            : "bg-danger-500/10 border-danger-500/30 text-danger-400"
        }`}>
          <div className="font-medium">
            {updateState.last_restore.ok
              ? "Last rollback completed"
              : `Last rollback FAILED at stage "${updateState.last_restore.stage}"`}
          </div>
          <div className="mt-1 text-dark-300 break-words">{updateState.last_restore.detail}</div>
          {/* Only shown when the restore itself says nothing changed. It
              describes the PRE-commit failure paths, and once a restore
              can also stop after the transaction has applied, printing it
              unconditionally would reassure the operator in exactly the
              cases where the reassurance is false. */}
          {!updateState.last_restore.ok
            && updateState.last_restore.detail?.includes("nothing changed") && (
            <div className="mt-1 text-dark-400">
              The database is applied in a single transaction — a failure before the
              "database" stage completes leaves it exactly as it was.
            </div>
          )}
          {!updateState.last_restore.ok
            && updateState.last_restore.detail?.includes("LEFT STOPPED") && (
            <div className="mt-1 text-dark-400">
              The panel API is stopped deliberately: the database was reverted but the
              binaries were not, and starting the installed build would migrate the
              database forward and undo this rollback. Restore the pre-rollback dump
              named above, or finish putting the previous binaries back, before starting it.
            </div>
          )}
          <div className="mt-1 font-mono text-[10px] text-dark-400">
            {updateState.last_restore.snapshot_id} · {new Date(updateState.last_restore.finished_at).toLocaleString()}
          </div>
        </div>
      ),
    } : null,
  ]
    .filter((v): v is { at: string; el: React.ReactElement } => v !== null)
    .sort((a, b) => new Date(b.at).getTime() - new Date(a.at).getTime());

  return (
    <div className="p-4 sm:p-6 lg:p-8 animate-fade-up">
      <div className="page-header">
        <div>
          <h1 className="page-header-title">Telemetry & Updates</h1>
          <p className="text-xs text-dark-400 mt-0.5">
            Diagnostic reporting and version management{config.current_version ? ` — v${config.current_version}` : ""}
          </p>
        </div>
        <div className="flex items-center gap-2">
          <button onClick={exportReport} className="px-3 py-1.5 bg-dark-800 text-dark-300 hover:bg-dark-700 hover:text-dark-100 border border-dark-600 rounded-lg text-xs transition-colors">
            Export Report
          </button>
          <button onClick={previewReport} className="px-3 py-1.5 bg-dark-800 text-dark-300 hover:bg-dark-700 hover:text-dark-100 border border-dark-600 rounded-lg text-xs transition-colors">
            Preview Report
          </button>
        </div>
      </div>

      {message.text && (
        <div className={`mb-4 px-4 py-2.5 rounded-lg border text-sm ${message.type === "success" ? "bg-rust-500/10 border-rust-500/20 text-rust-400" : "bg-danger-500/10 border-danger-500/20 text-danger-400"}`}>
          {message.text}
        </div>
      )}

      {/* Update banner */}
      {updateAvailable && (
        <div className="mb-4 px-4 py-3 rounded-lg border border-rust-500/30 bg-rust-500/10 flex items-center justify-between">
          <div className="flex items-center gap-3">
            <svg className="w-5 h-5 text-rust-400 flex-shrink-0" fill="none" viewBox="0 0 24 24" stroke="currentColor" strokeWidth={2}><path strokeLinecap="round" strokeLinejoin="round" d="M4 16v1a3 3 0 003 3h10a3 3 0 003-3v-1m-4-4l-4 4m0 0l-4-4m4 4V4" /></svg>
            <div>
              <span className="text-sm font-medium text-rust-300">
                DockPanel v{config.update_available_version} available
              </span>
              <span className="text-xs text-dark-400 ml-2">
                (current: v{config.current_version})
              </span>
            </div>
          </div>
          {safeHttpUrl(config.update_release_url) && (
            <a href={safeHttpUrl(config.update_release_url)!} target="_blank" rel="noopener noreferrer"
              className="px-3 py-1.5 bg-rust-500 hover:bg-rust-600 text-white rounded-lg text-xs font-medium transition-colors">
              View Release
            </a>
          )}
        </div>
      )}

      {/* Stats cards */}
      {stats && (
        <div className="grid grid-cols-2 sm:grid-cols-4 gap-3 mb-6">
          <div className="bg-dark-800 border border-dark-600 rounded-lg p-3">
            <div className="text-xs text-dark-400 mb-1">Total Events</div>
            <div className="text-xl font-mono font-bold text-dark-100">{stats.total}</div>
          </div>
          <div className="bg-dark-800 border border-dark-600 rounded-lg p-3">
            <div className="text-xs text-dark-400 mb-1">Unsent</div>
            <div className="text-xl font-mono font-bold text-warn-400">{stats.unsent}</div>
          </div>
          <div className="bg-dark-800 border border-dark-600 rounded-lg p-3">
            <div className="text-xs text-dark-400 mb-1">Last 24h</div>
            <div className="text-xl font-mono font-bold text-dark-100">{stats.last_24h}</div>
          </div>
          <div className="bg-dark-800 border border-dark-600 rounded-lg p-3">
            <div className="text-xs text-dark-400 mb-1">Status</div>
            <div className={`text-sm font-medium ${enabled ? "text-rust-400" : "text-dark-400"}`}>
              {enabled ? "Sending enabled" : "Local only"}
            </div>
          </div>
        </div>
      )}

      {/* Tabs */}
      <div className="flex gap-1 mb-4 border-b border-dark-600 pb-px">
        {(["events", "updates", "config"] as const).map(t => (
          <button key={t} onClick={() => setTab(t)}
            className={`px-4 py-2 text-xs font-medium rounded-t-lg transition-colors ${tab === t ? "bg-dark-700 text-dark-100 border border-dark-600 border-b-dark-900" : "text-dark-400 hover:text-dark-200"}`}>
            {t === "events" ? "Events" : t === "updates" ? "Updates" : "Configuration"}
          </button>
        ))}
      </div>

      {/* Events tab */}
      {tab === "events" && (
        <div>
          {/* Filters */}
          <div className="flex flex-wrap items-center gap-2 mb-4">
            <select value={categoryFilter} onChange={e => { setCategoryFilter(e.target.value); setPage(0); }}
              className="bg-dark-800 border border-dark-600 text-dark-200 text-xs rounded-lg px-3 py-1.5">
              <option value="">All Categories</option>
              {stats?.by_category.map(c => (
                <option key={c.category} value={c.category}>{c.category} ({c.count})</option>
              ))}
            </select>
            <select value={typeFilter} onChange={e => { setTypeFilter(e.target.value); setPage(0); }}
              className="bg-dark-800 border border-dark-600 text-dark-200 text-xs rounded-lg px-3 py-1.5">
              <option value="">All Types</option>
              {stats?.by_type.map(t => (
                <option key={t.type} value={t.type}>{t.type} ({t.count})</option>
              ))}
            </select>
            <div className="flex-1" />
            <button onClick={() => clearEvents(30)} disabled={clearing}
              className="px-3 py-1.5 text-xs text-dark-400 hover:text-dark-200 border border-dark-600 rounded-lg transition-colors disabled:opacity-50">
              Clear &gt; 30 days
            </button>
            <button onClick={() => clearEvents()} disabled={clearing}
              className="px-3 py-1.5 text-xs text-danger-400 hover:bg-danger-500/10 border border-danger-500/20 rounded-lg transition-colors disabled:opacity-50">
              Clear All
            </button>
          </div>

          {/* Confirm clear bar */}
          {pendingClear !== null && (
            <div className="border border-danger-500/30 bg-danger-500/5 rounded-lg px-4 py-3 mb-4 flex items-center justify-between">
              <span className="text-xs text-danger-400 font-mono">
                {pendingClear === "all" ? "Clear ALL telemetry events?" : `Clear events older than ${pendingClear} days?`}
              </span>
              <div className="flex items-center gap-2 shrink-0 ml-4">
                <button onClick={executeClear} className="px-3 py-1.5 bg-danger-500 text-white text-xs font-bold uppercase tracking-wider hover:bg-danger-600 transition-colors">Confirm</button>
                <button onClick={() => setPendingClear(null)} className="px-3 py-1.5 bg-dark-600 text-dark-200 text-xs font-bold uppercase tracking-wider hover:bg-dark-500 transition-colors">Cancel</button>
              </div>
            </div>
          )}

          {/* Events table */}
          {events.length === 0 ? (
            <div className="bg-dark-800 border border-dark-600 rounded-lg p-8 text-center text-dark-400 text-sm">
              No telemetry events recorded yet. Events are captured automatically when errors or issues occur.
            </div>
          ) : (
            <div className="bg-dark-800 border border-dark-600 rounded-lg overflow-hidden">
              <table className="w-full text-xs">
                <thead>
                  <tr className="border-b border-dark-600 text-dark-400">
                    <th className="text-left px-3 py-2 font-medium">Type</th>
                    <th className="text-left px-3 py-2 font-medium">Category</th>
                    <th className="text-left px-3 py-2 font-medium">Message</th>
                    <th className="text-left px-3 py-2 font-medium hidden sm:table-cell">Status</th>
                    <th className="text-left px-3 py-2 font-medium">Time</th>
                  </tr>
                </thead>
                <tbody>
                  {events.map(ev => (
                    <Fragment key={ev.id}>
                      <tr onClick={() => setExpandedEvent(expandedEvent === ev.id ? null : ev.id)}
                        className="border-b border-dark-700 hover:bg-dark-700 cursor-pointer transition-colors">
                        <td className="px-3 py-2">
                          <span className={`px-1.5 py-0.5 rounded text-[10px] font-medium border ${EVENT_TYPE_COLORS[ev.event_type] || "text-dark-300"}`}>
                            {ev.event_type}
                          </span>
                        </td>
                        <td className={`px-3 py-2 font-mono ${CATEGORY_COLORS[ev.category] || "text-dark-300"}`}>
                          {ev.category}
                        </td>
                        <td className="px-3 py-2 text-dark-200 max-w-xs truncate">{ev.message}</td>
                        <td className="px-3 py-2 hidden sm:table-cell">
                          {ev.sent_at ? (
                            <span className="text-rust-400 text-[10px]">Sent</span>
                          ) : (
                            <span className="text-warn-400 text-[10px]">Pending</span>
                          )}
                        </td>
                        <td className="px-3 py-2 text-dark-400 whitespace-nowrap">{formatDate(ev.created_at)}</td>
                      </tr>
                      {expandedEvent === ev.id && (
                        <tr>
                          <td colSpan={5} className="px-4 py-3 bg-dark-900">
                            <pre className="text-[10px] font-mono text-dark-300 whitespace-pre-wrap overflow-x-auto max-h-48">
                              {JSON.stringify(ev.context, null, 2)}
                            </pre>
                          </td>
                        </tr>
                      )}
                    </Fragment>
                  ))}
                </tbody>
              </table>

              {/* Pagination */}
              {totalPages > 1 && (
                <div className="flex items-center justify-between px-3 py-2 border-t border-dark-600">
                  <span className="text-xs text-dark-400">{total} events total</span>
                  <div className="flex items-center gap-1">
                    <button onClick={() => setPage(p => Math.max(0, p - 1))} disabled={page === 0}
                      className="px-2 py-1 text-xs text-dark-300 hover:text-dark-100 disabled:opacity-30">
                      Prev
                    </button>
                    <span className="text-xs text-dark-400 px-2">{page + 1} / {totalPages}</span>
                    <button onClick={() => setPage(p => Math.min(totalPages - 1, p + 1))} disabled={page >= totalPages - 1}
                      className="px-2 py-1 text-xs text-dark-300 hover:text-dark-100 disabled:opacity-30">
                      Next
                    </button>
                  </div>
                </div>
              )}
            </div>
          )}
        </div>
      )}

      {/* Updates tab */}
      {tab === "updates" && (
        <div className="space-y-4">
          {/* Verdicts of the last update and the last rollback, most recent
              first, ABOVE everything else on the tab.
              They rendered inside the Snapshots card until v2.48.0 — seventh of
              seven, so the one operator who most needs them, the one whose
              update just failed, scrolled past Version Status, Release Notes,
              Channel, Agent Auto-Update, Apply and the snapshot list to reach
              the sentence explaining why the panel is stopped.
              Ordered by finished_at rather than by kind: a rollback usually
              follows a failed update, but not always, and a fixed order would
              put a stale verdict above a fresh one. */}
          {(updateVerdicts.length > 0) && (
            <div className="space-y-3">{updateVerdicts.map(v => v.el)}</div>
          )}

          <div className="bg-dark-800 border border-dark-600 rounded-lg p-4">
            <div className="flex items-center justify-between mb-4">
              <h3 className="text-sm font-medium text-dark-100">Version Status</h3>
              <button onClick={checkUpdates} disabled={checking}
                className="px-3 py-1.5 bg-dark-700 text-dark-300 hover:bg-dark-600 hover:text-dark-100 border border-dark-500 rounded-lg text-xs transition-colors disabled:opacity-50">
                {checking ? "Checking..." : "Check Now"}
              </button>
            </div>

            <div className="grid grid-cols-1 sm:grid-cols-3 gap-4">
              <div>
                <div className="text-xs text-dark-400 mb-1">Current Version</div>
                <div className="text-sm font-mono text-dark-100">v{config.current_version}</div>
              </div>
              <div>
                <div className="text-xs text-dark-400 mb-1">Latest Available</div>
                <div className={`text-sm font-mono ${updateAvailable ? "text-warn-400" : "text-rust-400"}`}>
                  {updateAvailable ? `v${config.update_available_version}` : "Up to date"}
                </div>
              </div>
              <div>
                <div className="text-xs text-dark-400 mb-1">Last Checked</div>
                <div className="text-sm text-dark-300">
                  {config.update_checked_at ? formatDate(config.update_checked_at) : "Never"}
                </div>
              </div>
            </div>
          </div>

          {updateAvailable && config.update_release_notes && (
            <div className="bg-dark-800 border border-dark-600 rounded-lg p-4">
              <div className="flex items-center justify-between mb-3">
                <h3 className="text-sm font-medium text-dark-100">
                  Release Notes — v{config.update_available_version}
                </h3>
                {safeHttpUrl(config.update_release_url) && (
                  <a href={safeHttpUrl(config.update_release_url)!} target="_blank" rel="noopener noreferrer"
                    className="text-xs text-rust-400 hover:text-rust-300 transition-colors">
                    View on GitHub
                  </a>
                )}
              </div>
              <pre className="text-xs font-mono text-dark-300 whitespace-pre-wrap max-h-64 overflow-y-auto bg-dark-900 rounded-lg p-3 border border-dark-700">
                {config.update_release_notes}
              </pre>
            </div>
          )}

          {/* Phase 4 W4: channel selector + apply update from UI */}
          <div className="bg-dark-800 border border-dark-600 rounded-lg p-4">
            <div className="flex items-center justify-between mb-3">
              <h3 className="text-sm font-medium text-dark-100">Update Channel</h3>
              <button onClick={triggerManualCheck} disabled={checking}
                className="px-3 py-1.5 bg-dark-700 text-dark-300 hover:bg-dark-600 hover:text-dark-100 border border-dark-500 rounded-lg text-xs transition-colors disabled:opacity-50">
                {checking ? "Checking..." : "Check Now"}
              </button>
            </div>
            <div className="grid grid-cols-3 gap-2">
              {(["stable", "candidate", "hold"] as const).map(ch => (
                <button key={ch} disabled={savingChannel} onClick={() => changeChannel(ch)}
                  className={`px-3 py-2 rounded-lg text-xs font-medium border transition-colors ${
                    (updateState?.channel || "stable") === ch
                      ? "bg-rust-500/20 border-rust-500/40 text-rust-300"
                      : "bg-dark-700 border-dark-600 text-dark-300 hover:bg-dark-600 hover:text-dark-100"
                  } disabled:opacity-50`}>
                  {ch === "stable" ? "Stable" : ch === "candidate" ? "Candidate (RC)" : "Hold"}
                </button>
              ))}
            </div>
            <div className="mt-2 text-xs text-dark-400">
              {(updateState?.channel || "stable") === "stable" &&
                "GA releases only (recommended for production)."}
              {updateState?.channel === "candidate" &&
                "Includes pre-release builds — may break, snapshot rollback is your safety net."}
              {updateState?.channel === "hold" &&
                "Auto-polling disabled. Use Check Now to manually look for updates."}
            </div>
          </div>

          {/* s233: agent auto-update. Off unless switched on here — the panel
              answers every agent's version query with "nothing to do" until it
              is, because there is no other way to reach a remote agent. */}
          <div className="bg-dark-800 border border-dark-600 rounded-lg p-4">
            <div className="flex items-start justify-between gap-4">
              <div>
                <h3 className="text-sm font-medium text-dark-100">Agent Auto-Update</h3>
                <p className="text-xs text-dark-400 mt-1 max-w-xl">
                  Remote agents update <span className="text-dark-200">themselves</span> to this
                  panel&apos;s release (v{config.current_version || updateState?.current_version || "—"}),
                  unattended, within 6 hours of it changing. Each box verifies the download against
                  the release checksums, confirms the new agent answers <code>/health</code>, and
                  rolls back if it does not. Leave this off to update the fleet only when you choose
                  to, from Fleet Rolling Update below.
                </p>
                {(updateState?.channel || "stable") === "hold" && agentAutoUpdate && (
                  <p className="text-xs text-warn-400 mt-2">
                    Channel is on Hold — nothing will move until you change it.
                  </p>
                )}
              </div>
              <button
                type="button"
                role="switch"
                aria-checked={agentAutoUpdate}
                aria-label="Agent auto-update"
                onClick={toggleAgentAutoUpdate}
                disabled={savingAgentAutoUpdate}
                className={`shrink-0 relative inline-flex h-6 w-11 items-center rounded-full transition-colors disabled:opacity-50 ${
                  agentAutoUpdate ? "bg-rust-500" : "bg-dark-600"
                }`}>
                <span
                  className={`inline-block h-4 w-4 transform rounded-full bg-white transition-transform ${
                    agentAutoUpdate ? "translate-x-6" : "translate-x-1"
                  }`}
                />
              </button>
            </div>
          </div>

          {updateAvailable && (
            <div className="bg-dark-800 border border-dark-600 rounded-lg p-4">
              <h3 className="text-sm font-medium text-dark-100 mb-3">Apply Update</h3>
              <div className="bg-dark-900 rounded-lg p-3 border border-dark-700 mb-3">
                <div className="text-xs text-dark-400 mb-1">Target version</div>
                <div className="text-sm font-mono text-rust-300">v{config.update_available_version}</div>
                <div className="text-xs text-dark-400 mt-3 mb-1">What will happen</div>
                <ol className="text-xs text-dark-300 list-decimal list-inside space-y-0.5">
                  <li>Pre-update snapshot (binaries + DB dump + /etc/dockpanel)</li>
                  <li>Download new binaries from GitHub, verify checksums</li>
                  <li>Swap binaries + restart services (~5s admin UI unavailability)</li>
                  <li>Health check; auto-rollback on failure within 5min</li>
                </ol>
              </div>
              {applyConfirm ? (
                <div className="flex gap-2 items-center">
                  <span className="text-sm text-rust-300 font-medium flex-1">Apply v{config.update_available_version}?</span>
                  <button onClick={() => setApplyConfirm(false)}
                    className="px-3 py-1.5 bg-dark-700 text-dark-300 hover:bg-dark-600 border border-dark-600 rounded-lg text-xs">
                    Cancel
                  </button>
                  <button onClick={applyUpdate} disabled={applying}
                    className="px-3 py-1.5 bg-rust-500 hover:bg-rust-600 text-white rounded-lg text-xs font-medium disabled:opacity-50">
                    {applying ? "Starting..." : "Yes, apply update"}
                  </button>
                </div>
              ) : (
                <button onClick={() => setApplyConfirm(true)}
                  disabled={updateState?.state === "in_flight"}
                  className="px-4 py-2 bg-rust-500 hover:bg-rust-600 text-white rounded-lg text-sm font-medium transition-colors disabled:opacity-50">
                  {updateState?.state === "in_flight" ? "Update in flight..." : "Apply Update"}
                </button>
              )}
            </div>
          )}

          {/* Snapshots panel */}
          <div className="bg-dark-800 border border-dark-600 rounded-lg p-4">
            <div className="flex items-center justify-between mb-3">
              <h3 className="text-sm font-medium text-dark-100">
                Snapshots <span className="text-dark-400 font-normal">({snapshots.length})</span>
              </h3>
              <button onClick={createManualSnapshot} disabled={creatingSnapshot}
                className="px-3 py-1.5 bg-dark-700 text-dark-300 hover:bg-dark-600 hover:text-dark-100 border border-dark-500 rounded-lg text-xs transition-colors disabled:opacity-50">
                {creatingSnapshot ? "Snapshotting..." : "Snapshot Now"}
              </button>
            </div>
            {snapshots.length === 0 ? (
              <div className="text-xs text-dark-400 py-3 text-center">
                No snapshots yet. Pre-update snapshots appear here automatically when you Apply Update.
              </div>
            ) : (
              <div className="space-y-2">
                {snapshots.map(s => {
                  const isRollback = rollbackConfirm === s.id;
                  const isAbandoned = s.to_version === "abandoned";
                  return (
                    <div key={s.id} className="flex items-center gap-3 bg-dark-900 border border-dark-700 rounded-lg px-3 py-2">
                      <div className="flex-1 min-w-0">
                        <div className="text-xs font-mono text-dark-200 truncate">
                          v{s.from_version}
                          {s.to_version && s.to_version !== "abandoned" && s.to_version !== s.from_version && (
                            <span className="text-dark-400"> → v{s.to_version}</span>
                          )}
                          {s.rolled_back_at && (
                            <span className="text-warn-400 ml-2">(rolled back)</span>
                          )}
                          {isAbandoned && (
                            <span className="text-dark-400 ml-2">(abandoned)</span>
                          )}
                        </div>
                        <div className="text-xs text-dark-400 mt-0.5 flex items-center gap-3">
                          <span>{formatDate(s.created_at)}</span>
                          <span>{(s.size_bytes / 1024 / 1024).toFixed(1)} MB</span>
                          {s.trigger.startsWith("pre-update:") && (
                            <span className="font-mono text-dark-400">pre-update</span>
                          )}
                          {s.trigger === "manual" && (
                            <span className="font-mono text-dark-400">manual</span>
                          )}
                        </div>
                      </div>
                      {isRollback ? (
                        <Fragment>
                          <span className="text-xs text-warn-400">
                            Destructive — the database is reverted to this snapshot. Anything
                            created since (including tables added by a newer version) is removed.
                            A copy of the current database is saved first.
                          </span>
                          <button onClick={() => setRollbackConfirm(null)}
                            className="px-2 py-1 bg-dark-700 hover:bg-dark-600 text-dark-300 border border-dark-600 rounded text-xs">
                            Cancel
                          </button>
                          <button onClick={() => rollbackToSnapshot(s.id)} disabled={rollingBack}
                            className="px-2 py-1 bg-danger-500 hover:bg-danger-600 text-white rounded text-xs disabled:opacity-50">
                            {rollingBack ? "..." : "Roll back"}
                          </button>
                        </Fragment>
                      ) : (
                        <Fragment>
                          <button onClick={() => setRollbackConfirm(s.id)}
                            disabled={updateState?.state === "in_flight"}
                            className="px-2 py-1 bg-dark-700 hover:bg-dark-600 text-dark-300 border border-dark-600 rounded text-xs disabled:opacity-50">
                            Roll back
                          </button>
                          <button onClick={() => deleteSnapshot(s.id)}
                            className="px-2 py-1 text-dark-400 hover:text-danger-400 text-xs">
                            ✕
                          </button>
                        </Fragment>
                      )}
                    </div>
                  );
                })}
              </div>
            )}
            <div className="text-xs text-dark-400 mt-3">
              Retained 7 days (min 3 always kept). Stored under /var/backups/dockpanel/snapshots/.
            </div>
          </div>

          {/* Fleet rolling update */}
          <div className="bg-dark-800 border border-dark-600 rounded-lg p-4">
            <h3 className="text-sm font-medium text-dark-100 mb-3">Fleet Rolling Update</h3>
            <div className="text-xs text-dark-400 mb-3">
              Update all reachable remote agents to a target version, oldest first.
              Halts on first failure by default.
            </div>
            <div className="space-y-2 mb-3">
              <input type="text" value={fleetVersion} onChange={e => setFleetVersion(e.target.value)}
                placeholder="vX.Y.Z (e.g. v2.10.0)"
                className="w-full bg-dark-900 border border-dark-600 text-dark-100 text-sm rounded-lg px-3 py-1.5 font-mono" />
              <label className="flex items-center gap-2 text-xs text-dark-300">
                <input type="checkbox" checked={fleetHalt} onChange={e => setFleetHalt(e.target.checked)}
                  className="rounded bg-dark-900 border-dark-600" />
                Halt on first failure
              </label>
              <label className="flex items-center gap-2 text-xs text-dark-300">
                <input type="checkbox" checked={fleetIncludePanel} onChange={e => setFleetIncludePanel(e.target.checked)}
                  className="rounded bg-dark-900 border-dark-600" />
                Include this panel server (runs last)
              </label>
            </div>
            <button onClick={applyToFleet} disabled={fleetSubmitting || !fleetVersion.trim()}
              className="px-4 py-2 bg-rust-500 hover:bg-rust-600 text-white rounded-lg text-sm font-medium transition-colors disabled:opacity-50">
              {fleetSubmitting ? "Starting..." : "Apply to Fleet"}
            </button>

            {fleetRuns.length > 0 && (
              <div className="mt-4">
                <div className="text-xs text-dark-400 mb-2">Recent runs</div>
                <div className="space-y-2">
                  {fleetRuns.slice(0, 5).map(r => {
                    const outcomeColor =
                      r.outcome === "success"
                        ? "text-rust-400"
                        : r.outcome === "partial"
                        ? "text-warn-400"
                        : r.outcome === "halted"
                        ? "text-danger-400"
                        : "text-dark-300";
                    return (
                      <div key={r.id} className="bg-dark-900 border border-dark-700 rounded-lg px-3 py-2 text-xs">
                        <div className="flex items-center justify-between">
                          <span className="font-mono text-dark-200">v{r.target_version.replace(/^v/, "")}</span>
                          <span className={`font-medium ${outcomeColor}`}>
                            {r.outcome || "running..."}
                          </span>
                        </div>
                        <div className="text-dark-400 mt-1">
                          {formatDate(r.started_at)} ·{" "}
                          {Array.isArray(r.progress)
                            ? `${r.progress.filter(p => p.status === "succeeded").length}/${r.progress.length} succeeded`
                            : "—"}
                          {r.include_panel ? " · panel last" : ""}
                        </div>
                        {/* The reason a member failed was recorded but never
                            rendered, so a fleet run that stopped on an
                            actionable error ("update script not found at ...")
                            showed only "0/1 succeeded". */}
                        {Array.isArray(r.progress) &&
                          r.progress
                            .filter(p => p.error)
                            .map(p => {
                              const name =
                                (Array.isArray(r.plan)
                                  ? r.plan.find(pl => pl.server_id === p.server_id)?.name
                                  : null) || p.server_id.slice(0, 8);
                              return (
                                <div
                                  key={p.server_id}
                                  className="mt-1 text-danger-400 break-words"
                                  title={p.error || ""}
                                >
                                  {name}: {p.error}
                                </div>
                              );
                            })}
                        {Array.isArray(r.progress) &&
                          r.progress.some(p => p.status === "skipped") && (
                            <div className="mt-1 text-warn-400">
                              {r.progress.filter(p => p.status === "skipped").length} server(s)
                              skipped — the run halted on the first failure
                            </div>
                          )}
                      </div>
                    );
                  })}
                </div>
              </div>
            )}
          </div>

          {/* Phase 4 W5: fleet configuration drift */}
          <div className="bg-dark-800 border border-dark-600 rounded-lg p-4">
            <h3 className="text-sm font-medium text-dark-100 mb-1">Fleet Configuration Drift</h3>
            <div className="text-xs text-dark-400 mb-3">
              Compare each server's operational posture — alert rules, per-site config, cron jobs and
              backup coverage — against a reference server. Read-only; computed on demand from the panel.
            </div>

            {driftServers.length < 2 ? (
              <div className="text-xs text-dark-400 py-3 text-center">
                Only one server registered. Add fleet members (Servers page) to detect drift.
              </div>
            ) : (
              <Fragment>
                <div className="flex flex-wrap items-center gap-2 mb-3">
                  <label className="text-xs text-dark-400">Reference</label>
                  <select value={driftReference} onChange={e => setDriftReference(e.target.value)}
                    className="bg-dark-900 border border-dark-600 text-dark-100 text-xs rounded-lg px-2 py-1.5">
                    {driftServers.map(s => (
                      <option key={s.id} value={s.id}>
                        {s.name}{s.is_local ? " (this panel)" : ""}
                      </option>
                    ))}
                  </select>
                  <button onClick={runDrift} disabled={driftLoading || !driftReference}
                    className="px-3 py-1.5 bg-rust-500 hover:bg-rust-600 text-white rounded-lg text-xs font-medium transition-colors disabled:opacity-50">
                    {driftLoading ? "Comparing..." : "Compare fleet"}
                  </button>
                </div>

                {driftReport && (
                  <div className="space-y-2">
                    <div className="text-xs">
                      {driftReport.note ? (
                        <span className="text-dark-400">{driftReport.note}</span>
                      ) : driftReport.total_drifted_servers === 0 ? (
                        <span className="text-rust-400">
                          All {driftReport.servers_compared} server(s) match {driftReport.reference.name}.
                        </span>
                      ) : (
                        <span className="text-warn-400">
                          {driftReport.total_drifted_servers} of {driftReport.servers_compared} server(s)
                          {" "}diverge from {driftReport.reference.name}.
                        </span>
                      )}
                    </div>

                    {driftReport.entities.map(ent => {
                      const open = driftEntityOpen === ent.entity;
                      return (
                        <div key={ent.entity} className="bg-dark-900 border border-dark-700 rounded-lg">
                          <button type="button" onClick={() => setDriftEntityOpen(open ? null : ent.entity)}
                            className="w-full flex items-center justify-between px-3 py-2 text-left">
                            <span className="text-xs font-medium text-dark-200">
                              <span className="text-dark-400 mr-1">{open ? "▾" : "▸"}</span>{ent.label}
                            </span>
                            <span className={`text-xs font-medium ${ent.drift_count > 0 ? "text-warn-400" : "text-rust-400"}`}>
                              {ent.drift_count > 0 ? `${ent.drift_count} drifted` : "in sync"}
                            </span>
                          </button>
                          {open && (
                            <div className="px-3 pb-3 space-y-3 border-t border-dark-700 pt-2">
                              {ent.servers.length === 0 && (
                                <div className="text-xs text-dark-400">No other servers to compare.</div>
                              )}
                              {ent.servers.map(sv => (
                                <div key={sv.server_id} className="text-xs">
                                  <div className="flex items-center gap-2 mb-1">
                                    <span className="font-medium text-dark-200">{sv.name}</span>
                                    <span className={sv.in_sync ? "text-rust-400" : "text-warn-400"}>
                                      {sv.in_sync ? "in sync" : "drift"}
                                    </span>
                                    {sv.status && sv.status !== "online" && (
                                      <span className="text-dark-400">({sv.status})</span>
                                    )}
                                  </div>
                                  {sv.note && <div className="text-dark-400 mb-1">{sv.note}</div>}

                                  {sv.field_diffs && sv.field_diffs.length > 0 && (
                                    <FieldDiffTable fields={sv.field_diffs} />
                                  )}

                                  {sv.only_reference && sv.only_reference.length > 0 && (
                                    <div className="mb-1">
                                      <span className="text-dark-400">Missing here: </span>
                                      {sv.only_reference.map(id => (
                                        <span key={id} className="inline-block bg-warn-500/10 text-warn-500 rounded px-1.5 py-0.5 mr-1 mb-1 font-mono break-all">
                                          {id}
                                        </span>
                                      ))}
                                    </div>
                                  )}
                                  {sv.only_here && sv.only_here.length > 0 && (
                                    <div className="mb-1">
                                      <span className="text-dark-400">Only here: </span>
                                      {sv.only_here.map(id => (
                                        <span key={id} className="inline-block bg-accent-500/10 text-accent-500 rounded px-1.5 py-0.5 mr-1 mb-1 font-mono break-all">
                                          {id}
                                        </span>
                                      ))}
                                    </div>
                                  )}
                                  {sv.value_diffs && sv.value_diffs.map(vd => (
                                    <div key={vd.identity} className="mb-2">
                                      <div className="text-dark-300 font-mono mb-0.5 break-all">{vd.identity}</div>
                                      <FieldDiffTable fields={vd.fields} />
                                    </div>
                                  ))}

                                  {sv.coverage && (
                                    <div className="flex flex-wrap gap-3 text-dark-300">
                                      <span>{sv.coverage.backed_up}/{sv.coverage.total_sites} sites backed up</span>
                                      {sv.coverage.unprotected > 0 && (
                                        <span className="text-rust-300">{sv.coverage.unprotected} unprotected</span>
                                      )}
                                      <span>{sv.coverage.destinations} destination(s)</span>
                                      {sv.reference_coverage && (
                                        <span className="text-dark-400">
                                          reference: {sv.reference_coverage.backed_up}/{sv.reference_coverage.total_sites}
                                        </span>
                                      )}
                                    </div>
                                  )}
                                </div>
                              ))}
                            </div>
                          )}
                        </div>
                      );
                    })}
                  </div>
                )}
              </Fragment>
            )}
          </div>
        </div>
      )}

      {/* Apply progress modal */}
      {showApplyProgress && updateState && (
        <div className="fixed inset-0 bg-black/60 flex items-center justify-center z-50 p-4">
          <div className="bg-dark-800 border border-dark-600 rounded-lg p-6 max-w-md w-full">
            <h3 className="text-base font-medium text-dark-100 mb-3">
              {updateState.state === "in_flight" && "Update in progress"}
              {updateState.state === "succeeded" && "Update complete"}
              {updateState.state === "rolled_back" && "Update rolled back"}
              {updateState.state === "failed" && "Update failed"}
              {updateState.state === "idle" && "Update finished"}
            </h3>
            {updateState.state === "in_flight" && (
              <Fragment>
                <div className="flex items-center gap-3 mb-3">
                  <div className="w-3 h-3 rounded-full bg-rust-500 animate-pulse" />
                  <span className="text-sm text-dark-300">
                    Targeting v{updateState.target_version}
                  </span>
                </div>
                {updateState.last_log_line && (
                  <pre className="text-xs font-mono text-dark-400 bg-dark-900 border border-dark-700 rounded p-2 whitespace-pre-wrap break-all">
                    {updateState.last_log_line}
                  </pre>
                )}
                <div className="text-xs text-dark-400 mt-3">
                  Services may briefly be unavailable. The UI may also disconnect while binaries swap; refresh once it returns.
                </div>
              </Fragment>
            )}
            {updateState.state === "succeeded" && (
              <div className="text-sm text-rust-400 mb-3">
                v{updateState.from_version} → v{updateState.to_version}
              </div>
            )}
            {updateState.state === "rolled_back" && (
              <div className="text-sm text-warn-500 mb-3">
                Health check failed; rolled back to v{updateState.attempted_version
                  ? config.current_version
                  : "previous"}
                .
              </div>
            )}
            {updateState.state === "failed" && (
              <div className="text-sm text-danger-300 mb-3">
                {updateState.reason || "Unknown failure"}
              </div>
            )}
            <div className="flex justify-end">
              <button onClick={() => setShowApplyProgress(false)}
                className="px-3 py-1.5 bg-dark-700 text-dark-300 hover:bg-dark-600 hover:text-dark-100 border border-dark-600 rounded-lg text-xs">
                Close
              </button>
            </div>
          </div>
        </div>
      )}

      {/* Config tab */}
      {tab === "config" && (
        <div className="space-y-4">
          <div className="bg-dark-800 border border-dark-600 rounded-lg p-4">
            <h3 className="text-sm font-medium text-dark-100 mb-4">Telemetry Configuration</h3>

            <div className="space-y-4">
              <div className="flex items-center justify-between">
                <div>
                  <div className="text-sm text-dark-200">Enable Remote Telemetry</div>
                  <div className="text-xs text-dark-400 mt-0.5">
                    Send diagnostic events to a remote endpoint for analysis
                  </div>
                </div>
                <button onClick={() => setEnabled(!enabled)}
                  className={`relative w-10 h-5 rounded-full transition-colors ${enabled ? "bg-rust-500" : "bg-dark-600"}`}>
                  <span className={`absolute top-0.5 left-0.5 w-4 h-4 bg-white rounded-full transition-transform ${enabled ? "translate-x-5" : ""}`} />
                </button>
              </div>

              <div>
                <label className="text-xs text-dark-400 mb-1 block">Endpoint URL (HTTPS required)</label>
                <input type="url" value={endpoint} onChange={e => setEndpoint(e.target.value)}
                  placeholder="https://telemetry.example.com/collect"
                  className="w-full bg-dark-900 border border-dark-600 rounded-lg px-3 py-2 text-sm text-dark-200 placeholder:text-dark-500 focus:outline-none focus:border-rust-500" />
              </div>

              <div className="flex items-center gap-2">
                <button onClick={saveConfig} disabled={saving}
                  className="px-4 py-2 bg-rust-500 hover:bg-rust-600 text-white rounded-lg text-xs font-medium transition-colors disabled:opacity-50">
                  {saving ? "Saving..." : "Save Configuration"}
                </button>
                {enabled && endpoint && (
                  <button onClick={sendNow} disabled={sending}
                    className="px-4 py-2 bg-dark-700 text-dark-200 hover:bg-dark-600 border border-dark-500 rounded-lg text-xs font-medium transition-colors disabled:opacity-50">
                    {sending ? "Sending..." : "Send Now"}
                  </button>
                )}
              </div>
            </div>
          </div>

          <div className="bg-dark-800 border border-dark-600 rounded-lg p-4">
            <h3 className="text-sm font-medium text-dark-100 mb-3">Privacy</h3>
            <div className="space-y-2 text-xs text-dark-300">
              <p>Telemetry is <strong className="text-dark-100">completely opt-in</strong>. When disabled, all events are stored locally only.</p>
              <p>When enabled, the following is collected:</p>
              <ul className="list-disc list-inside space-y-1 ml-2">
                <li>Error messages and stack context (no file paths or user data)</li>
                <li>Service health status (running/stopped)</li>
                <li>System specs (OS, RAM, CPU count — no IP addresses or hostnames)</li>
                <li>DockPanel version</li>
              </ul>
              <p>All personal information (IPs, emails, domains, usernames, tokens) is <strong className="text-dark-100">automatically stripped</strong> before sending.</p>
              <p>Use the <strong className="text-dark-100">Preview Report</strong> button to see exactly what would be sent.</p>
            </div>
            {config.telemetry_installation_id && (
              <div className="mt-3 pt-3 border-t border-dark-700">
                <span className="text-xs text-dark-400">Installation ID: </span>
                <span className="text-xs font-mono text-dark-300">{config.telemetry_installation_id}</span>
              </div>
            )}
          </div>

          {/* Category breakdown */}
          {stats && stats.by_category.length > 0 && (
            <div className="bg-dark-800 border border-dark-600 rounded-lg p-4">
              <h3 className="text-sm font-medium text-dark-100 mb-3">Events by Category</h3>
              <div className="space-y-2">
                {stats.by_category.map(c => (
                  <div key={c.category} className="flex items-center justify-between">
                    <span className={`text-xs font-mono ${CATEGORY_COLORS[c.category] || "text-dark-300"}`}>{c.category}</span>
                    <div className="flex items-center gap-2">
                      <div className="w-24 h-1.5 bg-dark-700 rounded-full overflow-hidden">
                        <div className="h-full bg-rust-500 rounded-full" style={{ width: `${Math.min(100, (c.count / stats.total) * 100)}%` }} />
                      </div>
                      <span className="text-xs text-dark-400 w-8 text-right">{c.count}</span>
                    </div>
                  </div>
                ))}
              </div>
            </div>
          )}
        </div>
      )}

      {/* Preview modal */}
      {showPreview && previewData && (
        <div className="fixed inset-0 bg-black/60 flex items-center justify-center z-50 p-4" onClick={() => setShowPreview(false)}>
          <div className="bg-dark-800 border border-dark-600 rounded-xl max-w-3xl w-full max-h-[80vh] overflow-hidden flex flex-col" onClick={e => e.stopPropagation()}>
            <div className="flex items-center justify-between px-4 py-3 border-b border-dark-600">
              <h3 className="text-sm font-medium text-dark-100">Telemetry Report Preview</h3>
              <button onClick={() => setShowPreview(false)} className="text-dark-400 hover:text-dark-200">
                <svg className="w-4 h-4" fill="none" viewBox="0 0 24 24" stroke="currentColor" strokeWidth={2}><path strokeLinecap="round" strokeLinejoin="round" d="M6 18L18 6M6 6l12 12" /></svg>
              </button>
            </div>
            <div className="overflow-y-auto p-4">
              <pre className="text-[10px] font-mono text-dark-300 whitespace-pre-wrap">
                {JSON.stringify(previewData, null, 2)}
              </pre>
            </div>
            <div className="px-4 py-2 border-t border-dark-600 text-xs text-dark-400">
              This is exactly what would be sent. All PII has been stripped.
            </div>
          </div>
        </div>
      )}
    </div>
  );
}
