import { act, cleanup, fireEvent, render, screen, within } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import type { ComponentProps } from "react";
import { Board } from "./Board";
import { STALL_THRESHOLD_MS } from "./liveness";
import { deferredKey } from "./queueUtils";
import type { AuditDecision, DecisionRequest } from "../types/protocol";

const NOW = "2026-07-15T12:00:00Z";
const NOW_MS = Date.parse(NOW);

function iso(msAgo: number): string {
  return new Date(NOW_MS - msAgo).toISOString();
}

function audit(overrides: Partial<AuditDecision>): AuditDecision {
  return {
    kind: "auto_approved",
    decided_by: "level:all",
    project: "/proj/alpha",
    agent_id: "cc-alpha-1",
    tool_name: "Read",
    ts: iso(30_000),
    ...overrides,
  };
}

function request(overrides: Partial<DecisionRequest>): DecisionRequest {
  return {
    id: "11111111-2222-3333-4444-555555555555",
    agent_id: "cc-waiting-1",
    agent_type: "claude_code",
    project: "/proj/alpha",
    tool_name: "Grep",
    tool_input: { pattern: "TODO" },
    timestamp: iso(20_000),
    hook_event_name: "PreToolUse",
    ...overrides,
  };
}

function renderBoard(props: Partial<ComponentProps<typeof Board>> = {}) {
  return render(
    <Board
      sessions={[]}
      agents={[]}
      queue={[]}
      auditDecisions={[]}
      endedAgentIds={[]}
      onLoad={vi.fn()}
      onOpenInbox={vi.fn()}
      {...props}
    />,
  );
}

describe("Board", () => {
  beforeEach(() => {
    vi.useFakeTimers();
    vi.setSystemTime(new Date(NOW));
  });

  afterEach(() => {
    cleanup();
    vi.useRealTimers();
  });

  it("renders working, stalled, and done lanes grouped by project with state words", () => {
    renderBoard({
      auditDecisions: [
        audit({ agent_id: "cc-working-1", ts: iso(30_000), tool_name: "Bash" }),
        audit({
          agent_id: "cc-stalled-1",
          ts: iso(STALL_THRESHOLD_MS + 120_000),
          tool_name: "Edit",
          project: "/proj/bravo",
        }),
        audit({ agent_id: "cc-done-1", ts: iso(60_000), tool_name: "Stop" }),
      ],
    });

    // Counts header (Redundant Signal Rule: state words, not colour alone).
    expect(screen.getByRole("status")).toHaveTextContent(
      "1 working · 0 waiting · 1 stalled · 1 done",
    );

    // Project grouping headers.
    expect(screen.getByText("alpha")).toBeInTheDocument();
    expect(screen.getByText("bravo")).toBeInTheDocument();

    // Lane state words + full agent ids + task labels.
    const working = screen.getByRole("listitem", { name: /cc-working-1 — working/ });
    expect(within(working).getByText("last: Bash")).toBeInTheDocument();
    const stalled = screen.getByRole("listitem", { name: /cc-stalled-1 — stalled · silent 12m/ });
    expect(stalled.className).toContain("state-stalled");
    expect(
      screen.getByRole("listitem", { name: /cc-done-1 — turn ended/ }),
    ).toBeInTheDocument();
  });

  it("cross-links a waiting (queued decision) lane to its inbox item", () => {
    const onOpenInbox = vi.fn();
    const item = request({});
    renderBoard({ queue: [item], onOpenInbox });

    const lane = screen.getByRole("listitem", { name: /cc-waiting-1 — waiting on you/ });
    fireEvent.click(within(lane).getByRole("button", { name: "Answer in Inbox →" }));
    expect(onOpenInbox).toHaveBeenCalledWith({
      kind: "decision",
      requestId: item.id,
      toolName: "Grep",
    });
  });

  it("cross-links a waiting (deferred native prompt) lane to its deferred inbox row", () => {
    const onOpenInbox = vi.fn();
    const row = audit({
      kind: "deferred",
      agent_id: "cc-defer-1",
      tool_name: "AskUserQuestion",
      decided_by: "always_ask:intrinsic",
      ts: iso(90_000),
    });
    renderBoard({ auditDecisions: [row], onOpenInbox });

    const lane = screen.getByRole("listitem", { name: /cc-defer-1 — waiting on you/ });
    fireEvent.click(within(lane).getByRole("button", { name: "Answer in Inbox →" }));
    expect(onOpenInbox).toHaveBeenCalledWith({
      kind: "deferred",
      deferredKey: deferredKey(row),
      toolName: "AskUserQuestion",
    });
  });

  it("shows Codex activity in its own labelled lane", () => {
    renderBoard({
      auditDecisions: [
        audit({ agent_id: "cc-alpha-1" }),
        audit({ agent_id: "codex-bravo-1", project: "/proj/bravo", tool_name: "Bash" }),
      ],
    });
    const codexLane = screen.getByRole("listitem", { name: /codex-bravo-1 — working/ });
    expect(within(codexLane).getByText("codex")).toBeInTheDocument();
  });

  it("shows wave progress where derivable", () => {
    renderBoard({
      auditDecisions: [
        audit({ tool_name: "Task", ts: iso(300_000), tool_use_id: "t1" }),
        audit({ tool_name: "Task", ts: iso(280_000), tool_use_id: "t2" }),
        audit({ tool_name: "SubagentStop", ts: iso(100_000) }),
        audit({ tool_name: "Read", ts: iso(30_000) }),
      ],
    });
    expect(screen.getByText("subagents 1/2 done · 1 active")).toBeInTheDocument();
  });

  it("is a read-only state mirror: no agent control affordances (spec §5 hard constraint)", () => {
    renderBoard({
      queue: [request({})],
      auditDecisions: [audit({ agent_id: "cc-alpha-1" })],
    });
    // The ONLY button on the whole board is the waiting lane's inbox link.
    const buttons = screen.getAllByRole("button");
    expect(buttons.map((b) => b.textContent)).toEqual(["Answer in Inbox →"]);
    expect(screen.queryByRole("button", { name: /stop|start|kill|retarget|spawn/i })).toBeNull();
    // And the read-only contract is stated on the surface.
    expect(screen.getByText(/Read-only state mirror/)).toBeInTheDocument();
  });

  it("renders the empty state and polls sessions on mount", () => {
    const onLoad = vi.fn();
    renderBoard({ onLoad });
    expect(screen.getByText("No agent activity in the last hour")).toBeInTheDocument();
    expect(onLoad).toHaveBeenCalledTimes(1);
    vi.advanceTimersByTime(20_000);
    expect(onLoad).toHaveBeenCalledTimes(2);
  });

  it("flips a lane working → stalled on the clock without new events", () => {
    // Last event lands 2s before the threshold; two 5s derive ticks later the
    // lane must be loud. This is the killed-agent AC in miniature.
    renderBoard({
      auditDecisions: [
        audit({ agent_id: "cc-flip-1", ts: iso(STALL_THRESHOLD_MS - 2_000), tool_name: "Bash" }),
      ],
    });
    expect(screen.getByRole("listitem", { name: /cc-flip-1 — working/ })).toBeInTheDocument();
    act(() => {
      vi.advanceTimersByTime(10_000);
    });
    expect(
      screen.getByRole("listitem", { name: /cc-flip-1 — stalled · silent 10m/ }),
    ).toBeInTheDocument();
  });
});
