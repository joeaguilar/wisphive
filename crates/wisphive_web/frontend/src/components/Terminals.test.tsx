import { createRef, StrictMode } from "react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { act, cleanup, fireEvent, render, screen } from "@testing-library/react";
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
  stackedModal?: boolean;
}

function mountTerminals(overrides: MountOverrides = {}) {
  const onAttach = overrides.onAttach ?? vi.fn();
  const onDetach = overrides.onDetach ?? vi.fn();
  const onReplay = overrides.onReplay ?? vi.fn();
  const registerHandler = overrides.registerHandler ?? vi.fn(() => () => {});
  const backgroundRef = createRef<HTMLElement>();
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
      backgroundRef={backgroundRef}
      registerHandler={registerHandler}
    />
  );
  const view = overrides.shell ? (
    <div className="app">
      <nav ref={backgroundRef} className="sidebar">
        <button type="button">Occluded navigation</button>
      </nav>
      <main className="content">{terminalsView}</main>
      {overrides.stackedModal && (
        <div role="dialog" aria-label="Re-authenticate">
          <input aria-label="Password" type="password" />
        </div>
      )}
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

    // The ref connects the actual background shell to the inert boundary.
    expect(occludedNavigation.contains(document.activeElement)).toBe(false);
    expect(container.querySelector(".terminals-main")).toBe(dialog);
  });

  it("traps Tab inside the dialog when native inert behavior is unavailable", async () => {
    setMobileViewport(true);
    const user = userEvent.setup();
    mountTerminals({ shell: true });

    await user.click(screen.getByRole("button", { name: "Attach" }));

    const dialog = screen.getByRole("dialog", { name: "claude" });
    const occludedNavigation = screen.getByRole("navigation");
    const backgroundButton = screen.getByRole("button", { name: "Occluded navigation" });

    // jsdom does not implement inert focus suppression, which models the
    // browser gap the fallback must cover.
    backgroundButton.focus();
    expect(document.activeElement).toBe(backgroundButton);

    expect(fireEvent.keyDown(document, { key: "Tab" })).toBe(false);
    expect(dialog.contains(document.activeElement)).toBe(true);
    expect(occludedNavigation.contains(document.activeElement)).toBe(false);

    expect(fireEvent.keyDown(document, { key: "Tab", shiftKey: true })).toBe(false);
    expect(dialog.contains(document.activeElement)).toBe(true);
  });

  it("does not steal Tab focus from a modal stacked above the terminal dialog", async () => {
    setMobileViewport(true);
    const user = userEvent.setup();
    mountTerminals({ shell: true, stackedModal: true });

    await user.click(screen.getByRole("button", { name: "Attach" }));

    const terminalDialog = screen.getByRole("dialog", { name: "claude" });
    const stackedDialog = screen.getByRole("dialog", { name: "Re-authenticate" });
    const password = screen.getByLabelText("Password");
    password.focus();

    expect(terminalDialog.contains(password)).toBe(false);
    expect(screen.getByRole("navigation").contains(password)).toBe(false);
    expect(fireEvent.keyDown(document, { key: "Tab" })).toBe(true);
    expect(stackedDialog.contains(document.activeElement)).toBe(true);
    expect(document.activeElement).toBe(password);
  });

  it("wraps Tab at the dialog's real first and last focusable edges", async () => {
    setMobileViewport(true);
    const user = userEvent.setup();
    mountTerminals({ shell: true });

    await user.click(screen.getByRole("button", { name: "Attach" }));

    const dialog = screen.getByRole("dialog", { name: "claude" });
    const focusables = [...dialog.querySelectorAll<HTMLElement>(
      'a[href], button:not([disabled]), textarea:not([disabled]), input:not([disabled]), select:not([disabled]), [tabindex]:not([tabindex="-1"])',
    )];
    expect(focusables.length).toBeGreaterThan(1);

    // jsdom reports offsetParent === null universally. Give the dialog's
    // actual controls a layout parent so the production visibility filter
    // exercises its first/last ordering instead of the empty-list fallback.
    for (const element of focusables) {
      Object.defineProperty(element, "offsetParent", {
        configurable: true,
        value: dialog,
      });
    }

    const first = focusables[0];
    const last = focusables.at(-1)!;
    last.focus();
    expect(fireEvent.keyDown(document, { key: "Tab" })).toBe(false);
    expect(document.activeElement).toBe(first);

    first.focus();
    expect(fireEvent.keyDown(document, { key: "Tab", shiftKey: true })).toBe(false);
    expect(document.activeElement).toBe(last);
  });
});

describe("Terminals deferred deep-link focus (itr#437 / itr#449)", () => {
  interface FocusMountOptions {
    terminals?: TerminalSessionMeta[];
    focusSessionId?: string;
  }

  function mountWithFocus(options: FocusMountOptions = {}) {
    const onAttach = vi.fn();
    const onFocusHandled = vi.fn();
    const backgroundRef = createRef<HTMLElement>();
    const build = (terminals: TerminalSessionMeta[], focusSessionId?: string) => (
      <Terminals
        terminals={terminals}
        queue={[]}
        projects={[]}
        onRefresh={() => {}}
        onRefreshProjects={() => {}}
        onCreate={() => {}}
        onAttach={onAttach}
        onDetach={() => {}}
        onClose={() => {}}
        onReplay={() => {}}
        onInput={() => {}}
        onResize={() => {}}
        onSetGroup={() => {}}
        onReorder={() => {}}
        onApprove={() => {}}
        onDeny={() => {}}
        onJumpToQueue={() => {}}
        focusSessionId={focusSessionId}
        onFocusHandled={onFocusHandled}
        backgroundRef={backgroundRef}
        registerHandler={vi.fn(() => () => {})}
      />
    );
    const utils = render(build(options.terminals ?? [session], options.focusSessionId));
    return {
      ...utils,
      onAttach,
      onFocusHandled,
      rerenderWith: (terminals: TerminalSessionMeta[], focusSessionId?: string) =>
        utils.rerender(build(terminals, focusSessionId)),
    };
  }

  beforeEach(() => {
    vi.useFakeTimers();
  });
  afterEach(() => {
    vi.useRealTimers();
  });

  it("focuses a live target immediately: attaches, acks, and shows no notice", () => {
    const { container, onAttach, onFocusHandled } = mountWithFocus({
      focusSessionId: session.id,
    });

    expect(onAttach).toHaveBeenCalledWith(session.id);
    expect(onFocusHandled).toHaveBeenCalledTimes(1);
    expect(container.querySelector(".terminals-focus-notice")).toBeNull();
  });

  it("surfaces a notice and still acks when the target never appears (itr#449)", () => {
    const { container, onAttach, onFocusHandled } = mountWithFocus({
      focusSessionId: "gone-session-1",
    });

    // Inside the bounded wait the miss is not yet a verdict: term_list may
    // still be in flight, so no notice and no ack yet.
    expect(container.querySelector(".terminals-focus-notice")).toBeNull();
    expect(onFocusHandled).not.toHaveBeenCalled();

    act(() => {
      vi.advanceTimersByTime(2_000);
    });

    // Stale verdict: visible feedback instead of a silent no-op, and the
    // pending focus is acked so the inbox deep-link state can't wedge.
    const notice = container.querySelector(".terminals-focus-notice");
    expect(notice).not.toBeNull();
    expect(notice!.textContent).toContain("gone-ses");
    expect(onFocusHandled).toHaveBeenCalledTimes(1);
    expect(onAttach).not.toHaveBeenCalled();

    // The notice announces itself to assistive tech and is dismissible.
    expect(screen.getByRole("status").textContent).toContain("gone-ses");
    fireEvent.click(screen.getByRole("button", { name: "Dismiss terminal focus notice" }));
    expect(container.querySelector(".terminals-focus-notice")).toBeNull();
  });

  it("still honours a target that arrives late within the wait window", () => {
    const late: TerminalSessionMeta = { ...session, id: "late-arrival-1" };
    const { container, onAttach, onFocusHandled, rerenderWith } = mountWithFocus({
      terminals: [session],
      focusSessionId: late.id,
    });

    act(() => {
      vi.advanceTimersByTime(1_000);
    });
    expect(onFocusHandled).not.toHaveBeenCalled();

    // term_list delivers the target before the deadline.
    rerenderWith([session, late], late.id);

    expect(onAttach).toHaveBeenCalledWith(late.id);
    expect(onFocusHandled).toHaveBeenCalledTimes(1);
    expect(container.querySelector(".terminals-focus-notice")).toBeNull();

    // The abandoned deadline never fires afterwards.
    act(() => {
      vi.advanceTimersByTime(5_000);
    });
    expect(container.querySelector(".terminals-focus-notice")).toBeNull();
  });

  it("keeps the deadline anchored across unrelated terminal-list updates", () => {
    const other: TerminalSessionMeta = { ...session, id: "other-terminal-1" };
    const { container, onFocusHandled, rerenderWith } = mountWithFocus({
      terminals: [session],
      focusSessionId: "gone-session-2",
    });

    // Unrelated list churn re-runs the focus effect; the deadline must not
    // be extended past its original anchor by each update.
    act(() => {
      vi.advanceTimersByTime(1_500);
    });
    rerenderWith([session, other], "gone-session-2");
    act(() => {
      vi.advanceTimersByTime(600);
    });

    expect(container.querySelector(".terminals-focus-notice")).not.toBeNull();
    expect(onFocusHandled).toHaveBeenCalledTimes(1);
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
