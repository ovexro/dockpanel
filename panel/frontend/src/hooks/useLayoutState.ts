import { useState, useEffect, useRef, useMemo } from "react";
import { useAuth } from "../context/AuthContext";
import { api } from "../api";
import { navGroups, type NavGroup } from "../data/navItems";

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

  // Health check polling
  const healthTimer = useRef<ReturnType<typeof setInterval>>(undefined);
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

  // 2FA enforcement
  useEffect(() => {
    api.get<Record<string, string>>("/settings").then(s => {
      if (s.enforce_2fa === "true") setTwoFaEnforced(true);
    }).catch(() => {});
    api.get<{ enabled: boolean }>("/auth/2fa/status").then(d => setTwoFaEnabled(d.enabled)).catch(() => {});
  }, []);

  // Filter nav groups by role
  const visibleGroups = useMemo(() => {
    if (!user) return [];
    return navGroups.map(g => ({
      ...g,
      items: g.items.filter(item => {
        // Admin sees everything
        if (user.role === "admin") return true;
        // Reseller sees resellerVisible items + non-restricted items
        if (user.role === "reseller") return item.resellerVisible || (!item.adminOnly && !item.resellerVisible);
        // Regular user: hide adminOnly and resellerVisible items
        return !item.adminOnly && !item.resellerVisible;
      }),
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
    twoFaEnforced,
    twoFaEnabled,
    sidebarOpen,
    setSidebarOpen,
    visibleGroups,
  };
}
