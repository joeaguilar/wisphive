import { memo } from "react";
import type { AuditDecision, AuditDecisionKind } from "../types/protocol";
import { shortProject, timeAgo } from "./queueUtils";

interface AutoAnswerFeedProps {
  decisions: AuditDecision[];
  now: number;
}

// Human-facing label per audit kind. These are decisions Wisphive resolved
// *without* the human touching the queue — the "decided without you" feed
// (spec §5.1). Rendered as plain React text nodes; tool_name / decided_by /
// project are agent- or config-derived and untrusted, so never HTML.
const KIND_LABEL: Record<AuditDecisionKind, string> = {
  auto_approved: "auto-approved",
  deferred: "deferred",
  denied: "denied",
};

/**
 * Live+recent feed of auto-answered / deferred / denied decisions. Consumes the
 * `AuditDecision` stream + connect snapshot (itr#434) already merged and sorted
 * newest-first in `useWisphive`. Deliberately no search/pagination — that is the
 * filed fast-follow (itr#436 non-goal).
 */
export const AutoAnswerFeed = memo(function AutoAnswerFeed({ decisions, now }: AutoAnswerFeedProps) {
  return (
    <section className="auto-feed" aria-label="Decided without you">
      {decisions.length === 0 ? (
        <div className="auto-feed-empty">Nothing decided without you yet</div>
      ) : (
        <ul className="auto-feed-list">
          {decisions.map((decision, index) => (
            <AutoAnswerRow key={`${auditRowKey(decision)}-${index}`} decision={decision} now={now} />
          ))}
        </ul>
      )}
    </section>
  );
});

interface AutoAnswerRowProps {
  decision: AuditDecision;
  now: number;
}

function AutoAnswerRow({ decision, now }: AutoAnswerRowProps) {
  const sessionLabel = decision.terminal_session_id
    ? `term ${decision.terminal_session_id.slice(0, 8)}`
    : `session ${decision.agent_id.slice(0, 8)}`;

  return (
    <li className={`auto-feed-item kind-${decision.kind}`}>
      <div className="auto-feed-topline">
        <span className={`auto-feed-kind kind-${decision.kind}`}>{KIND_LABEL[decision.kind]}</span>
        <span className="tool-name">{decision.tool_name}</span>
        <span className="auto-feed-age">{timeAgo(decision.ts, now)}</span>
      </div>
      <div className="auto-feed-route">
        <span>{shortProject(decision.project)}</span>
        <span>{sessionLabel}</span>
        {decision.decided_by && <span className="auto-feed-rule">{decision.decided_by}</span>}
      </div>
    </li>
  );
}

function auditRowKey(decision: AuditDecision): string {
  return [decision.kind, decision.ts, decision.agent_id, decision.tool_name].join("|");
}
