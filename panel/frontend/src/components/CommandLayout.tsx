import { Navigate, Outlet, NavLink, Link, useNavigate } from "react-router-dom";
import { useLayoutState } from "../hooks/useLayoutState";
import { useServer } from "../context/ServerContext";
import { useBranding } from "../context/BrandingContext";
import { Icon } from "../data/icons";
import CommandPalette from "./CommandPalette";
import LayoutSwitcher from "./LayoutSwitcher";
import { useState, useRef, useEffect } from "react";

function ServerSelector() {
  const { servers, activeServer, setActiveServerId } = useServer();
  const [open, setOpen] = useState(false);
  const ref = useRef<HTMLDivElement>(null);
  const navigate = useNavigate();

  useEffect(() => {
    function handleClick(e: MouseEvent) {
      if (ref.current && !ref.current.contains(e.target as Node)) setOpen(false);
    }
    document.addEventListener("mousedown", handleClick);
    return () => document.removeEventListener("mousedown", handleClick);
  }, []);

  if (servers.length <= 1) return null;

  return (
    <div className="px-3 pt-2" ref={ref}>
      <button
        onClick={() => setOpen(!open)}
        className="w-full flex items-center gap-2 px-3 py-2 rounded-lg bg-dark-800/50 border border-dark-600/50 text-sm text-dark-200 hover:text-dark-100 hover:border-dark-400 transition-colors"
      >
        <div className={`w-2 h-2 rounded-full shrink-0 ${activeServer?.status === "online" ? "bg-rust-500" : activeServer?.status === "offline" ? "bg-danger-500" : "bg-dark-400"}`} />
        <span className="flex-1 text-left truncate">{activeServer?.name || "Select server"}</span>
        <svg className={`w-4 h-4 transition-transform ${open ? "rotate-180" : ""}`} fill="none" viewBox="0 0 24 24" stroke="currentColor" strokeWidth={2}>
          <path strokeLinecap="round" strokeLinejoin="round" d="M19.5 8.25l-7.5 7.5-7.5-7.5" />
        </svg>
      </button>
      {open && (
        <div className="mt-1 bg-dark-900 border border-dark-600 rounded-lg shadow-xl overflow-hidden">
          {servers.map((s) => (
            <button
              key={s.id}
              onClick={() => { setActiveServerId(s.id); setOpen(false); window.location.href = "/"; }}
              className={`w-full flex items-center gap-2 px-3 py-2 text-sm text-left hover:bg-dark-700/50 transition-colors ${s.id === activeServer?.id ? "bg-dark-800 text-dark-50" : "text-dark-300"}`}
            >
              <div className={`w-2 h-2 rounded-full shrink-0 ${s.status === "online" ? "bg-rust-500" : s.status === "offline" ? "bg-danger-500" : "bg-dark-400"}`} />
              <span className="flex-1 truncate">{s.name}</span>
              {s.is_local && <span className="text-[10px] text-dark-400 uppercase">local</span>}
            </button>
          ))}
        </div>
      )}
    </div>
  );
}

export default function CommandLayout() {
  const state = useLayoutState();
  const branding = useBranding();
  const isLight = state.theme === "clean" || state.theme === "arctic";

  // Layout options (persisted in localStorage)
  const [showHeader, setShowHeader] = useState(() => localStorage.getItem("dp-show-header") === "true");
  const [flatNav, setFlatNav] = useState(() => localStorage.getItem("dp-flat-nav") === "true");

  // Expose setters for Settings page
  useEffect(() => {
    const handler = () => {
      setShowHeader(localStorage.getItem("dp-show-header") === "true");
      setFlatNav(localStorage.getItem("dp-flat-nav") === "true");
    };
    window.addEventListener("dp-layout-options-change", handler);
    return () => window.removeEventListener("dp-layout-options-change", handler);
  }, []);

  if (state.loading) {
    return (
      <div className="flex items-center justify-center h-screen bg-dark-900">
        <div className="w-8 h-8 border-4 border-rust-500 border-t-transparent rounded-full animate-spin" />
      </div>
    );
  }

  if (!state.user.email) return <Navigate to="/login" replace />;

  // Flatten nav if option enabled
  const navItems = flatNav
    ? state.visibleGroups.flatMap(g => g.items)
    : null;

  return (
    <div className="flex h-screen bg-dark-900">
      <CommandPalette />

      {/* Skip to content */}
      <a href="#main-content" className="sr-only focus:not-sr-only focus:absolute focus:z-[100] focus:top-2 focus:left-2 focus:px-4 focus:py-2 focus:bg-accent-600 focus:text-dark-50 focus:rounded-lg">
        Skip to main content
      </a>

      {/* Mobile overlay */}
      {state.sidebarOpen && (
        <div
          className="fixed inset-0 bg-black/50 z-40 md:hidden"
          role="presentation"
          onClick={() => state.setSidebarOpen(false)}
        />
      )}

      {/* Sidebar */}
      <aside
        className={`fixed inset-y-0 left-0 z-50 w-64 bg-dark-950 border-r border-dark-600 text-dark-50 flex flex-col shrink-0 transform transition-transform duration-200 ease-in-out md:relative md:translate-x-0 ${
          state.sidebarOpen ? "translate-x-0" : "-translate-x-full"
        }`}
      >
        {/* Logo */}
        <div className={`px-6 border-b border-dark-600 flex items-center justify-between ${showHeader ? "h-16" : "py-5"}`}>
          <Link to="/" className="flex items-center gap-3 hover:opacity-90 transition-opacity">
            {branding.logoUrl ? (
              <img src={branding.logoUrl} alt={branding.panelName} className="h-10 w-auto max-w-[160px] object-contain" />
            ) : (
              <>
                <div className="w-8 h-8 bg-rust-500 rounded-lg flex items-center justify-center logo-icon-glow">
                  <svg className="w-5 h-5 text-dark-950" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="2.5" aria-hidden="true">
                    <path d="M5 16h4" strokeLinecap="square" />
                    <path d="M5 12h8" strokeLinecap="square" />
                    <path d="M5 8h6" strokeLinecap="square" />
                    <rect x="16" y="7" width="4" height="4" fill="currentColor" stroke="none" />
                    <rect x="16" y="13" width="4" height="4" fill="currentColor" stroke="none" />
                  </svg>
                </div>
                {!branding.hideBranding && (
                  <span className="text-lg font-bold logo-glow">
                    {branding.panelName === "DockPanel" ? (
                      <><span className="text-rust-400">Dock</span><span className="text-dark-50">Panel</span></>
                    ) : (
                      <span className="text-dark-50">{branding.panelName}</span>
                    )}
                  </span>
                )}
              </>
            )}
          </Link>
          <button
            onClick={() => state.setSidebarOpen(false)}
            className="p-1.5 text-dark-200 hover:text-dark-50 md:hidden rounded-lg"
            aria-label="Close sidebar"
          >
            <svg className="w-5 h-5" fill="none" viewBox="0 0 24 24" stroke="currentColor" strokeWidth={2} aria-hidden="true">
              <path strokeLinecap="round" strokeLinejoin="round" d="M6 18L18 6M6 6l12 12" />
            </svg>
          </button>
        </div>

        {/* Search shortcut (hide if header has search) */}
        {!showHeader && (
          <div className="px-3 pt-3 pb-1">
            <button
              onClick={() => window.dispatchEvent(new KeyboardEvent("keydown", { key: "k", ctrlKey: true }))}
              className="w-full flex items-center gap-2 px-3 py-2 rounded-lg bg-dark-800/30 border border-dark-600/50 text-sm text-dark-300 hover:text-dark-100 hover:border-dark-400 transition-colors outline-none"
            >
              <Icon name="search" className="w-[19px] h-[19px]" />
              <span className="flex-1 text-left">Search...</span>
              <kbd className="text-[10px] px-1.5 py-0.5 border border-dark-500 rounded bg-dark-700/50">Ctrl K</kbd>
            </button>
          </div>
        )}

        <ServerSelector />

        {/* Nav */}
        <nav className="flex-1 px-3 pt-4 overflow-y-auto">
          {flatNav && navItems ? (
            /* Flat nav — no group labels */
            <div className="space-y-1">
              {navItems.map((item) => (
                <NavLink
                  key={item.to}
                  to={item.to}
                  end={item.to === "/"}
                  onClick={() => state.setSidebarOpen(false)}
                  className={({ isActive }) =>
                    `flex items-center gap-3 px-3 py-2.5 text-sm font-medium rounded-lg transition-colors ${
                      isActive
                        ? "bg-rust-500/10 text-rust-400"
                        : "text-dark-300 hover:text-dark-100 hover:bg-dark-700/30"
                    }`
                  }
                >
                  {({ isActive }) => (
                    <>
                      <span className={isActive ? "text-rust-400" : "text-dark-500"}><Icon name={item.iconName} className="w-5 h-5" /></span>
                      <span>{item.label}</span>
                      {item.to === "/monitoring" && state.firingCount > 0 && (
                        <span className="ml-auto px-1.5 py-0.5 text-xs font-bold bg-danger-500 text-white rounded-full min-w-[20px] text-center">
                          {state.firingCount}
                        </span>
                      )}
                      {item.to === "/incidents" && state.incidentCount > 0 && (
                        <span className="ml-auto px-1.5 py-0.5 text-xs font-bold bg-warn-500 text-dark-900 rounded-full min-w-[20px] text-center">
                          {state.incidentCount}
                        </span>
                      )}
                    </>
                  )}
                </NavLink>
              ))}
            </div>
          ) : (
            /* Grouped nav — with labels */
            state.visibleGroups.map((group, gi) => (
              <div key={group.label} className={gi > 0 ? "mt-5" : ""}>
                {gi > 0 && (
                  <div className="px-4 pb-1.5 text-[10px] text-dark-400 uppercase tracking-widest font-medium">
                    {group.label}
                  </div>
                )}
                <div className="space-y-1">
                  {group.items.map((item) => (
                    <NavLink
                      key={item.label}
                      to={item.to}
                      end={item.to === "/"}
                      onClick={() => state.setSidebarOpen(false)}
                      className={({ isActive }) =>
                        `flex items-center gap-3 px-4 py-2 transition-colors text-sm ${
                          isActive
                            ? "bg-rust-500/10 text-rust-400 font-bold border-l-2 border-rust-500"
                            : "text-dark-300 hover:text-dark-100 hover:bg-dark-700/30"
                        }`
                      }
                    >
                      {({ isActive }) => (
                        <>
                          <Icon name={item.iconName} />
                          <span>{item.label}</span>
                          {item.to === "/monitoring" && state.firingCount > 0 ? (
                            <span className="ml-auto px-1.5 py-0.5 text-xs font-bold bg-danger-500 text-white rounded-full min-w-[20px] text-center">
                              {state.firingCount}
                            </span>
                          ) : item.to === "/incidents" && state.incidentCount > 0 ? (
                            <span className="ml-auto px-1.5 py-0.5 text-xs font-bold bg-warn-500 text-dark-900 rounded-full min-w-[20px] text-center">
                              {state.incidentCount}
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
            ))
          )}
        </nav>

        {/* User + Status (in sidebar footer when no header) */}
        {!showHeader && (
          <div className="px-3 py-3 border-t border-dark-600/50">
            <div className="flex items-center gap-3 px-3 py-2 rounded-lg hover:bg-dark-800/50 transition-colors group">
              <div className="w-8 h-8 rounded-full bg-rust-500/15 flex items-center justify-center shrink-0">
                <span className="text-xs font-bold text-rust-400 uppercase">{state.user.email?.charAt(0) || "?"}</span>
              </div>
              <div className="flex-1 min-w-0">
                <p className="text-sm font-medium truncate">{state.user.email}</p>
                <p className="text-[11px] text-dark-400 capitalize">{state.user.role}</p>
              </div>
              <button
                onClick={state.logout}
                className="p-1.5 text-dark-400 hover:text-dark-100 rounded-lg transition-colors opacity-0 group-hover:opacity-100"
                title="Logout"
                aria-label="Logout"
              >
                <Icon name="logout" className="w-4 h-4" />
              </button>
            </div>
            <div className="flex items-center justify-between px-3 mt-2">
              <div className="flex items-center gap-2">
                <div className={`w-1.5 h-1.5 rounded-full shrink-0 ${state.apiHealthy === null ? "bg-dark-400" : state.apiHealthy ? "bg-rust-500" : "bg-danger-500 animate-pulse"}`} />
                <span className="text-[10px] text-dark-400">{state.apiHealthy === null ? "Checking..." : state.apiHealthy ? "Connected" : "Disconnected"}</span>
              </div>
              <div className="flex items-center gap-1">
                <Link to="/notifications" className="relative p-1.5 text-dark-400 hover:text-dark-200 transition-colors rounded" title="Notifications" aria-label="Notifications">
                  <svg className="w-3.5 h-3.5" fill="none" viewBox="0 0 24 24" stroke="currentColor" strokeWidth={1.5}>
                    <path strokeLinecap="round" strokeLinejoin="round" d="M14.857 17.082a23.848 23.848 0 005.454-1.31A8.967 8.967 0 0118 9.75v-.7V9A6 6 0 006 9v.75a8.967 8.967 0 01-2.312 6.022c1.733.64 3.56 1.085 5.455 1.31m5.714 0a24.255 24.255 0 01-5.714 0m5.714 0a3 3 0 11-5.714 0" />
                  </svg>
                  {state.notifCount > 0 && (
                    <span className="absolute -top-0.5 -right-0.5 w-4 h-4 bg-danger-500 text-white text-[10px] font-bold rounded-full flex items-center justify-center">
                      {state.notifCount > 9 ? "9+" : state.notifCount}
                    </span>
                  )}
                </Link>
                <LayoutSwitcher variant={isLight ? "light" : "dark"} />
                <button
                  onClick={state.cycleTheme}
                  className="p-1.5 text-dark-400 hover:text-dark-200 transition-colors rounded"
                  title={`Theme: ${state.theme}`}
                  aria-label="Cycle theme"
                >
                  <svg className="w-3.5 h-3.5" fill="none" viewBox="0 0 24 24" stroke="currentColor" strokeWidth={1.5}>
                    <path strokeLinecap="round" strokeLinejoin="round" d="M4.098 19.902a3.75 3.75 0 0 0 5.304 0l6.401-6.402M6.75 21A3.75 3.75 0 0 1 3 17.25V4.125C3 3.504 3.504 3 4.125 3h5.25c.621 0 1.125.504 1.125 1.125v4.072M6.75 21a3.75 3.75 0 0 0 3.75-3.75V8.197M6.75 21h13.125c.621 0 1.125-.504 1.125-1.125v-5.25c0-.621-.504-1.125-1.125-1.125h-4.072M10.5 8.197l2.88-2.88c.438-.439 1.15-.439 1.59 0l3.712 3.713c.44.44.44 1.152 0 1.59l-2.88 2.88M6.75 17.25h.008v.008H6.75v-.008Z" />
                  </svg>
                </button>
              </div>
            </div>
          </div>
        )}

        {/* Minimal sidebar footer when header is shown */}
        {showHeader && (
          <div className="px-4 py-3 border-t border-dark-600/50">
            <div className="flex items-center gap-2">
              <div className={`w-2 h-2 rounded-full shrink-0 ${state.apiHealthy === null ? "bg-dark-400" : state.apiHealthy ? "bg-rust-500" : "bg-danger-500 animate-pulse"}`} />
              <span className="text-[10px] text-dark-400 flex-1">{state.apiHealthy === null ? "Checking..." : state.apiHealthy ? "All Systems OK" : "Issues Detected"}</span>
            </div>
          </div>
        )}
      </aside>

      {/* Main content */}
      <div className={`flex-1 min-w-0 flex flex-col ${showHeader ? "h-screen overflow-hidden" : "overflow-hidden"}`}>
        {/* Top header bar (optional, replaces controls from sidebar footer) */}
        {showHeader && (
          <header className="h-16 border-b flex items-center justify-between px-6 shrink-0 bg-dark-900 border-dark-700">
            {/* Mobile hamburger */}
            <button
              className="p-2 md:hidden rounded-lg text-dark-400 hover:text-dark-50"
              onClick={() => state.setSidebarOpen(true)}
              aria-label="Open menu"
            >
              <svg className="w-5 h-5" fill="none" viewBox="0 0 24 24" stroke="currentColor" strokeWidth={2}><path strokeLinecap="round" strokeLinejoin="round" d="M3.75 6.75h16.5M3.75 12h16.5m-16.5 5.25h16.5" /></svg>
            </button>

            {/* Search */}
            <div className="flex-1 flex items-center max-w-md">
              <button
                onClick={() => window.dispatchEvent(new KeyboardEvent("keydown", { key: "k", ctrlKey: true }))}
                className="w-full flex items-center gap-2 pl-3 pr-4 py-2 border rounded-lg text-sm transition-all bg-dark-950 border-dark-700 text-dark-400 hover:border-rust-500/50"
              >
                <svg className="w-4 h-4" fill="none" viewBox="0 0 24 24" stroke="currentColor" strokeWidth={2}><circle cx="11" cy="11" r="8" /><line x1="21" y1="21" x2="16.65" y2="16.65" /></svg>
                <span className="flex-1 text-left">Search...</span>
                <kbd className="text-[10px] px-1.5 py-0.5 border rounded border-dark-700 bg-dark-800 text-dark-500">Ctrl K</kbd>
              </button>
            </div>

            {/* Right side */}
            <div className="flex items-center gap-3">
              <Link to="/notifications" className="relative p-2 rounded-lg transition-colors text-dark-400 hover:text-dark-50 hover:bg-dark-800" title="Notifications" aria-label="Notifications">
                <svg className="w-4 h-4" fill="none" viewBox="0 0 24 24" stroke="currentColor" strokeWidth={1.5}>
                  <path strokeLinecap="round" strokeLinejoin="round" d="M14.857 17.082a23.848 23.848 0 005.454-1.31A8.967 8.967 0 0118 9.75v-.7V9A6 6 0 006 9v.75a8.967 8.967 0 01-2.312 6.022c1.733.64 3.56 1.085 5.455 1.31m5.714 0a24.255 24.255 0 01-5.714 0m5.714 0a3 3 0 11-5.714 0" />
                </svg>
                {state.notifCount > 0 && (
                  <span className="absolute top-0.5 right-0.5 w-4 h-4 bg-danger-500 text-white text-[10px] font-bold rounded-full flex items-center justify-center">
                    {state.notifCount > 9 ? "9+" : state.notifCount}
                  </span>
                )}
              </Link>
              {state.firingCount > 0 && (
                <Link to="/monitoring" className="flex items-center gap-1.5 px-2.5 py-1.5 text-xs font-bold bg-danger-500/15 text-danger-400 rounded-lg hover:bg-danger-500/25 transition-colors">
                  <svg className="w-4 h-4" fill="none" viewBox="0 0 24 24" stroke="currentColor" strokeWidth={1.5}><path d="M14.857 17.082a23.848 23.848 0 005.454-1.31A8.967 8.967 0 0118 9.75v-.7V9A6 6 0 006 9v.75a8.967 8.967 0 01-2.312 6.022c1.733.64 3.56 1.085 5.455 1.31m5.714 0a24.255 24.255 0 01-5.714 0m5.714 0a3 3 0 11-5.714 0" /></svg>
                  {state.firingCount}
                </Link>
              )}
              <div className="h-6 w-px hidden sm:block bg-dark-700" />
              <button onClick={state.cycleTheme} className="p-2 rounded-lg transition-colors text-dark-400 hover:text-dark-50 hover:bg-dark-800" title={`Theme: ${state.theme}`}>
                <svg className="w-4 h-4" fill="none" viewBox="0 0 24 24" stroke="currentColor" strokeWidth={1.5}>
                  <path strokeLinecap="round" strokeLinejoin="round" d="M4.098 19.902a3.75 3.75 0 0 0 5.304 0l6.401-6.402M6.75 21A3.75 3.75 0 0 1 3 17.25V4.125C3 3.504 3.504 3 4.125 3h5.25c.621 0 1.125.504 1.125 1.125v4.072M6.75 21a3.75 3.75 0 0 0 3.75-3.75V8.197M6.75 21h13.125c.621 0 1.125-.504 1.125-1.125v-5.25c0-.621-.504-1.125-1.125-1.125h-4.072M10.5 8.197l2.88-2.88c.438-.439 1.15-.439 1.59 0l3.712 3.713c.44.44.44 1.152 0 1.59l-2.88 2.88M6.75 17.25h.008v.008H6.75v-.008Z" />
                </svg>
              </button>
              <div className="hidden sm:block"><LayoutSwitcher variant={isLight ? "light" : "dark"} /></div>
              <div className="hidden sm:flex items-center gap-2">
                <div className="w-8 h-8 rounded-full flex items-center justify-center font-semibold text-sm bg-rust-500/15 text-rust-400">
                  {state.user.email[0]?.toUpperCase()}
                </div>
                <div className="text-left">
                  <p className="text-sm font-medium leading-none text-dark-100">{state.user.email.split("@")[0]}</p>
                  <p className="text-xs mt-0.5 text-dark-500">{state.user.role}</p>
                </div>
              </div>
              <button onClick={state.logout} className="p-1.5 text-dark-400 hover:text-dark-100 rounded-lg" title="Logout"><Icon name="logout" className="w-4 h-4" /></button>
            </div>
          </header>
        )}

        {/* Mobile header (only when no top header) */}
        {!showHeader && (
          <div className="sticky top-0 z-30 flex items-center gap-3 px-4 py-3 bg-dark-900/80 backdrop-blur-lg border-b border-dark-600 md:hidden">
            <button onClick={() => state.setSidebarOpen(true)} className="p-2 text-dark-200 hover:text-dark-50 hover:bg-dark-700 rounded-lg" aria-label="Open sidebar">
              <svg className="w-6 h-6" fill="none" viewBox="0 0 24 24" stroke="currentColor" strokeWidth={2}><path strokeLinecap="round" strokeLinejoin="round" d="M3.75 6.75h16.5M3.75 12h16.5m-16.5 5.25h16.5" /></svg>
            </button>
            <span className="text-base font-bold logo-glow">
              {branding.hideBranding ? "" : branding.panelName === "DockPanel" ? <><span className="text-rust-400">Dock</span><span className="text-dark-50">Panel</span></> : <span className="text-dark-50">{branding.panelName}</span>}
            </span>
          </div>
        )}

        {/* 2FA warning */}
        {state.twoFaEnforced && !state.twoFaEnabled && (
          <div className={`border-b px-4 py-3 flex items-center justify-between ${isLight ? "bg-warn-500/5 border-warn-500/20" : "bg-warn-500/10 border-warn-500/20"}`}>
            <div className="flex items-center gap-2">
              <svg className="w-5 h-5 text-warn-400" fill="none" viewBox="0 0 24 24" stroke="currentColor" strokeWidth={1.5}>
                <path strokeLinecap="round" strokeLinejoin="round" d="M12 9v3.75m-9.303 3.376c-.866 1.5.217 3.374 1.948 3.374h14.71c1.73 0 2.813-1.874 1.948-3.374L13.949 3.378c-.866-1.5-3.032-1.5-3.898 0L2.697 16.126ZM12 15.75h.007v.008H12v-.008Z" />
              </svg>
              <span className="text-sm text-warn-400 font-medium">Two-factor authentication is required. Please enable 2FA in Settings &rarr; Security.</span>
            </div>
            <a href="/settings" className="px-3 py-1.5 bg-warn-500 text-dark-900 rounded text-xs font-bold hover:bg-warn-400 transition-colors">Set Up 2FA</a>
          </div>
        )}

        <main id="main-content" className={`flex-1 ${showHeader ? "overflow-y-auto bg-dark-950" : "overflow-auto"}`}>
          <Outlet />
        </main>
      </div>
    </div>
  );
}
