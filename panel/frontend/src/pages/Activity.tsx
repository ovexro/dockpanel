import { useState, useEffect, useRef } from "react";
import { Navigate } from "react-router-dom";
import { useAuth } from "../context/AuthContext";
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
    return { bg: "bg-emerald-500/15", text: "text-emerald-400" };
  }
  if (lower.includes("delete") || lower.includes("remove")) {
    return { bg: "bg-red-500/15", text: "text-red-400" };
  }
  if (lower.includes("update") || lower.includes("edit") || lower.includes("change")) {
    return { bg: "bg-blue-500/15", text: "text-blue-400" };
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

export default function Activity() {
  const { user } = useAuth();
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

  if (user?.role !== "admin") return <Navigate to="/" replace />;

  const handleLoadMore = () => {
    setLoadingMore(true);
    loadEntries(entries.length, true);
  };

  return (
    <div className="p-6 lg:p-8">
      <div className="flex items-center justify-between mb-6">
        <div>
          <h1 className="text-2xl font-bold text-dark-50">Activity Log</h1>
          <p className="text-sm text-dark-200 mt-1">Track all admin actions</p>
        </div>
        <select
          value={filter}
          onChange={(e) => setFilter(e.target.value)}
          aria-label="Filter by activity type"
          className="px-3 py-2 border border-dark-500 rounded-lg text-sm focus:ring-2 focus:ring-rust-500 focus:border-rust-500"
        >
          {FILTERS.map((f) => (
            <option key={f.value} value={f.value}>
              {f.label}
            </option>
          ))}
        </select>
      </div>

      {error && (
        <div className="mb-4 px-4 py-3 rounded-lg text-sm border bg-red-500/10 text-red-400 border-red-500/20">
          {error}
        </div>
      )}

      <div className="bg-dark-800 rounded-xl border border-dark-500 overflow-hidden">
        {loading ? (
          <div className="p-8 text-center text-dark-300">Loading...</div>
        ) : entries.length === 0 ? (
          <div className="p-8 text-center text-dark-300">No activity found</div>
        ) : (
          <>
            <table className="w-full">
              <thead>
                <tr className="bg-dark-900 border-b border-dark-500">
                  <th className="text-left text-xs font-medium text-dark-200 uppercase px-5 py-3 w-28">
                    Time
                  </th>
                  <th className="text-left text-xs font-medium text-dark-200 uppercase px-5 py-3">
                    User
                  </th>
                  <th className="text-left text-xs font-medium text-dark-200 uppercase px-5 py-3 w-36">
                    Action
                  </th>
                  <th className="text-left text-xs font-medium text-dark-200 uppercase px-5 py-3">
                    Target
                  </th>
                  <th className="text-left text-xs font-medium text-dark-200 uppercase px-5 py-3">
                    Details
                  </th>
                </tr>
              </thead>
              <tbody className="divide-y divide-dark-600">
                {entries.map((entry) => {
                  const badge = actionBadge(entry.action);
                  return (
                    <tr key={entry.id} className="hover:bg-dark-800">
                      <td className="px-5 py-3 text-sm text-dark-200 whitespace-nowrap">
                        {timeAgo(entry.created_at)}
                      </td>
                      <td className="px-5 py-3">
                        <div className="flex items-center gap-2">
                          <div className="w-6 h-6 rounded-full bg-rust-500/15 text-rust-500 flex items-center justify-center text-xs font-medium shrink-0">
                            {entry.user_email?.[0]?.toUpperCase() || "?"}
                          </div>
                          <span className="text-sm text-dark-50 truncate max-w-[180px]">
                            {entry.user_email}
                          </span>
                        </div>
                      </td>
                      <td className="px-5 py-3">
                        <span
                          className={`inline-flex px-2.5 py-0.5 rounded-full text-xs font-medium ${badge.bg} ${badge.text}`}
                        >
                          {entry.action}
                        </span>
                      </td>
                      <td className="px-5 py-3 text-sm text-dark-50">
                        {entry.target_type && (
                          <span className="text-dark-300 text-xs mr-1">{entry.target_type}:</span>
                        )}
                        {entry.target_name || <span className="text-dark-400">-</span>}
                      </td>
                      <td className="px-5 py-3 text-sm text-dark-200 truncate max-w-[250px]">
                        {entry.details || <span className="text-dark-400">-</span>}
                      </td>
                    </tr>
                  );
                })}
              </tbody>
            </table>

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
