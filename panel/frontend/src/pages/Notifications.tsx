import { useState, useEffect, useCallback, useRef } from "react";
import { Link } from "react-router-dom";
import { useAuth } from "../context/AuthContext";
import { api } from "../api";
import { timeAgo, formatDate } from "../utils/format";
import { NOTIF_CHANGE_EVENT } from "../hooks/useLayoutState";

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

interface CategoryCount {
  category: string;
  total: number;
  unread: number;
}

interface Summary {
  total: number;
  unread: number;
  categories: CategoryCount[];
}

const PAGE_SIZE = 50;

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

/**
 * Is this link one we are willing to follow?
 *
 * `panel_notifications.link` is bare TEXT with no CHECK constraint, and
 * `notify_panel` binds whatever it is handed, so the render is where the shape
 * gets decided. In-app absolute paths only: a leading `/` and not `//`, which
 * is protocol-relative and leaves the panel. Anything else renders as plain
 * text rather than as a link that goes somewhere unexpected.
 */
function isSafeLink(link: string | null): link is string {
  return !!link && link.startsWith("/") && !link.startsWith("//");
}

export default function Notifications() {
  const { user } = useAuth();
  const [notifs, setNotifs] = useState<Notification[]>([]);
  const [summary, setSummary] = useState<Summary | null>(null);
  const [loading, setLoading] = useState(true);
  const [loadingMore, setLoadingMore] = useState(false);
  const [exhausted, setExhausted] = useState(false);
  const [error, setError] = useState("");
  const [unreadOnly, setUnreadOnly] = useState(false);
  const [category, setCategory] = useState<string>("");

  // The filters are read inside the SSE handler, which is installed once. A ref
  // keeps that handler looking at the CURRENT filter instead of the one that
  // was in scope when it was installed — the stale-closure trap.
  const filters = useRef({ unreadOnly, category });
  filters.current = { unreadOnly, category };

  const query = useCallback(
    (before?: string) => {
      const p = new URLSearchParams({ limit: String(PAGE_SIZE) });
      if (before) p.set("before", before);
      if (category) p.set("category", category);
      if (unreadOnly) p.set("unread", "true");
      return `/notifications?${p.toString()}`;
    },
    [category, unreadOnly]
  );

  const loadSummary = useCallback(() => {
    api
      .get<Summary>("/notifications/summary")
      .then(setSummary)
      .catch(() => {});
  }, []);

  const load = useCallback(() => {
    setLoading(true);
    api
      .get<Notification[]>(query())
      .then((data) => {
        setNotifs(data);
        setExhausted(data.length < PAGE_SIZE);
        setError("");
      })
      .catch((e) => setError(e.message))
      .finally(() => setLoading(false));
  }, [query]);

  useEffect(() => {
    load();
    loadSummary();
  }, [load, loadSummary]);

  // Live arrival. The bell has been on this stream since the notification
  // centre shipped; this page never was, so a notification that arrived while
  // you were reading the list stayed invisible until you reloaded — on the one
  // screen whose entire purpose is to show you what just happened. The server
  // now sends the whole row, so a new one can simply be prepended.
  useEffect(() => {
    const es = new EventSource("/api/notifications/stream");
    es.onmessage = (e) => {
      let incoming: Notification;
      try {
        incoming = JSON.parse(e.data) as Notification;
      } catch {
        return; // keepalive text, or a payload from an older API — ignore it
      }
      if (!incoming?.id) return;
      const { unreadOnly, category } = filters.current;
      if (category && incoming.category !== category) return;
      if (unreadOnly && incoming.read_at) return;
      setNotifs((prev) =>
        prev.some((n) => n.id === incoming.id) ? prev : [incoming, ...prev]
      );
      setSummary((prev) =>
        prev ? { ...prev, total: prev.total + 1, unread: prev.unread + 1 } : prev
      );
    };
    return () => es.close();
  }, []);

  const loadMore = () => {
    const oldest = notifs[notifs.length - 1];
    if (!oldest) return;
    setLoadingMore(true);
    api
      .get<Notification[]>(query(oldest.created_at))
      .then((data) => {
        setNotifs((prev) => [...prev, ...data]);
        if (data.length < PAGE_SIZE) setExhausted(true);
      })
      .catch((e) => setError(e.message))
      .finally(() => setLoadingMore(false));
  };

  /**
   * Tell the layout its badge is stale.
   *
   * The bell polls `unread-count` every 60s, so marking everything read used to
   * leave a red "9+" sitting over an empty feed for up to a minute — the panel
   * disagreeing with itself about something the operator had just done. The
   * window-event bus already exists for exactly this (the theme switcher uses
   * it); this is the second subscriber.
   */
  const announce = () => window.dispatchEvent(new Event(NOTIF_CHANGE_EVENT));

  const markRead = async (id: string) => {
    try {
      await api.post(`/notifications/${id}/read`);
    } catch {
      // Already read (the endpoint 404s on a no-op) or a transient failure.
      // Either way the local state below is what the operator asked for, and a
      // reload reconciles it. An unhandled rejection here used to reach the
      // console on a double-click.
    }
    setNotifs((prev) =>
      prev.map((n) => (n.id === id ? { ...n, read_at: new Date().toISOString() } : n))
    );
    setSummary((prev) => (prev ? { ...prev, unread: Math.max(0, prev.unread - 1) } : prev));
    announce();
  };

  const markAllRead = async () => {
    try {
      await api.post("/notifications/read-all");
    } catch (e) {
      setError((e as Error).message);
      return;
    }
    setNotifs((prev) =>
      prev.map((n) => ({ ...n, read_at: n.read_at || new Date().toISOString() }))
    );
    setSummary((prev) => (prev ? { ...prev, unread: 0 } : prev));
    announce();
    if (unreadOnly) load();
  };

  const remove = async (id: string) => {
    const gone = notifs.find((n) => n.id === id);
    try {
      await api.delete(`/notifications/${id}`);
    } catch (e) {
      setError((e as Error).message);
      return;
    }
    setNotifs((prev) => prev.filter((n) => n.id !== id));
    setSummary((prev) =>
      prev
        ? {
            ...prev,
            total: Math.max(0, prev.total - 1),
            unread: gone && !gone.read_at ? Math.max(0, prev.unread - 1) : prev.unread,
          }
        : prev
    );
    if (gone && !gone.read_at) announce();
  };

  const clearRead = async () => {
    try {
      await api.delete("/notifications/read");
    } catch (e) {
      setError((e as Error).message);
      return;
    }
    load();
    loadSummary();
  };

  const unreadCount = summary?.unread ?? notifs.filter((n) => !n.read_at).length;
  const readCount = (summary?.total ?? notifs.length) - unreadCount;

  if (loading && notifs.length === 0) {
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

  const filtering = unreadOnly || category !== "";

  return (
    <div className="p-6 md:p-8">
      {/* Header */}
      <div className="flex flex-wrap items-start justify-between gap-3 mb-5">
        <div>
          <h1 className="text-2xl font-bold text-dark-50">Notifications</h1>
          <p className="text-sm text-dark-400 mt-1">
            {unreadCount > 0
              ? `${unreadCount} unread notification${unreadCount !== 1 ? "s" : ""}`
              : "All caught up"}
            {summary ? ` · ${summary.total} total` : ""}
          </p>
          {/* This page is visible to every role, and the link was not — it sent
              a client to `/settings`, which is `adminOnly` in the nav registry
              and answers `GET /api/settings` with a 403. The alert RULES behind
              it are `AuthUser` and per-user (`routes/alerts.rs:267`, `:321`), so
              a client's own alert destinations are real per-user data still
              parked behind an admin door — that surface has not moved yet, so
              the honest thing is to stop promising it rather than to keep
              offering a door that refuses. See the s325 carry. */}
          {user?.role === "admin" && (
            <Link to="/settings?tab=channels" className="text-xs text-accent-400 hover:text-accent-300 mt-1 inline-block">
              Configure alert channels &rarr;
            </Link>
          )}
        </div>
        <div className="flex items-center gap-2">
          {unreadCount > 0 && (
            <button
              onClick={markAllRead}
              className="px-3 py-1.5 text-xs font-medium rounded-lg transition-colors bg-dark-700 text-dark-200 hover:bg-dark-600 hover:text-dark-50"
            >
              Mark all read
            </button>
          )}
          {readCount > 0 && (
            <button
              onClick={clearRead}
              title="Delete every notification you have already read"
              className="px-3 py-1.5 text-xs font-medium rounded-lg transition-colors bg-dark-800 text-dark-300 border border-dark-700 hover:bg-dark-700 hover:text-dark-100"
            >
              Clear read ({readCount})
            </button>
          )}
        </div>
      </div>

      {/* Filters. Categories come from the server's own GROUP BY rather than a
          hardcoded list, so a new producer's category appears here the first
          time it fires instead of being invisible until someone edits this
          file. */}
      <div className="flex flex-wrap items-center gap-2 mb-5">
        <button
          onClick={() => setUnreadOnly((v) => !v)}
          aria-pressed={unreadOnly}
          className={`px-2.5 py-1 text-xs font-medium rounded-lg border transition-colors ${
            unreadOnly
              ? "bg-rust-500/15 text-rust-400 border-rust-500/40"
              : "bg-dark-800 text-dark-300 border-dark-700 hover:text-dark-100"
          }`}
        >
          Unread only
        </button>
        <span className="w-px h-4 bg-dark-700" />
        <button
          onClick={() => setCategory("")}
          aria-pressed={category === ""}
          className={`px-2.5 py-1 text-xs font-medium rounded-lg border transition-colors ${
            category === ""
              ? "bg-dark-700 text-dark-100 border-dark-600"
              : "bg-dark-800 text-dark-400 border-dark-700 hover:text-dark-200"
          }`}
        >
          All
        </button>
        {(summary?.categories ?? []).map((c) => (
          <button
            key={c.category}
            onClick={() => setCategory(c.category)}
            aria-pressed={category === c.category}
            className={`px-2.5 py-1 text-xs font-medium rounded-lg border transition-colors ${
              category === c.category
                ? "bg-dark-700 text-dark-100 border-dark-600"
                : "bg-dark-800 text-dark-400 border-dark-700 hover:text-dark-200"
            }`}
          >
            {c.category}
            <span className="ml-1.5 text-dark-400">{c.total}</span>
          </button>
        ))}
      </div>

      {error && (
        <div className="mb-4 p-3 rounded-lg bg-danger-500/10 text-danger-400 text-sm border border-danger-500/20">
          {error}
        </div>
      )}

      {notifs.length === 0 ? (
        <div className="text-center py-20">
          <svg
            className="w-12 h-12 mx-auto text-dark-400 mb-4"
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
          {/* An empty state under a filter is not the same as an empty feed, and
              telling someone "nothing has happened yet" while a filter hides 78
              rows is simply wrong. */}
          {filtering ? (
            <>
              <p className="text-dark-400 text-sm">Nothing matches this filter</p>
              <button
                onClick={() => {
                  setUnreadOnly(false);
                  setCategory("");
                }}
                className="text-xs text-accent-400 hover:text-accent-300 mt-2"
              >
                Show all notifications
              </button>
            </>
          ) : (
            <>
              <p className="text-dark-400 text-sm">No notifications yet</p>
              <p className="text-dark-400 text-xs mt-1">
                Alerts and system events will appear here
              </p>
            </>
          )}
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
                    {/* The title is the link when there is one. `link` has been
                        written by most producers since the notification centre
                        shipped and rendered by nothing, so every notification
                        was a dead end: the panel knew where the deploy that
                        failed lives and made you go and find it. Reading it also
                        marks the row read, which is what following it means. */}
                    {isSafeLink(n.link) ? (
                      <Link
                        to={n.link}
                        onClick={() => {
                          if (!n.read_at) markRead(n.id);
                        }}
                        className={`text-sm font-medium truncate rounded-sm hover:text-rust-400 hover:underline underline-offset-2 focus:outline-none focus-visible:ring-2 focus-visible:ring-rust-500 ${
                          n.read_at ? "text-dark-300" : "text-dark-50"
                        }`}
                      >
                        {n.title}
                      </Link>
                    ) : (
                      <h3
                        className={`text-sm font-medium truncate ${
                          n.read_at ? "text-dark-300" : "text-dark-50"
                        }`}
                      >
                        {n.title}
                      </h3>
                    )}
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
                  {/* `whitespace-pre-line`: the security producer builds its body
                      as "User: …\nIP: …\nCountry: …", and without this every one
                      of those rendered as a single run-on line. */}
                  <p
                    className={`text-xs leading-relaxed whitespace-pre-line ${
                      n.read_at ? "text-dark-400" : "text-dark-300"
                    }`}
                  >
                    {n.message}
                  </p>
                  {/* "3d ago" is the wrong resolution for an incident timeline,
                      so the exact stamp is on hover and in the accessibility
                      tree rather than nowhere. */}
                  <p className="text-[10px] text-dark-400 mt-1.5" title={formatDate(n.created_at)}>
                    <time dateTime={n.created_at}>{timeAgo(n.created_at)}</time>
                  </p>
                </div>

                {/* Row actions. These used to be one button revealed only by
                    `group-hover`, which means it did not exist for a keyboard
                    and did not exist on a touch screen. `focus-within` and a
                    focus ring put them back on both. */}
                <div className="flex items-center gap-1 opacity-0 group-hover:opacity-100 focus-within:opacity-100 transition-opacity">
                  {!n.read_at && (
                    <button
                      onClick={() => markRead(n.id)}
                      className="p-1.5 text-dark-400 hover:text-dark-200 rounded-lg transition-colors focus:outline-none focus-visible:ring-2 focus-visible:ring-rust-500 focus-visible:opacity-100"
                      title="Mark as read"
                      aria-label={`Mark "${n.title}" as read`}
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
                  <button
                    onClick={() => remove(n.id)}
                    className="p-1.5 text-dark-400 hover:text-danger-400 rounded-lg transition-colors focus:outline-none focus-visible:ring-2 focus-visible:ring-rust-500 focus-visible:opacity-100"
                    title="Delete"
                    aria-label={`Delete "${n.title}"`}
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
                        d="M6 18L18 6M6 6l12 12"
                      />
                    </svg>
                  </button>
                </div>
              </div>
            </div>
          ))}

          {/* Paging. The list was a hard `LIMIT 50` with nothing able to ask for
              row 51, so on this panel 28 of 78 notifications had no route to the
              screen at all. */}
          {!exhausted && (
            <div className="pt-2 text-center">
              <button
                onClick={loadMore}
                disabled={loadingMore}
                className="px-4 py-2 text-xs font-medium rounded-lg bg-dark-800 text-dark-300 border border-dark-700 hover:bg-dark-700 hover:text-dark-100 transition-colors disabled:opacity-50"
              >
                {loadingMore ? "Loading…" : "Load older"}
              </button>
            </div>
          )}
        </div>
      )}
    </div>
  );
}
