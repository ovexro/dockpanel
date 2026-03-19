import { useState, useEffect, useRef } from "react";
import { api } from "../api";
import { timeAgo } from "../utils/format";

interface LogEntry {
  id: string;
  level: string;
  source: string;
  message: string;
  details: string | null;
  created_at: string;
}

interface LogCounts {
  error: number;
  warning: number;
  info: number;
}

const LEVELS = [
  { label: "All", value: "" },
  { label: "Errors", value: "error" },
  { label: "Warnings", value: "warning" },
  { label: "Info", value: "info" },
];

const TIME_RANGES = [
  { label: "1h", value: "1h" },
  { label: "24h", value: "24h" },
  { label: "7d", value: "7d" },
  { label: "30d", value: "30d" },
];

const SOURCES = [
  "",
  "api",
  "alert_engine",
  "auto_healer",
  "backup_scheduler",
  "metrics_collector",
  "security_scanner",
  "uptime",
];

const LIMIT = 50;

function levelBadge(level: string): { bg: string; text: string } {
  switch (level) {
    case "error":
      return { bg: "bg-danger-500/15", text: "text-danger-400" };
    case "warning":
      return { bg: "bg-warn-500/15", text: "text-warn-400" };
    case "info":
      return { bg: "bg-accent-500/15", text: "text-accent-400" };
    default:
      return { bg: "bg-dark-700", text: "text-dark-100" };
  }
}

export default function SystemLogsContent() {
  const [entries, setEntries] = useState<LogEntry[]>([]);
  const [counts, setCounts] = useState<LogCounts>({ error: 0, warning: 0, info: 0 });
  const [loading, setLoading] = useState(true);
  const [loadingMore, setLoadingMore] = useState(false);
  const [hasMore, setHasMore] = useState(true);
  const [levelFilter, setLevelFilter] = useState("");
  const [sourceFilter, setSourceFilter] = useState("");
  const [timeRange, setTimeRange] = useState("24h");
  const [expandedId, setExpandedId] = useState<string | null>(null);
  const [error, setError] = useState("");
  const refreshTimer = useRef<ReturnType<typeof setInterval>>(undefined);

  const loadEntries = async (offset: number, append: boolean) => {
    try {
      const params = new URLSearchParams({
        limit: String(LIMIT),
        offset: String(offset),
        since: timeRange,
      });
      if (levelFilter) params.set("level", levelFilter);
      if (sourceFilter) params.set("source", sourceFilter);
      const data = await api.get<LogEntry[]>(`/system-logs?${params}`);
      if (append) {
        setEntries((prev) => [...prev, ...data]);
      } else {
        setEntries(data);
      }
      setHasMore(data.length === LIMIT);
    } catch (e) {
      setError(e instanceof Error ? e.message : "Failed to load system logs");
    } finally {
      setLoading(false);
      setLoadingMore(false);
    }
  };

  const loadCounts = async () => {
    try {
      const data = await api.get<LogCounts>(`/system-logs/count?since=24h`);
      setCounts(data);
    } catch {
      // silent
    }
  };

  // Initial load and filter changes
  useEffect(() => {
    setLoading(true);
    setEntries([]);
    setHasMore(true);
    setExpandedId(null);
    loadEntries(0, false);
    loadCounts();
  }, [levelFilter, sourceFilter, timeRange]);

  // Auto-refresh every 30s
  useEffect(() => {
    refreshTimer.current = setInterval(() => {
      loadEntries(0, false);
      loadCounts();
    }, 30000);
    return () => clearInterval(refreshTimer.current);
  }, [levelFilter, sourceFilter, timeRange]);

  const handleLoadMore = () => {
    setLoadingMore(true);
    loadEntries(entries.length, true);
  };

  return (
    <div className="px-6 pb-6 animate-fade-up">
      {/* Summary Cards */}
      <div className="grid grid-cols-3 gap-4 mb-6">
        <div className="bg-dark-800 rounded-lg border border-dark-500 px-4 py-3">
          <div className="text-xs text-dark-300 uppercase font-mono tracking-wider">Errors (24h)</div>
          <div className="text-2xl font-bold font-mono text-danger-400 mt-1">{counts.error}</div>
        </div>
        <div className="bg-dark-800 rounded-lg border border-dark-500 px-4 py-3">
          <div className="text-xs text-dark-300 uppercase font-mono tracking-wider">Warnings (24h)</div>
          <div className="text-2xl font-bold font-mono text-warn-400 mt-1">{counts.warning}</div>
        </div>
        <div className="bg-dark-800 rounded-lg border border-dark-500 px-4 py-3">
          <div className="text-xs text-dark-300 uppercase font-mono tracking-wider">Info (24h)</div>
          <div className="text-2xl font-bold font-mono text-accent-400 mt-1">{counts.info}</div>
        </div>
      </div>

      {/* Filter Bar */}
      <div className="flex flex-wrap items-center gap-3 mb-4">
        {/* Level Filter */}
        <div className="flex items-center gap-1 bg-dark-800 border border-dark-500 rounded-lg p-0.5">
          {LEVELS.map((l) => (
            <button
              key={l.value}
              onClick={() => setLevelFilter(l.value)}
              className={`px-3 py-1.5 text-xs font-mono rounded-md transition-colors ${
                levelFilter === l.value
                  ? "bg-dark-600 text-dark-50"
                  : "text-dark-300 hover:text-dark-100"
              }`}
            >
              {l.label}
            </button>
          ))}
        </div>

        {/* Source Filter */}
        <select
          value={sourceFilter}
          onChange={(e) => setSourceFilter(e.target.value)}
          aria-label="Filter by source"
          className="px-3 py-2 border border-dark-500 rounded-lg text-sm font-mono focus:ring-2 focus:ring-accent-500 focus:border-accent-500"
        >
          <option value="">All Sources</option>
          {SOURCES.filter(Boolean).map((s) => (
            <option key={s} value={s}>
              {s}
            </option>
          ))}
        </select>

        {/* Time Range */}
        <div className="flex items-center gap-1 bg-dark-800 border border-dark-500 rounded-lg p-0.5">
          {TIME_RANGES.map((t) => (
            <button
              key={t.value}
              onClick={() => setTimeRange(t.value)}
              className={`px-3 py-1.5 text-xs font-mono rounded-md transition-colors ${
                timeRange === t.value
                  ? "bg-dark-600 text-dark-50"
                  : "text-dark-300 hover:text-dark-100"
              }`}
            >
              {t.label}
            </button>
          ))}
        </div>
      </div>

      {error && (
        <div className="mb-4 px-4 py-3 rounded-lg text-sm border bg-danger-500/10 text-danger-400 border-danger-500/20">
          {error}
        </div>
      )}

      {/* Log Entries Table */}
      <div className="bg-dark-800 rounded-lg border border-dark-500 overflow-x-auto">
        {loading ? (
          <div className="p-6 space-y-3 animate-pulse">
            {[...Array(5)].map((_, i) => (
              <div key={i} className="h-10 bg-dark-700 rounded w-full" />
            ))}
          </div>
        ) : entries.length === 0 ? (
          <div className="p-12 text-center">
            <svg className="w-12 h-12 text-dark-300 mx-auto mb-4" fill="none" viewBox="0 0 24 24" stroke="currentColor" strokeWidth={1} aria-hidden="true">
              <path strokeLinecap="round" strokeLinejoin="round" d="M9 12.75 11.25 15 15 9.75M21 12a9 9 0 1 1-18 0 9 9 0 0 1 18 0Z" />
            </svg>
            <p className="text-dark-200 font-medium">No system events recorded</p>
            <p className="text-dark-300 text-sm mt-1">Backend errors and warnings will appear here</p>
          </div>
        ) : (
          <>
            {/* Desktop Table */}
            <table className="w-full hidden sm:table">
              <thead>
                <tr className="bg-dark-900 border-b border-dark-500">
                  <th scope="col" className="text-left text-xs font-medium text-dark-200 uppercase font-mono tracking-widest px-5 py-3 w-28">Time</th>
                  <th scope="col" className="text-left text-xs font-medium text-dark-200 uppercase font-mono tracking-widest px-5 py-3 w-24">Level</th>
                  <th scope="col" className="text-left text-xs font-medium text-dark-200 uppercase font-mono tracking-widest px-5 py-3 w-40">Source</th>
                  <th scope="col" className="text-left text-xs font-medium text-dark-200 uppercase font-mono tracking-widest px-5 py-3">Message</th>
                </tr>
              </thead>
              <tbody className="divide-y divide-dark-600">
                {entries.map((entry) => {
                  const badge = levelBadge(entry.level);
                  const isExpanded = expandedId === entry.id;
                  return (
                    <tr
                      key={entry.id}
                      onClick={() => setExpandedId(isExpanded ? null : entry.id)}
                      className={`hover:bg-dark-700/30 transition-colors cursor-pointer ${isExpanded ? "bg-dark-700/20" : ""}`}
                    >
                      <td className="px-5 py-3 text-sm text-dark-200 whitespace-nowrap font-mono align-top">{timeAgo(entry.created_at)}</td>
                      <td className="px-5 py-3 align-top">
                        <span className={`inline-flex px-2.5 py-0.5 rounded-full text-xs font-medium font-mono ${badge.bg} ${badge.text}`}>{entry.level}</span>
                      </td>
                      <td className="px-5 py-3 text-sm text-dark-100 font-mono align-top">{entry.source}</td>
                      <td className="px-5 py-3 align-top">
                        <div className="text-sm text-dark-50 font-mono">{entry.message}</div>
                        {isExpanded && entry.details && (
                          <pre className="mt-2 p-3 bg-dark-900 rounded-lg text-xs text-dark-200 font-mono whitespace-pre-wrap break-words max-h-48 overflow-auto border border-dark-600">
                            {entry.details}
                          </pre>
                        )}
                        {isExpanded && !entry.details && (
                          <div className="mt-2 text-xs text-dark-300 font-mono">No additional details</div>
                        )}
                      </td>
                    </tr>
                  );
                })}
              </tbody>
            </table>

            {/* Mobile Cards */}
            <div className="sm:hidden divide-y divide-dark-600">
              {entries.map((entry) => {
                const badge = levelBadge(entry.level);
                const isExpanded = expandedId === entry.id;
                return (
                  <div
                    key={entry.id}
                    onClick={() => setExpandedId(isExpanded ? null : entry.id)}
                    className={`px-4 py-3 space-y-1.5 cursor-pointer ${isExpanded ? "bg-dark-700/20" : ""}`}
                  >
                    <div className="flex items-center justify-between">
                      <span className={`inline-flex px-2 py-0.5 rounded-full text-xs font-medium font-mono ${badge.bg} ${badge.text}`}>{entry.level}</span>
                      <span className="text-xs text-dark-300 font-mono">{timeAgo(entry.created_at)}</span>
                    </div>
                    <div className="text-xs text-dark-100 font-mono">{entry.source}</div>
                    <div className="text-xs text-dark-50 font-mono">{entry.message}</div>
                    {isExpanded && entry.details && (
                      <pre className="mt-1 p-2 bg-dark-900 rounded text-[10px] text-dark-200 font-mono whitespace-pre-wrap break-words max-h-32 overflow-auto border border-dark-600">
                        {entry.details}
                      </pre>
                    )}
                  </div>
                );
              })}
            </div>

            {hasMore && (
              <div className="px-5 py-4 border-t border-dark-600 text-center">
                <button
                  onClick={handleLoadMore}
                  disabled={loadingMore}
                  className="px-4 py-2 bg-dark-700 text-dark-100 rounded-lg text-sm font-medium hover:bg-dark-600 disabled:opacity-50"
                >
                  {loadingMore ? "Loading..." : "Load More"}
                </button>
              </div>
            )}
          </>
        )}
      </div>
    </div>
  );
}
