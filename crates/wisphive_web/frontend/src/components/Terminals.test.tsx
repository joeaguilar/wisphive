import { StrictMode } from "react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { cleanup, fireEvent, render, screen } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { Terminals } from "./Terminals";
import type { TerminalSessionMeta } from "../types/protocol";
import type { TerminalOutputHandler } from "../hooks/useWisphive";

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
const defaultMatchMedia = window.matchMedia;

function setMobileViewport(matches: boolean) {
  window.matchMedia = vi.fn((query: string) => ({
    matches: query === "(max-width: 900px)" && matches,
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

interface MountOverrides {
  onAttach?: (id: string) => void;
  onDetach?: (id: string) => void;
  onReplay?: (id: string, fromSeq?: number) => void;
  registerHandler?: (
    id: string,
    handler: TerminalOutputHandler,
    options?: { replayMode?: boolean },
  ) => () => void;
  strict?: boolean;
  shell?: boolean;
}

function mountTerminals(overrides: MountOverrides = {}) {
  const onAttach = overrides.onAttach ?? vi.fn();
  const onDetach = overrides.onDetach ?? vi.fn();
  const onReplay = overrides.onReplay ?? vi.fn();
  const registerHandler = overrides.registerHandler ?? vi.fn(() => () => {});
  const terminalsView = (
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
      onReplay={onReplay}
      onInput={() => {}}
      onResize={() => {}}
      onSetGroup={() => {}}
      onReorder={() => {}}
      onApprove={() => {}}
      onDeny={() => {}}
      onJumpToQueue={() => {}}
      registerHandler={registerHandler}
    />
  );
  const view = overrides.shell ? (
    <div className="app">
      <nav className="sidebar">
        <button type="button">Occluded navigation</button>
      </nav>
      <main className="content">{terminalsView}</main>
    </div>
  ) : terminalsView;
  const utils = render(overrides.strict ? <StrictMode>{view}</StrictMode> : view);
  return { ...utils, onAttach, onDetach, onReplay, registerHandler };
}

beforeEach(() => {
  // Silence xterm's noisy renderer warnings under jsdom (no real canvas).
  vi.spyOn(console, "warn").mockImplementation(() => {});
  vi.spyOn(console, "error").mockImplementation(() => {});
});
afterEach(() => {
  cleanup();
  vi.restoreAllMocks();
  window.matchMedia = defaultMatchMedia;
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

describe("Terminals mobile dialog accessibility (itr#488)", () => {
  it("makes the sub-window modal and prevents Tab reaching occluded navigation", async () => {
    setMobileViewport(true);
    const user = userEvent.setup();
    const { container } = mountTerminals({ shell: true });

    await user.click(screen.getByRole("button", { name: "Attach" }));

    const dialog = screen.getByRole("dialog", { name: "claude" });
    const occludedNavigation = screen.getByRole("navigation");
    expect(dialog).toHaveAttribute("aria-modal", "true");
    expect(occludedNavigation).toHaveAttribute("inert");

    // Browser focus navigation skips an inert subtree. jsdom does not model
    // that native behavior, so assert the platform-recognized inert boundary
    // rather than emulating a second tab-order algorithm in the test.
    expect(occludedNavigation.contains(document.activeElement)).toBe(false);
    expect(container.querySelector(".terminals-main")).toBe(dialog);
  });
});

describe("Terminals stream lifecycle (itr#375)", () => {
  it("detaches before replaying the currently-live session and marks replay routing", () => {
    const events: string[] = [];
    const onAttach = vi.fn((id: string) => events.push(`attach:${id}`));
    const onDetach = vi.fn((id: string) => events.push(`detach:${id}`));
    const onReplay = vi.fn((id: string) => events.push(`replay:${id}`));
    const registerHandler = vi.fn(
      (_id: string, _handler: TerminalOutputHandler, _options?: { replayMode?: boolean }) =>
        () => {},
    );
    mountTerminals({ onAttach, onDetach, onReplay, registerHandler });

    fireEvent.click(screen.getByRole("button", { name: "Attach" }));
    fireEvent.click(screen.getByRole("button", { name: "Replay" }));

    expect(events).toEqual([
      `attach:${session.id}`,
      `detach:${session.id}`,
      `replay:${session.id}`,
    ]);
    expect(onDetach).toHaveBeenCalledTimes(1);
    expect(registerHandler.mock.calls.at(-1)?.[2]).toEqual({ replayMode: true });
  });

  it("detaches the current live stream exactly once on StrictMode unmount", () => {
    const onDetach = vi.fn();
    const { unmount } = mountTerminals({ onDetach, strict: true });

    fireEvent.click(screen.getByRole("button", { name: "Attach" }));
    unmount();

    expect(onDetach).toHaveBeenCalledTimes(1);
    expect(onDetach).toHaveBeenCalledWith(session.id);
  });

  it("does not detach a replay-only selection on unmount", () => {
    const onDetach = vi.fn();
    const { unmount } = mountTerminals({ onDetach, strict: true });

    fireEvent.click(screen.getByRole("button", { name: "Replay" }));
    unmount();

    expect(onDetach).not.toHaveBeenCalled();
  });
});
