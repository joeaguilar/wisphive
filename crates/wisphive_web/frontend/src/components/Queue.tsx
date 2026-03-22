import type { DecisionRequest } from "../types/protocol";

interface QueueProps {
  items: DecisionRequest[];
  selectedId: string | null;
  onSelect: (id: string) => void;
  onApprove: (id: string) => void;
  onDeny: (id: string) => void;
}

function timeAgo(timestamp: string): string {
  const seconds = Math.floor(
    (Date.now() - new Date(timestamp).getTime()) / 1000,
  );
  if (seconds < 60) return `${seconds}s`;
  if (seconds < 3600) return `${Math.floor(seconds / 60)}m`;
  return `${Math.floor(seconds / 3600)}h`;
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
      {items.map((item) => (
        <div
          key={item.id}
          className={`queue-item ${selectedId === item.id ? "selected" : ""}`}
          onClick={() => onSelect(item.id)}
        >
          <div className="queue-item-header">
            <span className="tool-name">{item.tool_name}</span>
            <span className="event-type">{item.hook_event_name}</span>
            <span className="time-ago">{timeAgo(item.timestamp)}</span>
          </div>
          <div className="queue-item-meta">
            <span className="agent-id">{item.agent_id}</span>
          </div>
          {selectedId === item.id && (
            <div className="queue-item-actions">
              <button
                className="btn-approve"
                onClick={(e) => {
                  e.stopPropagation();
                  onApprove(item.id);
                }}
              >
                Approve
              </button>
              <button
                className="btn-deny"
                onClick={(e) => {
                  e.stopPropagation();
                  onDeny(item.id);
                }}
              >
                Deny
              </button>
            </div>
          )}
        </div>
      ))}
    </div>
  );
}
