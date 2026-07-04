import { useEffect, useMemo, useState } from "react";
import type { AuditDecision, DecisionRequest } from "../types/protocol";
import { eventPrefix, inputSummary, orderByAge, shortProject, timeAgo } from "./queueUtils";

interface InboxProps {
  items: DecisionRequest[];
  auditDecisions: AuditDecision[];
  selectedId: string | null;
  onSelect: (id: string) => void;
  onApprove: (id: string) => void;
  onDeny: (id: string) => void;
}

export function Inbox({ items, auditDecisions, selectedId, onSelect, onApprove, onDeny }: InboxProps) {
  const [now, setNow] = useState(() => Date.now());

  useEffect(() => {
    const id = window.setInterval(() => setNow(Date.now()), 1000);
    return () => window.clearInterval(id);
  }, []);

  const ordered = useMemo(() => orderByAge(items), [items]);
  const recentAutoCount = auditDecisions.filter(
    (audit) =>
      audit.kind === "auto_approved" &&
      now - new Date(audit.ts).getTime() <= 60 * 60 * 1000,
  ).length;

  return (
    <section className="inbox" aria-label="Waiting on you inbox">
      <div className="inbox-header">
        <div>
          <h2>Inbox</h2>
          <p>
            {items.length} waiting · {recentAutoCount} auto-answered in last hour
          </p>
        </div>
      </div>

      {ordered.length === 0 ? (
        <div className="inbox-empty">No pending decisions</div>
      ) : (
        <div className="inbox-list">
          {ordered.map((item, index) => (
            <InboxRow
              key={item.id}
              item={item}
              now={now}
              isOldest={index === 0 && ordered.length > 1}
              selected={item.id === selectedId}
              onSelect={onSelect}
              onApprove={onApprove}
              onDeny={onDeny}
            />
          ))}
        </div>
      )}
    </section>
  );
}

interface InboxRowProps {
  item: DecisionRequest;
  now: number;
  isOldest: boolean;
  selected: boolean;
  onSelect: (id: string) => void;
  onApprove: (id: string) => void;
  onDeny: (id: string) => void;
}

function InboxRow({ item, now, isOldest, selected, onSelect, onApprove, onDeny }: InboxRowProps) {
  const prefix = eventPrefix(item.hook_event_name);
  const summary = inputSummary(item);
  const sessionLabel = item.terminal_session_id
    ? `term ${item.terminal_session_id.slice(0, 8)}`
    : `session ${item.agent_id.slice(0, 8)}`;

  return (
    <article
      className={`inbox-item ${isOldest ? "oldest" : ""} ${selected ? "selected" : ""}`}
      aria-current={selected}
      onClick={() => onSelect(item.id)}
    >
      <div className="inbox-item-topline">
        {prefix && <span className="event-prefix">{prefix}</span>}
        <span className="tool-name">{item.tool_name}</span>
        <span className="inbox-age">{timeAgo(item.timestamp, now)}</span>
      </div>
      <div className="inbox-route">
        <span>{shortProject(item.project)}</span>
        <span>{sessionLabel}</span>
        <span>{item.agent_id.slice(0, 20)}</span>
      </div>
      {summary && <div className="inbox-summary">{summary}</div>}
      <div className="inbox-actions" aria-label={`Actions for ${item.tool_name}`}>
        <button className="btn-approve" onClick={(e) => { e.stopPropagation(); onApprove(item.id); }}>
          Approve
        </button>
        <button className="btn-deny" onClick={(e) => { e.stopPropagation(); onDeny(item.id); }}>
          Deny
        </button>
      </div>
    </article>
  );
}
