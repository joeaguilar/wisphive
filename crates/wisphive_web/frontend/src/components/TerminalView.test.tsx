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

let scrollToLine: ReturnType<typeof vi.spyOn>;

beforeEach(() => {
  // The handler drives xterm's public scrollToLine() with an absolute target
  // (anchor + row offset). Spy on it; mockImplementation also avoids the canvas
  // path jsdom can't run. The anchor is `term.buffer.active.viewportY`, which is
  // 0 on a fresh empty terminal, so scrollToLine is called with the bare offset.
  scrollToLine = vi.spyOn(Terminal.prototype, "scrollToLine").mockImplementation(() => {});
  // Silence xterm's noisy renderer warnings under jsdom (no real canvas).
  vi.spyOn(console, "warn").mockImplementation(() => {});
  vi.spyOn(console, "error").mockImplementation(() => {});
});
afterEach(() => {
  cleanup();
  vi.restoreAllMocks();
});

describe("TerminalView touch-to-scroll (itr#445)", () => {
  it("translates a vertical touch-drag into scrollToLine(anchor + rows)", () => {
    const { container } = mountView();
    expect(container.querySelector(".xterm-viewport"), "xterm viewport should mount").not.toBeNull();
    primeViewport(container);

    const mount = container.querySelector<HTMLElement>("div[style*='touch-action']")!;
    mount.dispatchEvent(touch("touchstart", 500));
    const move = touch("touchmove", 800); // dy = +300px, 20px/row → 15 rows
    mount.dispatchEvent(move);

    // Drag DOWN reveals earlier scrollback → smaller line index (anchor 0 → -15).
    expect(scrollToLine).toHaveBeenCalledWith(-15);
    expect(move.defaultPrevented).toBe(true); // page/pane scroll suppressed

    mount.dispatchEvent(touch("touchend", 800));
  });

  it("commands an absolute target from the anchor as the finger keeps moving", () => {
    const { container } = mountView();
    primeViewport(container);

    const mount = container.querySelector<HTMLElement>("div[style*='touch-action']")!;
    mount.dispatchEvent(touch("touchstart", 500));
    mount.dispatchEvent(touch("touchmove", 600)); // dy=100 → offset -5 → line -5
    mount.dispatchEvent(touch("touchmove", 700)); // dy=200 → offset -10 → line -10
    mount.dispatchEvent(touch("touchend", 700));

    // Absolute (anchor + offset), not an accumulated delta.
    expect(scrollToLine.mock.calls).toEqual([[-5], [-10]]);
  });

  it("tracks the finger back on reversal without accumulator drift (itr#477)", () => {
    const { container } = mountView();
    primeViewport(container);

    const mount = container.querySelector<HTMLElement>("div[style*='touch-action']")!;
    mount.dispatchEvent(touch("touchstart", 500));
    mount.dispatchEvent(touch("touchmove", 900)); // dy=400 → offset -20 → line -20
    mount.dispatchEvent(touch("touchmove", 600)); // reverse: dy=100 → offset -5 → line -5
    mount.dispatchEvent(touch("touchend", 600));

    // The reversal lands on the absolute line the finger points at (-5), not a
    // clamp-corrupted delta. With the old requested-row accumulator a boundary
    // clamp on the first move could desync this second target.
    expect(scrollToLine.mock.calls).toEqual([[-20], [-5]]);
  });

  it("ignores a tap (sub-threshold movement) so tap-to-focus still works", () => {
    const { container } = mountView();
    primeViewport(container);

    const mount = container.querySelector<HTMLElement>("div[style*='touch-action']")!;
    mount.dispatchEvent(touch("touchstart", 500));
    const move = touch("touchmove", 503); // dy = +3px, below the 6px lock
    mount.dispatchEvent(move);

    expect(scrollToLine).not.toHaveBeenCalled();
    expect(move.defaultPrevented).toBe(false); // gesture left for xterm/tap
    mount.dispatchEvent(touch("touchend", 503));
  });

  it("does not swallow a jitter that clears the lock but rounds to zero rows (itr#480)", () => {
    const { container } = mountView();
    primeViewport(container); // 20px/row

    const mount = container.querySelector<HTMLElement>("div[style*='touch-action']")!;
    mount.dispatchEvent(touch("touchstart", 500));
    // dy = +8px: past the 6px SCROLL_LOCK but round(-8/20) = 0 rows. The old
    // handler latched scrolling and preventDefault-ed here while applying no
    // scroll — a dead zone that swallowed a tap.
    const move = touch("touchmove", 508);
    mount.dispatchEvent(move);

    expect(scrollToLine).not.toHaveBeenCalled();
    expect(move.defaultPrevented).toBe(false); // tap/native gesture left intact
    mount.dispatchEvent(touch("touchend", 508)); // lift → still a focusing tap
  });

  it("sets touch-action:pinch-zoom on the mount so pinch-zoom stays available (itr#478)", () => {
    const { container } = mountView();
    const mount = container.querySelector<HTMLElement>("div[style*='touch-action']");
    expect(mount).not.toBeNull();
    expect(mount!.style.touchAction).toBe("pinch-zoom");
  });
});
