const BASE = "/api";

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
    if (
      window.location.pathname !== "/login" &&
      window.location.pathname !== "/setup"
    ) {
      window.location.href = "/login";
    }
    throw new ApiError(401, "Unauthorized");
  }

  const data = await res.json().catch(() => ({}));

  if (!res.ok) {
    let message = (data as { error?: string }).error || `Request failed (${res.status})`;
    // Translate common backend errors into user-friendly messages
    if (res.status === 502 || message.includes("agent connection failed")) {
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
