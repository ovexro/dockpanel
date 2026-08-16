import { useAuth } from "../context/AuthContext";
import { Navigate, useLocation, useNavigate } from "react-router-dom";
import { useState, useEffect } from "react";
import { api } from "../api";
import { formatSize, formatDate, timeAgo } from "../utils/format";
import { PrereqList, type PrereqResult } from "../components/Prerequisite";
import { FieldHelp } from "../components/FieldHelp";

interface ServerSla {
  server_id: string | null;
  server_name: string;
  verified: number;
  total: number;
  lag_p95_hours: number | null;
}

interface BackupHealth {
  total_site_backups: number;
  total_db_backups: number;
  total_volume_backups: number;
  total_storage_bytes: number;
  last_24h_success: number;
  last_24h_failed: number;
  policies_active: number;
  policies_total: number;
  verifications_passed: number;
  verifications_failed: number;
  oldest_unverified_days: number | null;
  // last_backup/days_since are null when the resource has never been backed up —
  // the worst case this list reports, and the one it used to be unable to encode.
  stale_backups: { resource_type: string; resource_name: string; last_backup: string | null; days_since: number | null }[];
  sla_window: number;
  sla_verified: number;
  sla_failed: number;
  sla_pending: number;
  verify_lag_p50_hours: number | null;
  verify_lag_p95_hours: number | null;
  per_server_sla: ServerSla[];
  // True when the SLA figures could not be computed at all. Distinguishes
  // "nothing to measure" from "not measured" — they used to render identically.
  sla_unavailable: boolean;
  drills_passed_30d: number;
  drills_failed_30d: number;
}

interface Drill {
  id: string;
  backup_type: string;
  backup_id: string;
  status: string;
  http_status: number | null;
  body_excerpt: string | null;
  error_message: string | null;
  duration_ms: number | null;
  started_at: string | null;
  completed_at: string | null;
  created_at: string;
}

interface DrillsResponse {
  items: Drill[];
  total: number;
}

interface BackupPolicy {
  id: string;
  name: string;
  backup_sites: boolean;
  backup_databases: boolean;
  backup_volumes: boolean;
  schedule: string;
  destination_id: string | null;
  retention_count: number;
  encrypt: boolean;
  verify_after_backup: boolean;
  enabled: boolean;
  drill_enabled: boolean;
  drill_schedule: string;
  last_drill_at: string | null;
  last_run: string | null;
  last_status: string | null;
  created_at: string;
}

interface DatabaseBackup {
  id: string;
  database_id: string;
  filename: string;
  size_bytes: number;
  db_type: string;
  db_name: string;
  encrypted: boolean;
  uploaded: boolean;
  created_at: string;
}

interface VolumeBackup {
  id: string;
  container_name: string;
  volume_name: string;
  filename: string;
  size_bytes: number;
  encrypted: boolean;
  created_at: string;
}

interface Verification {
  id: string;
  backup_type: string;
  backup_id: string;
  status: string;
  checks_run: number;
  checks_passed: number;
  error_message: string | null;
  duration_ms: number | null;
  created_at: string;
}

interface Destination {
  id: string;
  name: string;
  dtype: string;
  // Secret-bearing fields (secret_key, password) arrive masked as "********".
  // Sending them back unchanged is how an edit keeps the stored credential: the
  // API merges a masked value with the one already encrypted at rest.
  config: Record<string, string | number | undefined>;
}

/**
 * `POST /backup-destinations/{id}/test`, in either of its two shapes.
 *
 * Server-owned destination: `{ ok, success, message, server }` — one host was
 * asked and `server` names it (null only if its row could not be read back).
 * Unclaimed destination: `{ ok, shared, reachable, failed, not_asked, warning }`
 * — every online member was asked, `ok` is FALSE on a partial even though the
 * status is 200, and `warning` is the API's own sentence naming who missed.
 * Nothing reached it at all is a 502, which `api.ts` throws.
 */
interface DestTestResult {
  ok: boolean;
  /** Single-host shape: the host that ran the test. */
  server?: string | null;
  message?: string;
  /** Fleet shape: present (possibly empty) only when every member was asked. */
  shared?: boolean;
  reachable?: string[];
  failed?: { server: string; error: string }[];
  not_asked?: { server: string; status: string }[];
  warning?: string;
}

// One editable destination. Both transports are held in the same object so that
// switching Type in the form does not discard what was typed under the other.
interface DestForm {
  name: string;
  dtype: string;
  bucket: string;
  region: string;
  endpoint: string;
  access_key: string;
  secret_key: string;
  path_prefix: string;
  host: string;
  port: string;
  username: string;
  password: string;
  remote_path: string;
}

const EMPTY_DEST_FORM: DestForm = {
  name: "", dtype: "s3",
  bucket: "", region: "us-east-1", endpoint: "", access_key: "", secret_key: "", path_prefix: "backups",
  host: "", port: "22", username: "", password: "", remote_path: "/backups",
};

interface ServerRow {
  id: string;
  name: string;
  is_local: boolean;
}

interface UnifiedBackupRow {
  id: string;
  kind: "site" | "database" | "volume";
  resource_id: string | null;
  resource_name: string;
  filename: string;
  size_bytes: number;
  created_at: string;
  server_id: string | null;
  server_name: string;
  server_is_local: boolean;
  encrypted: boolean;
  uploaded: boolean;
  extra_type: string | null;
}

interface UnifiedBackupsResponse {
  items: UnifiedBackupRow[];
  total: number;
}

interface PolicyForm {
  name: string;
  schedule: string;
  backup_sites: boolean;
  backup_databases: boolean;
  backup_volumes: boolean;
  destination_id: string;
  retention_count: number;
  encrypt: boolean;
  verify_after_backup: boolean;
  // Only editable on an existing policy — a new one is always created enabled.
  // Nothing could flip it before, though the table has always rendered the word
  // "Paused" for the state that produced it.
  enabled: boolean;
  drill_enabled: boolean;
  drill_schedule: string;
}

const EMPTY_POLICY_FORM: PolicyForm = {
  name: "", schedule: "0 2 * * *", backup_sites: true, backup_databases: true,
  backup_volumes: false, destination_id: "", retention_count: 7,
  encrypt: false, verify_after_backup: false, enabled: true,
  drill_enabled: false, drill_schedule: "0 4 * * 0",
};

interface Database {
  id: string;
  name: string;
  engine: string;
}

type Tab = "overview" | "all" | "policies" | "databases" | "volumes" | "verifications" | "drills" | "destinations";

const TAB_KEYS: Tab[] = [
  "overview", "all", "policies", "databases", "volumes", "verifications", "drills", "destinations",
];

// Which tab `?tab=` asks for, or Overview. Deep-linkable because other pages send
// operators here to add a destination, and a link that lands on the wrong tab is
// how GH #93 started: text that named a place the reader then had to hunt for.
function tabFromSearch(search: string): Tab {
  const requested = new URLSearchParams(search).get("tab");
  return TAB_KEYS.find(k => k === requested) ?? "overview";
}

const ALL_PAGE_SIZE = 50;
const DRILLS_PAGE_SIZE = 50;

export default function BackupOrchestrator() {
  const { user } = useAuth();
  if (!user || user.role !== "admin") return <Navigate to="/" replace />;
  const location = useLocation();
  const navigate = useNavigate();
  const [tab, setTabState] = useState<Tab>(() => tabFromSearch(location.search));
  // Keep `?tab=` in step with the tab bar so the address is shareable and Back
  // steps between tabs instead of leaving the page.
  const setTab = (next: Tab) => {
    setTabState(next);
    navigate(next === "overview" ? "/backup-orchestrator" : `/backup-orchestrator?tab=${next}`, { replace: true });
  };
  const [health, setHealth] = useState<BackupHealth | null>(null);
  const [policies, setPolicies] = useState<BackupPolicy[]>([]);
  const [dbBackups, setDbBackups] = useState<DatabaseBackup[]>([]);
  const [volBackups, setVolBackups] = useState<VolumeBackup[]>([]);
  const [verifications, setVerifications] = useState<Verification[]>([]);
  const [drillsData, setDrillsData] = useState<DrillsResponse>({ items: [], total: 0 });
  const [drillsOffset, setDrillsOffset] = useState(0);
  const [drillsLoading, setDrillsLoading] = useState(false);
  const [destinations, setDestinations] = useState<Destination[]>([]);
  const [databases, setDatabases] = useState<Database[]>([]);
  const [servers, setServers] = useState<ServerRow[]>([]);
  const [prereqs, setPrereqs] = useState<PrereqResult[]>([]);
  const [unified, setUnified] = useState<UnifiedBackupsResponse>({ items: [], total: 0 });
  const [unifiedFilterServer, setUnifiedFilterServer] = useState<string>("");
  const [unifiedFilterKind, setUnifiedFilterKind] = useState<"" | "site" | "database" | "volume">("");
  const [unifiedOffset, setUnifiedOffset] = useState(0);
  const [unifiedLoading, setUnifiedLoading] = useState(false);
  const [loading, setLoading] = useState(true);
  const [message, setMessage] = useState({ text: "", type: "" });
  const [showDestForm, setShowDestForm] = useState(false);
  const [editingDestId, setEditingDestId] = useState<string | null>(null);
  const [destForm, setDestForm] = useState<DestForm>(EMPTY_DEST_FORM);
  const [savingDest, setSavingDest] = useState(false);
  const [testingDestId, setTestingDestId] = useState<string | null>(null);
  const [pendingDeleteDestId, setPendingDeleteDestId] = useState<string | null>(null);
  const [showPolicyForm, setShowPolicyForm] = useState(false);
  const [policyForm, setPolicyForm] = useState<PolicyForm>(EMPTY_POLICY_FORM);
  // Non-null while an existing policy is loaded into the form for editing.
  const [editingPolicyId, setEditingPolicyId] = useState<string | null>(null);
  const [protectAllBusy, setProtectAllBusy] = useState(false);
  // Two-step confirms for the destructive backup controls. Separate ids because a
  // restore and a delete on the same row must not share one armed state.
  const [pendingRestoreId, setPendingRestoreId] = useState<string | null>(null);
  const [pendingDeleteBackupId, setPendingDeleteBackupId] = useState<string | null>(null);

  useEffect(() => { loadAll(); }, []);

  const loadAll = async () => {
    setLoading(true);
    try {
      const [h, p, db, vol, ver, drl, dest, dbs, srv, pre] = await Promise.all([
        api.get<BackupHealth>("/backup-orchestrator/health").catch(() => null),
        api.get<BackupPolicy[]>("/backup-orchestrator/policies").catch(() => []),
        api.get<DatabaseBackup[]>("/backup-orchestrator/db-backups").catch(() => []),
        api.get<VolumeBackup[]>("/backup-orchestrator/volume-backups").catch(() => []),
        api.get<Verification[]>("/backup-orchestrator/verifications").catch(() => []),
        api.get<DrillsResponse>(`/backup-orchestrator/drills?limit=${DRILLS_PAGE_SIZE}&offset=0`).catch(() => ({ items: [], total: 0 })),
        api.get<Destination[]>("/backup-destinations").catch(() => []),
        api.get<Database[]>("/databases").catch(() => []),
        api.get<ServerRow[]>("/servers").catch(() => []),
        api.get<{ checks: PrereqResult[] }>("/backup-orchestrator/preflight").catch(() => ({ checks: [] })),
      ]);
      setHealth(h);
      setPolicies(p);
      setDbBackups(db);
      setVolBackups(vol);
      setVerifications(ver);
      setDrillsData(drl);
      setDrillsOffset(0);
      setDestinations(dest);
      setDatabases(dbs);
      setServers(srv);
      setPrereqs(pre.checks ?? []);
    } catch (e) {
      setMessage({ text: e instanceof Error ? e.message : "Failed to load", type: "error" });
    } finally {
      setLoading(false);
    }
  };

  const loadUnified = async (offset = 0) => {
    setUnifiedLoading(true);
    try {
      const qs = new URLSearchParams({
        limit: String(ALL_PAGE_SIZE),
        offset: String(offset),
      });
      if (unifiedFilterServer) qs.set("server_id", unifiedFilterServer);
      if (unifiedFilterKind) qs.set("kind", unifiedFilterKind);
      const res = await api.get<UnifiedBackupsResponse>(`/backup-orchestrator/all?${qs.toString()}`);
      setUnified(res);
      setUnifiedOffset(offset);
    } catch (e) {
      setMessage({ text: e instanceof Error ? e.message : "Failed to load unified view", type: "error" });
    } finally {
      setUnifiedLoading(false);
    }
  };

  useEffect(() => {
    if (tab === "all") loadUnified(0);
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [tab, unifiedFilterServer, unifiedFilterKind]);

  const loadDrills = async (offset = 0) => {
    setDrillsLoading(true);
    try {
      const res = await api.get<DrillsResponse>(
        `/backup-orchestrator/drills?limit=${DRILLS_PAGE_SIZE}&offset=${offset}`
      );
      setDrillsData(res);
      setDrillsOffset(offset);
    } catch (e) {
      setMessage({ text: e instanceof Error ? e.message : "Failed to load drills", type: "error" });
    } finally {
      setDrillsLoading(false);
    }
  };

  // Create OR save, depending on whether a policy is loaded into the form. The
  // PUT endpoint existed and was correct from the start; nothing in the browser
  // had ever called it, so schedule, scope, retention, destination, encryption,
  // verification and enabled were all create-once — while the preflight warning
  // told the operator to "edit the policy and pick a destination".
  //
  // Every field is sent on save, not a partial body: `destination_id: null` is
  // how the operator detaches a destination and returns to local-only.
  const savePolicy = async () => {
    try {
      const body = { ...policyForm, destination_id: policyForm.destination_id || null };
      if (editingPolicyId) {
        await api.put(`/backup-orchestrator/policies/${editingPolicyId}`, body);
        setMessage({ text: `Policy "${policyForm.name}" saved`, type: "success" });
      } else {
        await api.post("/backup-orchestrator/policies", body);
        setMessage({ text: "Policy created", type: "success" });
      }
      setEditingPolicyId(null);
      setPolicyForm(EMPTY_POLICY_FORM);
      setShowPolicyForm(false);
      loadAll();
    } catch (e) {
      setMessage({ text: e instanceof Error ? e.message : "Failed", type: "error" });
    }
  };

  const openEditPolicy = (p: BackupPolicy) => {
    setEditingPolicyId(p.id);
    setShowPolicyForm(false);
    setPolicyForm({
      name: p.name,
      schedule: p.schedule,
      backup_sites: p.backup_sites,
      backup_databases: p.backup_databases,
      backup_volumes: p.backup_volumes,
      destination_id: p.destination_id ?? "",
      retention_count: p.retention_count,
      encrypt: p.encrypt,
      verify_after_backup: p.verify_after_backup,
      enabled: p.enabled,
      drill_enabled: p.drill_enabled,
      drill_schedule: p.drill_schedule || "0 4 * * 0",
    });
  };

  const cancelEditPolicy = () => {
    setEditingPolicyId(null);
    setPolicyForm(EMPTY_POLICY_FORM);
    setShowPolicyForm(false);
  };

  const protectAll = async () => {
    setProtectAllBusy(true);
    try {
      await api.post("/backup-orchestrator/policies/protect-all");
      setMessage({ text: "Protect Everything policy created — nightly at 2 AM, keeping 7", type: "success" });
      loadAll();
    } catch (e) {
      // The handler answers 409 when the preset already exists; its message
      // names the policy, which is more useful than a generic failure.
      setMessage({ text: e instanceof Error ? e.message : "Failed", type: "error" });
    } finally {
      setProtectAllBusy(false);
    }
  };

  const deletePolicy = async (id: string) => {
    try {
      await api.delete(`/backup-orchestrator/policies/${id}`);
      setPolicies(policies.filter(p => p.id !== id));
      setMessage({ text: "Policy deleted", type: "success" });
    } catch (e) {
      setMessage({ text: e instanceof Error ? e.message : "Failed", type: "error" });
    }
  };

  const openNewDest = () => {
    setEditingDestId(null);
    setDestForm(EMPTY_DEST_FORM);
    setShowDestForm(true);
  };

  const openEditDest = (d: Destination) => {
    const c = d.config ?? {};
    const str = (k: string, fallback = "") => (c[k] === undefined || c[k] === null ? fallback : String(c[k]));
    setEditingDestId(d.id);
    setDestForm({
      ...EMPTY_DEST_FORM,
      name: d.name,
      dtype: d.dtype,
      bucket: str("bucket"),
      region: str("region", "us-east-1"),
      endpoint: str("endpoint"),
      access_key: str("access_key"),
      // Left masked on purpose. Cleared and retyped to rotate; sent back as
      // "********" to keep the credential already stored.
      secret_key: str("secret_key"),
      path_prefix: str("path_prefix", "backups"),
      host: str("host"),
      port: str("port", "22"),
      username: str("username"),
      password: str("password"),
      remote_path: str("remote_path", "/backups"),
    });
    setShowDestForm(true);
  };

  const saveDest = async () => {
    if (!destForm.name.trim()) {
      setMessage({ text: "Name is required", type: "error" });
      return;
    }
    const port = parseInt(destForm.port, 10);
    if (destForm.dtype === "sftp" && (!Number.isFinite(port) || port < 1 || port > 65535)) {
      setMessage({ text: "Port must be between 1 and 65535", type: "error" });
      return;
    }
    setSavingDest(true);
    try {
      const config = destForm.dtype === "s3"
        ? {
            bucket: destForm.bucket.trim(),
            region: destForm.region.trim(),
            endpoint: destForm.endpoint.trim(),
            access_key: destForm.access_key.trim(),
            secret_key: destForm.secret_key,
            path_prefix: destForm.path_prefix.trim(),
          }
        : {
            host: destForm.host.trim(),
            port,
            username: destForm.username.trim(),
            password: destForm.password,
            remote_path: destForm.remote_path.trim(),
          };
      if (editingDestId) {
        // dtype is immutable: the stored config is shaped by it, and the API's
        // update takes name + config only.
        await api.put(`/backup-destinations/${editingDestId}`, { name: destForm.name.trim(), config });
        setMessage({ text: `Destination "${destForm.name.trim()}" updated`, type: "success" });
      } else {
        await api.post("/backup-destinations", { name: destForm.name.trim(), dtype: destForm.dtype, config });
        setMessage({ text: `Destination "${destForm.name.trim()}" created`, type: "success" });
      }
      setShowDestForm(false);
      setEditingDestId(null);
      setDestForm(EMPTY_DEST_FORM);
      setDestinations(await api.get<Destination[]>("/backup-destinations"));
    } catch (e) {
      setMessage({ text: e instanceof Error ? e.message : "Failed to save destination", type: "error" });
    } finally {
      setSavingDest(false);
    }
  };

  const testDest = async (d: Destination) => {
    setTestingDestId(d.id);
    setMessage({ text: "", type: "" });
    try {
      // Two shapes, because two different questions were asked. A destination
      // that belongs to one server is tested from that server and the answer
      // names it. An unclaimed destination is tested from EVERY online member,
      // and then a 2xx does not mean "it works" — it means at least one host
      // reached it. The hosts that could not, and the ones that were never
      // asked, arrive in `warning`; a 502 (thrown) means nothing reached it.
      const res = await api.post<DestTestResult>(`/backup-destinations/${d.id}/test`);
      if (res?.warning) {
        const reached = res.reachable ?? [];
        setMessage({
          text: `"${d.name}" — ${res.warning}${reached.length > 0 ? ` Succeeded from: ${reached.join(", ")}.` : ""}`,
          type: "warning",
        });
      } else if (res?.reachable) {
        setMessage({
          text: `Connection to "${d.name}" succeeded from all ${res.reachable.length} server${res.reachable.length === 1 ? "" : "s"} (${res.reachable.join(", ")})`,
          type: "success",
        });
      } else {
        // Single-server destination. Say WHICH host answered — an unattributed
        // "connection succeeded" is how one box's result came to stand for the
        // whole fleet's.
        setMessage({
          text: `Connection to "${d.name}" succeeded from ${res?.server ?? "its server"}`,
          type: "success",
        });
      }
    } catch (e) {
      setMessage({ text: e instanceof Error ? e.message : "Connection failed", type: "error" });
    } finally {
      setTestingDestId(null);
    }
  };

  const deleteDest = async (d: Destination) => {
    setPendingDeleteDestId(null);
    try {
      // The API answers with a warning when schedules referenced it — those rows
      // are not deleted, their destination is nulled, so they silently become
      // local-only. Surfacing that is the whole point of showing it.
      const res = await api.delete<{ ok: boolean; warning?: string }>(`/backup-destinations/${d.id}`);
      setDestinations(destinations.filter(x => x.id !== d.id));
      setMessage({
        text: res?.warning ? `Destination "${d.name}" deleted — ${res.warning}` : `Destination "${d.name}" deleted`,
        type: res?.warning ? "error" : "success",
      });
      if (res?.warning) loadAll();
    } catch (e) {
      setMessage({ text: e instanceof Error ? e.message : "Failed to delete destination", type: "error" });
    }
  };

  const createDbBackup = async (databaseId: string) => {
    try {
      setMessage({ text: "Creating database backup...", type: "success" });
      await api.post("/backup-orchestrator/db-backup", { database_id: databaseId });
      setMessage({ text: "Database backup created", type: "success" });
      loadAll();
    } catch (e) {
      setMessage({ text: e instanceof Error ? e.message : "Failed", type: "error" });
    }
  };

  const triggerVerify = async (backupType: string, backupId: string) => {
    try {
      await api.post("/backup-orchestrator/verify", { backup_type: backupType, backup_id: backupId });
      setMessage({ text: "Verification started", type: "success" });
      setTimeout(loadAll, 5000);
    } catch (e) {
      setMessage({ text: e instanceof Error ? e.message : "Failed", type: "error" });
    }
  };

  // Restore and delete: the three doors this page listed backups behind and never
  // opened. `restore_db_backup`, `restore_volume_backup` and `delete_db_backup` have
  // been complete, ownership-scoped and server-pinned for months with zero callers —
  // so the page could prove a backup restorable with a drill and still leave the
  // operator no way to actually restore it. A drill restores into a scratch
  // container; these overwrite the live database or volume, which is why both sit
  // behind the same in-row confirm the drill uses and say what they overwrite.
  const restoreDbBackup = async (b: DatabaseBackup) => {
    setPendingRestoreId(null);
    try {
      setMessage({ text: `Restoring ${b.db_name} from ${b.filename}...`, type: "success" });
      await api.post(`/backup-orchestrator/db-backups/${b.id}/restore`);
      setMessage({ text: `Database ${b.db_name} restored from ${b.filename}`, type: "success" });
      loadAll();
    } catch (e) {
      setMessage({ text: e instanceof Error ? e.message : "Restore failed", type: "error" });
    }
  };

  const restoreVolumeBackup = async (b: VolumeBackup) => {
    setPendingRestoreId(null);
    try {
      setMessage({ text: `Restoring volume ${b.volume_name} from ${b.filename}...`, type: "success" });
      await api.post(`/backup-orchestrator/volume-backups/${b.id}/restore`);
      setMessage({ text: `Volume ${b.volume_name} restored from ${b.filename}`, type: "success" });
      loadAll();
    } catch (e) {
      setMessage({ text: e instanceof Error ? e.message : "Restore failed", type: "error" });
    }
  };

  // Delete exists for database backups only — there is no volume-backup delete
  // endpoint, so the volume table deliberately offers no delete button rather than
  // a control that would 404.
  const deleteDbBackup = async (b: DatabaseBackup) => {
    setPendingDeleteBackupId(null);
    try {
      await api.delete(`/backup-orchestrator/db-backups/${b.id}`);
      setDbBackups(prev => prev.filter(x => x.id !== b.id));
      setMessage({ text: `Backup ${b.filename} deleted`, type: "success" });
      loadAll();
    } catch (e) {
      setMessage({ text: e instanceof Error ? e.message : "Delete failed", type: "error" });
    }
  };

  const triggerDrill = async (backupId: string, backupType: "site" | "database" | "volume") => {
    try {
      await api.post("/backup-orchestrator/drill", { backup_type: backupType, backup_id: backupId });
      const msg =
        backupType === "site"
          ? "Site drill started — restoring into a scratch container, this can take 30s"
        : backupType === "database"
          ? "Database drill started — booting scratch engine + restoring + counting rows, this can take 60s"
          : "Volume drill started — restoring into a scratch volume + read-testing through a probe container, this can take 60s";
      setMessage({ text: msg, type: "success" });
      setTimeout(loadAll, 8000);
      setTimeout(loadAll, 30000);
      setTimeout(loadAll, 75000);
    } catch (e) {
      setMessage({ text: e instanceof Error ? e.message : "Failed", type: "error" });
    }
  };

  const tabs: { key: Tab; label: string }[] = [
    { key: "overview", label: "Overview" },
    { key: "all", label: "All Backups" },
    { key: "policies", label: "Policies" },
    { key: "databases", label: "DB Backups" },
    { key: "volumes", label: "Volume Backups" },
    { key: "verifications", label: "Verifications" },
    { key: "drills", label: "Drills" },
    { key: "destinations", label: "Destinations" },
  ];

  if (loading) {
    return <div className="p-8 text-center text-dark-300 font-mono">Loading backup orchestrator...</div>;
  }

  return (
    <div className="p-6 lg:p-8">
      {/* Header */}
      <div className="flex flex-col sm:flex-row sm:items-center justify-between gap-3 mb-6 pb-4 border-b border-dark-600">
        <div>
          <h1 className="text-sm font-medium text-dark-300 uppercase font-mono tracking-widest">Backup Orchestrator</h1>
          <p className="text-sm text-dark-200 mt-1 font-mono">Database, volume & site backups with verification</p>
        </div>
      </div>

      {/* Message */}
      {message.text && (
        <div className={`mb-4 px-4 py-3 rounded-lg text-sm border font-mono ${
          message.type === "success" ? "bg-rust-500/10 text-rust-400 border-rust-500/20"
          // A fleet operation that partly landed is neither a success nor a
          // failure, and the page already speaks warn/danger (see `tone` below).
          : message.type === "warning" ? "bg-warn-500/10 text-warn-400 border-warn-500/20"
          : "bg-danger-500/10 text-danger-400 border-danger-500/20"
        }`} role="alert">{message.text}</div>
      )}

      {/* Tabs */}
      <div className="flex gap-1 mb-6 border-b border-dark-600 overflow-x-auto">
        {tabs.map(t => (
          <button key={t.key} onClick={() => setTab(t.key)}
            className={`px-4 py-2 text-xs font-mono uppercase tracking-widest transition-colors whitespace-nowrap ${
              tab === t.key ? "text-rust-400 border-b-2 border-rust-400" : "text-dark-300 hover:text-dark-100"
            }`}>{t.label}</button>
        ))}
      </div>

      {/* Tab Content */}
      {/* Prerequisites lead the Overview: a 98%-verified SLA card above a fleet
          whose backups never leave the box is a reassuring number about the
          wrong thing. */}
      {tab === "overview" && <PrereqList checks={prereqs} className="mb-4" />}
      {tab === "overview" && health && <OverviewTab health={health} />}
      {tab === "all" && (
        <AllBackupsTab
          data={unified}
          servers={servers}
          loading={unifiedLoading}
          offset={unifiedOffset}
          pageSize={ALL_PAGE_SIZE}
          filterServer={unifiedFilterServer}
          filterKind={unifiedFilterKind}
          setFilterServer={setUnifiedFilterServer}
          setFilterKind={setUnifiedFilterKind}
          onPage={loadUnified}
          onDrill={triggerDrill}
        />
      )}
      {tab === "policies" && (
        <PoliciesTab
          policies={policies} destinations={destinations}
          showForm={showPolicyForm} setShowForm={setShowPolicyForm}
          form={policyForm} setForm={setPolicyForm}
          onCreate={savePolicy} onDelete={deletePolicy}
          editingId={editingPolicyId} onEdit={openEditPolicy} onCancelEdit={cancelEditPolicy}
          onProtectAll={protectAll} protectAllBusy={protectAllBusy}
        />
      )}
      {tab === "databases" && (
        <DatabasesTab backups={dbBackups} databases={databases}
          onCreateBackup={createDbBackup} onVerify={triggerVerify}
          onRestore={restoreDbBackup} onDelete={deleteDbBackup}
          pendingRestoreId={pendingRestoreId} setPendingRestoreId={setPendingRestoreId}
          pendingDeleteId={pendingDeleteBackupId} setPendingDeleteId={setPendingDeleteBackupId} />
      )}
      {tab === "volumes" && (
        <VolumesTab backups={volBackups} onVerify={triggerVerify}
          onRestore={restoreVolumeBackup}
          pendingRestoreId={pendingRestoreId} setPendingRestoreId={setPendingRestoreId} />
      )}
      {tab === "verifications" && <VerificationsTab verifications={verifications} />}
      {tab === "drills" && (
        <DrillsTab
          data={drillsData}
          loading={drillsLoading}
          offset={drillsOffset}
          pageSize={DRILLS_PAGE_SIZE}
          onPage={loadDrills}
          onRefresh={() => loadDrills(drillsOffset)}
        />
      )}
      {tab === "destinations" && (
        <DestinationsTab
          destinations={destinations}
          showForm={showDestForm}
          editingId={editingDestId}
          form={destForm}
          setForm={setDestForm}
          saving={savingDest}
          testingId={testingDestId}
          pendingDeleteId={pendingDeleteDestId}
          setPendingDeleteId={setPendingDeleteDestId}
          onNew={openNewDest}
          onEdit={openEditDest}
          onCancel={() => { setShowDestForm(false); setEditingDestId(null); }}
          onSave={saveDest}
          onTest={testDest}
          onDelete={deleteDest}
        />
      )}
    </div>
  );
}

// ── Overview Tab ──────────────────────────────────────────────────────────

function OverviewTab({ health }: { health: BackupHealth }) {
  const cards = [
    { label: "Site Backups", value: health.total_site_backups, color: "text-rust-400" },
    { label: "DB Backups", value: health.total_db_backups, color: "text-accent-400" },
    { label: "Volume Backups", value: health.total_volume_backups, color: "text-warn-400" },
    { label: "Total Storage", value: formatSize(health.total_storage_bytes), color: "text-dark-50" },
    { label: "24h Success", value: health.last_24h_success, color: "text-rust-400" },
    { label: "24h Failed", value: health.last_24h_failed, color: health.last_24h_failed > 0 ? "text-danger-400" : "text-dark-200" },
    { label: "Active Policies", value: `${health.policies_active}/${health.policies_total}`, color: "text-dark-50" },
    { label: "Verifications", value: `${health.verifications_passed} passed / ${health.verifications_failed} failed`, color: "text-dark-50" },
  ];

  return (
    <div className="space-y-6">
      <SlaCard health={health} />

      {/* Stats Grid */}
      <div className="grid grid-cols-2 md:grid-cols-4 gap-3">
        {cards.map(c => (
          <div key={c.label} className="bg-dark-800 rounded-lg border border-dark-500 p-4">
            <p className="text-xs text-dark-300 uppercase font-mono tracking-widest mb-1">{c.label}</p>
            <p className={`text-lg font-mono font-medium ${c.color}`}>{c.value}</p>
          </div>
        ))}
      </div>

      {/* Stale Backups Warning */}
      {health.stale_backups.length > 0 && (
        <div className="bg-dark-800 rounded-lg border border-warn-500/30 overflow-hidden">
          <div className="px-5 py-3 border-b border-dark-600 bg-warn-500/5">
            <h3 className="text-xs font-medium text-warn-400 uppercase font-mono tracking-widest">Stale Backups Warning</h3>
          </div>
          <div className="divide-y divide-dark-600">
            {health.stale_backups.map((s, i) => (
              <div key={i} className="px-5 py-3 flex items-center justify-between">
                <div>
                  <span className="text-sm text-dark-50 font-mono">{s.resource_name}</span>
                  <span className="text-xs text-dark-300 ml-2">({s.resource_type})</span>
                </div>
                <span className="text-xs text-warn-400 font-mono">
                  {s.days_since === null ? "never backed up" : `${s.days_since}d since last backup`}
                </span>
              </div>
            ))}
          </div>
        </div>
      )}
    </div>
  );
}

function formatLagHours(h: number | null): string {
  if (h === null || h === undefined || !isFinite(h)) return "—";
  if (h < 1) return `${Math.round(h * 60)}m`;
  if (h < 48) return `${h.toFixed(1)}h`;
  return `${(h / 24).toFixed(1)}d`;
}

function slaTone(verified: number, total: number): { num: string; ring: string; band: string } {
  if (total === 0) return { num: "text-dark-200", ring: "border-dark-500", band: "bg-dark-700" };
  const pct = (verified / total) * 100;
  if (pct >= 95) return { num: "text-rust-400", ring: "border-rust-500/40", band: "bg-rust-500/5" };
  if (pct >= 80) return { num: "text-warn-400", ring: "border-warn-500/40", band: "bg-warn-500/5" };
  return { num: "text-danger-400", ring: "border-danger-500/40", band: "bg-danger-500/5" };
}

function SlaCard({ health }: { health: BackupHealth }) {
  const total = health.sla_verified + health.sla_failed + health.sla_pending;
  const pct = total > 0 ? Math.round((health.sla_verified / total) * 100) : 0;
  const tone = slaTone(health.sla_verified, total);
  const showServers = health.per_server_sla.length > 1;

  return (
    <div className={`bg-dark-800 rounded-lg border ${tone.ring} overflow-hidden`}>
      <div className={`px-5 py-3 border-b border-dark-600 ${tone.band} flex items-center justify-between`}>
        <h3 className="text-xs font-medium text-dark-100 uppercase font-mono tracking-widest">Restore Confidence</h3>
        <span className="text-[10px] text-dark-300 font-mono">last {health.sla_window} backups</span>
      </div>

      {health.sla_unavailable ? (
        <div className="px-5 py-8 text-center">
          <p className="text-sm text-warn-400 font-mono">Restore confidence could not be measured</p>
          <p className="text-xs text-dark-300 font-mono mt-1">
            The verification query failed — see the dockpanel-api service log. This is not a report that your backups are unverified.
          </p>
        </div>
      ) : total === 0 ? (
        <div className="px-5 py-8 text-center">
          <p className="text-sm text-dark-200 font-mono">No recent backups to verify yet</p>
          <p className="text-xs text-dark-300 font-mono mt-1">Run a backup or enable verify-after-backup on a policy</p>
        </div>
      ) : (
        <div className="px-5 py-5 grid grid-cols-1 md:grid-cols-5 gap-4 items-center">
          <div className="md:col-span-2 flex items-baseline gap-3">
            <span className={`text-5xl font-mono font-medium ${tone.num}`}>{pct}%</span>
            <div className="flex flex-col">
              <span className="text-sm text-dark-50 font-mono">{health.sla_verified} of {total} verified</span>
              <span className="text-xs text-dark-300 font-mono">
                {health.sla_failed > 0 && <span className="text-danger-400">{health.sla_failed} failed</span>}
                {health.sla_failed > 0 && health.sla_pending > 0 && " · "}
                {health.sla_pending > 0 && <span className="text-warn-400">{health.sla_pending} pending</span>}
                {health.sla_failed === 0 && health.sla_pending === 0 && "all clear"}
              </span>
            </div>
          </div>

          <SlaStat label="p50 lag" value={formatLagHours(health.verify_lag_p50_hours)} />
          <SlaStat label="p95 lag" value={formatLagHours(health.verify_lag_p95_hours)} />
          <SlaStat
            label="oldest unverified"
            value={health.oldest_unverified_days === null ? "—" : `${health.oldest_unverified_days}d`}
            tone={health.oldest_unverified_days !== null && health.oldest_unverified_days > 7 ? "warn" : undefined}
          />
        </div>
      )}

      {(health.drills_passed_30d > 0 || health.drills_failed_30d > 0) && total > 0 && (
        <div className="px-5 py-2 border-t border-dark-600 text-[10px] font-mono flex items-center justify-between">
          <span className="text-dark-300 uppercase tracking-widest">End-to-end drills (30d)</span>
          <span className="text-dark-100">
            <span className="text-rust-400">{health.drills_passed_30d} passed</span>
            {health.drills_failed_30d > 0 && (
              <>
                <span className="text-dark-400 mx-1.5">·</span>
                <span className="text-danger-400">{health.drills_failed_30d} failed</span>
              </>
            )}
          </span>
        </div>
      )}

      {showServers && (
        <div className="border-t border-dark-600">
          <div className="px-5 py-2 text-[10px] text-dark-300 uppercase font-mono tracking-widest">Per server (last 30d)</div>
          <div className="divide-y divide-dark-600">
            {health.per_server_sla.map((s, i) => {
              const sTone = slaTone(s.verified, s.total);
              const sPct = s.total > 0 ? Math.round((s.verified / s.total) * 100) : 0;
              return (
                <div key={s.server_id ?? `idx-${i}`} className="px-5 py-2 flex items-center justify-between text-xs font-mono">
                  <span className="text-dark-50">{s.server_name}</span>
                  <div className="flex items-center gap-4">
                    <span className="text-dark-300">p95 {formatLagHours(s.lag_p95_hours)}</span>
                    <span className="text-dark-300">{s.verified}/{s.total}</span>
                    <span className={`${sTone.num} w-12 text-right`}>{sPct}%</span>
                  </div>
                </div>
              );
            })}
          </div>
        </div>
      )}
    </div>
  );
}

function SlaStat({ label, value, tone }: { label: string; value: string; tone?: "warn" | "danger" }) {
  const color = tone === "danger" ? "text-danger-400" : tone === "warn" ? "text-warn-400" : "text-dark-50";
  return (
    <div className="flex flex-col">
      <span className="text-[10px] text-dark-300 uppercase font-mono tracking-widest">{label}</span>
      <span className={`text-lg font-mono font-medium ${color}`}>{value}</span>
    </div>
  );
}

// ── All Backups Tab (fleet-wide unified view) ─────────────────────────────

function AllBackupsTab({
  data, servers, loading, offset, pageSize,
  filterServer, filterKind, setFilterServer, setFilterKind, onPage, onDrill,
}: {
  data: UnifiedBackupsResponse;
  servers: ServerRow[];
  loading: boolean;
  offset: number;
  pageSize: number;
  filterServer: string;
  filterKind: "" | "site" | "database" | "volume";
  setFilterServer: (s: string) => void;
  setFilterKind: (k: "" | "site" | "database" | "volume") => void;
  onPage: (offset: number) => void;
  onDrill: (backupId: string, backupType: "site" | "database" | "volume") => void;
}) {
  const [pendingDrillId, setPendingDrillId] = useState<string | null>(null);
  const hasNext = offset + data.items.length < data.total;
  const hasPrev = offset > 0;

  const drillCostHint = (kind: UnifiedBackupRow["kind"]): string => {
    if (kind === "site") return "";
    if (kind === "database") return "boots a 256 MB scratch DB engine, ~60s";
    return "boots a 128 MB scratch container + temp volume, ~60s";
  };

  const handleDrillClick = (row: UnifiedBackupRow) => {
    if (row.kind === "site") {
      onDrill(row.id, row.kind);
      return;
    }
    if (pendingDrillId === row.id) {
      onDrill(row.id, row.kind);
      setPendingDrillId(null);
    } else {
      setPendingDrillId(row.id);
    }
  };

  const kindBadge = (kind: UnifiedBackupRow["kind"]) => {
    const style = kind === "site"
      ? "bg-rust-500/10 text-rust-400 border-rust-500/20"
      : kind === "database"
        ? "bg-accent-500/10 text-accent-400 border-accent-500/20"
        : "bg-warn-500/10 text-warn-400 border-warn-500/20";
    return (
      <span className={`px-2 py-0.5 text-[10px] font-mono uppercase tracking-wider border rounded ${style}`}>
        {kind}
      </span>
    );
  };

  return (
    <div className="space-y-4">
      {/* Filters */}
      <div className="bg-dark-800 rounded-lg border border-dark-500 p-4 flex flex-wrap gap-3 items-end">
        <div className="flex flex-col">
          <label className="text-[10px] text-dark-300 uppercase font-mono tracking-widest mb-1">Server</label>
          <select
            value={filterServer}
            onChange={(e) => setFilterServer(e.target.value)}
            className="bg-dark-700 border border-dark-500 text-dark-50 text-sm font-mono rounded px-2 py-1.5 min-w-[180px]"
          >
            <option value="">All servers ({servers.length})</option>
            {servers.map(s => (
              <option key={s.id} value={s.id}>{s.name}{s.is_local ? " (local)" : ""}</option>
            ))}
          </select>
        </div>
        <div className="flex flex-col">
          <label className="text-[10px] text-dark-300 uppercase font-mono tracking-widest mb-1">Kind</label>
          <select
            value={filterKind}
            onChange={(e) => setFilterKind(e.target.value as "" | "site" | "database" | "volume")}
            className="bg-dark-700 border border-dark-500 text-dark-50 text-sm font-mono rounded px-2 py-1.5 min-w-[140px]"
          >
            <option value="">All kinds</option>
            <option value="site">Site</option>
            <option value="database">Database</option>
            <option value="volume">Volume</option>
          </select>
        </div>
        <div className="ml-auto text-xs text-dark-300 font-mono">
          {loading ? "Loading…" : `${data.total.toLocaleString()} total`}
        </div>
      </div>

      {/* Table */}
      <div className="bg-dark-800 rounded-lg border border-dark-500 overflow-hidden">
        {data.items.length === 0 && !loading ? (
          <p className="text-sm text-dark-300 text-center py-8 font-mono">
            No backups match the current filters.
          </p>
        ) : (
          <div className="overflow-x-auto">
            <table className="w-full text-sm">
              <thead>
                <tr className="bg-dark-700 text-[10px] text-dark-300 uppercase font-mono tracking-widest">
                  <th className="px-4 py-2 text-left">Kind</th>
                  <th className="px-4 py-2 text-left">Resource</th>
                  <th className="px-4 py-2 text-left">Server</th>
                  <th className="px-4 py-2 text-left">Size</th>
                  <th className="px-4 py-2 text-left">Created</th>
                  <th className="px-4 py-2 text-right">Actions</th>
                </tr>
              </thead>
              <tbody className="divide-y divide-dark-600">
                {data.items.map(row => (
                  <tr key={`${row.kind}-${row.id}`} className="hover:bg-dark-700/50">
                    <td className="px-4 py-2">{kindBadge(row.kind)}</td>
                    <td className="px-4 py-2">
                      <div className="text-dark-50 font-mono truncate max-w-[280px]" title={row.resource_name}>
                        {row.resource_name}
                      </div>
                      <div className="flex gap-1.5 mt-0.5">
                        {row.extra_type && (
                          <span className="text-[10px] text-dark-300 font-mono">{row.extra_type}</span>
                        )}
                        {row.encrypted && (
                          <span className="text-[9px] px-1 py-0 bg-dark-700 text-accent-400 uppercase tracking-wider rounded" title="Encrypted at rest">enc</span>
                        )}
                        {row.uploaded && (
                          <span className="text-[9px] px-1 py-0 bg-dark-700 text-rust-400 uppercase tracking-wider rounded" title="Pushed to remote destination">remote</span>
                        )}
                      </div>
                    </td>
                    <td className="px-4 py-2">
                      <span className="text-dark-100 font-mono">{row.server_name}</span>
                      {row.server_is_local && (
                        <span className="ml-1 text-[9px] text-dark-300 uppercase tracking-wider">local</span>
                      )}
                    </td>
                    <td className="px-4 py-2 text-dark-100 font-mono">{formatSize(row.size_bytes)}</td>
                    <td className="px-4 py-2 text-dark-200 font-mono">
                      <div>{formatDate(row.created_at)}</div>
                      <div className="text-[10px] text-dark-300">{timeAgo(row.created_at)}</div>
                    </td>
                    <td className="px-4 py-2 text-right">
                      {pendingDrillId === row.id ? (
                        <div className="inline-flex items-center gap-1.5">
                          <span className="text-[10px] text-dark-300 font-mono mr-1" title={drillCostHint(row.kind)}>
                            {drillCostHint(row.kind)}
                          </span>
                          <button
                            type="button"
                            onClick={() => handleDrillClick(row)}
                            className="px-2.5 py-1 text-[10px] font-mono uppercase tracking-wider bg-rust-500/10 hover:bg-rust-500/20 text-rust-400 border border-rust-500/40 rounded"
                          >Confirm</button>
                          <button
                            type="button"
                            onClick={() => setPendingDrillId(null)}
                            className="px-2.5 py-1 text-[10px] font-mono uppercase tracking-wider bg-dark-700 hover:bg-dark-600 text-dark-200 rounded"
                          >Cancel</button>
                        </div>
                      ) : (
                        <div className="inline-flex items-center gap-1.5">
                          <span
                            className="inline-flex items-center rounded border border-dark-500 overflow-hidden"
                            title="Chain-of-trust report: this backup's full provenance — integrity hashes, every verification, every restore drill"
                          >
                            <span className="px-2 py-1 text-[10px] font-mono uppercase tracking-wider bg-dark-800 text-dark-300 border-r border-dark-500">
                              Report
                            </span>
                            <a
                              href={`/api/backup-orchestrator/chain-report/${row.kind}/${row.id}`}
                              target="_blank"
                              rel="noopener noreferrer"
                              className="px-2 py-1 text-[10px] font-mono uppercase tracking-wider bg-dark-700 hover:bg-dark-600 text-accent-400 border-r border-dark-500"
                            >JSON</a>
                            <a
                              href={`/api/backup-orchestrator/chain-report/${row.kind}/${row.id}/pdf`}
                              target="_blank"
                              rel="noopener noreferrer"
                              className="px-2 py-1 text-[10px] font-mono uppercase tracking-wider bg-dark-700 hover:bg-dark-600 text-rust-400"
                            >PDF</a>
                          </span>
                          <button
                            type="button"
                            onClick={() => handleDrillClick(row)}
                            className="px-2.5 py-1 text-[10px] font-mono uppercase tracking-wider bg-dark-700 hover:bg-dark-600 text-rust-400 border border-rust-500/30 rounded"
                            title={
                              row.kind === "site"
                                ? "Restore into a scratch nginx and probe HTTP — proves the backup is actually deployable"
                              : row.kind === "database"
                                ? "Boot a scratch engine, restore the dump, count rows — proves the backup is actually queryable"
                                : "Restore into a scratch Docker volume + read-test through a probe container — proves the backup is actually mountable"
                            }
                          >
                            Drill
                          </button>
                        </div>
                      )}
                    </td>
                  </tr>
                ))}
              </tbody>
            </table>
          </div>
        )}
      </div>

      {/* Pagination */}
      {(hasPrev || hasNext) && (
        <div className="flex justify-between items-center text-xs font-mono text-dark-300">
          <span>
            Showing {offset + 1}–{offset + data.items.length} of {data.total.toLocaleString()}
          </span>
          <div className="flex gap-2">
            <button
              onClick={() => onPage(Math.max(0, offset - pageSize))}
              disabled={!hasPrev || loading}
              className="px-3 py-1 bg-dark-700 hover:bg-dark-600 disabled:opacity-40 disabled:cursor-not-allowed text-dark-100 rounded"
            >Prev</button>
            <button
              onClick={() => onPage(offset + pageSize)}
              disabled={!hasNext || loading}
              className="px-3 py-1 bg-dark-700 hover:bg-dark-600 disabled:opacity-40 disabled:cursor-not-allowed text-dark-100 rounded"
            >Next</button>
          </div>
        </div>
      )}
    </div>
  );
}

// ── Destinations Tab ──────────────────────────────────────────────────────
//
// This is where a destination is created, edited and deleted. It reads as new
// UI and is not: the form below was written in March, in Settings, and was
// switched off by the commit that moved this tab here (fd44a31) — which brought
// across the list and the Test button and left the three mutating endpoints with
// no caller in the panel at all. What the empty state then told operators to do,
// "add one via Settings", was the one place the control had just been taken from
// (GH #93). The docs have described the button below since that same commit.

const DEST_INPUT =
  "w-full px-3 py-2 bg-dark-900 border border-dark-500 rounded-lg text-sm font-mono text-dark-50 focus:ring-2 focus:ring-accent-500 outline-none";
const DEST_LABEL = "block text-xs font-medium text-dark-100 mb-1 font-mono";

function DestField({
  label, value, onChange, placeholder, type = "text", hint,
}: {
  label: string; value: string; onChange: (v: string) => void;
  placeholder?: string; type?: string; hint?: string;
}) {
  return (
    <div>
      <label className={DEST_LABEL}>{label}</label>
      <input type={type} value={value} placeholder={placeholder}
        onChange={e => onChange(e.target.value)} className={DEST_INPUT} />
      {hint && <p className="text-[10px] text-dark-300 mt-1 font-mono">{hint}</p>}
    </div>
  );
}

function DestinationsTab({
  destinations, showForm, editingId, form, setForm, saving, testingId,
  pendingDeleteId, setPendingDeleteId, onNew, onEdit, onCancel, onSave, onTest, onDelete,
}: {
  destinations: Destination[];
  showForm: boolean;
  editingId: string | null;
  form: DestForm;
  setForm: (v: DestForm) => void;
  saving: boolean;
  testingId: string | null;
  pendingDeleteId: string | null;
  setPendingDeleteId: (v: string | null) => void;
  onNew: () => void;
  onEdit: (d: Destination) => void;
  onCancel: () => void;
  onSave: () => void;
  onTest: (d: Destination) => void;
  onDelete: (d: Destination) => void;
}) {
  const set = (patch: Partial<DestForm>) => setForm({ ...form, ...patch });

  return (
    <div className="space-y-4">
      <div className="flex justify-end">
        <button onClick={showForm ? onCancel : onNew}
          className="px-4 py-2 bg-rust-500 text-white rounded-lg text-sm font-medium font-mono hover:bg-rust-600 transition-colors">
          {showForm ? "Cancel" : "Add Destination"}
        </button>
      </div>

      {showForm && (
        <div className="bg-dark-800 rounded-lg border border-dark-500 p-5 space-y-3">
          <div className="grid grid-cols-2 gap-3">
            <DestField label="Name" value={form.name} onChange={v => set({ name: v })} placeholder="Offsite S3" />
            <div>
              <label className={DEST_LABEL}>Type</label>
              <select value={form.dtype} onChange={e => set({ dtype: e.target.value })}
                disabled={editingId !== null} className={`${DEST_INPUT} disabled:opacity-50`}>
                <option value="s3">S3 / R2 / MinIO</option>
                <option value="sftp">SFTP</option>
              </select>
              {editingId !== null && (
                <p className="text-[10px] text-dark-300 mt-1 font-mono">
                  Type cannot change — delete and re-add to switch transport
                </p>
              )}
            </div>
          </div>

          {form.dtype === "s3" ? (
            <>
              <div className="grid grid-cols-2 gap-3">
                <DestField label="Bucket" value={form.bucket} onChange={v => set({ bucket: v })} placeholder="my-backups" />
                <DestField label="Region" value={form.region} onChange={v => set({ region: v })} placeholder="us-east-1" />
              </div>
              <DestField label="Endpoint URL" value={form.endpoint} onChange={v => set({ endpoint: v })}
                placeholder="https://s3.amazonaws.com"
                hint="Cloudflare R2: https://ACCOUNT_ID.r2.cloudflarestorage.com · MinIO: your own host" />
              <div className="grid grid-cols-2 gap-3">
                <DestField label="Access Key" value={form.access_key} onChange={v => set({ access_key: v })} />
                <DestField label="Secret Key" type="password" value={form.secret_key}
                  onChange={v => set({ secret_key: v })}
                  hint={editingId ? "Leave as ******** to keep the stored key" : undefined} />
              </div>
              <DestField label="Path Prefix" value={form.path_prefix} onChange={v => set({ path_prefix: v })} placeholder="backups" />
            </>
          ) : (
            <>
              <div className="grid grid-cols-2 gap-3">
                <DestField label="Host" value={form.host} onChange={v => set({ host: v })} placeholder="backup.example.com" />
                <DestField label="Port" value={form.port} onChange={v => set({ port: v })} placeholder="22" />
              </div>
              <div className="grid grid-cols-2 gap-3">
                <DestField label="Username" value={form.username} onChange={v => set({ username: v })} />
                <DestField label="Password" type="password" value={form.password}
                  onChange={v => set({ password: v })}
                  hint={editingId ? "Leave as ******** to keep the stored password" : undefined} />
              </div>
              <DestField label="Remote Path" value={form.remote_path} onChange={v => set({ remote_path: v })} placeholder="/backups" />
            </>
          )}

          <div className="flex justify-end gap-2 pt-1">
            <button onClick={onCancel}
              className="px-4 py-2 bg-dark-700 text-dark-100 rounded-lg text-sm font-medium font-mono hover:bg-dark-600">
              Cancel
            </button>
            <button onClick={onSave} disabled={saving}
              className="px-4 py-2 bg-rust-500 text-white rounded-lg text-sm font-medium font-mono hover:bg-rust-600 disabled:opacity-50">
              {saving ? "Saving..." : editingId ? "Save Changes" : "Create Destination"}
            </button>
          </div>
          <p className="text-[10px] text-dark-300 font-mono">
            Credentials are encrypted before they are stored. Use Test after saving — nothing is
            verified at save time.
          </p>
        </div>
      )}

      {destinations.length === 0 && !showForm ? (
        <div className="p-12 text-center">
          <p className="text-dark-200 text-sm font-mono">No backup destinations configured</p>
          <p className="text-dark-300 text-xs mt-1 font-mono">
            Backups without a destination stay on this server, and are lost with it. Add S3-compatible
            storage or an SFTP server, then select it on a policy.
          </p>
        </div>
      ) : (
        <div className="space-y-2">
          {destinations.map(d => (
            <div key={d.id} className="flex items-center justify-between p-3 bg-dark-800 rounded-lg border border-dark-500">
              <div className="min-w-0">
                <div className="flex items-center gap-2">
                  <span className="text-sm font-medium text-dark-50 truncate">{d.name}</span>
                  <span className="px-2 py-0.5 text-[10px] font-mono uppercase bg-dark-600 text-dark-300 rounded">{d.dtype}</span>
                </div>
                <p className="text-xs text-dark-300 font-mono truncate mt-0.5">
                  {d.dtype === "s3"
                    ? `${d.config?.bucket ?? "(no bucket)"}${d.config?.endpoint ? ` @ ${d.config.endpoint}` : ""}`
                    : `${d.config?.username ? `${d.config.username}@` : ""}${d.config?.host ?? "(no host)"}:${d.config?.port ?? 22}${d.config?.remote_path ?? ""}`}
                </p>
              </div>
              <div className="flex items-center gap-2 shrink-0">
                <button onClick={() => onTest(d)} disabled={testingId === d.id}
                  className="px-3 py-1 text-xs font-mono bg-accent-500/10 text-accent-400 rounded hover:bg-accent-500/20 disabled:opacity-50">
                  {testingId === d.id ? "Testing..." : "Test"}
                </button>
                <button onClick={() => onEdit(d)}
                  className="px-3 py-1 text-xs font-mono bg-dark-600 hover:bg-dark-500 text-dark-200 rounded">
                  Edit
                </button>
                {pendingDeleteId === d.id ? (
                  <>
                    <button onClick={() => onDelete(d)}
                      className="px-3 py-1 text-xs font-mono bg-danger-500/20 text-danger-400 border border-danger-500/40 rounded hover:bg-danger-500/30">
                      Confirm
                    </button>
                    <button onClick={() => setPendingDeleteId(null)}
                      className="px-3 py-1 text-xs font-mono bg-dark-700 hover:bg-dark-600 text-dark-200 rounded">
                      Cancel
                    </button>
                  </>
                ) : (
                  <button onClick={() => setPendingDeleteId(d.id)}
                    className="px-3 py-1 text-xs font-mono bg-danger-500/10 text-danger-400 rounded hover:bg-danger-500/20">
                    Delete
                  </button>
                )}
              </div>
            </div>
          ))}
        </div>
      )}
    </div>
  );
}

// ── Policies Tab ──────────────────────────────────────────────────────────

function PoliciesTab({
  policies, destinations, showForm, setShowForm, form, setForm, onCreate, onDelete,
  editingId, onEdit, onCancelEdit, onProtectAll, protectAllBusy
}: {
  policies: BackupPolicy[]; destinations: Destination[];
  showForm: boolean; setShowForm: (v: boolean) => void;
  form: PolicyForm;
  setForm: (v: PolicyForm) => void;
  onCreate: () => void; onDelete: (id: string) => void;
  editingId: string | null;
  onEdit: (p: BackupPolicy) => void;
  onCancelEdit: () => void;
  onProtectAll: () => void;
  protectAllBusy: boolean;
}) {
  const editing = editingId !== null;
  return (
    <div className="space-y-4">
      <div className="flex justify-end gap-2">
        {/* The guide's Quick Start and the preflight remediation have both told
            operators to click this since the preset shipped; the handler was
            complete and no button ever called it. */}
        <button onClick={onProtectAll} disabled={protectAllBusy}
          title="Create the one-click preset: sites, databases and volumes, nightly at 2 AM, keep 7, auto-verify"
          className="px-4 py-2 bg-dark-700 text-dark-50 border border-dark-500 rounded-lg text-sm font-medium font-mono hover:bg-dark-600 transition-colors disabled:opacity-50">
          {protectAllBusy ? "Working…" : "Protect Everything"}
        </button>
        <button onClick={() => (editing ? onCancelEdit() : setShowForm(!showForm))}
          className="px-4 py-2 bg-rust-500 text-white rounded-lg text-sm font-medium font-mono hover:bg-rust-600 transition-colors">
          {showForm || editing ? "Cancel" : "Create Policy"}
        </button>
      </div>

      {(showForm || editing) && (
        <div className="bg-dark-800 rounded-lg border border-dark-500 p-5 space-y-3">
          {editing && (
            <p className="text-xs text-accent-400 font-mono">Editing "{form.name}"</p>
          )}
          <div className="grid grid-cols-2 gap-3">
            <div>
              <label className="block text-xs font-medium text-dark-100 mb-1 font-mono">Name</label>
              <input type="text" value={form.name} onChange={e => setForm({ ...form, name: e.target.value })}
                className="w-full px-3 py-2 bg-dark-900 border border-dark-500 rounded-lg text-sm font-mono text-dark-50 focus:ring-2 focus:ring-accent-500 outline-none" />
            </div>
            <div>
              <label className="block text-xs font-medium text-dark-100 mb-1 font-mono">Schedule (Cron)</label>
              <select value={form.schedule} onChange={e => setForm({ ...form, schedule: e.target.value })}
                className="w-full px-3 py-2 bg-dark-900 border border-dark-500 rounded-lg text-sm font-mono text-dark-50 focus:ring-2 focus:ring-accent-500 outline-none">
                <option value="0 2 * * *">Daily 2 AM</option>
                <option value="0 4 * * *">Daily 4 AM</option>
                <option value="0 */12 * * *">Every 12 hours</option>
                <option value="0 3 * * 0">Weekly (Sun 3 AM)</option>
                <option value="0 3 1 * *">Monthly (1st, 3 AM)</option>
              </select>
            </div>
          </div>
          <div className="grid grid-cols-2 gap-3">
            <div>
              <label className="block text-xs font-medium text-dark-100 mb-1 font-mono">Destination</label>
              <select value={form.destination_id} onChange={e => setForm({ ...form, destination_id: e.target.value })}
                className="w-full px-3 py-2 bg-dark-900 border border-dark-500 rounded-lg text-sm font-mono text-dark-50 focus:ring-2 focus:ring-accent-500 outline-none">
                <option value="">Local only</option>
                {destinations.map(d => <option key={d.id} value={d.id}>{d.name} ({d.dtype})</option>)}
              </select>
              <FieldHelp id="backups.policy.destination" />
            </div>
            <div>
              <label className="block text-xs font-medium text-dark-100 mb-1 font-mono">Retention (backups)</label>
              <input type="number" value={form.retention_count} min={1} max={365}
                onChange={e => setForm({ ...form, retention_count: parseInt(e.target.value) || 7 })}
                className="w-full px-3 py-2 bg-dark-900 border border-dark-500 rounded-lg text-sm font-mono text-dark-50 focus:ring-2 focus:ring-accent-500 outline-none" />
            </div>
          </div>
          <div className="flex gap-4 text-sm font-mono">
            <label className="flex items-center gap-2 text-dark-100">
              <input type="checkbox" checked={form.backup_sites} onChange={e => setForm({ ...form, backup_sites: e.target.checked })} /> Sites
            </label>
            <label className="flex items-center gap-2 text-dark-100">
              <input type="checkbox" checked={form.backup_databases} onChange={e => setForm({ ...form, backup_databases: e.target.checked })} /> Databases
            </label>
            <label className="flex items-center gap-2 text-dark-100">
              <input type="checkbox" checked={form.backup_volumes} onChange={e => setForm({ ...form, backup_volumes: e.target.checked })} /> Volumes
            </label>
            {/* Scoped deliberately. The derived key reaches the database dump
                and nothing else: the agent has no encryption path for site or
                volume archives at all, so an unqualified "Encrypt" promised
                something the product cannot do. */}
            <label className="flex items-center gap-2 text-dark-100" title="Applies to database dumps only. Site and volume archives are stored and uploaded unencrypted.">
              <input type="checkbox" checked={form.encrypt} onChange={e => setForm({ ...form, encrypt: e.target.checked })} /> Encrypt DB dumps
            </label>
            <label className="flex items-center gap-2 text-dark-100">
              <input type="checkbox" checked={form.verify_after_backup} onChange={e => setForm({ ...form, verify_after_backup: e.target.checked })} /> Auto-verify
            </label>
          </div>
          <div className="flex items-center gap-3 text-sm font-mono pt-2 border-t border-dark-600">
            <label className="flex items-center gap-2 text-dark-100">
              <input type="checkbox" checked={form.drill_enabled} onChange={e => setForm({ ...form, drill_enabled: e.target.checked })} /> Drill on schedule
            </label>
            <select
              value={form.drill_schedule}
              onChange={e => setForm({ ...form, drill_schedule: e.target.value })}
              disabled={!form.drill_enabled}
              className="px-3 py-1.5 bg-dark-900 border border-dark-500 rounded-lg text-xs font-mono text-dark-50 focus:ring-2 focus:ring-accent-500 outline-none disabled:opacity-50"
            >
              <option value="0 4 * * 0">Weekly (Sun 4 AM)</option>
              <option value="0 4 * * 6">Weekly (Sat 4 AM)</option>
              <option value="0 5 1 * *">Monthly (1st, 5 AM)</option>
              <option value="0 4 */3 * *">Every 3 days</option>
            </select>
            <span className="text-[10px] text-dark-300">
              End-to-end restore probe of latest db + volume backup. Site backups not yet covered.
            </span>
          </div>
          {form.encrypt && (form.backup_sites || form.backup_volumes) && (
            <p className="text-[11px] text-warn-400 font-mono">
              Encryption covers database dumps only. This policy also backs up{" "}
              {[form.backup_sites && "sites", form.backup_volumes && "volumes"].filter(Boolean).join(" and ")}
              , and those archives are written and uploaded unencrypted — a site archive contains its webroot,
              including configuration files and any database credentials in them.
            </p>
          )}
          {form.destination_id !== "" && destinations.find(d => d.id === form.destination_id)?.dtype === "sftp" && (
            <p className="text-[11px] text-warn-400 font-mono">
              Retention is enforced on this server only. The agent cannot prune an SFTP destination, so remote copies accumulate there until you remove them.
            </p>
          )}
          {editing && (
            <div className="flex items-center gap-2 text-sm font-mono pt-2 border-t border-dark-600">
              <label className="flex items-center gap-2 text-dark-100">
                <input type="checkbox" checked={form.enabled} onChange={e => setForm({ ...form, enabled: e.target.checked })} /> Enabled
              </label>
              <span className="text-[10px] text-dark-300">Unticking pauses the policy without deleting it or its history.</span>
            </div>
          )}
          <div className="flex justify-end">
            <button onClick={onCreate}
              className="px-4 py-2 bg-rust-500 text-white rounded-lg text-sm font-medium font-mono hover:bg-rust-600">
              {editing ? "Save Changes" : "Create"}
            </button>
          </div>
        </div>
      )}

      {policies.length === 0 ? (
        <div className="p-12 text-center">
          <p className="text-dark-200 text-sm font-mono">No backup policies yet</p>
          <p className="text-dark-300 text-xs mt-1 font-mono">Create a policy to automate backups across sites, databases, and volumes</p>
        </div>
      ) : (
        <div className="bg-dark-800 rounded-lg border border-dark-500 overflow-x-auto">
          <table className="w-full">
            <thead>
              <tr className="bg-dark-900 border-b border-dark-500">
                {["Name", "Schedule", "Scope", "Retention", "Status", "Last Run", ""].map(h => (
                  <th key={h} className="text-left text-xs font-medium text-dark-200 uppercase font-mono tracking-widest px-5 py-3">{h}</th>
                ))}
              </tr>
            </thead>
            <tbody className="divide-y divide-dark-600">
              {policies.map(p => (
                <tr key={p.id} className="hover:bg-dark-700/30 transition-colors">
                  <td className="px-5 py-4 text-sm text-dark-50 font-mono">{p.name}</td>
                  <td className="px-5 py-4 text-sm text-dark-200 font-mono">
                    <div>{p.schedule}</div>
                    {p.drill_enabled && (
                      <div className="text-[10px] text-rust-400 mt-0.5" title={`Drill schedule: ${p.drill_schedule}${p.last_drill_at ? ` · last drilled ${timeAgo(p.last_drill_at)}` : ""}`}>
                        drill {p.drill_schedule}
                      </div>
                    )}
                  </td>
                  <td className="px-5 py-4 text-xs font-mono">
                    {p.backup_sites && <span className="inline-flex px-2 py-0.5 rounded-full bg-rust-500/15 text-rust-400 mr-1">Sites</span>}
                    {p.backup_databases && <span className="inline-flex px-2 py-0.5 rounded-full bg-accent-500/15 text-accent-400 mr-1">DBs</span>}
                    {p.backup_volumes && <span className="inline-flex px-2 py-0.5 rounded-full bg-warn-500/15 text-warn-400">Vols</span>}
                  </td>
                  <td className="px-5 py-4 text-sm text-dark-200 font-mono">{p.retention_count}</td>
                  <td className="px-5 py-4">
                    <span className={`inline-flex px-2 py-0.5 rounded-full text-xs font-medium font-mono ${
                      p.enabled ? "bg-rust-500/15 text-rust-400" : "bg-dark-700 text-dark-200"
                    }`}>{p.enabled ? "Active" : "Paused"}</span>
                  </td>
                  <td className="px-5 py-4 text-xs text-dark-300 font-mono">
                    {p.last_run ? timeAgo(p.last_run) : "Never"}
                    {p.last_status && (
                      <span className={`ml-1 ${p.last_status === "success" ? "text-rust-400" : "text-danger-400"}`}>
                        ({p.last_status})
                      </span>
                    )}
                  </td>
                  <td className="px-5 py-4">
                    <div className="flex gap-2">
                      <button onClick={() => onEdit(p)}
                        className="px-3 py-1 bg-dark-700 text-dark-50 rounded-md text-xs font-medium font-mono hover:bg-dark-600 transition-colors">
                        Edit
                      </button>
                      <button onClick={() => onDelete(p.id)}
                        className="px-3 py-1 bg-danger-500/10 text-danger-400 rounded-md text-xs font-medium font-mono hover:bg-danger-500/20 transition-colors">
                        Delete
                      </button>
                    </div>
                  </td>
                </tr>
              ))}
            </tbody>
          </table>
        </div>
      )}
    </div>
  );
}

// ── Database Backups Tab ──────────────────────────────────────────────────

function DatabasesTab({
  backups, databases, onCreateBackup, onVerify,
  onRestore, onDelete, pendingRestoreId, setPendingRestoreId, pendingDeleteId, setPendingDeleteId
}: {
  backups: DatabaseBackup[]; databases: Database[];
  onCreateBackup: (id: string) => void; onVerify: (type: string, id: string) => void;
  onRestore: (b: DatabaseBackup) => void; onDelete: (b: DatabaseBackup) => void;
  pendingRestoreId: string | null; setPendingRestoreId: (id: string | null) => void;
  pendingDeleteId: string | null; setPendingDeleteId: (id: string | null) => void;
}) {
  return (
    <div className="space-y-4">
      {/* Quick backup buttons */}
      {databases.length > 0 && (
        <div className="bg-dark-800 rounded-lg border border-dark-500 p-4">
          <p className="text-xs text-dark-300 uppercase font-mono tracking-widest mb-3">Quick Backup</p>
          <div className="flex flex-wrap gap-2">
            {databases.map(db => (
              <button key={db.id} onClick={() => onCreateBackup(db.id)}
                className="px-3 py-1.5 bg-dark-700 text-dark-100 rounded-lg text-xs font-mono hover:bg-dark-600 transition-colors border border-dark-500">
                {db.name} <span className="text-dark-300">({db.engine})</span>
              </button>
            ))}
          </div>
        </div>
      )}

      {backups.length === 0 ? (
        <div className="p-12 text-center">
          <p className="text-dark-200 text-sm font-mono">No database backups yet</p>
          <p className="text-dark-300 text-xs mt-1 font-mono">Click a database above to create its first backup</p>
        </div>
      ) : (
        <div className="bg-dark-800 rounded-lg border border-dark-500 overflow-x-auto">
          <table className="w-full">
            <thead>
              <tr className="bg-dark-900 border-b border-dark-500">
                {["Database", "Type", "Filename", "Size", "Encrypted", "Created", ""].map(h => (
                  <th key={h} className="text-left text-xs font-medium text-dark-200 uppercase font-mono tracking-widest px-5 py-3">{h}</th>
                ))}
              </tr>
            </thead>
            <tbody className="divide-y divide-dark-600">
              {backups.map(b => (
                <tr key={b.id} className="hover:bg-dark-700/30 transition-colors">
                  <td className="px-5 py-4 text-sm text-dark-50 font-mono">{b.db_name}</td>
                  <td className="px-5 py-4 text-xs text-dark-200 font-mono uppercase">{b.db_type}</td>
                  <td className="px-5 py-4 text-xs text-dark-200 font-mono truncate max-w-[200px]">{b.filename}</td>
                  <td className="px-5 py-4 text-sm text-dark-200 font-mono">{formatSize(b.size_bytes)}</td>
                  <td className="px-5 py-4">
                    {b.encrypted && <span className="inline-flex px-2 py-0.5 rounded-full text-xs font-medium bg-accent-500/15 text-accent-400 font-mono">Encrypted</span>}
                  </td>
                  <td className="px-5 py-4 text-xs text-dark-300 font-mono">{formatDate(b.created_at)}</td>
                  <td className="px-5 py-4">
                    {pendingRestoreId === b.id ? (
                      <div className="inline-flex items-center gap-1.5">
                        <span className="text-[10px] text-danger-400 font-mono mr-1">
                          Overwrites the live {b.db_name} database
                        </span>
                        <button type="button" onClick={() => onRestore(b)}
                          className="px-2.5 py-1 text-[10px] font-mono uppercase tracking-wider bg-danger-500/10 hover:bg-danger-500/20 text-danger-400 border border-danger-500/40 rounded">
                          Restore
                        </button>
                        <button type="button" onClick={() => setPendingRestoreId(null)}
                          className="px-2.5 py-1 text-[10px] font-mono uppercase tracking-wider bg-dark-700 hover:bg-dark-600 text-dark-200 rounded">
                          Cancel
                        </button>
                      </div>
                    ) : pendingDeleteId === b.id ? (
                      <div className="inline-flex items-center gap-1.5">
                        <span className="text-[10px] text-danger-400 font-mono mr-1">
                          Deletes the archive and its record
                        </span>
                        <button type="button" onClick={() => onDelete(b)}
                          className="px-2.5 py-1 text-[10px] font-mono uppercase tracking-wider bg-danger-500/10 hover:bg-danger-500/20 text-danger-400 border border-danger-500/40 rounded">
                          Delete
                        </button>
                        <button type="button" onClick={() => setPendingDeleteId(null)}
                          className="px-2.5 py-1 text-[10px] font-mono uppercase tracking-wider bg-dark-700 hover:bg-dark-600 text-dark-200 rounded">
                          Cancel
                        </button>
                      </div>
                    ) : (
                      <div className="inline-flex items-center gap-1.5">
                        <button onClick={() => onVerify("database", b.id)}
                          className="px-3 py-1 bg-accent-500/10 text-accent-400 rounded-md text-xs font-medium font-mono hover:bg-accent-500/20 transition-colors">
                          Verify
                        </button>
                        <button type="button"
                          onClick={() => { setPendingDeleteId(null); setPendingRestoreId(b.id); }}
                          title="Restore this dump over the live database it was taken from"
                          className="px-3 py-1 bg-warn-500/10 text-warn-400 rounded-md text-xs font-medium font-mono hover:bg-warn-500/20 transition-colors">
                          Restore
                        </button>
                        <button type="button"
                          onClick={() => { setPendingRestoreId(null); setPendingDeleteId(b.id); }}
                          title="Delete this archive from the host that holds it, and its record"
                          className="px-3 py-1 bg-dark-700 text-dark-200 rounded-md text-xs font-medium font-mono hover:bg-danger-500/20 hover:text-danger-400 transition-colors">
                          Delete
                        </button>
                      </div>
                    )}
                  </td>
                </tr>
              ))}
            </tbody>
          </table>
        </div>
      )}
    </div>
  );
}

// ── Volume Backups Tab ────────────────────────────────────────────────────

function VolumesTab({
  backups, onVerify, onRestore, pendingRestoreId, setPendingRestoreId
}: {
  backups: VolumeBackup[]; onVerify: (type: string, id: string) => void;
  onRestore: (b: VolumeBackup) => void;
  pendingRestoreId: string | null; setPendingRestoreId: (id: string | null) => void;
}) {
  return backups.length === 0 ? (
    <div className="p-12 text-center">
      <p className="text-dark-200 text-sm font-mono">No volume backups yet</p>
      <p className="text-dark-300 text-xs mt-1 font-mono">Volume backups will appear here when created via policies or API</p>
    </div>
  ) : (
    <div className="bg-dark-800 rounded-lg border border-dark-500 overflow-x-auto">
      <table className="w-full">
        <thead>
          <tr className="bg-dark-900 border-b border-dark-500">
            {["Container", "Volume", "Filename", "Size", "Created", ""].map(h => (
              <th key={h} className="text-left text-xs font-medium text-dark-200 uppercase font-mono tracking-widest px-5 py-3">{h}</th>
            ))}
          </tr>
        </thead>
        <tbody className="divide-y divide-dark-600">
          {backups.map(b => (
            <tr key={b.id} className="hover:bg-dark-700/30 transition-colors">
              <td className="px-5 py-4 text-sm text-dark-50 font-mono">{b.container_name}</td>
              <td className="px-5 py-4 text-sm text-dark-200 font-mono">{b.volume_name}</td>
              <td className="px-5 py-4 text-xs text-dark-200 font-mono truncate max-w-[200px]">{b.filename}</td>
              <td className="px-5 py-4 text-sm text-dark-200 font-mono">{formatSize(b.size_bytes)}</td>
              <td className="px-5 py-4 text-xs text-dark-300 font-mono">{formatDate(b.created_at)}</td>
              <td className="px-5 py-4">
                {pendingRestoreId === b.id ? (
                  <div className="inline-flex items-center gap-1.5">
                    <span className="text-[10px] text-danger-400 font-mono mr-1">
                      Unpacks over the live {b.volume_name} volume
                    </span>
                    <button type="button" onClick={() => onRestore(b)}
                      className="px-2.5 py-1 text-[10px] font-mono uppercase tracking-wider bg-danger-500/10 hover:bg-danger-500/20 text-danger-400 border border-danger-500/40 rounded">
                      Restore
                    </button>
                    <button type="button" onClick={() => setPendingRestoreId(null)}
                      className="px-2.5 py-1 text-[10px] font-mono uppercase tracking-wider bg-dark-700 hover:bg-dark-600 text-dark-200 rounded">
                      Cancel
                    </button>
                  </div>
                ) : (
                  <div className="inline-flex items-center gap-1.5">
                    <button onClick={() => onVerify("volume", b.id)}
                      className="px-3 py-1 bg-accent-500/10 text-accent-400 rounded-md text-xs font-medium font-mono hover:bg-accent-500/20 transition-colors">
                      Verify
                    </button>
                    <button type="button" onClick={() => setPendingRestoreId(b.id)}
                      title="Unpack this archive over the live volume it was taken from"
                      className="px-3 py-1 bg-warn-500/10 text-warn-400 rounded-md text-xs font-medium font-mono hover:bg-warn-500/20 transition-colors">
                      Restore
                    </button>
                  </div>
                )}
              </td>
            </tr>
          ))}
        </tbody>
      </table>
    </div>
  );
}

// ── Verifications Tab ─────────────────────────────────────────────────────

function VerificationsTab({ verifications }: { verifications: Verification[] }) {
  return verifications.length === 0 ? (
    <div className="p-12 text-center">
      <p className="text-dark-200 text-sm font-mono">No verifications yet</p>
      <p className="text-dark-300 text-xs mt-1 font-mono">Trigger a verification from any backup or enable auto-verify in policies</p>
    </div>
  ) : (
    <div className="bg-dark-800 rounded-lg border border-dark-500 overflow-x-auto">
      <table className="w-full">
        <thead>
          <tr className="bg-dark-900 border-b border-dark-500">
            {["Type", "Status", "Checks", "Duration", "Error", "Created"].map(h => (
              <th key={h} className="text-left text-xs font-medium text-dark-200 uppercase font-mono tracking-widest px-5 py-3">{h}</th>
            ))}
          </tr>
        </thead>
        <tbody className="divide-y divide-dark-600">
          {verifications.map(v => (
            <tr key={v.id} className="hover:bg-dark-700/30 transition-colors">
              <td className="px-5 py-4 text-sm text-dark-50 font-mono capitalize">{v.backup_type}</td>
              <td className="px-5 py-4">
                <span className={`inline-flex px-2 py-0.5 rounded-full text-xs font-medium font-mono ${
                  v.status === "passed" ? "bg-rust-500/15 text-rust-400"
                  : v.status === "failed" ? "bg-danger-500/15 text-danger-400"
                  : v.status === "running" ? "bg-warn-500/15 text-warn-400"
                  : "bg-dark-700 text-dark-200"
                }`}>{v.status}</span>
              </td>
              <td className="px-5 py-4 text-sm text-dark-200 font-mono">{v.checks_passed}/{v.checks_run}</td>
              <td className="px-5 py-4 text-sm text-dark-200 font-mono">{v.duration_ms ? `${v.duration_ms}ms` : "-"}</td>
              <td className="px-5 py-4 text-xs text-danger-400 font-mono truncate max-w-[200px]">{v.error_message || "-"}</td>
              <td className="px-5 py-4 text-xs text-dark-300 font-mono">{formatDate(v.created_at)}</td>
            </tr>
          ))}
        </tbody>
      </table>
    </div>
  );
}

// ── Drills Tab (Phase 4 W1.2: end-to-end restore probes) ──────────────────

function httpTone(code: number): string {
  if (code >= 200 && code < 300) return "text-rust-400";
  if (code >= 300 && code < 400) return "text-dark-100";
  if (code >= 400 && code < 500) return "text-warn-400";
  if (code >= 500) return "text-danger-400";
  return "text-dark-200";
}

function ResultCell({ drill }: { drill: Drill }) {
  if (drill.backup_type === "site") {
    if (drill.http_status == null) return <span className="text-dark-300">—</span>;
    return <span className={`font-mono ${httpTone(drill.http_status)}`}>HTTP {drill.http_status}</span>;
  }
  // database + volume: body_excerpt holds the row/table or file/byte summary
  return (
    <span className="text-dark-100 font-mono">
      {drill.body_excerpt ?? <span className="text-dark-300">—</span>}
    </span>
  );
}

function DrillStatusPill({ status }: { status: string }) {
  const tone =
    status === "passed" ? "bg-rust-500/15 text-rust-400"
    : status === "failed" ? "bg-danger-500/15 text-danger-400"
    : status === "running" ? "bg-warn-500/15 text-warn-400"
    : "bg-dark-700 text-dark-200";
  return (
    <span className={`inline-flex items-center gap-1.5 px-2 py-0.5 rounded-full text-xs font-medium font-mono ${tone}`}>
      {status === "running" && (
        <span className="relative flex h-1.5 w-1.5" aria-hidden="true">
          <span className="absolute inline-flex h-full w-full rounded-full bg-warn-400 opacity-60 animate-ping" />
          <span className="relative inline-flex h-1.5 w-1.5 rounded-full bg-warn-400" />
        </span>
      )}
      {status}
    </span>
  );
}

function DrillsTab({
  data, loading, offset, pageSize, onPage, onRefresh,
}: {
  data: DrillsResponse;
  loading: boolean;
  offset: number;
  pageSize: number;
  onPage: (offset: number) => void;
  onRefresh: () => void;
}) {
  const hasNext = offset + data.items.length < data.total;
  const hasPrev = offset > 0;
  const hasRunning = data.items.some(d => d.status === "running");

  if (data.items.length === 0 && !loading) {
    return (
      <div className="p-12 text-center">
        <p className="text-dark-200 text-sm font-mono">No drills yet</p>
        <p className="text-dark-300 text-xs mt-1 font-mono">Click <span className="text-rust-400">Drill</span> on any backup in the All Backups tab to do a real end-to-end restore probe.</p>
      </div>
    );
  }

  return (
    <div className="space-y-4">
      <div className="flex items-center justify-between text-xs font-mono text-dark-300">
        <span>
          {loading ? "Loading…" : `${data.total.toLocaleString()} total`}
          {hasRunning && !loading && <span className="ml-2 text-warn-400">· {data.items.filter(d => d.status === "running").length} running</span>}
        </span>
        {hasRunning && (
          <button
            type="button"
            onClick={onRefresh}
            disabled={loading}
            className="px-2.5 py-1 text-[10px] uppercase tracking-wider bg-dark-700 hover:bg-dark-600 text-dark-100 rounded disabled:opacity-40"
          >Refresh</button>
        )}
      </div>

      <div className="bg-dark-800 rounded-lg border border-dark-500 overflow-x-auto">
        <table className="w-full">
          <thead>
            <tr className="bg-dark-900 border-b border-dark-500">
              {["Type", "Status", "Result", "Duration", "Error", "Created"].map(h => (
                <th key={h} className="text-left text-xs font-medium text-dark-200 uppercase font-mono tracking-widest px-5 py-3">{h}</th>
              ))}
            </tr>
          </thead>
          <tbody className="divide-y divide-dark-600">
            {data.items.map(d => (
              <tr key={d.id} className="hover:bg-dark-700/30 transition-colors">
                <td className="px-5 py-4 text-sm text-dark-50 font-mono capitalize">{d.backup_type}</td>
                <td className="px-5 py-4"><DrillStatusPill status={d.status} /></td>
                <td className="px-5 py-4 text-sm truncate max-w-[260px]" title={d.body_excerpt ?? ""}>
                  <ResultCell drill={d} />
                </td>
                <td className="px-5 py-4 text-sm text-dark-200 font-mono">{d.duration_ms ? `${(d.duration_ms / 1000).toFixed(1)}s` : "—"}</td>
                <td className="px-5 py-4 text-xs text-danger-400 font-mono truncate max-w-[240px]" title={d.error_message ?? ""}>{d.error_message || "—"}</td>
                <td className="px-5 py-4 text-xs text-dark-300 font-mono" title={formatDate(d.created_at)}>{timeAgo(d.created_at)}</td>
              </tr>
            ))}
          </tbody>
        </table>
      </div>

      {(hasPrev || hasNext) && (
        <div className="flex justify-between items-center text-xs font-mono text-dark-300">
          <span>
            Showing {offset + 1}–{offset + data.items.length} of {data.total.toLocaleString()}
          </span>
          <div className="flex gap-2">
            <button
              type="button"
              onClick={() => onPage(Math.max(0, offset - pageSize))}
              disabled={!hasPrev || loading}
              className="px-3 py-1 bg-dark-700 hover:bg-dark-600 disabled:opacity-40 disabled:cursor-not-allowed text-dark-100 rounded"
            >Prev</button>
            <button
              type="button"
              onClick={() => onPage(offset + pageSize)}
              disabled={!hasNext || loading}
              className="px-3 py-1 bg-dark-700 hover:bg-dark-600 disabled:opacity-40 disabled:cursor-not-allowed text-dark-100 rounded"
            >Next</button>
          </div>
        </div>
      )}
    </div>
  );
}
