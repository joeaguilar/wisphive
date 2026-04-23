import { useCallback, useEffect, useState } from "react";
import {
  apiFetch,
  clearWebToken,
  getWebToken,
  setWebToken,
  subscribeAuthChange,
} from "../api";

export type AuthPhase =
  | "loading"       // probing /api/auth/status on first mount
  | "setup"         // no password set on host — can't log in yet
  | "unauthed"      // password set, no local token
  | "authed";       // local token present

export interface AuthError {
  /** `invalid`: the submitted password didn't meet the endpoint's rules
   *  (wrong for login, too short for set-password). `conflict`: the host
   *  is already provisioned (409 from set-password) — a distinct signal
   *  so the UI can wipe the typed "new password" before flipping to
   *  login mode. `throttled`/`network`/`server`: not the user's fault. */
  kind: "invalid" | "conflict" | "throttled" | "network" | "server";
  message: string;
  /** Seconds the client must wait before retrying (429 Retry-After). */
  retryAfter?: number;
}

export interface UseAuth {
  phase: AuthPhase;
  token: string | null;
  error: AuthError | null;
  /** Submit credentials. Returns true on success. */
  login: (password: string, deviceName?: string) => Promise<boolean>;
  /** First-run only: set the initial password and log in atomically.
   * Returns true on success. 409 (password already set) flips phase to
   * unauthed so the form can recover without a reload. */
  setPassword: (password: string, deviceName?: string) => Promise<boolean>;
  logout: () => Promise<void>;
  /** Re-probe /api/auth/status (e.g. after an external password-set). */
  refreshStatus: () => Promise<void>;
  /** Dismiss the current error so the form can be retried cleanly. */
  clearError: () => void;
}

interface AuthStatus {
  password_set: boolean;
  setup_required: boolean;
}

/**
 * useAuth — single source of truth for web-UI login state.
 *
 * Mount-time flow:
 * 1. If a token is already in localStorage → phase = "authed" (optimistic;
 *    the first authenticated request will 401 us back to "unauthed" via
 *    api.ts::apiFetch if the token is stale).
 * 2. Otherwise probe GET /api/auth/status to distinguish "setup required"
 *    (first-run host, no password ever set) from "just log in".
 *
 * Subscribes to api.ts auth-change events so a 401 from any fetch site
 * drops us back to "unauthed" without the caller wiring anything.
 */
export function useAuth(): UseAuth {
  const [token, setToken] = useState<string | null>(() => getWebToken());
  const [phase, setPhase] = useState<AuthPhase>(() =>
    getWebToken() ? "authed" : "loading",
  );
  const [error, setError] = useState<AuthError | null>(null);

  const probeStatus = useCallback(async () => {
    try {
      // Route through apiFetch even though /api/auth/status is the one
      // unauthenticated endpoint: keeps the "all fetch sites share the
      // same auth-failure side effects" invariant documented in api.ts
      // intact if this endpoint ever gains auth semantics.
      const res = await apiFetch("/api/auth/status");
      if (!res.ok) {
        setPhase("unauthed");
        return;
      }
      const body = (await res.json()) as AuthStatus;
      setPhase(body.setup_required ? "setup" : "unauthed");
    } catch {
      // Network error during status probe → assume unauthed so the user
      // at least sees the login form and can retry. Setup detection will
      // correct itself on the next successful probe.
      setPhase("unauthed");
    }
  }, []);

  // On mount: if we don't already hold a token, figure out if setup is
  // needed. This is an external-system probe (GET /api/auth/status), so
  // the setState happens in the fetch callback — that's the "subscribe"
  // branch of the react-hooks/set-state-in-effect rule.
  useEffect(() => {
    if (!token) {
      // eslint-disable-next-line react-hooks/set-state-in-effect
      void probeStatus();
    }
  }, [token, probeStatus]);

  // Re-gate when api.ts clears the token (401/403 from any fetch).
  useEffect(() => {
    return subscribeAuthChange(() => {
      const next = getWebToken();
      setToken(next);
      if (!next) {
        setPhase((prev) => (prev === "setup" ? "setup" : "unauthed"));
      } else {
        setPhase("authed");
        setError(null);
      }
    });
  }, []);

  const login = useCallback(
    async (password: string, deviceName?: string): Promise<boolean> => {
      setError(null);
      try {
        const res = await apiFetch("/api/auth/login", {
          method: "POST",
          headers: { "content-type": "application/json" },
          body: JSON.stringify({
            password,
            device_name: deviceName?.trim() || undefined,
          }),
        });
        if (res.status === 200) {
          const body = (await res.json()) as { device_id: string; token: string };
          // setWebToken fires the auth-change subscription, but also
          // update local state directly — don't make correctness depend
          // on listener iteration ordering, and prevent a racing render
          // from seeing phase="unauthed" between setWebToken and the
          // listener callback.
          setWebToken(body.token);
          setToken(body.token);
          setPhase("authed");
          setError(null);
          return true;
        }
        if (res.status === 401) {
          setError({ kind: "invalid", message: "Invalid password." });
          return false;
        }
        if (res.status === 429) {
          const retry = Number(res.headers.get("retry-after"));
          // Cap at 1 hour — a malformed or malicious upstream sending
          // Retry-After: 999999999 would otherwise freeze the UI for
          // decades with a valid-looking countdown. Floor at 30s keeps
          // the throttle visible without being noise.
          const retryAfter =
            Number.isFinite(retry) && retry > 0 ? Math.min(retry, 3600) : 30;
          setError({
            kind: "throttled",
            message: "Too many attempts.",
            retryAfter,
          });
          return false;
        }
        setError({
          kind: "server",
          message: `Login failed (${res.status}).`,
        });
        return false;
      } catch (e) {
        setError({
          kind: "network",
          message: `Could not reach daemon: ${e instanceof Error ? e.message : String(e)}`,
        });
        return false;
      }
    },
    [],
  );

  const setPassword = useCallback(
    async (password: string, deviceName?: string): Promise<boolean> => {
      setError(null);
      try {
        const res = await apiFetch("/api/auth/set-password", {
          method: "POST",
          headers: { "content-type": "application/json" },
          body: JSON.stringify({
            password,
            device_name: deviceName?.trim() || undefined,
          }),
        });
        if (res.status === 200) {
          const body = (await res.json()) as { device_id: string; token: string };
          // Same atomic-login pattern as login(): persist token, move
          // phase, and clear error without relying on listener ordering.
          setWebToken(body.token);
          setToken(body.token);
          setPhase("authed");
          return true;
        }
        if (res.status === 400) {
          const text = await res.text();
          setError({
            kind: "invalid",
            message: text || "Password does not meet requirements.",
          });
          return false;
        }
        if (res.status === 409) {
          // Someone else set the password first (or the operator did it
          // via CLI between page load and form submit). Flip to login
          // phase so the same form can transition without a reload.
          // Use the `conflict` kind so Login.tsx knows to wipe the
          // typed "new password" (it's not a valid login input).
          setPhase("unauthed");
          setError({
            kind: "conflict",
            message: "Password is already set — sign in instead.",
          });
          return false;
        }
        if (res.status === 429) {
          const retry = Number(res.headers.get("retry-after"));
          const retryAfter =
            Number.isFinite(retry) && retry > 0 ? Math.min(retry, 3600) : 30;
          setError({
            kind: "throttled",
            message: "Too many attempts.",
            retryAfter,
          });
          return false;
        }
        setError({
          kind: "server",
          message: `Set password failed (${res.status}).`,
        });
        return false;
      } catch (e) {
        setError({
          kind: "network",
          message: `Could not reach daemon: ${e instanceof Error ? e.message : String(e)}`,
        });
        return false;
      }
    },
    [],
  );

  const logout = useCallback(async () => {
    // Best-effort server-side revoke; local state is authoritative for UX.
    try {
      await apiFetch("/api/auth/logout", { method: "POST" });
    } catch {
      // Offline logout still clears the local token below.
    }
    clearWebToken();
  }, []);

  const clearError = useCallback(() => setError(null), []);

  return {
    phase,
    token,
    error,
    login,
    setPassword,
    logout,
    refreshStatus: probeStatus,
    clearError,
  };
}
