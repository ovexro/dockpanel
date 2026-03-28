import { Navigate, Outlet, NavLink, Link, useNavigate } from "react-router-dom";
import { useLayoutState } from "../hooks/useLayoutState";
import { useServer } from "../context/ServerContext";
import { useBranding } from "../context/BrandingContext";
import CommandPalette from "./CommandPalette";
import LayoutSwitcher from "./LayoutSwitcher";
import { Icon } from "../data/icons";
import { useState, useRef, useEffect } from "react";

/* ── Server Selector (Nexus style) ────────────────────────────────────── */
function ServerSelector() {
  const { servers, activeServer, setActiveServerId } = useServer();
  const [open, setOpen] = useState(false);
  const ref = useRef<HTMLDivElement>(null);

  useEffect(() => {
    function handleClick(e: MouseEvent) {
      if (ref.current && !ref.current.contains(e.target as Node)) setOpen(false);
    }
    document.addEventListener("mousedown", handleClick);
    return () => document.removeEventListener("mousedown", handleClick);
  }, []);

  if (servers.length <= 1) return null;

  return (
    <div className="px-3 pt-3" ref={ref}>
      <button
        onClick={() => setOpen(!open)}
        className="w-full flex items-center gap-2 px-3 py-2 rounded-lg bg-dark-800 border border-dark-600 text-sm text-dark-300 hover:text-dark-50 hover:border-dark-400 transition-colors"
      >
        <div className={`w-2 h-2 rounded-full shrink-0 ${activeServer?.status === "online" ? "bg-rust-500" : activeServer?.status === "offline" ? "bg-danger-500" : "bg-dark-400"}`} />
        <span className="flex-1 text-left truncate">{activeServer?.name || "Select server"}</span>
        <svg className={`w-4 h-4 transition-transform ${open ? "rotate-180" : ""}`} fill="none" viewBox="0 0 24 24" stroke="currentColor" strokeWidth={2}><path strokeLinecap="round" strokeLinejoin="round" d="M19.5 8.25l-7.5 7.5-7.5-7.5" /></svg>
      </button>
      {open && (
        <div className="mt-1 bg-dark-900 border border-dark-600 rounded-lg shadow-xl overflow-hidden">
          {servers.map((s) => (
            <button
              key={s.id}
              onClick={() => { setActiveServerId(s.id); setOpen(false); window.location.href = "/"; }}
              className={`w-full flex items-center gap-2 px-3 py-2 text-sm text-left hover:bg-dark-800 transition-colors ${s.id === activeServer?.id ? "bg-dark-800 text-dark-50" : "text-dark-400"}`}
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

/* ── Main Layout ──────────────────────────────────────────────────────── */
export default function NexusLayout() {
  const state = useLayoutState();
  const branding = useBranding();

  // Light theme detection — only needed for status color tones (emerald/rose/amber)
  const isLight = state.theme === "clean" || state.theme === "arctic";

  if (state.loading) {
    return (
      <div className="flex items-center justify-center h-screen bg-dark-950">
        <div className="w-8 h-8 border-4 border-rust-500 border-t-transparent rounded-full animate-spin" />
      </div>
    );
  }

  if (!state.user.email) return <Navigate to="/login" replace />;

  // Flatten nav groups for Nexus (flat sidebar, no group headers)
  const allItems = state.visibleGroups.flatMap(g => g.items);

  return (
    <div className="flex h-screen font-sans overflow-hidden bg-dark-950">
      <CommandPalette />

      {/* Skip to content */}
      <a href="#main-content" className="sr-only focus:not-sr-only focus:absolute focus:z-[100] focus:top-2 focus:left-2 focus:px-4 focus:py-2 focus:bg-rust-600 focus:text-white focus:rounded-lg">
        Skip to main content
      </a>

      {/* Mobile overlay */}
      {state.sidebarOpen && (
        <div
          className="fixed inset-0 bg-black/40 z-40 md:hidden"
          role="presentation"
          onClick={() => state.setSidebarOpen(false)}
        />
      )}

      {/* ── Sidebar ──────────────────────────────────────────────────── */}
      <aside
        className={`fixed inset-y-0 left-0 z-50 w-64 flex flex-col h-screen border-r bg-dark-900 text-dark-300 border-dark-700 shadow-xl shadow-black/20 transform transition-transform duration-200 ease-in-out md:relative md:translate-x-0 ${
          state.sidebarOpen ? "translate-x-0" : "-translate-x-full"
        }`}
      >
        {/* Logo */}
        <div className="h-16 flex items-center justify-between px-6 border-b border-dark-700">
          <Link to="/" className="flex items-center gap-3 hover:opacity-90 transition-opacity">
            {branding.logoUrl ? (
              <img src={branding.logoUrl} alt={branding.panelName} className="h-8 w-auto max-w-[160px] object-contain" />
            ) : (
              <>
                <div className="w-8 h-8 bg-rust-500 rounded-lg flex items-center justify-center">
                  <svg className="w-5 h-5 text-dark-950" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="2.5" aria-hidden="true">
                    <path d="M5 16h4" strokeLinecap="square" />
                    <path d="M5 12h8" strokeLinecap="square" />
                    <path d="M5 8h6" strokeLinecap="square" />
                    <rect x="16" y="7" width="4" height="4" fill="currentColor" stroke="none" />
                    <rect x="16" y="13" width="4" height="4" fill="currentColor" stroke="none" />
                  </svg>
                </div>
                {!branding.hideBranding && (
                  <span className="text-lg font-bold tracking-tight">
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
            className="p-1.5 md:hidden rounded-lg text-dark-400 hover:text-dark-50"
            aria-label="Close sidebar"
          >
            <svg className="w-5 h-5" fill="none" viewBox="0 0 24 24" stroke="currentColor" strokeWidth={2}><path strokeLinecap="round" strokeLinejoin="round" d="M6 18L18 6M6 6l12 12" /></svg>
          </button>
        </div>

        <ServerSelector />

        {/* Nav — flat list, no group headers */}
        <nav className="flex-1 py-4 px-3 space-y-1 overflow-y-auto">
          {allItems.map((item) => (
            <NavLink
              key={item.to}
              to={item.to}
              end
              onClick={() => state.setSidebarOpen(false)}
              className={({ isActive }) =>
                `flex items-center gap-3 px-3 py-2.5 text-sm font-medium rounded-lg transition-colors ${
                  isActive
                    ? "bg-rust-500/10 text-rust-400"
                    : "text-dark-400 hover:bg-dark-800 hover:text-dark-50"
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
                </>
              )}
            </NavLink>
          ))}
        </nav>

        {/* Footer */}
        <div className="p-4 border-t border-dark-700 space-y-3">
          {/* Health */}
          <div className={`flex items-center gap-2.5 px-3 py-2 rounded-lg ${
  state.apiHealthy === true ? "bg-rust-500/10 border border-rust-500/10" :
  state.apiHealthy === false ? "bg-danger-500/10 border border-danger-500/10" :
  "bg-dark-800"
}`}>
            <div className={`w-2 h-2 rounded-full shrink-0 ${state.apiHealthy === true ? "bg-rust-500" : state.apiHealthy === false ? "bg-danger-500 animate-pulse" : "bg-dark-400"}`} />
            <span className={`text-xs font-medium ${
  state.apiHealthy === true ? "text-rust-400" :
  state.apiHealthy === false ? "text-danger-400" :
  "text-dark-500"
}`}>
              {state.apiHealthy === true ? "All Systems OK" : state.apiHealthy === false ? "Issues Detected" : "Checking..."}
            </span>
          </div>
          {/* User + layout + logout */}
          <div className="flex items-center gap-2 px-3">
            <div className="w-8 h-8 rounded-full bg-rust-500/15 flex items-center justify-center text-rust-400 text-xs font-bold shrink-0">
              {state.user.email[0]?.toUpperCase()}
            </div>
            <span className="flex-1 text-sm truncate text-dark-400">{state.user.email}</span>
            <button
              onClick={state.logout}
              className="p-1 text-dark-400 hover:text-danger-400 transition-colors"
              title="Log out"
            >
              <svg className="w-4 h-4" fill="none" viewBox="0 0 24 24" stroke="currentColor" strokeWidth={1.5}><path strokeLinecap="round" strokeLinejoin="round" d="M15.75 9V5.25A2.25 2.25 0 0013.5 3h-6a2.25 2.25 0 00-2.25 2.25v13.5A2.25 2.25 0 007.5 21h6a2.25 2.25 0 002.25-2.25V15m3-3l3-3m0 0l-3-3m3 3H9" /></svg>
            </button>
          </div>
        </div>
      </aside>

      {/* ── Main area ────────────────────────────────────────────────── */}
      <div className="flex-1 flex flex-col h-screen overflow-hidden">

        {/* Header */}
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
            {/* Notification bell */}
            <Link to="/notifications" className="relative p-2 text-dark-400 hover:bg-dark-800 rounded-full transition-colors" title="Notifications" aria-label="Notifications">
              <svg className="w-5 h-5" fill="none" viewBox="0 0 24 24" stroke="currentColor" strokeWidth={1.5}>
                <path strokeLinecap="round" strokeLinejoin="round" d="M14.857 17.082a23.848 23.848 0 005.454-1.31A8.967 8.967 0 0118 9.75v-.7V9A6 6 0 006 9v.75a8.967 8.967 0 01-2.312 6.022c1.733.64 3.56 1.085 5.455 1.31m5.714 0a24.255 24.255 0 01-5.714 0m5.714 0a3 3 0 11-5.714 0" />
              </svg>
              {state.notifCount > 0 && (
                <span className="absolute top-0.5 right-0.5 w-4 h-4 bg-danger-500 text-white text-[10px] font-bold rounded-full flex items-center justify-center">
                  {state.notifCount > 9 ? "9+" : state.notifCount}
                </span>
              )}
            </Link>
            {/* Alert badge */}
            {state.firingCount > 0 && (
              <Link to="/monitoring" className="relative p-2 text-dark-400 hover:bg-dark-800 rounded-full transition-colors">
                <svg className="w-5 h-5" fill="none" viewBox="0 0 24 24" stroke="currentColor" strokeWidth={1.5}><path d="M14.857 17.082a23.848 23.848 0 005.454-1.31A8.967 8.967 0 0118 9.75v-.7V9A6 6 0 006 9v.75a8.967 8.967 0 01-2.312 6.022c1.733.64 3.56 1.085 5.455 1.31m5.714 0a24.255 24.255 0 01-5.714 0m5.714 0a3 3 0 11-5.714 0" /></svg>
                <span className="absolute top-1 right-1 w-2.5 h-2.5 bg-danger-500 rounded-full border-2 border-dark-900" />
              </Link>
            )}

            <div className="h-6 w-px hidden sm:block bg-dark-700" />

            {/* Theme cycle — same paint bucket as other layouts */}
            <button
              onClick={state.cycleTheme}
              className="p-2 rounded-lg transition-colors text-dark-400 hover:text-dark-50 hover:bg-dark-800"
              title={`Theme: ${state.theme}`}
              aria-label="Cycle theme"
            >
              <svg className="w-4.5 h-4.5" fill="none" viewBox="0 0 24 24" stroke="currentColor" strokeWidth={1.5}>
                <path strokeLinecap="round" strokeLinejoin="round" d="M4.098 19.902a3.75 3.75 0 0 0 5.304 0l6.401-6.402M6.75 21A3.75 3.75 0 0 1 3 17.25V4.125C3 3.504 3.504 3 4.125 3h5.25c.621 0 1.125.504 1.125 1.125v4.072M6.75 21a3.75 3.75 0 0 0 3.75-3.75V8.197M6.75 21h13.125c.621 0 1.125-.504 1.125-1.125v-5.25c0-.621-.504-1.125-1.125-1.125h-4.072M10.5 8.197l2.88-2.88c.438-.439 1.15-.439 1.59 0l3.712 3.713c.44.44.44 1.152 0 1.59l-2.88 2.88M6.75 17.25h.008v.008H6.75v-.008Z" />
              </svg>
            </button>

            {/* Layout switcher */}
            <div className="hidden sm:block">
              <LayoutSwitcher variant={isLight ? "light" : "dark"} />
            </div>

            {/* User info (desktop) */}
            <div className="hidden sm:flex items-center gap-2">
              <div className="w-8 h-8 rounded-full flex items-center justify-center font-semibold text-sm bg-rust-500/15 text-rust-400">
                {state.user.email[0]?.toUpperCase()}{state.user.email[1]?.toUpperCase()}
              </div>
              <div className="text-left">
                <p className="text-sm font-medium leading-none text-dark-100">{state.user.email.split("@")[0]}</p>
                <p className="text-xs mt-0.5 text-dark-500">{state.user.role}</p>
              </div>
            </div>
          </div>
        </header>

        {/* 2FA enforcement banner */}
        {state.twoFaEnforced && !state.twoFaEnabled && (
          <div className={`border-b px-6 py-2 text-sm flex items-center gap-2 ${isLight ? "bg-warn-500/5 border-warn-500/20 text-warn-600" : "bg-warn-500/10 border-warn-500/20 text-warn-400"}`}>
            <svg className="w-4 h-4 text-warn-500 shrink-0" fill="none" viewBox="0 0 24 24" stroke="currentColor" strokeWidth={2}><path strokeLinecap="round" strokeLinejoin="round" d="M12 9v3.75m-9.303 3.376c-.866 1.5.217 3.374 1.948 3.374h14.71c1.73 0 2.813-1.874 1.948-3.374L13.949 3.378c-.866-1.5-3.032-1.5-3.898 0L2.697 16.126z" /><path strokeLinecap="round" strokeLinejoin="round" d="M12 15.75h.007v.008H12v-.008z" /></svg>
            <span>Two-factor authentication is required.</span>
            <Link to="/settings" className="font-medium underline hover:no-underline ml-1">Set up 2FA</Link>
          </div>
        )}

        {/* Content */}
        <main id="main-content" className="flex-1 overflow-y-auto bg-dark-950">
          <Outlet />
        </main>
      </div>
    </div>
  );
}
