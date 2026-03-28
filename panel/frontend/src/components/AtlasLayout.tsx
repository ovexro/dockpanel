import { useMemo, useState } from "react";
import { Navigate, Outlet, NavLink, Link, useLocation } from "react-router-dom";
import { useLayoutState } from "../hooks/useLayoutState";
import { useServer } from "../context/ServerContext";
import { Icon } from "../data/icons";
import CommandPalette from "./CommandPalette";
import LayoutSwitcher from "./LayoutSwitcher";

export default function AtlasLayout() {
  const {
    user,
    logout,
    loading,
    theme,
    cycleTheme,
    firingCount,
    incidentCount,
    notifCount,
    apiHealthy,
    twoFaEnforced,
    twoFaEnabled,
    sidebarOpen,
    setSidebarOpen,
    visibleGroups,
  } = useLayoutState();
  const isLight = theme === "clean" || theme === "arctic";
  const { servers, activeServer, setActiveServerId } = useServer();

  const location = useLocation();

  // Build breadcrumbs from current path
  const crumbs = useMemo(() => {
    const parts = location.pathname.split("/").filter(Boolean);
    const result: { label: string; to: string }[] = [{ label: "Home", to: "/" }];

    const labels: Record<string, string> = {
      sites: "Sites",
      databases: "Databases",
      apps: "Docker Apps",
      "git-deploys": "Git Deploy",
      dns: "DNS",
      mail: "Mail",
      monitoring: "Monitoring",
      logs: "Logs",
      terminal: "Terminal",
      security: "Security",
      settings: "Settings",
      files: "Files",
      backups: "Backups",
      crons: "Crons",
      deploy: "Deploy",
      wordpress: "WordPress",
    };

    let path = "";
    for (const part of parts) {
      path += "/" + part;
      const label = labels[part] || part;
      result.push({ label, to: path });
    }
    return result;
  }, [location.pathname]);

  if (loading) {
    return (
      <div className="flex items-center justify-center h-screen bg-dark-900">
        <div className="w-8 h-8 border-4 border-rust-500 border-t-transparent rounded-full animate-spin" />
      </div>
    );
  }

  if (!user.email) return <Navigate to="/login" replace />;

  return (
    <div className="flex flex-col h-screen bg-dark-900">
      <CommandPalette />

      {/* Skip to content */}
      <a
        href="#main-content"
        className="sr-only focus:not-sr-only focus:absolute focus:z-[100] focus:top-2 focus:left-2 focus:px-4 focus:py-2 focus:bg-accent-600 focus:text-dark-50 focus:rounded-lg"
      >
        Skip to main content
      </a>

      {/* ── Top Navbar ─────────────────────────────────────────────────── */}
      <header className="sticky top-0 z-40 bg-dark-950 border-b border-dark-600 h-14">
        <div className="flex items-center h-full px-4">
          {/* Mobile hamburger */}
          <button
            className="md:hidden p-2 text-dark-200 hover:text-dark-50 mr-2"
            onClick={() => setSidebarOpen(true)}
            aria-label="Open navigation"
          >
            <Icon name="hamburger" className="w-5 h-5" />
          </button>

          {/* Logo */}
          <Link to="/" className="flex items-center gap-2 shrink-0 mr-6">
            <div className="w-8 h-8 bg-rust-500 rounded-md flex items-center justify-center">
              <svg
                className="w-5 h-5 text-dark-950"
                viewBox="0 0 24 24"
                fill="none"
                stroke="currentColor"
                strokeWidth="2.5"
                aria-hidden="true"
              >
                <path d="M5 16h4" strokeLinecap="square" />
                <path d="M5 12h8" strokeLinecap="square" />
                <path d="M5 8h6" strokeLinecap="square" />
                <rect x="16" y="7" width="4" height="4" fill="currentColor" stroke="none" />
                <rect x="16" y="13" width="4" height="4" fill="currentColor" stroke="none" />
              </svg>
            </div>
            <span className="text-sm font-bold">
              <span className="text-rust-400">Dock</span>
              <span className="text-dark-50">Panel</span>
            </span>
          </Link>

          {/* Desktop nav tabs */}
          <nav className="hidden md:flex items-center gap-1 flex-1 overflow-x-auto scrollbar-none">
            {visibleGroups.map((group, gi) => (
              <div key={group.key} className="contents">
                {/* Group separator */}
                {gi > 0 && (
                  <div className="w-px h-5 bg-dark-600 mx-1.5 shrink-0" />
                )}
                {group.items.map((item) => (
                  <NavLink
                    key={item.to}
                    to={item.to}
                    end
                    className={({ isActive }) =>
                      `flex items-center gap-1.5 px-3 py-1.5 text-sm rounded-md whitespace-nowrap shrink-0 transition-colors ${
                        isActive
                          ? "bg-rust-500/10 text-rust-400 font-medium"
                          : "text-dark-300 hover:text-dark-100 hover:bg-dark-700/30"
                      }`
                    }
                  >
                    <Icon name={item.iconName} className="w-4 h-4" />
                    <span>{item.label}</span>
                    {item.to === "/monitoring" && firingCount > 0 && (
                      <span className="ml-1 px-1.5 py-0.5 text-[10px] font-bold bg-danger-500 text-white rounded-full">
                        {firingCount}
                      </span>
                    )}
                    {item.to === "/incidents" && incidentCount > 0 && (
                      <span className="ml-1 px-1.5 py-0.5 text-[10px] font-bold bg-warn-500 text-dark-900 rounded-full">
                        {incidentCount}
                      </span>
                    )}
                  </NavLink>
                ))}
              </div>
            ))}
          </nav>

          {/* Right actions */}
          <div className="flex items-center gap-2 shrink-0 ml-auto md:ml-4">
            {/* Server selector */}
            {servers.length > 1 && (
              <select
                value={activeServer?.id || ""}
                onChange={e => { setActiveServerId(e.target.value); window.location.href = "/"; }}
                className="hidden md:block px-2 py-1 rounded-md bg-dark-800 border border-dark-600 text-xs text-dark-200 outline-none"
              >
                {servers.map(s => <option key={s.id} value={s.id}>{s.name}</option>)}
              </select>
            )}
            {/* Search */}
            <button
              onClick={() =>
                window.dispatchEvent(
                  new KeyboardEvent("keydown", { key: "k", ctrlKey: true })
                )
              }
              className="p-1.5 text-dark-300 hover:text-dark-100 rounded-md hover:bg-dark-700/30"
              title="Search (Ctrl+K)"
              aria-label="Search"
            >
              <Icon name="search" className="w-4 h-4" />
            </button>

            {/* Alert badge */}
            {firingCount > 0 && (
              <Link
                to="/monitoring"
                className="px-2 py-1 text-xs font-bold bg-danger-500/15 text-danger-400 rounded-md"
              >
                {firingCount} alert{firingCount > 1 ? "s" : ""}
              </Link>
            )}

            {/* Health dot */}
            <div
              className={`w-2 h-2 rounded-full ${
                apiHealthy === null
                  ? "bg-dark-400"
                  : apiHealthy
                  ? "bg-rust-500"
                  : "bg-danger-500 animate-pulse"
              }`}
              title={
                apiHealthy === null
                  ? "Checking..."
                  : apiHealthy
                  ? "Healthy"
                  : "Health Issue"
              }
            />

            {/* Notification bell */}
            <Link to="/notifications" className="relative p-1.5 text-dark-300 hover:text-dark-100 rounded-md hover:bg-dark-700/30" title="Notifications" aria-label="Notifications">
              <svg className="w-4 h-4" fill="none" viewBox="0 0 24 24" stroke="currentColor" strokeWidth={1.5}>
                <path strokeLinecap="round" strokeLinejoin="round" d="M14.857 17.082a23.848 23.848 0 005.454-1.31A8.967 8.967 0 0118 9.75v-.7V9A6 6 0 006 9v.75a8.967 8.967 0 01-2.312 6.022c1.733.64 3.56 1.085 5.455 1.31m5.714 0a24.255 24.255 0 01-5.714 0m5.714 0a3 3 0 11-5.714 0" />
              </svg>
              {notifCount > 0 && (
                <span className="absolute -top-0.5 -right-0.5 w-4 h-4 bg-danger-500 text-white text-[10px] font-bold rounded-full flex items-center justify-center">
                  {notifCount > 9 ? "9+" : notifCount}
                </span>
              )}
            </Link>

            {/* Layout switcher */}
            <LayoutSwitcher variant={isLight ? "light" : "dark"} />

            {/* Theme cycle */}
            <button
              onClick={cycleTheme}
              className="p-1.5 text-dark-300 hover:text-dark-100 rounded-md hover:bg-dark-700/30"
              title={`Theme: ${theme}`}
              aria-label="Cycle theme"
            >
              <svg
                className="w-4 h-4"
                fill="none"
                viewBox="0 0 24 24"
                stroke="currentColor"
                strokeWidth={1.5}
                aria-hidden="true"
              >
                <path
                  strokeLinecap="round"
                  strokeLinejoin="round"
                  d="M4.098 19.902a3.75 3.75 0 0 0 5.304 0l6.401-6.402M6.75 21A3.75 3.75 0 0 1 3 17.25V4.125C3 3.504 3.504 3 4.125 3h5.25c.621 0 1.125.504 1.125 1.125v4.072M6.75 21a3.75 3.75 0 0 0 3.75-3.75V8.197M6.75 21h13.125c.621 0 1.125-.504 1.125-1.125v-5.25c0-.621-.504-1.125-1.125-1.125h-4.072M10.5 8.197l2.88-2.88c.438-.439 1.15-.439 1.59 0l3.712 3.713c.44.44.44 1.152 0 1.59l-2.88 2.88M6.75 17.25h.008v.008H6.75v-.008Z"
                />
              </svg>
            </button>

            {/* User */}
            <div className="flex items-center gap-2 pl-2 border-l border-dark-600">
              <span className="text-xs text-dark-300 hidden lg:block truncate max-w-[160px]">
                {user.email}
              </span>
              <button
                onClick={logout}
                className="p-1.5 text-dark-300 hover:text-dark-100 rounded-md hover:bg-dark-700/30"
                title="Logout"
                aria-label="Logout"
              >
                <Icon name="logout" className="w-4 h-4" />
              </button>
            </div>
          </div>
        </div>
      </header>

      {/* ── Breadcrumb Bar (desktop only) ──────────────────────────────── */}
      <div className="hidden md:block sticky top-14 z-30 bg-dark-900 border-b border-dark-700 px-4 py-2">
        <nav className="flex items-center gap-2 text-sm" aria-label="Breadcrumb">
          {crumbs.map((crumb, i) => {
            const isLast = i === crumbs.length - 1;
            return (
              <span key={crumb.to} className="flex items-center gap-2">
                {i > 0 && (
                  <svg
                    className="w-3 h-3 text-dark-500"
                    fill="none"
                    viewBox="0 0 24 24"
                    stroke="currentColor"
                    strokeWidth={2}
                    aria-hidden="true"
                  >
                    <path
                      strokeLinecap="round"
                      strokeLinejoin="round"
                      d="M8.25 4.5l7.5 7.5-7.5 7.5"
                    />
                  </svg>
                )}
                {isLast ? (
                  <span className="text-dark-100 font-medium">{crumb.label}</span>
                ) : (
                  <Link
                    to={crumb.to}
                    className="text-dark-400 hover:text-dark-200 transition-colors"
                  >
                    {crumb.label}
                  </Link>
                )}
              </span>
            );
          })}
        </nav>
      </div>

      {/* ── 2FA Warning ────────────────────────────────────────────────── */}
      {twoFaEnforced && !twoFaEnabled && (
        <div className={`border-b px-4 py-3 flex items-center justify-between ${isLight ? "bg-warn-500/5 border-warn-500/20" : "bg-warn-500/10 border-warn-500/20"}`}>
          <div className="flex items-center gap-2">
            <svg
              className="w-5 h-5 text-warn-400"
              fill="none"
              viewBox="0 0 24 24"
              stroke="currentColor"
              strokeWidth={1.5}
              aria-hidden="true"
            >
              <path
                strokeLinecap="round"
                strokeLinejoin="round"
                d="M12 9v3.75m-9.303 3.376c-.866 1.5.217 3.374 1.948 3.374h14.71c1.73 0 2.813-1.874 1.948-3.374L13.949 3.378c-.866-1.5-3.032-1.5-3.898 0L2.697 16.126ZM12 15.75h.007v.008H12v-.008Z"
              />
            </svg>
            <span className="text-sm text-warn-400 font-medium">
              Two-factor authentication is required. Please enable 2FA in Settings &rarr; Security.
            </span>
          </div>
          <a
            href="/settings"
            className="px-3 py-1.5 bg-warn-500 text-dark-900 rounded text-xs font-bold hover:bg-warn-400 transition-colors"
          >
            Set Up 2FA
          </a>
        </div>
      )}

      {/* ── Main Content ───────────────────────────────────────────────── */}
      <main id="main-content" className="flex-1 overflow-auto">
        <Outlet />
      </main>

      {/* ── Mobile Drawer ──────────────────────────────────────────────── */}
      {sidebarOpen && (
        <div className="fixed inset-0 z-50 md:hidden">
          {/* Backdrop */}
          <div
            className="absolute inset-0 bg-black/50"
            role="presentation"
            onClick={() => setSidebarOpen(false)}
          />

          {/* Drawer panel */}
          <div className="absolute inset-y-0 left-0 w-72 bg-dark-950 border-r border-dark-600 flex flex-col overflow-y-auto">
            {/* Drawer header */}
            <div className="flex items-center justify-between px-4 py-4 border-b border-dark-600">
              <Link
                to="/"
                onClick={() => setSidebarOpen(false)}
                className="flex items-center gap-2"
              >
                <div className="w-8 h-8 bg-rust-500 rounded-md flex items-center justify-center">
                  <svg
                    className="w-5 h-5 text-dark-950"
                    viewBox="0 0 24 24"
                    fill="none"
                    stroke="currentColor"
                    strokeWidth="2.5"
                    aria-hidden="true"
                  >
                    <path d="M5 16h4" strokeLinecap="square" />
                    <path d="M5 12h8" strokeLinecap="square" />
                    <path d="M5 8h6" strokeLinecap="square" />
                    <rect x="16" y="7" width="4" height="4" fill="currentColor" stroke="none" />
                    <rect x="16" y="13" width="4" height="4" fill="currentColor" stroke="none" />
                  </svg>
                </div>
                <span className="text-sm font-bold">
                  <span className="text-rust-400">Dock</span>
                  <span className="text-dark-50">Panel</span>
                </span>
              </Link>
              <button
                onClick={() => setSidebarOpen(false)}
                className="p-1.5 text-dark-200 hover:text-dark-50 rounded-lg"
                aria-label="Close navigation"
              >
                <Icon name="close" className="w-5 h-5" />
              </button>
            </div>

            {/* Nav items */}
            <nav className="flex-1 px-3 py-4">
              {visibleGroups.map((group, gi) => (
                <div key={group.key} className={gi > 0 ? "mt-4 pt-4 border-t border-dark-700" : ""}>
                  <p className="px-3 pb-2 text-[10px] font-bold uppercase tracking-widest text-dark-400">
                    {group.label}
                  </p>
                  <div className="space-y-0.5">
                    {group.items.map((item) => (
                      <NavLink
                        key={item.to}
                        to={item.to}
                        end
                        onClick={() => setSidebarOpen(false)}
                        className={({ isActive }) =>
                          `flex items-center gap-3 px-3 py-2.5 text-sm rounded-md transition-colors ${
                            isActive
                              ? "bg-rust-500/10 text-rust-400 font-medium"
                              : "text-dark-300 hover:text-dark-100 hover:bg-dark-700/30"
                          }`
                        }
                      >
                        <Icon name={item.iconName} className="w-5 h-5" />
                        <span>{item.label}</span>
                        {item.to === "/monitoring" && firingCount > 0 && (
                          <span className="ml-auto px-1.5 py-0.5 text-[10px] font-bold bg-danger-500 text-white rounded-full">
                            {firingCount}
                          </span>
                        )}
                        {item.to === "/incidents" && incidentCount > 0 && (
                          <span className="ml-auto px-1.5 py-0.5 text-[10px] font-bold bg-warn-500 text-dark-900 rounded-full">
                            {incidentCount}
                          </span>
                        )}
                      </NavLink>
                    ))}
                  </div>
                </div>
              ))}
            </nav>

            {/* User section at bottom */}
            <div className="px-4 py-4 border-t border-dark-600">
              <div className="flex items-center justify-between">
                <div className="min-w-0">
                  <p className="text-sm font-medium text-dark-100 truncate">{user.email}</p>
                  <p className="text-xs text-dark-400 capitalize">{user.role}</p>
                </div>
                <button
                  onClick={logout}
                  className="p-2 text-dark-300 hover:text-dark-100 hover:bg-dark-700/30 rounded-md transition-colors"
                  title="Logout"
                  aria-label="Logout"
                >
                  <Icon name="logout" className="w-5 h-5" />
                </button>
              </div>
            </div>
          </div>
        </div>
      )}
    </div>
  );
}
