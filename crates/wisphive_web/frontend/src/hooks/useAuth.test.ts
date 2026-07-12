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
});
