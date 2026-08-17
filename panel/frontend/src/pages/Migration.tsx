import { useAuth } from "../context/AuthContext";
import { Navigate } from "react-router-dom";
import { useState, useEffect, useRef } from "react";
import { api, ApiError } from "../api";

interface MigrationSite {
  domain: string;
  doc_root: string;
  size_bytes: number;
  runtime: string;
  file_count: number;
}

interface MigrationDb {
  name: string;
  file: string;
  size_bytes: number;
  engine: string;
}

interface MigrationMail {
  email: string;
  domain: string;
}

interface Inventory {
  id: string;
  source: string;
  sites: MigrationSite[];
  databases: MigrationDb[];
  mail_accounts: MigrationMail[];
  warnings: string[];
}

interface MigrationRecord {
  id: string;
  source: string;
  status: string;
  server_id: string | null;
  backup_path: string;
  inventory: Inventory | null;
  result: Record<string, unknown> | null;
  created_at: string;
}

interface ProgressStep {
  step: string;
  label: string;
  status: string;
  message?: string;
}

const fmtElapsed = (s: number) =>
  s < 60 ? `${s}s` : `${Math.floor(s / 60)}m ${String(s % 60).padStart(2, "0")}s`;

const fmtSize = (b: number) => {
  if (b > 1e9) return `${(b / 1e9).toFixed(1)} GB`;
  if (b > 1e6) return `${(b / 1e6).toFixed(1)} MB`;
  if (b > 1e3) return `${(b / 1e3).toFixed(0)} KB`;
  return `${b} B`;
};

export default function Migration() {
  const { user } = useAuth();
  // The admin redirect used to sit above these hooks. That was survivable while
  // every hook was a `useState`, but this page now runs effects on mount, and a
  // conditional return before them makes the hook count differ between the
  // render where `user` is still loading and the one after it — React's
  // "rendered more hooks than during the previous render". The guard moved
  // below the hooks; the redirect is unchanged.
  const [step, setStep] = useState<1 | 2 | 3 | 4>(1);
  const [source, setSource] = useState("cpanel");
  const [backupPath, setBackupPath] = useState("");
  const [analyzing, setAnalyzing] = useState(false);
  const [error, setError] = useState("");
  const [migration, setMigration] = useState<MigrationRecord | null>(null);
  const [selectedSites, setSelectedSites] = useState<Set<string>>(new Set());
  const [selectedDbs, setSelectedDbs] = useState<Set<string>>(new Set());
  // Which site each selected database belongs to, keyed by database name. A
  // database row cannot exist without a site, and a control-panel archive does
  // not record the pairing — so it is the operator's to state. Left empty when
  // there is exactly one site to choose, because then there is nothing to ask.
  const [dbSites, setDbSites] = useState<Record<string, string>>({});
  const [progress, setProgress] = useState<ProgressStep[]>([]);
  const [importing, setImporting] = useState(false);
  const [analyzingId, setAnalyzingId] = useState<string | null>(null);
  // When the run began, not when this tab started watching it — a resumed
  // analysis has to show the age of the work, not the age of the page.
  const [startedAt, setStartedAt] = useState(0);
  const [elapsed, setElapsed] = useState(0);
  const [resumed, setResumed] = useState(false);
  const eventSourceRef = useRef<EventSource | null>(null);
  // The mount fetch below and the Analyze button both want to own `analyzingId`.
  // A fast click beats the fetch, and the resume would then quietly replace the
  // run the operator just started with an older one — same page, same spinner,
  // wrong migration. A ref, not state: the resume's callback closed over its
  // render and would never see a state update made after it began.
  const startedHere = useRef(false);

  // The analysis has finished on the server, one way or the other. Everything
  // that used to happen inline after `await` happens here instead, because the
  // record can now arrive from three places: the initial POST, the poller, or a
  // run this tab did not start.
  const settleAnalysis = (rec: MigrationRecord) => {
    setMigration(rec);
    setAnalyzingId(null);
    setAnalyzing(false);
    if (rec.status === "failed") {
      const reason = (rec.result as { error?: string } | null)?.error;
      setError(reason || "Analysis failed");
      return;
    }
    if (rec.inventory) {
      setSelectedSites(new Set(rec.inventory.sites.map((s) => s.domain)));
      setSelectedDbs(new Set(rec.inventory.databases.map((d) => d.name)));
    }
    setStep(2);
  };

  // Step 1: Analyze — accepted, then polled.
  //
  // This used to hold one request open for the whole analysis, which is minutes
  // for any real cPanel account. Every gateway in front of the panel gives up
  // sooner than that, so the browser was shown a timeout — issue #91's `Request
  // failed (524)`, which is this file's own fallback message rendering a
  // Cloudflare error page that carried no JSON to read. The work always
  // continued on the server; nothing was ever able to come back and say so.
  const handleAnalyze = async () => {
    if (!backupPath.trim()) return;
    startedHere.current = true;
    setError("");
    setAnalyzing(true);
    setResumed(false);
    setStartedAt(Date.now());
    setElapsed(0);
    try {
      const res = await api.post<MigrationRecord>("/migration/analyze", {
        path: backupPath.trim(),
        source,
      });
      if (res.status === "analyzing") {
        setMigration(res);
        setAnalyzingId(res.id);
      } else {
        settleAnalysis(res);
      }
    } catch (e) {
      setError(e instanceof ApiError ? e.message : "Analysis failed");
      setAnalyzing(false);
    }
  };

  // Poll the record until it stops saying `analyzing`. The verdict lives in the
  // row rather than in this component, so a refused poll is a network blip and
  // not an answer — keep asking. The run is bounded on the agent side, so this
  // always terminates.
  useEffect(() => {
    if (!analyzingId) return;
    let stopped = false;

    const ticker = window.setInterval(
      () => setElapsed(Math.floor((Date.now() - startedAt) / 1000)),
      1000,
    );
    const poller = window.setInterval(async () => {
      try {
        const rec = await api.get<MigrationRecord>(`/migration/${analyzingId}`);
        if (!stopped && rec.status !== "analyzing") settleAnalysis(rec);
      } catch (e) {
        // A transient failure is not an answer — the row outlives this tab, so
        // keep asking. A 404 or a 403 is: the record is gone, or is not ours, and
        // no amount of asking changes that. Without this the spinner turned
        // forever over a run that no longer existed, which is a different way of
        // telling the operator nothing — and the shape #91 was reported for.
        if (e instanceof ApiError && (e.status === 403 || e.status === 404)) {
          setAnalyzingId(null);
          setAnalyzing(false);
          setError(
            e.status === 404
              ? "This analysis is no longer on the server. Start it again."
              : e.message,
          );
        }
      }
    }, 3000);

    return () => {
      stopped = true;
      window.clearInterval(ticker);
      window.clearInterval(poller);
    };
  }, [analyzingId, startedAt]);

  // Pick an analysis back up after a reload. Persisting the verdict is only half
  // the fix; without this the operator still watches a run they can no longer
  // see, which is the complaint #91 was actually about.
  const isAdmin = user?.role === "admin";
  useEffect(() => {
    if (!isAdmin) return;
    (async () => {
      try {
        const recent = await api.get<MigrationRecord[]>("/migration");
        // Scoped to the server the operator is looking at. The list is per-user
        // and not per-server, so on a fleet this would otherwise adopt a run on
        // a different machine and present it as this one's — with a path from
        // that server's filesystem in the field.
        const activeServer = localStorage.getItem("dp-active-server");
        const live = recent.find(
          (m) =>
            m.status === "analyzing" &&
            (!activeServer || !m.server_id || m.server_id === activeServer),
        );
        if (!live || startedHere.current) return;
        setBackupPath(live.backup_path);
        setSource(live.source);
        setMigration(live);
        setResumed(true);
        setAnalyzing(true);
        setStartedAt(new Date(live.created_at).getTime());
        setElapsed(Math.floor((Date.now() - new Date(live.created_at).getTime()) / 1000));
        setAnalyzingId(live.id);
      } catch {
        /* nothing to resume is the normal case */
      }
    })();
  }, [isAdmin]);

  // The sites this run will create, which are the only ones a database imported
  // alongside them can be attached to — plus whatever the operator has already.
  const selectedSiteDomains = migration?.inventory
    ? migration.inventory.sites.filter((s) => selectedSites.has(s.domain)).map((s) => s.domain)
    : [];
  // One site is not a question. Several is, and an unanswered one is refused by
  // the server rather than guessed at, so the button below refuses first.
  const siteForDb = (name: string) =>
    dbSites[name] || (selectedSiteDomains.length === 1 ? selectedSiteDomains[0] : "");
  const dbsWithoutSite = Array.from(selectedDbs).filter((n) => !siteForDb(n));

  // Step 3: Import
  const handleImport = async () => {
    if (!migration?.inventory) return;
    setError("");
    setImporting(true);
    setProgress([]);
    setStep(3);

    try {
      await api.post(`/migration/${migration.id}/import`, {
        sites: migration.inventory.sites
          .filter((s) => selectedSites.has(s.domain))
          .map((s) => ({ domain: s.domain, doc_root: s.doc_root, runtime: s.runtime })),
        databases: migration.inventory.databases
          .filter((d) => selectedDbs.has(d.name))
          .map((d) => ({
            name: d.name,
            file: d.file,
            engine: d.engine,
            site_domain: siteForDb(d.name) || undefined,
          })),
      });

      // Connect SSE for progress
      const es = new EventSource(`/api/migration/${migration.id}/progress`);
      eventSourceRef.current = es;
      es.onmessage = (event) => {
        try {
          const data = JSON.parse(event.data) as ProgressStep;
          setProgress((prev) => {
            const existing = prev.findIndex((p) => p.step === data.step);
            if (existing >= 0) {
              const updated = [...prev];
              updated[existing] = data;
              return updated;
            }
            return [...prev, data];
          });
          if (data.step === "complete") {
            es.close();
            setImporting(false);
            setStep(4);
            // Refresh migration record
            api.get<MigrationRecord>(`/migration/${migration.id}`).then(setMigration).catch(() => {});
          }
        } catch {}
      };
      es.onerror = () => {
        es.close();
        setImporting(false);
        if (progress.length === 0) setStep(4);
      };
    } catch (e) {
      setError(e instanceof ApiError ? e.message : "Import failed");
      setImporting(false);
    }
  };

  // Cleanup on unmount
  useEffect(() => {
    return () => { eventSourceRef.current?.close(); };
  }, []);

  // Every hook above this line, without exception — see the note at the top.
  if (!isAdmin) return <Navigate to="/" replace />;

  const inv = migration?.inventory;

  return (
    <div className="p-6 space-y-6">
      <div>
        <h1 className="text-2xl font-bold text-dark-50 font-mono">Migration Wizard</h1>
        {/* "and email" used to be here. It was never true: there is no mail import
            route and no mail import function — the request body carries only `sites`
            and `databases`. cPanel archives are PARSED for mail accounts so the
            wizard can list them, and Plesk and HestiaCP do not even do that. The
            screen already said so 160 lines below, under the inventory. A header
            that promises what its own body retracts is worse than silence. #107. */}
        <p className="text-sm text-dark-300 mt-1">Import sites and databases from cPanel, Plesk, or HestiaCP — mail accounts are listed for reference, not moved</p>
      </div>

      {/* Step indicator */}
      <div className="flex items-center gap-2 text-xs font-mono">
        {[1, 2, 3, 4].map((s) => (
          <div key={s} className={`flex items-center gap-1 ${step >= s ? "text-rust-400" : "text-dark-400"}`}>
            <div className={`w-6 h-6 rounded-full flex items-center justify-center text-xs font-bold ${step >= s ? "bg-rust-500 text-dark-950" : "bg-dark-700 text-dark-400"}`}>{s}</div>
            <span className="hidden sm:inline">{["Source", "Review", "Import", "Done"][s - 1]}</span>
            {s < 4 && <span className="text-dark-600 mx-1">&mdash;</span>}
          </div>
        ))}
      </div>

      {error && (
        <div className="px-4 py-3 bg-danger-500/10 border border-danger-500/30 rounded-lg text-sm text-danger-400">{error}</div>
      )}

      {/* Step 1: Source + Path */}
      {step === 1 && (
        <div className="bg-dark-800 border border-dark-600 rounded-lg p-6 space-y-5">
          <h2 className="text-lg font-bold text-dark-50 font-mono">Select Backup Source</h2>

          <div className="flex gap-3">
            {[
              { id: "cpanel", label: "cPanel", desc: "Full backup (.tar.gz)" },
              { id: "plesk", label: "Plesk", desc: "Domain backup" },
              { id: "hestiacp", label: "HestiaCP", desc: "User backup (.tar)" },
            ].map((s) => (
              <button
                key={s.id}
                onClick={() => setSource(s.id)}
                className={`flex-1 p-4 rounded-lg border text-left transition-colors ${source === s.id ? "border-rust-500 bg-rust-500/10" : "border-dark-600 bg-dark-900 hover:border-dark-400"}`}
              >
                <div className={`text-sm font-bold ${source === s.id ? "text-rust-400" : "text-dark-200"}`}>{s.label}</div>
                <div className="text-xs text-dark-400 mt-1">{s.desc}</div>
              </button>
            ))}
          </div>

          <div>
            <label className="block text-sm text-dark-200 mb-1">Backup File Path</label>
            <input
              value={backupPath}
              onChange={(e) => setBackupPath(e.target.value)}
              placeholder="/var/backups/backup-1.2.2026_12-00-00_username.tar.gz"
              className="w-full px-3 py-2 bg-dark-900 border border-dark-600 rounded-lg text-dark-50 text-sm focus:border-rust-500 focus:outline-none font-mono"
            />
            <p className="text-xs text-dark-400 mt-1">
              Upload the backup to your server via SFTP first, then enter the full path here.
              It must be under <code className="text-dark-200">/var/backups/</code>.{" "}
              <code className="text-dark-200">/tmp/</code> will not work: the agent runs with a
              private <code className="text-dark-200">/tmp</code> and cannot see the host's.
            </p>
          </div>

          <button
            onClick={handleAnalyze}
            disabled={!backupPath.trim() || analyzing}
            className="px-5 py-2.5 bg-rust-500 text-dark-950 rounded-lg text-sm font-bold hover:bg-rust-400 transition-colors disabled:opacity-50"
          >
            {analyzing ? "Analyzing..." : "Analyze Backup"}
          </button>

          {analyzing && (
            <div className="flex items-start gap-3 px-4 py-3 bg-dark-900 border border-dark-600 rounded-lg">
              <svg className="w-4 h-4 mt-0.5 animate-spin text-rust-400 shrink-0" fill="none" viewBox="0 0 24 24">
                <circle className="opacity-25" cx="12" cy="12" r="10" stroke="currentColor" strokeWidth="4" />
                <path className="opacity-75" fill="currentColor" d="M4 12a8 8 0 0 1 8-8V0C5.4 0 0 5.4 0 12h4z" />
              </svg>
              <div className="text-sm">
                <p className="text-dark-100">
                  {resumed
                    ? "Picking up an analysis that was already running on the server."
                    : "Unpacking and reading the archive."}{" "}
                  <span className="font-mono text-dark-200">{fmtElapsed(elapsed)}</span>
                </p>
                <p className="text-xs text-dark-400 mt-1">
                  A full cPanel account takes minutes. This runs on the server, not in this tab —
                  you can leave the page and come back, and closing the browser will not stop it.
                </p>
              </div>
            </div>
          )}
        </div>
      )}

      {/* Step 2: Review */}
      {step === 2 && inv && (
        <div className="space-y-4">
          <div className="bg-dark-800 border border-dark-600 rounded-lg p-5">
            <h2 className="text-lg font-bold text-dark-50 font-mono mb-1">Analysis Results</h2>
            <p className="text-sm text-dark-300">
              Found {inv.sites.length} site{inv.sites.length !== 1 ? "s" : ""}, {inv.databases.length} database{inv.databases.length !== 1 ? "s" : ""}, {inv.mail_accounts.length} email account{inv.mail_accounts.length !== 1 ? "s" : ""}
            </p>
          </div>

          {inv.warnings.length > 0 && (
            <div className="px-4 py-3 bg-warn-500/10 border border-warn-500/30 rounded-lg">
              {inv.warnings.map((w, i) => (
                <p key={i} className="text-sm text-warn-400">{w}</p>
              ))}
            </div>
          )}

          {/* Sites */}
          {inv.sites.length > 0 && (
            <div className="bg-dark-800 border border-dark-600 rounded-lg p-5">
              <h3 className="text-sm font-bold text-dark-50 font-mono uppercase tracking-wider mb-3">Sites ({selectedSites.size}/{inv.sites.length})</h3>
              <div className="space-y-2">
                {inv.sites.map((s) => (
                  <label key={s.domain} className="flex items-center gap-3 p-2 rounded hover:bg-dark-700/30 cursor-pointer">
                    <input
                      type="checkbox"
                      checked={selectedSites.has(s.domain)}
                      onChange={(e) => {
                        const next = new Set(selectedSites);
                        e.target.checked ? next.add(s.domain) : next.delete(s.domain);
                        setSelectedSites(next);
                      }}
                      className="w-4 h-4 text-rust-500 border-dark-500 rounded"
                    />
                    <div className="flex-1 min-w-0">
                      <span className="text-sm text-dark-50 font-mono">{s.domain}</span>
                      <span className="text-xs text-dark-400 ml-2">{s.runtime} &middot; {fmtSize(s.size_bytes)} &middot; {s.file_count} files</span>
                    </div>
                  </label>
                ))}
              </div>
            </div>
          )}

          {/* Databases */}
          {inv.databases.length > 0 && (
            <div className="bg-dark-800 border border-dark-600 rounded-lg p-5">
              <h3 className="text-sm font-bold text-dark-50 font-mono uppercase tracking-wider mb-3">Databases ({selectedDbs.size}/{inv.databases.length})</h3>
              <div className="space-y-2">
                {inv.databases.map((d) => (
                  <label key={d.name} className="flex items-center gap-3 p-2 rounded hover:bg-dark-700/30 cursor-pointer">
                    <input
                      type="checkbox"
                      checked={selectedDbs.has(d.name)}
                      onChange={(e) => {
                        const next = new Set(selectedDbs);
                        e.target.checked ? next.add(d.name) : next.delete(d.name);
                        setSelectedDbs(next);
                      }}
                      className="w-4 h-4 text-rust-500 border-dark-500 rounded"
                    />
                    <div className="flex-1">
                      <span className="text-sm text-dark-50 font-mono">{d.name}</span>
                      <span className="text-xs text-dark-400 ml-2">{d.engine} &middot; {fmtSize(d.size_bytes)}</span>
                    </div>
                    {/* A database belongs to a site — the archive does not say
                        which, so this is the only place the answer can come
                        from. Shown only when there is a choice to make. */}
                    {selectedDbs.has(d.name) && selectedSiteDomains.length > 1 && (
                      <select
                        aria-label={`Site for database ${d.name}`}
                        value={siteForDb(d.name)}
                        onClick={(e) => e.preventDefault()}
                        onChange={(e) => setDbSites({ ...dbSites, [d.name]: e.target.value })}
                        className="px-2 py-1 bg-dark-900 border border-dark-500 rounded text-xs text-dark-50 font-mono"
                      >
                        <option value="">Choose a site…</option>
                        {selectedSiteDomains.map((dom) => (
                          <option key={dom} value={dom}>{dom}</option>
                        ))}
                      </select>
                    )}
                  </label>
                ))}
              </div>
              {selectedDbs.size > 0 && selectedSiteDomains.length === 0 && (
                <p className="text-xs text-warn-400 mt-3">
                  A database has to belong to a site. Select at least one site above, or import the
                  site first and run this again — otherwise these databases will be refused.
                </p>
              )}
              {selectedSiteDomains.length === 1 && selectedDbs.size > 0 && (
                <p className="text-xs text-dark-400 mt-3">
                  Databases will be attached to <span className="font-mono text-dark-200">{selectedSiteDomains[0]}</span>.
                </p>
              )}
            </div>
          )}

          {/* Mail (info only) */}
          {inv.mail_accounts.length > 0 && (
            <div className="bg-dark-800 border border-dark-600 rounded-lg p-5">
              <h3 className="text-sm font-bold text-dark-50 font-mono uppercase tracking-wider mb-3">Email Accounts ({inv.mail_accounts.length})</h3>
              <p className="text-xs text-dark-400 mb-2">Email accounts will need to be recreated manually in the Mail section.</p>
              <div className="flex flex-wrap gap-2">
                {inv.mail_accounts.map((m) => (
                  <span key={m.email} className="px-2 py-1 bg-dark-700 rounded text-xs text-dark-200 font-mono">{m.email}</span>
                ))}
              </div>
            </div>
          )}

          <div className="flex gap-3">
            <button onClick={() => setStep(1)} className="px-4 py-2 bg-dark-700 text-dark-200 rounded-lg text-sm hover:bg-dark-600 transition-colors">
              Back
            </button>
            <button
              onClick={handleImport}
              disabled={(selectedSites.size === 0 && selectedDbs.size === 0) || dbsWithoutSite.length > 0}
              title={dbsWithoutSite.length > 0 ? `Choose a site for: ${dbsWithoutSite.join(", ")}` : undefined}
              className="px-5 py-2.5 bg-rust-500 text-dark-950 rounded-lg text-sm font-bold hover:bg-rust-400 transition-colors disabled:opacity-50"
            >
              Import {selectedSites.size + selectedDbs.size} item{selectedSites.size + selectedDbs.size !== 1 ? "s" : ""}
            </button>
          </div>
          {dbsWithoutSite.length > 0 && (
            <p className="text-xs text-warn-400">
              Choose a site for {dbsWithoutSite.join(", ")} before importing.
            </p>
          )}
        </div>
      )}

      {/* Step 3: Progress */}
      {step === 3 && (
        <div className="bg-dark-800 border border-dark-600 rounded-lg p-5 space-y-3">
          <h2 className="text-lg font-bold text-dark-50 font-mono">Importing...</h2>
          <div className="space-y-2">
            {progress.map((p) => (
              <div key={p.step} className="flex items-center gap-3">
                {p.status === "in_progress" && (
                  <div className="w-4 h-4 border-2 border-rust-500 border-t-transparent rounded-full animate-spin shrink-0" />
                )}
                {p.status === "done" && (
                  <svg className="w-4 h-4 text-rust-500 shrink-0" fill="none" viewBox="0 0 24 24" stroke="currentColor" strokeWidth={2.5}><path strokeLinecap="round" strokeLinejoin="round" d="M4.5 12.75l6 6 9-13.5" /></svg>
                )}
                {p.status === "error" && (
                  <svg className="w-4 h-4 text-danger-500 shrink-0" fill="none" viewBox="0 0 24 24" stroke="currentColor" strokeWidth={2.5}><path strokeLinecap="round" strokeLinejoin="round" d="M6 18L18 6M6 6l12 12" /></svg>
                )}
                {p.status === "skipped" && (
                  <svg className="w-4 h-4 text-warn-500 shrink-0" fill="none" viewBox="0 0 24 24" stroke="currentColor" strokeWidth={2}><path strokeLinecap="round" strokeLinejoin="round" d="M12 9v3.75m9-.75a9 9 0 11-18 0 9 9 0 0118 0zm-9 3.75h.008v.008H12v-.008z" /></svg>
                )}
                <span className={`text-sm font-mono ${p.status === "error" ? "text-danger-400" : p.status === "skipped" ? "text-warn-400" : "text-dark-100"}`}>
                  {p.label}
                </span>
                {p.message && <span className="text-xs text-dark-400 ml-auto">{p.message}</span>}
              </div>
            ))}
            {importing && progress.length === 0 && (
              <div className="flex items-center gap-3">
                <div className="w-4 h-4 border-2 border-rust-500 border-t-transparent rounded-full animate-spin" />
                <span className="text-sm text-dark-300">Starting import...</span>
              </div>
            )}
          </div>
        </div>
      )}

      {/* Step 4: Summary */}
      {step === 4 && (
        <div className="space-y-4">
          <div className="bg-dark-800 border border-dark-600 rounded-lg p-5">
            <h2 className="text-lg font-bold text-dark-50 font-mono mb-3">Migration Complete</h2>
            <div className="space-y-2">
              {progress.map((p) => (
                <div key={p.step} className="flex items-center gap-3">
                  {p.status === "done" ? (
                    <svg className="w-4 h-4 text-rust-500 shrink-0" fill="none" viewBox="0 0 24 24" stroke="currentColor" strokeWidth={2.5}><path strokeLinecap="round" strokeLinejoin="round" d="M4.5 12.75l6 6 9-13.5" /></svg>
                  ) : p.status === "error" ? (
                    <svg className="w-4 h-4 text-danger-500 shrink-0" fill="none" viewBox="0 0 24 24" stroke="currentColor" strokeWidth={2.5}><path strokeLinecap="round" strokeLinejoin="round" d="M6 18L18 6M6 6l12 12" /></svg>
                  ) : p.status === "skipped" ? (
                    <svg className="w-4 h-4 text-warn-500 shrink-0" fill="none" viewBox="0 0 24 24" stroke="currentColor" strokeWidth={2}><path strokeLinecap="round" strokeLinejoin="round" d="M12 9v3.75m9-.75a9 9 0 11-18 0 9 9 0 0118 0zm-9 3.75h.008v.008H12v-.008z" /></svg>
                  ) : null}
                  <span className={`text-sm font-mono ${p.status === "error" ? "text-danger-400" : p.status === "skipped" ? "text-warn-400" : "text-dark-100"}`}>
                    {p.label}
                  </span>
                  {p.message && <span className="text-xs text-dark-400 ml-auto">{p.message}</span>}
                </div>
              ))}
            </div>
          </div>

          <div className="flex gap-3">
            <a href="/sites" className="px-4 py-2 bg-rust-500 text-dark-950 rounded-lg text-sm font-bold hover:bg-rust-400 transition-colors">
              View Sites
            </a>
            <a href="/databases" className="px-4 py-2 bg-dark-700 text-dark-200 rounded-lg text-sm hover:bg-dark-600 transition-colors">
              View Databases
            </a>
            <button onClick={() => { setStep(1); setMigration(null); setProgress([]); setError(""); }} className="px-4 py-2 bg-dark-700 text-dark-200 rounded-lg text-sm hover:bg-dark-600 transition-colors">
              Start New Migration
            </button>
          </div>
        </div>
      )}
    </div>
  );
}
