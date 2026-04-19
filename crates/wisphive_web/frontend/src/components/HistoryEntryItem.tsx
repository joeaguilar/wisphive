import type { HistoryEntry } from "../types/protocol";
import { ToolContent } from "./ToolContent";

interface HistoryEntryItemProps {
  entry: HistoryEntry;
  expanded: boolean;
  onToggle: () => void;
  onAgentClick?: (agentId: string) => void;
  showAgent?: boolean;
  showProjectMeta?: boolean;
}

function decisionBadge(decision: string) {
  const d = decision.replace(/"/g, "");
  const cls = d === "approve" ? "badge-approve" : d === "deny" ? "badge-deny" : "badge-defer";
  return <span className={`decision-badge ${cls}`}>{d.toUpperCase()}</span>;
}

function formatTime(ts: string) {
  return new Date(ts).toLocaleString();
}

export function HistoryEntryItem({
  entry,
  expanded,
  onToggle,
  onAgentClick,
  showAgent = true,
  showProjectMeta = true,
}: HistoryEntryItemProps) {
  return (
    <div className="history-item">
      <div
        className="history-item-row history-item-toggle"
        onClick={onToggle}
        role="button"
        tabIndex={0}
        aria-expanded={expanded}
        onKeyDown={(e) => {
          if (e.key === "Enter" || e.key === " ") {
            e.preventDefault();
            onToggle();
          }
        }}
      >
        {decisionBadge(entry.decision)}
        <span className="tool-name">{entry.tool_name}</span>
        {showAgent && (
          onAgentClick ? (
            <span
              className="agent-link"
              onClick={(e) => { e.stopPropagation(); onAgentClick(entry.agent_id); }}
            >
              {entry.agent_id.slice(0, 16)}
            </span>
          ) : (
            <span className="agent-id-small">{entry.agent_id.slice(0, 16)}</span>
          )
        )}
        <span className="time-ago">{formatTime(entry.resolved_at)}</span>
        {entry.tool_result && <span className="result-indicator">+</span>}
      </div>
      {expanded && (
        <div className="history-detail">
          {showProjectMeta && (
            <div className="detail-meta">
              <div><strong>Agent:</strong> {entry.agent_id}</div>
              <div><strong>Project:</strong> {entry.project}</div>
              <div><strong>Requested:</strong> {formatTime(entry.requested_at)}</div>
              <div><strong>Resolved:</strong> {formatTime(entry.resolved_at)}</div>
            </div>
          )}
          <ToolContent
            toolName={entry.tool_name}
            toolInput={entry.tool_input}
            hookEventName={entry.hook_event_name}
            toolResult={entry.tool_result}
          />
        </div>
      )}
    </div>
  );
}
