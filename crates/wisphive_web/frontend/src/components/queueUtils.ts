import type { AuditDecision, DecisionRequest } from "../types/protocol";
import { parseToolInput } from "./toolInput";

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
  const parsed = parseToolInput(item.tool_name, item.tool_input);
  const eventData = isRecord(item.event_data) ? item.event_data : null;

  if (parsed.kind === "bash" && parsed.input.command) {
    const cmd = parsed.input.command;
    return cmd.length > 80 ? cmd.slice(0, 77) + "..." : cmd;
  }
  if (parsed.fields.filePath) return parsed.fields.filePath;
  if (parsed.fields.pattern) return `/${parsed.fields.pattern}/`;

  if (parsed.kind === "ask-user-question" && parsed.input) {
    const text = parsed.input.questions[0]?.question;
    if (text) return text.length > 80 ? text.slice(0, 77) + "..." : text;
  }

  if (typeof eventData?.last_assistant_message === "string") {
    const msg = eventData.last_assistant_message;
    return msg.length > 80 ? msg.slice(0, 77) + "..." : msg;
  }

  if (typeof eventData?.plan_content === "string") {
    return "Plan ready for review";
  }
  return null;
}

// ── Deferred native-prompt extraction ───────────────────────────────
//
// A deferred AuditDecision carries the (already-redacted, itr#89) `tool_input`
// of the native prompt so the inbox can show the literal question/plan instead
// of just a tool name. `tool_input` is UNTRUSTED agent output — every consumer
// renders it as React text nodes (never dangerouslySetInnerHTML).

export interface DeferredQuestion {
  question: string;
  header?: string;
  options: { label: string; description?: string }[];
}

export type DeferredPrompt =
  | { kind: "questions"; questions: DeferredQuestion[] }
  | { kind: "plan"; plan: string }
  | { kind: "raw"; text: string }
  | { kind: "none" };

function isRecord(value: unknown): value is Record<string, unknown> {
  return typeof value === "object" && value !== null && !Array.isArray(value);
}

// Parse a deferred tool_input into a shape both the row summary and the full
// detail view can render. Never throws on unexpected shapes; falls back to a
// pretty-printed JSON blob, then to `none` for null/empty input.
export function parseDeferredPrompt(
  input: unknown,
): DeferredPrompt {
  if (!isRecord(input)) return { kind: "none" };

  // AskUserQuestion: { questions: [{ question, header?, options: [{label, description?}] }] }
  const parsed = parseToolInput("AskUserQuestion", input);
  if (parsed.kind === "ask-user-question" && parsed.input) {
    return {
      kind: "questions",
      questions: parsed.input.questions.map((question) => ({
        question: question.question,
        header: question.header ?? undefined,
        options: question.options.map((option) => ({
          label: option.label,
          description: option.description ?? undefined,
        })),
      })),
    };
  }

  // ExitPlanMode: { plan: "<markdown>" }
  if (typeof input.plan === "string" && input.plan.trim().length > 0) {
    return { kind: "plan", plan: input.plan };
  }

  // Elicitation / unknown shape: honest pretty-printed JSON fallback.
  const keys = Object.keys(input);
  if (keys.length === 0) return { kind: "none" };
  try {
    return { kind: "raw", text: JSON.stringify(input, null, 2) };
  } catch {
    return { kind: "none" };
  }
}

// One-line, truncatable summary for the collapsed deferred row. The full,
// untruncated prompt stays reachable via DeferredDetailView (no single-place
// truncation — project no-truncation rule).
export function deferredPromptSummary(
  input: unknown,
): string | null {
  const prompt = parseDeferredPrompt(input);
  const clip = (s: string) => (s.length > 80 ? s.slice(0, 77) + "..." : s);
  switch (prompt.kind) {
    case "questions": {
      const first = prompt.questions[0];
      if (first?.question) return clip(first.question);
      const label = first?.options[0]?.label;
      return label ? clip(label) : null;
    }
    case "plan":
      return clip(prompt.plan.trim().split("\n")[0] || "Plan ready for review");
    case "raw":
      return clip(prompt.text.replace(/\s+/g, " ").trim());
    case "none":
      return null;
  }
}

export function shortProject(project: string): string {
  const parts = project.split("/");
  return parts[parts.length - 1] || project;
}

// Stable identity for a deferred AuditDecision row. Shared by the Inbox (row
// expansion state) and the liveness board (waiting-lane → inbox deep-link,
// itr#400) so a board cross-link targets exactly the row the Inbox renders.
export function deferredKey(d: AuditDecision): string {
  return `${d.ts}|${d.agent_id}|${d.tool_name}|${d.terminal_session_id ?? ""}`;
}

// Oldest-first ordering for the inbox. Shared by the Inbox render and the
// App keyboard navigation so j/k/y/n operate on exactly the on-screen order.
export function orderByAge(items: DecisionRequest[]): DecisionRequest[] {
  return [...items].sort(
    (a, b) => new Date(a.timestamp).getTime() - new Date(b.timestamp).getTime(),
  );
}
