/**
 * useAuthProfile — Vitest coverage of the once-on-mount probe.
 *
 * The hook owns one effect (a single fetch to `/api/auth/profile`) and
 * one state transition (loaded=true once the response resolves). These
 * tests pin the contract from #312:
 *   - mounts → exactly one GET to /api/auth/profile (NOT /api/auth/status)
 *   - response is parsed into the camelCased UseAuthProfile shape
 *   - re-rendering the consumer doesn't re-probe (cache forever — the
 *     profile is frozen at daemon startup)
 *   - non-OK response / fetch reject fails closed: loaded=true,
 *     canEnrollPasskeyOnThisOrigin=false (the safe default — passkey
 *     UIs hide, password form keeps working)
 *
 * We mock `fetch` directly because api.ts::apiFetch is a thin wrapper
 * around it and we want to assert on the path + method.
 */

import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { act, renderHook, waitFor } from "@testing-library/react";

import {
  __resetAuthProfileForTesting,
  useAuthProfile,
  waitForAuthProfile,
} from "./useAuthProfile";

function makeJsonResponse(body: unknown, init?: { status?: number }): Response {
  // Construct a real Response so apiFetch's status / .json() / .text()
  // surface behave exactly as in production. Easier than hand-rolling a
  // mock object that has to track which methods were called.
  return new Response(JSON.stringify(body), {
    status: init?.status ?? 200,
    headers: { "content-type": "application/json" },
  });
}

describe("useAuthProfile", () => {
  beforeEach(() => {
    // Wipe the module-level singleton so each test sees a fresh probe
    // attempt against its own fetch mock. Without this, test N+1 would
    // observe test N's cached snapshot and pass for the wrong reason.
    __resetAuthProfileForTesting();
    // Clear any token left over from a previous test so the apiFetch
    // codepath doesn't see a stale Authorization header that could
    // trigger 401-side-effects mid-test.
    try {
      localStorage.removeItem("wisphive-web-token");
    } catch {
      // jsdom storage hiccup — non-fatal for these tests.
    }
  });

  afterEach(() => {
    vi.unstubAllGlobals();
    // Belt-and-braces: clear the singleton again so a failing test
    // can't leak state into a sibling suite that imports this module.
    __resetAuthProfileForTesting();
  });

  it("probes /api/auth/profile on mount and returns the parsed shape", async () => {
    const fetchMock = vi.fn().mockResolvedValue(
      makeJsonResponse({
        profile: "local-lan",
        can_enroll_passkey_on_this_origin: true,
        passkey_required: false,
        allow_ephemeral_listener: true,
      }),
    );
    vi.stubGlobal("fetch", fetchMock);

    const { result } = renderHook(() => useAuthProfile());
    // Initial state: not loaded yet, all flags false.
    expect(result.current.loaded).toBe(false);
    expect(result.current.profile).toBeNull();
    expect(result.current.canEnrollPasskeyOnThisOrigin).toBe(false);

    await waitFor(() => {
      expect(result.current.loaded).toBe(true);
    });

    expect(fetchMock).toHaveBeenCalledTimes(1);
    // The path must be /api/auth/profile — a typo to /api/auth/status
    // here would silently re-use the bootstrap-discovery surface and
    // skip the origin-aware bit entirely, breaking the LocalLAN +
    // LAN-IP gate. This assertion catches that regression.
    expect(fetchMock).toHaveBeenCalledWith(
      "/api/auth/profile",
      // apiFetch passes init with header merging; assert the path only,
      // not the headers, so test isn't brittle to apiFetch internals.
      expect.any(Object),
    );

    expect(result.current.profile).toBe("local-lan");
    expect(result.current.canEnrollPasskeyOnThisOrigin).toBe(true);
    expect(result.current.passkeyRequired).toBe(false);
    expect(result.current.allowEphemeralListener).toBe(true);
  });

  it("returns canEnrollPasskeyOnThisOrigin=false on a LAN-IP origin", async () => {
    // Backend returns false for the LAN-IP case (LocalLAN profile,
    // RFC1918 origin) — the SPA must surface that as-is so Login.tsx
    // can hide the enroll button.
    const fetchMock = vi.fn().mockResolvedValue(
      makeJsonResponse({
        profile: "local-lan",
        can_enroll_passkey_on_this_origin: false,
        passkey_required: false,
        allow_ephemeral_listener: true,
      }),
    );
    vi.stubGlobal("fetch", fetchMock);

    const { result } = renderHook(() => useAuthProfile());
    await waitFor(() => expect(result.current.loaded).toBe(true));
    expect(result.current.profile).toBe("local-lan");
    expect(result.current.canEnrollPasskeyOnThisOrigin).toBe(false);
  });

  it("does not re-probe on consumer re-render", async () => {
    const fetchMock = vi.fn().mockResolvedValue(
      makeJsonResponse({
        profile: "local-lan",
        can_enroll_passkey_on_this_origin: true,
        passkey_required: false,
        allow_ephemeral_listener: true,
      }),
    );
    vi.stubGlobal("fetch", fetchMock);

    const { result, rerender } = renderHook(() => useAuthProfile());
    await waitFor(() => expect(result.current.loaded).toBe(true));
    expect(fetchMock).toHaveBeenCalledTimes(1);

    // Re-render the consumer multiple times — the hook MUST NOT fire
    // a second probe. The profile doesn't change without a daemon
    // restart; re-probing would waste a request and could even race
    // a server-side cache invalidation.
    rerender();
    rerender();
    rerender();
    await act(async () => {
      await Promise.resolve();
    });
    expect(fetchMock).toHaveBeenCalledTimes(1);
  });

  it("fails closed on non-2xx response (loaded=true, no passkey UI)", async () => {
    const fetchMock = vi.fn().mockResolvedValue(
      new Response("internal error", { status: 500 }),
    );
    vi.stubGlobal("fetch", fetchMock);

    const { result } = renderHook(() => useAuthProfile());
    await waitFor(() => expect(result.current.loaded).toBe(true));
    // Defaults all collapse to "passkey UI hidden". Renders the
    // password form un-cluttered.
    expect(result.current.profile).toBeNull();
    expect(result.current.canEnrollPasskeyOnThisOrigin).toBe(false);
    expect(result.current.passkeyRequired).toBe(false);
    expect(result.current.allowEphemeralListener).toBe(false);
  });

  it("fails closed on fetch reject (network down)", async () => {
    const fetchMock = vi.fn().mockRejectedValue(new TypeError("network down"));
    vi.stubGlobal("fetch", fetchMock);

    const { result } = renderHook(() => useAuthProfile());
    await waitFor(() => expect(result.current.loaded).toBe(true));
    expect(result.current.profile).toBeNull();
    expect(result.current.canEnrollPasskeyOnThisOrigin).toBe(false);
  });

  it("singleton: two consumers share one probe (no duplicate fetches)", async () => {
    // Sprint-1 wave-4 review item #1 regression: pre-singleton, both
    // useAuth and Login.tsx called useAuthProfile() independently — each
    // creating its own useState + useEffect → two parallel
    // /api/auth/profile requests. Worse, the two snapshots could
    // disagree if the daemon flipped state mid-probe. Singleton
    // guarantees one probe per page lifetime regardless of consumer count.
    const fetchMock = vi.fn().mockResolvedValue(
      makeJsonResponse({
        profile: "local-lan",
        can_enroll_passkey_on_this_origin: true,
        passkey_required: false,
        allow_ephemeral_listener: true,
      }),
    );
    vi.stubGlobal("fetch", fetchMock);

    // Mount two independent hook instances. In real code these would be
    // useAuth and Login.tsx; here they're two renderHook calls. Both
    // must see the same snapshot AND only ONE HTTP request fires.
    const a = renderHook(() => useAuthProfile());
    const b = renderHook(() => useAuthProfile());
    await waitFor(() => {
      expect(a.result.current.loaded).toBe(true);
      expect(b.result.current.loaded).toBe(true);
    });
    expect(fetchMock).toHaveBeenCalledTimes(1);
    expect(a.result.current.canEnrollPasskeyOnThisOrigin).toBe(true);
    expect(b.result.current.canEnrollPasskeyOnThisOrigin).toBe(true);
  });

  it("waitForAuthProfile: resolves to the singleton snapshot", async () => {
    // Async barrier callers (useAuth.setPassword is the canonical
    // example) must be able to read the resolved posture without
    // mounting a hook. Test that waitForAuthProfile returns the same
    // shape the hook does.
    const fetchMock = vi.fn().mockResolvedValue(
      makeJsonResponse({
        profile: "local-lan",
        can_enroll_passkey_on_this_origin: true,
        passkey_required: false,
        allow_ephemeral_listener: true,
      }),
    );
    vi.stubGlobal("fetch", fetchMock);

    const snapshot = await waitForAuthProfile();
    expect(snapshot.loaded).toBe(true);
    expect(snapshot.canEnrollPasskeyOnThisOrigin).toBe(true);
    expect(snapshot.profile).toBe("local-lan");
    expect(fetchMock).toHaveBeenCalledTimes(1);

    // A second waitForAuthProfile must NOT re-probe — it returns the
    // cached success.
    const second = await waitForAuthProfile();
    expect(second).toBe(snapshot);
    expect(fetchMock).toHaveBeenCalledTimes(1);
  });

  it("waitForAuthProfile: retries past a cached fail-closed snapshot (review item #1)", async () => {
    // The load-bearing fix for the manual-smoke regression: a single
    // transient probe failure must NOT permanently doom every
    // subsequent setPassword call. waitForAuthProfile kicks a fresh
    // probe even when the cache is a fail-closed snapshot — that way
    // the user clicking "Set password" gets a real attempt at the
    // network and the enroll card appears when the daemon comes back.
    const fetchMock = vi
      .fn()
      // Attempt 1: network down → fail-closed cached
      .mockRejectedValueOnce(new TypeError("network down"))
      // Attempt 2: succeeds
      .mockResolvedValueOnce(
        makeJsonResponse({
          profile: "local-lan",
          can_enroll_passkey_on_this_origin: true,
          passkey_required: false,
          allow_ephemeral_listener: true,
        }),
      );
    vi.stubGlobal("fetch", fetchMock);

    const first = await waitForAuthProfile();
    expect(first.loaded).toBe(true);
    expect(first.canEnrollPasskeyOnThisOrigin).toBe(false); // fail-closed
    expect(fetchMock).toHaveBeenCalledTimes(1);

    // Retry kicks a fresh probe — this time it succeeds and we get the
    // real posture. setPassword can now route to authed-pending-enroll.
    const second = await waitForAuthProfile();
    expect(second.canEnrollPasskeyOnThisOrigin).toBe(true);
    expect(second.profile).toBe("local-lan");
    expect(fetchMock).toHaveBeenCalledTimes(2);

    // And a THIRD call after a success must NOT re-probe — the cache
    // is now a successful snapshot, so further awaits return it.
    const third = await waitForAuthProfile();
    expect(third).toBe(second);
    expect(fetchMock).toHaveBeenCalledTimes(2);
  });

  it("waitForAuthProfile: concurrent callers share the in-flight probe", async () => {
    // Two callers racing waitForAuthProfile() at the same instant must
    // produce ONE network request, not two. Otherwise a fast user click
    // racing the hook's mount-time probe could double-fetch.
    let resolveFetch!: (r: Response) => void;
    const fetchMock = vi.fn().mockImplementation(
      () =>
        new Promise<Response>((res) => {
          resolveFetch = res;
        }),
    );
    vi.stubGlobal("fetch", fetchMock);

    const a = waitForAuthProfile();
    const b = waitForAuthProfile();
    expect(fetchMock).toHaveBeenCalledTimes(1);

    resolveFetch(
      makeJsonResponse({
        profile: "local-lan",
        can_enroll_passkey_on_this_origin: true,
        passkey_required: false,
        allow_ephemeral_listener: true,
      }),
    );

    const [snapA, snapB] = await Promise.all([a, b]);
    expect(snapA).toBe(snapB);
    expect(snapA.canEnrollPasskeyOnThisOrigin).toBe(true);
  });

  it("singleton notifies hook subscribers when waitForAuthProfile resolves the probe", async () => {
    // Scenario: hook mounts, kicks the probe via useEffect.
    // waitForAuthProfile (called from useAuth.setPassword) is racing
    // the hook's first render. When it resolves, the hook subscriber
    // must fire and the consumer re-renders with the new snapshot.
    const fetchMock = vi.fn().mockResolvedValue(
      makeJsonResponse({
        profile: "local-lan",
        can_enroll_passkey_on_this_origin: true,
        passkey_required: false,
        allow_ephemeral_listener: true,
      }),
    );
    vi.stubGlobal("fetch", fetchMock);

    const { result } = renderHook(() => useAuthProfile());
    // Race condition simulated: kick waitForAuthProfile from "outside"
    // the React tree (i.e. as if useAuth.setPassword called it). The
    // hook's useEffect call to waitForAuthProfile is idempotent.
    await waitForAuthProfile();
    // Hook must re-render with the resolved snapshot, not stuck at
    // UNLOADED. Verified via the loaded flag flipping true.
    await waitFor(() => {
      expect(result.current.loaded).toBe(true);
      expect(result.current.canEnrollPasskeyOnThisOrigin).toBe(true);
    });
    expect(fetchMock).toHaveBeenCalledTimes(1);
  });
});
