import { afterEach, describe, expect, it, vi } from "vitest";
import { cleanup, render } from "@testing-library/react";
import { TerminalView } from "./TerminalView";
import type { TerminalSessionMeta } from "../types/protocol";

// jsdom has no ResizeObserver; xterm's open() path also touches it via our
// component. A no-op stub is enough — we assert on scroll, not on fit.
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

afterEach(cleanup);

describe("TerminalView touch-to-scroll (itr#445)", () => {
  it("translates a vertical touch-drag into a scrollback scroll", () => {
    let write!: (id: string, dir: "chunk" | "catchup" | "replay_chunk", bytes: Uint8Array) => void;
    const registerHandler = (_id: string, handler: typeof write) => {
      write = handler;
      return () => {};
    };

    const { container } = render(
      <TerminalView
        session={session}
        replayMode={false}
        onInput={() => {}}
        onResize={() => {}}
        registerHandler={registerHandler}
      />,
    );

    // Fill the buffer well past the visible rows so there is scrollback to reach.
    const lines = Array.from({ length: 200 }, (_, i) => `line ${i}\r\n`).join("");
    write(session.id, "chunk", new TextEncoder().encode(lines));

    const viewport = container.querySelector<HTMLElement>(".xterm-viewport");
    expect(viewport, "xterm viewport should mount").not.toBeNull();

    // xterm auto-scrolls to bottom on write; jsdom does no layout so give the
    // viewport a real scroll range to move within, then park it at the bottom.
    Object.defineProperty(viewport!, "scrollHeight", { value: 3000, configurable: true });
    Object.defineProperty(viewport!, "clientHeight", { value: 100, configurable: true });
    viewport!.scrollTop = 2900;

    // Drag the finger DOWN 300px → reveal earlier scrollback (scrollTop drops).
    const mount = container.querySelector<HTMLElement>("div[style*='touch-action']")!;
    mount.dispatchEvent(touch("touchstart", 500));
    const move = touch("touchmove", 800); // dy = +300
    mount.dispatchEvent(move);

    expect(viewport!.scrollTop).toBe(2600); // 2900 - 300
    expect(move.defaultPrevented).toBe(true); // page scroll suppressed

    mount.dispatchEvent(touch("touchend", 800));
  });

  it("ignores a tap (sub-threshold movement) so tap-to-focus still works", () => {
    const registerHandler = () => () => {};
    const { container } = render(
      <TerminalView
        session={session}
        replayMode={false}
        onInput={() => {}}
        onResize={() => {}}
        registerHandler={registerHandler}
      />,
    );
    const viewport = container.querySelector<HTMLElement>(".xterm-viewport")!;
    Object.defineProperty(viewport, "scrollHeight", { value: 3000, configurable: true });
    Object.defineProperty(viewport, "clientHeight", { value: 100, configurable: true });
    viewport.scrollTop = 2900;

    const mount = container.querySelector<HTMLElement>("div[style*='touch-action']")!;
    mount.dispatchEvent(touch("touchstart", 500));
    const move = touch("touchmove", 503); // dy = +3, below the 6px lock
    mount.dispatchEvent(move);

    expect(viewport.scrollTop).toBe(2900); // unchanged
    expect(move.defaultPrevented).toBe(false); // gesture left for xterm/tap
    mount.dispatchEvent(touch("touchend", 503));
  });
});

// Silence xterm's noisy renderer warnings under jsdom (no real canvas).
vi.spyOn(console, "warn").mockImplementation(() => {});
vi.spyOn(console, "error").mockImplementation(() => {});
