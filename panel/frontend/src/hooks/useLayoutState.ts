import { useState, useEffect, useRef, useMemo } from "react";
import { useAuth } from "../context/AuthContext";
import { api } from "../api";
import { navGroups, isNavVisible, type NavGroup } from "../data/navItems";

const themeOrder = ["terminal", "midnight", "ember", "arctic", "clean", "clean-dark"] as const;

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

  const [theme, setThemeRaw] = useState(() => {
    const stored = localStorage.getItem("dp-theme");
    if (!stored || stored === "dark") return "midnight";
    if (stored === "light") return "arctic";
    if (stored === "nexus") return "clean";
    if (stored === "nexus-dark") return "clean-dark";
    return stored;
  });

  const layout = localStorage.getItem("dp-layout") || "command";

  const setTheme = (t: string) => {
    setThemeRaw(t);
    localStorage.setItem("dp-theme", t);
    document.documentElement.setAttribute("data-theme", t);
    document.documentElement.setAttribute("data-color-scheme", (t === "clean" || t === "arctic") ? "light" : "dark");
  };

  const cycleTheme = () => {
    const idx = themeOrder.indexOf(theme as (typeof themeOrder)[number]);
    const next = themeOrder[(idx + 1) % themeOrder.length];
    setTheme(next);
  };

  // Sync theme to DOM
  useEffect(() => {
    document.documentElement.setAttribute("data-theme", theme);
    localStorage.setItem("dp-theme", theme);
  }, [theme]);

  // Alert count + notification count polling (fallback, 60s since SSE handles real-time)
  const alertTimer = useRef<ReturnType<typeof setInterval>>(undefined);
  useEffect(() => {
    const fetchCounts = () => {
      api.get<{ firing: number }>("/alerts/summary")
        .then((s) => setFiringCount(s.firing))
        .catch(() => {});
      api.get<{ id: string; status: string }[]>("/incidents?status=investigating&limit=100")
        .then((incs) => setIncidentCount(Array.isArray(incs) ? incs.length : 0))
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
