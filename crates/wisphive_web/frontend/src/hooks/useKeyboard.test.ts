import { cleanup, renderHook } from "@testing-library/react";
import { afterEach, describe, expect, it, vi } from "vitest";
import { useKeyboard } from "./useKeyboard";

afterEach(() => {
  cleanup();
  vi.restoreAllMocks();
});

describe("useKeyboard", () => {
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
