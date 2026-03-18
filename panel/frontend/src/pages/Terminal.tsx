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

interface TermTab {
  id: string;
  label: string;
  siteId: string;
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

export default function Terminal() {
  const [searchParams] = useSearchParams();
  const initialSiteId = searchParams.get("site") || "";
  const termRef = useRef<HTMLDivElement>(null);
  const wsRef = useRef<WebSocket | null>(null);
  const xtermRef = useRef<XTerm | null>(null);
  const fitRef = useRef<FitAddon | null>(null);
  const searchAddonRef = useRef<SearchAddon | null>(null);
  const idleTimer = useRef<ReturnType<typeof setTimeout>>(undefined);

  const [connected, setConnected] = useState(false);
  const [status, setStatus] = useState("");
  const [error, setError] = useState("");
  const [sites, setSites] = useState<Site[]>([]);
  const [selectedSite, setSelectedSite] = useState(initialSiteId);

  // Tabs
  const [tabs, setTabs] = useState<TermTab[]>([
    { id: "default", label: initialSiteId ? "Site" : "Server", siteId: initialSiteId },
  ]);
  const [activeTab, setActiveTab] = useState("default");

  // Snippets
  const [showSnippets, setShowSnippets] = useState(false);

  // Font size (persisted)
  const [fontSize, setFontSize] = useState(() =>
    parseInt(localStorage.getItem("dp-terminal-font") || "14")
  );

  // Theme (persisted)
  const [themeName, setThemeName] = useState(
    () => localStorage.getItem("dp-terminal-theme") || "mocha"
  );

  // Search
  const [showSearch, setShowSearch] = useState(false);
  const [searchTerm, setSearchTerm] = useState("");
  const searchInputRef = useRef<HTMLInputElement>(null);

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

      // Cleanup previous
      if (wsRef.current) {
        wsRef.current.close();
        wsRef.current = null;
      }
      if (xtermRef.current) {
        xtermRef.current.dispose();
        xtermRef.current = null;
      }

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
          term.clear();
          resetIdleTimer();
        };

        ws.onmessage = (event) => {
          term.write(event.data);
        };

        ws.onclose = () => {
          setConnected(false);
          if (!status) {
            term.writeln("\r\n\x1b[31m● Connection closed\x1b[0m");
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
    [fontSize, themeName, resetIdleTimer, status]
  );

  // Connect on mount
  useEffect(() => {
    connect(selectedSite || undefined);
    return () => {
      if (idleTimer.current) clearTimeout(idleTimer.current);
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
    // Update active tab's siteId
    setTabs((prev) =>
      prev.map((t) => (t.id === activeTab ? { ...t, siteId: newSiteId } : t))
    );
    connect(newSiteId || undefined);
  };

  const switchTab = (tabId: string) => {
    const tab = tabs.find((t) => t.id === tabId);
    if (!tab || tabId === activeTab) return;
    setActiveTab(tabId);
    setSelectedSite(tab.siteId);
    connect(tab.siteId || undefined);
  };

  const addTab = () => {
    const id = Date.now().toString();
    const newTab: TermTab = { id, label: "New", siteId: "" };
    setTabs((prev) => [...prev, newTab]);
    setActiveTab(id);
    setSelectedSite("");
    connect();
  };

  const closeTab = (tabId: string) => {
    if (tabs.length <= 1) return;
    const remaining = tabs.filter((t) => t.id !== tabId);
    setTabs(remaining);
    if (activeTab === tabId) {
      const next = remaining[0];
      setActiveTab(next.id);
      setSelectedSite(next.siteId);
      connect(next.siteId || undefined);
    }
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

  const currentThemeBg = (themes[themeName] || themes.mocha).background;

  return (
    <div className="flex flex-col h-full p-4">
      <div className="flex flex-col flex-1 border border-dark-500 min-h-0">
        {/* Header */}
        <div className="px-5 py-3 border-b border-dark-500 bg-dark-800 flex items-center justify-between shrink-0">
          <div className="flex items-center gap-4">
            <div>
              <h1 className="text-sm font-medium text-dark-300 uppercase font-mono tracking-widest">
                Terminal
              </h1>
              <div className="flex items-center gap-2 mt-0.5">
                <div
                  className={`w-2 h-2 rounded-full ${
                    connected ? "bg-rust-500" : "bg-gray-300"
                  }`}
                />
                <span className="text-xs text-dark-200">
                  {status || (connected ? "Connected" : "Disconnected")}
                </span>
              </div>
            </div>
          </div>
          <div className="flex items-center gap-3">
            {/* Font size controls */}
            <div className="flex items-center gap-1">
              <button
                onClick={() => changeFontSize(-1)}
                className="px-1.5 py-0.5 bg-dark-700 text-dark-200 rounded text-xs hover:bg-dark-600 transition-colors"
              >
                A-
              </button>
              <span className="text-xs text-dark-300 font-mono w-6 text-center">
                {fontSize}
              </span>
              <button
                onClick={() => changeFontSize(1)}
                className="px-1.5 py-0.5 bg-dark-700 text-dark-200 rounded text-xs hover:bg-dark-600 transition-colors"
              >
                A+
              </button>
            </div>

            {/* Theme selector */}
            <select
              value={themeName}
              onChange={(e) => changeTheme(e.target.value)}
              className="px-2 py-1 bg-dark-700 text-dark-200 rounded text-xs border border-dark-600"
            >
              <option value="mocha">Mocha</option>
              <option value="dracula">Dracula</option>
              <option value="light">Light</option>
            </select>

            {/* Site selector */}
            <select
              value={selectedSite}
              onChange={(e) => handleSiteChange(e.target.value)}
              className="text-sm border border-dark-500 rounded-lg px-3 py-1.5 bg-dark-800 focus:ring-2 focus:ring-accent-500 focus:border-accent-500"
            >
              <option value="">Server root</option>
              {sites.map((s) => (
                <option key={s.id} value={s.id}>
                  {s.domain}
                </option>
              ))}
            </select>

            {/* Snippets toggle */}
            <button
              onClick={() => setShowSnippets(!showSnippets)}
              className={`px-3 py-1.5 rounded-lg text-sm transition-colors ${
                showSnippets
                  ? "bg-rust-500/20 text-rust-400 border border-rust-500/30"
                  : "bg-dark-700 text-dark-100 hover:bg-dark-600"
              }`}
            >
              Snippets
            </button>

            {/* File upload (site terminals only) */}
            {selectedSite && (
              <label className="px-3 py-1.5 bg-dark-700 text-dark-100 rounded-lg text-sm cursor-pointer hover:bg-dark-600 transition-colors">
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
                        if (wsRef.current && wsRef.current.readyState === WebSocket.OPEN) {
                          wsRef.current.send(
                            JSON.stringify({
                              type: "input",
                              data: `echo "Uploaded: ${file.name}"\n`,
                            })
                          );
                        }
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

            {/* Reconnect */}
            <button
              onClick={() => connect(selectedSite || undefined)}
              className="px-3 py-1.5 bg-dark-700 text-dark-100 rounded-lg text-sm hover:bg-dark-600 transition-colors"
            >
              Reconnect
            </button>
          </div>
        </div>

        {/* Tab bar */}
        <div className="flex items-center border-b border-dark-600 bg-dark-800/50 shrink-0">
          <div className="flex gap-0.5 px-2 overflow-x-auto">
            {tabs.map((tab) => (
              <div
                key={tab.id}
                className={`flex items-center gap-1 px-3 py-1.5 text-xs cursor-pointer border-b-2 transition-colors shrink-0 ${
                  activeTab === tab.id
                    ? "border-rust-400 text-rust-400"
                    : "border-transparent text-dark-300 hover:text-dark-100"
                }`}
              >
                <button onClick={() => switchTab(tab.id)} className="font-mono">
                  {tab.label}
                  {tab.siteId
                    ? ` (${sites.find((s) => s.id === tab.siteId)?.domain || "site"})`
                    : ""}
                </button>
                {tabs.length > 1 && (
                  <button
                    onClick={(e) => {
                      e.stopPropagation();
                      closeTab(tab.id);
                    }}
                    className="text-dark-400 hover:text-danger-400 ml-1"
                  >
                    ×
                  </button>
                )}
              </div>
            ))}
          </div>
          <button
            onClick={addTab}
            className="px-2 py-1.5 text-dark-400 hover:text-dark-100 text-xs shrink-0"
            title="New tab"
          >
            +
          </button>
        </div>

        {/* Snippets bar */}
        {showSnippets && (
          <div className="flex flex-wrap gap-1.5 px-3 py-2 bg-dark-900 border-b border-dark-600 shrink-0">
            {snippets.map((s) => (
              <button
                key={s.label}
                onClick={() => {
                  if (wsRef.current && wsRef.current.readyState === WebSocket.OPEN) {
                    wsRef.current.send(
                      JSON.stringify({ type: "input", data: s.cmd + "\n" })
                    );
                  }
                }}
                className="px-2 py-1 bg-dark-700 text-dark-200 rounded text-[11px] font-mono hover:bg-dark-600 hover:text-dark-50 transition-colors"
                title={s.cmd}
              >
                {s.label}
              </button>
            ))}
          </div>
        )}

        {error && (
          <div className="px-6 py-2 bg-red-500/10 text-danger-400 text-sm border-b border-red-500/20 shrink-0">
            {error}
          </div>
        )}

        {/* Terminal */}
        <div className="flex-1 p-2 min-h-0 relative" style={{ backgroundColor: currentThemeBg }}>
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
      </div>
    </div>
  );
}
