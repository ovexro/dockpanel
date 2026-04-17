import { createContext, useContext, useEffect, useState, useCallback, type ReactNode } from "react";
import { api } from "../api";

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
  const [servers, setServers] = useState<Server[]>([]);
  const [activeServerId, setActiveServerIdState] = useState<string | null>(
    () => localStorage.getItem("dp-active-server")
  );
  const [loading, setLoading] = useState(true);

  const fetchServers = useCallback(async () => {
    try {
      const data = await api.get<Server[]>("/servers");
      setServers(data);

      // If no active server selected, default to local
      const stored = localStorage.getItem("dp-active-server");
      if (!stored || !data.find((s) => s.id === stored)) {
        const local = data.find((s) => s.is_local);
        if (local) {
          localStorage.setItem("dp-active-server", local.id);
          setActiveServerIdState(local.id);
        }
      }
    } catch {
      // Not logged in yet or API error — ignore
    } finally {
      setLoading(false);
    }
  }, []);

  useEffect(() => {
    fetchServers();
    // Refresh server list every 60s
    const interval = setInterval(fetchServers, 60000);
    return () => clearInterval(interval);
  }, [fetchServers]);

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
