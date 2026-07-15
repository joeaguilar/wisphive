import { useEffect, useMemo, useState } from "react";
import type {
  AgentInfo,
  AgentType,
  AuditDecision,
  DecisionRequest,
  SessionSummary,
} from "../types/protocol";
import {
  deriveBoard,
  fmtDuration,
  type InboxTarget,
  type Lane,
  type LaneState,
} from "./liveness";
import { shortProject } from "./queueUtils";

interface BoardProps {
  sessions: SessionSummary[];
  agents: AgentInfo[];
  queue: DecisionRequest[];
  auditDecisions: AuditDecision[];
  endedAgentIds: string[];
  /** Refresh the session aggregates (query_sessions). Called on mount and on
   * a slow poll so `last_seen` stays fresh between live events. */
  onLoad: () => void;
  /** Deep-link a waiting lane's blocker into the Inbox (the ONLY action this
   * board offers — spec §5 hard constraint: state mirror, not a steering
   * wheel; no start/stop/retarget controls). */
  onOpenInbox: (target: InboxTarget) => void;
}

// Lane-state clock: 5s buckets. Stall detection is 600s-scale, so per-second
// re-derivation over the (≤10k-row) audit array would be waste; 5s keeps the
// working→stalled flip within the documented 600s+epsilon.
const DERIVE_TICK_MS = 5_000;
// Session-aggregate poll. Live events stream in between polls; this only
// refreshes decision_log-backed last_seen/is_live.
const SESSIONS_POLL_MS = 20_000;

const STATE_LABEL: Record<LaneState, string> = {
  working: "working",
  waiting: "waiting on you",
  stalled: "stalled",
  done: "turn ended",
};

const AGENT_TYPE_LABEL: Record<AgentType, string> = {
  claude_code: "claude",
  codex: "codex",
  red: "red",
  local_llm: "local",
};

/**
 * Agent liveness board (spec §5.2, itr#400): per project → session lanes with
 * working / waiting / stalled / done states, current task label, silence age,
 * and derivable subagent wave progress. Read-only by design — the only
 * affordance is the waiting lane's cross-link into the Inbox.
 */
export function Board({
  sessions,
  agents,
  queue,
  auditDecisions,
  endedAgentIds,
  onLoad,
  onOpenInbox,
}: BoardProps) {
  const [nowMs, setNowMs] = useState(() => Date.now());

  useEffect(() => {
    const id = window.setInterval(() => setNowMs(Date.now()), DERIVE_TICK_MS);
    return () => window.clearInterval(id);
  }, []);

  useEffect(() => {
    onLoad();
    const id = window.setInterval(onLoad, SESSIONS_POLL_MS);
    return () => window.clearInterval(id);
  }, [onLoad]);

  const model = useMemo(
    () =>
      deriveBoard({
        sessions,
        agents,
        queue,
        auditDecisions,
        endedAgentIds,
        nowMs,
      }),
    [sessions, agents, queue, auditDecisions, endedAgentIds, nowMs],
  );

  const { totals } = model;
  const empty = model.projects.length === 0;

  return (
    <section className="board" aria-label="Agent liveness board">
      <div className="board-header">
        <h2>Agent board</h2>
        <p className="board-counts" role="status" aria-live="polite">
          {totals.working} working · {totals.waiting} waiting · {totals.stalled} stalled ·{" "}
          {totals.done} done
        </p>
        <p className="board-note">
          Read-only state mirror — decisions are answered in the Inbox, sessions steered in their
          terminals.
        </p>
      </div>

      {empty ? (
        <div className="board-empty">No agent activity in the last hour</div>
      ) : (
        model.projects.map((group) => (
          <div className="board-project" key={group.project}>
            <header className="board-project-header">
              {/* Short name for scanning; the full path stays reachable. */}
              <span className="board-project-name" title={group.project}>
                {shortProject(group.project)}
              </span>
              <span className="board-project-path">{group.project}</span>
            </header>
            <div className="board-lanes" role="list" aria-label={`Lanes for ${group.project}`}>
              {group.lanes.map((lane) => (
                <BoardLane key={lane.agentId} lane={lane} onOpenInbox={onOpenInbox} />
              ))}
            </div>
          </div>
        ))
      )}
    </section>
  );
}

function BoardLane({ lane, onOpenInbox }: { lane: Lane; onOpenInbox: (t: InboxTarget) => void }) {
  const silence = fmtDuration(lane.silentForMs);
  const inboxTarget = lane.inboxTarget;
  // Redundant Signal Rule: state is carried by the dot COLOR, the word, and
  // the row class together — never hue alone.
  const stateText =
    lane.state === "stalled" ? `stalled · silent ${silence}` : STATE_LABEL[lane.state];

  return (
    <article
      role="listitem"
      className={`board-lane state-${lane.state}`}
      aria-label={`${lane.agentId} — ${stateText}`}
    >
      <span className={`lane-dot ${lane.state}`} aria-hidden="true" />
      <div className="lane-main">
        <div className="lane-topline">
          {/* Full agent id (no slice); CSS may ellipsize but the title keeps
              the untruncated value reachable, and Sessions carries it too. */}
          <span className="lane-agent" title={lane.agentId}>
            {lane.agentId}
          </span>
          <span className={`lane-type-badge type-${lane.agentType}`}>
            {AGENT_TYPE_LABEL[lane.agentType]}
          </span>
          <span className={`lane-state-label ${lane.state}`}>{stateText}</span>
        </div>
        <div className="lane-meta">
          {/* Agent-derived tool names are untrusted — inert text nodes only. */}
          {lane.lastToolName && <span className="lane-task">last: {lane.lastToolName}</span>}
          <span className="lane-age">{silence} ago</span>
          {lane.wave && (
            <span className="lane-wave">
              subagents {lane.wave.done}/{lane.wave.spawned} done
              {lane.subagentsActive > 0 && ` · ${lane.subagentsActive} active`}
            </span>
          )}
          {lane.pendingCount > 0 && (
            <span className="lane-pending">{lane.pendingCount} pending</span>
          )}
        </div>
      </div>
      {inboxTarget && (
        <button type="button" className="lane-inbox-link" onClick={() => onOpenInbox(inboxTarget)}>
          Answer in Inbox →
        </button>
      )}
    </article>
  );
}
