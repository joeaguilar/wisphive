import { useCallback, useEffect, useRef, useState } from "react";
import {
  apiFetch,
  clearWebToken,
  getWebToken,
  setWebToken,
  subscribeAuthChange,
} from "../api";
import { waitForAuthProfile } from "./useAuthProfile";
import type { PasskeyError } from "./usePasskey";

export type AuthPhase =
  | "loading"                 // probing /api/auth/status on first mount
  | "setup"                   // no password set on host — can't log in yet
  | "unauthed"                // password set, no local token
  | "authed-pending-enroll"   // token issued from set-password; Login.tsx
                              // still mounted so the user can opt into
                              // (or skip) passkey enrollment before the
                              // app shell takes over. Transient — flips
                              // to "authed" via `completeEnrollGate`.
                              // Only emitted when the active origin can
                              // host the enroll ceremony; otherwise
                              // setPassword goes straight to "authed".
  | "authed";                 // local token present, app shell visible

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
   * Returns true on success.
   *
   * Post-success phase semantics:
   * - If the active origin CAN host a passkey ceremony (per
   *   `useAuthProfile().canEnrollPasskeyOnThisOrigin`), phase moves to
   *   `"authed-pending-enroll"` so the Login surface stays mounted and
   *   can offer the optional enroll step. Call `completeEnrollGate()`
   *   from Login.tsx once the user has enrolled OR skipped.
   * - If the origin CAN'T host enrollment (e.g. LocalLAN on a LAN-IP),
   *   phase moves straight to `"authed"` — parking in the transient
   *   state would just render an empty card before flipping.
   *
   * 409 (password already set) flips phase to unauthed so the form can
   * recover without a reload. */
  setPassword: (password: string, deviceName?: string) => Promise<boolean>;
  /** Flip phase from `"authed-pending-enroll"` → `"authed"`. Called by
   * Login.tsx when the user finishes (or skips) the passkey enroll
   * step. A no-op if called from any other phase — defensive against
   * Login.tsx re-renders that fire the gate completion more than once.
   *
   * Held as a separate method (vs auto-flipping on a timer) so the
   * UX contract is explicit: the app shell appears when the Login
   * surface says it's done, not on a wall-clock guess. */
  completeEnrollGate: () => void;
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

function isAbortError(error: unknown): boolean {
  return (
    typeof error === "object" &&
    error !== null &&
    "name" in error &&
    error.name === "AbortError"
  );
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
  const requestsRef = useRef<Set<AbortController>>(new Set());

  const fetchWithAbort = useCallback(
    async (path: string, init: RequestInit = {}): Promise<Response> => {
      const controller = new AbortController();
      requestsRef.current.add(controller);
      try {
        return await apiFetch(path, { ...init, signal: controller.signal });
      } finally {
        requestsRef.current.delete(controller);
      }
    },
    [],
  );

  // Requests issued by this hook only matter while its consumer is mounted.
  // Abort them during teardown so their callbacks cannot publish stale state.
  useEffect(() => {
    const requests = requestsRef.current;
    return () => {
      for (const controller of requests) {
        controller.abort();
      }
      requests.clear();
    };
  }, []);

  const probeStatus = useCallback(async () => {
    try {
      // Route through apiFetch even though /api/auth/status is the one
      // unauthenticated endpoint: keeps the "all fetch sites share the
      // same auth-failure side effects" invariant documented in api.ts
      // intact if this endpoint ever gains auth semantics.
      const res = await fetchWithAbort("/api/auth/status");
      if (!res.ok) {
        setPhase("unauthed");
        return;
      }
      const body = (await res.json()) as AuthStatus;
      setPhase(body.setup_required ? "setup" : "unauthed");
    } catch (e) {
      if (isAbortError(e)) return;
      // Network error during status probe → assume unauthed so the user
      // at least sees the login form and can retry. Setup detection will
      // correct itself on the next successful probe.
      setPhase("unauthed");
    }
  }, [fetchWithAbort]);

  // On mount: if we don't already hold a token, figure out if setup is
  // needed. This is an external-system probe (GET /api/auth/status), so
  // the setState happens in the fetch callback — that's the "subscribe"
  // branch of the react-hooks/set-state-in-effect rule.
  useEffect(() => {
    if (!token) {
      void probeStatus();
    }
  }, [token, probeStatus]);

  // setPassword consults the auth-profile snapshot to decide whether to
  // park in `authed-pending-enroll` or flip straight to `authed`. It uses
  // `waitForAuthProfile()` rather than reading a `useAuthProfile()` hook
  // value because:
  //   1. The hook's render-time `loaded` flag is `false` until the probe
  //      resolves. A user who clicks "Set password" before the singleton
  //      probe finished would have seen `canEnroll=false` and skipped
  //      the enroll-pending phase — the manual smoke during wave-4
  //      caught exactly that race (see hooks/useAuthProfile.ts module
  //      docstring).
  //   2. `useAuthProfile()` here would create a second, race-prone
  //      probe whose result could disagree with Login.tsx's own hook
  //      instance (both pre-singleton probes were independent useStates).
  // The singleton in hooks/useAuthProfile.ts handles both concerns: one
  // probe per page, no duplicate fetches, and `waitForAuthProfile()` is
  // an async barrier that resolves to the same snapshot every consumer
  // sees.

  // Re-gate when api.ts clears the token (401/403 from any fetch) or
  // sets a new one (passkey login uses setWebToken directly to atomically
  // update the bearer through the shared event channel).
  useEffect(() => {
    return subscribeAuthChange(() => {
      const next = getWebToken();
      setToken(next);
      if (!next) {
        setPhase((prev) => (prev === "setup" ? "setup" : "unauthed"));
      } else {
        // External token arrival (e.g. usePasskey.loginWithPasskey called
        // from Login.tsx) means the user is fully authed — the
        // pending-enroll gate is only used for the post-set-password
        // flow, which goes through this hook's own setPassword path.
        setPhase("authed");
        setError(null);
      }
    });
  }, []);

  const login = useCallback(
    async (password: string, deviceName?: string): Promise<boolean> => {
      setError(null);
      try {
        const res = await fetchWithAbort("/api/auth/login", {
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
        if (isAbortError(e)) return false;
        setError({
          kind: "network",
          message: `Could not reach daemon: ${e instanceof Error ? e.message : String(e)}`,
        });
        return false;
      }
    },
    [fetchWithAbort],
  );

  const setPassword = useCallback(
    async (password: string, deviceName?: string): Promise<boolean> => {
      setError(null);
      try {
        const res = await fetchWithAbort("/api/auth/set-password", {
          method: "POST",
          headers: { "content-type": "application/json" },
          body: JSON.stringify({
            password,
            device_name: deviceName?.trim() || undefined,
          }),
        });
        if (res.status === 200) {
          const body = (await res.json()) as { device_id: string; token: string };
          // Persist token + update local state atomically (same pattern
          // as `login`) BUT branch on whether the active origin can host
          // the optional passkey enroll ceremony before deciding what
          // phase to move to.
          //
          // Why the branch: setPhase + Login.tsx's local setPendingEnroll
          // were previously batched by React 19, with the App.tsx gate
          // unmounting Login before the local state could render the
          // enroll card. The fix moves the gate into useAuth itself —
          // we land in `authed-pending-enroll` (Login stays mounted) and
          // Login.tsx calls `completeEnrollGate()` to advance to
          // `authed` once the user enrolls or skips.
          //
          // When the origin CAN'T host enrollment (LocalLAN + LAN-IP,
          // probe persistently failed, etc.) we skip the transient state
          // entirely — parking there would render an empty card before
          // flipping (the enroll affordance would be hidden).
          //
          // `waitForAuthProfile()` resolves to the singleton snapshot —
          // either the cached success from a prior subscriber's probe,
          // the in-flight probe's eventual result, or a fresh probe if
          // none ever ran. Either way, by the time this line returns we
          // have the authoritative `canEnrollPasskeyOnThisOrigin` for
          // *this* origin and aren't gambling on render-tick timing.
          setWebToken(body.token);
          setToken(body.token);
          const snapshot = await waitForAuthProfile();
          setPhase(
            snapshot.canEnrollPasskeyOnThisOrigin
              ? "authed-pending-enroll"
              : "authed",
          );
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
        if (isAbortError(e)) return false;
        setError({
          kind: "network",
          message: `Could not reach daemon: ${e instanceof Error ? e.message : String(e)}`,
        });
        return false;
      }
    },
    // setPassword's post-success phase choice depends on the live
    // `canEnrollPasskeyOnThisOrigin` flag, but it now reads it via
    // `waitForAuthProfile()` rather than closing over a hook value —
    // so the callback identity doesn't need to churn on profile
    // changes and the dep array stays empty. The async barrier handles
    // the timing.
    [fetchWithAbort],
  );

  const completeEnrollGate = useCallback(() => {
    // Defensive guard: only advance from the transient state. If the
    // caller fires this from any other phase (re-render, race with
    // the auth-change listener that already moved us to "authed",
    // tests, etc.), do nothing — the gate is one-way and idempotent.
    setPhase((prev) => (prev === "authed-pending-enroll" ? "authed" : prev));
  }, []);

  const logout = useCallback(async () => {
    // Best-effort server-side revoke; local state is authoritative for UX.
    try {
      await fetchWithAbort("/api/auth/logout", { method: "POST" });
    } catch {
      // Offline logout still clears the local token below.
    }
    clearWebToken();
  }, [fetchWithAbort]);

  const clearError = useCallback(() => setError(null), []);

  return {
    phase,
    token,
    error,
    login,
    setPassword,
    completeEnrollGate,
    logout,
    refreshStatus: probeStatus,
    clearError,
  };
}

// Re-export PasskeyError so consumers that hold a `UseAuth` reference
// can render passkey-specific inline errors without a second import.
// (The `loginWithPasskey` wrapper was deleted as YAGNI — Login.tsx calls
// `usePasskey().loginWithPasskey` directly. Keeping the type re-export
// because callers still import it from "../hooks/useAuth" for cohesion.)
export type { PasskeyError };
