const BASE = "/api";

// Routes the SPA serves to unauthenticated users. A 401 from a background
// API call (e.g. ServerProvider's /servers fetch on mount) must NOT bounce
// the user off these pages — the user got here on purpose, usually with a
// time-limited token in the URL that doesn't survive a full navigation.
//
// ⚠ "/status" belongs to that description and was missing from it until
// v2.87.0. The public status page is the one page whose entire purpose is to
// be readable by someone with no account, during the outage that made them
// look — and ServerProvider mounts OUTSIDE BrowserRouter, so its /servers
// fetch runs on that route too, took a 401 and navigated the visitor to
// /login before the page could render. The page was public and unreachable.
const PUBLIC_PATHS = new Set([
  "/login",
  "/setup",
  "/register",
  "/forgot-password",
  "/reset-password",
  "/verify-email",
  "/status",
]);

// The backend's marker for "you have no usable session". It is minted in
// exactly one place — the AuthUser extractor — and it is the ONLY thing that
// entitles this module to navigate someone to /login.
const CODE_SESSION_INVALID = "session_invalid";

export class ApiError extends Error {
  status: number;
  /// Stable marker the backend sends alongside the status, for the cases where
  /// the status alone cannot say what happened.
  code?: string;
  body?: Record<string, unknown>;
  constructor(
    status: number,
    message: string,
    code?: string,
    body?: Record<string, unknown>
  ) {
    super(message);
    this.status = status;
    this.code = code;
    this.body = body;
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

  const data = await res.json().catch(() => ({}));

  if (res.status === 401) {
    // The body is parsed ABOVE this branch on purpose. Until v2.87.0 the 401
    // case returned first, so every sentence written at a 401 site in this
    // codebase was discarded unread — "Invalid credentials" on the login form,
    // "Current password is incorrect" on the account card, and "Two-factor
    // authentication is no longer set up on this account. Sign in again.",
    // which is the one message an administrator whose 2FA was reset needs. All
    // of them reached the user as the fixed word "Unauthorized".
    //
    // Redirecting is likewise not something a status code can justify. Only the
    // backend knows whether a 401 means "your session is gone" or "the
    // credential you just typed was wrong", and it says so with a marker minted
    // in one place. A 401 from a handler is proof the session was ACCEPTED —
    // the extractor let the request through — so bouncing the user to /login
    // for one answers a question nobody asked, and destroys the explanation on
    // the way out.
    if (
      (data as { code?: string }).code === CODE_SESSION_INVALID &&
      !PUBLIC_PATHS.has(window.location.pathname)
    ) {
      window.location.href = "/login";
    }
    throw new ApiError(
      401,
      (data as { error?: string }).error || "Unauthorized",
      (data as { code?: string }).code,
      data as Record<string, unknown>
    );
  }

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
    throw new ApiError(
      res.status,
      message,
      (data as { code?: string }).code,
      data as Record<string, unknown>
    );
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
