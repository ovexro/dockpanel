import { useState, useEffect, useRef, useMemo } from "react";
import { useAuth } from "../context/AuthContext";
import { api } from "../api";
import { navGroups, isNavVisible, type NavGroup } from "../data/navItems";

const themeOrder = ["terminal", "midnight", "ember", "arctic", "clean", "clean-dark"] as const;

/** Fired after any user-initiated theme change so every mounted reader re-seeds.
 *  Distinct from `dp-layout-change` on purpose — reusing that name would make
 *  LayoutShell re-read `dp-layout` on every theme click. */
export const THEME_CHANGE_EVENT = "dp-theme-change";

/** Resolves the stored theme WITHOUT writing anything back. The legacy ids are
 *  migrated on read, so an old value keeps working while a never-set value stays
 *  detectably absent. Mirrored (unavoidably) in public/theme-init.js, which is a
 *  classic render-blocking script and cannot import this module. */
export function readStoredTheme(): string {
  const stored = localStorage.getItem("dp-theme");
  if (!stored || stored === "dark") return "midnight";
  if (stored === "light") return "arctic";
  if (stored === "nexus") return "clean";
  if (stored === "nexus-dark") return "clean-dark";
  return stored;
}

/** THE writer for a user-initiated theme change: persists, stamps both root
 *  attributes, and announces. Every theme setter in the app must go through
 *  here — a caller that writes the DOM itself leaves `theme` below stale, which
 *  is what made the header cycle button look dead for one click.
 *
 *  `data-color-scheme` is load-bearing: index.css reads it to hand the native
 *  scrollbars and form controls the right scheme, so it must stay "light" for
 *  arctic and clean and "dark" for the other four. */
export function applyTheme(t: string): void {
  localStorage.setItem("dp-theme", t);
  const root = document.documentElement;
  root.setAttribute("data-theme", t);
  root.setAttribute("data-color-scheme", (t === "clean" || t === "arctic") ? "light" : "dark");
  window.dispatchEvent(new Event(THEME_CHANGE_EVENT));
}

export interface LayoutState {
  user: { email: string; role: string };
  logout: () => void;
  loading: boolean;
  theme: string;
  setTheme: (t: string) => void;
  cycleTheme: () => void;
  layout: string;
  firingCount: number;
  incidentCount: number;
  notifCount: number;
  apiHealthy: boolean | null;
  /** Whether to render the host-health indicator at all. False for every
   *  non-admin: `/settings/health` is admin-only, so there is nothing behind it
   *  for them — and a permanent "Checking..." is a quieter version of the same
   *  lie the pulsing "Disconnected" told. */
  canSeeHealth: boolean;
  twoFaEnforced: boolean;
  twoFaEnabled: boolean;
  sidebarOpen: boolean;
  setSidebarOpen: (v: boolean) => void;
  visibleGroups: NavGroup[];
}

export function useLayoutState(): LayoutState {
  const { user, logout, loading } = useAuth();
  const [sidebarOpen, setSidebarOpen] = useState(false);
  const [firingCount, setFiringCount] = useState(0);
  const [incidentCount, setIncidentCount] = useState(0);
  const [notifCount, setNotifCount] = useState(0);
  const [apiHealthy, setApiHealthy] = useState<boolean | null>(null);
  const [twoFaEnforced, setTwoFaEnforced] = useState(false);
  const [twoFaEnabled, setTwoFaEnabled] = useState(true);

  const [theme, setThemeRaw] = useState(readStoredTheme);

  const layout = localStorage.getItem("dp-layout") || "command";

  const setTheme = (t: string) => {
    setThemeRaw(t);
    applyTheme(t);
  };

  const cycleTheme = () => {
    const idx = themeOrder.indexOf(theme as (typeof themeOrder)[number]);
    const next = themeOrder[(idx + 1) % themeOrder.length];
    setTheme(next);
  };

  // Re-seed after any OTHER writer (the Settings theme picker) changes the theme.
  //
  // This replaces a `useEffect(..., [theme])` that stamped both the DOM attribute
  // and `dp-theme` into localStorage. A dep-array effect RUNS ON MOUNT, so that
  // one persisted "midnight" on the first authenticated render of a user who had
  // never picked a theme — the same defect as the old main.tsx write, and fixing
  // only main.tsx would have left this one live.
  //
  // Nothing needs to re-assert the DOM here: theme-init.js sets it before first
  // paint and applyTheme() sets it on every user-initiated change. This effect
  // only keeps `theme` — the value cycleTheme steps from — in step, so the first
  // click after a Settings pick advances from what is actually on screen.
  useEffect(() => {
    const onThemeChange = () => setThemeRaw(readStoredTheme());
    window.addEventListener(THEME_CHANGE_EVENT, onThemeChange);
    return () => window.removeEventListener(THEME_CHANGE_EVENT, onThemeChange);
  }, []);

  // Alert count + notification count polling (fallback, 60s since SSE handles real-time)
  const alertTimer = useRef<ReturnType<typeof setInterval>>(undefined);
  useEffect(() => {
    const fetchCounts = () => {
      api.get<{ firing: number }>("/alerts/summary")
        .then((s) => setFiringCount(s.firing))
        .catch(() => {});
      // Asks the panel's own question rather than a narrower one of its own.
      // This used to fetch `?status=investigating&limit=100` and take the array
      // length: `investigating` is one of three open statuses, so an incident
      // dropped out of this count the moment anyone moved it to `identified` —
      // which is the first thing the incident screen offers — while the
      // dashboard went on counting it. The length was also capped at the page
      // size it asked for.
      api.get<{ open: number }>("/incidents/summary")
        .then((s) => setIncidentCount(s.open ?? 0))
        .catch(() => {});
      api.get<{ count: number }>("/notifications/unread-count")
        .then((d) => setNotifCount(d.count))
        .catch(() => {});
    };
    fetchCounts();
    alertTimer.current = setInterval(fetchCounts, 60000);
    return () => { if (alertTimer.current) clearInterval(alertTimer.current); };
  }, []);

  // SSE connection for real-time notification delivery
  useEffect(() => {
    const es = new EventSource("/api/notifications/stream");
    es.onmessage = () => {
      // Refresh unread count on any new notification
      api.get<{ count: number }>("/notifications/unread-count")
        .then((d) => setNotifCount(d.count))
        .catch(() => {});
    };
    es.onerror = () => {
      // Browser auto-reconnects EventSource on error
    };
    return () => es.close();
  }, []);

  // Health check polling.
  //
  // `/settings/health` is admin-only, and this `.catch` did not distinguish "the
  // panel is unwell" from "you were not allowed to ask" — so every non-admin got
  // `apiHealthy === false` on the first tick and every 30s after, which the four
  // layouts render as a pulsing red "Disconnected" / "Issues Detected" in the
  // sidebar. A client signed in and the panel told them, on every page, that it
  // was broken. Not asking is the fix; the state already models "unknown" as
  // `null`, and `canSeeHealth` below keeps the indicator off the screen rather
  // than leaving it to say "Checking..." for ever.
  const isAdmin = user?.role === "admin";
  const healthTimer = useRef<ReturnType<typeof setInterval>>(undefined);
  useEffect(() => {
    if (!isAdmin) return;
    const checkHealth = () => {
      api.get<{ db: string; agent: string }>("/settings/health")
        .then((h) => setApiHealthy(h.db === "ok" && h.agent === "ok"))
        .catch(() => setApiHealthy(false));
    };
    checkHealth();
    healthTimer.current = setInterval(checkHealth, 30000);
    return () => { if (healthTimer.current) clearInterval(healthTimer.current); };
  }, [isAdmin]);

  // 2FA status AND whether the panel requires it.
  //
  // Both now come from `/auth/2fa/status`, which is `AuthUser`. Enforcement used
  // to be read from `GET /api/settings` — an `AdminUser` route — with the 403
  // swallowed by an empty catch, so `twoFaEnforced` stayed false for exactly the
  // population the banner exists to warn, and only admins were ever told that
  // admins had made 2FA mandatory.
  useEffect(() => {
    api.get<{ enabled: boolean; enforced?: boolean }>("/auth/2fa/status")
      .then(d => { setTwoFaEnabled(d.enabled); setTwoFaEnforced(!!d.enforced); })
      .catch(() => {});
  }, []);

  // Filter nav groups by role
  const visibleGroups = useMemo(() => {
    if (!user) return [];
    return navGroups.map(g => ({
      ...g,
      // Extracted to `navItems.ts` so the command palette can read the same
      // decision instead of having its own (it had none at all).
      items: g.items.filter(item => isNavVisible(item, user.role)),
    })).filter(g => g.items.length > 0);
  }, [user]);

  return {
    user: user || { email: "", role: "" },
    logout,
    loading,
    theme,
    setTheme,
    cycleTheme,
    layout,
    firingCount,
    incidentCount,
    notifCount,
    apiHealthy,
    canSeeHealth: !!isAdmin,
    twoFaEnforced,
    twoFaEnabled,
    sidebarOpen,
    setSidebarOpen,
    visibleGroups,
  };
}
