import { describe, expect, it } from "vitest";
import type { ArtifactTouch, AuditDecision } from "../types/protocol";
import {
  BURN_WINDOW_MS,
  classifyArtifact,
  DEAD_RUN_MIN_ACTIVE_MS,
  DEAD_RUN_MIN_TOOL_CALLS,
  deriveBurn,
} from "./burnMeter";

const NOW_MS = Date.parse("2026-07-15T12:00:00Z");

function iso(msAgo: number): string {
  return new Date(NOW_MS - msAgo).toISOString();
}

function audit(overrides: Partial<AuditDecision>): AuditDecision {
  return {
    kind: "auto_approved",
    decided_by: "level:all",
    project: "/proj/alpha",
    agent_id: "cc-1",
    tool_name: "Read",
    ts: iso(30_000),
    ...overrides,
  };
}

function touch(overrides: Partial<ArtifactTouch>): ArtifactTouch {
  return {
    agent_id: "cc-1",
    project: "/proj/alpha",
    tool_name: "Write",
    tool_input: { file_path: "/proj/alpha/src/lib.rs" },
    ts: iso(20_000),
    ...overrides,
  };
}

function derive(inputs: {
  auditDecisions?: AuditDecision[];
  touches?: ArtifactTouch[];
  sessions?: Parameters<typeof deriveBurn>[0]["sessions"];
}) {
  return deriveBurn({
    sessions: inputs.sessions ?? [],
    auditDecisions: inputs.auditDecisions ?? [],
    touches: inputs.touches ?? [],
    nowMs: NOW_MS,
  });
}

function onlySession(model: ReturnType<typeof deriveBurn>) {
  expect(model.projects).toHaveLength(1);
  expect(model.projects[0].sessions).toHaveLength(1);
  return model.projects[0].sessions[0];
}

// ── classifyArtifact ────────────────────────────────────────────────

describe("classifyArtifact", () => {
  it("classifies file-writing tools as file artifacts with their path", () => {
    expect(classifyArtifact("Edit", { file_path: "/p/a.rs" })).toEqual({
      kind: "file",
      label: "/p/a.rs",
    });
    expect(classifyArtifact("Write", { file_path: "/p/b.ts" })).toEqual({
      kind: "file",
      label: "/p/b.ts",
    });
    expect(classifyArtifact("MultiEdit", { file_path: "/p/c.md" })).toEqual({
      kind: "file",
      label: "/p/c.md",
    });
    expect(classifyArtifact("NotebookEdit", { notebook_path: "/p/d.ipynb" })).toEqual({
      kind: "file",
      label: "/p/d.ipynb",
    });
  });

  it("keeps a file-write signal honest when the path is missing", () => {
    // A redacted/absent path does not erase the fact a Write ran — but the
    // label never fabricates a path.
    expect(classifyArtifact("Write", undefined)).toEqual({
      kind: "file",
      label: "(path unknown)",
    });
  });

  it("never classifies read-shaped tools or read-only Bash as artifacts (itr#549)", () => {
    expect(classifyArtifact("Read", { file_path: "/p/a.rs" })).toBeNull();
    expect(classifyArtifact("Grep", { pattern: "commit" })).toBeNull();
    expect(classifyArtifact("Bash", { command: "ls -la /p" })).toBeNull();
    expect(classifyArtifact("Bash", { command: "cat notes.md" })).toBeNull();
    // Mentioning "commit" is not committing.
    expect(classifyArtifact("Bash", { command: "git log --grep commit" })).toBeNull();
    expect(classifyArtifact("Bash", { command: "echo commit" })).toBeNull();
    expect(classifyArtifact("Bash", { command: "git status" })).toBeNull();
    expect(classifyArtifact("Bash", undefined)).toBeNull();
  });

  it("classifies a git commit invocation as a commit artifact with its subject", () => {
    expect(classifyArtifact("Bash", { command: "git commit -m 'feat: add meter'" })).toEqual({
      kind: "commit",
      label: "feat: add meter",
    });
    expect(classifyArtifact("Bash", { command: 'git commit -m "fix: y"' })).toEqual({
      kind: "commit",
      label: "fix: y",
    });
    // Pre-subcommand options are skipped, not mistaken for the subcommand.
    expect(classifyArtifact("Bash", { command: "git -C /repo commit --amend" })).toEqual({
      kind: "commit",
      label: "git commit",
    });
    // Compound commands are scanned per shell segment.
    expect(
      classifyArtifact("Bash", { command: "cargo test && git commit -m 'chore: green'" }),
    ).toEqual({ kind: "commit", label: "chore: green" });
  });
});

// ── spend proxy math ────────────────────────────────────────────────

describe("deriveBurn — spend proxy", () => {
  it("counts approved calls and derives the active span from observed events", () => {
    const model = derive({
      auditDecisions: [
        audit({ tool_name: "Read", ts: iso(300_000) }),
        audit({ tool_name: "Grep", ts: iso(200_000) }),
        audit({ tool_name: "Read", ts: iso(100_000) }),
      ],
    });
    const session = onlySession(model);
    expect(session.toolCalls).toBe(3);
    expect(session.activeSpanMs).toBe(200_000);
    expect(session.lastMs).toBe(NOW_MS - 100_000);
    expect(session.artifacts).toEqual([]);
  });

  it("deduplicates a call served by both the audit stream and the touches", () => {
    const ts = iso(60_000);
    const model = derive({
      auditDecisions: [audit({ tool_name: "Write", ts })],
      touches: [touch({ tool_name: "Write", ts })],
    });
    const session = onlySession(model);
    expect(session.toolCalls).toBe(1);
    // …but the touch still classifies the artifact the audit row can't.
    expect(session.artifacts).toHaveLength(1);
  });

  it("counts human-approved touches the audit stream never carries", () => {
    const model = derive({ touches: [touch({})] });
    expect(onlySession(model).toolCalls).toBe(1);
  });

  it("denied and deferred events prove activity (span) but are not spend", () => {
    const model = derive({
      auditDecisions: [
        audit({ kind: "denied", tool_name: "Bash", ts: iso(500_000) }),
        audit({ kind: "deferred", tool_name: "AskUserQuestion", ts: iso(400_000) }),
        audit({ tool_name: "Read", ts: iso(100_000) }),
      ],
    });
    const session = onlySession(model);
    expect(session.toolCalls).toBe(1);
    expect(session.activeSpanMs).toBe(400_000);
  });

  it("ignores events outside the burn window and drops event-less sessions", () => {
    const model = derive({
      auditDecisions: [audit({ agent_id: "cc-old", ts: iso(BURN_WINDOW_MS + 1) })],
      touches: [touch({ agent_id: "cc-old", ts: iso(BURN_WINDOW_MS + 60_000) })],
    });
    expect(model.projects).toHaveLength(0);
    expect(model.totals.sessions).toBe(0);
  });
});

// ── artifact aggregation ────────────────────────────────────────────

describe("deriveBurn — artifacts", () => {
  it("aggregates repeated writes to one file into one artifact with a count", () => {
    const model = derive({
      touches: [
        touch({ tool_name: "Edit", ts: iso(90_000) }),
        touch({ tool_name: "Edit", ts: iso(60_000) }),
        touch({ tool_name: "Write", ts: iso(30_000) }),
      ],
    });
    const session = onlySession(model);
    expect(session.artifacts).toHaveLength(1);
    expect(session.artifacts[0]).toMatchObject({
      kind: "file",
      label: "/proj/alpha/src/lib.rs",
      count: 3,
      toolName: "Write", // newest signal wins the tool attribution
    });
    expect(session.artifactCalls).toBe(3);
  });

  it("lists commits and files as separate signals, newest first", () => {
    const model = derive({
      touches: [
        touch({ ts: iso(120_000) }),
        touch({
          tool_name: "Bash",
          tool_input: { command: "git commit -m 'feat: ship'" },
          ts: iso(30_000),
        }),
      ],
    });
    const session = onlySession(model);
    expect(session.artifacts.map((a) => a.kind)).toEqual(["commit", "file"]);
    expect(session.artifacts[0].label).toBe("feat: ship");
  });

  it("counts a read-only Bash touch as spend but never as an artifact (itr#549)", () => {
    const model = derive({
      touches: [touch({ tool_name: "Bash", tool_input: { command: "ls -la" } })],
    });
    const session = onlySession(model);
    expect(session.toolCalls).toBe(1);
    expect(session.artifacts).toEqual([]);
  });
});

// ── dead-run threshold boundaries ───────────────────────────────────

describe("deriveBurn — dead-run alert", () => {
  /** N approved read-shaped audit events spanning exactly `spanMs`. */
  function spend(calls: number, spanMs: number, agentId = "cc-dead"): AuditDecision[] {
    const rows: AuditDecision[] = [];
    for (let i = 0; i < calls; i++) {
      const offset = calls === 1 ? 0 : Math.round((i * spanMs) / (calls - 1));
      rows.push(audit({ agent_id: agentId, tool_name: "Read", ts: iso(spanMs + 60_000 - offset) }));
    }
    return rows;
  }

  it("trips exactly at the documented floor and window (boundary)", () => {
    const model = derive({
      auditDecisions: spend(DEAD_RUN_MIN_TOOL_CALLS, DEAD_RUN_MIN_ACTIVE_MS),
    });
    const session = onlySession(model);
    expect(session.toolCalls).toBe(DEAD_RUN_MIN_TOOL_CALLS);
    expect(session.activeSpanMs).toBe(DEAD_RUN_MIN_ACTIVE_MS);
    expect(session.deadRun).toBe(true);
    expect(model.totals.deadRuns).toBe(1);
  });

  it("does not trip one call below the floor", () => {
    const model = derive({
      auditDecisions: spend(DEAD_RUN_MIN_TOOL_CALLS - 1, DEAD_RUN_MIN_ACTIVE_MS),
    });
    expect(onlySession(model).deadRun).toBe(false);
  });

  it("does not trip one millisecond below the threshold window", () => {
    const model = derive({
      auditDecisions: spend(DEAD_RUN_MIN_TOOL_CALLS, DEAD_RUN_MIN_ACTIVE_MS - 1),
    });
    const session = onlySession(model);
    expect(session.activeSpanMs).toBe(DEAD_RUN_MIN_ACTIVE_MS - 1);
    expect(session.deadRun).toBe(false);
  });

  it("a single artifact signal disarms the alert", () => {
    const model = derive({
      auditDecisions: spend(DEAD_RUN_MIN_TOOL_CALLS, DEAD_RUN_MIN_ACTIVE_MS),
      touches: [touch({ agent_id: "cc-dead", ts: iso(30_000) })],
    });
    expect(onlySession(model).deadRun).toBe(false);
  });

  it("orders dead runs loudest across projects", () => {
    const model = derive({
      auditDecisions: [
        // Productive, more recent session in another project.
        audit({ agent_id: "cc-ok", project: "/proj/bravo", ts: iso(10_000) }),
        ...spend(DEAD_RUN_MIN_TOOL_CALLS, DEAD_RUN_MIN_ACTIVE_MS),
      ],
      touches: [touch({ agent_id: "cc-ok", project: "/proj/bravo", ts: iso(10_000) })],
    });
    expect(model.projects[0].project).toBe("/proj/alpha");
    expect(model.projects[0].sessions[0].deadRun).toBe(true);
  });
});
