/**
 * Per-device bearer token storage for the web UI.
 *
 * itr#213 retired the per-process `/api/web-token` bootstrap in favour
 * of `/api/auth/login`, which returns a device-scoped token the operator
 * is expected to save. itr#217 ships the Login.tsx flow that populates
 * this localStorage slot on successful login; on 401/403 from any
 * authenticated request the token is cleared and subscribers (the app
 * shell) are notified so the UI re-gates to Login.
 *
 * Design invariants:
 * - The raw token only lives in localStorage on the device it was issued
 *   to. The server stores `sha256(raw)` in `web_devices.token_hash` and
 *   never sees the raw after login. See crates/wisphive_web/src/auth.rs.
 * - `getWebToken` is sync; no round-trip on every call. The token arrives
 *   in one atomic `setItem` at login success and is removed on logout or
 *   server-side revocation (401/403).
 * - Auth-failure side effects are centralised here so every fetch site
 *   gets the same behaviour without each caller re-implementing it.
 */

const API_BASE = import.meta.env.VITE_API_URL || "";
const TOKEN_STORAGE_KEY = "wisphive-web-token";

/** Returns the currently-stored device token, or null if none is set. */
export function getWebToken(): string | null {
  try {
    return localStorage.getItem(TOKEN_STORAGE_KEY);
  } catch {
    // Private-browsing / storage-disabled → no token, no crash.
    return null;
  }
}

/** Persist a device token (called by Login.tsx on successful login). */
export function setWebToken(raw: string): void {
  try {
    localStorage.setItem(TOKEN_STORAGE_KEY, raw);
  } catch (e) {
    console.warn("localStorage write failed:", e);
  }
  notifyAuthChange();
}

/** Clear the device token (logout or server-side revocation). */
export function clearWebToken(): void {
  try {
    localStorage.removeItem(TOKEN_STORAGE_KEY);
  } catch {
    // Nothing actionable; next page load won't see the token either way.
  }
  notifyAuthChange();
}

type AuthListener = () => void;
const authListeners = new Set<AuthListener>();

/**
 * Subscribe to auth-state transitions (token set, cleared, or rejected by
 * the server). The callback fires after the token has been mutated, so
 * readers can re-read `getWebToken()` to see the new state.
 */
export function subscribeAuthChange(listener: AuthListener): () => void {
  authListeners.add(listener);
  return () => {
    authListeners.delete(listener);
  };
}

function notifyAuthChange(): void {
  for (const l of authListeners) {
    try {
      l();
    } catch (e) {
      console.warn("auth listener threw:", e);
    }
  }
}

/** `fetch` wrapper that adds the bearer token and handles auth failure. */
export async function apiFetch(
  path: string,
  init: RequestInit = {},
): Promise<Response> {
  const token = getWebToken();
  const headers = new Headers(init.headers);
  if (token) headers.set("Authorization", `Bearer ${token}`);
  const res = await fetch(`${API_BASE}${path}`, { ...init, headers });
  if ((res.status === 401 || res.status === 403) && token) {
    // Server rejected our token — either revoked, expired, or the daemon
    // restarted with a fresh DB. Drop the local token so the app re-gates
    // to Login on the next render. Endpoints that legitimately 401 without
    // a token (e.g. /api/auth/login itself) are unaffected because the
    // guard above skips when no token was sent.
    clearWebToken();
  }
  return res;
}
