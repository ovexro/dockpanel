import { useState, useEffect, useRef } from "react";
import { api } from "../api";
import { timeAgo } from "../utils/format";

interface ActivityEntry {
  id: string;
  user_email: string;
  action: string;
  target_type: string | null;
  target_name: string | null;
  details: string | null;
  ip_address: string | null;
  created_at: string;
}

function actionBadge(action: string): { bg: string; text: string } {
  const lower = action.toLowerCase();
  if (lower.includes("create") || lower.includes("deploy")) {
    return { bg: "bg-rust-500/15", text: "text-rust-400" };
  }
  if (lower.includes("delete") || lower.includes("remove")) {
    return { bg: "bg-danger-500/15", text: "text-danger-400" };
  }
  if (lower.includes("update") || lower.includes("edit") || lower.includes("change")) {
    return { bg: "bg-accent-500/15", text: "text-accent-400" };
  }
  return { bg: "bg-dark-700", text: "text-dark-100" };
}

const FILTERS = [
  { label: "All", value: "" },
  { label: "Sites", value: "site" },
  { label: "Users", value: "user" },
  { label: "Apps", value: "app" },
  { label: "Security", value: "security" },
  { label: "Backups", value: "backup" },
];

const LIMIT = 50;

export default function AuditLogContent() {
  const [entries, setEntries] = useState<ActivityEntry[]>([]);
  const [loading, setLoading] = useState(true);
  const [loadingMore, setLoadingMore] = useState(false);
  const [hasMore, setHasMore] = useState(true);
  const [filter, setFilter] = useState("");
  const [error, setError] = useState("");
  const refreshTimer = useRef<ReturnType<typeof setInterval>>(undefined);

  const loadEntries = async (offset: number, append: boolean) => {
    try {
      const params = new URLSearchParams({
        limit: String(LIMIT),
        offset: String(offset),
      });
      if (filter) params.set("action", filter);
      const data = await api.get<ActivityEntry[]>(`/activity?${params}`);
      if (append) {
        setEntries((prev) => [...prev, ...data]);
      } else {
        setEntries(data);
      }
      setHasMore(data.length === LIMIT);
    } catch (e) {
      setError(e instanceof Error ? e.message : "Failed to load activity");
    } finally {
      setLoading(false);
      setLoadingMore(false);
    }
  };

  // Initial load and filter changes
  useEffect(() => {
    setLoading(true);
    setEntries([]);
    setHasMore(true);
    loadEntries(0, false);
  }, [filter]);

  // Auto-refresh first page every 10s
  useEffect(() => {
    refreshTimer.current = setInterval(() => {
      loadEntries(0, false);
    }, 10000);
    return () => clearInterval(refreshTimer.current);
  }, [filter]);

  const handleLoadMore = () => {
    setLoadingMore(true);
    loadEntries(entries.length, true);
  };

  return (
    <div className="px-6 pb-6 animate-fade-up">
      <div className="flex items-center justify-end mb-4">
        <select
          value={filter}
          onChange={(e) => setFilter(e.target.value)}
          aria-label="Filter by activity type"
          className="px-3 py-2 border border-dark-500 rounded-lg text-sm focus:ring-2 focus:ring-accent-500 focus:border-accent-500"
        >
          {FILTERS.map((f) => (
            <option key={f.value} value={f.value}>
              {f.label}
            </option>
          ))}
        </select>
      </div>

      {error && (
        <div className="mb-4 px-4 py-3 rounded-lg text-sm border bg-danger-500/10 text-danger-400 border-danger-500/20">
          {error}
        </div>
      )}

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
              <path strokeLinecap="round" strokeLinejoin="round" d="M12 6v6h4.5m4.5 0a9 9 0 1 1-18 0 9 9 0 0 1 18 0Z" />
            </svg>
            <p className="text-dark-200 font-medium">No activity found</p>
            <p className="text-dark-300 text-sm mt-1">Admin actions will appear here</p>
          </div>
        ) : (
          <>
            {/* Desktop Table */}
            <table className="w-full hidden sm:table">
              <thead>
                <tr className="bg-dark-900 border-b border-dark-500">
                  <th scope="col" className="text-left text-xs font-medium text-dark-200 uppercase font-mono tracking-widest px-5 py-3 w-28">Time</th>
                  <th scope="col" className="text-left text-xs font-medium text-dark-200 uppercase font-mono tracking-widest px-5 py-3">User</th>
                  <th scope="col" className="text-left text-xs font-medium text-dark-200 uppercase font-mono tracking-widest px-5 py-3 w-36">Action</th>
                  <th scope="col" className="text-left text-xs font-medium text-dark-200 uppercase font-mono tracking-widest px-5 py-3 hidden md:table-cell">Target</th>
                  <th scope="col" className="text-left text-xs font-medium text-dark-200 uppercase font-mono tracking-widest px-5 py-3 hidden lg:table-cell">Details</th>
                </tr>
              </thead>
              <tbody className="divide-y divide-dark-600">
                {entries.map((entry) => {
                  const badge = actionBadge(entry.action);
                  return (
                    <tr key={entry.id} className="hover:bg-dark-700/30 transition-colors">
                      <td className="px-5 py-3 text-sm text-dark-200 whitespace-nowrap font-mono">{timeAgo(entry.created_at)}</td>
                      <td className="px-5 py-3">
                        <div className="flex items-center gap-2">
                          <div className="w-6 h-6 rounded-full bg-rust-500/15 text-rust-500 flex items-center justify-center text-xs font-medium shrink-0">{entry.user_email?.[0]?.toUpperCase() || "?"}</div>
                          <span className="text-sm text-dark-50 truncate max-w-[180px] font-mono">{entry.user_email}</span>
                        </div>
                      </td>
                      <td className="px-5 py-3">
                        <span className={`inline-flex px-2.5 py-0.5 rounded-full text-xs font-medium font-mono ${badge.bg} ${badge.text}`}>{entry.action}</span>
                      </td>
                      <td className="px-5 py-3 text-sm text-dark-50 hidden md:table-cell font-mono">
                        {entry.target_type && <span className="text-dark-300 text-xs mr-1">{entry.target_type}:</span>}
                        {entry.target_name || <span className="text-dark-300">-</span>}
                      </td>
                      <td className="px-5 py-3 text-sm text-dark-200 truncate max-w-[250px] hidden lg:table-cell font-mono">
                        {entry.details || <span className="text-dark-300">-</span>}
                      </td>
                    </tr>
                  );
                })}
              </tbody>
            </table>

            {/* Mobile Cards */}
            <div className="sm:hidden divide-y divide-dark-600">
              {entries.map((entry) => {
                const badge = actionBadge(entry.action);
                return (
                  <div key={entry.id} className="px-4 py-3 space-y-1.5">
                    <div className="flex items-center justify-between">
                      <span className={`inline-flex px-2 py-0.5 rounded-full text-xs font-medium font-mono ${badge.bg} ${badge.text}`}>{entry.action}</span>
                      <span className="text-xs text-dark-300 font-mono">{timeAgo(entry.created_at)}</span>
                    </div>
                    <div className="flex items-center gap-2">
                      <div className="w-5 h-5 rounded-full bg-rust-500/15 text-rust-500 flex items-center justify-center text-[10px] font-medium shrink-0">{entry.user_email?.[0]?.toUpperCase() || "?"}</div>
                      <span className="text-xs text-dark-200 font-mono truncate">{entry.user_email}</span>
                    </div>
                    {(entry.target_name || entry.details) && (
                      <div className="text-xs text-dark-300 font-mono truncate">
                        {entry.target_type && <span className="text-dark-300">{entry.target_type}: </span>}
                        {entry.target_name}{entry.details && <span className="text-dark-300 ml-2">{entry.details}</span>}
                      </div>
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
