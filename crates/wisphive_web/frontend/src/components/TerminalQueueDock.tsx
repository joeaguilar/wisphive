import { useState } from "react";
import type { DecisionRequest } from "../types/protocol";
import { DetailView } from "./DetailView";
import { eventPrefix, inputSummary, timeAgo } from "./queueUtils";

interface Props {
  terminalPending: DecisionRequest[];
  otherPendingCount: number;
  onApprove: (id: string, opts?: { additional_context?: string; always_allow?: boolean }) => void;
  onDeny: (id: string, message?: string) => void;
  onJumpToQueue: () => void;
}

export function TerminalQueueDock({
  terminalPending,
  otherPendingCount,
  onApprove,
  onDeny,
  onJumpToQueue,
}: Props) {
  const [requestedExpandedId, setRequestedExpandedId] = useState<string | null>(null);
  const expandedId = terminalPending.some((r) => r.id === requestedExpandedId) ? requestedExpandedId : null;

  if (terminalPending.length === 0 && otherPendingCount === 0) return null;

  return (
    <div className="terminal-queue-dock">
      <div className="terminal-queue-dock-header">
        <span className="terminal-queue-dock-title">
          Pending for this terminal · {terminalPending.length}
        </span>
        {otherPendingCount > 0 && (
          <button
            type="button"
            className="terminal-queue-other-chip"
            onClick={onJumpToQueue}
            title="Jump to the Queue view"
          >
            {otherPendingCount} other pending →
          </button>
        )}
      </div>
      {terminalPending.map((row) => {
        const prefix = eventPrefix(row.hook_event_name);
        const summary = inputSummary(row);
        const isExpanded = expandedId === row.id;
        return (
          <div
            key={row.id}
            className={`terminal-queue-dock-row${isExpanded ? " expanded" : ""}`}
          >
            <div
              className="terminal-queue-dock-row-head"
              onClick={() => setRequestedExpandedId(isExpanded ? null : row.id)}
            >
              {prefix && <span className="event-prefix">{prefix}</span>}
              <span className="tool-name">{row.tool_name}</span>
              {summary && (
                <span className="terminal-queue-dock-summary">{summary}</span>
              )}
              <span className="time-ago">{timeAgo(row.timestamp)}</span>
              <div className="terminal-queue-dock-inline-actions">
                <button
                  className="btn-approve btn-sm"
                  onClick={(e) => {
                    e.stopPropagation();
                    onApprove(row.id);
                  }}
                >
                  Approve
                </button>
                <button
                  className="btn-deny btn-sm"
                  onClick={(e) => {
                    e.stopPropagation();
                    onDeny(row.id);
                  }}
                >
                  Deny
                </button>
              </div>
            </div>
            {isExpanded && (
              <div className="terminal-queue-dock-detail">
                <DetailView
                  request={row}
                  onApprove={onApprove}
                  onDeny={onDeny}
                />
              </div>
            )}
          </div>
        );
      })}
    </div>
  );
}
