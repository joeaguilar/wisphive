import { cleanup, fireEvent, render, screen, within } from "@testing-library/react";
import { afterEach, describe, expect, it, vi } from "vitest";
import { DeferredDetailView, DetailView } from "./DetailView";
import type { AuditDecision, DecisionRequest, JsonValue } from "../types/protocol";
import { Queue } from "./Queue";

function deferred(overrides: Partial<AuditDecision>): AuditDecision {
  return {
    kind: "deferred",
    decided_by: "always_ask:intrinsic",
    project: "/Users/dev/AI_Projects/wisphive",
    agent_id: "cc-agent-alpha",
    tool_name: "AskUserQuestion",
    ts: "2026-07-04T11:58:00Z",
    ...overrides,
  };
}

describe("DeferredDetailView", () => {
  afterEach(cleanup);

  it("renders read-only detail with a focus deep-link for a wisphive terminal session", () => {
    const onFocusTerminal = vi.fn();
    render(
      <DeferredDetailView
        decision={deferred({ terminal_session_id: "66666666-6666-4666-8666-666666666666" })}
        onFocusTerminal={onFocusTerminal}
      />,
    );

    const detail = screen.getByLabelText("Deferred detail for AskUserQuestion");
    expect(within(detail).getByText(/never entered the in-console queue/)).toBeInTheDocument();
    expect(within(detail).getByText("always_ask:intrinsic")).toBeInTheDocument();

    fireEvent.click(within(detail).getByRole("button", { name: "Focus terminal" }));
    expect(onFocusTerminal).toHaveBeenCalledWith("66666666-6666-4666-8666-666666666666");

    // No in-console answer control on a deferred item (ADR-0002).
    expect(within(detail).queryByRole("button", { name: "Approve" })).not.toBeInTheDocument();
    expect(within(detail).queryByRole("button", { name: "Deny" })).not.toBeInTheDocument();
  });

  it("shows a go-to-terminal pointer naming the project for a hook-only session", () => {
    render(
      <DeferredDetailView
        decision={deferred({ terminal_session_id: undefined, project: "/home/dev/acme-api" })}
        onFocusTerminal={vi.fn()}
      />,
    );

    const detail = screen.getByLabelText("Deferred detail for AskUserQuestion");
    expect(within(detail).queryByRole("button", { name: "Focus terminal" })).not.toBeInTheDocument();
    expect(within(detail).getByText(/Answer in your/)).toHaveTextContent("acme-api");
  });

  it("renders the full AskUserQuestion text and all option labels read-only", () => {
    render(
      <DeferredDetailView
        decision={deferred({
          tool_input: {
            questions: [
              {
                question: "Which database should we use?",
                header: "Storage",
                options: [
                  { label: "PostgreSQL", description: "relational" },
                  { label: "SQLite", description: "embedded" },
                ],
              },
            ],
          },
        })}
        onFocusTerminal={vi.fn()}
      />,
    );

    const detail = screen.getByLabelText("Deferred detail for AskUserQuestion");
    expect(within(detail).getByText("Which database should we use?")).toBeInTheDocument();
    expect(within(detail).getByText("Storage")).toBeInTheDocument();
    expect(within(detail).getByText("PostgreSQL")).toBeInTheDocument();
    expect(within(detail).getByText("SQLite")).toBeInTheDocument();
    // Options are read-only list items, never actionable buttons.
    expect(within(detail).queryByRole("button", { name: /PostgreSQL/ })).not.toBeInTheDocument();
  });

  it("renders the plan text for a deferred ExitPlanMode", () => {
    render(
      <DeferredDetailView
        decision={deferred({
          tool_name: "ExitPlanMode",
          tool_input: { plan: "Step 1: build\nStep 2: ship" },
        })}
        onFocusTerminal={vi.fn()}
      />,
    );

    const detail = screen.getByLabelText("Deferred detail for ExitPlanMode");
    expect(within(detail).getByText(/Step 1: build/)).toBeInTheDocument();
    expect(within(detail).getByText(/Step 2: ship/)).toBeInTheDocument();
  });

  it("falls back gracefully when tool_input is null or an unknown shape", () => {
    render(
      <DeferredDetailView
        decision={deferred({ tool_input: null })}
        onFocusTerminal={vi.fn()}
      />,
    );
    const detail = screen.getByLabelText("Deferred detail for AskUserQuestion");
    expect(within(detail).getByText(/not available here/)).toBeInTheDocument();

    cleanup();

    render(
      <DeferredDetailView
        decision={deferred({ tool_name: "Elicitation", tool_input: { foo: "bar" } })}
        onFocusTerminal={vi.fn()}
      />,
    );
    const raw = screen.getByLabelText("Deferred detail for Elicitation");
    expect(within(raw).getByText(/"foo": "bar"/)).toBeInTheDocument();
  });

  it("renders XSS in question text as inert text, not injected HTML", () => {
    const evil = '<img src=x onerror="alert(1)">';
    render(
      <DeferredDetailView
        decision={deferred({
          tool_input: { questions: [{ question: evil, options: [{ label: "ok" }] }] },
        })}
        onFocusTerminal={vi.fn()}
      />,
    );

    const detail = screen.getByLabelText("Deferred detail for AskUserQuestion");
    // The literal markup is present as a text node...
    expect(within(detail).getByText(evil)).toBeInTheDocument();
    // ...and never became a real element.
    expect(detail.querySelector("img")).toBeNull();
  });

  it("renders malformed deferred options through the raw fallback", () => {
    render(
      <DeferredDetailView
        decision={deferred({
          tool_input: {
            questions: [{
              question: "Choose a deployment target",
              options: [{ description: "missing required label" }],
            }],
          },
        })}
        onFocusTerminal={vi.fn()}
      />,
    );

    const detail = screen.getByLabelText("Deferred detail for AskUserQuestion");
    expect(within(detail).getByText(/"description": "missing required label"/)).toBeInTheDocument();
    expect(within(detail).queryByRole("listitem")).not.toBeInTheDocument();
  });
});

function pending(toolInput: JsonValue): DecisionRequest {
  return {
    id: "request-1",
    agent_id: "cc-agent-alpha",
    agent_type: "claude_code",
    project: "/Users/dev/AI_Projects/wisphive",
    tool_name: "AskUserQuestion",
    tool_input: toolInput,
    timestamp: "2026-07-04T11:58:00Z",
    hook_event_name: "PreToolUse",
  };
}

describe("DetailView", () => {
  afterEach(cleanup);

  it("falls back without throwing for malformed questions and options arrays", () => {
    const props = { onApprove: vi.fn(), onDeny: vi.fn() };
    render(
      <DetailView
        request={pending({ questions: ["not a question"] })}
        {...props}
      />,
    );
    expect(screen.getByText("Question details unavailable.")).toBeInTheDocument();

    cleanup();

    render(
      <DetailView
        request={pending({
          questions: [{
            question: "Choose a deployment target",
            options: [{ description: "missing required label" }],
          }],
        })}
        {...props}
      />,
    );
    expect(screen.getByText("Question details unavailable.")).toBeInTheDocument();
    expect(screen.queryByText("Choose a deployment target")).not.toBeInTheDocument();
  });
});

describe("Queue", () => {
  afterEach(cleanup);

  it("summarizes only validated AskUserQuestion payloads", () => {
    const handlers = {
      selectedId: null,
      onSelect: vi.fn(),
      onApprove: vi.fn(),
      onDeny: vi.fn(),
    };
    const { container, rerender } = render(
      <Queue
        items={[pending({
          questions: [{
            question: "Choose a deployment target",
            options: [{ label: "Staging" }],
          }],
        })]}
        {...handlers}
      />,
    );
    expect(screen.getByText("Choose a deployment target")).toBeInTheDocument();

    rerender(
      <Queue
        items={[pending({
          questions: [{
            question: "Choose a deployment target",
            options: [{ description: "missing required label" }],
          }],
        })]}
        {...handlers}
      />,
    );
    expect(container.querySelector(".queue-item-summary")).toBeNull();
  });
});
