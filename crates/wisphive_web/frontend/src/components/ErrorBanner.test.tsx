import { cleanup, render, screen } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { afterEach, describe, expect, it, vi } from "vitest";
import { ErrorBanner } from "./ErrorBanner";
import type { UiError } from "../hooks/useWisphive";

function uiError(overrides: Partial<UiError> = {}): UiError {
  return {
    id: 1,
    source: "daemon",
    message: "failed to spawn agent: denied",
    at: "2026-07-18T12:00:00Z",
    count: 1,
    ...overrides,
  };
}

describe("ErrorBanner (itr#567)", () => {
  afterEach(cleanup);

  it("renders nothing when there are no errors", () => {
    const { container } = render(<ErrorBanner errors={[]} onDismiss={() => undefined} />);
    expect(container.firstChild).toBeNull();
  });

  it("renders each error with its source label and full message", () => {
    render(
      <ErrorBanner
        errors={[
          uiError({ id: 1, source: "daemon", message: "failed to spawn agent: denied" }),
          uiError({ id: 2, source: "terminal", message: "session not found" }),
          uiError({ id: 3, source: "client", message: 'Not connected — "approve" was not sent.' }),
        ]}
        onDismiss={() => undefined}
      />,
    );
    expect(screen.getByText("Daemon error")).toBeInTheDocument();
    expect(screen.getByText("failed to spawn agent: denied")).toBeInTheDocument();
    expect(screen.getByText("Terminal error")).toBeInTheDocument();
    expect(screen.getByText("session not found")).toBeInTheDocument();
    expect(screen.getByText("Not delivered")).toBeInTheDocument();
    expect(screen.getByText('Not connected — "approve" was not sent.')).toBeInTheDocument();
  });

  it("renders untrusted daemon markup as inert text, never as HTML", () => {
    const hostile = '<img src=x onerror=alert(1)> **[click](javascript:x)**';
    render(<ErrorBanner errors={[uiError({ message: hostile })]} onDismiss={() => undefined} />);
    expect(screen.getByText(hostile)).toBeInTheDocument();
    expect(document.querySelector("img")).toBeNull();
  });

  it("shows a collapse count for repeated errors", () => {
    render(
      <ErrorBanner errors={[uiError({ count: 3 })]} onDismiss={() => undefined} />,
    );
    expect(screen.getByText("×3")).toBeInTheDocument();
  });

  it("dismiss buttons call onDismiss with the row id", async () => {
    const onDismiss = vi.fn();
    render(
      <ErrorBanner
        errors={[uiError({ id: 7, message: "one" }), uiError({ id: 9, message: "two" })]}
        onDismiss={onDismiss}
      />,
    );
    const buttons = screen.getAllByRole("button", { name: /dismiss/i });
    expect(buttons).toHaveLength(2);
    await userEvent.click(buttons[1]);
    expect(onDismiss).toHaveBeenCalledWith(9);
  });
});
