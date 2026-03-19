import { useState } from "react";
import { api } from "../api";

interface DiagnosticFinding {
  id: string;
  category: string;
  severity: string;
  title: string;
  description: string;
  fix_available: boolean;
  fix_id: string | null;
}

interface DiagnosticReport {
  findings: DiagnosticFinding[];
  summary: {
    critical: number;
    warning: number;
    info: number;
    total: number;
  };
}

const severityColors: Record<string, { bg: string; text: string; dot: string }> = {
  critical: { bg: "bg-danger-500/10", text: "text-danger-400", dot: "bg-danger-500" },
  warning: { bg: "bg-warn-500/10", text: "text-warn-400", dot: "bg-warn-500" },
  info: { bg: "bg-accent-500/10", text: "text-accent-400", dot: "bg-accent-500" },
};

const categoryLabels: Record<string, string> = {
  nginx: "Nginx",
  resources: "Resources",
  services: "Services",
  ssl: "SSL Certificates",
  logs: "Log Analysis",
  security: "Security",
};

export default function Diagnostics() {
  const [report, setReport] = useState<DiagnosticReport | null>(null);
  const [loading, setLoading] = useState(false);
  const [fixing, setFixing] = useState<string | null>(null);
  const [fixResults, setFixResults] = useState<Record<string, { ok: boolean; msg: string }>>({});
  const [filter, setFilter] = useState<string>("all");

  const runDiagnostics = async () => {
    setLoading(true);
    setFixResults({});
    try {
      const data = await api.get<DiagnosticReport>("/agent/diagnostics");
      setReport(data);
    } catch {
      setReport(null);
    } finally {
      setLoading(false);
    }
  };

  const applyFix = async (fixId: string, findingId: string) => {
    setFixing(findingId);
    try {
      const res = await api.post<{ success: boolean; message: string }>("/agent/diagnostics/fix", { fix_id: fixId });
      setFixResults((prev) => ({ ...prev, [findingId]: { ok: true, msg: res.message } }));
    } catch (e: unknown) {
      const msg = e instanceof Error ? e.message : "Fix failed";
      setFixResults((prev) => ({ ...prev, [findingId]: { ok: false, msg } }));
    } finally {
      setFixing(null);
    }
  };

  const filtered = report?.findings.filter((f) => filter === "all" || f.severity === filter) ?? [];

  const grouped = filtered.reduce<Record<string, DiagnosticFinding[]>>((acc, f) => {
    (acc[f.category] ??= []).push(f);
    return acc;
  }, {});

  return (
    <div>
      <div className="flex flex-col sm:flex-row sm:items-center justify-between gap-3 mb-6">
        <p className="text-dark-300 text-sm font-mono">
          Scan your server for misconfigurations, resource issues, and security concerns.
        </p>
        <button
          onClick={runDiagnostics}
          disabled={loading}
          className="px-4 py-2 bg-rust-500 hover:bg-rust-600 text-white rounded-lg transition-colors disabled:opacity-50 flex items-center gap-2"
        >
          {loading ? (
            <>
              <svg className="w-4 h-4 animate-spin" viewBox="0 0 24 24" fill="none">
                <circle className="opacity-25" cx="12" cy="12" r="10" stroke="currentColor" strokeWidth="4" />
                <path className="opacity-75" fill="currentColor" d="M4 12a8 8 0 018-8V0C5.373 0 0 5.373 0 12h4z" />
              </svg>
              Scanning...
            </>
          ) : (
            <>
              <svg className="w-4 h-4" fill="none" viewBox="0 0 24 24" stroke="currentColor" strokeWidth={2}>
                <path strokeLinecap="round" strokeLinejoin="round" d="M21 21l-5.197-5.197m0 0A7.5 7.5 0 105.196 5.196a7.5 7.5 0 0010.607 10.607z" />
              </svg>
              Run Scan
            </>
          )}
        </button>
      </div>

      {/* Summary Cards */}
      {report && (
        <div className="grid grid-cols-2 sm:grid-cols-4 gap-4 mb-6">
          <button
            onClick={() => setFilter("all")}
            className={`p-4 rounded-lg border transition-colors ${filter === "all" ? "border-rust-500 bg-dark-800" : "border-dark-500 bg-dark-900 hover:bg-dark-800"}`}
          >
            <div className="text-2xl font-bold text-dark-50">{report.summary.total}</div>
            <div className="text-sm text-dark-300">Total</div>
          </button>
          <button
            onClick={() => setFilter("critical")}
            className={`p-4 rounded-lg border transition-colors ${filter === "critical" ? "border-danger-500 bg-dark-800" : "border-dark-500 bg-dark-900 hover:bg-dark-800"}`}
          >
            <div className="text-2xl font-bold text-danger-400">{report.summary.critical}</div>
            <div className="text-sm text-dark-300">Critical</div>
          </button>
          <button
            onClick={() => setFilter("warning")}
            className={`p-4 rounded-lg border transition-colors ${filter === "warning" ? "border-warn-500 bg-dark-800" : "border-dark-500 bg-dark-900 hover:bg-dark-800"}`}
          >
            <div className="text-2xl font-bold text-warn-400">{report.summary.warning}</div>
            <div className="text-sm text-dark-300">Warnings</div>
          </button>
          <button
            onClick={() => setFilter("info")}
            className={`p-4 rounded-lg border transition-colors ${filter === "info" ? "border-accent-500 bg-dark-800" : "border-dark-500 bg-dark-900 hover:bg-dark-800"}`}
          >
            <div className="text-2xl font-bold text-accent-400">{report.summary.info}</div>
            <div className="text-sm text-dark-300">Info</div>
          </button>
        </div>
      )}

      {/* No scan yet */}
      {!report && !loading && (
        <div className="text-center py-20 text-dark-300">
          <svg className="w-16 h-16 mx-auto mb-4 opacity-30" fill="none" viewBox="0 0 24 24" stroke="currentColor" strokeWidth={1}>
            <path strokeLinecap="round" strokeLinejoin="round" d="M9.75 3.104v5.714a2.25 2.25 0 0 1-.659 1.591L5 14.5M9.75 3.104c-.251.023-.501.05-.75.082m.75-.082a24.301 24.301 0 0 1 4.5 0m0 0v5.714c0 .597.237 1.17.659 1.591L19.8 15.3M14.25 3.104c.251.023.501.05.75.082M19.8 15.3l-1.57.393A9.065 9.065 0 0 1 12 15a9.065 9.065 0 0 0-6.23.693L5 14.5m14.8.8 1.402 1.402c1.232 1.232.65 3.318-1.067 3.611A48.309 48.309 0 0 1 12 21a48.25 48.25 0 0 1-8.134-.888c-1.716-.293-2.3-2.379-1.067-3.61L5 14.5" />
          </svg>
          <p className="text-lg">Click "Run Scan" to check your server health</p>
        </div>
      )}

      {/* All clear */}
      {report && report.summary.total === 0 && (
        <div className="text-center py-16 bg-dark-900 rounded-lg border border-dark-500">
          <svg className="w-16 h-16 mx-auto mb-4 text-rust-400" fill="none" viewBox="0 0 24 24" stroke="currentColor" strokeWidth={1.5}>
            <path strokeLinecap="round" strokeLinejoin="round" d="M9 12.75 11.25 15 15 9.75M21 12a9 9 0 1 1-18 0 9 9 0 0 1 18 0Z" />
          </svg>
          <p className="text-lg text-rust-400 font-medium">All checks passed</p>
          <p className="text-dark-300 mt-1">No issues detected on your server.</p>
        </div>
      )}

      {/* Findings grouped by category */}
      {Object.entries(grouped).map(([category, items]) => (
        <div key={category} className="mb-6">
          <h2 className="text-sm font-semibold text-dark-300 uppercase tracking-wider mb-3 font-mono">
            {categoryLabels[category] ?? category}
          </h2>
          <div className="space-y-2">
            {items.map((finding) => {
              const colors = severityColors[finding.severity] ?? severityColors.info;
              const result = fixResults[finding.id];

              return (
                <div
                  key={finding.id}
                  className={`p-4 rounded-lg border border-dark-500 bg-dark-900 ${result?.ok ? "opacity-50" : ""}`}
                >
                  <div className="flex items-start justify-between gap-4">
                    <div className="flex items-start gap-3 min-w-0">
                      <span className={`mt-1.5 w-2 h-2 rounded-full shrink-0 ${colors.dot}`} />
                      <div className="min-w-0">
                        <div className="flex items-center gap-2 flex-wrap">
                          <span className="font-medium text-dark-50">{finding.title}</span>
                          <span className={`text-xs px-1.5 py-0.5 rounded ${colors.bg} ${colors.text}`}>
                            {finding.severity}
                          </span>
                        </div>
                        <p className="text-sm text-dark-300 mt-1 font-mono">{finding.description}</p>
                        {result && (
                          <p className={`text-sm mt-2 ${result.ok ? "text-rust-400" : "text-danger-400"}`}>
                            {result.ok ? "\u2713" : "\u2717"} {result.msg}
                          </p>
                        )}
                      </div>
                    </div>
                    {finding.fix_available && finding.fix_id && !result?.ok && (
                      <button
                        onClick={() => applyFix(finding.fix_id!, finding.id)}
                        disabled={fixing === finding.id}
                        className="shrink-0 px-3 py-1.5 text-sm bg-rust-500 hover:bg-rust-600 text-white rounded transition-colors disabled:opacity-50"
                      >
                        {fixing === finding.id ? "Fixing..." : "Fix"}
                      </button>
                    )}
                  </div>
                </div>
              );
            })}
          </div>
        </div>
      ))}
    </div>
  );
}
