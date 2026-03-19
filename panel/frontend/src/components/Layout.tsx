import { ReactNode, useState, useEffect, useRef } from "react";
import { Navigate, Outlet, NavLink, Link } from "react-router-dom";
import { useAuth } from "../context/AuthContext";
import { api } from "../api";
import CommandPalette from "./CommandPalette";

interface NavItem {
  to: string;
  label: string;
  icon: ReactNode;
  adminOnly?: boolean;
}

interface NavGroup {
  label: string;
  items: NavItem[];
}

const icon = (d: string) => (
  <svg className="w-[19px] h-[19px]" fill="none" viewBox="0 0 24 24" stroke="currentColor" strokeWidth={1.5} aria-hidden="true">
    <path strokeLinecap="round" strokeLinejoin="round" d={d} />
  </svg>
);

const settingsIcon = (
  <svg className="w-[19px] h-[19px]" fill="none" viewBox="0 0 24 24" stroke="currentColor" strokeWidth={1.5} aria-hidden="true">
    <path strokeLinecap="round" strokeLinejoin="round" d="M9.594 3.94c.09-.542.56-.94 1.11-.94h2.593c.55 0 1.02.398 1.11.94l.213 1.281c.063.374.313.686.645.87.074.04.147.083.22.127.325.196.72.257 1.075.124l1.217-.456a1.125 1.125 0 0 1 1.37.49l1.296 2.247a1.125 1.125 0 0 1-.26 1.431l-1.003.827c-.293.241-.438.613-.43.992a7.723 7.723 0 0 1 0 .255c-.008.378.137.75.43.991l1.004.827c.424.35.534.955.26 1.43l-1.298 2.247a1.125 1.125 0 0 1-1.369.491l-1.217-.456c-.355-.133-.75-.072-1.076.124a6.47 6.47 0 0 1-.22.128c-.331.183-.581.495-.644.869l-.213 1.281c-.09.543-.56.94-1.11.94h-2.594c-.55 0-1.019-.398-1.11-.94l-.213-1.281c-.062-.374-.312-.686-.644-.87a6.52 6.52 0 0 1-.22-.127c-.325-.196-.72-.257-1.076-.124l-1.217.456a1.125 1.125 0 0 1-1.369-.49l-1.297-2.247a1.125 1.125 0 0 1 .26-1.431l1.004-.827c.292-.24.437-.613.43-.991a6.932 6.932 0 0 1 0-.255c.007-.38-.138-.751-.43-.992l-1.004-.827a1.125 1.125 0 0 1-.26-1.43l1.297-2.247a1.125 1.125 0 0 1 1.37-.491l1.216.456c.356.133.751.072 1.076-.124.072-.044.146-.086.22-.128.332-.183.582-.495.644-.869l.214-1.28Z" />
    <path strokeLinecap="round" strokeLinejoin="round" d="M15 12a3 3 0 1 1-6 0 3 3 0 0 1 6 0Z" />
  </svg>
);

const navGroups: NavGroup[] = [
  {
    label: "Hosting",
    items: [
      { to: "/", label: "Dashboard", icon: icon("M3.75 13.5l10.5-11.25L12 10.5h8.25L9.75 21.75 12 13.5H3.75z") },
      { to: "/sites", label: "Sites", icon: icon("M13.19 8.688a4.5 4.5 0 011.242 7.244l-4.5 4.5a4.5 4.5 0 01-6.364-6.364l1.757-1.757m9.86-2.07a4.5 4.5 0 00-6.364-6.364L4.5 8.257a4.5 4.5 0 006.364 6.364") },
      { to: "/databases", label: "Databases", icon: icon("M20.25 6.375c0 2.278-3.694 4.125-8.25 4.125S3.75 8.653 3.75 6.375m16.5 0c0-2.278-3.694-4.125-8.25-4.125S3.75 4.097 3.75 6.375m16.5 0v11.25c0 2.278-3.694 4.125-8.25 4.125s-8.25-1.847-8.25-4.125V6.375m16.5 0v3.75m-16.5-3.75v3.75m16.5 0v3.75C20.25 16.153 16.556 18 12 18s-8.25-1.847-8.25-4.125v-3.75m16.5 0c0 2.278-3.694 4.125-8.25 4.125s-8.25-1.847-8.25-4.125") },
      { to: "/apps", label: "Docker Apps", adminOnly: true, icon: icon("M21 7.5l-9-5.25L3 7.5m18 0l-9 5.25m9-5.25v9l-9 5.25M3 7.5l9 5.25M3 7.5v9l9 5.25m0-9v9") },
      { to: "/git-deploys", label: "Git Deploy", adminOnly: true, icon: icon("M17.25 6.75L22.5 12l-5.25 5.25m-10.5 0L1.5 12l5.25-5.25m7.5-3l-4.5 16.5") },
    ],
  },
  {
    label: "Operations",
    items: [
      { to: "/dns", label: "DNS", icon: icon("M5.25 14.25h13.5m-13.5 0a3 3 0 0 1-3-3m3 3a3 3 0 1 0 0 6h13.5a3 3 0 1 0 0-6m-16.5-3a3 3 0 0 1 3-3h13.5a3 3 0 0 1 3 3m-19.5 0a4.5 4.5 0 0 1 .9-2.7L5.737 5.1a3.375 3.375 0 0 1 2.7-1.35h7.126c1.062 0 2.062.5 2.7 1.35l2.587 3.45a4.5 4.5 0 0 1 .9 2.7m0 0a3 3 0 0 1-3 3m0 3h.008v.008h-.008v-.008Zm0-6h.008v.008h-.008v-.008Zm-3 6h.008v.008h-.008v-.008Zm0-6h.008v.008h-.008v-.008Z") },
      { to: "/mail", label: "Mail", adminOnly: true, icon: icon("M21.75 6.75v10.5a2.25 2.25 0 0 1-2.25 2.25h-15a2.25 2.25 0 0 1-2.25-2.25V6.75m19.5 0A2.25 2.25 0 0 0 19.5 4.5h-15a2.25 2.25 0 0 0-2.25 2.25m19.5 0v.243a2.25 2.25 0 0 1-1.07 1.916l-7.5 4.615a2.25 2.25 0 0 1-2.36 0L3.32 8.91a2.25 2.25 0 0 1-1.07-1.916V6.75") },
      { to: "/monitoring", label: "Monitoring", icon: icon("M2.25 18L9 11.25l4.306 4.307a11.95 11.95 0 015.814-5.519l2.74-1.22m0 0l-5.94-2.28m5.94 2.28l-2.28 5.941") },
      { to: "/logs", label: "Logs", adminOnly: true, icon: icon("M19.5 14.25v-2.625a3.375 3.375 0 0 0-3.375-3.375h-1.5A1.125 1.125 0 0 1 13.5 7.125v-1.5a3.375 3.375 0 0 0-3.375-3.375H8.25m0 12.75h7.5m-7.5 3H12M10.5 2.25H5.625c-.621 0-1.125.504-1.125 1.125v17.25c0 .621.504 1.125 1.125 1.125h12.75c.621 0 1.125-.504 1.125-1.125V11.25a9 9 0 0 0-9-9Z") },
      { to: "/terminal", label: "Terminal", icon: icon("m6.75 7.5 3 2.25-3 2.25m4.5 0h3m-9 8.25h13.5A2.25 2.25 0 0 0 21 18V6a2.25 2.25 0 0 0-2.25-2.25H5.25A2.25 2.25 0 0 0 3 6v12a2.25 2.25 0 0 0 2.25 2.25Z") },
    ],
  },
  {
    label: "Admin",
    items: [
      { to: "/security", label: "Security", adminOnly: true, icon: icon("M9 12.75L11.25 15 15 9.75m-3-7.036A11.959 11.959 0 0 1 3.598 6 11.99 11.99 0 0 0 3 9.749c0 5.592 3.824 10.29 9 11.623 5.176-1.332 9-6.03 9-11.622 0-1.31-.21-2.571-.598-3.751h-.152c-3.196 0-6.1-1.248-8.25-3.285Z") },
      { to: "/settings", label: "Settings", adminOnly: true, icon: settingsIcon },
    ],
  },
];

export default function Layout() {
  const { user, logout, loading } = useAuth();
  const [sidebarOpen, setSidebarOpen] = useState(false);
  const [firingCount, setFiringCount] = useState(0);
  const [apiHealthy, setApiHealthy] = useState<boolean | null>(null);
  const themeOrder = ["terminal", "midnight", "ember", "arctic"] as const;
  const [theme, setTheme] = useState(() => {
    const stored = localStorage.getItem("dp-theme");
    if (!stored || stored === "dark") return "terminal";
    if (stored === "light") return "arctic";
    return stored;
  });
  const [twoFaEnforced, setTwoFaEnforced] = useState(false);
  const [twoFaEnabled, setTwoFaEnabled] = useState(true); // assume true until checked

  useEffect(() => {
    document.documentElement.setAttribute("data-theme", theme);
    localStorage.setItem("dp-theme", theme);
  }, [theme]);
  const alertTimer = useRef<ReturnType<typeof setInterval>>(undefined);
  const healthTimer = useRef<ReturnType<typeof setInterval>>(undefined);

  useEffect(() => {
    const fetchFiring = () => {
      api.get<{ firing: number }>("/alerts/summary")
        .then((s) => setFiringCount(s.firing))
        .catch(() => {});
    };
    fetchFiring();
    alertTimer.current = setInterval(fetchFiring, 30000);
    return () => { if (alertTimer.current) clearInterval(alertTimer.current); };
  }, []);

  useEffect(() => {
    const checkHealth = () => {
      api.get<{ db: string; agent: string }>("/settings/health")
        .then((h) => setApiHealthy(h.db === "ok" && h.agent === "ok"))
        .catch(() => setApiHealthy(false));
    };
    checkHealth();
    healthTimer.current = setInterval(checkHealth, 30000);
    return () => { if (healthTimer.current) clearInterval(healthTimer.current); };
  }, []);

  // Check 2FA enforcement
  useEffect(() => {
    api.get<Record<string, string>>("/settings").then(s => {
        if (s.enforce_2fa === "true") setTwoFaEnforced(true);
    }).catch(() => {});
    api.get<{ enabled: boolean }>("/auth/2fa/status").then(d => setTwoFaEnabled(d.enabled)).catch(() => {});
  }, []);

  if (loading) {
    return (
      <div className="flex items-center justify-center h-screen bg-dark-900">
        <div className="w-8 h-8 border-4 border-rust-500 border-t-transparent rounded-full animate-spin" />
      </div>
    );
  }

  if (!user) return <Navigate to="/login" replace />;

  const visibleGroups = navGroups.map(g => ({
    ...g,
    items: g.items.filter(item => !item.adminOnly || user.role === "admin"),
  })).filter(g => g.items.length > 0);

  return (
    <div className="flex h-screen bg-dark-900">
      <CommandPalette />

      {/* Skip to content */}
      <a href="#main-content" className="sr-only focus:not-sr-only focus:absolute focus:z-[100] focus:top-2 focus:left-2 focus:px-4 focus:py-2 focus:bg-accent-600 focus:text-dark-50 focus:rounded-lg">
        Skip to main content
      </a>

      {/* Mobile overlay */}
      {sidebarOpen && (
        <div
          className="fixed inset-0 bg-black/50 z-40 md:hidden"
          role="presentation"
          onClick={() => setSidebarOpen(false)}
        />
      )}

      {/* Sidebar */}
      <aside
        className={`fixed inset-y-0 left-0 z-50 w-64 bg-dark-950 border-r border-dark-600 text-dark-50 flex flex-col shrink-0 transform transition-transform duration-200 ease-in-out md:relative md:translate-x-0 ${
          sidebarOpen ? "translate-x-0" : "-translate-x-full"
        }`}
      >
        {/* Logo */}
        <div className="px-6 py-5 border-b border-dark-600 flex items-center justify-between">
          <Link to="/" className="flex items-center gap-3 hover:opacity-90 transition-opacity">
            <div className="w-10 h-10 bg-rust-500 flex items-center justify-center logo-icon-glow">
              <svg className="w-6 h-6 text-dark-950" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="2.5" aria-hidden="true">
                <path d="M5 16h4" strokeLinecap="square" />
                <path d="M5 12h8" strokeLinecap="square" />
                <path d="M5 8h6" strokeLinecap="square" />
                <rect x="16" y="7" width="4" height="4" fill="currentColor" stroke="none" />
                <rect x="16" y="13" width="4" height="4" fill="currentColor" stroke="none" />
              </svg>
            </div>
            <span className="text-lg font-bold tracking-widest uppercase font-mono logo-glow"><span className="text-rust-500">Dock</span><span className="text-dark-50">Panel</span></span>
          </Link>
          {/* Close button (mobile) */}
          <button
            onClick={() => setSidebarOpen(false)}
            className="p-1.5 text-dark-200 hover:text-dark-50 md:hidden rounded-lg"
            aria-label="Close sidebar"
          >
            <svg className="w-5 h-5" fill="none" viewBox="0 0 24 24" stroke="currentColor" strokeWidth={2} aria-hidden="true">
              <path strokeLinecap="round" strokeLinejoin="round" d="M6 18L18 6M6 6l12 12" />
            </svg>
          </button>
        </div>

        {/* Search shortcut */}
        <div className="px-3 pt-3 pb-1">
          <button
            onClick={() => window.dispatchEvent(new KeyboardEvent("keydown", { key: "k", ctrlKey: true }))}
            className="w-full flex items-center gap-2 px-3 py-2 rounded-lg bg-dark-800/30 border border-dark-600/50 text-sm font-mono text-dark-300 hover:text-dark-100 hover:border-dark-400 transition-colors outline-none focus:outline-none focus:border-dark-400"
          >
            <svg className="w-[19px] h-[19px]" fill="none" viewBox="0 0 24 24" stroke="currentColor" strokeWidth={2}><circle cx="11" cy="11" r="8" /><path d="M21 21l-4.35-4.35" /></svg>
            <span className="flex-1 text-left">Search...</span>
            <kbd className="text-[10px] px-1.5 py-0.5 border border-dark-500 rounded bg-dark-700/50">Ctrl K</kbd>
          </button>
        </div>

        {/* Nav */}
        <nav className="flex-1 px-3 pt-4 overflow-y-auto">
          {visibleGroups.map((group, gi) => (
            <div key={group.label} className={gi > 0 ? "mt-5" : ""}>
              <div className="space-y-1">
                {group.items.map((item) => (
                  <NavLink
                    key={item.label}
                    to={item.to}
                    end={item.to === "/"}
                    onClick={() => setSidebarOpen(false)}
                    className={({ isActive }) =>
                      `flex items-center gap-3 px-4 py-2 transition-colors text-sm uppercase tracking-wider ${
                        isActive
                          ? "bg-dark-50/5 text-dark-50 font-bold border-l-2 border-dark-50"
                          : "text-dark-300 hover:text-dark-50 hover:bg-dark-700/50"
                      }`
                    }
                  >
                    {({ isActive }) => (
                      <>
                        {item.icon}
                        <span>{item.label}</span>
                        {item.to === "/monitoring" && firingCount > 0 ? (
                          <span className="ml-auto px-1.5 py-0.5 text-xs font-bold bg-danger-500 text-white rounded-full min-w-[20px] text-center">
                            {firingCount}
                          </span>
                        ) : isActive ? (
                          <span className="ml-auto blinking-cursor text-xs">_</span>
                        ) : null}
                      </>
                    )}
                  </NavLink>
                ))}
              </div>
            </div>
          ))}
        </nav>

        {/* User + Build Status */}
        <div className="mx-3 px-4 py-4 border-t border-dark-600/50 space-y-3">
          <div className="flex items-center justify-between">
            <div className="min-w-0">
              <p className="text-sm font-medium truncate">{user.email}</p>
              <p className="text-xs text-dark-200 capitalize">{user.role}</p>
            </div>
            <button
              onClick={logout}
              className="p-2 text-dark-200 hover:text-dark-50 hover:bg-dark-800 rounded-lg transition-colors"
              title="Logout"
              aria-label="Logout"
            >
              <svg className="w-5 h-5" fill="none" viewBox="0 0 24 24" stroke="currentColor" strokeWidth={1.5} aria-hidden="true">
                <path strokeLinecap="round" strokeLinejoin="round" d="M15.75 9V5.25A2.25 2.25 0 0 0 13.5 3h-6a2.25 2.25 0 0 0-2.25 2.25v13.5A2.25 2.25 0 0 0 7.5 21h6a2.25 2.25 0 0 0 2.25-2.25V15m3 0 3-3m0 0-3-3m3 3H9" />
              </svg>
            </button>
          </div>
          <div className="flex items-center gap-2">
            <div className="flex items-center gap-2 px-2.5 py-1.5 bg-dark-800 border border-dark-600/40 flex-1">
              <div className={`w-2 h-2 rounded-full shrink-0 ${apiHealthy === null ? "bg-dark-400" : apiHealthy ? "bg-rust-500" : "bg-danger-500 animate-pulse"}`} />
              <span className="text-[10px] font-semibold uppercase tracking-widest text-dark-300 font-mono">{apiHealthy === null ? "Checking..." : apiHealthy ? "All Systems OK" : "Health Issue"}</span>
            </div>
            <button
              onClick={() => {
                const idx = themeOrder.indexOf(theme as typeof themeOrder[number]);
                const next = themeOrder[(idx + 1) % themeOrder.length];
                setTheme(next);
              }}
              className="p-1.5 text-dark-300 hover:text-dark-50 bg-dark-800 border border-dark-600/40 transition-colors rounded"
              title={`Theme: ${theme}`}
              aria-label="Cycle theme"
            >
              <svg className="w-[19px] h-[19px]" fill="none" viewBox="0 0 24 24" stroke="currentColor" strokeWidth={1.5}>
                <path strokeLinecap="round" strokeLinejoin="round" d="M4.098 19.902a3.75 3.75 0 0 0 5.304 0l6.401-6.402M6.75 21A3.75 3.75 0 0 1 3 17.25V4.125C3 3.504 3.504 3 4.125 3h5.25c.621 0 1.125.504 1.125 1.125v4.072M6.75 21a3.75 3.75 0 0 0 3.75-3.75V8.197M6.75 21h13.125c.621 0 1.125-.504 1.125-1.125v-5.25c0-.621-.504-1.125-1.125-1.125h-4.072M10.5 8.197l2.88-2.88c.438-.439 1.15-.439 1.59 0l3.712 3.713c.44.44.44 1.152 0 1.59l-2.88 2.88M6.75 17.25h.008v.008H6.75v-.008Z" />
              </svg>
            </button>
          </div>
        </div>
      </aside>

      {/* Main content */}
      <main id="main-content" className="flex-1 overflow-auto">
        {/* Mobile header with hamburger */}
        <div className="sticky top-0 z-30 flex items-center gap-3 px-4 py-3 bg-dark-900 border-b border-dark-600 md:hidden">
          <button
            onClick={() => setSidebarOpen(true)}
            className="p-2 text-dark-200 hover:text-dark-50 hover:bg-dark-700 rounded-lg"
            aria-label="Open sidebar"
          >
            <svg className="w-6 h-6" fill="none" viewBox="0 0 24 24" stroke="currentColor" strokeWidth={2} aria-hidden="true">
              <path strokeLinecap="round" strokeLinejoin="round" d="M3.75 6.75h16.5M3.75 12h16.5m-16.5 5.25h16.5" />
            </svg>
          </button>
          <span className="text-base font-bold tracking-widest uppercase font-mono logo-glow"><span className="text-rust-500">Dock</span><span className="text-dark-100">Panel</span></span>
        </div>
        {twoFaEnforced && !twoFaEnabled && (
          <div className="bg-warn-500/10 border-b border-warn-500/20 px-4 py-3 flex items-center justify-between">
            <div className="flex items-center gap-2">
              <svg className="w-5 h-5 text-warn-400" fill="none" viewBox="0 0 24 24" stroke="currentColor" strokeWidth={1.5}>
                <path strokeLinecap="round" strokeLinejoin="round" d="M12 9v3.75m-9.303 3.376c-.866 1.5.217 3.374 1.948 3.374h14.71c1.73 0 2.813-1.874 1.948-3.374L13.949 3.378c-.866-1.5-3.032-1.5-3.898 0L2.697 16.126ZM12 15.75h.007v.008H12v-.008Z" />
              </svg>
              <span className="text-sm text-warn-400 font-medium">Two-factor authentication is required. Please enable 2FA in Settings &rarr; Security.</span>
            </div>
            <a href="/settings" className="px-3 py-1.5 bg-warn-500 text-dark-900 rounded text-xs font-bold hover:bg-warn-400 transition-colors">
              Set Up 2FA
            </a>
          </div>
        )}
        <Outlet />
      </main>
    </div>
  );
}
