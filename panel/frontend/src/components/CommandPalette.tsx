import { useState, useEffect, useRef, useCallback } from "react";
import { useNavigate } from "react-router-dom";

interface Command {
  id: string;
  label: string;
  description?: string;
  icon: string;
  action: () => void;
  category: string;
  keywords?: string;
}

export default function CommandPalette() {
  const [open, setOpen] = useState(false);
  const [query, setQuery] = useState("");
  const [selectedIndex, setSelectedIndex] = useState(0);
  const inputRef = useRef<HTMLInputElement>(null);
  const listRef = useRef<HTMLDivElement>(null);
  const navigate = useNavigate();

  const go = useCallback((path: string) => {
    navigate(path);
    setOpen(false);
  }, [navigate]);

  const commands: Command[] = [
    // Navigation
    { id: "dashboard", label: "Dashboard", icon: "M3 12l2-2m0 0l7-7 7 7M5 10v10a1 1 0 001 1h3m10-11l2 2m-2-2v10a1 1 0 01-1 1h-3m-4 0h4", action: () => go("/"), category: "Navigate", keywords: "home overview health" },
    { id: "sites", label: "Sites", icon: "M12 21a9 9 0 100-18 9 9 0 000 18z", action: () => go("/sites"), category: "Navigate", keywords: "websites domains nginx" },
    { id: "databases", label: "Databases", icon: "M4 7v10c0 2.21 3.582 4 8 4s8-1.79 8-4V7", action: () => go("/databases"), category: "Navigate", keywords: "mysql postgres sql" },
    { id: "apps", label: "Docker Apps", icon: "M21 7.5l-9-5.25L3 7.5m18 0l-9 5.25m9-5.25v9l-9 5.25M3 7.5l9 5.25M3 7.5v9l9 5.25", action: () => go("/apps"), category: "Navigate", keywords: "containers docker deploy template" },
    { id: "terminal", label: "Terminal", icon: "M8 9l3 3-3 3m5 0h3M5 20h14a2 2 0 002-2V6a2 2 0 00-2-2H5a2 2 0 00-2 2v12a2 2 0 002 2z", action: () => go("/terminal"), category: "Navigate", keywords: "ssh console shell bash" },
    { id: "logs", label: "Logs", icon: "M19.5 14.25v-2.625a3.375 3.375 0 00-3.375-3.375h-1.5", action: () => go("/logs"), category: "Navigate", keywords: "system access error nginx" },
    { id: "security", label: "Security", icon: "M9 12.75L11.25 15 15 9.75m-3-7.036A11.959 11.959 0 013.598 6", action: () => go("/security"), category: "Navigate", keywords: "firewall fail2ban ssh hardening" },
    { id: "diagnostics", label: "Diagnostics", icon: "M9.75 3.104v5.714a2.25 2.25 0 01-.659 1.591L5 14.5", action: () => go("/diagnostics"), category: "Navigate", keywords: "health checks issues fixes" },
    { id: "settings", label: "Settings", icon: "M9.594 3.94c.09-.542.56-.94 1.11-.94h2.593", action: () => go("/settings"), category: "Navigate", keywords: "config smtp 2fa totp" },
    { id: "alerts", label: "Alerts", icon: "M14.857 17.082a23.848 23.848 0 005.454-1.31", action: () => go("/alerts"), category: "Navigate", keywords: "notifications monitoring rules" },
    { id: "monitors", label: "Monitors", icon: "M12 6v6h4.5m4.5 0a9 9 0 11-18 0 9 9 0 0118 0z", action: () => go("/monitors"), category: "Navigate", keywords: "uptime ping http" },
    { id: "servers", label: "Servers", icon: "M5 12h14M5 12a2 2 0 01-2-2V6a2 2 0 012-2h14", action: () => go("/servers"), category: "Navigate", keywords: "infrastructure machines agents" },
    { id: "dns", label: "DNS", icon: "M5.25 14.25h13.5m-13.5 0a3 3 0 01-3-3m3 3a3 3 0 100 6", action: () => go("/dns"), category: "Navigate", keywords: "records a cname mx" },
    { id: "users", label: "Users", icon: "M15 19.128a9.38 9.38 0 002.625.372", action: () => go("/users"), category: "Navigate", keywords: "accounts roles admin" },
    { id: "teams", label: "Teams", icon: "M18 18.72a9.094 9.094 0 003.741-.479", action: () => go("/teams"), category: "Navigate", keywords: "members invite access" },
    { id: "activity", label: "Activity", icon: "M12 6v6h4.5m4.5 0a9 9 0 11-18 0", action: () => go("/activity"), category: "Navigate", keywords: "audit log history" },
  ];

  const filtered = query
    ? commands.filter((c) => {
        const q = query.toLowerCase();
        return (
          c.label.toLowerCase().includes(q) ||
          c.category.toLowerCase().includes(q) ||
          (c.keywords && c.keywords.includes(q)) ||
          (c.description && c.description.toLowerCase().includes(q))
        );
      })
    : commands;

  // Reset selected index when results change
  useEffect(() => {
    setSelectedIndex(0);
  }, [query]);

  // Scroll selected item into view
  useEffect(() => {
    if (listRef.current) {
      const el = listRef.current.children[selectedIndex] as HTMLElement;
      if (el) el.scrollIntoView({ block: "nearest" });
    }
  }, [selectedIndex]);

  // Keyboard shortcut to open
  useEffect(() => {
    const handler = (e: KeyboardEvent) => {
      if ((e.metaKey || e.ctrlKey) && e.key === "k") {
        e.preventDefault();
        setOpen((prev) => !prev);
        setQuery("");
        setSelectedIndex(0);
      }
      if (e.key === "Escape" && open) {
        setOpen(false);
      }
    };
    window.addEventListener("keydown", handler);
    return () => window.removeEventListener("keydown", handler);
  }, [open]);

  // Focus input when opened
  useEffect(() => {
    if (open) {
      setTimeout(() => inputRef.current?.focus(), 50);
    }
  }, [open]);

  const handleKeyDown = (e: React.KeyboardEvent) => {
    if (e.key === "ArrowDown") {
      e.preventDefault();
      setSelectedIndex((i) => Math.min(i + 1, filtered.length - 1));
    } else if (e.key === "ArrowUp") {
      e.preventDefault();
      setSelectedIndex((i) => Math.max(i - 1, 0));
    } else if (e.key === "Enter" && filtered[selectedIndex]) {
      filtered[selectedIndex].action();
    }
  };

  if (!open) return null;

  // Group by category
  const grouped = filtered.reduce<Record<string, Command[]>>((acc, cmd) => {
    (acc[cmd.category] ??= []).push(cmd);
    return acc;
  }, {});

  let globalIdx = -1;

  return (
    <div
      className="fixed inset-0 z-[100] flex items-start justify-center pt-[15vh]"
      onClick={() => setOpen(false)}
    >
      <div className="fixed inset-0 bg-black/60 backdrop-blur-sm" />
      <div
        role="dialog"
        aria-modal="true"
        aria-label="Command palette"
        className="relative w-full max-w-lg mx-4 bg-dark-800 rounded-lg shadow-xl border border-dark-600 overflow-hidden"
        onClick={(e) => e.stopPropagation()}
      >
        {/* Search input */}
        <div className="flex items-center gap-3 px-4 border-b border-dark-600">
          <svg className="w-5 h-5 text-dark-300 shrink-0" fill="none" viewBox="0 0 24 24" stroke="currentColor" strokeWidth={2}>
            <circle cx="11" cy="11" r="8" />
            <path d="M21 21l-4.35-4.35" />
          </svg>
          <input
            ref={inputRef}
            type="text"
            value={query}
            onChange={(e) => setQuery(e.target.value)}
            onKeyDown={handleKeyDown}
            placeholder="Search pages, actions..."
            aria-label="Search commands"
            className="w-full py-3.5 bg-transparent text-sm text-dark-50 placeholder-dark-300 focus:outline-none"
          />
          <kbd className="hidden sm:inline-flex items-center gap-0.5 px-1.5 py-0.5 text-[10px] text-dark-300 border border-dark-500 rounded bg-dark-700/50 shrink-0">
            ESC
          </kbd>
        </div>

        {/* Results */}
        <div ref={listRef} className="max-h-80 overflow-y-auto py-2">
          {filtered.length === 0 ? (
            <div className="px-4 py-8 text-center text-sm text-dark-300">
              No results for &ldquo;{query}&rdquo;
            </div>
          ) : (
            Object.entries(grouped).map(([category, cmds]) => (
              <div key={category}>
                <div className="px-4 pt-2 pb-1 text-[10px] font-semibold uppercase tracking-wider text-dark-400">
                  {category}
                </div>
                {cmds.map((cmd) => {
                  globalIdx++;
                  const idx = globalIdx;
                  return (
                    <button
                      key={cmd.id}
                      onClick={cmd.action}
                      onMouseEnter={() => setSelectedIndex(idx)}
                      className={`w-full flex items-center gap-3 px-4 py-2.5 text-left text-sm transition-colors ${
                        selectedIndex === idx
                          ? "bg-rust-500/10 text-dark-50"
                          : "text-dark-200 hover:bg-dark-700/50"
                      }`}
                    >
                      <svg className="w-4 h-4 shrink-0 text-dark-300" fill="none" viewBox="0 0 24 24" stroke="currentColor" strokeWidth={1.5}>
                        <path strokeLinecap="round" strokeLinejoin="round" d={cmd.icon} />
                      </svg>
                      <span className="flex-1">{cmd.label}</span>
                      {selectedIndex === idx && (
                        <kbd className="text-[10px] text-dark-400">
                          Enter
                        </kbd>
                      )}
                    </button>
                  );
                })}
              </div>
            ))
          )}
        </div>

        {/* Footer */}
        <div className="flex items-center gap-4 px-4 py-2 border-t border-dark-600 text-[10px] text-dark-400">
          <span className="flex items-center gap-1">
            <kbd className="px-1 py-0.5 border border-dark-500 rounded bg-dark-700/50">&uarr;&darr;</kbd>
            Navigate
          </span>
          <span className="flex items-center gap-1">
            <kbd className="px-1 py-0.5 border border-dark-500 rounded bg-dark-700/50">Enter</kbd>
            Open
          </span>
          <span className="flex items-center gap-1">
            <kbd className="px-1 py-0.5 border border-dark-500 rounded bg-dark-700/50">Esc</kbd>
            Close
          </span>
        </div>
      </div>
    </div>
  );
}
