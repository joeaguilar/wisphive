import { useState } from "react";
import type { DecisionRequest } from "../types/protocol";
import { TextModal, ConfirmModal } from "./Modal";
import { ToolContent } from "./ToolContent";
import { MarkdownText } from "./MarkdownText";
import { CopyButton } from "./CopyButton";

interface DetailViewProps {
  request: DecisionRequest;
  onApprove: (id: string, opts?: { additional_context?: string; always_allow?: boolean }) => void;
  onDeny: (id: string, message?: string) => void;
}

// Safe string extraction from unknown values
const str = (v: unknown): string => (typeof v === "string" ? v : String(v ?? ""));

export function DetailView({ request, onApprove, onDeny }: DetailViewProps) {
  const [modal, setModal] = useState<"deny-msg" | "context" | "always" | null>(null);
  const { tool_name, tool_input: rawInput, agent_id, project, timestamp, hook_event_name, event_data } = request;
  const tool_input = rawInput ?? {};
  const planContent = typeof event_data?.plan_content === "string" ? event_data.plan_content : null;

  const buildFullText = (): string => {
    const lines: string[] = [];
    lines.push(`Tool: ${tool_name}`);
    if (hook_event_name) lines.push(`Event: ${hook_event_name}`);
    lines.push(`Agent: ${agent_id}`);
    lines.push(`Project: ${project}`);
    lines.push(`Time: ${new Date(timestamp).toISOString()}`);
    if (planContent) {
      lines.push("", "--- Plan ---", planContent);
    }
    if (rawInput && Object.keys(rawInput).length > 0) {
      lines.push("", "--- Tool Input ---", JSON.stringify(rawInput, null, 2));
    }
    if (event_data && Object.keys(event_data).length > 0) {
      lines.push("", "--- Event Data ---", JSON.stringify(event_data, null, 2));
    }
    return lines.join("\n");
  };

  return (
    <div className="detail-view">
      <div className="detail-header">
        <h2>{tool_name}</h2>
        <span className="event-badge">{hook_event_name}</span>
        <CopyButton value={buildFullText} label="Copy All" title="Copy full message to clipboard" className="copy-btn-header" />
      </div>

      <div className="detail-meta">
        <div><strong>Agent:</strong> {agent_id}</div>
        <div><strong>Project:</strong> {project}</div>
        <div><strong>Time:</strong> {new Date(timestamp).toLocaleTimeString()}</div>
      </div>

      {/* ExitPlanMode: plan content with markdown rendering */}
      {planContent && (
        <div className="detail-section">
          <h3>Plan</h3>
          <MarkdownText text={planContent} />
        </div>
      )}

      {/* AskUserQuestion */}
      {Array.isArray(tool_input.questions) && (
        <div className="detail-section">
          {(tool_input.questions as Array<Record<string, unknown>>).map((q, i) => (
            <div key={i}>
              <h3>{str(q.header) || "Question"}</h3>
              <p className="question-text">{str(q.question)}</p>
              {Array.isArray(q.options) && (
                <div className="options-list">
                  {(q.options as Array<Record<string, string>>).map((opt, j) => (
                    <button key={j} className="option-btn" onClick={() => onApprove(request.id)}>
                      <strong>{opt.label}</strong>
                      {opt.description && <span> — {opt.description}</span>}
                    </button>
                  ))}
                </div>
              )}
            </div>
          ))}
        </div>
      )}

      {/* Shared tool/event content (unless handled above) */}
      {!planContent && !Array.isArray(tool_input.questions) && (
        <ToolContent
          toolName={tool_name}
          toolInput={rawInput}
          hookEventName={hook_event_name}
          eventData={event_data}
        />
      )}

      <div className="detail-actions">
        {hook_event_name === "Stop" || hook_event_name === "SubagentStop" ? (
          <button className="btn-approve" onClick={() => onApprove(request.id)}>
            Accept (Stop)
          </button>
        ) : hook_event_name === "UserPromptSubmit" || hook_event_name === "ConfigChange" ? (
          <>
            <button className="btn-approve" onClick={() => onApprove(request.id)}>Allow</button>
            <button className="btn-deny" onClick={() => onDeny(request.id)}>Block</button>
            <button className="btn-secondary" onClick={() => setModal("deny-msg")}>Block + Message</button>
          </>
        ) : (
          <>
            <button className="btn-approve" onClick={() => onApprove(request.id)}>Approve</button>
            <button className="btn-secondary" onClick={() => setModal("context")}>+ Context</button>
            <button className="btn-deny" onClick={() => onDeny(request.id)}>Deny</button>
            <button className="btn-secondary" onClick={() => setModal("deny-msg")}>Deny + Message</button>
            <button className="btn-secondary" onClick={() => setModal("always")}>Always Allow</button>
          </>
        )}
      </div>

      {modal === "deny-msg" && (
        <TextModal
          title="Deny with Message"
          placeholder="Claude will see this as feedback..."
          submitLabel="Deny"
          submitClass="btn-deny"
          onSubmit={(msg) => { onDeny(request.id, msg); setModal(null); }}
          onClose={() => setModal(null)}
        />
      )}
      {modal === "context" && (
        <TextModal
          title="Approve with Context"
          placeholder="Additional context injected into Claude's conversation..."
          submitLabel="Approve"
          onSubmit={(ctx) => { onApprove(request.id, { additional_context: ctx }); setModal(null); }}
          onClose={() => setModal(null)}
        />
      )}
      {modal === "always" && (
        <ConfirmModal
          title="Always Allow"
          message={`Always allow "${tool_name}"? This adds it to auto-approve.`}
          confirmLabel="Always Allow"
          onConfirm={() => { onApprove(request.id, { always_allow: true }); setModal(null); }}
          onClose={() => setModal(null)}
        />
      )}
    </div>
  );
}
