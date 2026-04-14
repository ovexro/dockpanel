import { useState, useEffect, useRef, useCallback } from "react";
import { Navigate } from "react-router-dom";
import { useAuth } from "../context/AuthContext";
import { api } from "../api";
import SystemLogsContent from "./SystemLogs";
import AuditLogContent from "./Activity";

interface Site {
  id: string;
  domain: string;
}

interface LogStatsTopUrl {
  url: string;
  count: number;
}

interface LogStats {
  requests_total: number;
  errors_5xx: number;
  status_breakdown: Record<string, number>;
  top_urls: LogStatsTopUrl[];
}

interface ErrorCheckResult {
  status: string;
  error_rate_percent: number;
  error_5xx: number;
  total_requests: number;
}

interface LogFileEntry {
  path: string;
  label: string;
  size_mb: number;
}

interface LogSizesResult {
  total_mb: number;
  files: LogFileEntry[];
  logrotate: boolean;
}

const LOG_TYPES = [
  { value: "nginx_access", label: "Nginx Access" },
  { value: "nginx_error", label: "Nginx Error" },
  { value: "syslog", label: "System Log" },
  { value: "auth", label: "Auth Log" },
  { value: "php_fpm", label: "PHP-FPM" },
  { value: "docker", label: "Docker" },
  { value: "service", label: "Services" },
];

const SITE_LOG_TYPES = [
  { value: "access", label: "Access Log" },
  { value: "error", label: "Error Log" },
];

const SERVICES = [
  "dockpanel-agent",
  "dockpanel-api",
  "nginx",
  "postfix",
  "dovecot",
  "fail2ban",
  "docker",
  "opendkim",
  "rspamd",
  "redis-server",
  "php8.3-fpm",
  "php8.2-fpm",
];

const TIME_PRESETS = [
  { label: "1h", lines: 500 },
  { label: "6h", lines: 3000 },
  { label: "24h", lines: 10000 },
];

function SiteLogsContent() {
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

  // Feature #2: Log Stats
  const [showStats, setShowStats] = useState(false);
  const [logStats, setLogStats] = useState<LogStats | null>(null);
  const [statsLoading, setStatsLoading] = useState(false);

  // Feature #3: Docker logs
  const [dockerContainers, setDockerContainers] = useState<string[]>([]);
  const [selectedContainer, setSelectedContainer] = useState("");

  // Feature #4: Service logs
  const [selectedService, setSelectedService] = useState("dockpanel-api");

  // Feature #6: Error alerting
  const [errorCheck, setErrorCheck] = useState<ErrorCheckResult | null>(null);
  const [checkingErrors, setCheckingErrors] = useState(false);

  // Feature #7: Log sizes
  const [showSizes, setShowSizes] = useState(false);
  const [logSizes, setLogSizes] = useState<LogSizesResult | null>(null);
  const [sizesLoading, setSizesLoading] = useState(false);
  const [truncating, setTruncating] = useState<string | null>(null);
  const [pendingTruncate, setPendingTruncate] = useState<string | null>(null);

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

  // Feature #1: Export/Download
  const handleExportLogs = () => {
    const content = lines.join("\n");
    if (!content) return;
    const blob = new Blob([content], { type: "text/plain" });
    const url = URL.createObjectURL(blob);
    const a = document.createElement("a");
    a.href = url;
    a.download = `logs-${new Date().toISOString().slice(0, 19).replace(/:/g, "-")}.txt`;
    a.click();
    URL.revokeObjectURL(url);
  };

  // Feature #2: Load stats
  const loadStats = async () => {
    setStatsLoading(true);
    try {
      const site = sites.find((s) => s.id === selectedSite);
      const domain = site?.domain ? `?domain=${site.domain}` : "";
      const data = await api.get<LogStats>(`/logs/stats${domain}`);
      setLogStats(data);
    } catch {
      setLogStats(null);
    } finally {
      setStatsLoading(false);
    }
  };

  // Feature #3: Load docker containers
  const loadDockerContainers = async () => {
    try {
      const data = await api.get<{ containers: string[] }>("/logs/docker");
      setDockerContainers(data.containers || []);
      if (data.containers?.length > 0 && !selectedContainer) {
        setSelectedContainer(data.containers[0]);
      }
    } catch {
      setDockerContainers([]);
    }
  };

  // Feature #3: Load docker logs
  const fetchDockerLogs = async (container?: string) => {
    const c = container || selectedContainer;
    if (!c) return;
    setLoading(true);
    try {
      const data = await api.get<{ logs: string; lines: number }>(
        `/logs/docker/${c}?lines=${lineCount}`
      );
      setLines(data.logs ? data.logs.split("\n") : ["No logs found"]);
      scrollToBottom();
    } catch (e) {
      setLines([`Error: ${e instanceof Error ? e.message : "Failed to load Docker logs"}`]);
    } finally {
      setLoading(false);
    }
  };

  // Feature #4: Load service logs
  const fetchServiceLogs = async (service?: string) => {
    const s = service || selectedService;
    if (!s) return;
    setLoading(true);
    try {
      const data = await api.get<{ logs: string; lines: number }>(
        `/logs/service/${s}?lines=${lineCount}`
      );
      setLines(data.logs ? data.logs.split("\n") : ["No logs found"]);
      scrollToBottom();
    } catch (e) {
      setLines([`Error: ${e instanceof Error ? e.message : "Failed to load service logs"}`]);
    } finally {
      setLoading(false);
    }
  };

  // Feature #6: Check errors
  const handleCheckErrors = async () => {
    setCheckingErrors(true);
    try {
      const data = await api.post<ErrorCheckResult>("/logs/check-errors");
      setErrorCheck(data);
    } catch {
      setErrorCheck(null);
    } finally {
      setCheckingErrors(false);
    }
  };

  // Feature #7: Load log sizes
  const loadLogSizes = async () => {
    setSizesLoading(true);
    try {
      const data = await api.get<LogSizesResult>("/logs/sizes");
      setLogSizes(data);
    } catch {
      setLogSizes(null);
    } finally {
      setSizesLoading(false);
    }
  };

  // Feature #7: Truncate log
  const handleTruncate = (path: string) => {
    setPendingTruncate(path);
  };

  const executeTruncate = async () => {
    const path = pendingTruncate;
    if (!path) return;
    setPendingTruncate(null);
    setTruncating(path);
    try {
      await api.post("/logs/truncate", { path });
      await loadLogSizes();
    } catch (e) {
      setError(`Failed to truncate: ${e instanceof Error ? e.message : "Unknown error"}`);
    } finally {
      setTruncating(null);
    }
  };

  const fetchLogs = useCallback(async () => {
    // Don't fetch regular logs when docker/service type is selected
    if (logType === "docker" || logType === "service") return;

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
      if (logType === "docker") {
        loadDockerContainers();
        if (selectedContainer) fetchDockerLogs();
      } else if (logType === "service") {
        fetchServiceLogs();
      } else {
        fetchLogs();
      }
    }
  }, [fetchLogs, mode, streaming, logType, selectedContainer, selectedService]);

  // Auto-refresh
  useEffect(() => {
    if (autoRefresh && mode === "tail" && !streaming) {
      const fn = logType === "docker" ? () => fetchDockerLogs() : logType === "service" ? () => fetchServiceLogs() : fetchLogs;
      intervalRef.current = setInterval(fn, 5000);
    }
    return () => {
      if (intervalRef.current) clearInterval(intervalRef.current);
    };
  }, [autoRefresh, fetchLogs, mode, streaming, logType, selectedContainer, selectedService]);

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
    if (/\berror\b|ERROR|\b5\d{2}\b/i.test(line)) return "text-danger-400";
    if (/\bwarn\b|WARN|\b4\d{2}\b/i.test(line)) return "text-warn-400";
    if (/\binfo\b|INFO/i.test(line)) return "text-accent-400";
    return "text-dark-200";
  };

  const isFileLogType = logType !== "docker" && logType !== "service";

  return (
    <div className="flex flex-col h-full">
      {/* Header */}
      <div className="px-4 sm:px-6 py-4 border-b border-dark-500 bg-dark-800 shrink-0">
        <div className="flex items-center justify-between mb-3 gap-3">
          <div className="flex items-center gap-1.5 sm:gap-2 flex-wrap justify-end ml-auto">
            {/* Mode toggle */}
            {isFileLogType && (
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
            )}
            {isFileLogType && mode === "tail" && (
              <>
                <button
                  onClick={streaming ? stopStream : startStream}
                  className={`px-3 py-1.5 rounded-lg text-xs font-medium transition-colors ${
                    streaming
                      ? "bg-danger-500/15 text-danger-400 hover:bg-danger-500/20"
                      : "bg-rust-500/15 text-rust-400 hover:bg-rust-500/20"
                  }`}
                >
                  {streaming ? "Stop Stream" : "Live Stream"}
                </button>
                {!streaming && (
                  <button
                    onClick={() => setAutoRefresh(!autoRefresh)}
                    className={`px-3 py-1.5 rounded-lg text-xs font-medium transition-colors ${
                      autoRefresh
                        ? "bg-rust-500/15 text-rust-400"
                        : "bg-dark-700 text-dark-200 hover:bg-dark-600"
                    }`}
                  >
                    {autoRefresh ? "Auto ON" : "Auto"}
                  </button>
                )}
              </>
            )}
            {/* Feature #1: Export button */}
            <button
              onClick={handleExportLogs}
              disabled={lines.length === 0}
              className="px-3 py-1.5 bg-dark-700 text-dark-100 rounded-lg text-xs font-medium hover:bg-dark-600 transition-colors disabled:opacity-50"
              title="Download logs as text file"
            >
              Export
            </button>
            {/* Feature #2: Stats toggle */}
            {!selectedSite && isFileLogType && (
              <button
                onClick={() => {
                  setShowStats(!showStats);
                  if (!showStats && !logStats) loadStats();
                }}
                className={`px-3 py-1.5 rounded-lg text-xs font-medium transition-colors ${
                  showStats ? "bg-accent-500/15 text-accent-400" : "bg-dark-700 text-dark-200 hover:bg-dark-600"
                }`}
              >
                Stats
              </button>
            )}
            {/* Feature #7: Storage toggle */}
            <button
              onClick={() => {
                setShowSizes(!showSizes);
                if (!showSizes && !logSizes) loadLogSizes();
              }}
              className={`px-3 py-1.5 rounded-lg text-xs font-medium transition-colors ${
                showSizes ? "bg-accent-500/15 text-accent-400" : "bg-dark-700 text-dark-200 hover:bg-dark-600"
              }`}
            >
              Storage
            </button>
            <button
              onClick={() => {
                if (logType === "docker") fetchDockerLogs();
                else if (logType === "service") fetchServiceLogs();
                else if (mode === "search") handleSearch();
                else fetchLogs();
              }}
              disabled={loading || streaming}
              className="px-3 py-1.5 bg-rust-500 text-white rounded-lg text-xs font-medium hover:bg-rust-600 disabled:opacity-50"
            >
              {loading ? "Loading..." : mode === "search" && isFileLogType ? "Search" : "Refresh"}
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
            onChange={(e) => {
              const val = e.target.value;
              setLogType(val);
              stopStream();
              if (val === "docker") loadDockerContainers();
            }}
            className="text-sm border border-dark-500 rounded-lg px-3 py-1.5 bg-dark-800"
            aria-label="Log type"
          >
            {(selectedSite ? SITE_LOG_TYPES : LOG_TYPES).map((t) => (
              <option key={t.value} value={t.value}>
                {t.label}
              </option>
            ))}
          </select>

          {/* Feature #3: Docker container selector */}
          {logType === "docker" && !selectedSite && (
            <select
              value={selectedContainer}
              onChange={(e) => {
                setSelectedContainer(e.target.value);
                fetchDockerLogs(e.target.value);
              }}
              className="text-sm border border-dark-500 rounded-lg px-3 py-1.5 bg-dark-800"
              aria-label="Docker container"
            >
              {dockerContainers.length === 0 ? (
                <option value="">No containers found</option>
              ) : (
                dockerContainers.map((c) => (
                  <option key={c} value={c}>{c}</option>
                ))
              )}
            </select>
          )}

          {/* Feature #4: Service selector */}
          {logType === "service" && !selectedSite && (
            <select
              value={selectedService}
              onChange={(e) => {
                setSelectedService(e.target.value);
                fetchServiceLogs(e.target.value);
              }}
              className="text-sm border border-dark-500 rounded-lg px-3 py-1.5 bg-dark-800"
              aria-label="Service"
            >
              {SERVICES.map((s) => (
                <option key={s} value={s}>{s}</option>
              ))}
            </select>
          )}

          {(isFileLogType || logType === "docker" || logType === "service") && mode === "tail" && !streaming && (
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

              {/* Feature #5: Time range quick buttons */}
              <div className="flex gap-1">
                {TIME_PRESETS.map((t) => (
                  <button
                    key={t.label}
                    onClick={() => {
                      setLineCount(t.lines);
                      // Trigger reload on next render via lineCount change
                    }}
                    className={`px-2 py-1 rounded text-xs font-medium transition-colors ${
                      lineCount === t.lines
                        ? "bg-rust-500/15 text-rust-400"
                        : "bg-dark-700 text-dark-200 hover:bg-dark-600"
                    }`}
                  >
                    {t.label}
                  </button>
                ))}
              </div>

              {isFileLogType && (
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
              )}
            </>
          )}

          {isFileLogType && mode === "search" && (
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
              <span className="animate-ping absolute inline-flex h-full w-full rounded-full bg-rust-400 opacity-75" />
              <span className="relative inline-flex rounded-full h-2.5 w-2.5 bg-rust-500" />
            </span>
            <span className="text-xs text-rust-400 font-medium">
              Live streaming — {lines.length} lines received
            </span>
          </div>
        )}
      </div>

      {error && (
        <div className="px-6 py-3 bg-danger-500/10 border-b border-danger-500/20 text-danger-400 text-sm">
          {error}
        </div>
      )}

      {/* Confirm truncate bar */}
      {pendingTruncate && (
        <div className="mx-4 sm:mx-6 mt-4 border border-danger-500/30 bg-danger-500/5 rounded-lg px-4 py-3 flex items-center justify-between">
          <span className="text-xs text-danger-400 font-mono">Clear log file? This cannot be undone: {pendingTruncate}</span>
          <div className="flex items-center gap-2 shrink-0 ml-4">
            <button onClick={executeTruncate} className="px-3 py-1.5 bg-danger-500 text-white text-xs font-bold uppercase tracking-wider hover:bg-danger-400 transition-colors">Confirm</button>
            <button onClick={() => setPendingTruncate(null)} className="px-3 py-1.5 bg-dark-600 text-dark-200 text-xs font-bold uppercase tracking-wider hover:bg-dark-500 transition-colors">Cancel</button>
          </div>
        </div>
      )}

      {/* Feature #2: Stats Panel */}
      {showStats && (
        <div className="px-4 sm:px-6 py-4 border-b border-dark-500 bg-dark-800/50">
          <div className="flex items-center justify-between mb-3">
            <h3 className="text-sm font-medium text-dark-100">Log Statistics</h3>
            <div className="flex items-center gap-2">
              {/* Feature #6: Check Errors button */}
              <button
                onClick={handleCheckErrors}
                disabled={checkingErrors}
                className="px-3 py-1 bg-danger-500/10 text-danger-400 rounded text-xs font-medium hover:bg-danger-500/20 disabled:opacity-50"
              >
                {checkingErrors ? "Checking..." : "Check Error Rate"}
              </button>
              <button
                onClick={loadStats}
                disabled={statsLoading}
                className="px-3 py-1 bg-dark-700 text-dark-200 rounded text-xs hover:bg-dark-600 disabled:opacity-50"
              >
                {statsLoading ? "Loading..." : "Reload"}
              </button>
            </div>
          </div>
          {logStats ? (
            <div className="grid grid-cols-2 sm:grid-cols-4 gap-4 mb-3">
              <div className="bg-dark-700 rounded-lg p-3">
                <div className="text-xs text-dark-300">Total Requests</div>
                <div className="text-lg font-mono text-dark-50">{logStats.requests_total?.toLocaleString() || 0}</div>
              </div>
              <div className="bg-dark-700 rounded-lg p-3">
                <div className="text-xs text-dark-300">5xx Errors</div>
                <div className={`text-lg font-mono ${logStats.errors_5xx > 0 ? "text-danger-400" : "text-rust-400"}`}>
                  {logStats.errors_5xx?.toLocaleString() || 0}
                </div>
              </div>
              <div className="bg-dark-700 rounded-lg p-3">
                <div className="text-xs text-dark-300">Status Codes</div>
                <div className="text-xs font-mono text-dark-100 mt-1 space-y-0.5">
                  {logStats.status_breakdown && Object.entries(logStats.status_breakdown)
                    .sort(([, a], [, b]) => (b as number) - (a as number))
                    .slice(0, 5)
                    .map(([code, count]) => (
                      <div key={code} className="flex justify-between">
                        <span className={code.startsWith("5") ? "text-danger-400" : code.startsWith("4") ? "text-warn-400" : "text-rust-400"}>
                          {code}
                        </span>
                        <span>{(count as number).toLocaleString()}</span>
                      </div>
                    ))}
                </div>
              </div>
              <div className="bg-dark-700 rounded-lg p-3">
                <div className="text-xs text-dark-300">Top URLs</div>
                <div className="text-xs font-mono text-dark-100 mt-1 space-y-0.5 max-h-20 overflow-auto">
                  {logStats.top_urls?.slice(0, 5).map((u, i) => (
                    <div key={i} className="flex justify-between gap-2">
                      <span className="truncate">{u.url}</span>
                      <span className="shrink-0">{u.count}</span>
                    </div>
                  ))}
                </div>
              </div>
            </div>
          ) : statsLoading ? (
            <div className="text-dark-300 text-sm">Loading stats...</div>
          ) : (
            <div className="text-dark-300 text-sm">No stats available</div>
          )}
          {/* Feature #6: Error check result */}
          {errorCheck && (
            <div className={`mt-2 p-3 rounded-lg text-sm ${
              errorCheck.status === "warning" ? "bg-danger-500/10 border border-danger-500/20" : "bg-rust-500/10 border border-rust-500/20"
            }`}>
              <span className={errorCheck.status === "warning" ? "text-danger-400" : "text-rust-400"}>
                Error rate: {errorCheck.error_rate_percent}% ({errorCheck.error_5xx} / {errorCheck.total_requests} requests)
                {errorCheck.status === "warning"
                  ? " — High error rate detected!"
                  : " — Within normal range"}
              </span>
            </div>
          )}
        </div>
      )}

      {/* Feature #7: Log Sizes Panel */}
      {showSizes && (
        <div className="px-4 sm:px-6 py-4 border-b border-dark-500 bg-dark-800/50">
          <div className="flex items-center justify-between mb-3">
            <h3 className="text-sm font-medium text-dark-100">
              Log Storage
              {logSizes && <span className="text-dark-300 ml-2 font-normal">({logSizes.total_mb} MB total)</span>}
            </h3>
            <button
              onClick={loadLogSizes}
              disabled={sizesLoading}
              className="px-3 py-1 bg-dark-700 text-dark-200 rounded text-xs hover:bg-dark-600 disabled:opacity-50"
            >
              {sizesLoading ? "Loading..." : "Reload"}
            </button>
          </div>
          {logSizes?.files ? (
            <div className="space-y-1">
              {logSizes.files.map((f) => (
                <div key={f.path} className="flex items-center justify-between bg-dark-700 rounded px-3 py-2">
                  <div className="flex items-center gap-3">
                    <span className="text-sm text-dark-100 font-mono">{f.label}</span>
                    <span className="text-xs text-dark-300">{f.path}</span>
                  </div>
                  <div className="flex items-center gap-3">
                    <span className={`text-sm font-mono ${f.size_mb > 100 ? "text-warn-400" : "text-dark-200"}`}>
                      {f.size_mb} MB
                    </span>
                    <button
                      onClick={() => handleTruncate(f.path)}
                      disabled={truncating === f.path}
                      className="px-2 py-0.5 bg-danger-500/10 text-danger-400 rounded text-xs hover:bg-danger-500/20 disabled:opacity-50"
                    >
                      {truncating === f.path ? "..." : "Clear"}
                    </button>
                  </div>
                </div>
              ))}
              {logSizes.logrotate && (
                <div className="text-xs text-rust-400 mt-2">Logrotate is configured for nginx</div>
              )}
              {!logSizes.logrotate && (
                <div className="text-xs text-warn-400 mt-2">Logrotate not detected for nginx</div>
              )}
            </div>
          ) : sizesLoading ? (
            <div className="text-dark-300 text-sm">Loading...</div>
          ) : (
            <div className="text-dark-300 text-sm">No data available</div>
          )}
        </div>
      )}

      {/* Log content */}
      <div
        ref={containerRef}
        className="flex-1 bg-dark-950 overflow-auto font-mono text-sm p-4"
      >
        {lines.length === 0 ? (
          <div className="text-dark-200 text-center py-12">
            {logType === "docker" && dockerContainers.length === 0
              ? "No managed Docker containers found"
              : mode === "search" && isFileLogType
                ? "Enter a regex pattern and click Search"
                : "No log entries found"}
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

type LogTab = "site" | "system" | "audit";

export default function Logs() {
  const { user } = useAuth();
  const [tab, setTab] = useState<LogTab>("site");

  if (!user) return <Navigate to="/login" replace />;
  if (user.role !== "admin") return <Navigate to="/" replace />;

  return (
    <div className="flex flex-col h-full">
      {/* Header */}
      <div className="px-6 pt-6 pb-0">
        <div className="flex flex-col sm:flex-row sm:items-center justify-between gap-3 mb-4 pb-4 border-b border-dark-600">
          <div>
            <h1 className="text-sm font-medium text-dark-300 uppercase font-mono tracking-widest">Logs</h1>
            <p className="text-sm text-dark-200 font-mono mt-1">Site logs, system events, and audit trail</p>
          </div>
        </div>
        {/* Tabs */}
        <div className="flex gap-6 mb-6 text-sm font-mono overflow-x-auto">
          <button onClick={() => setTab("site")} className={`whitespace-nowrap ${tab === "site" ? "border-b-2 border-rust-500 text-dark-50 pb-2" : "text-dark-300 hover:text-dark-100 pb-2"}`}>Site Logs</button>
          <button onClick={() => setTab("system")} className={`whitespace-nowrap ${tab === "system" ? "border-b-2 border-rust-500 text-dark-50 pb-2" : "text-dark-300 hover:text-dark-100 pb-2"}`}>System Logs</button>
          <button onClick={() => setTab("audit")} className={`whitespace-nowrap ${tab === "audit" ? "border-b-2 border-rust-500 text-dark-50 pb-2" : "text-dark-300 hover:text-dark-100 pb-2"}`}>Audit Log</button>
        </div>
      </div>
      {/* Content */}
      <div className="flex-1 overflow-auto">
        {tab === "site" && <SiteLogsContent />}
        {tab === "system" && <SystemLogsContent />}
        {tab === "audit" && <AuditLogContent />}
      </div>
    </div>
  );
}
