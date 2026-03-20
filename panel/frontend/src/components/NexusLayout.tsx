import { Navigate, Outlet, NavLink, Link, useNavigate } from "react-router-dom";
import { useLayoutState } from "../hooks/useLayoutState";
import { useServer } from "../context/ServerContext";
import { useBranding } from "../context/BrandingContext";
import CommandPalette from "./CommandPalette";
import React, { useState, useRef, useEffect } from "react";

/* ── Lucide-style inline SVG icons (indigo accent) ────────────────────── */
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
        className="w-full flex items-center gap-2 px-3 py-2 rounded-lg bg-zinc-900 border border-zinc-800 text-sm text-zinc-300 hover:text-white hover:border-zinc-600 transition-colors"
      >
        <div className={`w-2 h-2 rounded-full shrink-0 ${activeServer?.status === "online" ? "bg-emerald-500" : activeServer?.status === "offline" ? "bg-rose-500" : "bg-zinc-500"}`} />
        <span className="flex-1 text-left truncate">{activeServer?.name || "Select server"}</span>
        <svg className={`w-4 h-4 transition-transform ${open ? "rotate-180" : ""}`} fill="none" viewBox="0 0 24 24" stroke="currentColor" strokeWidth={2}><path strokeLinecap="round" strokeLinejoin="round" d="M19.5 8.25l-7.5 7.5-7.5-7.5" /></svg>
      </button>
      {open && (
        <div className="mt-1 bg-zinc-900 border border-zinc-700 rounded-lg shadow-xl overflow-hidden">
          {servers.map((s) => (
            <button
              key={s.id}
              onClick={() => { setActiveServerId(s.id); setOpen(false); window.location.href = "/"; }}
              className={`w-full flex items-center gap-2 px-3 py-2 text-sm text-left hover:bg-zinc-800 transition-colors ${s.id === activeServer?.id ? "bg-zinc-800 text-white" : "text-zinc-400"}`}
            >
              <div className={`w-2 h-2 rounded-full shrink-0 ${s.status === "online" ? "bg-emerald-500" : s.status === "offline" ? "bg-rose-500" : "bg-zinc-500"}`} />
              <span className="flex-1 truncate">{s.name}</span>
              {s.is_local && <span className="text-[10px] text-zinc-500 uppercase">local</span>}
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

  // Nexus layout forces light theme for consistent appearance
  useEffect(() => {
    document.documentElement.setAttribute("data-theme", "arctic");
  }, []);

  if (state.loading) {
    return (
      <div className="flex items-center justify-center h-screen bg-zinc-50">
        <div className="w-8 h-8 border-4 border-indigo-500 border-t-transparent rounded-full animate-spin" />
      </div>
    );
  }

  if (!state.user.email) return <Navigate to="/login" replace />;

  // Flatten nav groups for Nexus (flat sidebar, no group headers)
  const allItems = state.visibleGroups.flatMap(g => g.items);

  return (
    <div className="flex h-screen bg-zinc-50 font-sans overflow-hidden">
      <CommandPalette />

      {/* Skip to content */}
      <a href="#main-content" className="sr-only focus:not-sr-only focus:absolute focus:z-[100] focus:top-2 focus:left-2 focus:px-4 focus:py-2 focus:bg-indigo-600 focus:text-white focus:rounded-lg">
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
        className={`fixed inset-y-0 left-0 z-50 w-64 bg-zinc-950 text-zinc-300 flex flex-col h-screen border-r border-zinc-800 transform transition-transform duration-200 ease-in-out md:relative md:translate-x-0 ${
          state.sidebarOpen ? "translate-x-0" : "-translate-x-full"
        }`}
      >
        {/* Logo */}
        <div className="h-16 flex items-center justify-between px-6 border-b border-zinc-800">
          <Link to="/" className="flex items-center gap-3 hover:opacity-90 transition-opacity">
            {branding.logoUrl ? (
              <img src={branding.logoUrl} alt={branding.panelName} className="h-8 w-auto max-w-[160px] object-contain" />
            ) : (
              <>
                <div className="p-1.5 bg-indigo-600 rounded-lg">
                  <svg className="w-5 h-5 text-white" fill="none" viewBox="0 0 24 24" stroke="currentColor" strokeWidth={2}>
                    <rect x="2" y="2" width="20" height="8" rx="2" />
                    <rect x="2" y="14" width="20" height="8" rx="2" />
                    <circle cx="6" cy="6" r="1" fill="currentColor" />
                    <circle cx="6" cy="18" r="1" fill="currentColor" />
                  </svg>
                </div>
                {!branding.hideBranding && (
                  <span className="text-lg font-semibold text-white tracking-tight">
                    {branding.panelName}
                  </span>
                )}
              </>
            )}
          </Link>
          <button
            onClick={() => state.setSidebarOpen(false)}
            className="p-1.5 text-zinc-400 hover:text-white md:hidden rounded-lg"
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
                    ? "bg-indigo-500/10 text-indigo-400"
                    : "text-zinc-400 hover:bg-zinc-900 hover:text-white"
                }`
              }
            >
              {({ isActive }) => (
                <>
                  <span className={isActive ? "text-indigo-400" : "text-zinc-500"}>{getIcon(item.iconName)}</span>
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
        <div className="p-4 border-t border-zinc-800 space-y-3">
          {/* Health */}
          <div className="flex items-center gap-2 px-3">
            <div className={`w-2 h-2 rounded-full ${state.apiHealthy === true ? "bg-emerald-500" : state.apiHealthy === false ? "bg-rose-500" : "bg-zinc-500"}`} />
            <span className="text-xs text-zinc-500">{state.apiHealthy === true ? "All systems OK" : state.apiHealthy === false ? "Issues detected" : "Checking..."}</span>
          </div>
          {/* User + layout + logout */}
          <div className="flex items-center gap-2 px-3">
            <div className="w-7 h-7 bg-indigo-600 rounded-full flex items-center justify-center text-white text-xs font-semibold shrink-0">
              {state.user.email[0]?.toUpperCase()}
            </div>
            <span className="flex-1 text-sm text-zinc-300 truncate">{state.user.email}</span>
            <button
              onClick={state.logout}
              className="p-1 text-zinc-500 hover:text-rose-400 transition-colors"
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
        <header className="h-16 bg-white border-b border-zinc-200 flex items-center justify-between px-6 shrink-0">
          {/* Mobile hamburger */}
          <button
            className="p-2 text-zinc-500 hover:text-zinc-900 md:hidden rounded-lg"
            onClick={() => state.setSidebarOpen(true)}
            aria-label="Open menu"
          >
            <svg className="w-5 h-5" fill="none" viewBox="0 0 24 24" stroke="currentColor" strokeWidth={2}><path strokeLinecap="round" strokeLinejoin="round" d="M3.75 6.75h16.5M3.75 12h16.5m-16.5 5.25h16.5" /></svg>
          </button>

          {/* Search */}
          <div className="flex-1 flex items-center max-w-md">
            <button
              onClick={() => window.dispatchEvent(new KeyboardEvent("keydown", { key: "k", ctrlKey: true }))}
              className="w-full flex items-center gap-2 pl-3 pr-4 py-2 bg-zinc-50 border border-zinc-200 rounded-lg text-sm text-zinc-400 hover:border-indigo-300 hover:bg-white transition-all"
            >
              <svg className="w-4 h-4" fill="none" viewBox="0 0 24 24" stroke="currentColor" strokeWidth={2}><circle cx="11" cy="11" r="8" /><line x1="21" y1="21" x2="16.65" y2="16.65" /></svg>
              <span className="flex-1 text-left">Search...</span>
              <kbd className="text-[10px] px-1.5 py-0.5 border border-zinc-300 rounded bg-white text-zinc-400">Ctrl K</kbd>
            </button>
          </div>

          {/* Right side */}
          <div className="flex items-center gap-3">
            {/* Alert badge */}
            {state.firingCount > 0 && (
              <Link to="/monitoring" className="relative p-2 text-zinc-500 hover:bg-zinc-100 rounded-full transition-colors">
                <svg className="w-5 h-5" fill="none" viewBox="0 0 24 24" stroke="currentColor" strokeWidth={1.5}><path d="M14.857 17.082a23.848 23.848 0 005.454-1.31A8.967 8.967 0 0118 9.75v-.7V9A6 6 0 006 9v.75a8.967 8.967 0 01-2.312 6.022c1.733.64 3.56 1.085 5.455 1.31m5.714 0a24.255 24.255 0 01-5.714 0m5.714 0a3 3 0 11-5.714 0" /></svg>
                <span className="absolute top-1 right-1 w-2.5 h-2.5 bg-rose-500 rounded-full border-2 border-white" />
              </Link>
            )}

            <div className="h-6 w-px bg-zinc-200 hidden sm:block" />

            {/* Layout switcher */}
            <select
              value={state.layout}
              onChange={(e) => {
                localStorage.setItem("dp-layout", e.target.value);
                window.dispatchEvent(new Event("dp-layout-change"));
              }}
              className="text-xs border border-zinc-200 rounded-md px-2 py-1 bg-white text-zinc-600 hidden sm:block"
            >
              <option value="command">Terminal</option>
              <option value="glass">Glass</option>
              <option value="atlas">Atlas</option>
              <option value="nexus">Nexus</option>
            </select>

            {/* User info (desktop) */}
            <div className="hidden sm:flex items-center gap-2">
              <div className="w-8 h-8 bg-indigo-100 text-indigo-600 rounded-full flex items-center justify-center font-semibold text-sm">
                {state.user.email[0]?.toUpperCase()}{state.user.email[1]?.toUpperCase()}
              </div>
              <div className="text-left">
                <p className="text-sm font-medium text-zinc-900 leading-none">{state.user.email.split("@")[0]}</p>
                <p className="text-xs text-zinc-500 mt-0.5">{state.user.role}</p>
              </div>
            </div>
          </div>
        </header>

        {/* 2FA enforcement banner */}
        {state.twoFaEnforced && !state.twoFaEnabled && (
          <div className="bg-amber-50 border-b border-amber-200 px-6 py-2 text-sm text-amber-800 flex items-center gap-2">
            <svg className="w-4 h-4 text-amber-500 shrink-0" fill="none" viewBox="0 0 24 24" stroke="currentColor" strokeWidth={2}><path strokeLinecap="round" strokeLinejoin="round" d="M12 9v3.75m-9.303 3.376c-.866 1.5.217 3.374 1.948 3.374h14.71c1.73 0 2.813-1.874 1.948-3.374L13.949 3.378c-.866-1.5-3.032-1.5-3.898 0L2.697 16.126z" /><path strokeLinecap="round" strokeLinejoin="round" d="M12 15.75h.007v.008H12v-.008z" /></svg>
            <span>Two-factor authentication is required.</span>
            <Link to="/settings" className="font-medium text-amber-900 underline hover:no-underline ml-1">Set up 2FA</Link>
          </div>
        )}

        {/* Content */}
        <main id="main-content" className="flex-1 overflow-y-auto bg-zinc-50">
          <Outlet />
        </main>
      </div>
    </div>
  );
}
