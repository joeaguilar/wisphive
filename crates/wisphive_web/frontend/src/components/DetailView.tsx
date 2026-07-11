import { useState } from "react";
import type { AuditDecision, DecisionRequest } from "../types/protocol";
import { TextModal, ConfirmModal } from "./Modal";
import { ToolContent } from "./ToolContent";
import { MarkdownText } from "./MarkdownText";
import { CopyButton } from "./CopyButton";
import { parseDeferredPrompt, shortProject } from "./queueUtils";
import { parseToolInput } from "./toolInput";

interface DetailViewProps {
  request: DecisionRequest;
  onApprove: (id: string, opts?: { additional_context?: string; always_allow?: boolean }) => void;
  onDeny: (id: string, message?: string) => void;
}

// Safe string extraction from unknown values
const str = (v: unknown): string => (typeof v === "string" ? v : String(v ?? ""));

function isRecord(value: unknown): value is Record<string, unknown> {
  return typeof value === "object" && value !== null && !Array.isArray(value);
}

function hasContent(value: unknown): boolean {
  if (value === null || value === undefined) return false;
  if (typeof value !== "object") return true;
  return Object.keys(value).length > 0;
}

function formatJson(value: unknown): string {
  const json = JSON.stringify(value, null, 2);
  return typeof json === "string" ? json : String(value ?? "");
}

interface DeferredDetailProps {
  decision: AuditDecision;
  onFocusTerminal: (terminalSessionId: string) => void;
}

/**
 * Read-only detail for an always-deferred native prompt (AskUserQuestion /
 * ExitPlanMode / Elicitation). These never enter the in-console queue
 * (ADR-0002), so this renders NO approve/deny/answer control — only a
 * deep-link/focus CTA (wisphive terminal) or a go-to-terminal pointer
 * (hook-only session). The literal question text/options ARE carried on the
 * `AuditDecision` wire as the (already-redacted, itr#89) `tool_input` for
 * deferred items, so we render the full untruncated prompt here read-only —
 * agent output is UNTRUSTED and rendered only as inert React text nodes.
 */
export function DeferredDetailView({ decision, onFocusTerminal }: DeferredDetailProps) {
  const { tool_name, agent_id, project, decided_by, terminal_session_id, ts } = decision;
  const sessionLabel = terminal_session_id
    ? `term ${terminal_session_id.slice(0, 8)}`
    : `session ${agent_id.slice(0, 8)}`;
  const prompt = parseDeferredPrompt(isRecord(decision.tool_input) ? decision.tool_input : null);

  return (
    <div className="deferred-detail" aria-label={`Deferred detail for ${tool_name}`}>
      <p className="deferred-detail-note">
        <strong>{tool_name}</strong> was deferred to the agent&apos;s native prompt (ADR-0002)
        and never entered the in-console queue, so it cannot be answered here. The full question
        and options are shown below (read-only) and in the agent&apos;s own terminal prompt — use
        the route below to answer.
      </p>

      {/* Full, untruncated prompt. Read-only: NO approve/deny/answer control. */}
      {prompt.kind === "questions" && (
        <div className="deferred-prompt" aria-label="Deferred question">
          {prompt.questions.map((q, i) => (
            <div key={i} className="deferred-question">
              {q.header && <h4 className="deferred-question-header">{q.header}</h4>}
              {q.question && <p className="deferred-question-text">{q.question}</p>}
              {q.options.length > 0 && (
                <ul className="deferred-options">
                  {q.options.map((opt, j) => (
                    <li key={j} className="deferred-option">
                      <strong>{opt.label}</strong>
                      {opt.description && <span> — {opt.description}</span>}
                    </li>
                  ))}
                </ul>
              )}
            </div>
          ))}
        </div>
      )}
      {prompt.kind === "plan" && (
        <div className="deferred-prompt" aria-label="Deferred plan">
          <h4 className="deferred-question-header">Plan</h4>
          <pre className="deferred-plan">{prompt.plan}</pre>
        </div>
      )}
      {prompt.kind === "raw" && (
        <div className="deferred-prompt" aria-label="Deferred prompt">
          <pre className="deferred-plan">{prompt.text}</pre>
        </div>
      )}
      {prompt.kind === "none" && (
        <p className="deferred-detail-note">
          The question details are not available here — see the prompt in your terminal.
        </p>
      )}

      <div className="detail-meta">
        <div><strong>Agent:</strong> {agent_id}</div>
        <div><strong>Project:</strong> {str(project)}</div>
        <div><strong>Session:</strong> {sessionLabel}</div>
        {decided_by && <div><strong>Deferred by:</strong> {decided_by}</div>}
        <div><strong>Time:</strong> {new Date(ts).toLocaleTimeString()}</div>
      </div>
      <div className="detail-actions">
        {terminal_session_id ? (
          <button className="btn-focus" onClick={() => onFocusTerminal(terminal_session_id)}>
            Focus terminal
          </button>
        ) : (
          <span className="deferred-goto-pointer">
            Answer in your <strong>{shortProject(str(project))}</strong> terminal
          </span>
        )}
      </div>
    </div>
  );
}

export function DetailView({ request, onApprove, onDeny }: DetailViewProps) {
  const [modal, setModal] = useState<"deny-msg" | "context" | "always" | null>(null);
  const { tool_name, tool_input: rawInput, agent_id, project, timestamp, hook_event_name, event_data } = request;
  const parsedInput = parseToolInput(tool_name, rawInput);
  const eventData = isRecord(event_data) ? event_data : {};
  const planContent = typeof eventData.plan_content === "string" ? eventData.plan_content : null;

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
    if (hasContent(rawInput)) {
      lines.push("", "--- Tool Input ---", formatJson(rawInput));
    }
    if (hasContent(event_data)) {
      lines.push("", "--- Event Data ---", formatJson(event_data));
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
      {parsedInput.kind === "ask-user-question" && parsedInput.input && (
        <div className="detail-section">
          {parsedInput.input.questions.map((q, i) => (
            <div key={i}>
              <h3>{q.header || "Question"}</h3>
              <p className="question-text">{q.question}</p>
              {q.options.length > 0 && (
                <div className="options-list">
                  {q.options.map((opt, j) => (
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
      {parsedInput.kind === "ask-user-question" && !parsedInput.input && (
        <div className="detail-section">
          <h3>Question</h3>
          <p className="question-text">Question details unavailable.</p>
        </div>
      )}

      {/* Shared tool/event content (unless handled above) */}
      {!planContent && parsedInput.kind !== "ask-user-question" && (
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
