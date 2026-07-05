import { afterEach, describe, expect, it, vi } from "vitest";
import { act, cleanup, renderHook } from "@testing-library/react";
import { isMobileViewport, useIsMobile, MOBILE_BREAKPOINT_PX } from "./useViewport";

// Controllable matchMedia stub: tests flip `matches` and fire the change
// listeners the way a real viewport resize across the breakpoint would.
function installMatchMedia(initialMatches: boolean) {
  let matches = initialMatches;
  let listeners: Array<() => void> = [];
  window.matchMedia = ((query: string) => ({
    get matches() {
      return matches;
    },
    media: query,
    onchange: null,
    addEventListener(_: string, fn: () => void) {
      listeners.push(fn);
    },
    removeEventListener(_: string, fn: () => void) {
      listeners = listeners.filter((l) => l !== fn);
    },
    addListener() {},
    removeListener() {},
    dispatchEvent() {
      return false;
    },
  })) as unknown as typeof window.matchMedia;
  return {
    setMatches(next: boolean) {
      matches = next;
      for (const fn of [...listeners]) fn();
    },
  };
}

afterEach(() => {
  cleanup();
  vi.restoreAllMocks();
});

describe("useViewport (itr#487)", () => {
  it("isMobileViewport reflects the matchMedia result and queries the shared breakpoint", () => {
    const spy = vi.fn().mockReturnValue({ matches: true });
    window.matchMedia = spy as unknown as typeof window.matchMedia;
    expect(isMobileViewport()).toBe(true);
    expect(spy).toHaveBeenCalledWith(`(max-width: ${MOBILE_BREAKPOINT_PX}px)`);
  });

  it("useIsMobile tracks breakpoint changes and mirrors them onto :root", () => {
    const media = installMatchMedia(false);
    const { result } = renderHook(() => useIsMobile());
    expect(result.current).toBe(false);

    act(() => media.setMatches(true));
    expect(result.current).toBe(true);
    expect(document.documentElement.style.getPropertyValue("--is-mobile")).toBe("1");
    expect(document.documentElement.dataset.viewport).toBe("mobile");

    act(() => media.setMatches(false));
    expect(result.current).toBe(false);
    expect(document.documentElement.style.getPropertyValue("--is-mobile")).toBe("0");
    expect(document.documentElement.dataset.viewport).toBe("desktop");
  });
});
