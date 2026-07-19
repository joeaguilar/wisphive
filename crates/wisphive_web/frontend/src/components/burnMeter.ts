import type {
  AgentType,
  ArtifactTouch,
  AuditDecision,
  JsonValue,
  SessionSummary,
} from "../types/protocol";
import { knownAgentType } from "../types/protocol";

// ── Burn-meter constants (spec §5.4, itr#402) ───────────────────────
//
// HONESTY CONTRACT: wisphive gates tool calls — it never sees model tokens or
// credits. The "spend" side of this meter is therefore an ACTIVITY PROXY
// (gated tool calls + active wall-clock), labelled as such on the tile per the
// spec's data-source rule: "if per-token cost is not observable, degrade to
// event-volume proxy and say so on the tile — never fabricate numbers."

// BURN_WINDOW_MS: the meter's horizon. Mirrors BOARD_WINDOW_MS (liveness.ts)
// and the daemon's 1-hour audit snapshot, so a reconnecting client can
// honestly reconstruct every visible meter row. The daemon's query_burn
// window (server.rs BURN_WINDOW_SECS) matches.
export const BURN_WINDOW_MS = 60 * 60 * 1000;

// Dead-run alert thresholds (deliberately conservative — the alert is for the
// documented brutal case, "blew through the credits … and got nothing in
// return", not for flagging a session that is merely reading before writing):
//
// DEAD_RUN_MIN_TOOL_CALLS: the spend-proxy floor. A run must have burned at
// least this many APPROVED gated calls before it can be called dead — fewer
// is a warm-up, not a burn.
export const DEAD_RUN_MIN_TOOL_CALLS = 10;

// DEAD_RUN_MIN_ACTIVE_MS: the threshold window. The run's observed activity
// span (first→last event) must cover at least this much wall-clock with ZERO
// artifact signals. 10 minutes of sustained spend with nothing to show is the
// alert condition; long research phases below the call floor never trip it.
export const DEAD_RUN_MIN_ACTIVE_MS = 10 * 60 * 1000;

export type ArtifactKind = "file" | "commit";

/** One concrete artifact signal derived from the approved-call stream:
 * a file write (Edit/Write/MultiEdit/NotebookEdit) or a `git commit`
 * invocation. Labels are agent-influenced untrusted data — inert text only. */
export interface Artifact {
  kind: ArtifactKind;
  /** File path, or the commit message subject (fallback "git commit"). */
  label: string;
  /** Tool that produced the newest signal for this label. */
  toolName: string;
  /** How many approved calls produced this same signal (e.g. a file edited
   * three times aggregates to one artifact with count 3). */
  count: number;
  /** Epoch ms of the newest call for this signal. */
  lastTsMs: number;
}

export interface BurnSession {
  agentId: string;
  agentType: AgentType;
  project: string;
  /** Spend proxy: APPROVED gated tool calls observed in the window. */
  toolCalls: number;
  /** Spend proxy: observed activity span (first→last event) in the window. */
  activeSpanMs: number;
  /** Epoch ms of the newest observed event in the window. */
  lastMs: number;
  /** Artifact signals in the window, newest first. Empty = nothing to show. */
  artifacts: Artifact[];
  /** Total artifact-producing calls (sum of per-artifact counts). */
  artifactCalls: number;
  /** Spend above both floors with zero artifact signals (spec §5.4 alert). */
  deadRun: boolean;
}

export interface ProjectBurn {
  project: string;
  sessions: BurnSession[];
}

export interface BurnTotals {
  sessions: number;
  deadRuns: number;
  artifactCalls: number;
}

export interface BurnModel {
  projects: ProjectBurn[];
  totals: BurnTotals;
}

export interface BurnInputs {
  sessions: SessionSummary[];
  auditDecisions: AuditDecision[];
  touches: ArtifactTouch[];
  nowMs: number;
}

/** Tools whose approved calls ARE a file-write artifact signal by name. */
const FILE_ARTIFACT_TOOLS = new Set(["Edit", "Write", "MultiEdit", "NotebookEdit"]);

/** `git` global options that take a separate value argument and may precede
 * the subcommand (`git -C /repo commit …`). */
const GIT_VALUE_OPTIONS = new Set(["-C", "-c", "--git-dir", "--work-tree", "--namespace"]);

/**
 * Classify one approved tool call into an artifact signal, or null.
 *
 * Honest by construction (itr#549): a read-only Bash command that merely
 * MENTIONS a file or the word "commit" is NOT an artifact. Only two signals
 * qualify:
 *   - a file-writing tool call (Edit/Write/MultiEdit/NotebookEdit) — the tool
 *     name alone is the signal; the redacted input supplies the path;
 *   - a Bash call whose git SUBCOMMAND is `commit` (parsed per shell segment,
 *     skipping `git`'s pre-subcommand options), so `git log --grep commit`
 *     or `echo commit` never count.
 */
export function classifyArtifact(
  toolName: string,
  toolInput: JsonValue | undefined,
): { kind: ArtifactKind; label: string } | null {
  if (FILE_ARTIFACT_TOOLS.has(toolName)) {
    const path =
      readStringProp(toolInput, "file_path") ?? readStringProp(toolInput, "notebook_path");
    return { kind: "file", label: path ?? "(path unknown)" };
  }
  if (toolName === "Bash") {
    const command = readStringProp(toolInput, "command");
    if (!command) return null;
    // Split into shell segments so `cargo test && git commit -m 'x'` is seen.
    for (const segment of command.split(/&&|\|\||;|\|/)) {
      const subject = gitCommitSubject(segment);
      if (subject !== null) return { kind: "commit", label: subject };
    }
  }
  return null;
}

/** If `segment` is a `git … commit …` invocation, return its `-m` subject
 * (first line) or "git commit"; otherwise null. */
function gitCommitSubject(segment: string): string | null {
  const tokens = segment.trim().split(/\s+/);
  if (tokens[0] !== "git") return null;
  let i = 1;
  while (i < tokens.length) {
    const token = tokens[i];
    if (token.startsWith("-")) {
      // A pre-subcommand git option; value-taking ones consume the next token.
      i += GIT_VALUE_OPTIONS.has(token) ? 2 : 1;
      continue;
    }
    if (token !== "commit") return null; // some other git subcommand
    const message = segment.match(/(?:^|\s)(?:-m|--message)[\s=]+(?:"([^"]*)"|'([^']*)'|(\S+))/);
    const subject = (message?.[1] ?? message?.[2] ?? message?.[3])?.split("\n")[0].trim();
    return subject || "git commit";
  }
  return null; // bare `git` / options only
}

function readStringProp(value: JsonValue | undefined, key: string): string | null {
  if (typeof value !== "object" || value === null || Array.isArray(value)) return null;
  const prop = value[key];
  return typeof prop === "string" && prop.length > 0 ? prop : null;
}

/** Daemon-enforced hook agent-id prefixes — same honest fallback the liveness
 * board uses when no typed source knows the session. */
function agentTypeFromId(agentId: string): AgentType {
  if (agentId.startsWith("codex-")) return "codex";
  if (agentId.startsWith("red-")) return "red";
  if (agentId.startsWith("local-")) return "local_llm";
  return "claude_code";
}

interface Draft {
  agentId: string;
  project: string | null;
  agentType: AgentType | null;
  toolCalls: number;
  firstMs: number;
  lastMs: number;
  /** `agent\u0000tool\u0000tsMs` keys already counted — an auto-approved
   * Write arrives via BOTH the audit stream and the decision-log touches;
   * spend must count it once. */
  counted: Set<string>;
  artifacts: Map<string, Artifact>;
  artifactCalls: number;
}

function draftFor(map: Map<string, Draft>, agentId: string): Draft {
  let draft = map.get(agentId);
  if (!draft) {
    draft = {
      agentId,
      project: null,
      agentType: null,
      toolCalls: 0,
      firstMs: Infinity,
      lastMs: 0,
      counted: new Set(),
      artifacts: new Map(),
      artifactCalls: 0,
    };
    map.set(agentId, draft);
  }
  return draft;
}

function noteSpan(draft: Draft, ms: number) {
  if (ms < draft.firstMs) draft.firstMs = ms;
  if (ms > draft.lastMs) draft.lastMs = ms;
}

/**
 * Derive the burn-meter model from state the daemon already streams (audit
 * decisions, session aggregates) plus the query_burn artifact-candidate rows.
 * Pure function of its inputs + `nowMs`, so the spend-proxy math, artifact
 * classification, and the dead-run threshold boundary are unit-testable
 * without timers (mirrors liveness.ts deriveBoard).
 */
export function deriveBurn(inputs: BurnInputs): BurnModel {
  const { sessions, auditDecisions, touches, nowMs } = inputs;
  const windowStart = nowMs - BURN_WINDOW_MS;
  const drafts = new Map<string, Draft>();

  // Audit stream: every hook decision event. Any event proves activity (span);
  // only APPROVED calls count as spend — a denied call did not run and a
  // deferred prompt is waiting, not burning.
  for (const audit of auditDecisions) {
    const ms = new Date(audit.ts).getTime();
    if (Number.isNaN(ms) || ms < windowStart || ms > nowMs) continue;
    const draft = draftFor(drafts, audit.agent_id);
    draft.project = draft.project ?? audit.project;
    noteSpan(draft, ms);
    if (audit.kind === "auto_approved") {
      const key = `${audit.agent_id}\u0000${audit.tool_name}\u0000${ms}`;
      if (!draft.counted.has(key)) {
        draft.counted.add(key);
        draft.toolCalls += 1;
      }
    }
  }

  // Decision-log touches: approved artifact-candidate calls (covers
  // human-approved calls the audit stream never carries) + the artifact
  // classification source (the audit stream strips tool_input).
  for (const touch of touches) {
    const ms = new Date(touch.ts).getTime();
    if (Number.isNaN(ms) || ms < windowStart || ms > nowMs) continue;
    const draft = draftFor(drafts, touch.agent_id);
    draft.project = draft.project ?? touch.project;
    noteSpan(draft, ms);
    const key = `${touch.agent_id}\u0000${touch.tool_name}\u0000${ms}`;
    if (!draft.counted.has(key)) {
      draft.counted.add(key);
      draft.toolCalls += 1;
    }
    const signal = classifyArtifact(touch.tool_name, touch.tool_input);
    if (signal) {
      draft.artifactCalls += 1;
      const artifactKey = `${signal.kind}\u0000${signal.label}`;
      const existing = draft.artifacts.get(artifactKey);
      if (existing) {
        existing.count += 1;
        if (ms > existing.lastTsMs) {
          existing.lastTsMs = ms;
          existing.toolName = touch.tool_name;
        }
      } else {
        draft.artifacts.set(artifactKey, {
          kind: signal.kind,
          label: signal.label,
          toolName: touch.tool_name,
          count: 1,
          lastTsMs: ms,
        });
      }
    }
  }

  // Session aggregates supply the typed agent_type / project fallbacks; they
  // never create meter rows (no observed event in the window = no burn row).
  for (const session of sessions) {
    const draft = drafts.get(session.agent_id);
    if (!draft) continue;
    // Narrow query-boundary labels (itr#607): unknown agent_type labels
    // (itr#562 refusal rows) fall through to the agent-id heuristic below.
    draft.agentType = draft.agentType ?? knownAgentType(session.agent_type);
    draft.project = draft.project ?? session.project;
  }

  const burnSessions: BurnSession[] = [];
  for (const draft of drafts.values()) {
    if (draft.lastMs === 0) continue;
    const activeSpanMs = Math.max(0, draft.lastMs - draft.firstMs);
    const artifacts = [...draft.artifacts.values()].sort((a, b) => b.lastTsMs - a.lastTsMs);
    const deadRun =
      draft.toolCalls >= DEAD_RUN_MIN_TOOL_CALLS &&
      activeSpanMs >= DEAD_RUN_MIN_ACTIVE_MS &&
      artifacts.length === 0;
    burnSessions.push({
      agentId: draft.agentId,
      agentType: draft.agentType ?? agentTypeFromId(draft.agentId),
      project: draft.project ?? "",
      toolCalls: draft.toolCalls,
      activeSpanMs,
      lastMs: draft.lastMs,
      artifacts,
      artifactCalls: draft.artifactCalls,
      deadRun,
    });
  }

  // Group by project; dead runs loudest, then most recent activity.
  const rank = (s: BurnSession) => (s.deadRun ? 0 : 1);
  const byProject = new Map<string, BurnSession[]>();
  for (const session of burnSessions) {
    const bucket = byProject.get(session.project);
    if (bucket) bucket.push(session);
    else byProject.set(session.project, [session]);
  }
  const projects: ProjectBurn[] = [...byProject.entries()].map(([project, projectSessions]) => ({
    project,
    sessions: projectSessions.sort((a, b) => rank(a) - rank(b) || b.lastMs - a.lastMs),
  }));
  projects.sort((a, b) => {
    const aTop = a.sessions[0];
    const bTop = b.sessions[0];
    return rank(aTop) - rank(bTop) || bTop.lastMs - aTop.lastMs;
  });

  const totals: BurnTotals = { sessions: 0, deadRuns: 0, artifactCalls: 0 };
  for (const session of burnSessions) {
    totals.sessions += 1;
    if (session.deadRun) totals.deadRuns += 1;
    totals.artifactCalls += session.artifactCalls;
  }

  return { projects, totals };
}
