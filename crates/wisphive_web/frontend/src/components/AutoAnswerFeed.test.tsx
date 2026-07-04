import { cleanup, render, screen, within } from "@testing-library/react";
import { afterEach, describe, expect, it } from "vitest";
import { AutoAnswerFeed } from "./AutoAnswerFeed";
import type { AuditDecision } from "../types/protocol";

const NOW = new Date("2026-07-04T12:00:00Z").getTime();

function audit(overrides: Partial<AuditDecision>): AuditDecision {
  return {
    kind: "auto_approved",
    decided_by: "level:all",
    project: "/Users/josefaguilar/AI_Projects/wisphive",
    agent_id: "cc-agent-alpha",
    tool_name: "Read",
    ts: "2026-07-04T11:59:00Z",
    ...overrides,
  };
}

describe("AutoAnswerFeed", () => {
  afterEach(cleanup);

  it("renders an auto-answered event with its decided_by rule visible", () => {
    render(<AutoAnswerFeed decisions={[audit({})]} now={NOW} />);

    const feed = screen.getByLabelText("Decided without you");
    expect(within(feed).getByText("auto-approved")).toBeInTheDocument();
    expect(within(feed).getByText("Read")).toBeInTheDocument();
    // The deciding rule (decided_by) must be visible — this is the whole point
    // of the audit feed.
    expect(within(feed).getByText("level:all")).toBeInTheDocument();
    // Project (short) and session are shown for routing context.
    expect(within(feed).getByText("wisphive")).toBeInTheDocument();
    expect(within(feed).getByText("session cc-agent")).toBeInTheDocument();
  });

  it("labels deferred and denied kinds and shows their rules", () => {
    const decisions: AuditDecision[] = [
      audit({
        kind: "deferred",
        decided_by: "always_ask:intrinsic",
        tool_name: "AskUserQuestion",
        agent_id: "cc-defer",
      }),
      audit({
        kind: "denied",
        decided_by: "tool_rules:Bash:deny_pattern",
        tool_name: "Bash",
        agent_id: "cc-deny",
      }),
    ];

    render(<AutoAnswerFeed decisions={decisions} now={NOW} />);
    const feed = screen.getByLabelText("Decided without you");

    expect(within(feed).getByText("deferred")).toBeInTheDocument();
    expect(within(feed).getByText("always_ask:intrinsic")).toBeInTheDocument();
    expect(within(feed).getByText("denied")).toBeInTheDocument();
    expect(within(feed).getByText("tool_rules:Bash:deny_pattern")).toBeInTheDocument();
  });

  it("prefers a terminal session label when present", () => {
    render(
      <AutoAnswerFeed
        decisions={[audit({ terminal_session_id: "abcdef12-3456-4789-8abc-def012345678" })]}
        now={NOW}
      />,
    );

    const feed = screen.getByLabelText("Decided without you");
    expect(within(feed).getByText("term abcdef12")).toBeInTheDocument();
  });

  it("renders untrusted tool names as text, never as HTML", () => {
    render(
      <AutoAnswerFeed
        decisions={[audit({ tool_name: "<img src=x onerror=alert(1)>" })]}
        now={NOW}
      />,
    );

    const feed = screen.getByLabelText("Decided without you");
    // The payload is present as literal text and produced no <img> element.
    expect(within(feed).getByText("<img src=x onerror=alert(1)>")).toBeInTheDocument();
    expect(feed.querySelector("img")).toBeNull();
  });

  it("shows an explicit empty-state when nothing was decided", () => {
    render(<AutoAnswerFeed decisions={[]} now={NOW} />);
    expect(screen.getByText("Nothing decided without you yet")).toBeInTheDocument();
  });

  it("renders every decision in the feed (no pagination)", () => {
    const decisions = Array.from({ length: 25 }, (_, i) =>
      audit({ agent_id: `cc-${i}`, ts: `2026-07-04T11:${String(i).padStart(2, "0")}:00Z` }),
    );

    const { container } = render(<AutoAnswerFeed decisions={decisions} now={NOW} />);
    expect(container.querySelectorAll(".auto-feed-item")).toHaveLength(25);
  });
});
