import { Navigate, Outlet, NavLink, Link, useNavigate } from "react-router-dom";
import { useLayoutState } from "../hooks/useLayoutState";
import { useServer } from "../context/ServerContext";
import { useBranding } from "../context/BrandingContext";
import CommandPalette from "./CommandPalette";
import LayoutSwitcher from "./LayoutSwitcher";
import React, { useState, useRef, useEffect } from "react";

/* ── Lucide-style inline SVG icons (blue accent) ──────────────────────── */
const icons: Record<string, React.ReactNode> = {
  dashboard: <svg className="w-5 h-5" fill="none" viewBox="0 0 24 24" stroke="currentColor" strokeWidth={1.5}><rect x="3" y="3" width="7" height="7" rx="1" /><rect x="14" y="3" width="7" height="7" rx="1" /><rect x="3" y="14" width="7" height="7" rx="1" /><rect x="14" y="14" width="7" height="7" rx="1" /></svg>,
  sites: <svg className="w-5 h-5" fill="none" viewBox="0 0 24 24" stroke="currentColor" strokeWidth={1.5}><circle cx="12" cy="12" r="10" /><path d="M2 12h20M12 2a15.3 15.3 0 0 1 4 10 15.3 15.3 0 0 1-4 10 15.3 15.3 0 0 1-4-10 15.3 15.3 0 0 1 4-10z" /></svg>,
  databases: <svg className="w-5 h-5" fill="none" viewBox="0 0 24 24" stroke="currentColor" strokeWidth={1.5}><ellipse cx="12" cy="5" rx="9" ry="3" /><path d="M21 12c0 1.66-4 3-9 3s-9-1.34-9-3" /><path d="M3 5v14c0 1.66 4 3 9 3s9-1.34 9-3V5" /></svg>,
  wordpress: <svg className="w-5 h-5" fill="none" viewBox="0 0 24 24" stroke="currentColor" strokeWidth={1.5}><circle cx="12" cy="12" r="10" /><path d="M8 12l4 8 2-6" /><path d="M6 8h12" /></svg>,
  apps: <svg className="w-5 h-5" fill="none" viewBox="0 0 24 24" stroke="currentColor" strokeWidth={1.5}><path d="M21 16V8a2 2 0 0 0-1-1.73l-7-4a2 2 0 0 0-2 0l-7 4A2 2 0 0 0 3 8v8a2 2 0 0 0 1 1.73l7 4a2 2 0 0 0 2 0l7-4A2 2 0 0 0 21 16z" /><polyline points="3.27 6.96 12 12.01 20.73 6.96" /><line x1="12" y1="22.08" x2="12" y2="12" /></svg>,
  gitDeploys: <svg className="w-5 h-5" fill="none" viewBox="0 0 24 24" stroke="currentColor" strokeWidth={1.5}><circle cx="12" cy="12" r="4" /><line x1="1.05" y1="12" x2="7" y2="12" /><line x1="17.01" y1="12" x2="22.96" y2="12" /></svg>,
  servers: <svg className="w-5 h-5" fill="none" viewBox="0 0 24 24" stroke="currentColor" strokeWidth={1.5}><rect x="2" y="2" width="20" height="8" rx="2" /><rect x="2" y="14" width="20" height="8" rx="2" /><circle cx="6" cy="6" r="1" fill="currentColor" /><circle cx="6" cy="18" r="1" fill="currentColor" /></svg>,
  dns: <svg className="w-5 h-5" fill="none" viewBox="0 0 24 24" stroke="currentColor" strokeWidth={1.5}><path d="M12 2L2 7l10 5 10-5-10-5z" /><path d="M2 17l10 5 10-5" /><path d="M2 12l10 5 10-5" /></svg>,
  mail: <svg className="w-5 h-5" fill="none" viewBox="0 0 24 24" stroke="currentColor" strokeWidth={1.5}><rect x="2" y="4" width="20" height="16" rx="2" /><path d="M22 4L12 13 2 4" /></svg>,
  monitoring: <svg className="w-5 h-5" fill="none" viewBox="0 0 24 24" stroke="currentColor" strokeWidth={1.5}><polyline points="22 12 18 12 15 21 9 3 6 12 2 12" /></svg>,
  logs: <svg className="w-5 h-5" fill="none" viewBox="0 0 24 24" stroke="currentColor" strokeWidth={1.5}><path d="M14 2H6a2 2 0 0 0-2 2v16a2 2 0 0 0 2 2h12a2 2 0 0 0 2-2V8z" /><polyline points="14 2 14 8 20 8" /><line x1="16" y1="13" x2="8" y2="13" /><line x1="16" y1="17" x2="8" y2="17" /></svg>,
  terminal: <svg className="w-5 h-5" fill="none" viewBox="0 0 24 24" stroke="currentColor" strokeWidth={1.5}><polyline points="4 17 10 11 4 5" /><line x1="12" y1="19" x2="20" y2="19" /></svg>,
  security: <svg className="w-5 h-5" fill="none" viewBox="0 0 24 24" stroke="currentColor" strokeWidth={1.5}><path d="M12 22s8-4 8-10V5l-8-3-8 3v7c0 6 8 10 8 10z" /></svg>,
  settings: <svg className="w-5 h-5" fill="none" viewBox="0 0 24 24" stroke="currentColor" strokeWidth={1.5}><circle cx="12" cy="12" r="3" /><path d="M19.4 15a1.65 1.65 0 0 0 .33 1.82l.06.06a2 2 0 0 1 0 2.83 2 2 0 0 1-2.83 0l-.06-.06a1.65 1.65 0 0 0-1.82-.33 1.65 1.65 0 0 0-1 1.51V21a2 2 0 0 1-2 2 2 2 0 0 1-2-2v-.09A1.65 1.65 0 0 0 9 19.4a1.65 1.65 0 0 0-1.82.33l-.06.06a2 2 0 0 1-2.83 0 2 2 0 0 1 0-2.83l.06-.06A1.65 1.65 0 0 0 4.68 15a1.65 1.65 0 0 0-1.51-1H3a2 2 0 0 1-2-2 2 2 0 0 1 2-2h.09A1.65 1.65 0 0 0 4.6 9a1.65 1.65 0 0 0-.33-1.82l-.06-.06a2 2 0 0 1 0-2.83 2 2 0 0 1 2.83 0l.06.06A1.65 1.65 0 0 0 9 4.68a1.65 1.65 0 0 0 1-1.51V3a2 2 0 0 1 2-2 2 2 0 0 1 2 2v.09a1.65 1.65 0 0 0 1 1.51 1.65 1.65 0 0 0 1.82-.33l.06-.06a2 2 0 0 1 2.83 0 2 2 0 0 1 0 2.83l-.06.06a1.65 1.65 0 0 0-.33 1.82V9a1.65 1.65 0 0 0 1.51 1H21a2 2 0 0 1 2 2 2 2 0 0 1-2 2h-.09a1.65 1.65 0 0 0-1.51 1z" /></svg>,
};

function getIcon(name: string) {
  return icons[name] || icons.dashboard;
}

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
        <div className={`w-2 h-2 rounded-full shrink-0 ${activeServer?.status === "online" ? "bg-emerald-500" : activeServer?.status === "offline" ? "bg-rose-500" : "bg-slate-500"}`} />
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
              <div className={`w-2 h-2 rounded-full shrink-0 ${s.status === "online" ? "bg-emerald-500" : s.status === "offline" ? "bg-rose-500" : "bg-slate-500"}`} />
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

  // Nexus layout supports light/dark
  const [isDark, setIsDark] = useState(() => {
    const stored = localStorage.getItem("dp-nexus-dark");
    return stored === "1";
  });

  // Save previous theme on first mount so it can be restored when leaving Nexus
  useEffect(() => {
    const currentTheme = localStorage.getItem("dp-theme") || "terminal";
    if (currentTheme !== "nexus" && currentTheme !== "nexus-dark") {
      localStorage.setItem("dp-pre-nexus-theme", currentTheme);
    }
  }, []);

  useEffect(() => {
    const theme = isDark ? "nexus-dark" : "nexus";
    document.documentElement.setAttribute("data-theme", theme);
    localStorage.setItem("dp-theme", theme);
    localStorage.setItem("dp-nexus-dark", isDark ? "1" : "0");
  }, [isDark]);

  const toggleDark = () => setIsDark(prev => !prev);

  if (state.loading) {
    return (
      <div className="flex items-center justify-center h-screen bg-slate-50">
        <div className="w-8 h-8 border-4 border-blue-500 border-t-transparent rounded-full animate-spin" />
      </div>
    );
  }

  if (!state.user.email) return <Navigate to="/login" replace />;

  // Flatten nav groups for Nexus (flat sidebar, no group headers)
  const allItems = state.visibleGroups.flatMap(g => g.items);

  return (
    <div className={`flex h-screen font-sans overflow-hidden ${isDark ? "bg-[#0d1117]" : "bg-slate-50"}`}>
      <CommandPalette />

      {/* Skip to content */}
      <a href="#main-content" className="sr-only focus:not-sr-only focus:absolute focus:z-[100] focus:top-2 focus:left-2 focus:px-4 focus:py-2 focus:bg-blue-600 focus:text-white focus:rounded-lg">
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
        className={`fixed inset-y-0 left-0 z-50 w-64 flex flex-col h-screen border-r transform transition-transform duration-200 ease-in-out md:relative md:translate-x-0 ${
          isDark ? "bg-[#161b22] text-gray-300 border-gray-700/40 shadow-xl shadow-black/20" : "bg-white text-slate-600 border-slate-200 shadow-sm"
        } ${state.sidebarOpen ? "translate-x-0" : "-translate-x-full"}`}
      >
        {/* Logo */}
        <div className={`h-16 flex items-center justify-between px-6 border-b ${isDark ? "border-[#2d333b]/60" : "border-slate-200"}`}>
          <Link to="/" className="flex items-center gap-3 hover:opacity-90 transition-opacity">
            {branding.logoUrl ? (
              <img src={branding.logoUrl} alt={branding.panelName} className="h-8 w-auto max-w-[160px] object-contain" />
            ) : (
              <>
                <div className="p-1.5 bg-blue-600 rounded-lg">
                  <svg className="w-5 h-5 text-white" fill="none" viewBox="0 0 24 24" stroke="currentColor" strokeWidth={2}>
                    <rect x="2" y="2" width="20" height="8" rx="2" />
                    <rect x="2" y="14" width="20" height="8" rx="2" />
                    <circle cx="6" cy="6" r="1" fill="currentColor" />
                    <circle cx="6" cy="18" r="1" fill="currentColor" />
                  </svg>
                </div>
                {!branding.hideBranding && (
                  <span className={`text-lg font-bold tracking-tight ${isDark ? "text-gray-100" : "text-slate-900"}`}>
                    {branding.panelName}
                  </span>
                )}
              </>
            )}
          </Link>
          <button
            onClick={() => state.setSidebarOpen(false)}
            className={`p-1.5 md:hidden rounded-lg ${isDark ? "text-gray-400 hover:text-gray-100" : "text-slate-400 hover:text-slate-900"}`}
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
              end={item.to === "/"}
              onClick={() => state.setSidebarOpen(false)}
              className={({ isActive }) =>
                `flex items-center gap-3 px-3 py-2.5 text-sm font-medium rounded-lg transition-colors ${
                  isActive
                    ? isDark ? "bg-blue-500/10 text-blue-400" : "bg-blue-50 text-blue-700"
                    : isDark ? "text-[#8b949e] hover:bg-[#1c2333] hover:text-[#e6edf3]" : "text-slate-600 hover:bg-slate-50 hover:text-slate-900"
                }`
              }
            >
              {({ isActive }) => (
                <>
                  <span className={isActive ? (isDark ? "text-blue-400" : "text-blue-700") : (isDark ? "text-gray-500" : "text-slate-400")}>{getIcon(item.iconName)}</span>
                  <span>{item.label}</span>
                  {item.to === "/monitoring" && state.firingCount > 0 && (
                    <span className="ml-auto px-1.5 py-0.5 text-xs font-bold bg-rose-500 text-white rounded-full min-w-[20px] text-center">
                      {state.firingCount}
                    </span>
                  )}
                </>
              )}
            </NavLink>
          ))}
        </nav>

        {/* Footer */}
        <div className={`p-4 border-t space-y-3 ${isDark ? "border-[#2d333b]/60" : "border-slate-200"}`}>
          {/* Health */}
          <div className={`flex items-center gap-2.5 px-3 py-2 rounded-lg ${
  state.apiHealthy === true ? (isDark ? "bg-emerald-500/10 border border-emerald-500/10" : "bg-emerald-50") :
  state.apiHealthy === false ? (isDark ? "bg-rose-500/10 border border-rose-500/10" : "bg-rose-50") :
  (isDark ? "bg-[#1c2333]" : "bg-slate-50")
}`}>
            <div className={`w-2 h-2 rounded-full shrink-0 ${state.apiHealthy === true ? "bg-emerald-500" : state.apiHealthy === false ? "bg-rose-500" : "bg-slate-400"}`} />
            <span className={`text-xs font-medium ${
  state.apiHealthy === true ? (isDark ? "text-emerald-400" : "text-emerald-700") :
  state.apiHealthy === false ? (isDark ? "text-rose-400" : "text-rose-700") :
  (isDark ? "text-gray-500" : "text-slate-500")
}`}>
              {state.apiHealthy === true ? "All Systems OK" : state.apiHealthy === false ? "Issues Detected" : "Checking..."}
            </span>
          </div>
          {/* User + layout + logout */}
          <div className="flex items-center gap-2 px-3">
            <div className="w-7 h-7 bg-blue-600 rounded-full flex items-center justify-center text-white text-xs font-semibold shrink-0">
              {state.user.email[0]?.toUpperCase()}
            </div>
            <span className={`flex-1 text-sm truncate ${isDark ? "text-gray-400" : "text-slate-600"}`}>{state.user.email}</span>
            <button
              onClick={state.logout}
              className="p-1 text-slate-400 hover:text-rose-400 transition-colors"
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
        <header className={`h-16 border-b flex items-center justify-between px-6 shrink-0 ${isDark ? "bg-[#161b22] border-gray-700/40" : "bg-white border-slate-200"}`}>
          {/* Mobile hamburger */}
          <button
            className={`p-2 md:hidden rounded-lg ${isDark ? "text-gray-400 hover:text-gray-100" : "text-slate-400 hover:text-slate-900"}`}
            onClick={() => state.setSidebarOpen(true)}
            aria-label="Open menu"
          >
            <svg className="w-5 h-5" fill="none" viewBox="0 0 24 24" stroke="currentColor" strokeWidth={2}><path strokeLinecap="round" strokeLinejoin="round" d="M3.75 6.75h16.5M3.75 12h16.5m-16.5 5.25h16.5" /></svg>
          </button>

          {/* Search */}
          <div className="flex-1 flex items-center max-w-md">
            <button
              onClick={() => window.dispatchEvent(new KeyboardEvent("keydown", { key: "k", ctrlKey: true }))}
              className={`w-full flex items-center gap-2 pl-3 pr-4 py-2 border rounded-lg text-sm transition-all ${
  isDark ? "bg-[#0d1117] border-[#2d333b] text-[#8b949e] hover:border-blue-500/50" : "bg-slate-50 border-slate-200 text-slate-400 hover:border-blue-300 hover:bg-white"
}`}
            >
              <svg className="w-4 h-4" fill="none" viewBox="0 0 24 24" stroke="currentColor" strokeWidth={2}><circle cx="11" cy="11" r="8" /><line x1="21" y1="21" x2="16.65" y2="16.65" /></svg>
              <span className="flex-1 text-left">Search...</span>
              <kbd className={`text-[10px] px-1.5 py-0.5 border rounded ${isDark ? "border-[#2d333b] bg-[#1c2333] text-[#636e7b]" : "border-slate-300 bg-white text-slate-400"}`}>Ctrl K</kbd>
            </button>
          </div>

          {/* Right side */}
          <div className="flex items-center gap-3">
            {/* Alert badge */}
            {state.firingCount > 0 && (
              <Link to="/monitoring" className="relative p-2 text-slate-400 hover:bg-slate-100 rounded-full transition-colors">
                <svg className="w-5 h-5" fill="none" viewBox="0 0 24 24" stroke="currentColor" strokeWidth={1.5}><path d="M14.857 17.082a23.848 23.848 0 005.454-1.31A8.967 8.967 0 0118 9.75v-.7V9A6 6 0 006 9v.75a8.967 8.967 0 01-2.312 6.022c1.733.64 3.56 1.085 5.455 1.31m5.714 0a24.255 24.255 0 01-5.714 0m5.714 0a3 3 0 11-5.714 0" /></svg>
                <span className="absolute top-1 right-1 w-2.5 h-2.5 bg-rose-500 rounded-full border-2 border-white" />
              </Link>
            )}

            <div className={`h-6 w-px hidden sm:block ${isDark ? "bg-[#2d333b]" : "bg-slate-200"}`} />

            {/* Dark mode toggle */}
            <button
              onClick={toggleDark}
              className={`p-2 rounded-lg transition-colors ${isDark ? "text-yellow-400 hover:bg-[#1c2333]" : "text-slate-400 hover:bg-slate-100"}`}
              title={isDark ? "Switch to light mode" : "Switch to dark mode"}
              aria-label="Toggle dark mode"
            >
              {isDark ? (
                <svg className="w-5 h-5" fill="none" viewBox="0 0 24 24" stroke="currentColor" strokeWidth={1.5}><circle cx="12" cy="12" r="5" /><line x1="12" y1="1" x2="12" y2="3" /><line x1="12" y1="21" x2="12" y2="23" /><line x1="4.22" y1="4.22" x2="5.64" y2="5.64" /><line x1="18.36" y1="18.36" x2="19.78" y2="19.78" /><line x1="1" y1="12" x2="3" y2="12" /><line x1="21" y1="12" x2="23" y2="12" /><line x1="4.22" y1="19.78" x2="5.64" y2="18.36" /><line x1="18.36" y1="5.64" x2="19.78" y2="4.22" /></svg>
              ) : (
                <svg className="w-5 h-5" fill="none" viewBox="0 0 24 24" stroke="currentColor" strokeWidth={1.5}><path strokeLinecap="round" strokeLinejoin="round" d="M21.752 15.002A9.718 9.718 0 0118 15.75c-5.385 0-9.75-4.365-9.75-9.75 0-1.33.266-2.597.748-3.752A9.753 9.753 0 003 11.25C3 16.635 7.365 21 12.75 21a9.753 9.753 0 009.002-5.998z" /></svg>
              )}
            </button>

            {/* Layout switcher */}
            <div className="hidden sm:block">
              <LayoutSwitcher variant="light" />
            </div>

            {/* User info (desktop) */}
            <div className="hidden sm:flex items-center gap-2">
              <div className={`w-8 h-8 rounded-full flex items-center justify-center font-semibold text-sm ${isDark ? "bg-blue-500/15 text-blue-400" : "bg-blue-100 text-blue-600"}`}>
                {state.user.email[0]?.toUpperCase()}{state.user.email[1]?.toUpperCase()}
              </div>
              <div className="text-left">
                <p className={`text-sm font-medium leading-none ${isDark ? "text-gray-100" : "text-slate-900"}`}>{state.user.email.split("@")[0]}</p>
                <p className={`text-xs mt-0.5 ${isDark ? "text-gray-500" : "text-slate-400"}`}>{state.user.role}</p>
              </div>
            </div>
          </div>
        </header>

        {/* 2FA enforcement banner */}
        {state.twoFaEnforced && !state.twoFaEnabled && (
          <div className={`border-b px-6 py-2 text-sm flex items-center gap-2 ${isDark ? "bg-amber-500/10 border-amber-500/20 text-amber-300" : "bg-amber-50 border-amber-200 text-amber-800"}`}>
            <svg className="w-4 h-4 text-amber-500 shrink-0" fill="none" viewBox="0 0 24 24" stroke="currentColor" strokeWidth={2}><path strokeLinecap="round" strokeLinejoin="round" d="M12 9v3.75m-9.303 3.376c-.866 1.5.217 3.374 1.948 3.374h14.71c1.73 0 2.813-1.874 1.948-3.374L13.949 3.378c-.866-1.5-3.032-1.5-3.898 0L2.697 16.126z" /><path strokeLinecap="round" strokeLinejoin="round" d="M12 15.75h.007v.008H12v-.008z" /></svg>
            <span>Two-factor authentication is required.</span>
            <Link to="/settings" className="font-medium text-amber-900 underline hover:no-underline ml-1">Set up 2FA</Link>
          </div>
        )}

        {/* Content */}
        <main id="main-content" className={`flex-1 overflow-y-auto ${isDark ? "bg-[#0d1117]" : "bg-slate-50"}`}>
          <Outlet />
        </main>
      </div>
    </div>
  );
}
