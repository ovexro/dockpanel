import { createContext, useContext, useState, useEffect, ReactNode } from "react";
import { api, ApiError } from "../api";
import { logger } from "../utils/logger";

interface User {
  id: string;
  email: string;
  role: string;
}

interface TwoFaChallenge {
  temp_token: string;
}

interface AuthContextType {
  user: User | null;
  login: (email: string, password: string) => Promise<TwoFaChallenge | null>;
  verify2fa: (tempToken: string, code: string) => Promise<void>;
  logout: () => void;
  loading: boolean;
}

const AuthContext = createContext<AuthContextType | null>(null);

export function AuthProvider({ children }: { children: ReactNode }) {
  const [user, setUser] = useState<User | null>(null);
  const [loading, setLoading] = useState(true);

  useEffect(() => {
    fetch("/api/auth/me", { credentials: "same-origin" })
      .then((res) => (res.ok ? res.json() : null))
      .then((data) => {
        if (data) setUser(data);
      })
      .catch(() => { /* not logged in */ })
      .finally(() => setLoading(false));
  }, []);

  const login = async (email: string, password: string): Promise<TwoFaChallenge | null> => {
    const data = await api.post<{ user?: User; requires_2fa?: boolean; temp_token?: string }>("/auth/login", {
      email,
      password,
    });
    if (data.requires_2fa && data.temp_token) {
      return { temp_token: data.temp_token };
    }
    if (data.user) {
      setUser(data.user);
    }
    return null;
  };

  const verify2fa = async (tempToken: string, code: string) => {
    const data = await api.post<{ user: User }>("/auth/2fa/verify", {
      temp_token: tempToken,
      code,
    });
    setUser(data.user);
  };

  const logout = () => {
    api.post("/auth/logout").catch((e) => logger.error("Logout error:", e));
    setUser(null);
  };

  return (
    <AuthContext.Provider value={{ user, login, verify2fa, logout, loading }}>
      {children}
    </AuthContext.Provider>
  );
}

export function useAuth() {
  const ctx = useContext(AuthContext);
  if (!ctx) throw new Error("useAuth must be used within AuthProvider");
  return ctx;
}
