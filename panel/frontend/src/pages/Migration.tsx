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

const fmtSize = (b: number) => {
  if (b > 1e9) return `${(b / 1e9).toFixed(1)} GB`;
  if (b > 1e6) return `${(b / 1e6).toFixed(1)} MB`;
  if (b > 1e3) return `${(b / 1e3).toFixed(0)} KB`;
  return `${b} B`;
};

export default function Migration() {
  const { user } = useAuth();
  if (!user || user.role !== "admin") return <Navigate to="/" replace />;
  const [step, setStep] = useState<1 | 2 | 3 | 4>(1);
  const [source, setSource] = useState("cpanel");
  const [backupPath, setBackupPath] = useState("");
  const [analyzing, setAnalyzing] = useState(false);
  const [error, setError] = useState("");
  const [migration, setMigration] = useState<MigrationRecord | null>(null);
  const [selectedSites, setSelectedSites] = useState<Set<string>>(new Set());
  const [selectedDbs, setSelectedDbs] = useState<Set<string>>(new Set());
  const [progress, setProgress] = useState<ProgressStep[]>([]);
  const [importing, setImporting] = useState(false);
  const eventSourceRef = useRef<EventSource | null>(null);

  // Step 1: Analyze
  const handleAnalyze = async () => {
    if (!backupPath.trim()) return;
    setError("");
    setAnalyzing(true);
    try {
      const res = await api.post<MigrationRecord>("/migration/analyze", {
        path: backupPath.trim(),
        source,
      });
      setMigration(res);
      // Select all items by default
      if (res.inventory) {
        setSelectedSites(new Set(res.inventory.sites.map((s) => s.domain)));
        setSelectedDbs(new Set(res.inventory.databases.map((d) => d.name)));
      }
      setStep(2);
    } catch (e) {
      setError(e instanceof ApiError ? e.message : "Analysis failed");
    } finally {
      setAnalyzing(false);
    }
  };

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
          .map((d) => ({ name: d.name, file: d.file, engine: d.engine })),
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

  const inv = migration?.inventory;

  return (
    <div className="p-6 space-y-6">
      <div>
        <h1 className="text-2xl font-bold text-dark-50 font-mono">Migration Wizard</h1>
        <p className="text-sm text-dark-300 mt-1">Import sites, databases, and email from cPanel, Plesk, or HestiaCP</p>
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
              placeholder="/home/user/backup-1.2.2026_12-00-00_username.tar.gz"
              className="w-full px-3 py-2 bg-dark-900 border border-dark-600 rounded-lg text-dark-50 text-sm focus:border-rust-500 focus:outline-none font-mono"
            />
            <p className="text-xs text-dark-400 mt-1">Upload the backup to your server via SFTP first, then enter the full path here.</p>
          </div>

          <button
            onClick={handleAnalyze}
            disabled={!backupPath.trim() || analyzing}
            className="px-5 py-2.5 bg-rust-500 text-dark-950 rounded-lg text-sm font-bold hover:bg-rust-400 transition-colors disabled:opacity-50"
          >
            {analyzing ? "Analyzing..." : "Analyze Backup"}
          </button>
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
                  </label>
                ))}
              </div>
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
              disabled={selectedSites.size === 0 && selectedDbs.size === 0}
              className="px-5 py-2.5 bg-rust-500 text-dark-950 rounded-lg text-sm font-bold hover:bg-rust-400 transition-colors disabled:opacity-50"
            >
              Import {selectedSites.size + selectedDbs.size} item{selectedSites.size + selectedDbs.size !== 1 ? "s" : ""}
            </button>
          </div>
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
