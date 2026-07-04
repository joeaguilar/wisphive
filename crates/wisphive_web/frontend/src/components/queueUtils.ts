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

// Parse a deferred tool_input into a shape both the row summary and the full
// detail view can render. Never throws on unexpected shapes; falls back to a
// pretty-printed JSON blob, then to `none` for null/empty input.
export function parseDeferredPrompt(
  input: Record<string, unknown> | null | undefined,
): DeferredPrompt {
  if (input == null || typeof input !== "object") return { kind: "none" };

  // AskUserQuestion: { questions: [{ question, header?, options: [{label, description?}] }] }
  if (Array.isArray(input.questions)) {
    const questions: DeferredQuestion[] = [];
    for (const raw of input.questions as unknown[]) {
      if (raw == null || typeof raw !== "object") continue;
      const q = raw as Record<string, unknown>;
      const question = typeof q.question === "string" ? q.question : "";
      const header = typeof q.header === "string" ? q.header : undefined;
      const options: DeferredQuestion["options"] = [];
      if (Array.isArray(q.options)) {
        for (const optRaw of q.options as unknown[]) {
          if (optRaw == null || typeof optRaw !== "object") continue;
          const opt = optRaw as Record<string, unknown>;
          const label = typeof opt.label === "string" ? opt.label : "";
          const description = typeof opt.description === "string" ? opt.description : undefined;
          options.push({ label, description });
        }
      }
      if (question || options.length > 0) questions.push({ question, header, options });
    }
    if (questions.length > 0) return { kind: "questions", questions };
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
  input: Record<string, unknown> | null | undefined,
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

// Oldest-first ordering for the inbox. Shared by the Inbox render and the
// App keyboard navigation so j/k/y/n operate on exactly the on-screen order.
export function orderByAge(items: DecisionRequest[]): DecisionRequest[] {
  return [...items].sort(
    (a, b) => new Date(a.timestamp).getTime() - new Date(b.timestamp).getTime(),
  );
}
