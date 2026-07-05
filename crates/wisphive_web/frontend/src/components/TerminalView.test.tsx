import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { cleanup, render } from "@testing-library/react";
import { TerminalView } from "./TerminalView";
import type { TerminalSessionMeta } from "../types/protocol";

// jsdom has no ResizeObserver; xterm's open() path touches it via our component.
class ResizeObserverStub {
  observe() {}
  unobserve() {}
  disconnect() {}
}
globalThis.ResizeObserver = ResizeObserverStub as unknown as typeof ResizeObserver;

// xterm's CoreBrowserService probes matchMedia (device-pixel-ratio tracking),
// which jsdom does not implement — stub a stationary media query.
if (!window.matchMedia) {
  window.matchMedia = ((query: string) => ({
    matches: false,
    media: query,
    onchange: null,
    addEventListener() {},
    removeEventListener() {},
    addListener() {},
    removeListener() {},
    dispatchEvent() {
      return false;
    },
  })) as unknown as typeof window.matchMedia;
}

const session: TerminalSessionMeta = {
  id: "sess-touch-1",
  label: "touch",
  command: "bash",
  args: [],
  cwd: "/tmp",
  cols: 80,
  rows: 6,
  status: "running",
  started_at: new Date(0).toISOString(),
  group_name: undefined,
  sort_order: 0,
};

function mountView() {
  return render(
    <TerminalView
      session={session}
      replayMode={false}
      onInput={() => {}}
      onResize={() => {}}
      registerHandler={() => () => {}}
    />,
  );
}

beforeEach(() => {
  // Silence xterm's noisy renderer warnings under jsdom (no real canvas).
  vi.spyOn(console, "warn").mockImplementation(() => {});
  vi.spyOn(console, "error").mockImplementation(() => {});
});
afterEach(() => {
  cleanup();
  vi.restoreAllMocks();
});

describe("TerminalView touch scrolling (itr#445)", () => {
  // Touch-drag scrollback is handled by xterm 6's own touch Gesture (untestable
  // under jsdom — no canvas/layout; proven instead against real xterm in a
  // Playwright touch-context harness). The one thing this component owns for
  // #445 is claiming the vertical pan away from the overflow:auto ancestor
  // panes so xterm's Gesture wins it — assert that contract here.
  it("sets touch-action:none on the terminal mount so xterm owns the pan", () => {
    const { container } = mountView();
    const mount = container.querySelector<HTMLElement>("div[style*='touch-action']");
    expect(mount, "terminal mount div should exist").not.toBeNull();
    expect(mount!.style.touchAction).toBe("none");
    // The xterm viewport actually mounted (so there is something to scroll).
    expect(container.querySelector(".xterm-viewport")).not.toBeNull();
  });
});
