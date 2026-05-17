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

import { useAuthProfile } from "./useAuthProfile";

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
});
