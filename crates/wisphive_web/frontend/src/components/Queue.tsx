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

// Event type prefix badges matching TUI indicators
function eventPrefix(eventName: string): string {
  switch (eventName) {
    case "PermissionRequest": return "P";
    case "Elicitation": return "E";
    case "Stop": case "SubagentStop": return "S";
    case "UserPromptSubmit": return "U";
    case "ConfigChange": return "C";
    case "TeammateIdle": return "T";
    case "TaskCompleted": return "D";
    default: return "";
  }
}

// Extract a brief summary of tool input for the queue list
function inputSummary(item: DecisionRequest): string | null {
  const input = item.tool_input;
  if (!input) return null;

  // Bash: show command
  if (typeof input.command === "string") {
    const cmd = input.command as string;
    return cmd.length > 80 ? cmd.slice(0, 77) + "..." : cmd;
  }
  // Edit: show file path
  if (typeof input.file_path === "string") return input.file_path as string;
  // Write: show file path
  if (item.tool_name === "Write" && typeof input.file_path === "string") return input.file_path as string;
  // Read/Grep/Glob: show path or pattern
  if (typeof input.pattern === "string") return `/${input.pattern as string}/`;
  // AskUserQuestion: show question
  if (Array.isArray(input.questions)) {
    const q = input.questions[0] as Record<string, unknown> | undefined;
    if (q && typeof q.question === "string") {
      const text = q.question as string;
      return text.length > 80 ? text.slice(0, 77) + "..." : text;
    }
  }
  // Stop: show last message
  if (item.event_data && typeof item.event_data.last_assistant_message === "string") {
    const msg = item.event_data.last_assistant_message as string;
    return msg.length > 80 ? msg.slice(0, 77) + "..." : msg;
  }
  // Plan
  if (item.event_data && typeof item.event_data.plan_content === "string") {
    return "Plan ready for review";
  }
  return null;
}

// Extract short project name from path
function shortProject(project: string): string {
  const parts = project.split("/");
  return parts[parts.length - 1] || project;
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
