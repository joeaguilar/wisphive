import { cleanup, fireEvent, render, screen, within } from "@testing-library/react";
import { afterEach, describe, expect, it, vi } from "vitest";
import type { ComponentProps } from "react";
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

function deferred(overrides: Partial<AuditDecision>): AuditDecision {
  return {
    kind: "deferred",
    decided_by: "always_ask:intrinsic",
    project: "/Users/josefaguilar/AI_Projects/wisphive",
    agent_id: "cc-agent-alpha",
    tool_name: "AskUserQuestion",
    ts: "2026-07-04T11:58:00Z",
    ...overrides,
  };
}

function renderInbox(props: Partial<ComponentProps<typeof Inbox>> = {}) {
  return render(
    <Inbox
      items={[]}
      auditDecisions={[]}
      selectedId={null}
      onSelect={vi.fn()}
      onApprove={vi.fn()}
      onDeny={vi.fn()}
      onFocusTerminal={vi.fn()}
      {...props}
    />,
  );
}

describe("Inbox", () => {
  afterEach(() => {
    cleanup();
    vi.useRealTimers();
  });

  it("renders project·session group chip, agent, age, and summary for queued decisions", () => {
    vi.useFakeTimers();
    vi.setSystemTime(new Date(NOW));

    renderInbox({
      items: [request({ terminal_session_id: "11111111-1111-4111-8111-111111111111" })],
    });

    expect(screen.getByText("Inbox")).toBeInTheDocument();
    // Grouping signal: a single project·session chip (beyond a plain route line).
    expect(screen.getByText("wisphive · term 11111111")).toBeInTheDocument();
    expect(screen.getByText("cc-agent-alpha")).toBeInTheDocument();
    expect(screen.getByText("1m")).toBeInTheDocument();
    expect(screen.getByText("cargo test -p wisphive_daemon")).toBeInTheDocument();
  });

  it("orders older decisions first and marks the oldest row", () => {
    vi.useFakeTimers();
    vi.setSystemTime(new Date(NOW));

    const oldItem = request({ id: "old", tool_name: "OldTool", timestamp: "2026-07-04T11:45:00Z" });
    const newItem = request({ id: "new", tool_name: "NewTool", timestamp: "2026-07-04T11:59:30Z" });
    const { container } = renderInbox({ items: [newItem, oldItem] });

    const rows = [...container.querySelectorAll(".inbox-item")];
    expect(rows).toHaveLength(2);
    expect(within(rows[0] as HTMLElement).getByText("OldTool")).toBeInTheDocument();
    expect(rows[0]).toHaveClass("oldest");
    expect(within(rows[1] as HTMLElement).getByText("NewTool")).toBeInTheDocument();
  });

  it("colour-distinguishes rows from different project·session groups", () => {
    vi.useFakeTimers();
    vi.setSystemTime(new Date(NOW));

    const a = request({ id: "a", terminal_session_id: "aaaaaaaa-1111-4111-8111-111111111111" });
    const b = request({ id: "b", terminal_session_id: "bbbbbbbb-2222-4222-8222-222222222222" });
    const { container } = renderInbox({ items: [a, b] });

    const rows = [...container.querySelectorAll(".inbox-item")] as HTMLElement[];
    expect(rows).toHaveLength(2);
    // Different sessions → different deterministic left-rail colours.
    expect(rows[0].style.borderLeftColor).not.toBe("");
    expect(rows[0].style.borderLeftColor).not.toBe(rows[1].style.borderLeftColor);
  });

  it("wires approve and deny actions", () => {
    vi.useFakeTimers();
    vi.setSystemTime(new Date(NOW));

    const onApprove = vi.fn();
    const onDeny = vi.fn();
    renderInbox({ items: [request({ id: "decision-1" })], onApprove, onDeny });

    fireEvent.click(screen.getByRole("button", { name: "Approve" }));
    fireEvent.click(screen.getByRole("button", { name: "Deny" }));

    expect(onApprove).toHaveBeenCalledWith("decision-1");
    expect(onDeny).toHaveBeenCalledWith("decision-1");
  });

  it("highlights the selected row and selects on click", () => {
    vi.useFakeTimers();
    vi.setSystemTime(new Date(NOW));

    const onSelect = vi.fn();
    const { container } = renderInbox({
      items: [request({ id: "sel-1", tool_name: "AlphaTool" })],
      selectedId: "sel-1",
      onSelect,
    });

    const row = container.querySelector(".inbox-item") as HTMLElement;
    expect(row).toHaveClass("selected");

    fireEvent.click(row);
    expect(onSelect).toHaveBeenCalledWith("sel-1");
  });

  it("shows the untruncated input and deny-with-message affordance on the selected row", () => {
    vi.useFakeTimers();
    vi.setSystemTime(new Date(NOW));

    const longCmd = "echo " + "a".repeat(200);
    const onDeny = vi.fn();
    renderInbox({
      items: [request({ id: "long", tool_input: { command: longCmd } })],
      selectedId: "long",
      onDeny,
    });

    // Full command is reachable in-place (no single-place truncation).
    expect(screen.getByText(longCmd)).toBeInTheDocument();

    // Deny + Message opens the feedback modal and forwards the message.
    fireEvent.click(screen.getByRole("button", { name: "Deny + Message" }));
    const textarea = screen.getByPlaceholderText("Claude will see this as feedback...");
    fireEvent.change(textarea, { target: { value: "wrong dir" } });
    fireEvent.keyDown(textarea, { key: "Enter" });
    expect(onDeny).toHaveBeenCalledWith("long", "wrong dir");
  });

  it("shows ALL decision info on the selected row — a Write's full content, not just the path", () => {
    vi.useFakeTimers();
    vi.setSystemTime(new Date(NOW));

    const fileContent = "fn main() {\n    println!(\"secret sauce\");\n}\n";
    renderInbox({
      items: [
        request({
          id: "w1",
          tool_name: "Write",
          tool_input: { file_path: "/repo/src/main.rs", content: fileContent },
        }),
      ],
      selectedId: "w1",
    });

    // The gap that motivated this change: collapsed rows only showed file_path,
    // hiding the content you actually need to accept a Write. Selected row must
    // surface the full content (via the shared DetailView/ToolContent renderer).
    expect(screen.getByText("/repo/src/main.rs")).toBeInTheDocument();
    expect(screen.getByText(/secret sauce/)).toBeInTheDocument();
    // Full action set is reachable (richer than plain approve/deny).
    expect(screen.getByRole("button", { name: "+ Context" })).toBeInTheDocument();
    expect(screen.getByRole("button", { name: "Always Allow" })).toBeInTheDocument();
  });

  it("surfaces a deferred native prompt as a waiting-in-your-terminal row with a focus deep-link", () => {
    vi.useFakeTimers();
    vi.setSystemTime(new Date(NOW));

    const onFocusTerminal = vi.fn();
    render(
      <Inbox
        items={[]}
        auditDecisions={[deferred({ terminal_session_id: "22222222-2222-4222-8222-222222222222" })]}
        selectedId={null}
        onSelect={vi.fn()}
        onApprove={vi.fn()}
        onDeny={vi.fn()}
        onFocusTerminal={onFocusTerminal}
      />,
    );

    const section = screen.getByLabelText("Waiting in your terminal");
    // Labeled "deferred" with its tool name and a project·session group header.
    expect(within(section).getByText("deferred")).toBeInTheDocument();
    expect(within(section).getByText("AskUserQuestion")).toBeInTheDocument();
    expect(within(section).getByText("wisphive · term 22222222")).toBeInTheDocument();

    // Deep-link/focus CTA present; clicking focuses that exact terminal.
    const focusBtn = within(section).getByRole("button", { name: "Focus terminal" });
    fireEvent.click(focusBtn);
    expect(onFocusTerminal).toHaveBeenCalledWith("22222222-2222-4222-8222-222222222222");

    // Deferred rows never render an in-console answer control.
    expect(within(section).queryByRole("button", { name: "Approve" })).not.toBeInTheDocument();
    expect(within(section).queryByRole("button", { name: "Deny" })).not.toBeInTheDocument();
  });

  it("shows a go-to-terminal pointer naming the project for a hook-only deferred item", () => {
    vi.useFakeTimers();
    vi.setSystemTime(new Date(NOW));

    render(
      <Inbox
        items={[]}
        auditDecisions={[deferred({ terminal_session_id: undefined, project: "/home/dev/acme-api" })]}
        selectedId={null}
        onSelect={vi.fn()}
        onApprove={vi.fn()}
        onDeny={vi.fn()}
        onFocusTerminal={vi.fn()}
      />,
    );

    const section = screen.getByLabelText("Waiting in your terminal");
    // No embedded terminal to focus → a pointer naming the project instead.
    expect(within(section).queryByRole("button", { name: "Focus terminal" })).not.toBeInTheDocument();
    const pointer = within(section).getByText(/Answer in your/);
    expect(pointer).toHaveTextContent("acme-api");
    // Still no in-console answer control.
    expect(within(section).queryByRole("button", { name: "Approve" })).not.toBeInTheDocument();
  });

  it("expands a deferred row to a read-only detail with the deep-link and no answer control", () => {
    vi.useFakeTimers();
    vi.setSystemTime(new Date(NOW));

    const onFocusTerminal = vi.fn();
    render(
      <Inbox
        items={[]}
        auditDecisions={[deferred({ terminal_session_id: "33333333-3333-4333-8333-333333333333" })]}
        selectedId={null}
        onSelect={vi.fn()}
        onApprove={vi.fn()}
        onDeny={vi.fn()}
        onFocusTerminal={onFocusTerminal}
      />,
    );

    const detailLabel = "Deferred detail for AskUserQuestion";
    expect(screen.queryByLabelText(detailLabel)).not.toBeInTheDocument();

    // Click the row body (topline) to expand its read-only detail.
    fireEvent.click(screen.getByText("AskUserQuestion"));

    const detail = screen.getByLabelText(detailLabel);
    expect(detail).toBeInTheDocument();
    expect(within(detail).getByText(/never entered the in-console queue/)).toBeInTheDocument();
    // The detail carries its own focus CTA, no approve/deny.
    expect(within(detail).getByRole("button", { name: "Focus terminal" })).toBeInTheDocument();
    expect(within(detail).queryByRole("button", { name: "Approve" })).not.toBeInTheDocument();
  });

  it("shows a one-line question summary on the deferred row and the full options in its detail", () => {
    vi.useFakeTimers();
    vi.setSystemTime(new Date(NOW));

    render(
      <Inbox
        items={[]}
        auditDecisions={[
          deferred({
            terminal_session_id: "77777777-7777-4777-8777-777777777777",
            tool_input: {
              questions: [
                { question: "Deploy to prod now?", options: [{ label: "Yes, deploy" }, { label: "Wait" }] },
              ],
            },
          }),
        ]}
        selectedId={null}
        onSelect={vi.fn()}
        onApprove={vi.fn()}
        onDeny={vi.fn()}
        onFocusTerminal={vi.fn()}
      />,
    );

    const section = screen.getByLabelText("Waiting in your terminal");
    // Collapsed row shows the literal question summary (not just the tool name).
    expect(within(section).getByText("Deploy to prod now?")).toBeInTheDocument();

    // Expanding reveals every option label, untruncated and read-only.
    fireEvent.click(within(section).getByText("AskUserQuestion"));
    const detail = screen.getByLabelText("Deferred detail for AskUserQuestion");
    expect(within(detail).getByText("Yes, deploy")).toBeInTheDocument();
    expect(within(detail).getByText("Wait")).toBeInTheDocument();
  });

  it("renders a deferred row with null tool_input without crashing", () => {
    vi.useFakeTimers();
    vi.setSystemTime(new Date(NOW));

    render(
      <Inbox
        items={[]}
        auditDecisions={[deferred({ tool_input: null })]}
        selectedId={null}
        onSelect={vi.fn()}
        onApprove={vi.fn()}
        onDeny={vi.fn()}
        onFocusTerminal={vi.fn()}
      />,
    );

    const section = screen.getByLabelText("Waiting in your terminal");
    expect(within(section).getByText("AskUserQuestion")).toBeInTheDocument();
  });

  it("groups deferred items by project·session under distinct headers", () => {
    vi.useFakeTimers();
    vi.setSystemTime(new Date(NOW));

    render(
      <Inbox
        items={[]}
        auditDecisions={[
          deferred({ terminal_session_id: "44444444-4444-4444-8444-444444444444", ts: "2026-07-04T11:59:00Z" }),
          deferred({ terminal_session_id: "55555555-5555-4555-8555-555555555555", ts: "2026-07-04T11:57:00Z" }),
        ]}
        selectedId={null}
        onSelect={vi.fn()}
        onApprove={vi.fn()}
        onDeny={vi.fn()}
        onFocusTerminal={vi.fn()}
      />,
    );

    const section = screen.getByLabelText("Waiting in your terminal");
    expect(within(section).getByText("wisphive · term 44444444")).toBeInTheDocument();
    expect(within(section).getByText("wisphive · term 55555555")).toBeInTheDocument();
  });

  it("counts recent auto-answered + deferred events in the header", () => {
    vi.useFakeTimers();
    vi.setSystemTime(new Date(NOW));
    const auditDecisions: AuditDecision[] = [
      { kind: "auto_approved", decided_by: "level:all", project: "/proj", agent_id: "cc-1", tool_name: "Read", ts: "2026-07-04T11:30:00Z" },
      { kind: "deferred", decided_by: "always_ask:intrinsic", project: "/proj", agent_id: "cc-2", tool_name: "AskUserQuestion", ts: "2026-07-04T11:40:00Z" },
      { kind: "auto_approved", decided_by: "level:all", project: "/proj", agent_id: "cc-3", tool_name: "Read", ts: "2026-07-04T10:30:00Z" },
    ];

    const { container } = renderInbox({ auditDecisions });

    const header = container.querySelector(".inbox-count") as HTMLElement;
    expect(header.textContent).toBe("0 waiting · 1 in your terminal · 1 auto-answered in last hour (view)");
  });

  it("reveals the auto-answer feed when '(view)' is clicked", () => {
    vi.useFakeTimers();
    vi.setSystemTime(new Date(NOW));
    const auditDecisions: AuditDecision[] = [
      {
        kind: "auto_approved",
        decided_by: "level:all",
        project: "/Users/josefaguilar/AI_Projects/wisphive",
        agent_id: "cc-agent-alpha",
        tool_name: "Read",
        ts: "2026-07-04T11:55:00Z",
      },
    ];

    renderInbox({ auditDecisions });

    expect(screen.queryByLabelText("Decided without you")).not.toBeInTheDocument();

    fireEvent.click(screen.getByRole("button", { name: "(view)" }));

    const feed = screen.getByLabelText("Decided without you");
    expect(feed).toBeInTheDocument();
    expect(within(feed).getByText("level:all")).toBeInTheDocument();
    expect(within(feed).getByText("Read")).toBeInTheDocument();

    expect(screen.getByRole("button", { name: "(hide)" })).toBeInTheDocument();
  });

  it("omits the '(view)' toggle when nothing was auto-answered recently", () => {
    vi.useFakeTimers();
    vi.setSystemTime(new Date(NOW));

    const { container } = renderInbox();

    const header = container.querySelector(".inbox-count") as HTMLElement;
    expect(header.textContent).toBe("0 waiting · 0 auto-answered in last hour");
    expect(screen.queryByRole("button", { name: "(view)" })).not.toBeInTheDocument();
  });
});
