import { useState } from "react";
import type { DecisionRequest } from "../types/protocol";
import { TextModal, ConfirmModal } from "./Modal";
import { ToolContent } from "./ToolContent";
import { MarkdownText } from "./MarkdownText";
import { CopyButton } from "./CopyButton";

interface DetailViewProps {
  request: DecisionRequest;
  onApprove: (
    id: string,
    opts?: { additional_context?: string; always_allow?: boolean; updated_input?: unknown },
  ) => void;
  onDeny: (id: string, message?: string) => void;
}

// Safe string extraction from unknown values
const str = (v: unknown): string => (typeof v === "string" ? v : String(v ?? ""));

// Mirror of the TUI's build_ask_answer (wisphive_tui/src/input.rs): embed the
// chosen option as the answer to its question so the daemon forwards it to
// Claude as updatedInput (itr#250). PreToolUse cannot pass the answer back — it
// must ride the PermissionRequest response's updatedInput.
const buildAskAnswer = (
  toolInput: Record<string, unknown>,
  question: string,
  answer: string,
): Record<string, unknown> => ({ ...toolInput, answers: { [question]: answer } });

export function DetailView({ request, onApprove, onDeny }: DetailViewProps) {
  const [modal, setModal] = useState<"deny-msg" | "context" | "always" | null>(null);
  const { tool_name, tool_input: rawInput, agent_id, project, timestamp, hook_event_name, event_data } = request;
  const tool_input = rawInput ?? {};
  // plan_content is either the plan text (string) or, when extraction failed,
  // a structured { error, path } object the hook set so we render an explicit
  // failure rather than an empty view (itr#253).
  const pc = event_data?.plan_content;
  const planContent = typeof pc === "string" ? pc : null;
  const planError =
    pc && typeof pc === "object" && "error" in (pc as object)
      ? (pc as { error?: unknown; path?: unknown })
      : null;
  const isPlanTool = tool_name === "ExitPlanMode";
  const isAskQuestion = Array.isArray(tool_input.questions);

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

      {/* ExitPlanMode: structured extraction failure (itr#253) */}
      {planError && (
        <div className="detail-section detail-section-error">
          <h3>Plan unavailable</h3>
          <p className="plan-error">
            Wisphive could not read the plan from the transcript
            {str(planError.error) ? `: ${str(planError.error)}` : "."}
            {str(planError.path) ? ` (path: ${str(planError.path)})` : ""}
          </p>
        </div>
      )}

      {/* ExitPlanMode limitation note (itr#249): Claude's native plan prompt
          offers richer choices, but the hook layer only carries allow/deny. */}
      {isPlanTool && (
        <p className="plan-note">
          Claude's native plan prompt offers richer choices (auto-accept edits, review each
          edit, keep planning). Wisphive gates it at the hook layer, which only supports
          <strong> Approve</strong> (accept the plan; Claude proceeds in its current mode) or
          <strong> Deny</strong> (reject; Claude keeps planning). The finer options aren't
          expressible through the gate.
        </p>
      )}

      {/* AskUserQuestion */}
      {isAskQuestion && (
        <div className="detail-section">
          {(tool_input.questions as Array<Record<string, unknown>>).map((q, i) => (
            <div key={i}>
              <h3>{str(q.header) || "Question"}</h3>
              <p className="question-text">{str(q.question)}</p>
              {Array.isArray(q.options) && (
                <div className="options-list">
                  {(q.options as Array<Record<string, string>>).map((opt, j) => (
                    <button
                      key={j}
                      className="option-btn"
                      onClick={() =>
                        onApprove(request.id, {
                          updated_input: buildAskAnswer(tool_input, str(q.question), str(opt.label)),
                        })
                      }
                    >
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
      {!planContent && !planError && !isAskQuestion && (
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
        ) : isAskQuestion ? (
          // The option buttons above ARE the approve path (they carry the
          // answer). A bare Approve here would resolve with no answer (itr#250),
          // so offer only Deny / Deny + Message.
          <>
            <button className="btn-deny" onClick={() => onDeny(request.id)}>Deny</button>
            <button className="btn-secondary" onClick={() => setModal("deny-msg")}>Deny + Message</button>
          </>
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
