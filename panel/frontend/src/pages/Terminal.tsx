import { useState, useEffect, useRef, useCallback } from "react";
import { useSearchParams } from "react-router-dom";
import { Terminal as XTerm, type ITheme } from "@xterm/xterm";
import { FitAddon } from "@xterm/addon-fit";
import { SearchAddon } from "@xterm/addon-search";
import "@xterm/xterm/css/xterm.css";
import { api, ApiError } from "../api";
import { useAuth } from "../context/AuthContext";

interface Site {
  id: string;
  domain: string;
}

// ── Terminal themes ──
//
// Typed ITheme, not Record<string, string>: xterm ignores a key it does not
// recognise, so `selection` for `selectionBackground` compiled clean and did
// nothing. That is how cursorAccent and selectionInactiveBackground came to be
// missing from all three — nothing could tell us they were absent.
//
// cursorAccent is the GLYPH under a block cursor. Unset, xterm uses #000000, so
// the light theme drew a black character inside a blue block. It belongs to the
// background in every theme, which is what "inverted cell" means.
//
// scrollbarSliderBackground is otherwise derived as foreground at 20% alpha,
// which measured 1.42:1 on light and 1.69:1 on mocha — a scrollbar you cannot
// see is a scrollbar that is not there.
const themes: Record<string, ITheme> = {
  mocha: {
    background: "#1e1e2e",
    foreground: "#cdd6f4",
    cursor: "#f5e0dc",
    cursorAccent: "#1e1e2e",
    selectionBackground: "#585b7066",
    selectionInactiveBackground: "#585b7033",
    scrollbarSliderBackground: "#585b7099",
    scrollbarSliderHoverBackground: "#6c7086cc",
    scrollbarSliderActiveBackground: "#7f849c",
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
    cursorAccent: "#282a36",
    selectionBackground: "#44475a66",
    selectionInactiveBackground: "#44475a33",
    scrollbarSliderBackground: "#6272a499",
    scrollbarSliderHoverBackground: "#6272a4cc",
    scrollbarSliderActiveBackground: "#7b8cc4",
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
    cursor: "#526fff",
    cursorAccent: "#fafafa",
    selectionBackground: "#d0d0d066",
    selectionInactiveBackground: "#d0d0d033",
    scrollbarSliderBackground: "#a0a1a799",
    scrollbarSliderHoverBackground: "#8a8b91cc",
    scrollbarSliderActiveBackground: "#696c77",
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
    // Was #fafafa — byte-identical to this theme's own background, so `\e[97m`
    // text was invisible at a contrast ratio of exactly 1.00:1. No palette slot
    // may hold its own background colour.
    brightWhite: "#383a42",
  },
};

// ── Saved command snippets ──
// Every one of these is a ROOT command against the host. A site shell is a
// different machine as far as its occupant is concerned: it runs as www-data,
// under `bash --restricted`, with NO_NEW_PRIVS set and a command blocklist —
// so `systemctl restart nginx` and `docker ps` are not merely unhelpful there,
// they are refused. Offering them on a site session is a menu of failures.
const serverSnippets = [
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

// Things that actually work from inside /var/www/<domain> as www-data.
const siteSnippets = [
  { label: "List Files", cmd: "ls -lah" },
  { label: "Folder Size", cmd: "du -sh ." },
  { label: "Largest Files", cmd: "du -ah . | sort -rh | head -20" },
  { label: "Find Recently Changed", cmd: "find . -type f -mtime -1" },
  { label: "PHP Version", cmd: "php -v" },
  { label: "WP-CLI Info", cmd: "wp --info" },
  { label: "Composer Install", cmd: "composer install --no-dev" },
  { label: "Search In Files", cmd: "grep -rn 'needle' . | head -20" },
];

const IDLE_TIMEOUT = 30 * 60 * 1000; // 30 minutes
const MAX_RECONNECT_ATTEMPTS = 3;

export default function Terminal() {
  const [searchParams] = useSearchParams();
  const initialSiteId = searchParams.get("site") || "";
  const { user } = useAuth();
  // The server shell is admin-only and stays that way: `terminal.rs` refuses a
  // ticket with no site to anybody else, and v2.75.0 bound that decision into
  // the signed ticket precisely because a site owner could previously drop the
  // scope and land in a root shell. What this page got wrong was not the gate —
  // it was arriving at the gate by default. A site owner already has a terminal
  // on the sites they own; this page just never opened it for them.
  const isAdmin = user?.role === "admin";
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
  // A deliberate answer from the panel (feature switched off, or the selected
  // server is a fleet member) rather than something that went wrong.
  const [notice, setNotice] = useState("");
  const [sites, setSites] = useState<Site[]>([]);
  const [sitesLoaded, setSitesLoaded] = useState(false);
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
    api
      .get<Site[]>("/sites")
      .then(setSites)
      .catch(() => setError("Failed to load sites"))
      // Distinguishes "no sites" from "not asked yet" — without it the
      // owns-nothing notice below fires for a moment on every load.
      .finally(() => setSitesLoaded(true));
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
      setNotice("");
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

      // xterm measures cell width from the computed font at construction time.
      // JetBrains Mono arrives over the network, so a cold load of /terminal can
      // measure the fallback face and produce a grid whose columns are wrong
      // until something forces a refit. Resolves instantly once the font is in.
      if (document.fonts?.ready) {
        try {
          await document.fonts.ready;
        } catch {
          /* a font-loading failure must not stop the terminal opening */
        }
      }

      // Create terminal
      const term = new XTerm({
        cursorBlink: true,
        fontSize,
        // Matches --font-mono in index.css. The two stacks had drifted, so a box
        // without the webfont rendered the terminal and the chrome around it in
        // two different faces.
        fontFamily:
          "'JetBrains Mono', ui-monospace, 'Cascadia Code', 'Source Code Pro', Menlo, monospace",
        // 1.0 is xterm's default and is tight for a long log session.
        lineHeight: 1.2,
        // The default is 1000 lines. `journalctl -n 2000` silently lost its top,
        // and both Copy Output and Share iterate this same buffer, so a shared
        // link was truncated without saying so.
        scrollback: 10000,
        // The single highest-value option here. xterm raises any foreground that
        // fails this against its own cell background, at render time, for every
        // theme at once. Measured before: brightBlack — git hashes, systemd
        // timestamps, most secondary CLI output — was 2.46:1 on mocha and 3.03:1
        // on dracula, and dim (\e[2m) halved both again. Fixing it here rather
        // than by editing palette entries keeps all three themes faithful to
        // their upstream originals.
        minimumContrastRatio: 4.5,
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
          try {
            if (ws.readyState === WebSocket.OPEN) {
              ws.send(JSON.stringify({ type: "resize", cols, rows }));
            }
          } catch {
            // Socket may have closed between readyState check and send
          }
        });
      } catch (e) {
        const message = e instanceof Error ? e.message : "Failed to connect";

        // 403 (the terminal is switched off) and 501 (a fleet member is
        // selected) are the panel answering a question, not a fault. Painting
        // them in the red fault colour beside a Reconnect button invited the
        // operator to retry a decision. Say what it is and where it lives.
        const configured = e instanceof ApiError && (e.status === 403 || e.status === 501);
        setNotice(configured ? message : "");
        setError(configured ? "" : message);

        term.writeln(
          configured
            ? `\r\n\x1b[33m● ${message}\x1b[0m`
            : `\r\n\x1b[31m● Error: ${message}\x1b[0m`
        );
      }
    },
    [fontSize, themeName, resetIdleTimer]
  );

  // Connect on mount.
  //
  // A non-admin with no `?site=` has nothing to connect to YET — their session
  // is a site session and the site list has not arrived. Dialling anyway is what
  // produced the report: sidebar → Terminal → an immediate "Admin access required
  // for server terminal", from an account that does in fact have a terminal. The
  // effect below picks their first site the moment the list lands.
  useEffect(() => {
    // Conditional CONNECT, unconditional cleanup — an early return here would
    // skip registering the teardown that disposes the xterm the auto-select
    // effect goes on to create.
    if (isAdmin || selectedSite) connect(selectedSite || undefined);
    return () => {
      if (idleTimer.current) clearTimeout(idleTimer.current);
      if (reconnectTimer.current) clearTimeout(reconnectTimer.current);
      intentionalClose.current = true;
      wsRef.current?.close();
      xtermRef.current?.dispose();
    };
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, []);

  // A non-admin's default session is their own first site, chosen once the list
  // arrives rather than at mount (it is empty at mount). `/sites` is already
  // scoped to the caller, so "the first site" is always one they own.
  // Must live BELOW `connect` — a dependency array naming it any earlier is
  // evaluated during render, before the useCallback has initialised.
  useEffect(() => {
    if (isAdmin || selectedSite || sites.length === 0) return;
    setSelectedSite(sites[0].id);
    connect(sites[0].id);
  }, [isAdmin, selectedSite, sites, connect]);

  // ...and if they own none, say so rather than leaving a dead black rectangle.
  useEffect(() => {
    if (isAdmin || selectedSite || !sitesLoaded || sites.length > 0) return;
    setNotice(
      "This terminal opens a shell inside a site you own. No site is assigned to your account yet — once one is, it will appear in the selector above."
    );
  }, [isAdmin, selectedSite, sitesLoaded, sites]);

  // Handle resize
  //
  // This was a bare window "resize" listener, which misses every way this
  // terminal actually changes size. The Glass sidebar expands ON HOVER
  // (GlassLayout.tsx: md:w-16 ↔ md:w-56, still in flow), so pointing at the nav
  // swings the terminal's width by 160px with no window event at all. Vertically
  // the same happens whenever Snippets, SSH Info, the notice banner, the error
  // banner or the mobile More toolbar appears. The PTY kept the stale cols/rows,
  // so wrapped output tore and any full-screen TUI — top, tmux, nano, htop —
  // stayed corrupt until you resized the browser itself.
  //
  // Observing the container catches all of it, window resizes included.
  useEffect(() => {
    const el = termRef.current;
    if (!el) return;

    let frame = 0;
    const refit = () => {
      cancelAnimationFrame(frame);
      // Coalesce the burst a CSS transition emits, and never fit a collapsed
      // box: FitAddon divides by cell size, and a zero-height container yields
      // a nonsense geometry that gets sent to the PTY as a real resize.
      frame = requestAnimationFrame(() => {
        if (el.clientWidth > 0 && el.clientHeight > 0) fitRef.current?.fit();
      });
    };

    const ro = new ResizeObserver(refit);
    ro.observe(el);
    window.addEventListener("resize", refit);
    return () => {
      cancelAnimationFrame(frame);
      ro.disconnect();
      window.removeEventListener("resize", refit);
    };
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
    // Without decorations a hit is merely SELECTED, and the selection colour
    // here is a low-alpha grey — about a 1.8:1 shift against the background. In
    // a wall of log output that is close to invisible, which is the whole reason
    // you opened the search. Derived from the active theme so the highlight
    // belongs to it rather than being a fourth hardcoded colour.
    const t = themes[themeName] || themes.mocha;
    const opts = {
      decorations: {
        matchBackground: `${t.yellow}55`,
        matchBorder: t.yellow as string,
        matchOverviewRuler: t.yellow as string,
        activeMatchBackground: `${t.cursor}99`,
        activeMatchBorder: t.cursor as string,
        activeMatchColorOverviewRuler: t.cursor as string,
      },
    };
    if (direction === "next") {
      searchAddonRef.current.findNext(searchTerm, opts);
    } else {
      searchAddonRef.current.findPrevious(searchTerm, opts);
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
                {/* The default option was the one shell a non-admin can never
                    open, so the page refused itself before they touched it. */}
                {isAdmin && <option value="">Server root</option>}
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
                          const safeName = file.name.replace(/[^a-zA-Z0-9._\- ]/g, '_');
                          wsRef.current.send(
                            JSON.stringify({
                              type: "input",
                              data: `echo "Uploaded: ${safeName}"\n`,
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

            {/* Share — administrator-only, like the SSH Info button below it. The
                endpoint calls `require_admin`, so for anyone else this offered a
                link that could only ever answer "Failed to create share link".
                Copy Output above is deliberately NOT gated: it never leaves the
                browser. */}
            {isAdmin && <button
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
            </button>}

            {/* SSH Info — the credentials it prints are for the ROOT account on
                the host (`server_utils.rs` reads /root/.ssh/authorized_keys), so
                it is an operator panel, not a tenant one. */}
            {isAdmin && <button
              onClick={() => setShowSshInfo(!showSshInfo)}
              className={`px-2 py-1 rounded text-xs transition-colors ${
                showSshInfo
                  ? "bg-rust-500/20 text-rust-400 border border-rust-500/30"
                  : "bg-dark-700 text-dark-200 hover:bg-dark-600"
              }`}
            >
              SSH Info
            </button>}

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
            {/* Keyed on the SESSION, not the role: an admin who selects a site is
                in the same restricted www-data shell a client is, and wants the
                same commands. */}
            {(selectedSite ? siteSnippets : serverSnippets).map((s) => (
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

        {/* SSH Info panel — admin only, same reason as the button that opens it. */}
        {isAdmin && showSshInfo && (
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

        {/* warn-*, not stock amber-*. The warn tokens are re-tuned per theme;
            amber is not, so on the light panel themes this banner measured
            1.19:1 — invisible. It is exactly what an owner with no site assigned
            sees, so it was the one banner that had to be readable. */}
        {notice && (
          <div className="px-6 py-2 bg-warn-500/10 text-warn-400 text-sm border-b border-warn-500/20 shrink-0 flex items-center justify-between">
            <span>{notice}</span>
            <button
              onClick={() => setNotice("")}
              className="text-warn-400 hover:text-warn-500 ml-4 text-xs"
            >
              Dismiss
            </button>
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
        {/* focus-within, because there is otherwise NO signal that keystrokes
            will reach the shell: xterm sets .xterm:focus { outline: none }, the
            element that actually takes focus is an off-screen helper textarea,
            and this wrapper is not focusable — so the panel's global
            :focus-visible ring had nothing to land on. */}
        <div
          className="flex-1 p-2 min-h-0 relative overflow-hidden focus-within:ring-2 focus-within:ring-inset focus-within:ring-accent-500"
          style={{ backgroundColor: currentThemeBg }}
        >
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
