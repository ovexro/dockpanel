import { createContext, useContext, useEffect, useState, useCallback, type ReactNode } from "react";
import { api } from "../api";
import { useAuth } from "./AuthContext";

export interface Server {
  id: string;
  name: string;
  ip_address: string | null;
  agent_url: string | null;
  status: string;
  is_local: boolean;
  os_info: string | null;
  cpu_cores: number | null;
  ram_mb: number | null;
  disk_gb: number | null;
  agent_version: string | null;
  cpu_usage: number | null;
  mem_used_mb: number | null;
  uptime_secs: number | null;
  last_seen_at: string | null;
  cert_fingerprint: string | null;
  created_at: string;
}

interface ServerContextType {
  servers: Server[];
  activeServer: Server | null;
  activeServerId: string | null;
  setActiveServerId: (id: string | null) => void;
  isLocal: boolean;
  refreshServers: () => Promise<void>;
  loading: boolean;
}

const ServerContext = createContext<ServerContextType>({
  servers: [],
  activeServer: null,
  activeServerId: null,
  setActiveServerId: () => {},
  isLocal: true,
  refreshServers: async () => {},
  loading: true,
});

export function ServerProvider({ children }: { children: ReactNode }) {
  const { user } = useAuth();
  const [servers, setServers] = useState<Server[]>([]);
  const [activeServerId, setActiveServerIdState] = useState<string | null>(
    () => localStorage.getItem("dp-active-server")
  );
  const [loading, setLoading] = useState(true);

  const fetchServers = useCallback(async () => {
    try {
      const data = await api.get<Server[]>("/servers");
      setServers(data);

      // If no active server selected, default to local.
      //
      // ⚠ The `else` below is load-bearing and its absence was a 403 on every
      // request. `dp-active-server` is localStorage, so it is per-BROWSER, not
      // per-account, and `GET /api/servers` is `WHERE user_id = $1` — no
      // non-admin owns a `servers` row, so a client's list comes back EMPTY.
      // Without the else, `data.find(is_local)` was undefined, the stale id the
      // admin left on that browser was never cleared, `api.ts` kept attaching it
      // as `X-Server-Id`, and `ServerScope` answered
      // "Server not found or access denied" for a server the client does not own.
      // Reported from the field on #51 by an operator testing a client account in
      // his own browser — the one way anybody would test it.
      const stored = localStorage.getItem("dp-active-server");
      if (!stored || !data.find((s) => s.id === stored)) {
        const local = data.find((s) => s.is_local);
        if (local) {
          localStorage.setItem("dp-active-server", local.id);
          setActiveServerIdState(local.id);
        } else {
          // Nothing in this account's list can back the stored id. Drop it and
          // send no header at all, which `ServerScope` resolves to the local
          // server rather than refusing.
          localStorage.removeItem("dp-active-server");
          setActiveServerIdState(null);
        }
      }
    } catch {
      // Not logged in yet or API error — ignore
    } finally {
      setLoading(false);
    }
  }, []);

  // ⚠ Keyed on the ACCOUNT, not just on mount. The server list is per-account —
  // `GET /api/servers` is `WHERE user_id = $1` — and signing in is an SPA state
  // change, not a remount, so an effect that runs once at boot leaves the
  // previous account's list (and the selection derived from it) in place until
  // the 60s tick. That window is exactly long enough for a client's first screen
  // to fail with "Server not found or access denied" on a fix that had already
  // shipped. Re-read whenever the account changes.
  useEffect(() => {
    fetchServers();
    // Refresh server list every 60s
    const interval = setInterval(fetchServers, 60000);
    return () => clearInterval(interval);
  }, [fetchServers, user?.id]);

  const setActiveServerId = useCallback((id: string | null) => {
    if (id) {
      localStorage.setItem("dp-active-server", id);
    } else {
      localStorage.removeItem("dp-active-server");
    }
    setActiveServerIdState(id);
  }, []);

  const activeServer = servers.find((s) => s.id === activeServerId) ?? null;
  const isLocal = activeServer?.is_local ?? true;

  return (
    <ServerContext.Provider
      value={{
        servers,
        activeServer,
        activeServerId,
        setActiveServerId,
        isLocal,
        refreshServers: fetchServers,
        loading,
      }}
    >
      {children}
    </ServerContext.Provider>
  );
}

export function useServer() {
  return useContext(ServerContext);
}
