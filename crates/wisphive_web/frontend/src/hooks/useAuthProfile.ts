/**
 * useAuthProfile — origin-aware probe of `GET /api/auth/profile`.
 *
 * Per the LOCKED design (itr#312 / itr#310): the frontend learns the
 * active auth posture from a dedicated endpoint (NOT a Vite env var, NOT
 * piggybacked on /api/auth/status) because `can_enroll_passkey_on_this_origin`
 * is computed server-side from the request's `Origin` header. A LAN-IP
 * origin under LocalLAN returns `false` here so the SPA can hide the
 * "enroll passkey" affordance — WebAuthn forbids IP literals as RP IDs
 * and silently failing the WebAuthn call would be a worse UX than just
 * never offering it.
 *
 * Probe semantics:
 * - **Module-level singleton.** The entire app shares ONE probe per page
 *   lifetime. Pre-refactor, every component that called `useAuthProfile`
 *   spawned its own probe — both `useAuth` and `Login.tsx` did, so they
 *   raced and could disagree if the daemon flipped state mid-probe.
 *   Worse: `setPassword` consulted its own `useAuthProfile()` instance
 *   whose default was `loaded=false`, meaning a fast form submit could
 *   set the password before the probe resolved and skip the
 *   `authed-pending-enroll` transition. Single source of truth removes
 *   the race; the wave-4 manual smoke caught this.
 * - **Successes are cached forever.** Profile is frozen at daemon startup;
 *   re-probing on every render or every auth-change burns a request for
 *   no information gain.
 * - **Failures are NOT cached.** A transient probe failure (network blip,
 *   slow daemon, etc.) leaves the singleton in "needs retry" state; the
 *   next consumer that mounts (or the next `waitForAuthProfile()` caller)
 *   re-kicks the fetch. Prevents the "first probe died, no passkey ever"
 *   class of failure the reviewer flagged.
 * - The hook does NOT set a custom `Origin` header (it can't — browsers
 *   forbid scripted overrides of `Origin` for security reasons). On a
 *   same-origin GET like this one browsers actually OMIT `Origin`
 *   entirely (per the Fetch standard), so the backend cannot rely on it
 *   alone. The server-side handler reads `Origin` first and falls back
 *   to the request URI authority (HTTP/2) or `Host` header (HTTP/1.1)
 *   to compute `can_enroll_passkey_on_this_origin` — see
 *   `lib.rs::origin_can_enroll_passkey`. That fallback was added in
 *   sprint-1 wave-4 after the first browser smoke surfaced the gap.
 * - On network failure / non-2xx, the public hook return value collapses
 *   to "no passkey, no enroll button" (all booleans `false`, `loaded=true`).
 *   That's the safe, non-leaky default for a UI render — falling back to
 *   "passkey enabled" on a transient failure would offer a button that
 *   400s when clicked.
 *
 * Async API:
 * - `useAuthProfile()` — React hook for rendering decisions. Always
 *   returns the latest snapshot; re-renders on probe resolve.
 * - `waitForAuthProfile()` — async function for code paths that MUST
 *   know the resolved posture before deciding (the `setPassword` →
 *   "authed-pending-enroll" branch is the canonical caller). Awaits
 *   the in-flight probe or kicks off a fresh one if no cache + no
 *   in-flight; resolves to the fully-loaded snapshot.
 */

import { useEffect, useState } from "react";
import { apiFetch } from "../api";

/** Stable string tag for the active profile — mirrors the backend's
 * `AuthPolicy::profile_str()` output. Add new tags as new profiles ship
 * (itr#310 ships `local-lan` and `enterprise`). */
export type AuthProfileTag = "local-lan" | "enterprise";

/** Raw `/api/auth/profile` response shape, exactly as the Rust
 * `get_auth_profile` handler emits it. Kept as an internal type so the
 * camelCased `UseAuthProfile` result is what callers consume. */
interface AuthProfileResponse {
  profile: AuthProfileTag;
  can_enroll_passkey_on_this_origin: boolean;
  passkey_required: boolean;
  allow_ephemeral_listener: boolean;
}

export interface UseAuthProfile {
  /** Stable profile tag, or `null` if the probe hasn't loaded / failed. */
  profile: AuthProfileTag | null;
  /** Whether this origin can host passkey ceremonies. The load-bearing
   * bit for both the enroll step and the login-with-passkey button —
   * gates BOTH UIs uniformly per the locked spec. */
  canEnrollPasskeyOnThisOrigin: boolean;
  /** Whether passkey login is *required* (vs allowed-as-convenience).
   * v1 keeps both profiles at `false`. Plumbed so a future Enterprise
   * "passkey-or-bust" posture lights up automatically. */
  passkeyRequired: boolean;
  /** Whether the daemon may bind an ephemeral LAN pairing listener.
   * Not consumed by Login.tsx in this PR but exposed because callers
   * outside Login.tsx (#220, future Devices UI) will want it. */
  allowEphemeralListener: boolean;
  /** True once the probe has resolved (success or failure). Login.tsx
   * uses this to avoid rendering the passkey UI in the in-flight
   * window — otherwise the button would flicker in on slow connections.
   */
  loaded: boolean;
}

const FAIL_CLOSED_SNAPSHOT: UseAuthProfile = {
  profile: null,
  canEnrollPasskeyOnThisOrigin: false,
  passkeyRequired: false,
  allowEphemeralListener: false,
  loaded: true,
};

const UNLOADED_SNAPSHOT: UseAuthProfile = {
  ...FAIL_CLOSED_SNAPSHOT,
  loaded: false,
};

/** Module-level singleton state. There is exactly one of these per page
 * lifetime — both `useAuth` and any component (e.g. `Login.tsx`) that
 * calls `useAuthProfile()` see the same snapshot, and only one network
 * round-trip happens regardless of mount order or remount churn. */
let cachedSnapshot: UseAuthProfile | null = null;
let inflight: Promise<UseAuthProfile> | null = null;
const subscribers = new Set<() => void>();

function notifySubscribers(): void {
  // Snapshot the set before iterating: a listener that re-mounts mid-
  // notify would mutate the live set otherwise.
  for (const cb of Array.from(subscribers)) {
    try {
      cb();
    } catch (e) {
      console.warn("auth profile subscriber threw:", e);
    }
  }
}

async function fetchProfile(): Promise<UseAuthProfile> {
  try {
    const res = await apiFetch("/api/auth/profile");
    if (!res.ok) {
      // Non-2xx — the endpoint is unauthenticated so 401/403 here would
      // be a daemon bug. Surface as "fail closed, but loaded" and don't
      // cache; a later caller can retry.
      return FAIL_CLOSED_SNAPSHOT;
    }
    const body = (await res.json()) as AuthProfileResponse;
    return {
      profile: body.profile,
      canEnrollPasskeyOnThisOrigin: !!body.can_enroll_passkey_on_this_origin,
      passkeyRequired: !!body.passkey_required,
      allowEphemeralListener: !!body.allow_ephemeral_listener,
      loaded: true,
    };
  } catch {
    return FAIL_CLOSED_SNAPSHOT;
  }
}

/**
 * Wait for the auth-profile probe to resolve. Returns the cached snapshot
 * if a previous probe succeeded; otherwise awaits the in-flight probe;
 * otherwise kicks off a fresh fetch.
 *
 * Cached-failure semantics: a fail-closed snapshot (probe non-2xx or
 * fetch reject) IS stored in the cache so hook consumers can render
 * `loaded=true` and show a stable UI. But `waitForAuthProfile()` treats
 * that cache entry as "no successful answer yet" — every async-barrier
 * caller gets a fresh attempt. A single transient probe failure
 * therefore does not doom the rest of the session: the next caller
 * (typically the user clicking "Set password") kicks a new probe and
 * succeeds in the common case. The hook still re-renders to the new
 * snapshot via the subscribers callback once the retry resolves.
 *
 * Code paths that MUST know `canEnrollPasskeyOnThisOrigin` before
 * deciding (e.g. `useAuth.setPassword` choosing between `"authed"` and
 * `"authed-pending-enroll"`) should `await waitForAuthProfile()` rather
 * than reading the hook's render-time `loaded` flag — the hook return
 * may not have flipped to `loaded=true` yet at the moment the user
 * clicks submit.
 */
export async function waitForAuthProfile(): Promise<UseAuthProfile> {
  if (cachedSnapshot && cachedSnapshot.profile !== null) {
    // Successful resolution cached — return without touching the network.
    return cachedSnapshot;
  }
  if (inflight) {
    // Probe in flight (initial mount or a prior retry) — share it.
    return inflight;
  }
  // No cache or cached failure — kick a fresh probe. The result, whether
  // success or fail-closed, lands in the cache so hook consumers can
  // render a stable `loaded=true` UI. A subsequent `waitForAuthProfile()`
  // caller will re-kick if this attempt also failed.
  const promise = fetchProfile().then((snapshot) => {
    cachedSnapshot = snapshot;
    inflight = null;
    notifySubscribers();
    return snapshot;
  });
  inflight = promise;
  return promise;
}

/**
 * Probe `/api/auth/profile` once (per page) and return the resolved
 * posture. Multiple `useAuthProfile()` callers share the same probe
 * and the same snapshot — see module docstring.
 *
 * Cleanup is per-component (each hook instance unsubscribes on
 * unmount); the underlying probe runs to completion regardless because
 * other consumers may still be mounted.
 */
export function useAuthProfile(): UseAuthProfile {
  // Render-time snapshot is the singleton if cached, else the unloaded
  // default. We don't store the snapshot in `useState` because that would
  // duplicate the source of truth — instead we use a "tick" counter to
  // force re-renders when the singleton notifies us.
  const [, setTick] = useState(0);

  useEffect(() => {
    // Subscribe so we re-render when the singleton publishes a new
    // snapshot (initial resolve, or a retry that succeeded).
    const cb = () => setTick((t) => t + 1);
    subscribers.add(cb);
    // Kick the probe if it hasn't started yet. `waitForAuthProfile`
    // is idempotent — if a probe is already in flight or cached, this
    // is a no-op.
    void waitForAuthProfile();
    return () => {
      subscribers.delete(cb);
    };
  }, []);

  return cachedSnapshot ?? UNLOADED_SNAPSHOT;
}

/**
 * Test-only: wipe the module-level singleton between test cases.
 *
 * Vitest runs tests in the same process, so without an explicit reset
 * each test sees whatever the previous test's probe resolved to. Test
 * setup should call this in `beforeEach` (or rely on `restoreMocks` +
 * call this once after stubbing fetch).
 *
 * Production code MUST NOT call this — there's no "refresh the profile"
 * use case in v1, and re-probing mid-session would silently mask a
 * stale-cache bug rather than fix one. If a future feature genuinely
 * needs a refresh API, add a different function with explicit semantics
 * (e.g. `refreshAuthProfile()` that returns a Promise and bumps the
 * subscriber tick) rather than overloading this test hook.
 */
export function __resetAuthProfileForTesting(): void {
  cachedSnapshot = null;
  inflight = null;
  subscribers.clear();
}
