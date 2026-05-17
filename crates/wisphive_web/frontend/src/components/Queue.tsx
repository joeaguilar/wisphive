import type { DecisionRequest } from "../types/protocol";
import { eventPrefix, inputSummary, shortProject, timeAgo } from "./queueUtils";

interface QueueProps {
  items: DecisionRequest[];
  selectedId: string | null;
  onSelect: (id: string) => void;
  onApprove: (id: string) => void;
  onDeny: (id: string) => void;
}

export function Queue({
  items,
  selectedId,
  onSelect,
  onApprove,
  onDeny,
}: QueueProps) {
  if (items.length === 0) {
    return (
      <div className="queue-empty">
        <p>No pending decisions</p>
      </div>
    );
  }

  return (
    <div className="queue">
      {items.map((item) => {
        const prefix = eventPrefix(item.hook_event_name);
        const summary = inputSummary(item);
        return (
          <div
            key={item.id}
            className={`queue-item ${selectedId === item.id ? "selected" : ""}`}
            onClick={() => onSelect(item.id)}
          >
            <div className="queue-item-header">
              {prefix && <span className="event-prefix">{prefix}</span>}
              <span className="tool-name">{item.tool_name}</span>
              <span className="project-name">{shortProject(item.project)}</span>
              <span className="time-ago">{timeAgo(item.timestamp)}</span>
            </div>
            {summary && (
              <div className="queue-item-summary">{summary}</div>
            )}
            <div className="queue-item-meta">
              <span className="agent-id">{item.agent_id.slice(0, 20)}</span>
            </div>
            {selectedId === item.id && (
              <div className="queue-item-actions">
                <button
                  className="btn-approve"
                  onClick={(e) => { e.stopPropagation(); onApprove(item.id); }}
                >
                  Approve
                </button>
                <button
                  className="btn-deny"
                  onClick={(e) => { e.stopPropagation(); onDeny(item.id); }}
                >
                  Deny
                </button>
              </div>
            )}
          </div>
        );
      })}
    </div>
  );
}
