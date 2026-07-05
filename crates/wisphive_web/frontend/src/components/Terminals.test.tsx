import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { cleanup, fireEvent, render, screen } from "@testing-library/react";
import { Terminals } from "./Terminals";
import type { TerminalSessionMeta } from "../types/protocol";

// jsdom has no ResizeObserver; xterm's open() path touches it via TerminalView.
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
  id: "sess-two-step-1",
  label: "claude",
  command: "claude",
  args: [],
  cwd: "/tmp/project",
  cols: 80,
  rows: 24,
  status: "running",
  started_at: new Date(0).toISOString(),
  group_name: undefined,
  sort_order: 0,
};

function mountTerminals(overrides: { onAttach?: () => void; onDetach?: (id: string) => void } = {}) {
  const onAttach = overrides.onAttach ?? vi.fn();
  const onDetach = overrides.onDetach ?? vi.fn();
  const utils = render(
    <Terminals
      terminals={[session]}
      queue={[]}
      projects={[]}
      onRefresh={() => {}}
      onRefreshProjects={() => {}}
      onCreate={() => {}}
      onAttach={onAttach}
      onDetach={onDetach}
      onClose={() => {}}
      onReplay={() => {}}
      onInput={() => {}}
      onResize={() => {}}
      onSetGroup={() => {}}
      onReorder={() => {}}
      onApprove={() => {}}
      onDeny={() => {}}
      onJumpToQueue={() => {}}
      registerHandler={() => () => {}}
    />,
  );
  return { ...utils, onAttach, onDetach };
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

describe("Terminals two-step mobile workflow (itr#487)", () => {
  it("starts list-only: no terminal-open state class, no sub-window header", () => {
    const { container } = mountTerminals();
    const layout = container.querySelector(".terminals-layout")!;
    expect(layout.classList.contains("terminal-open")).toBe(false);
    expect(container.querySelector(".terminals-mobile-header")).toBeNull();
  });

  it("selecting a session sets terminal-open and renders the sub-window header", () => {
    const { container, onAttach } = mountTerminals();
    fireEvent.click(container.querySelector(".terminals-sidebar-item")!);

    const layout = container.querySelector(".terminals-layout")!;
    expect(layout.classList.contains("terminal-open")).toBe(true);
    expect(onAttach).toHaveBeenCalledWith(session.id);

    // The header carries the way back plus the session identity (full label —
    // the sub-window is the only surface on mobile, so nothing is truncated
    // structurally; CSS ellipsis only guards pathological widths).
    expect(screen.getByRole("button", { name: "Terminals — back to session list" })).toBeTruthy();
    expect(container.querySelector(".terminals-mobile-title")!.textContent).toBe("claude");
  });

  it("back returns to the list: clears terminal-open, detaches, and restores focus", () => {
    const { container, onDetach } = mountTerminals();
    // Open via the item's Attach button with real focus on it, the way a
    // keyboard user would — back must hand focus to that button, not <body>.
    const attach = screen.getByRole("button", { name: "Attach" });
    attach.focus();
    fireEvent.click(attach);
    fireEvent.click(screen.getByRole("button", { name: "Terminals — back to session list" }));

    const layout = container.querySelector(".terminals-layout")!;
    expect(layout.classList.contains("terminal-open")).toBe(false);
    expect(container.querySelector(".terminals-mobile-header")).toBeNull();
    expect(onDetach).toHaveBeenCalledWith(session.id);
    expect(document.activeElement).toBe(attach);
  });
});
