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
 * - Fires exactly ONCE on mount. The profile is frozen at daemon startup;
 *   re-probing on every render or every auth-change burns a request for
 *   no information gain.
 * - The `fetch` defaults already send the page's `Origin` header
 *   automatically (browsers attach it to cross-origin AND same-origin
 *   POSTs/GETs the same way), so the server's response is meaningful
 *   for *this* page load.
 * - On network failure / non-2xx, returns `loaded=true` with
 *   `profile=null` + all booleans `false`. That collapses to "no
 *   passkey, no enroll button" — the safe, non-leaky default that
 *   matches the LocalLAN-on-LAN-IP case the gate exists to handle in
 *   the first place. Falling back to "passkey enabled" on a transient
 *   failure would offer a button that 400s when clicked.
 *
 * Not exported and intentionally simple: this hook owns no UI, no
 * caching layer, no refresh API. The whole point is "ask once at the
 * top of the app and never again."
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

/**
 * Probe `/api/auth/profile` once and return the resolved posture.
 *
 * The hook owns its in-flight effect via the standard "mounted" flag —
 * a fast unmount (e.g. user navigates away during the probe) must not
 * trigger a "setState on unmounted component" warning under React 19's
 * stricter dev-mode checks.
 */
export function useAuthProfile(): UseAuthProfile {
  const [state, setState] = useState<UseAuthProfile>({
    profile: null,
    canEnrollPasskeyOnThisOrigin: false,
    passkeyRequired: false,
    allowEphemeralListener: false,
    loaded: false,
  });

  useEffect(() => {
    let mounted = true;
    // External-system probe — same pattern as useAuth's status probe.
    // Wrapped in an IIFE so the effect cleanup signature stays the
    // synchronous `() => void` React requires. The lint rule is OK
    // with this shape because the setState happens inside an async
    // callback (the "subscribe" branch of react-hooks/set-state-in-effect).
    void (async () => {
      try {
        const res = await apiFetch("/api/auth/profile");
        if (!mounted) return;
        if (!res.ok) {
          // Non-2xx (probably the daemon-misconfigured branch — the
          // endpoint is unauthenticated so 401/403 here would be a
          // bug). Fail closed: no passkey UI rendered.
          setState((s) => ({ ...s, loaded: true }));
          return;
        }
        const body = (await res.json()) as AuthProfileResponse;
        if (!mounted) return;
        setState({
          profile: body.profile,
          canEnrollPasskeyOnThisOrigin: !!body.can_enroll_passkey_on_this_origin,
          passkeyRequired: !!body.passkey_required,
          allowEphemeralListener: !!body.allow_ephemeral_listener,
          loaded: true,
        });
      } catch {
        // Network error — same "fail closed, surface as loaded" path.
        // A retry button on Login.tsx wouldn't help here (no transient
        // condition the user can fix); the password form keeps working
        // because it doesn't depend on this probe.
        if (!mounted) return;
        setState((s) => ({ ...s, loaded: true }));
      }
    })();
    return () => {
      mounted = false;
    };
    // Empty dep array — "probe once on mount, never again" is the
    // entire contract of this hook. See module docstring.
  }, []);

  return state;
}
