import { useEffect, useMemo, useState } from "react";
import type { ArtifactTouch, AuditDecision, SessionSummary } from "../types/protocol";
import type { AgentType } from "../types/protocol";
import { deriveBurn, type Artifact, type BurnSession } from "./burnMeter";
import { fmtDuration } from "./liveness";
import { shortProject } from "./queueUtils";

interface BurnProps {
  sessions: SessionSummary[];
  auditDecisions: AuditDecision[];
  burnTouches: ArtifactTouch[];
  /** Refresh the meter's pull feeds (query_burn + query_sessions). Called on
   * mount and on a slow poll; the audit stream keeps spend live in between. */
  onLoad: () => void;
}

// Meter clock: 5s buckets (mirrors the Board). Dead-run detection is
// 10-minute-scale, so per-second re-derivation would be waste.
const DERIVE_TICK_MS = 5_000;
// Pull-feed poll (query_burn artifact rows + session aggregates).
const POLL_MS = 15_000;

// Artifact rows shown before the per-session expander. The FULL list is
// always reachable via "Show all N" (no-truncation rule) — this only bounds
// the initial paint.
const ARTIFACTS_COLLAPSED = 6;

const AGENT_TYPE_LABEL: Record<AgentType, string> = {
  claude_code: "claude",
  codex: "codex",
  red: "red",
  local_llm: "local",
};

/**
 * Burn meter (spec §5.4, itr#402): per session — an honest ACTIVITY PROXY for
 * spend (gated tool calls + active wall-clock; wisphive never sees model
 * tokens, and the tile says so) next to the concrete artifact signals the
 * session produced (files written, `git commit` invocations), with a loud
 * dead-run alert when spend accrues past the documented thresholds with zero
 * artifacts.
 *
 * HARD CONSTRAINT (spec §5): a read-only state mirror with ZERO write
 * affordances — it never stops, throttles, or retargets anything. The ONLY
 * interactive elements are per-session "Show all" artifact-list expanders.
 * Enforced by enumeration test like the Board's read-only contract.
 *
 * Agent ids, paths, and commit subjects are agent-influenced untrusted data —
 * rendered exclusively as inert React text nodes.
 */
export function Burn({ sessions, auditDecisions, burnTouches, onLoad }: BurnProps) {
  const [nowMs, setNowMs] = useState(() => Date.now());

  useEffect(() => {
    const id = window.setInterval(() => setNowMs(Date.now()), DERIVE_TICK_MS);
    return () => window.clearInterval(id);
  }, []);

  useEffect(() => {
    onLoad();
    const id = window.setInterval(onLoad, POLL_MS);
    return () => window.clearInterval(id);
  }, [onLoad]);

  const model = useMemo(
    () => deriveBurn({ sessions, auditDecisions, touches: burnTouches, nowMs }),
    [sessions, auditDecisions, burnTouches, nowMs],
  );

  const { totals } = model;
  const empty = model.projects.length === 0;

  return (
    <section className="burn" aria-label="Burn meter">
      <div className="burn-header">
        <h2>Burn meter</h2>
        <p className="burn-counts" role="status" aria-live="polite">
          {totals.sessions} sessions · {totals.deadRuns} dead runs · {totals.artifactCalls}{" "}
          artifact calls — last hour
        </p>
        <p className="burn-note">
          Read-only state mirror — spend is an <strong>activity proxy</strong> (gated tool calls ·
          active wall-clock): wisphive cannot see model tokens and never fabricates numbers. The
          meter alerts; it never stops or throttles a session.
        </p>
      </div>

      {empty ? (
        <div className="burn-empty">No gated agent activity in the last hour</div>
      ) : (
        model.projects.map((group) => (
          <div className="burn-project" key={group.project}>
            <header className="burn-project-header">
              <span className="burn-project-name" title={group.project}>
                {shortProject(group.project)}
              </span>
              <span className="burn-project-path">{group.project}</span>
            </header>
            <div className="burn-tiles" role="list" aria-label={`Burn for ${group.project}`}>
              {group.sessions.map((session) => (
                <BurnTile key={session.agentId} session={session} nowMs={nowMs} />
              ))}
            </div>
          </div>
        ))
      )}
    </section>
  );
}

function BurnTile({ session, nowMs }: { session: BurnSession; nowMs: number }) {
  const [expanded, setExpanded] = useState(false);
  const overflow = session.artifacts.length > ARTIFACTS_COLLAPSED;
  const shown = expanded ? session.artifacts : session.artifacts.slice(0, ARTIFACTS_COLLAPSED);

  return (
    <article
      role="listitem"
      className={`burn-tile ${session.deadRun ? "dead-run" : ""}`.trim()}
      aria-label={`${session.agentId} — ${session.deadRun ? "dead run" : `${session.artifacts.length} artifacts`}`}
    >
      <div className="burn-topline">
        {/* Full agent id (no slice); CSS may ellipsize but the title keeps
            the untruncated value reachable. */}
        <span className="burn-agent" title={session.agentId}>
          {session.agentId}
        </span>
        <span className={`burn-type-badge type-${session.agentType}`}>
          {AGENT_TYPE_LABEL[session.agentType]}
        </span>
        <span className="burn-age">{fmtDuration(Math.max(0, nowMs - session.lastMs))} ago</span>
      </div>

      {/* The spend proxy, labelled as a proxy right where the number is. */}
      <p className="burn-spend">
        <span className="burn-proxy-label">activity proxy</span>
        <span className="burn-spend-value">
          {session.toolCalls} tool {session.toolCalls === 1 ? "call" : "calls"} ·{" "}
          {fmtDuration(session.activeSpanMs)} active
        </span>
        <span className="burn-proxy-note">token spend not observable</span>
      </p>

      {session.deadRun && (
        <p className="burn-dead-alert" role="alert">
          DEAD RUN — {session.toolCalls} tool calls over {fmtDuration(session.activeSpanMs)} with
          zero artifacts
        </p>
      )}

      {session.artifacts.length === 0 ? (
        !session.deadRun && <p className="burn-no-artifacts">no artifact signals yet</p>
      ) : (
        <>
          <p className="burn-artifact-summary">
            {session.artifacts.length} artifact {session.artifacts.length === 1 ? "signal" : "signals"}
            {" · "}
            {session.artifactCalls} {session.artifactCalls === 1 ? "call" : "calls"}
          </p>
          <ul className="burn-artifacts" aria-label={`Artifacts from ${session.agentId}`}>
            {shown.map((artifact) => (
              <ArtifactRow key={`${artifact.kind} ${artifact.label}`} artifact={artifact} />
            ))}
          </ul>
          {overflow && (
            <button
              type="button"
              className="burn-expand"
              aria-expanded={expanded}
              onClick={() => setExpanded((v) => !v)}
            >
              {expanded
                ? "Show fewer"
                : `Show all ${session.artifacts.length} artifacts`}
            </button>
          )}
        </>
      )}
    </article>
  );
}

function ArtifactRow({ artifact }: { artifact: Artifact }) {
  return (
    <li className="burn-artifact">
      <span className={`artifact-kind ${artifact.kind}`}>
        {artifact.kind === "commit" ? "commit" : "file"}
      </span>
      {/* Untrusted label (path / commit subject) — inert text node only. */}
      <code className="artifact-label">{artifact.label}</code>
      {artifact.count > 1 && <span className="artifact-count">×{artifact.count}</span>}
      <span className="artifact-tool">{artifact.toolName}</span>
    </li>
  );
}
