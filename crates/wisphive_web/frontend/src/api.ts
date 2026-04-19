/**
 * Bootstrap the per-process bearer token the daemon requires on /ws and
 * /api/*. `/api/web-token` itself is gated by the daemon's Origin+Host
 * allowlists (no bearer needed), which is what lets us fetch it at all.
 *
 * The token is cached as a Promise so concurrent callers share a single
 * fetch, and cleared on failure so the next call will retry.
 */

const API_BASE = import.meta.env.VITE_API_URL || "";

let tokenPromise: Promise<string | null> | null = null;

export function getWebToken(): Promise<string | null> {
  if (!tokenPromise) {
    tokenPromise = fetch(`${API_BASE}/api/web-token`, { credentials: "omit" })
      .then((res) => {
        if (!res.ok) {
          console.warn(`/api/web-token returned ${res.status}`);
          return null;
        }
        return res.json();
      })
      .then((data: { token?: string } | null) => data?.token ?? null)
      .catch((e) => {
        console.warn("/api/web-token bootstrap failed:", e);
        tokenPromise = null;
        return null;
      });
  }
  return tokenPromise;
}

/** `fetch` wrapper that adds the bearer token to the Authorization header. */
export async function apiFetch(
  path: string,
  init: RequestInit = {},
): Promise<Response> {
  const token = await getWebToken();
  const headers = new Headers(init.headers);
  if (token) headers.set("Authorization", `Bearer ${token}`);
  return fetch(`${API_BASE}${path}`, { ...init, headers });
}
