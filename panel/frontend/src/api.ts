const BASE = "/api";

// Routes the SPA serves to unauthenticated users. A 401 from a background
// API call (e.g. ServerProvider's /servers fetch on mount) must NOT bounce
// the user off these pages — the user got here on purpose, usually with a
// time-limited token in the URL that doesn't survive a full navigation.
const PUBLIC_AUTH_PATHS = new Set([
  "/login",
  "/setup",
  "/register",
  "/forgot-password",
  "/reset-password",
  "/verify-email",
]);

export class ApiError extends Error {
  status: number;
  constructor(status: number, message: string) {
    super(message);
    this.status = status;
  }
}

async function request<T = unknown>(
  path: string,
  options?: RequestInit
): Promise<T> {
  const headers: Record<string, string> = {
    "X-Requested-With": "DockPanel",
  };
  if (options?.body) headers["Content-Type"] = "application/json";

  // Multi-server: attach X-Server-Id header if a server is selected
  const serverId = localStorage.getItem("dp-active-server");
  if (serverId) headers["X-Server-Id"] = serverId;

  const res = await fetch(`${BASE}${path}`, {
    ...options,
    credentials: "same-origin",
    headers: { ...headers, ...(options?.headers as Record<string, string>) },
  });

  if (res.status === 401) {
    if (!PUBLIC_AUTH_PATHS.has(window.location.pathname)) {
      window.location.href = "/login";
    }
    throw new ApiError(401, "Unauthorized");
  }

  const data = await res.json().catch(() => ({}));

  if (!res.ok) {
    let message = (data as { error?: string }).error || `Request failed (${res.status})`;
    // Only the backend can tell "the agent never answered" apart from "the
    // agent answered and something failed inside it" — both arrive as 502.
    // Keying this off the bare status is what reported a working agent as
    // offline whenever it refused a request (issues #90, #91, #92), and did
    // the same for a Stripe or Cloudflare failure that never involved it.
    if ((data as { code?: string }).code === "agent_unreachable") {
      message = "Agent offline — the DockPanel agent is not responding.";
    }
    throw new ApiError(res.status, message);
  }

  return data as T;
}

export const api = {
  get: <T = unknown>(path: string) => request<T>(path),
  post: <T = unknown>(path: string, body?: unknown) =>
    request<T>(path, {
      method: "POST",
      body: body ? JSON.stringify(body) : undefined,
    }),
  put: <T = unknown>(path: string, body?: unknown) =>
    request<T>(path, {
      method: "PUT",
      body: body ? JSON.stringify(body) : undefined,
    }),
  delete: <T = unknown>(path: string) =>
    request<T>(path, { method: "DELETE" }),
};
