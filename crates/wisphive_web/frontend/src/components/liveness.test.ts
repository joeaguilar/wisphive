import { describe, expect, it } from "vitest";
import {
  BOARD_WINDOW_MS,
  STALL_THRESHOLD_MS,
  deriveBoard,
  fmtDuration,
  type BoardInputs,
} from "./liveness";
import { deferredKey } from "./queueUtils";
import type {
  AgentInfo,
  AuditDecision,
  DecisionRequest,
  SessionSummary,
} from "../types/protocol";

// Fixed clock: all fixture timestamps are derived from NOW_MS so the
// working→stalled threshold is tested to the millisecond, not with timers.
const NOW_MS = Date.parse("2026-07-15T12:00:00Z");

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

function session(overrides: Partial<SessionSummary>): SessionSummary {
  return {
    agent_id: "cc-alpha-1",
    agent_type: "claude_code",
    project: "/proj/alpha",
    first_seen: iso(3_600_000),
    last_seen: iso(30_000),
    total_calls: 5,
    approved: 5,
    denied: 0,
    is_live: true,
    pending_count: 0,
    ...overrides,
  };
}

function agent(overrides: Partial<AgentInfo>): AgentInfo {
  return {
    agent_id: "cc-alpha-1",
    agent_type: "claude_code",
    project: "/proj/alpha",
    connected_at: iso(3_600_000),
    last_seen: iso(30_000),
    ...overrides,
  };
}

function request(overrides: Partial<DecisionRequest>): DecisionRequest {
  return {
    id: "11111111-2222-3333-4444-555555555555",
    agent_id: "cc-alpha-1",
    agent_type: "claude_code",
    project: "/proj/alpha",
    tool_name: "Grep",
    tool_input: { pattern: "TODO" },
    timestamp: iso(20_000),
    hook_event_name: "PreToolUse",
    ...overrides,
  };
}

function inputs(overrides: Partial<BoardInputs>): BoardInputs {
  return {
    sessions: [],
    agents: [],
    queue: [],
    auditDecisions: [],
    endedAgentIds: [],
    nowMs: NOW_MS,
    ...overrides,
  };
}

function onlyLane(model: ReturnType<typeof deriveBoard>) {
  expect(model.projects).toHaveLength(1);
  expect(model.projects[0].lanes).toHaveLength(1);
  return model.projects[0].lanes[0];
}

describe("deriveBoard lane states", () => {
  it("recent activity derives a working lane grouped under its project", () => {
    const model = deriveBoard(
      inputs({ auditDecisions: [audit({ ts: iso(30_000), tool_name: "Bash" })] }),
    );
    const lane = onlyLane(model);
    expect(model.projects[0].project).toBe("/proj/alpha");
    expect(lane.state).toBe("working");
    expect(lane.lastToolName).toBe("Bash");
    expect(model.totals).toEqual({ working: 1, waiting: 0, stalled: 0, done: 0 });
  });

  it("stays working at exactly the stall threshold, flips stalled just past it", () => {
    const atThreshold = deriveBoard(
      inputs({ auditDecisions: [audit({ ts: iso(STALL_THRESHOLD_MS) })] }),
    );
    expect(onlyLane(atThreshold).state).toBe("working");

    const pastThreshold = deriveBoard(
      inputs({ auditDecisions: [audit({ ts: iso(STALL_THRESHOLD_MS + 1_000) })] }),
    );
    const lane = onlyLane(pastThreshold);
    expect(lane.state).toBe("stalled");
    expect(lane.silentForMs).toBe(STALL_THRESHOLD_MS + 1_000);
  });

  it("a killed agent (reaped/disconnected, no Stop event) is stalled, not done", () => {
    // The daemon reaps inactive agents (~300s) and broadcasts a disconnect;
    // that is inactivity evidence, not termination — the lane must still go
    // loud at the 600s stall threshold rather than fading as 'done'.
    const model = deriveBoard(
      inputs({
        auditDecisions: [audit({ ts: iso(STALL_THRESHOLD_MS + 60_000), tool_name: "Bash" })],
        endedAgentIds: ["cc-alpha-1"],
      }),
    );
    const lane = onlyLane(model);
    expect(lane.state).toBe("stalled");
    expect(lane.isLive).toBe(false);
  });

  it("a trailing Stop event marks the lane done even past the stall threshold", () => {
    const model = deriveBoard(
      inputs({
        auditDecisions: [
          audit({ ts: iso(STALL_THRESHOLD_MS + 120_000), tool_name: "Bash" }),
          audit({ ts: iso(STALL_THRESHOLD_MS + 60_000), tool_name: "Stop" }),
        ],
      }),
    );
    expect(onlyLane(model).state).toBe("done");
  });

  it("a pending queued decision makes the lane waiting and cross-links the decision", () => {
    // Waiting beats stalled: the agent is blocked on the human, not dead.
    const item = request({ timestamp: iso(STALL_THRESHOLD_MS + 60_000) });
    const model = deriveBoard(inputs({ queue: [item] }));
    const lane = onlyLane(model);
    expect(lane.state).toBe("waiting");
    expect(lane.inboxTarget).toEqual({
      kind: "decision",
      requestId: item.id,
      toolName: "Grep",
    });
    expect(lane.waitingSinceMs).toBe(new Date(item.timestamp).getTime());
    expect(lane.pendingCount).toBe(1);
  });

  it("an unresolved deferred prompt makes the lane waiting and cross-links its deferredKey", () => {
    const row = audit({
      kind: "deferred",
      tool_name: "AskUserQuestion",
      decided_by: "always_ask:intrinsic",
      ts: iso(120_000),
      tool_use_id: "toolu_01",
    });
    const model = deriveBoard(inputs({ auditDecisions: [row] }));
    const lane = onlyLane(model);
    expect(lane.state).toBe("waiting");
    expect(lane.inboxTarget).toEqual({
      kind: "deferred",
      deferredKey: deferredKey(row),
      toolName: "AskUserQuestion",
    });
  });

  it("a resolved deferred row does not hold the lane in waiting", () => {
    const model = deriveBoard(
      inputs({
        auditDecisions: [
          audit({ kind: "deferred", tool_name: "AskUserQuestion", resolved: true, ts: iso(120_000) }),
        ],
      }),
    );
    expect(onlyLane(model).state).toBe("working");
  });

  it("a dead session's deferred prompt cannot make it waiting (itr#464)", () => {
    const model = deriveBoard(
      inputs({
        auditDecisions: [
          audit({ kind: "deferred", tool_name: "AskUserQuestion", ts: iso(120_000) }),
        ],
        endedAgentIds: ["cc-alpha-1"],
      }),
    );
    // No blocker → falls through to the time-based states.
    expect(onlyLane(model).state).toBe("working");
  });
});

describe("deriveBoard lanes and identity", () => {
  it("gives Codex activity its own lane with the codex agent type", () => {
    const model = deriveBoard(
      inputs({
        agents: [
          agent({}),
          agent({ agent_id: "codex-bravo-1", agent_type: "codex", project: "/proj/bravo" }),
        ],
      }),
    );
    expect(model.projects).toHaveLength(2);
    const codexLane = model.projects
      .flatMap((p) => p.lanes)
      .find((l) => l.agentId === "codex-bravo-1");
    expect(codexLane?.agentType).toBe("codex");
    expect(codexLane?.project).toBe("/proj/bravo");
  });

  it("falls back to the daemon-enforced id prefix when no typed source knows the session", () => {
    const model = deriveBoard(
      inputs({ auditDecisions: [audit({ agent_id: "codex-solo-1" })] }),
    );
    expect(onlyLane(model).agentType).toBe("codex");
  });

  it("derives wave progress from Task spawns and SubagentStop completions", () => {
    const model = deriveBoard(
      inputs({
        auditDecisions: [
          audit({ tool_name: "Task", ts: iso(300_000), tool_use_id: "t1" }),
          audit({ tool_name: "Task", ts: iso(280_000), tool_use_id: "t2" }),
          audit({ tool_name: "Task", ts: iso(260_000), tool_use_id: "t3" }),
          audit({ tool_name: "SubagentStop", ts: iso(100_000) }),
          audit({ tool_name: "Read", ts: iso(30_000) }),
        ],
      }),
    );
    const lane = onlyLane(model);
    expect(lane.wave).toEqual({ spawned: 3, done: 1 });
    expect(lane.subagentsActive).toBe(2);
  });

  it("ages non-waiting lanes off the board past the visibility window", () => {
    const model = deriveBoard(
      inputs({ sessions: [session({ last_seen: iso(BOARD_WINDOW_MS + 60_000), is_live: false })] }),
    );
    expect(model.projects).toHaveLength(0);
  });

  it("keeps a waiting lane visible regardless of age", () => {
    const model = deriveBoard(
      inputs({ queue: [request({ timestamp: iso(BOARD_WINDOW_MS + 60_000) })] }),
    );
    expect(onlyLane(model).state).toBe("waiting");
  });

  it("orders lanes loudest-first within a project and counts totals", () => {
    const model = deriveBoard(
      inputs({
        auditDecisions: [
          audit({ agent_id: "cc-working", ts: iso(30_000) }),
          audit({ agent_id: "cc-stalled", ts: iso(STALL_THRESHOLD_MS + 30_000) }),
          audit({ agent_id: "cc-done", tool_name: "Stop", ts: iso(60_000) }),
        ],
        queue: [request({ agent_id: "cc-waiting" })],
      }),
    );
    expect(model.projects).toHaveLength(1);
    expect(model.projects[0].lanes.map((l) => l.agentId)).toEqual([
      "cc-stalled",
      "cc-waiting",
      "cc-working",
      "cc-done",
    ]);
    expect(model.totals).toEqual({ working: 1, waiting: 1, stalled: 1, done: 1 });
  });
});

describe("fmtDuration", () => {
  it("formats seconds, minutes, and hours", () => {
    expect(fmtDuration(38_000)).toBe("38s");
    expect(fmtDuration(600_001)).toBe("10m");
    expect(fmtDuration(3_840_000)).toBe("1h 4m");
    expect(fmtDuration(-5)).toBe("0s");
  });
});
