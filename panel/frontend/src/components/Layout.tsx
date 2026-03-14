import { ReactNode, useState, useEffect, useRef } from "react";
import { Navigate, Outlet, NavLink } from "react-router-dom";
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
  <svg className="w-[22px] h-[22px]" fill="none" viewBox="0 0 24 24" stroke="currentColor" strokeWidth={1.5} aria-hidden="true">
    <path strokeLinecap="round" strokeLinejoin="round" d={d} />
  </svg>
);

const settingsIcon = (
  <svg className="w-[22px] h-[22px]" fill="none" viewBox="0 0 24 24" stroke="currentColor" strokeWidth={1.5} aria-hidden="true">
    <path strokeLinecap="round" strokeLinejoin="round" d="M9.594 3.94c.09-.542.56-.94 1.11-.94h2.593c.55 0 1.02.398 1.11.94l.213 1.281c.063.374.313.686.645.87.074.04.147.083.22.127.325.196.72.257 1.075.124l1.217-.456a1.125 1.125 0 0 1 1.37.49l1.296 2.247a1.125 1.125 0 0 1-.26 1.431l-1.003.827c-.293.241-.438.613-.43.992a7.723 7.723 0 0 1 0 .255c-.008.378.137.75.43.991l1.004.827c.424.35.534.955.26 1.43l-1.298 2.247a1.125 1.125 0 0 1-1.369.491l-1.217-.456c-.355-.133-.75-.072-1.076.124a6.47 6.47 0 0 1-.22.128c-.331.183-.581.495-.644.869l-.213 1.281c-.09.543-.56.94-1.11.94h-2.594c-.55 0-1.019-.398-1.11-.94l-.213-1.281c-.062-.374-.312-.686-.644-.87a6.52 6.52 0 0 1-.22-.127c-.325-.196-.72-.257-1.076-.124l-1.217.456a1.125 1.125 0 0 1-1.369-.49l-1.297-2.247a1.125 1.125 0 0 1 .26-1.431l1.004-.827c.292-.24.437-.613.43-.991a6.932 6.932 0 0 1 0-.255c.007-.38-.138-.751-.43-.992l-1.004-.827a1.125 1.125 0 0 1-.26-1.43l1.297-2.247a1.125 1.125 0 0 1 1.37-.491l1.216.456c.356.133.751.072 1.076-.124.072-.044.146-.086.22-.128.332-.183.582-.495.644-.869l.214-1.28Z" />
    <path strokeLinecap="round" strokeLinejoin="round" d="M15 12a3 3 0 1 1-6 0 3 3 0 0 1 6 0Z" />
  </svg>
);

const navGroups: NavGroup[] = [
  {
    label: "Core",
    items: [
      { to: "/", label: "Dashboard", icon: icon("M3.75 6A2.25 2.25 0 0 1 6 3.75h2.25A2.25 2.25 0 0 1 10.5 6v2.25a2.25 2.25 0 0 1-2.25 2.25H6a2.25 2.25 0 0 1-2.25-2.25V6ZM3.75 15.75A2.25 2.25 0 0 1 6 13.5h2.25a2.25 2.25 0 0 1 2.25 2.25V18a2.25 2.25 0 0 1-2.25 2.25H6A2.25 2.25 0 0 1 3.75 18v-2.25ZM13.5 6a2.25 2.25 0 0 1 2.25-2.25H18A2.25 2.25 0 0 1 20.25 6v2.25A2.25 2.25 0 0 1 18 10.5h-2.25a2.25 2.25 0 0 1-2.25-2.25V6ZM13.5 15.75a2.25 2.25 0 0 1 2.25-2.25H18a2.25 2.25 0 0 1 2.25 2.25V18A2.25 2.25 0 0 1 18 20.25h-2.25a2.25 2.25 0 0 1-2.25-2.25v-2.25Z") },
      { to: "/sites", label: "Sites", icon: icon("M12 21a9.004 9.004 0 0 0 8.716-6.747M12 21a9.004 9.004 0 0 1-8.716-6.747M12 21c2.485 0 4.5-4.03 4.5-9S14.485 3 12 3m0 18c-2.485 0-4.5-4.03-4.5-9S9.515 3 12 3m0 0a8.997 8.997 0 0 1 7.843 4.582M12 3a8.997 8.997 0 0 0-7.843 4.582m15.686 0A11.953 11.953 0 0 1 12 10.5c-2.998 0-5.74-1.1-7.843-2.918m15.686 0A8.959 8.959 0 0 1 21 12c0 .778-.099 1.533-.284 2.253m0 0A17.919 17.919 0 0 1 12 16.5a17.92 17.92 0 0 1-8.716-2.247m0 0A9 9 0 0 1 3 12c0-1.47.353-2.856.978-4.082") },
      { to: "/dns", label: "DNS", icon: icon("M5.25 14.25h13.5m-13.5 0a3 3 0 0 1-3-3m3 3a3 3 0 1 0 0 6h13.5a3 3 0 1 0 0-6m-16.5-3a3 3 0 0 1 3-3h13.5a3 3 0 0 1 3 3m-19.5 0a4.5 4.5 0 0 1 .9-2.7L5.737 5.1a3.375 3.375 0 0 1 2.7-1.35h7.126c1.062 0 2.062.5 2.7 1.35l2.587 3.45a4.5 4.5 0 0 1 .9 2.7m0 0a3 3 0 0 1-3 3m0 3h.008v.008h-.008v-.008Zm0-6h.008v.008h-.008v-.008Zm-3 6h.008v.008h-.008v-.008Zm0-6h.008v.008h-.008v-.008Z") },
      { to: "/databases", label: "Databases", icon: icon("M20.25 6.375c0 2.278-3.694 4.125-8.25 4.125S3.75 8.653 3.75 6.375m16.5 0c0-2.278-3.694-4.125-8.25-4.125S3.75 4.097 3.75 6.375m16.5 0v11.25c0 2.278-3.694 4.125-8.25 4.125s-8.25-1.847-8.25-4.125V6.375m16.5 0v3.75m-16.5-3.75v3.75m16.5 0v3.75C20.25 16.153 16.556 18 12 18s-8.25-1.847-8.25-4.125v-3.75m16.5 0c0 2.278-3.694 4.125-8.25 4.125s-8.25-1.847-8.25-4.125") },
    ],
  },
  {
    label: "Docker",
    items: [
      { to: "/apps", label: "Apps", adminOnly: true, icon: icon("M21 7.5l-2.25-1.313M21 7.5v2.25m0-2.25l-2.25 1.313M3 7.5l2.25-1.313M3 7.5l2.25 1.313M3 7.5v2.25m9 3l2.25-1.313M12 12.75l-2.25-1.313M12 12.75V15m0 6.75l2.25-1.313M12 21.75V19.5m0 2.25l-2.25-1.313m0-16.875L12 2.25l2.25 1.313M21 14.25v2.25l-2.25 1.313m-13.5 0L3 16.5v-2.25") },
      { to: "/terminal", label: "Terminal", icon: icon("m6.75 7.5 3 2.25-3 2.25m4.5 0h3m-9 8.25h13.5A2.25 2.25 0 0 0 21 18V6a2.25 2.25 0 0 0-2.25-2.25H5.25A2.25 2.25 0 0 0 3 6v12a2.25 2.25 0 0 0 2.25 2.25Z") },
    ],
  },
  {
    label: "Operations",
    items: [
      { to: "/servers", label: "Servers", icon: icon("M5 12h14M5 12a2 2 0 0 1-2-2V6a2 2 0 0 1 2-2h14a2 2 0 0 1 2 2v4a2 2 0 0 1-2 2M5 12a2 2 0 0 0-2 2v4a2 2 0 0 0 2 2h14a2 2 0 0 0 2-2v-4a2 2 0 0 0-2-2m-2-4h.01M17 16h.01") },
      { to: "/monitors", label: "Monitors", icon: icon("M12 6v6h4.5m4.5 0a9 9 0 1 1-18 0 9 9 0 0 1 18 0Z") },
      { to: "/alerts", label: "Alerts", icon: icon("M14.857 17.082a23.848 23.848 0 0 0 5.454-1.31A8.967 8.967 0 0 1 18 9.75V9A6 6 0 0 0 6 9v.75a8.967 8.967 0 0 1-2.312 6.022c1.733.64 3.56 1.085 5.455 1.31m5.714 0a24.255 24.255 0 0 1-5.714 0m5.714 0a3 3 0 1 1-5.714 0") },
      { to: "/logs", label: "Logs", adminOnly: true, icon: icon("M19.5 14.25v-2.625a3.375 3.375 0 0 0-3.375-3.375h-1.5A1.125 1.125 0 0 1 13.5 7.125v-1.5a3.375 3.375 0 0 0-3.375-3.375H8.25m0 12.75h7.5m-7.5 3H12M10.5 2.25H5.625c-.621 0-1.125.504-1.125 1.125v17.25c0 .621.504 1.125 1.125 1.125h12.75c.621 0 1.125-.504 1.125-1.125V11.25a9 9 0 0 0-9-9Z") },
    ],
  },
  {
    label: "Admin",
    items: [
      { to: "/security", label: "Security", adminOnly: true, icon: icon("M9 12.75L11.25 15 15 9.75m-3-7.036A11.959 11.959 0 0 1 3.598 6 11.99 11.99 0 0 0 3 9.749c0 5.592 3.824 10.29 9 11.623 5.176-1.332 9-6.03 9-11.622 0-1.31-.21-2.571-.598-3.751h-.152c-3.196 0-6.1-1.248-8.25-3.285Z") },
      { to: "/diagnostics", label: "Diagnostics", adminOnly: true, icon: icon("M9.75 3.104v5.714a2.25 2.25 0 0 1-.659 1.591L5 14.5M9.75 3.104c-.251.023-.501.05-.75.082m.75-.082a24.301 24.301 0 0 1 4.5 0m0 0v5.714c0 .597.237 1.17.659 1.591L19.8 15.3M14.25 3.104c.251.023.501.05.75.082M19.8 15.3l-1.57.393A9.065 9.065 0 0 1 12 15a9.065 9.065 0 0 0-6.23.693L5 14.5m14.8.8 1.402 1.402c1.232 1.232.65 3.318-1.067 3.611A48.309 48.309 0 0 1 12 21a48.25 48.25 0 0 1-8.134-.888c-1.716-.293-2.3-2.379-1.067-3.61L5 14.5") },
      { to: "/settings", label: "Settings", adminOnly: true, icon: settingsIcon },
      { to: "/activity", label: "Activity", adminOnly: true, icon: icon("M12 6v6h4.5m4.5 0a9 9 0 1 1-18 0 9 9 0 0 1 18 0Z") },
      { to: "/users", label: "Users", adminOnly: true, icon: icon("M15 19.128a9.38 9.38 0 0 0 2.625.372 9.337 9.337 0 0 0 4.121-.952 4.125 4.125 0 0 0-7.533-2.493M15 19.128v-.003c0-1.113-.285-2.16-.786-3.07M15 19.128v.106A12.318 12.318 0 0 1 8.624 21c-2.331 0-4.512-.645-6.374-1.766l-.001-.109a6.375 6.375 0 0 1 11.964-3.07M12 6.375a3.375 3.375 0 1 1-6.75 0 3.375 3.375 0 0 1 6.75 0Zm8.25 2.25a2.625 2.625 0 1 1-5.25 0 2.625 2.625 0 0 1 5.25 0Z") },
      { to: "/teams", label: "Teams", icon: icon("M18 18.72a9.094 9.094 0 0 0 3.741-.479 3 3 0 0 0-4.682-2.72m.94 3.198.001.031c0 .225-.012.447-.037.666A11.944 11.944 0 0 1 12 21c-2.17 0-4.207-.576-5.963-1.584A6.062 6.062 0 0 1 6 18.719m12 0a5.971 5.971 0 0 0-.941-3.197m0 0A5.995 5.995 0 0 0 12 12.75a5.995 5.995 0 0 0-5.058 2.772m0 0a3 3 0 0 0-4.681 2.72 8.986 8.986 0 0 0 3.74.477m.94-3.197a5.971 5.971 0 0 0-.94 3.197M15 6.75a3 3 0 1 1-6 0 3 3 0 0 1 6 0Zm6 3a2.25 2.25 0 1 1-4.5 0 2.25 2.25 0 0 1 4.5 0Zm-13.5 0a2.25 2.25 0 1 1-4.5 0 2.25 2.25 0 0 1 4.5 0Z") },
    ],
  },
];

export default function Layout() {
  const { user, logout, loading } = useAuth();
  const [sidebarOpen, setSidebarOpen] = useState(false);
  const [firingCount, setFiringCount] = useState(0);
  const alertTimer = useRef<ReturnType<typeof setInterval>>(undefined);
  const [collapsed, setCollapsed] = useState<Record<string, boolean>>(() => {
    try {
      return JSON.parse(localStorage.getItem("dp-nav-collapsed") || "{}");
    } catch { return {}; }
  });

  const toggleGroup = (label: string) => {
    setCollapsed(prev => {
      const next = { ...prev, [label]: !prev[label] };
      localStorage.setItem("dp-nav-collapsed", JSON.stringify(next));
      return next;
    });
  };

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
      <a href="#main-content" className="sr-only focus:not-sr-only focus:absolute focus:z-[100] focus:top-2 focus:left-2 focus:px-4 focus:py-2 focus:bg-indigo-600 focus:text-white focus:rounded-lg">
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
        className={`fixed inset-y-0 left-0 z-50 w-64 bg-dark-950 border-r border-dark-600 text-white flex flex-col shrink-0 transform transition-transform duration-200 ease-in-out md:relative md:translate-x-0 ${
          sidebarOpen ? "translate-x-0" : "-translate-x-full"
        }`}
      >
        {/* Logo */}
        <div className="px-6 py-5 border-b border-dark-600 flex items-center justify-between">
          <div className="flex items-center gap-3">
            <div className="w-8 h-8 bg-rust-500 rounded-lg flex items-center justify-center">
              <svg className="w-5 h-5 text-white" viewBox="0 0 32 32" fill="currentColor" aria-hidden="true">
                <rect x="4" y="4" width="10" height="10" rx="2" opacity="0.9" />
                <rect x="18" y="4" width="10" height="10" rx="2" opacity="0.7" />
                <rect x="4" y="18" width="10" height="10" rx="2" opacity="0.7" />
                <rect x="18" y="18" width="10" height="10" rx="2" opacity="0.5" />
              </svg>
            </div>
            <span className="text-base font-bold tracking-widest uppercase font-mono text-rust-500">DockPanel</span>
          </div>
          {/* Close button (mobile) */}
          <button
            onClick={() => setSidebarOpen(false)}
            className="p-1.5 text-dark-200 hover:text-white md:hidden rounded-lg"
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
            className="w-full flex items-center gap-2 px-3 py-2 rounded-lg bg-dark-800/30 border border-dark-600/50 text-sm font-mono text-dark-300 hover:text-dark-100 hover:border-dark-400 transition-colors"
          >
            <svg className="w-4 h-4" fill="none" viewBox="0 0 24 24" stroke="currentColor" strokeWidth={2}><circle cx="11" cy="11" r="8" /><path d="M21 21l-4.35-4.35" /></svg>
            <span className="flex-1 text-left">Search...</span>
            <kbd className="text-[10px] px-1.5 py-0.5 border border-dark-500 rounded bg-dark-700/50">Ctrl K</kbd>
          </button>
        </div>

        {/* Nav */}
        <nav className="flex-1 px-3 py-2 space-y-1 overflow-y-auto">
          {visibleGroups.map((group, gi) => (
            <div key={group.label} className={gi > 0 ? "pt-2 mt-1 border-t border-dark-600/40" : ""}>
              <button
                onClick={() => toggleGroup(group.label)}
                className="flex items-center justify-between w-full px-3 py-1 text-[10px] font-semibold uppercase tracking-widest text-dark-400 hover:text-dark-200 font-mono transition-colors"
              >
                {group.label}
                <svg className={`w-3 h-3 transition-transform ${collapsed[group.label] ? "-rotate-90" : ""}`} fill="none" viewBox="0 0 24 24" stroke="currentColor" strokeWidth={2}>
                  <path strokeLinecap="round" strokeLinejoin="round" d="M19 9l-7 7-7-7" />
                </svg>
              </button>
              {!collapsed[group.label] && (
                <div className="mt-1 space-y-0.5">
                  {group.items.map((item) => (
                    <NavLink
                      key={item.label}
                      to={item.to}
                      end={item.to === "/"}
                      onClick={() => setSidebarOpen(false)}
                      className={({ isActive }) =>
                        `flex items-center gap-3 px-4 py-2 transition-colors text-sm uppercase tracking-wider ${
                          isActive
                            ? "bg-rust-500 text-dark-900 font-bold"
                            : "text-dark-300 hover:text-dark-50 hover:bg-dark-500/50"
                        }`
                      }
                    >
                      {item.icon}
                      <span>{item.label}</span>
                      {item.to === "/alerts" && firingCount > 0 && (
                        <span className="ml-auto px-1.5 py-0.5 text-xs font-bold bg-red-500 text-white rounded-full min-w-[20px] text-center">
                          {firingCount}
                        </span>
                      )}
                    </NavLink>
                  ))}
                </div>
              )}
            </div>
          ))}
        </nav>

        {/* User */}
        <div className="px-4 py-4 border-t border-dark-600">
          <div className="flex items-center justify-between">
            <div className="min-w-0">
              <p className="text-sm font-medium truncate">{user.email}</p>
              <p className="text-xs text-dark-200 capitalize">{user.role}</p>
            </div>
            <button
              onClick={logout}
              className="p-2 text-dark-200 hover:text-white hover:bg-dark-800 rounded-lg transition-colors"
              title="Logout"
              aria-label="Logout"
            >
              <svg className="w-5 h-5" fill="none" viewBox="0 0 24 24" stroke="currentColor" strokeWidth={1.5} aria-hidden="true">
                <path strokeLinecap="round" strokeLinejoin="round" d="M15.75 9V5.25A2.25 2.25 0 0 0 13.5 3h-6a2.25 2.25 0 0 0-2.25 2.25v13.5A2.25 2.25 0 0 0 7.5 21h6a2.25 2.25 0 0 0 2.25-2.25V15m3 0 3-3m0 0-3-3m3 3H9" />
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
          <span className="text-base font-bold tracking-widest uppercase font-mono text-rust-500">DockPanel</span>
        </div>
        <Outlet />
      </main>
    </div>
  );
}
