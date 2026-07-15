import { cleanup, renderHook } from "@testing-library/react";
import { afterEach, describe, expect, it, vi } from "vitest";
import { useKeyboard } from "./useKeyboard";

afterEach(() => {
  cleanup();
  vi.restoreAllMocks();
});

describe("useKeyboard", () => {
  it("invokes onViewTerminals when 7 is pressed", () => {
    const onViewTerminals = vi.fn();

    renderHook(() => useKeyboard({ onViewTerminals }));

    window.dispatchEvent(new KeyboardEvent("keydown", { key: "7" }));

    expect(onViewTerminals).toHaveBeenCalledOnce();
  });

  it("invokes onViewBoard when 8 is pressed", () => {
    const onViewBoard = vi.fn();

    renderHook(() => useKeyboard({ onViewBoard }));

    window.dispatchEvent(new KeyboardEvent("keydown", { key: "8" }));

    expect(onViewBoard).toHaveBeenCalledOnce();
  });

  it("invokes onViewWorktrees when 9 is pressed", () => {
    const onViewWorktrees = vi.fn();

    renderHook(() => useKeyboard({ onViewWorktrees }));

    window.dispatchEvent(new KeyboardEvent("keydown", { key: "9" }));

    expect(onViewWorktrees).toHaveBeenCalledOnce();
  });

  it("invokes onViewBurn when 0 is pressed", () => {
    const onViewBurn = vi.fn();

    renderHook(() => useKeyboard({ onViewBurn }));

    window.dispatchEvent(new KeyboardEvent("keydown", { key: "0" }));

    expect(onViewBurn).toHaveBeenCalledOnce();
  });

  it("keeps one listener across action changes and calls the latest action", () => {
    const addListener = vi.spyOn(window, "addEventListener");
    const removeListener = vi.spyOn(window, "removeEventListener");
    const firstOnNext = vi.fn();
    const latestOnNext = vi.fn();

    const { rerender, unmount } = renderHook(
      ({ onNext }) => useKeyboard({ onNext }),
      { initialProps: { onNext: firstOnNext } },
    );

    const keydownAdds = () =>
      addListener.mock.calls.filter(([event]) => event === "keydown");
    const keydownRemovals = () =>
      removeListener.mock.calls.filter(([event]) => event === "keydown");

    expect(keydownAdds()).toHaveLength(1);

    rerender({ onNext: latestOnNext });
    rerender({ onNext: latestOnNext });

    expect(keydownAdds()).toHaveLength(1);
    expect(keydownRemovals()).toHaveLength(0);

    window.dispatchEvent(new KeyboardEvent("keydown", { key: "j" }));

    expect(firstOnNext).not.toHaveBeenCalled();
    expect(latestOnNext).toHaveBeenCalledOnce();

    const attachedHandler = keydownAdds()[0][1];
    unmount();

    expect(keydownRemovals()).toHaveLength(1);
    expect(removeListener).toHaveBeenCalledWith("keydown", attachedHandler);
  });
});
