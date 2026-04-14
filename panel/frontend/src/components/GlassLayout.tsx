import { useState, useEffect } from "react";
import { Navigate, Outlet, NavLink, Link } from "react-router-dom";
import { useLayoutState } from "../hooks/useLayoutState";
import { useServer } from "../context/ServerContext";
import { Icon } from "../data/icons";
import CommandPalette from "./CommandPalette";
import LayoutSwitcher from "./LayoutSwitcher";

export default function GlassLayout() {
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

  const [hovered, setHovered] = useState(false);
  const [flatNav, setFlatNav] = useState(() => localStorage.getItem("dp-flat-nav") === "true");

  useEffect(() => {
    const handler = () => setFlatNav(localStorage.getItem("dp-flat-nav") === "true");
    window.addEventListener("dp-layout-options-change", handler);
    return () => window.removeEventListener("dp-layout-options-change", handler);
  }, []);

  /* ── Auth guard ──────────────────────────────────────────────────── */
  if (loading) {
    return (
      <div className="flex items-center justify-center h-screen bg-dark-900">
        <div className="w-8 h-8 border-4 border-rust-500 border-t-transparent rounded-full animate-spin" />
      </div>
    );
  }

  if (!user) return <Navigate to="/login" replace />;

  /* ── Expanded state: hovered on desktop, always on mobile when open ── */
  const expanded = hovered;

  return (
    <div className="flex h-screen bg-dark-900">
      <CommandPalette />

      {/* Skip to content */}
      <a
        href="#main-content"
        className="sr-only focus:not-sr-only focus:absolute focus:z-[100] focus:top-2 focus:left-2 focus:px-4 focus:py-2 focus:bg-accent-600 focus:text-dark-50 focus:rounded-lg"
      >
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

      {/* ── Sidebar ────────────────────────────────────────────────── */}
      <aside
        onMouseEnter={() => setHovered(true)}
        onMouseLeave={() => setHovered(false)}
        className={[
          "fixed inset-y-0 left-0 z-50 flex flex-col shrink-0",
          "bg-dark-950/80 backdrop-blur-xl border-r border-dark-600/30 text-dark-50",
          "transition-all duration-200 ease-in-out overflow-hidden",
          /* Desktop: narrow or wide */
          expanded ? "md:w-56" : "md:w-16",
          /* Mobile: slide in/out at full width */
          sidebarOpen ? "w-56 translate-x-0" : "-translate-x-full",
          "md:relative md:translate-x-0",
        ].join(" ")}
      >
        {/* ── Logo ──────────────────────────────────────────────── */}
        <div className="px-3 py-4 flex items-center gap-3 border-b border-dark-600/30">
          <Link
            to="/"
            className="flex items-center gap-3 min-w-0 hover:opacity-90 transition-opacity"
          >
            <div className="w-8 h-8 bg-rust-500 rounded-lg flex items-center justify-center shrink-0">
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
            <span className="text-base font-semibold whitespace-nowrap overflow-hidden">
              <span className="text-rust-400">Dock</span>
              <span className="text-dark-50">Panel</span>
            </span>
          </Link>

          {/* Close button (mobile only) */}
          <button
            onClick={() => setSidebarOpen(false)}
            className="ml-auto p-1.5 text-dark-200 hover:text-dark-50 md:hidden rounded-lg shrink-0"
            aria-label="Close sidebar"
          >
            <Icon name="close" className="w-5 h-5" />
          </button>
        </div>

        {/* ── Search shortcut (visible when expanded) ───────────── */}
        <div
          className={[
            "px-3 pt-3 pb-1 transition-opacity duration-200",
            expanded || sidebarOpen ? "opacity-100" : "opacity-0 h-0 overflow-hidden md:h-0 md:py-0",
          ].join(" ")}
        >
          <button
            onClick={() =>
              window.dispatchEvent(
                new KeyboardEvent("keydown", { key: "k", ctrlKey: true })
              )
            }
            className="w-full flex items-center gap-2 px-3 py-2 rounded-lg bg-dark-800/30 border border-dark-600/30 text-sm text-dark-300 hover:text-dark-100 hover:border-dark-400/50 transition-colors outline-none focus:outline-none focus:border-dark-400/50"
          >
            <Icon name="search" className="w-4 h-4 shrink-0" />
            <span className="flex-1 text-left whitespace-nowrap overflow-hidden">Search...</span>
            <kbd className="text-[10px] px-1.5 py-0.5 border border-dark-500/50 rounded bg-dark-700/50 shrink-0">
              Ctrl K
            </kbd>
          </button>
        </div>

        {/* Server selector (expanded only) */}
        {servers.length > 1 && (expanded || sidebarOpen) && (
          <div className="px-3 pt-2">
            <select
              value={activeServer?.id || ""}
              onChange={e => { setActiveServerId(e.target.value); window.location.href = "/"; }}
              className="w-full px-3 py-2 rounded-lg bg-dark-800/50 border border-dark-600/30 text-sm text-dark-200 outline-none"
            >
              {servers.map(s => <option key={s.id} value={s.id}>{s.name}</option>)}
            </select>
          </div>
        )}

        {/* ── Nav ────────────────────────────────────────────────── */}
        <nav className="flex-1 px-2 pt-4 overflow-y-auto overflow-x-hidden sidebar-scroll-fade">
          {flatNav ? (
            /* Flat nav — no group labels */
            <div className="space-y-0.5">
              {visibleGroups.flatMap(g => g.items).map((item) => (
                <NavLink
                  key={item.to}
                  to={item.to}
                  end
                  onClick={() => setSidebarOpen(false)}
                >
                  {({ isActive }) => (
                    <div
                      title={!expanded && !sidebarOpen ? item.label : undefined}
                      className={[
                        `flex items-center py-2.5 rounded-lg transition-all ${expanded || sidebarOpen ? "gap-3 px-3" : "justify-center px-0"}`,
                        isActive
                          ? "bg-rust-500/10 text-rust-400 border-l-2 border-rust-500 ml-0.5"
                          : "text-dark-300 hover:text-dark-100 hover:bg-dark-700/30",
                      ].join(" ")}
                    >
                      <div className={`shrink-0 flex items-center justify-center transition-all duration-200 ${expanded || sidebarOpen ? "w-5 h-5" : "w-7 h-7"}`}>
                        <Icon name={item.iconName} className={`transition-all duration-200 ${expanded || sidebarOpen ? "w-[19px] h-[19px]" : "w-6 h-6"}`} />
                      </div>
                      {(expanded || sidebarOpen) && (
                        <span className="text-sm whitespace-nowrap overflow-hidden">
                          {item.label}
                        </span>
                      )}
                      {item.to === "/monitoring" && firingCount > 0 && (
                        <span className="ml-auto px-1.5 py-0.5 text-xs font-bold bg-danger-500 text-white rounded-full min-w-[20px] text-center shrink-0">
                          {firingCount}
                        </span>
                      )}
                      {item.to === "/incidents" && incidentCount > 0 && (
                        <span className="ml-auto px-1.5 py-0.5 text-xs font-bold bg-warn-500 text-dark-900 rounded-full min-w-[20px] text-center shrink-0">
                          {incidentCount}
                        </span>
                      )}
                    </div>
                  )}
                </NavLink>
              ))}
            </div>
          ) : (
            /* Grouped nav */
            visibleGroups.map((group, gi) => (
              <div key={group.key} className={gi > 0 ? "mt-5" : ""}>
                <div
                  className={[
                    "px-3 pb-1.5 text-[10px] text-dark-400 uppercase tracking-widest whitespace-nowrap overflow-hidden transition-opacity duration-200",
                    expanded || sidebarOpen ? "opacity-100" : "opacity-0",
                  ].join(" ")}
                >
                  {group.label}
                </div>

                <div className="space-y-0.5">
                  {group.items.map((item) => (
                    <NavLink
                      key={item.to}
                      to={item.to}
                      end
                      onClick={() => setSidebarOpen(false)}
                    >
                      {({ isActive }) => (
                        <div
                          title={!expanded && !sidebarOpen ? item.label : undefined}
                          className={[
                            `flex items-center py-2.5 rounded-lg transition-all ${expanded || sidebarOpen ? "gap-3 px-3" : "justify-center px-0"}`,
                            isActive
                              ? "bg-rust-500/10 text-rust-400 border-l-2 border-rust-500 ml-0.5"
                              : "text-dark-300 hover:text-dark-100 hover:bg-dark-700/30",
                          ].join(" ")}
                        >
                          <div className={`shrink-0 flex items-center justify-center transition-all duration-200 ${expanded || sidebarOpen ? "w-5 h-5" : "w-7 h-7"}`}>
                            <Icon name={item.iconName} className={`transition-all duration-200 ${expanded || sidebarOpen ? "w-[19px] h-[19px]" : "w-6 h-6"}`} />
                          </div>
                          {(expanded || sidebarOpen) && (
                            <span className="text-sm whitespace-nowrap overflow-hidden">
                              {item.label}
                            </span>
                          )}
                          {item.to === "/monitoring" && firingCount > 0 && (
                            <span className="ml-auto px-1.5 py-0.5 text-xs font-bold bg-danger-500 text-white rounded-full min-w-[20px] text-center shrink-0">
                              {firingCount}
                            </span>
                          )}
                          {item.to === "/incidents" && incidentCount > 0 && (
                            <span className="ml-auto px-1.5 py-0.5 text-xs font-bold bg-warn-500 text-dark-900 rounded-full min-w-[20px] text-center shrink-0">
                              {incidentCount}
                            </span>
                          )}
                        </div>
                      )}
                    </NavLink>
                  ))}
                </div>
              </div>
            ))
          )}
        </nav>

        {/* ── Footer ─────────────────────────────────────────────── */}
        <div className="px-2 py-3 border-t border-dark-600/30">
          {/* Collapsed: just the health dot centered */}
          {!expanded && !sidebarOpen && (
            <div className="flex flex-col items-center gap-3">
              <div
                className={[
                  "w-2.5 h-2.5 rounded-full shrink-0",
                  apiHealthy === null
                    ? "bg-dark-400"
                    : apiHealthy
                      ? "bg-rust-500"
                      : "bg-danger-500 animate-pulse",
                ].join(" ")}
                title={
                  apiHealthy === null
                    ? "Checking..."
                    : apiHealthy
                      ? "All systems OK"
                      : "Health issue"
                }
              />
            </div>
          )}

          {/* Expanded: full footer */}
          {(expanded || sidebarOpen) && (
            <div>
              {/* User row */}
              <div className="flex items-center gap-3 px-3 py-2 rounded-lg hover:bg-dark-800/50 transition-colors group">
                <div className="w-8 h-8 rounded-full bg-dark-700 flex items-center justify-center shrink-0">
                  <span className="text-xs font-bold text-dark-200 uppercase">{user.email?.charAt(0) || "?"}</span>
                </div>
                <div className="flex-1 min-w-0">
                  <p className="text-sm font-medium truncate">{user.email}</p>
                  <p className="text-[11px] text-dark-400 capitalize">{user.role}</p>
                </div>
                <button
                  onClick={logout}
                  className="p-1.5 text-dark-400 hover:text-dark-100 rounded-lg transition-colors opacity-0 group-hover:opacity-100 shrink-0"
                  title="Logout"
                  aria-label="Logout"
                >
                  <Icon name="logout" className="w-4 h-4" />
                </button>
              </div>

              {/* Health + layout + theme */}
              <div className="flex items-center justify-between px-3 mt-2">
                <div className="flex items-center gap-2">
                  <div
                    className={[
                      "w-1.5 h-1.5 rounded-full shrink-0",
                      apiHealthy === null
                        ? "bg-dark-400"
                        : apiHealthy
                          ? "bg-rust-500"
                          : "bg-danger-500 animate-pulse",
                    ].join(" ")}
                  />
                  <span className="text-[10px] text-dark-400">
                    {apiHealthy === null ? "Checking..." : apiHealthy ? "Connected" : "Disconnected"}
                  </span>
                </div>
                <div className="flex items-center gap-1">
                <Link to="/notifications" className="relative p-1.5 text-dark-400 hover:text-dark-200 transition-colors rounded shrink-0" title="Notifications" aria-label="Notifications">
                  <svg className="w-3.5 h-3.5" fill="none" viewBox="0 0 24 24" stroke="currentColor" strokeWidth={1.5}>
                    <path strokeLinecap="round" strokeLinejoin="round" d="M14.857 17.082a23.848 23.848 0 005.454-1.31A8.967 8.967 0 0118 9.75v-.7V9A6 6 0 006 9v.75a8.967 8.967 0 01-2.312 6.022c1.733.64 3.56 1.085 5.455 1.31m5.714 0a24.255 24.255 0 01-5.714 0m5.714 0a3 3 0 11-5.714 0" />
                  </svg>
                  {notifCount > 0 && (
                    <span className="absolute -top-0.5 -right-0.5 w-4 h-4 bg-danger-500 text-white text-[10px] font-bold rounded-full flex items-center justify-center">
                      {notifCount > 9 ? "9+" : notifCount}
                    </span>
                  )}
                </Link>
                <LayoutSwitcher variant={isLight ? "light" : "dark"} compact />
                <button
                  onClick={cycleTheme}
                  className="p-1.5 text-dark-400 hover:text-dark-200 transition-colors rounded shrink-0"
                  title={`Theme: ${theme}`}
                  aria-label="Cycle theme"
                >
                  <svg
                    className="w-3.5 h-3.5"
                    fill="none"
                    viewBox="0 0 24 24"
                    stroke="currentColor"
                    strokeWidth={1.5}
                  >
                    <path
                      strokeLinecap="round"
                      strokeLinejoin="round"
                      d="M4.098 19.902a3.75 3.75 0 0 0 5.304 0l6.401-6.402M6.75 21A3.75 3.75 0 0 1 3 17.25V4.125C3 3.504 3.504 3 4.125 3h5.25c.621 0 1.125.504 1.125 1.125v4.072M6.75 21a3.75 3.75 0 0 0 3.75-3.75V8.197M6.75 21h13.125c.621 0 1.125-.504 1.125-1.125v-5.25c0-.621-.504-1.125-1.125-1.125h-4.072M10.5 8.197l2.88-2.88c.438-.439 1.15-.439 1.59 0l3.712 3.713c.44.44.44 1.152 0 1.59l-2.88 2.88M6.75 17.25h.008v.008H6.75v-.008Z"
                    />
                  </svg>
                </button>
                </div>
              </div>
            </div>
          )}
        </div>
      </aside>

      {/* ── Main content ───────────────────────────────────────────── */}
      <main id="main-content" className="flex-1 overflow-auto">
        {/* Mobile header with hamburger */}
        <div className="sticky top-0 z-30 flex items-center gap-3 px-4 py-3 bg-dark-900/80 backdrop-blur-lg border-b border-dark-600/30 md:hidden">
          <button
            onClick={() => setSidebarOpen(true)}
            className="p-2 text-dark-300 hover:text-dark-50 hover:bg-dark-700/30 rounded-lg"
            aria-label="Open sidebar"
          >
            <Icon name="hamburger" className="w-6 h-6" />
          </button>
          <span className="text-base font-semibold">
            <span className="text-rust-400">Dock</span>
            <span className="text-dark-50">Panel</span>
          </span>
        </div>

        {/* 2FA enforcement warning */}
        {twoFaEnforced && !twoFaEnabled && (
          <div className={`border-b px-4 py-3 flex items-center justify-between ${isLight ? "bg-warn-500/5 border-warn-500/20" : "bg-warn-500/10 border-warn-500/20"}`}>
            <div className="flex items-center gap-2">
              <svg
                className="w-5 h-5 text-warn-400"
                fill="none"
                viewBox="0 0 24 24"
                stroke="currentColor"
                strokeWidth={1.5}
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

        <Outlet />
      </main>
    </div>
  );
}
