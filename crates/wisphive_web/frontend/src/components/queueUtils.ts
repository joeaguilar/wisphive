import type { DecisionRequest } from "../types/protocol";

export function timeAgo(timestamp: string, nowMs = Date.now()): string {
  const seconds = Math.floor(
    (nowMs - new Date(timestamp).getTime()) / 1000,
  );
  if (seconds < 60) return `${seconds}s`;
  if (seconds < 3600) return `${Math.floor(seconds / 60)}m`;
  return `${Math.floor(seconds / 3600)}h`;
}

// Event type prefix badges matching TUI indicators.
export function eventPrefix(eventName: string): string {
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

// Extract a brief summary of tool input for the queue list.
export function inputSummary(item: DecisionRequest): string | null {
  const input = item.tool_input;
  if (!input) return null;

  if (typeof input.command === "string") {
    const cmd = input.command as string;
    return cmd.length > 80 ? cmd.slice(0, 77) + "..." : cmd;
  }
  if (typeof input.file_path === "string") return input.file_path as string;
  if (item.tool_name === "Write" && typeof input.file_path === "string") return input.file_path as string;
  if (typeof input.pattern === "string") return `/${input.pattern as string}/`;

  if (Array.isArray(input.questions)) {
    const q = input.questions[0] as Record<string, unknown> | undefined;
    if (q && typeof q.question === "string") {
      const text = q.question as string;
      return text.length > 80 ? text.slice(0, 77) + "..." : text;
    }
  }

  if (item.event_data && typeof item.event_data.last_assistant_message === "string") {
    const msg = item.event_data.last_assistant_message as string;
    return msg.length > 80 ? msg.slice(0, 77) + "..." : msg;
  }

  if (item.event_data && typeof item.event_data.plan_content === "string") {
    return "Plan ready for review";
  }
  return null;
}

export function shortProject(project: string): string {
  const parts = project.split("/");
  return parts[parts.length - 1] || project;
}

// Oldest-first ordering for the inbox. Shared by the Inbox render and the
// App keyboard navigation so j/k/y/n operate on exactly the on-screen order.
export function orderByAge(items: DecisionRequest[]): DecisionRequest[] {
  return [...items].sort(
    (a, b) => new Date(a.timestamp).getTime() - new Date(b.timestamp).getTime(),
  );
}
