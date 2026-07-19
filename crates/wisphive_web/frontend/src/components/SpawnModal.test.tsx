import { act, cleanup, fireEvent, render, screen } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { afterEach, describe, expect, it, vi } from "vitest";
import { SpawnModal } from "./SpawnModal";
import type { SpawnStatus } from "../hooks/useWisphive";

function renderModal(status: SpawnStatus | null, onSpawn = vi.fn(), onClose = vi.fn()) {
  const view = render(
    <SpawnModal
      projects={[]}
      defaultProject="/proj"
      status={status}
      onSpawn={onSpawn}
      onClose={onClose}
    />,
  );
  return { view, onSpawn, onClose };
}

/** Fill the prompt so the submit button's enablement reflects only the
 * correlated status, not empty-form validation. fireEvent (not userEvent)
 * so it also works under fake timers. */
function fillPrompt(text = "do it") {
  fireEvent.change(screen.getByPlaceholderText("What should the agent do?"), {
    target: { value: text },
  });
}

describe("SpawnModal correlated submit status (itr#567)", () => {
  afterEach(cleanup);

  it("submits the form and does NOT close on its own (result decides)", async () => {
    const { onSpawn, onClose } = renderModal(null);
    await userEvent.type(screen.getByPlaceholderText("What should the agent do?"), "do it");
    await userEvent.click(screen.getByRole("button", { name: "Spawn" }));
    expect(onSpawn).toHaveBeenCalledWith(
      expect.objectContaining({ project: "/proj", prompt: "do it" }),
    );
    expect(onClose).not.toHaveBeenCalled();
  });

  it("pending disables the submit and shows progress", () => {
    renderModal({ phase: "pending", correlationId: "spawn-1" });
    fillPrompt();
    // Disabled even with a complete form — the wait, not validation, gates it.
    const btn = screen.getByRole("button", { name: "Spawning…" });
    expect(btn).toBeDisabled();
  });

  it("queued shows the positive confirmation and a Close action", async () => {
    const { onClose } = renderModal({
      phase: "queued",
      correlationId: "spawn-1",
      decisionId: "00000000-0000-4000-8000-000000000042",
    });
    expect(screen.getByRole("status")).toHaveTextContent(
      "Spawn queued for approval — review and approve it from the Inbox.",
    );
    await userEvent.click(screen.getByRole("button", { name: "Close" }));
    expect(onClose).toHaveBeenCalled();
  });

  it("refused shows the daemon's full message and re-enables retry", () => {
    const message =
      "failed to spawn agent: approved SpawnAgent action failed: refusing to spawn Codex into /proj: .codex/hooks.json contains non-Wisphive hooks";
    renderModal({ phase: "refused", correlationId: "spawn-1", message });
    fillPrompt();
    // The full daemon message is reachable — no truncation.
    expect(screen.getByRole("alert")).toHaveTextContent(message);
    expect(screen.getByRole("button", { name: "Retry Spawn" })).toBeEnabled();
  });

  it("a pending submit that gets no reply times out into a retryable error", () => {
    vi.useFakeTimers();
    try {
      renderModal({ phase: "pending", correlationId: "spawn-1" });
      fillPrompt();
      expect(screen.getByRole("button", { name: "Spawning…" })).toBeDisabled();
      act(() => vi.advanceTimersByTime(10_000));
      expect(screen.getByRole("alert")).toHaveTextContent("No response from the daemon");
      expect(screen.getByRole("button", { name: "Retry Spawn" })).toBeEnabled();
    } finally {
      vi.useRealTimers();
    }
  });

  it("a late queued ack clears the timeout warning", () => {
    vi.useFakeTimers();
    try {
      const view = render(
        <SpawnModal
          projects={[]}
          defaultProject="/proj"
          status={{ phase: "pending", correlationId: "spawn-1" }}
          onSpawn={vi.fn()}
          onClose={vi.fn()}
        />,
      );
      act(() => vi.advanceTimersByTime(10_000));
      expect(screen.getByRole("alert")).toHaveTextContent("No response from the daemon");

      view.rerender(
        <SpawnModal
          projects={[]}
          defaultProject="/proj"
          status={{
            phase: "queued",
            correlationId: "spawn-1",
            decisionId: "00000000-0000-4000-8000-000000000042",
          }}
          onSpawn={vi.fn()}
          onClose={vi.fn()}
        />,
      );
      expect(screen.queryByRole("alert")).toBeNull();
      expect(screen.getByRole("status")).toHaveTextContent("Spawn queued for approval");
    } finally {
      vi.useRealTimers();
    }
  });
});
