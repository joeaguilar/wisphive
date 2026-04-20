/**
 * Per-device bearer token storage for the web UI.
 *
 * itr#213 retired the per-process `/api/web-token` bootstrap in favour
 * of `/api/auth/login`, which returns a device-scoped token the operator
 * is expected to save. itr#217 will ship the proper Login.tsx / setup
 * flow that populates this localStorage slot on successful login. Until
 * then, this file is the minimum glue that lets a manually-acquired
 * token (e.g. `curl -sk -X POST .../api/auth/login`) drive the UI — set
 * `localStorage['wisphive-web-token']` via devtools and reload.
 *
 * Design invariants:
 * - The raw token only lives in localStorage on the device it was issued
 *   to. The server stores `sha256(raw)` in `web_devices.token_hash` and
 *   never sees the raw after login. See crates/wisphive_web/src/auth.rs.
 * - `getWebToken` is sync; no round-trip on every call. When we ship the
 *   proper login UI the token arrives in one atomic `setItem` at login
 *   success and is removed at logout, so no caching dance is needed.
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

/** Persist a device token (called by Login.tsx once itr#217 lands). */
export function setWebToken(raw: string): void {
  try {
    localStorage.setItem(TOKEN_STORAGE_KEY, raw);
  } catch (e) {
    console.warn("localStorage write failed:", e);
  }
}

/** Clear the device token (logout). */
export function clearWebToken(): void {
  try {
    localStorage.removeItem(TOKEN_STORAGE_KEY);
  } catch {
    // Nothing actionable; next page load won't see the token either way.
  }
}

/** `fetch` wrapper that adds the bearer token to the Authorization header. */
export async function apiFetch(
  path: string,
  init: RequestInit = {},
): Promise<Response> {
  const token = getWebToken();
  const headers = new Headers(init.headers);
  if (token) headers.set("Authorization", `Bearer ${token}`);
  return fetch(`${API_BASE}${path}`, { ...init, headers });
}
