import { useState, useEffect, useRef, useCallback } from "react";
import { useSearchParams } from "react-router-dom";
import { Terminal as XTerm } from "@xterm/xterm";
import { FitAddon } from "@xterm/addon-fit";
import { SearchAddon } from "@xterm/addon-search";
import "@xterm/xterm/css/xterm.css";
import { api } from "../api";

interface Site {
  id: string;
  domain: string;
}

// ── Terminal themes ──
const themes: Record<string, Record<string, string>> = {
  mocha: {
    background: "#1e1e2e",
    foreground: "#cdd6f4",
    cursor: "#f5e0dc",
    selectionBackground: "#585b7066",
    black: "#45475a",
    red: "#f38ba8",
    green: "#a6e3a1",
    yellow: "#f9e2af",
    blue: "#89b4fa",
    magenta: "#f5c2e7",
    cyan: "#94e2d5",
    white: "#bac2de",
    brightBlack: "#585b70",
    brightRed: "#f38ba8",
    brightGreen: "#a6e3a1",
    brightYellow: "#f9e2af",
    brightBlue: "#89b4fa",
    brightMagenta: "#f5c2e7",
    brightCyan: "#94e2d5",
    brightWhite: "#a6adc8",
  },
  dracula: {
    background: "#282a36",
    foreground: "#f8f8f2",
    cursor: "#f8f8f2",
    selectionBackground: "#44475a66",
    black: "#21222c",
    red: "#ff5555",
    green: "#50fa7b",
    yellow: "#f1fa8c",
    blue: "#bd93f9",
    magenta: "#ff79c6",
    cyan: "#8be9fd",
    white: "#f8f8f2",
    brightBlack: "#6272a4",
    brightRed: "#ff6e6e",
    brightGreen: "#69ff94",
    brightYellow: "#ffffa5",
    brightBlue: "#d6acff",
    brightMagenta: "#ff92df",
    brightCyan: "#a4ffff",
    brightWhite: "#ffffff",
  },
  light: {
    background: "#fafafa",
    foreground: "#383a42",
    cursor: "#526eff",
    selectionBackground: "#d0d0d066",
    black: "#383a42",
    red: "#e45649",
    green: "#50a14f",
    yellow: "#c18401",
    blue: "#4078f2",
    magenta: "#a626a4",
    cyan: "#0184bc",
    white: "#a0a1a7",
    brightBlack: "#696c77",
    brightRed: "#e45649",
    brightGreen: "#50a14f",
    brightYellow: "#c18401",
    brightBlue: "#4078f2",
    brightMagenta: "#a626a4",
    brightCyan: "#0184bc",
    brightWhite: "#fafafa",
  },
};

// ── Saved command snippets ──
const snippets = [
  { label: "Restart Nginx", cmd: "systemctl restart nginx" },
  { label: "Restart PHP-FPM", cmd: "systemctl restart php8.3-fpm" },
  { label: "Disk Usage", cmd: "df -h" },
  { label: "Memory Usage", cmd: "free -h" },
  { label: "Top Processes", cmd: "top -bn1 | head -20" },
  { label: "Docker Containers", cmd: "docker ps" },
  { label: "Nginx Test", cmd: "nginx -t" },
  { label: "Clear Cache", cmd: "sync && echo 3 > /proc/sys/vm/drop_caches" },
  { label: "System Info", cmd: "uname -a && uptime" },
  { label: "Tail Nginx Errors", cmd: "tail -50 /var/log/nginx/error.log" },
  { label: "Persistent Session (tmux)", cmd: "tmux new-session -A -s dockpanel" },
];

const IDLE_TIMEOUT = 30 * 60 * 1000; // 30 minutes
const MAX_RECONNECT_ATTEMPTS = 3;

export default function Terminal() {
  const [searchParams] = useSearchParams();
  const initialSiteId = searchParams.get("site") || "";
  const termRef = useRef<HTMLDivElement>(null);
  const wsRef = useRef<WebSocket | null>(null);
  const xtermRef = useRef<XTerm | null>(null);
  const fitRef = useRef<FitAddon | null>(null);
  const searchAddonRef = useRef<SearchAddon | null>(null);
  const idleTimer = useRef<ReturnType<typeof setTimeout>>(undefined);

  // Refs for avoiding stale closures
  const intentionalClose = useRef(false);
  const reconnectAttempts = useRef(0);
  const reconnectTimer = useRef<ReturnType<typeof setTimeout>>(undefined);
  const statusRef = useRef("");

  const [connected, setConnected] = useState(false);
  const [status, setStatus] = useState("");
  const [error, setError] = useState("");
  const [sites, setSites] = useState<Site[]>([]);
  const [selectedSite, setSelectedSite] = useState(initialSiteId);

  // Snippets
  const [showSnippets, setShowSnippets] = useState(false);

  // Font size (persisted, smaller default on mobile)
  const [fontSize, setFontSize] = useState(() => {
    const stored = localStorage.getItem("dp-terminal-font");
    if (stored) return parseInt(stored);
    return window.innerWidth < 768 ? 11 : 14;
  });

  // Mobile toolbar toggle
  const [showMoreTools, setShowMoreTools] = useState(false);

  // Theme (persisted)
  const [themeName, setThemeName] = useState(
    () => localStorage.getItem("dp-terminal-theme") || "mocha"
  );

  // Search
  const [showSearch, setShowSearch] = useState(false);
  const [searchTerm, setSearchTerm] = useState("");
  const searchInputRef = useRef<HTMLInputElement>(null);

  // SSH Info panel
  const [showSshInfo, setShowSshInfo] = useState(false);

  // Terminal Recording
  const [recording, setRecording] = useState(false);
  const recordingRef = useRef(false);
  const recordingData = useRef<{ time: number; data: string; type: "o" | "i" }[]>([]);
  const recordingStart = useRef<number>(0);

  // Keep statusRef in sync with status state
  useEffect(() => {
    statusRef.current = status;
  }, [status]);

  // Persist font size
  useEffect(() => {
    localStorage.setItem("dp-terminal-font", fontSize.toString());
  }, [fontSize]);

  // Persist theme
  useEffect(() => {
    localStorage.setItem("dp-terminal-theme", themeName);
  }, [themeName]);

  // Load sites
  useEffect(() => {
    api.get<Site[]>("/sites").then(setSites).catch(() => setError("Failed to load sites"));
  }, []);

  // Idle timeout reset
  const resetIdleTimer = useCallback(() => {
    if (idleTimer.current) clearTimeout(idleTimer.current);
    idleTimer.current = setTimeout(() => {
      if (wsRef.current && wsRef.current.readyState === WebSocket.OPEN) {
        intentionalClose.current = true;
        wsRef.current.close();
        setStatus("Disconnected (idle timeout — 30 min)");
        setConnected(false);
      }
    }, IDLE_TIMEOUT);
  }, []);

  const connect = useCallback(
    async (siteIdParam?: string) => {
      if (!termRef.current) return;
      setError("");
      setStatus("");

      // Clear any pending reconnect timer
      if (reconnectTimer.current) {
        clearTimeout(reconnectTimer.current);
        reconnectTimer.current = undefined;
      }

      // Cleanup previous
      intentionalClose.current = true; // Prevent reconnect from the old socket's onclose
      if (wsRef.current) {
        wsRef.current.close();
        wsRef.current = null;
      }
      if (xtermRef.current) {
        xtermRef.current.dispose();
        xtermRef.current = null;
      }
      intentionalClose.current = false; // Reset for the new connection

      const currentTheme = themes[themeName] || themes.mocha;

      // Create terminal
      const term = new XTerm({
        cursorBlink: true,
        fontSize,
        fontFamily: "'JetBrains Mono', 'Fira Code', 'Cascadia Code', monospace",
        theme: currentTheme,
      });

      const fit = new FitAddon();
      const searchAddon = new SearchAddon();
      term.loadAddon(fit);
      term.loadAddon(searchAddon);
      term.open(termRef.current);
      fit.fit();
      xtermRef.current = term;
      fitRef.current = fit;
      searchAddonRef.current = searchAddon;

      // Ctrl+F handler for search
      term.attachCustomKeyEventHandler((e: KeyboardEvent) => {
        if (e.ctrlKey && e.key === "f" && e.type === "keydown") {
          e.preventDefault();
          setShowSearch((prev) => {
            const next = !prev;
            if (next) {
              setTimeout(() => searchInputRef.current?.focus(), 50);
            }
            return next;
          });
          return false;
        }
        return true;
      });

      term.writeln("\x1b[34m● Connecting to server...\x1b[0m");

      // Get token from backend
      try {
        const qs = siteIdParam ? `?site_id=${siteIdParam}` : "";
        const data = await api.get<{ token: string; domain: string | null }>(
          `/terminal/token${qs}`
        );

        // Connect via WebSocket to the agent
        const proto = window.location.protocol === "https:" ? "wss:" : "ws:";
        const domain = data.domain || "";
        const cols = term.cols;
        const rows = term.rows;
        const wsUrl = `${proto}//${window.location.host}/agent/terminal/ws?token=${data.token}&domain=${encodeURIComponent(domain)}&cols=${cols}&rows=${rows}`;

        const ws = new WebSocket(wsUrl);
        wsRef.current = ws;

        ws.onopen = () => {
          setConnected(true);
          setStatus("");
          setError("");
          reconnectAttempts.current = 0;
          term.clear();
          resetIdleTimer();
        };

        ws.onmessage = (event) => {
          term.write(event.data);
          // Capture output for recording (use ref to avoid stale closure)
          if (recordingRef.current) {
            recordingData.current.push({ time: Date.now(), data: event.data, type: "o" });
          }
        };

        ws.onclose = () => {
          setConnected(false);

          // Auto-reconnect on unexpected close
          if (!intentionalClose.current && reconnectAttempts.current < MAX_RECONNECT_ATTEMPTS) {
            const delay = Math.min(2000 * Math.pow(2, reconnectAttempts.current), 10000);
            reconnectAttempts.current++;
            const msg = `Connection lost. Reconnecting in ${delay / 1000}s... (attempt ${reconnectAttempts.current}/${MAX_RECONNECT_ATTEMPTS})`;
            setError(msg);
            term.writeln(`\r\n\x1b[33m● ${msg}\x1b[0m`);
            reconnectTimer.current = setTimeout(() => connect(siteIdParam), delay);
          } else if (!intentionalClose.current && reconnectAttempts.current >= MAX_RECONNECT_ATTEMPTS) {
            setError("Connection lost. Click Reconnect to try again.");
            term.writeln("\r\n\x1b[31m● Connection lost after 3 attempts. Click Reconnect to try again.\x1b[0m");
          } else {
            // Intentional close or already showing a status message
            if (!statusRef.current) {
              term.writeln("\r\n\x1b[31m● Connection closed\x1b[0m");
            }
          }
        };

        ws.onerror = () => {
          setError("WebSocket connection failed");
          setConnected(false);
        };

        // Send terminal input to WebSocket
        term.onData((inputData) => {
          if (ws.readyState === WebSocket.OPEN) {
            ws.send(JSON.stringify({ type: "input", data: inputData }));
            resetIdleTimer();
            // Capture input for recording (asciinema "i" event type)
            if (recordingRef.current) {
              recordingData.current.push({ time: Date.now(), data: inputData, type: "i" });
            }
          }
        });

        // Handle resize
        term.onResize(({ cols, rows }) => {
          if (ws.readyState === WebSocket.OPEN) {
            ws.send(JSON.stringify({ type: "resize", cols, rows }));
          }
        });
      } catch (e) {
        setError(e instanceof Error ? e.message : "Failed to connect");
        term.writeln(
          `\r\n\x1b[31m● Error: ${e instanceof Error ? e.message : "Connection failed"}\x1b[0m`
        );
      }
    },
    [fontSize, themeName, resetIdleTimer]
  );

  // Connect on mount
  useEffect(() => {
    connect(selectedSite || undefined);
    return () => {
      if (idleTimer.current) clearTimeout(idleTimer.current);
      if (reconnectTimer.current) clearTimeout(reconnectTimer.current);
      intentionalClose.current = true;
      wsRef.current?.close();
      xtermRef.current?.dispose();
    };
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, []);

  // Handle resize
  useEffect(() => {
    const handleResize = () => fitRef.current?.fit();
    window.addEventListener("resize", handleResize);
    return () => window.removeEventListener("resize", handleResize);
  }, []);

  const handleSiteChange = (newSiteId: string) => {
    setSelectedSite(newSiteId);
    reconnectAttempts.current = 0;
    connect(newSiteId || undefined);
  };

  const handleReconnect = () => {
    reconnectAttempts.current = 0;
    connect(selectedSite || undefined);
  };

  const changeFontSize = (delta: number) => {
    const newSize = Math.max(10, Math.min(24, fontSize + delta));
    setFontSize(newSize);
    if (xtermRef.current) {
      xtermRef.current.options.fontSize = newSize;
      fitRef.current?.fit();
    }
  };

  const changeTheme = (name: string) => {
    setThemeName(name);
    if (xtermRef.current && themes[name]) {
      xtermRef.current.options.theme = themes[name];
    }
  };

  const handleSearch = (direction: "next" | "prev") => {
    if (!searchAddonRef.current || !searchTerm) return;
    if (direction === "next") {
      searchAddonRef.current.findNext(searchTerm);
    } else {
      searchAddonRef.current.findPrevious(searchTerm);
    }
  };

  const toggleRecording = () => {
    if (recording) {
      // Stop and download
      const cast = {
        version: 2,
        width: xtermRef.current?.cols || 80,
        height: xtermRef.current?.rows || 24,
        timestamp: Math.floor(recordingStart.current / 1000),
        env: { SHELL: "/bin/bash", TERM: "xterm-256color" },
      };
      const header = JSON.stringify(cast);
      const events = recordingData.current
        .map((e) =>
          JSON.stringify([
            (e.time - recordingStart.current) / 1000,
            e.type,
            e.data,
          ])
        )
        .join("\n");
      const content = header + "\n" + events;

      const blob = new Blob([content], { type: "text/plain" });
      const url = URL.createObjectURL(blob);
      const a = document.createElement("a");
      a.href = url;
      a.download = `terminal-${new Date().toISOString().slice(0, 19).replace(/:/g, "-")}.cast`;
      a.click();
      URL.revokeObjectURL(url);

      recordingData.current = [];
      recordingRef.current = false;
      setRecording(false);
      setStatus("Recording saved");
      setTimeout(() => setStatus(""), 2000);
    } else {
      // Start recording
      recordingData.current = [];
      recordingStart.current = Date.now();
      recordingRef.current = true;
      setRecording(true);
      setStatus("Recording started...");
      setTimeout(() => setStatus(""), 2000);
    }
  };

  // Derive header label from selected site
  const headerLabel = selectedSite
    ? sites.find((s) => s.id === selectedSite)?.domain || "Site Terminal"
    : "Server Terminal";

  const currentThemeBg = (themes[themeName] || themes.mocha).background;

  return (
    <div className="flex flex-col h-full p-2 sm:p-4 max-w-full overflow-hidden">
      <div className="flex flex-col flex-1 border border-dark-500 min-h-0 overflow-hidden">
        {/* Header */}
        <div className="px-3 sm:px-5 py-2 sm:py-3 border-b border-dark-500 bg-dark-800 shrink-0">
          <div className="flex items-center justify-between">
            <div className="flex items-center gap-3 min-w-0">
              <div className="min-w-0">
                <h1 className="text-xs sm:text-sm font-medium text-dark-300 uppercase font-mono tracking-widest truncate">
                  {headerLabel}
                </h1>
                <div className="flex items-center gap-2 mt-0.5">
                  <div
                    className={`w-2 h-2 rounded-full shrink-0 ${
                      connected ? "bg-rust-500" : "bg-dark-300"
                    }`}
                  />
                  <span className="text-xs text-dark-200 truncate">
                    {status || (connected ? "Connected" : "Disconnected")}
                  </span>
                </div>
              </div>
            </div>
            {/* Primary controls (always visible) */}
            <div className="flex items-center gap-2 sm:gap-3 shrink-0">
              {/* Site selector */}
              <select
                value={selectedSite}
                onChange={(e) => handleSiteChange(e.target.value)}
                className="text-xs sm:text-sm border border-dark-500 rounded-lg px-2 sm:px-3 py-1 sm:py-1.5 bg-dark-800 max-w-[120px] sm:max-w-none"
              >
                <option value="">Server root</option>
                {sites.map((s) => (
                  <option key={s.id} value={s.id}>
                    {s.domain}
                  </option>
                ))}
              </select>

              {/* More tools toggle (mobile) */}
              <button
                onClick={() => setShowMoreTools(!showMoreTools)}
                className="px-2 py-1 bg-dark-700 text-dark-200 rounded text-xs hover:bg-dark-600 md:hidden"
              >
                {showMoreTools ? "Less" : "More"}
              </button>
            </div>
          </div>

          {/* Secondary controls (hidden on mobile unless toggled) */}
          <div className={`mt-2 ${showMoreTools ? "" : "hidden md:block"}`}>
            {/* Mobile: grid layout. Desktop: flex row */}
            <div className="grid grid-cols-4 gap-1 md:flex md:flex-wrap md:items-center md:gap-2">
            {/* Font size controls */}
            <div className="flex items-center justify-center gap-1 col-span-2 md:col-span-1">
              <button
                onClick={() => changeFontSize(-1)}
                className="px-2 py-1.5 bg-dark-700 text-dark-200 rounded text-xs hover:bg-dark-600 active:bg-dark-500 touch-manipulation"
              >
                A-
              </button>
              <span className="text-xs text-dark-300 font-mono w-6 text-center">
                {fontSize}
              </span>
              <button
                onClick={() => changeFontSize(1)}
                className="px-2 py-1.5 bg-dark-700 text-dark-200 rounded text-xs hover:bg-dark-600 active:bg-dark-500 touch-manipulation"
              >
                A+
              </button>
            </div>

            {/* Theme selector */}
            <select
              value={themeName}
              onChange={(e) => changeTheme(e.target.value)}
              className="px-2 py-1.5 bg-dark-700 text-dark-200 rounded text-xs border border-dark-600 col-span-2 md:col-span-1"
            >
              <option value="mocha">Mocha</option>
              <option value="dracula">Dracula</option>
              <option value="light">Light</option>
            </select>

            {/* Snippets toggle */}
            <button
              onClick={() => setShowSnippets(!showSnippets)}
              className={`py-1.5 rounded text-xs font-mono text-center touch-manipulation ${
                showSnippets
                  ? "bg-rust-500/20 text-rust-400 border border-rust-500/30"
                  : "bg-dark-700 text-dark-200 hover:bg-dark-600 active:bg-dark-500"
              }`}
            >
              Snippets
            </button>

            {/* File upload (site terminals only) */}
            {selectedSite && (
              <label className="py-1.5 bg-dark-700 text-dark-200 rounded text-xs font-mono cursor-pointer hover:bg-dark-600 active:bg-dark-500 touch-manipulation text-center">
                Upload
                <input
                  type="file"
                  className="hidden"
                  onChange={async (e) => {
                    const file = e.target.files?.[0];
                    if (!file || !selectedSite) return;
                    const reader = new FileReader();
                    reader.onload = async () => {
                      const base64 = (reader.result as string).split(",")[1];
                      try {
                        await api.post(`/sites/${selectedSite}/files/upload`, {
                          path: "",
                          filename: file.name,
                          content: base64,
                        });
                        setError("");
                        if (wsRef.current && wsRef.current.readyState === WebSocket.OPEN) {
                          wsRef.current.send(
                            JSON.stringify({
                              type: "input",
                              data: `echo "Uploaded: ${file.name}"\n`,
                            })
                          );
                        }
                        setStatus(`Uploaded: ${file.name}`);
                        setTimeout(() => setStatus(""), 3000);
                      } catch {
                        setError(`Upload failed: ${file.name}`);
                      }
                    };
                    reader.readAsDataURL(file);
                    e.target.value = "";
                  }}
                />
              </label>
            )}

            {/* Copy Output */}
            <button
              onClick={() => {
                if (xtermRef.current) {
                  const buffer = xtermRef.current.buffer.active;
                  let text = "";
                  for (let i = 0; i < buffer.length; i++) {
                    const line = buffer.getLine(i);
                    if (line) text += line.translateToString(true) + "\n";
                  }
                  navigator.clipboard
                    .writeText(text.trimEnd())
                    .then(() => {
                      setError("");
                      setStatus("Terminal output copied to clipboard");
                      setTimeout(() => setStatus(""), 2000);
                    })
                    .catch(() => {
                      setError("Failed to copy to clipboard");
                    });
                }
              }}
              className="py-1.5 bg-dark-700 text-dark-200 rounded text-xs font-mono hover:bg-dark-600 active:bg-dark-500 touch-manipulation text-center"
              title="Copy all terminal output"
            >
              Copy Output
            </button>

            {/* Share */}
            <button
              onClick={async () => {
                if (!xtermRef.current) return;
                const buffer = xtermRef.current.buffer.active;
                let text = "";
                for (let i = 0; i < buffer.length; i++) {
                  const line = buffer.getLine(i);
                  if (line) text += line.translateToString(true) + "\n";
                }
                try {
                  const result = await api.post<{
                    share_id: string;
                    url: string;
                  }>("/terminal/share", { content: text.trimEnd() });
                  const url = `${window.location.origin}${result.url}`;
                  navigator.clipboard
                    .writeText(url)
                    .then(() => {
                      setError("");
                      setStatus("Share link copied! Expires in 1 hour");
                      setTimeout(() => setStatus(""), 3000);
                    })
                    .catch(() => {
                      setError("Failed to copy share link to clipboard");
                    });
                } catch {
                  setError("Failed to create share link");
                }
              }}
              className="py-1.5 bg-dark-700 text-dark-200 rounded text-xs font-mono hover:bg-dark-600 active:bg-dark-500 touch-manipulation text-center"
              title="Share terminal output (1hr link)"
            >
              Share
            </button>

            {/* SSH Info */}
            <button
              onClick={() => setShowSshInfo(!showSshInfo)}
              className={`px-2 py-1 rounded text-xs transition-colors ${
                showSshInfo
                  ? "bg-rust-500/20 text-rust-400 border border-rust-500/30"
                  : "bg-dark-700 text-dark-200 hover:bg-dark-600"
              }`}
            >
              SSH Info
            </button>

            {/* Record */}
            <button
              onClick={toggleRecording}
              className={`px-2 py-1 rounded text-xs transition-colors ${
                recording
                  ? "bg-danger-400/20 text-danger-400 animate-pulse"
                  : "bg-dark-700 text-dark-200 hover:bg-dark-600"
              }`}
              title={recording ? "Stop recording and download .cast file" : "Record terminal session (asciinema format)"}
            >
              {recording ? "Stop Rec" : "Record"}
            </button>

            {/* Reconnect */}
            <button
              onClick={handleReconnect}
              className="py-1.5 bg-dark-700 text-dark-200 rounded text-xs font-mono hover:bg-dark-600 active:bg-dark-500 touch-manipulation text-center"
            >
              Reconnect
            </button>
            </div>
          </div>
        </div>

        {/* Snippets bar */}
        {showSnippets && (
          <div className="flex flex-wrap gap-1.5 px-3 py-2 bg-dark-900 border-b border-dark-600 shrink-0">
            {snippets.map((s) => (
              <button
                key={s.label}
                onClick={() => {
                  if (wsRef.current && wsRef.current.readyState === WebSocket.OPEN) {
                    // Paste command without executing — user presses Enter to confirm
                    wsRef.current.send(
                      JSON.stringify({ type: "input", data: s.cmd })
                    );
                  }
                }}
                className="px-2 py-1 bg-dark-700 text-dark-200 rounded text-[11px] font-mono hover:bg-dark-600 hover:text-dark-50 transition-colors"
                title={`${s.cmd} (pastes into terminal — press Enter to run)`}
              >
                {s.label}
              </button>
            ))}
          </div>
        )}

        {/* SSH Info panel */}
        {showSshInfo && (
          <div className="px-3 py-2 bg-dark-900 border-b border-dark-600 space-y-1.5 shrink-0">
            <p className="text-xs text-dark-300 uppercase font-mono tracking-widest mb-1">
              SSH Connection
            </p>
            <div className="flex items-center gap-2 text-xs">
              <span className="text-dark-300 w-16">Host:</span>
              <code className="text-dark-100 font-mono bg-dark-700 px-2 py-0.5 rounded">
                {window.location.hostname}
              </code>
              <button
                onClick={() =>
                  navigator.clipboard
                    .writeText(window.location.hostname)
                    .catch(() => setError("Failed to copy"))
                }
                className="text-dark-400 hover:text-dark-100"
              >
                Copy
              </button>
            </div>
            <div className="flex items-center gap-2 text-xs">
              <span className="text-dark-300 w-16">Port:</span>
              <code className="text-dark-100 font-mono bg-dark-700 px-2 py-0.5 rounded">
                22
              </code>
            </div>
            <div className="flex items-center gap-2 text-xs">
              <span className="text-dark-300 w-16">User:</span>
              <code className="text-dark-100 font-mono bg-dark-700 px-2 py-0.5 rounded">
                root
              </code>
            </div>
            <div className="flex items-center gap-2 text-xs">
              <span className="text-dark-300 w-16">Command:</span>
              <code className="text-dark-100 font-mono bg-dark-700 px-2 py-0.5 rounded">
                ssh root@{window.location.hostname}
              </code>
              <button
                onClick={() =>
                  navigator.clipboard
                    .writeText(`ssh root@${window.location.hostname}`)
                    .catch(() => setError("Failed to copy"))
                }
                className="text-dark-400 hover:text-dark-100"
              >
                Copy
              </button>
            </div>
            {selectedSite && (
              <div className="flex items-center gap-2 text-xs">
                <span className="text-dark-300 w-16">Site dir:</span>
                <code className="text-dark-100 font-mono bg-dark-700 px-2 py-0.5 rounded">
                  /var/www/
                  {sites.find((s) => s.id === selectedSite)?.domain || ""}
                </code>
              </div>
            )}
          </div>
        )}

        {error && (
          <div className="px-6 py-2 bg-danger-500/10 text-danger-400 text-sm border-b border-danger-500/20 shrink-0 flex items-center justify-between">
            <span>{error}</span>
            <button
              onClick={() => setError("")}
              className="text-danger-400 hover:text-danger-300 ml-4 text-xs"
            >
              Dismiss
            </button>
          </div>
        )}

        {/* Terminal */}
        <div className="flex-1 p-2 min-h-0 relative overflow-hidden" style={{ backgroundColor: currentThemeBg }}>
          {/* Search overlay */}
          {showSearch && (
            <div className="absolute top-0 right-0 m-2 flex items-center gap-1 bg-dark-800 border border-dark-500 rounded-lg p-1.5 shadow-lg z-10">
              <input
                ref={searchInputRef}
                value={searchTerm}
                onChange={(e) => setSearchTerm(e.target.value)}
                onKeyDown={(e) => {
                  if (e.key === "Enter") {
                    handleSearch(e.shiftKey ? "prev" : "next");
                  }
                  if (e.key === "Escape") {
                    setShowSearch(false);
                    xtermRef.current?.focus();
                  }
                }}
                placeholder="Search..."
                autoFocus
                className="px-2 py-1 bg-dark-900 border border-dark-600 rounded text-xs w-40 outline-none text-dark-100 placeholder-dark-400"
              />
              <button
                onClick={() => handleSearch("prev")}
                className="text-dark-300 hover:text-dark-100 text-xs px-1"
                title="Previous (Shift+Enter)"
              >
                ↑
              </button>
              <button
                onClick={() => handleSearch("next")}
                className="text-dark-300 hover:text-dark-100 text-xs px-1"
                title="Next (Enter)"
              >
                ↓
              </button>
              <button
                onClick={() => {
                  setShowSearch(false);
                  xtermRef.current?.focus();
                }}
                className="text-dark-300 hover:text-dark-100 text-xs px-1"
              >
                ×
              </button>
            </div>
          )}
          <div ref={termRef} className="h-full" />
        </div>

        {/* Mobile action bar — keyboard-like layout */}
        <div className="bg-dark-800 border-t border-dark-500 shrink-0 md:hidden px-1.5 py-1.5 space-y-1">
          {/* Row 1: Esc, Tab, arrows cluster, Paste */}
          <div className="grid grid-cols-7 gap-1">
            {[
              { label: "Esc", key: "\x1b" },
              { label: "Tab", key: "\t" },
              { label: "←", key: "\x1b[D" },
              { label: "↑", key: "\x1b[A" },
              { label: "↓", key: "\x1b[B" },
              { label: "→", key: "\x1b[C" },
              { label: "Paste", key: "__paste__" },
            ].map((btn) => (
              <button key={btn.label} onClick={async () => {
                if (!wsRef.current || wsRef.current.readyState !== WebSocket.OPEN) return;
                if (btn.key === "__paste__") {
                  try { const text = await navigator.clipboard.readText(); wsRef.current.send(JSON.stringify({ type: "input", data: text })); } catch { setError("Clipboard access denied"); }
                } else { wsRef.current.send(JSON.stringify({ type: "input", data: btn.key })); }
                xtermRef.current?.focus();
              }} className="py-2 bg-dark-700 text-dark-200 rounded text-[11px] font-mono font-medium active:bg-dark-500 touch-manipulation text-center">{btn.label}</button>
            ))}
          </div>
          {/* Row 2: Ctrl combos + Enter (wider) */}
          <div className="grid grid-cols-5 gap-1">
            {[
              { label: "Ctrl+C", key: "\x03" },
              { label: "Ctrl+D", key: "\x04" },
              { label: "Ctrl+Z", key: "\x1a" },
              { label: "Ctrl+L", key: "\x0c" },
            ].map((btn) => (
              <button key={btn.label} onClick={() => {
                if (!wsRef.current || wsRef.current.readyState !== WebSocket.OPEN) return;
                wsRef.current.send(JSON.stringify({ type: "input", data: btn.key }));
                xtermRef.current?.focus();
              }} className="py-2 bg-dark-700 text-dark-200 rounded text-[11px] font-mono font-medium active:bg-dark-500 touch-manipulation text-center">{btn.label}</button>
            ))}
            <button onClick={() => {
              if (!wsRef.current || wsRef.current.readyState !== WebSocket.OPEN) return;
              wsRef.current.send(JSON.stringify({ type: "input", data: "\r" }));
              xtermRef.current?.focus();
            }} className="py-2 bg-rust-600 text-white rounded text-[11px] font-mono font-bold active:bg-rust-700 touch-manipulation text-center">Enter</button>
          </div>
        </div>
      </div>
    </div>
  );
}
