import { useState, useEffect, useRef } from "react";
import { useSearchParams, Link } from "react-router-dom";
import { Terminal as XTerm } from "@xterm/xterm";
import { FitAddon } from "@xterm/addon-fit";
import "@xterm/xterm/css/xterm.css";
import { api } from "../api";

interface Site {
  id: string;
  domain: string;
}

export default function Terminal() {
  const [searchParams] = useSearchParams();
  const siteId = searchParams.get("site");
  const termRef = useRef<HTMLDivElement>(null);
  const wsRef = useRef<WebSocket | null>(null);
  const xtermRef = useRef<XTerm | null>(null);
  const fitRef = useRef<FitAddon | null>(null);
  const [connected, setConnected] = useState(false);
  const [error, setError] = useState("");
  const [sites, setSites] = useState<Site[]>([]);
  const [selectedSite, setSelectedSite] = useState(siteId || "");

  // Load sites for selector
  useEffect(() => {
    api.get<Site[]>("/sites").then(setSites).catch(() => setError("Failed to load sites"));
  }, []);

  const connect = async (siteIdParam?: string) => {
    if (!termRef.current) return;
    setError("");

    // Cleanup previous
    if (wsRef.current) {
      wsRef.current.close();
      wsRef.current = null;
    }
    if (xtermRef.current) {
      xtermRef.current.dispose();
      xtermRef.current = null;
    }

    // Create terminal
    const term = new XTerm({
      cursorBlink: true,
      fontSize: 14,
      fontFamily: "'JetBrains Mono', 'Fira Code', 'Cascadia Code', monospace",
      theme: {
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
    });

    const fit = new FitAddon();
    term.loadAddon(fit);
    term.open(termRef.current);
    fit.fit();
    xtermRef.current = term;
    fitRef.current = fit;

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
      // WebSocket API doesn't support Authorization headers; token is short-lived and same-origin
      const wsUrl = `${proto}//${window.location.host}/agent/terminal/ws?token=${data.token}&domain=${encodeURIComponent(domain)}&cols=${cols}&rows=${rows}`;

      const ws = new WebSocket(wsUrl);
      wsRef.current = ws;

      ws.onopen = () => {
        setConnected(true);
        term.clear();
      };

      ws.onmessage = (event) => {
        term.write(event.data);
      };

      ws.onclose = () => {
        setConnected(false);
        term.writeln("\r\n\x1b[31m● Connection closed\x1b[0m");
      };

      ws.onerror = () => {
        setError("WebSocket connection failed");
        setConnected(false);
      };

      // Send terminal input to WebSocket
      term.onData((data) => {
        if (ws.readyState === WebSocket.OPEN) {
          ws.send(
            JSON.stringify({ type: "input", data })
          );
        }
      });

      // Handle resize
      term.onResize(({ cols, rows }) => {
        if (ws.readyState === WebSocket.OPEN) {
          ws.send(
            JSON.stringify({ type: "resize", cols, rows })
          );
        }
      });
    } catch (e) {
      setError(e instanceof Error ? e.message : "Failed to connect");
      term.writeln(`\r\n\x1b[31m● Error: ${e instanceof Error ? e.message : "Connection failed"}\x1b[0m`);
    }
  };

  // Connect on mount or site change
  useEffect(() => {
    if (selectedSite) {
      connect(selectedSite);
    } else {
      connect();
    }
    return () => {
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
    connect(newSiteId || undefined);
  };

  return (
    <div className="flex flex-col h-full">
      {/* Header */}
      <div className="px-6 py-4 border-b border-dark-500 bg-dark-800 flex items-center justify-between shrink-0">
        <div className="flex items-center gap-4">
          <div>
            <h1 className="text-lg font-semibold text-dark-50">Terminal</h1>
            <div className="flex items-center gap-2 mt-0.5">
              <div
                className={`w-2 h-2 rounded-full ${
                  connected ? "bg-emerald-500" : "bg-gray-300"
                }`}
              />
              <span className="text-xs text-dark-200">
                {connected ? "Connected" : "Disconnected"}
              </span>
            </div>
          </div>
        </div>
        <div className="flex items-center gap-3">
          <select
            value={selectedSite}
            onChange={(e) => handleSiteChange(e.target.value)}
            className="text-sm border border-dark-500 rounded-lg px-3 py-1.5 bg-dark-800 focus:ring-2 focus:ring-rust-500 focus:border-rust-500"
          >
            <option value="">Server root</option>
            {sites.map((s) => (
              <option key={s.id} value={s.id}>
                {s.domain}
              </option>
            ))}
          </select>
          <button
            onClick={() => connect(selectedSite || undefined)}
            className="px-3 py-1.5 bg-dark-700 text-dark-100 rounded-lg text-sm hover:bg-gray-200 transition-colors"
          >
            Reconnect
          </button>
        </div>
      </div>

      {error && (
        <div className="px-6 py-2 bg-red-500/10 text-red-400 text-sm border-b border-red-500/20">
          {error}
        </div>
      )}

      {/* Terminal */}
      <div className="flex-1 bg-[#1e1e2e] p-2">
        <div ref={termRef} className="h-full" />
      </div>
    </div>
  );
}
