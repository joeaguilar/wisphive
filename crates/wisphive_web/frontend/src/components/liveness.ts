import type {
  AgentInfo,
  AgentType,
  AuditDecision,
  DecisionRequest,
  SessionSummary,
} from "../types/protocol";
import { deferredKey } from "./queueUtils";

// ── Liveness constants (spec §5.2, itr#400) ─────────────────────────
//
// STALL_THRESHOLD_MS: a lane with NO observed event for longer than this is
// declared **stalled**. 600s matches the spec's stall definition ("Stall = no
// event for 600s") and the documented pain evidence (blitz agents dying
// silently after 600s). Deliberately longer than the daemon's registry reap
// (`agent_timeout_secs`, default 300s): a reap-driven `agent_disconnected` is
// inactivity evidence, NOT termination evidence — an agent can legitimately
// think (no tool calls → no hook events) past the reap window — so disconnects
// never accelerate the stall verdict. Only positive terminal events (a `Stop`
// turn-end) mark a lane done; everything else silent past this threshold is
// loud. The flip is client-clock-driven, so a killed agent's lane turns
// stalled within 600s + the board's tick epsilon (≤5s) of its last event.
export const STALL_THRESHOLD_MS = 600_000;

// BOARD_WINDOW_MS: how long a lane stays on the board after its last observed
// event. The board is the Burst-mode surface (spec §2 — "during a working
// sitting"); it mirrors the daemon's 1-hour audit snapshot horizon, so a
// reconnecting client can honestly reconstruct every visible lane. Sessions
// silent beyond this age off the board (waiting lanes are always kept — a
// pending decision is live state regardless of age); older history lives in
// the Sessions view.
export const BOARD_WINDOW_MS = 60 * 60 * 1000;

// Events that positively mark a session's turn as finished. `Stop` fires when
// the agent completes its turn; `SessionEnd` when the session closes. A lane
// whose LATEST event is one of these is "done" (clean end), never "stalled".
const TERMINAL_EVENT_TOOLS = new Set(["Stop", "SessionEnd"]);

export type LaneState = "working" | "waiting" | "stalled" | "done";

/** Subagent wave progress derived from the event stream: `spawned` = Task
 * tool calls observed for the session, `done` = SubagentStop events. Hook
 * events carry the parent session id, so per-subagent identity is not in the
 * data — the wave aggregate is the honest derivable granularity. */
export interface WaveProgress {
  spawned: number;
  done: number;
}

/** Where a waiting lane's blocker lives in the Inbox. `decision` rows are
 * daemon-queued (answerable in-console); `deferred` rows are native prompts
 * waiting in the agent's terminal (ADR-0002). */
export type InboxTarget =
  | { kind: "decision"; requestId: string; toolName: string }
  | { kind: "deferred"; deferredKey: string; toolName: string };

export interface Lane {
  agentId: string;
  agentType: AgentType;
  project: string;
  state: LaneState;
  /** Epoch ms of the latest observed event for this session. */
  lastActivityMs: number;
  /** Current task label: tool name of the latest observed event. */
  lastToolName: string | null;
  /** nowMs - lastActivityMs (never negative). */
  silentForMs: number;
  pendingCount: number;
  isLive: boolean;
  /** Oldest blocker timestamp when waiting; null otherwise. */
  waitingSinceMs: number | null;
  /** Inbox cross-link for the oldest blocker when waiting; null otherwise. */
  inboxTarget: InboxTarget | null;
  wave: WaveProgress | null;
  /** spawned - done, clamped ≥ 0. */
  subagentsActive: number;
}

export interface ProjectLanes {
  project: string;
  lanes: Lane[];
}

export interface BoardTotals {
  working: number;
  waiting: number;
  stalled: number;
  done: number;
}

export interface BoardModel {
  projects: ProjectLanes[];
  totals: BoardTotals;
}

export interface BoardInputs {
  sessions: SessionSummary[];
  agents: AgentInfo[];
  queue: DecisionRequest[];
  auditDecisions: AuditDecision[];
  endedAgentIds: string[];
  nowMs: number;
}

const STATE_RANK: Record<LaneState, number> = {
  stalled: 0,
  waiting: 1,
  working: 2,
  done: 3,
};

interface Draft {
  agentId: string;
  project: string | null;
  agentType: AgentType | null;
  lastActivityMs: number;
  lastToolName: string | null;
  pending: DecisionRequest[];
  deferredWaiting: AuditDecision[];
  taskSpawns: number;
  subagentStops: number;
  isLive: boolean;
  sessionPendingCount: number;
}

function draftFor(map: Map<string, Draft>, agentId: string): Draft {
  let draft = map.get(agentId);
  if (!draft) {
    draft = {
      agentId,
      project: null,
      agentType: null,
      lastActivityMs: 0,
      lastToolName: null,
      pending: [],
      deferredWaiting: [],
      taskSpawns: 0,
      subagentStops: 0,
      isLive: false,
      sessionPendingCount: 0,
    };
    map.set(agentId, draft);
  }
  return draft;
}

function noteActivity(draft: Draft, ts: string, toolName: string | null) {
  const ms = new Date(ts).getTime();
  if (Number.isNaN(ms)) return;
  if (ms >= draft.lastActivityMs) {
    draft.lastActivityMs = ms;
    if (toolName !== null) draft.lastToolName = toolName;
  }
}

/** Daemon-enforced hook agent-id prefixes (`is_valid_hook_agent_id`) — the
 * honest fallback when no typed source (registry/queue/sessions) knows the
 * session. */
function agentTypeFromId(agentId: string): AgentType {
  if (agentId.startsWith("codex-")) return "codex";
  if (agentId.startsWith("red-")) return "red";
  if (agentId.startsWith("local-")) return "local_llm";
  return "claude_code";
}

/**
 * Derive the liveness board model from state the daemon already streams —
 * live agent registry, session aggregates, the decision queue, and the audit
 * event stream (every hook event lands in exactly one of the last two). Pure
 * function of its inputs + `nowMs`, so state transitions (including the
 * working→stalled clock flip) are unit-testable without timers.
 */
export function deriveBoard(inputs: BoardInputs): BoardModel {
  const { sessions, agents, queue, auditDecisions, endedAgentIds, nowMs } = inputs;
  const ended = new Set(endedAgentIds);
  const drafts = new Map<string, Draft>();

  for (const session of sessions) {
    const draft = draftFor(drafts, session.agent_id);
    draft.project = draft.project ?? session.project;
    draft.agentType = draft.agentType ?? session.agent_type;
    noteActivity(draft, session.last_seen, null);
    draft.isLive = draft.isLive || session.is_live;
    draft.sessionPendingCount = Math.max(draft.sessionPendingCount, session.pending_count);
  }

  for (const agent of agents) {
    const draft = draftFor(drafts, agent.agent_id);
    draft.project = agent.project || draft.project;
    draft.agentType = agent.agent_type;
    noteActivity(draft, agent.last_seen, null);
    draft.isLive = true;
  }

  for (const audit of auditDecisions) {
    const draft = draftFor(drafts, audit.agent_id);
    draft.project = draft.project ?? audit.project;
    noteActivity(draft, audit.ts, audit.tool_name);
    if (audit.tool_name === "Task") draft.taskSpawns += 1;
    if (audit.tool_name === "SubagentStop") draft.subagentStops += 1;
    if (audit.kind === "deferred" && !audit.resolved) {
      draft.deferredWaiting.push(audit);
    }
  }

  for (const item of queue) {
    const draft = draftFor(drafts, item.agent_id);
    draft.project = draft.project ?? item.project;
    draft.agentType = draft.agentType ?? item.agent_type;
    noteActivity(draft, item.timestamp, item.tool_name);
    draft.pending.push(item);
  }

  const lanes: Lane[] = [];
  for (const draft of drafts.values()) {
    if (draft.lastActivityMs === 0) continue;
    const isEnded = ended.has(draft.agentId);
    const silentForMs = Math.max(0, nowMs - draft.lastActivityMs);

    // Blockers, oldest first. A dead session's deferred prompt can never be
    // answered (itr#464), so an ended lane is never "waiting" on deferrals.
    const pending = [...draft.pending].sort(
      (a, b) => new Date(a.timestamp).getTime() - new Date(b.timestamp).getTime(),
    );
    const deferredRows = isEnded
      ? []
      : [...draft.deferredWaiting].sort(
          (a, b) => new Date(a.ts).getTime() - new Date(b.ts).getTime(),
        );

    let state: LaneState;
    let waitingSinceMs: number | null = null;
    let inboxTarget: InboxTarget | null = null;
    if (pending.length > 0 || deferredRows.length > 0) {
      state = "waiting";
      const oldestPendingMs =
        pending.length > 0 ? new Date(pending[0].timestamp).getTime() : Infinity;
      const oldestDeferredMs =
        deferredRows.length > 0 ? new Date(deferredRows[0].ts).getTime() : Infinity;
      if (oldestPendingMs <= oldestDeferredMs) {
        waitingSinceMs = oldestPendingMs;
        inboxTarget = {
          kind: "decision",
          requestId: pending[0].id,
          toolName: pending[0].tool_name,
        };
      } else {
        waitingSinceMs = oldestDeferredMs;
        inboxTarget = {
          kind: "deferred",
          deferredKey: deferredKey(deferredRows[0]),
          toolName: deferredRows[0].tool_name,
        };
      }
    } else if (draft.lastToolName !== null && TERMINAL_EVENT_TOOLS.has(draft.lastToolName)) {
      state = "done";
    } else if (silentForMs > STALL_THRESHOLD_MS) {
      state = "stalled";
    } else {
      state = "working";
    }

    // Visibility: waiting lanes always show (a pending decision is live state
    // no matter how old); everything else ages off past the board window.
    if (state !== "waiting" && silentForMs > BOARD_WINDOW_MS) continue;

    const wave =
      draft.taskSpawns > 0
        ? { spawned: draft.taskSpawns, done: Math.min(draft.subagentStops, draft.taskSpawns) }
        : null;

    lanes.push({
      agentId: draft.agentId,
      agentType: draft.agentType ?? agentTypeFromId(draft.agentId),
      project: draft.project ?? "",
      state,
      lastActivityMs: draft.lastActivityMs,
      lastToolName: draft.lastToolName,
      silentForMs,
      pendingCount: Math.max(draft.sessionPendingCount, pending.length),
      isLive: draft.isLive && !isEnded,
      waitingSinceMs,
      inboxTarget,
      wave,
      subagentsActive: Math.max(0, draft.taskSpawns - draft.subagentStops),
    });
  }

  // Group by project; loudest state first, then most recent activity.
  const byProject = new Map<string, Lane[]>();
  for (const lane of lanes) {
    const bucket = byProject.get(lane.project);
    if (bucket) bucket.push(lane);
    else byProject.set(lane.project, [lane]);
  }
  const projects: ProjectLanes[] = [...byProject.entries()].map(([project, projectLanes]) => ({
    project,
    lanes: projectLanes.sort(
      (a, b) => STATE_RANK[a.state] - STATE_RANK[b.state] || b.lastActivityMs - a.lastActivityMs,
    ),
  }));
  projects.sort((a, b) => {
    const aRank = STATE_RANK[a.lanes[0].state];
    const bRank = STATE_RANK[b.lanes[0].state];
    return aRank - bRank || b.lanes[0].lastActivityMs - a.lanes[0].lastActivityMs;
  });

  const totals: BoardTotals = { working: 0, waiting: 0, stalled: 0, done: 0 };
  for (const lane of lanes) totals[lane.state] += 1;

  return { projects, totals };
}

/** "38s" / "12m" / "1h 4m" — silence/age label from a millisecond duration. */
export function fmtDuration(ms: number): string {
  const seconds = Math.max(0, Math.floor(ms / 1000));
  if (seconds < 60) return `${seconds}s`;
  const minutes = Math.floor(seconds / 60);
  if (minutes < 60) return `${minutes}m`;
  return `${Math.floor(minutes / 60)}h ${minutes % 60}m`;
}
