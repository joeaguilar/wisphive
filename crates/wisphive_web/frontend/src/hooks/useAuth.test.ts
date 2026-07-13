import { act, renderHook, waitFor } from "@testing-library/react";
import { afterEach, describe, expect, it, vi } from "vitest";
import { useAuth } from "./useAuth";

describe("useAuth", () => {
  afterEach(() => {
    localStorage.clear();
    vi.unstubAllGlobals();
  });

  it("aborts the status probe on unmount without surfacing an error state (itr#273)", async () => {
    let signal: AbortSignal | undefined;
    const fetchMock = vi.fn(
      (_path: string, init?: RequestInit) =>
        new Promise<Response>((_resolve, reject) => {
          signal = init?.signal ?? undefined;
          signal?.addEventListener("abort", () => {
            reject(new DOMException("The operation was aborted.", "AbortError"));
          });
        }),
    );
    vi.stubGlobal("fetch", fetchMock);

    const { result, unmount } = renderHook(() => useAuth());
    await waitFor(() => expect(fetchMock).toHaveBeenCalledTimes(1));

    unmount();
    expect(signal?.aborted).toBe(true);

    await act(async () => {
      await Promise.resolve();
    });
    expect(result.current.error).toBeNull();
  });

  it("keeps login error null for AbortError on unmount, but surfaces other failures", async () => {
    let loginSignal: AbortSignal | undefined;
    const fetchMock = vi.fn((path: string, init?: RequestInit) => {
      if (path === "/api/auth/status") {
        return Promise.resolve(
          new Response(JSON.stringify({ password_set: true, setup_required: false }), {
            headers: { "content-type": "application/json" },
          }),
        );
      }
      return new Promise<Response>((_resolve, reject) => {
        loginSignal = init?.signal ?? undefined;
        loginSignal?.addEventListener("abort", () => {
          reject(new DOMException("The operation was aborted.", "AbortError"));
        });
      });
    });
    vi.stubGlobal("fetch", fetchMock);

    const { result, unmount } = renderHook(() => useAuth());
    await waitFor(() => expect(result.current.phase).toBe("unauthed"));

    let login: Promise<boolean>;
    act(() => {
      login = result.current.login("password");
    });
    await waitFor(() => expect(fetchMock).toHaveBeenCalledTimes(2));
    unmount();
    expect(loginSignal?.aborted).toBe(true);

    await act(async () => {
      await Promise.resolve();
    });
    await expect(login!).resolves.toBe(false);
    expect(result.current.error).toBeNull();

    const networkFetchMock = vi
      .fn()
      .mockResolvedValueOnce(
        new Response(JSON.stringify({ password_set: true, setup_required: false }), {
          headers: { "content-type": "application/json" },
        }),
      )
      .mockRejectedValueOnce(new TypeError("network down"));
    vi.stubGlobal("fetch", networkFetchMock);

    const network = renderHook(() => useAuth());
    await waitFor(() => expect(network.result.current.phase).toBe("unauthed"));
    await act(async () => {
      await expect(network.result.current.login("password")).resolves.toBe(false);
    });
    expect(network.result.current.error).toMatchObject({ kind: "network" });
  });
});
