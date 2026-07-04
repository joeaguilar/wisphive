import { cleanup, fireEvent, render, screen, within } from "@testing-library/react";
import { afterEach, describe, expect, it, vi } from "vitest";
import { Inbox } from "./Inbox";
import type { AuditDecision, DecisionRequest } from "../types/protocol";

const NOW = "2026-07-04T12:00:00Z";

function request(overrides: Partial<DecisionRequest>): DecisionRequest {
  return {
    id: "decision-base",
    agent_id: "cc-agent-alpha",
    agent_type: "codex",
    project: "/Users/josefaguilar/AI_Projects/wisphive",
    tool_name: "Bash",
    tool_input: { command: "cargo test -p wisphive_daemon" },
    timestamp: "2026-07-04T11:59:00Z",
    hook_event_name: "PermissionRequest",
    ...overrides,
  };
}

describe("Inbox", () => {
  afterEach(() => {
    cleanup();
    vi.useRealTimers();
  });

  it("renders project, session, agent, age, and summary for queued decisions", () => {
    vi.useFakeTimers();
    vi.setSystemTime(new Date(NOW));

    render(
      <Inbox
        items={[
          request({
            terminal_session_id: "11111111-1111-4111-8111-111111111111",
          }),
        ]}
        auditDecisions={[]}
        selectedId={null}
        onSelect={vi.fn()}
        onApprove={vi.fn()}
        onDeny={vi.fn()}
      />,
    );

    expect(screen.getByText("Inbox")).toBeInTheDocument();
    expect(screen.getByText("wisphive")).toBeInTheDocument();
    expect(screen.getByText("term 11111111")).toBeInTheDocument();
    expect(screen.getByText("cc-agent-alpha")).toBeInTheDocument();
    expect(screen.getByText("1m")).toBeInTheDocument();
    expect(screen.getByText("cargo test -p wisphive_daemon")).toBeInTheDocument();
  });

  it("orders older decisions first and marks the oldest row", () => {
    vi.useFakeTimers();
    vi.setSystemTime(new Date(NOW));

    const oldItem = request({
      id: "old",
      tool_name: "OldTool",
      timestamp: "2026-07-04T11:45:00Z",
    });
    const newItem = request({
      id: "new",
      tool_name: "NewTool",
      timestamp: "2026-07-04T11:59:30Z",
    });
    const { container } = render(
      <Inbox
        items={[newItem, oldItem]}
        auditDecisions={[]}
        selectedId={null}
        onSelect={vi.fn()}
        onApprove={vi.fn()}
        onDeny={vi.fn()}
      />,
    );

    const rows = [...container.querySelectorAll(".inbox-item")];
    expect(rows).toHaveLength(2);
    expect(within(rows[0] as HTMLElement).getByText("OldTool")).toBeInTheDocument();
    expect(rows[0]).toHaveClass("oldest");
    expect(within(rows[1] as HTMLElement).getByText("NewTool")).toBeInTheDocument();
  });

  it("wires approve and deny actions", () => {
    vi.useFakeTimers();
    vi.setSystemTime(new Date(NOW));

    const onApprove = vi.fn();
    const onDeny = vi.fn();
    render(
      <Inbox
        items={[request({ id: "decision-1" })]}
        auditDecisions={[]}
        selectedId={null}
        onSelect={vi.fn()}
        onApprove={onApprove}
        onDeny={onDeny}
      />,
    );

    fireEvent.click(screen.getByRole("button", { name: "Approve" }));
    fireEvent.click(screen.getByRole("button", { name: "Deny" }));

    expect(onApprove).toHaveBeenCalledWith("decision-1");
    expect(onDeny).toHaveBeenCalledWith("decision-1");
  });

  it("highlights the selected row and selects on click", () => {
    vi.useFakeTimers();
    vi.setSystemTime(new Date(NOW));

    const onSelect = vi.fn();
    const { container } = render(
      <Inbox
        items={[request({ id: "sel-1", tool_name: "AlphaTool" })]}
        auditDecisions={[]}
        selectedId="sel-1"
        onSelect={onSelect}
        onApprove={vi.fn()}
        onDeny={vi.fn()}
      />,
    );

    const row = container.querySelector(".inbox-item") as HTMLElement;
    // The highlighted row is the one keyboard approve/deny act on.
    expect(row).toHaveClass("selected");

    fireEvent.click(row);
    expect(onSelect).toHaveBeenCalledWith("sel-1");
  });

  it("counts recent auto-answered audit events in the header", () => {
    vi.useFakeTimers();
    vi.setSystemTime(new Date(NOW));
    const auditDecisions: AuditDecision[] = [
      {
        kind: "auto_approved",
        decided_by: "level:all",
        project: "/proj",
        agent_id: "cc-1",
        tool_name: "Read",
        ts: "2026-07-04T11:30:00Z",
      },
      {
        kind: "deferred",
        decided_by: "always_ask:intrinsic",
        project: "/proj",
        agent_id: "cc-2",
        tool_name: "AskUserQuestion",
        ts: "2026-07-04T11:40:00Z",
      },
      {
        kind: "auto_approved",
        decided_by: "level:all",
        project: "/proj",
        agent_id: "cc-3",
        tool_name: "Read",
        ts: "2026-07-04T10:30:00Z",
      },
    ];

    render(
      <Inbox
        items={[]}
        auditDecisions={auditDecisions}
        selectedId={null}
        onSelect={vi.fn()}
        onApprove={vi.fn()}
        onDeny={vi.fn()}
      />,
    );

    expect(screen.getByText("0 waiting · 1 auto-answered in last hour")).toBeInTheDocument();
  });
});
