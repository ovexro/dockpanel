import { useState, useEffect, useRef, useCallback } from "react";
import { api } from "../api";

interface Site {
  id: string;
  domain: string;
}

const LOG_TYPES = [
  { value: "nginx_access", label: "Nginx Access" },
  { value: "nginx_error", label: "Nginx Error" },
  { value: "syslog", label: "System Log" },
  { value: "auth", label: "Auth Log" },
  { value: "php_fpm", label: "PHP-FPM" },
];

const SITE_LOG_TYPES = [
  { value: "access", label: "Access Log" },
  { value: "error", label: "Error Log" },
];

export default function Logs() {
  const [lines, setLines] = useState<string[]>([]);
  const [loading, setLoading] = useState(false);
  const [logType, setLogType] = useState("nginx_access");
  const [lineCount, setLineCount] = useState(100);
  const [filter, setFilter] = useState("");
  const [filterInput, setFilterInput] = useState("");
  const [sites, setSites] = useState<Site[]>([]);
  const [selectedSite, setSelectedSite] = useState("");
  const [autoRefresh, setAutoRefresh] = useState(false);
  const [streaming, setStreaming] = useState(false);
  const [mode, setMode] = useState<"tail" | "search">("tail");
  const [searchPattern, setSearchPattern] = useState("");
  const [searchMax, setSearchMax] = useState(500);
  const containerRef = useRef<HTMLDivElement>(null);
  const intervalRef = useRef<ReturnType<typeof setInterval> | null>(null);
  const wsRef = useRef<WebSocket | null>(null);
  const streamLinesRef = useRef<string[]>([]);
  const reconnectTimerRef = useRef<ReturnType<typeof setTimeout> | null>(null);
  const userStoppedRef = useRef(false);
  const [error, setError] = useState<string | null>(null);

  useEffect(() => {
    api.get<Site[]>("/sites").then(setSites).catch(() => setError("Failed to load sites. Please try again."));
  }, []);

  const scrollToBottom = () => {
    setTimeout(() => {
      if (containerRef.current) {
        containerRef.current.scrollTop = containerRef.current.scrollHeight;
      }
    }, 50);
  };

  const fetchLogs = useCallback(async () => {
    setLoading(true);
    try {
      let path: string;
      const type = selectedSite
        ? logType === "nginx_access"
          ? "access"
          : logType === "nginx_error"
            ? "error"
            : logType
        : logType;

      if (selectedSite) {
        path = `/sites/${selectedSite}/logs?type=${type}&lines=${lineCount}`;
      } else {
        path = `/logs?type=${logType}&lines=${lineCount}`;
      }
      if (filter) {
        path += `&filter=${encodeURIComponent(filter)}`;
      }

      const data = await api.get<string[]>(path);
      setLines(data);
      scrollToBottom();
    } catch (e) {
      setLines([
        `Error: ${e instanceof Error ? e.message : "Failed to load logs"}`,
      ]);
    } finally {
      setLoading(false);
    }
  }, [logType, lineCount, filter, selectedSite]);

  const handleSearch = async () => {
    if (!searchPattern.trim()) return;
    setLoading(true);
    try {
      let path: string;
      if (selectedSite) {
        const type = logType === "nginx_access" ? "access" : logType === "nginx_error" ? "error" : logType;
        path = `/sites/${selectedSite}/logs/search?type=${type}&pattern=${encodeURIComponent(searchPattern)}&max=${searchMax}`;
      } else {
        path = `/logs/search?type=${logType}&pattern=${encodeURIComponent(searchPattern)}&max=${searchMax}`;
      }
      const data = await api.get<string[]>(path);
      setLines(data);
      scrollToBottom();
    } catch (e) {
      setLines([`Error: ${e instanceof Error ? e.message : "Search failed"}`]);
    } finally {
      setLoading(false);
    }
  };

  // Fetch logs on param change (tail mode only)
  useEffect(() => {
    if (mode === "tail" && !streaming) {
      fetchLogs();
    }
  }, [fetchLogs, mode, streaming]);

  // Auto-refresh
  useEffect(() => {
    if (autoRefresh && mode === "tail" && !streaming) {
      intervalRef.current = setInterval(fetchLogs, 5000);
    }
    return () => {
      if (intervalRef.current) clearInterval(intervalRef.current);
    };
  }, [autoRefresh, fetchLogs, mode, streaming]);

  // WebSocket streaming with auto-reconnect
  const connectStream = async (isReconnect = false) => {
    try {
      const siteLogType = selectedSite
        ? logType === "nginx_access" ? "access" : logType === "nginx_error" ? "error" : logType
        : logType;
      let tokenUrl = `/logs/stream/token?type=${siteLogType}`;
      if (selectedSite) {
        tokenUrl += `&site_id=${selectedSite}`;
      }
      const resp = await api.get<{ token: string; domain?: string; type: string }>(tokenUrl);

      const proto = window.location.protocol === "https:" ? "wss:" : "ws:";
      // WebSocket API doesn't support Authorization headers; token is short-lived and same-origin
      let wsUrl = `${proto}//${window.location.host}/agent/logs/stream?token=${resp.token}&type=${resp.type}`;
      if (resp.domain) {
        wsUrl += `&domain=${resp.domain}`;
      }

      const ws = new WebSocket(wsUrl);
      wsRef.current = ws;
      if (!isReconnect) {
        streamLinesRef.current = [];
      }

      ws.onopen = () => {
        setStreaming(true);
        setAutoRefresh(false);
        if (isReconnect) {
          setLines((prev) => [...prev, "--- Stream reconnected ---"]);
        }
      };

      ws.onmessage = (e) => {
        streamLinesRef.current.push(e.data);
        // Keep max 2000 stream lines in memory
        if (streamLinesRef.current.length > 2000) {
          streamLinesRef.current = streamLinesRef.current.slice(-1500);
        }
        setLines((prev) => {
          const next = [...prev, e.data];
          return next.length > 2000 ? next.slice(-1500) : next;
        });
        scrollToBottom();
      };

      ws.onclose = () => {
        wsRef.current = null;
        // Auto-reconnect if the user didn't manually stop
        if (!userStoppedRef.current) {
          setLines((prev) => [...prev, "--- Stream disconnected, reconnecting in 3s... ---"]);
          reconnectTimerRef.current = setTimeout(() => {
            if (!userStoppedRef.current) {
              connectStream(true);
            }
          }, 3000);
        } else {
          setStreaming(false);
        }
      };

      ws.onerror = () => {
        // onclose will fire after onerror, reconnect logic lives there
        wsRef.current = null;
      };
    } catch (e) {
      setLines((prev) => [
        ...prev,
        `Stream error: ${e instanceof Error ? e.message : "Failed to connect"}`,
      ]);
      // Retry on token fetch failure too
      if (!userStoppedRef.current) {
        reconnectTimerRef.current = setTimeout(() => {
          if (!userStoppedRef.current) {
            connectStream(true);
          }
        }, 5000);
      } else {
        setStreaming(false);
      }
    }
  };

  const startStream = () => {
    stopStream();
    userStoppedRef.current = false;
    connectStream();
  };

  const stopStream = () => {
    userStoppedRef.current = true;
    if (reconnectTimerRef.current) {
      clearTimeout(reconnectTimerRef.current);
      reconnectTimerRef.current = null;
    }
    if (wsRef.current) {
      wsRef.current.close();
      wsRef.current = null;
    }
    setStreaming(false);
  };

  // Cleanup on unmount
  useEffect(() => {
    return () => {
      userStoppedRef.current = true;
      if (reconnectTimerRef.current) clearTimeout(reconnectTimerRef.current);
      if (wsRef.current) { wsRef.current.close(); wsRef.current = null; }
      if (intervalRef.current) clearInterval(intervalRef.current);
    };
  }, []);

  const handleFilter = () => {
    setFilter(filterInput);
  };

  const lineClass = (line: string) => {
    if (/\berror\b|ERROR|\b5\d{2}\b/i.test(line)) return "text-red-400";
    if (/\bwarn\b|WARN|\b4\d{2}\b/i.test(line)) return "text-amber-400";
    if (/\binfo\b|INFO/i.test(line)) return "text-blue-400";
    return "text-gray-300";
  };

  return (
    <div className="flex flex-col h-full">
      {/* Header */}
      <div className="px-4 sm:px-6 py-4 border-b border-dark-500 bg-dark-800 shrink-0">
        <div className="flex items-center justify-between mb-3 gap-3">
          <h1 className="text-sm font-medium text-dark-300 uppercase font-mono tracking-widest shrink-0">Log Viewer</h1>
          <div className="flex items-center gap-1.5 sm:gap-2 flex-wrap justify-end">
            {/* Mode toggle */}
            <div className="flex bg-dark-700 rounded-lg p-0.5">
              <button
                onClick={() => { setMode("tail"); stopStream(); }}
                className={`px-3 py-1 rounded-md text-xs font-medium transition-colors ${
                  mode === "tail" ? "bg-dark-800 text-dark-50 shadow-sm" : "text-dark-200"
                }`}
              >
                Tail
              </button>
              <button
                onClick={() => { setMode("search"); stopStream(); }}
                className={`px-3 py-1 rounded-md text-xs font-medium transition-colors ${
                  mode === "search" ? "bg-dark-800 text-dark-50 shadow-sm" : "text-dark-200"
                }`}
              >
                Search
              </button>
            </div>
            {mode === "tail" && (
              <>
                <button
                  onClick={streaming ? stopStream : startStream}
                  className={`px-3 py-1.5 rounded-lg text-xs font-medium transition-colors ${
                    streaming
                      ? "bg-red-500/15 text-red-400 hover:bg-red-500/20"
                      : "bg-emerald-500/15 text-emerald-400 hover:bg-emerald-500/20"
                  }`}
                >
                  {streaming ? "Stop Stream" : "Live Stream"}
                </button>
                {!streaming && (
                  <button
                    onClick={() => setAutoRefresh(!autoRefresh)}
                    className={`px-3 py-1.5 rounded-lg text-xs font-medium transition-colors ${
                      autoRefresh
                        ? "bg-emerald-500/15 text-emerald-400"
                        : "bg-dark-700 text-dark-200 hover:bg-dark-600"
                    }`}
                  >
                    {autoRefresh ? "Auto ON" : "Auto"}
                  </button>
                )}
              </>
            )}
            <button
              onClick={mode === "search" ? handleSearch : fetchLogs}
              disabled={loading || streaming}
              className="px-3 py-1.5 bg-accent-500 text-white rounded-lg text-xs font-medium hover:bg-accent-600 disabled:opacity-50"
            >
              {loading ? "Loading..." : mode === "search" ? "Search" : "Refresh"}
            </button>
          </div>
        </div>
        <div className="flex items-center gap-3 flex-wrap">
          <select
            value={selectedSite}
            onChange={(e) => { setSelectedSite(e.target.value); setLogType(e.target.value ? "nginx_access" : "nginx_access"); stopStream(); }}
            className="text-sm border border-dark-500 rounded-lg px-3 py-1.5 bg-dark-800"
            aria-label="Select site"
          >
            <option value="">System logs</option>
            {sites.map((s) => (
              <option key={s.id} value={s.id}>
                {s.domain}
              </option>
            ))}
          </select>
          <select
            value={logType}
            onChange={(e) => { setLogType(e.target.value); stopStream(); }}
            className="text-sm border border-dark-500 rounded-lg px-3 py-1.5 bg-dark-800"
            aria-label="Log type"
          >
            {(selectedSite ? SITE_LOG_TYPES : LOG_TYPES).map((t) => (
              <option key={t.value} value={t.value}>
                {t.label}
              </option>
            ))}
          </select>

          {mode === "tail" && !streaming && (
            <>
              <select
                value={lineCount}
                onChange={(e) => setLineCount(Number(e.target.value))}
                className="text-sm border border-dark-500 rounded-lg px-3 py-1.5 bg-dark-800"
                aria-label="Line count"
              >
                <option value={50}>50 lines</option>
                <option value={100}>100 lines</option>
                <option value={200}>200 lines</option>
                <option value={500}>500 lines</option>
                <option value={1000}>1000 lines</option>
              </select>
              <div className="flex items-center gap-1">
                <input
                  type="text"
                  value={filterInput}
                  onChange={(e) => setFilterInput(e.target.value)}
                  onKeyDown={(e) => e.key === "Enter" && handleFilter()}
                  placeholder="Filter..."
                  className="text-sm border border-dark-500 rounded-lg px-3 py-1.5 w-48"
                  aria-label="Filter logs"
                />
                <button
                  onClick={handleFilter}
                  className="px-3 py-1.5 bg-dark-700 text-dark-200 rounded-lg text-sm hover:bg-dark-600"
                >
                  Filter
                </button>
                {filter && (
                  <button
                    onClick={() => { setFilter(""); setFilterInput(""); }}
                    className="px-2 py-1.5 text-dark-300 hover:text-dark-200"
                  >
                    <svg className="w-4 h-4" fill="none" viewBox="0 0 24 24" stroke="currentColor" strokeWidth={2}>
                      <path strokeLinecap="round" strokeLinejoin="round" d="M6 18L18 6M6 6l12 12" />
                    </svg>
                  </button>
                )}
              </div>
            </>
          )}

          {mode === "search" && (
            <>
              <div className="flex items-center gap-1">
                <input
                  type="text"
                  value={searchPattern}
                  onChange={(e) => setSearchPattern(e.target.value)}
                  onKeyDown={(e) => e.key === "Enter" && handleSearch()}
                  placeholder="Regex pattern (e.g. 404|500)"
                  className="text-sm border border-dark-500 rounded-lg px-3 py-1.5 w-64 font-mono"
                  aria-label="Search pattern"
                />
              </div>
              <select
                value={searchMax}
                onChange={(e) => setSearchMax(Number(e.target.value))}
                className="text-sm border border-dark-500 rounded-lg px-3 py-1.5 bg-dark-800"
                aria-label="Max results"
              >
                <option value={100}>100 results</option>
                <option value={500}>500 results</option>
                <option value={1000}>1000 results</option>
                <option value={5000}>5000 results</option>
              </select>
            </>
          )}
        </div>

        {/* Stream indicator */}
        {streaming && (
          <div className="mt-2 flex items-center gap-2">
            <span className="relative flex h-2.5 w-2.5">
              <span className="animate-ping absolute inline-flex h-full w-full rounded-full bg-emerald-400 opacity-75" />
              <span className="relative inline-flex rounded-full h-2.5 w-2.5 bg-emerald-500" />
            </span>
            <span className="text-xs text-emerald-600 font-medium">
              Live streaming — {lines.length} lines received
            </span>
          </div>
        )}
      </div>

      {error && (
        <div className="px-6 py-3 bg-red-500/10 border-b border-red-500/20 text-red-400 text-sm">
          {error}
        </div>
      )}

      {/* Log content */}
      <div
        ref={containerRef}
        className="flex-1 bg-[#1e1e2e] overflow-auto font-mono text-sm p-4"
      >
        {lines.length === 0 ? (
          <div className="text-dark-200 text-center py-12">
            {mode === "search" ? "Enter a regex pattern and click Search" : "No log entries found"}
          </div>
        ) : (
          lines.map((line, i) => (
            <div
              key={i}
              className={`py-0.5 leading-relaxed whitespace-pre-wrap break-all ${lineClass(line)}`}
            >
              <span className="text-dark-200 select-none mr-3">
                {String(i + 1).padStart(4)}
              </span>
              {line}
            </div>
          ))
        )}
      </div>
    </div>
  );
}
