import { useState, useEffect } from "react";
import { Link } from "react-router-dom";
import { api } from "../api";
import { timeAgo } from "../utils/format";

interface Notification {
  id: string;
  title: string;
  message: string;
  severity: string;
  category: string;
  link: string | null;
  read_at: string | null;
  created_at: string;
}

function severityColor(s: string) {
  switch (s) {
    case "critical":
      return "bg-danger-500/15 text-danger-400 border-danger-500/30";
    case "warning":
      return "bg-warn-500/15 text-warn-400 border-warn-500/30";
    case "info":
      return "bg-accent-500/15 text-accent-400 border-accent-500/30";
    default:
      return "bg-dark-700/50 text-dark-300 border-dark-600";
  }
}

function severityDot(s: string) {
  switch (s) {
    case "critical":
      return "bg-danger-500";
    case "warning":
      return "bg-warn-500";
    case "info":
      return "bg-accent-500";
    default:
      return "bg-dark-400";
  }
}

export default function Notifications() {
  const [notifs, setNotifs] = useState<Notification[]>([]);
  const [loading, setLoading] = useState(true);
  const [error, setError] = useState("");

  const load = () => {
    api
      .get<Notification[]>("/notifications")
      .then((data) => {
        setNotifs(data);
        setError("");
      })
      .catch((e) => setError(e.message))
      .finally(() => setLoading(false));
  };

  useEffect(() => {
    load();
  }, []);

  const markRead = async (id: string) => {
    await api.post(`/notifications/${id}/read`);
    setNotifs((prev) =>
      prev.map((n) =>
        n.id === id ? { ...n, read_at: new Date().toISOString() } : n
      )
    );
  };

  const markAllRead = async () => {
    await api.post("/notifications/read-all");
    setNotifs((prev) =>
      prev.map((n) => ({ ...n, read_at: n.read_at || new Date().toISOString() }))
    );
  };

  const unreadCount = notifs.filter((n) => !n.read_at).length;

  if (loading) {
    return (
      <div className="p-6 md:p-8">
        <div className="animate-pulse space-y-4">
          <div className="h-8 w-48 bg-dark-700 rounded" />
          <div className="h-20 bg-dark-700 rounded-lg" />
          <div className="h-20 bg-dark-700 rounded-lg" />
          <div className="h-20 bg-dark-700 rounded-lg" />
        </div>
      </div>
    );
  }

  return (
    <div className="p-6 md:p-8">
      {/* Header */}
      <div className="flex items-center justify-between mb-6">
        <div>
          <h1 className="text-2xl font-bold text-dark-50">Notifications</h1>
          <p className="text-sm text-dark-400 mt-1">
            {unreadCount > 0
              ? `${unreadCount} unread notification${unreadCount !== 1 ? "s" : ""}`
              : "All caught up"}
          </p>
          <Link to="/settings" className="text-xs text-accent-400 hover:text-accent-300 mt-1 inline-block">
            Configure alert channels &rarr;
          </Link>
        </div>
        {unreadCount > 0 && (
          <button
            onClick={markAllRead}
            className="px-3 py-1.5 text-xs font-medium rounded-lg transition-colors bg-dark-700 text-dark-200 hover:bg-dark-600 hover:text-dark-50"
          >
            Mark all read
          </button>
        )}
      </div>

      {error && (
        <div className="mb-4 p-3 rounded-lg bg-danger-500/10 text-danger-400 text-sm border border-danger-500/20">
          {error}
        </div>
      )}

      {notifs.length === 0 ? (
        <div className="text-center py-20">
          <svg
            className="w-12 h-12 mx-auto text-dark-500 mb-4"
            fill="none"
            viewBox="0 0 24 24"
            stroke="currentColor"
            strokeWidth={1}
          >
            <path
              strokeLinecap="round"
              strokeLinejoin="round"
              d="M14.857 17.082a23.848 23.848 0 005.454-1.31A8.967 8.967 0 0118 9.75v-.7V9A6 6 0 006 9v.75a8.967 8.967 0 01-2.312 6.022c1.733.64 3.56 1.085 5.455 1.31m5.714 0a24.255 24.255 0 01-5.714 0m5.714 0a3 3 0 11-5.714 0"
            />
          </svg>
          <p className="text-dark-400 text-sm">No notifications yet</p>
          <p className="text-dark-500 text-xs mt-1">
            Alerts and system events will appear here
          </p>
        </div>
      ) : (
        <div className="space-y-2">
          {notifs.map((n) => (
            <div
              key={n.id}
              className={`group relative rounded-lg border p-4 transition-all ${
                n.read_at
                  ? "bg-dark-800/30 border-dark-700/50 opacity-60"
                  : "bg-dark-800/60 border-dark-600/50"
              }`}
            >
              <div className="flex items-start gap-3">
                {/* Severity dot */}
                <div
                  className={`w-2 h-2 rounded-full mt-2 shrink-0 ${severityDot(
                    n.severity
                  )} ${!n.read_at ? "animate-pulse" : ""}`}
                />

                {/* Content */}
                <div className="flex-1 min-w-0">
                  <div className="flex items-center gap-2 mb-1">
                    <h3
                      className={`text-sm font-medium truncate ${
                        n.read_at ? "text-dark-300" : "text-dark-50"
                      }`}
                    >
                      {n.title}
                    </h3>
                    <span
                      className={`px-1.5 py-0.5 text-[10px] font-medium rounded border ${severityColor(
                        n.severity
                      )}`}
                    >
                      {n.severity}
                    </span>
                    <span className="px-1.5 py-0.5 text-[10px] font-medium rounded bg-dark-700/50 text-dark-400 border border-dark-600/50">
                      {n.category}
                    </span>
                  </div>
                  <p
                    className={`text-xs leading-relaxed ${
                      n.read_at ? "text-dark-500" : "text-dark-300"
                    }`}
                  >
                    {n.message}
                  </p>
                  <p className="text-[10px] text-dark-500 mt-1.5">
                    {timeAgo(n.created_at)}
                  </p>
                </div>

                {/* Mark read button */}
                {!n.read_at && (
                  <button
                    onClick={() => markRead(n.id)}
                    className="p-1.5 text-dark-500 hover:text-dark-200 rounded-lg transition-colors opacity-0 group-hover:opacity-100"
                    title="Mark as read"
                  >
                    <svg
                      className="w-4 h-4"
                      fill="none"
                      viewBox="0 0 24 24"
                      stroke="currentColor"
                      strokeWidth={2}
                    >
                      <path
                        strokeLinecap="round"
                        strokeLinejoin="round"
                        d="M4.5 12.75l6 6 9-13.5"
                      />
                    </svg>
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
