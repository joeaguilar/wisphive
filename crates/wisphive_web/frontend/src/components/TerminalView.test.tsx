import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { cleanup, render } from "@testing-library/react";
import { Terminal } from "@xterm/xterm";
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

/** Build a TouchEvent jsdom can dispatch (it lacks the Touch/TouchEvent ctors). */
function touch(type: string, clientY: number, identifier = 1): Event {
  const e = new Event(type, { bubbles: true, cancelable: true });
  const t = { identifier, clientY, clientX: 10 };
  Object.defineProperty(e, "touches", { value: type === "touchend" ? [] : [t] });
  Object.defineProperty(e, "changedTouches", { value: [t] });
  return e;
}

// Give the xterm viewport a real row height: 6 rows × 20px = 120px tall, so the
// handler computes 20px per row.
function primeViewport(container: HTMLElement) {
  const viewport = container.querySelector<HTMLElement>(".xterm-viewport")!;
  Object.defineProperty(viewport, "clientHeight", { value: 120, configurable: true });
  return viewport;
}

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

let scrollLines: ReturnType<typeof vi.spyOn>;

beforeEach(() => {
  // xterm 6 renders through a custom scrollable (not native scrollTop), so the
  // handler drives the public scrollLines() API — spy on it. mockImplementation
  // also avoids the canvas path jsdom can't run.
  scrollLines = vi.spyOn(Terminal.prototype, "scrollLines").mockImplementation(() => {});
  // Silence xterm's noisy renderer warnings under jsdom (no real canvas).
  vi.spyOn(console, "warn").mockImplementation(() => {});
  vi.spyOn(console, "error").mockImplementation(() => {});
});
afterEach(() => {
  cleanup();
  vi.restoreAllMocks();
});

describe("TerminalView touch-to-scroll (itr#445)", () => {
  it("translates a vertical touch-drag into scrollLines(rows) up into scrollback", () => {
    const { container } = mountView();
    expect(container.querySelector(".xterm-viewport"), "xterm viewport should mount").not.toBeNull();
    primeViewport(container);

    const mount = container.querySelector<HTMLElement>("div[style*='touch-action']")!;
    mount.dispatchEvent(touch("touchstart", 500));
    const move = touch("touchmove", 800); // dy = +300px, 20px/row → 15 rows
    mount.dispatchEvent(move);

    // Drag DOWN reveals earlier scrollback → scroll UP → negative scrollLines.
    expect(scrollLines).toHaveBeenCalledWith(-15);
    expect(move.defaultPrevented).toBe(true); // page/pane scroll suppressed

    mount.dispatchEvent(touch("touchend", 800));
  });

  it("emits only the incremental row delta as the finger keeps moving", () => {
    const { container } = mountView();
    primeViewport(container);

    const mount = container.querySelector<HTMLElement>("div[style*='touch-action']")!;
    mount.dispatchEvent(touch("touchstart", 500));
    mount.dispatchEvent(touch("touchmove", 600)); // dy=100 → -5 rows
    mount.dispatchEvent(touch("touchmove", 700)); // dy=200 → -10 rows, delta -5
    mount.dispatchEvent(touch("touchend", 700));

    expect(scrollLines.mock.calls).toEqual([[-5], [-5]]); // cumulative -10, no double-count
  });

  it("ignores a tap (sub-threshold movement) so tap-to-focus still works", () => {
    const { container } = mountView();
    primeViewport(container);

    const mount = container.querySelector<HTMLElement>("div[style*='touch-action']")!;
    mount.dispatchEvent(touch("touchstart", 500));
    const move = touch("touchmove", 503); // dy = +3px, below the 6px lock
    mount.dispatchEvent(move);

    expect(scrollLines).not.toHaveBeenCalled();
    expect(move.defaultPrevented).toBe(false); // gesture left for xterm/tap
    mount.dispatchEvent(touch("touchend", 503));
  });
});
