import { cleanup, fireEvent, render, screen, within } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import type { ComponentProps } from "react";
import { Burn } from "./Burn";
import { DEAD_RUN_MIN_ACTIVE_MS, DEAD_RUN_MIN_TOOL_CALLS } from "./burnMeter";
import type { ArtifactTouch, AuditDecision } from "../types/protocol";

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

function touch(overrides: Partial<ArtifactTouch>): ArtifactTouch {
  return {
    agent_id: "cc-alpha-1",
    project: "/proj/alpha",
    tool_name: "Write",
    tool_input: { file_path: "/proj/alpha/src/lib.rs" },
    ts: iso(20_000),
    ...overrides,
  };
}

/** Audit rows tripping the documented dead-run thresholds exactly. */
function deadRunRows(agentId = "cc-dead-1"): AuditDecision[] {
  const rows: AuditDecision[] = [];
  for (let i = 0; i < DEAD_RUN_MIN_TOOL_CALLS; i++) {
    const offset = Math.round((i * DEAD_RUN_MIN_ACTIVE_MS) / (DEAD_RUN_MIN_TOOL_CALLS - 1));
    rows.push(
      audit({
        agent_id: agentId,
        tool_name: "Grep",
        ts: iso(DEAD_RUN_MIN_ACTIVE_MS + 60_000 - offset),
      }),
    );
  }
  return rows;
}

function renderBurn(props: Partial<ComponentProps<typeof Burn>> = {}) {
  return render(
    <Burn
      sessions={[]}
      auditDecisions={[]}
      burnTouches={[]}
      onLoad={vi.fn()}
      {...props}
    />,
  );
}

describe("Burn", () => {
  beforeEach(() => {
    vi.useFakeTimers();
    vi.setSystemTime(new Date(NOW));
  });

  afterEach(() => {
    cleanup();
    vi.useRealTimers();
  });

  it("shows the labelled activity proxy alongside the session's artifact list", () => {
    renderBurn({
      auditDecisions: [
        audit({ ts: iso(300_000) }),
        audit({ tool_name: "Grep", ts: iso(200_000) }),
      ],
      burnTouches: [
        touch({ ts: iso(100_000) }),
        touch({
          tool_name: "Bash",
          tool_input: { command: "git commit -m 'feat: ship meter'" },
          ts: iso(50_000),
        }),
      ],
    });

    const tile = screen.getByRole("listitem", { name: /cc-alpha-1/ });
    // The spend number is LABELLED as a proxy, with the honesty note.
    expect(within(tile).getByText("activity proxy")).toBeInTheDocument();
    expect(within(tile).getByText(/4 tool calls · 4m active/)).toBeInTheDocument();
    expect(within(tile).getByText("token spend not observable")).toBeInTheDocument();
    // Artifacts: the full file path and the commit subject are rendered.
    expect(within(tile).getByText("/proj/alpha/src/lib.rs")).toBeInTheDocument();
    expect(within(tile).getByText("feat: ship meter")).toBeInTheDocument();
    expect(within(tile).getByText("commit")).toBeInTheDocument();
    // No dead-run alert on a productive session.
    expect(within(tile).queryByRole("alert")).toBeNull();
  });

  it("trips a loud dead-run alert on spend with zero artifacts", () => {
    renderBurn({ auditDecisions: deadRunRows() });

    const tile = screen.getByRole("listitem", { name: /cc-dead-1 — dead run/ });
    expect(tile.className).toContain("dead-run");
    const alert = within(tile).getByRole("alert");
    expect(alert).toHaveTextContent(
      `DEAD RUN — ${DEAD_RUN_MIN_TOOL_CALLS} tool calls over 10m with zero artifacts`,
    );
    expect(screen.getByRole("status")).toHaveTextContent("1 dead runs");
  });

  it("keeps the FULL artifact list reachable behind the expander (no truncation)", () => {
    const files = Array.from({ length: 9 }, (_, i) =>
      touch({ tool_input: { file_path: `/proj/alpha/src/file-${i}.rs` }, ts: iso(9_000 - i) }),
    );
    renderBurn({ burnTouches: files });

    // Collapsed: 6 rows + the expander naming the full count.
    expect(document.querySelectorAll(".burn-artifact")).toHaveLength(6);
    const expander = screen.getByRole("button", { name: "Show all 9 artifacts" });
    fireEvent.click(expander);
    expect(document.querySelectorAll(".burn-artifact")).toHaveLength(9);
    for (let i = 0; i < 9; i++) {
      expect(screen.getByText(`/proj/alpha/src/file-${i}.rs`)).toBeInTheDocument();
    }
    // And it collapses back.
    fireEvent.click(screen.getByRole("button", { name: "Show fewer" }));
    expect(document.querySelectorAll(".burn-artifact")).toHaveLength(6);
  });

  it("is a read-only state mirror: zero write affordances (spec §5 hard constraint)", () => {
    renderBurn({
      auditDecisions: [...deadRunRows(), audit({})],
      burnTouches: Array.from({ length: 9 }, (_, i) =>
        touch({ tool_input: { file_path: `/f-${i}` }, ts: iso(9_000 - i) }),
      ),
    });
    // The ONLY buttons on the whole meter are artifact-list expanders.
    const buttons = screen.getAllByRole("button");
    expect(buttons.map((b) => b.textContent)).toEqual(["Show all 9 artifacts"]);
    expect(
      screen.queryByRole("button", { name: /stop|kill|throttle|retarget|pause|spawn|deny/i }),
    ).toBeNull();
    // And the read-only + honesty contract is stated on the surface.
    expect(screen.getByText(/Read-only state mirror/)).toBeInTheDocument();
    expect(screen.getByText(/never stops or throttles/)).toBeInTheDocument();
  });

  it("renders the empty state and polls the pull feeds", () => {
    const onLoad = vi.fn();
    renderBurn({ onLoad });
    expect(screen.getByText("No gated agent activity in the last hour")).toBeInTheDocument();
    expect(onLoad).toHaveBeenCalledTimes(1);
    vi.advanceTimersByTime(15_000);
    expect(onLoad).toHaveBeenCalledTimes(2);
  });

  it("groups sessions by project with the full path rendered", () => {
    renderBurn({
      auditDecisions: [
        audit({}),
        audit({ agent_id: "codex-b-1", project: "/proj/bravo", ts: iso(40_000) }),
      ],
    });
    expect(screen.getByText("alpha")).toBeInTheDocument();
    expect(screen.getByText("/proj/alpha")).toBeInTheDocument();
    expect(screen.getByText("bravo")).toBeInTheDocument();
    // Codex sessions carry their own type badge (honest id-prefix fallback).
    const codexTile = screen.getByRole("listitem", { name: /codex-b-1/ });
    expect(within(codexTile).getByText("codex")).toBeInTheDocument();
  });
});
