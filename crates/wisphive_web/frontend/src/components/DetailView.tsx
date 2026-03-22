import { useState } from "react";
import type { DecisionRequest } from "../types/protocol";
import { TextModal, ConfirmModal } from "./Modal";

interface DetailViewProps {
  request: DecisionRequest;
  onApprove: (id: string, opts?: { additional_context?: string; always_allow?: boolean }) => void;
  onDeny: (id: string, message?: string) => void;
}

// Safe string extraction from unknown values
const str = (v: unknown): string => (typeof v === "string" ? v : String(v ?? ""));

// Simple diff renderer — split old/new into lines and show unified view
function DiffView({ oldStr, newStr }: { oldStr: string; newStr: string }) {
  const oldLines = oldStr.split("\n");
  const newLines = newStr.split("\n");
  return (
    <div className="diff-view">
      {oldLines.map((line, i) => (
        <div key={`old-${i}`} className="diff-line diff-remove">
          <span className="diff-gutter">-</span>
          <span className="diff-text">{line}</span>
        </div>
      ))}
      {newLines.map((line, i) => (
        <div key={`new-${i}`} className="diff-line diff-add">
          <span className="diff-gutter">+</span>
          <span className="diff-text">{line}</span>
        </div>
      ))}
    </div>
  );
}

// Simple markdown to HTML (headers, bold, code, lists)
function renderMarkdown(text: string): string {
  return text
    .replace(/^### (.+)$/gm, '<h4 class="md-h3">$1</h4>')
    .replace(/^## (.+)$/gm, '<h3 class="md-h2">$1</h3>')
    .replace(/^# (.+)$/gm, '<h2 class="md-h1">$1</h2>')
    .replace(/\*\*(.+?)\*\*/g, '<strong>$1</strong>')
    .replace(/`([^`]+)`/g, '<code class="md-code">$1</code>')
    .replace(/^- (.+)$/gm, '<div class="md-li">• $1</div>')
    .replace(/^(\d+)\. (.+)$/gm, '<div class="md-li">$1. $2</div>')
    .replace(/\n\n/g, '<br/><br/>')
    .replace(/\n/g, '<br/>');
}

export function DetailView({ request, onApprove, onDeny }: DetailViewProps) {
  const [modal, setModal] = useState<"deny-msg" | "context" | "always" | null>(null);
  const { tool_name, tool_input: rawInput, agent_id, project, timestamp, hook_event_name, event_data } = request;
  const tool_input = rawInput ?? {};
  // Event data fields
  const planContent = typeof event_data?.plan_content === "string" ? event_data.plan_content : null;
  const promptText = typeof event_data?.prompt === "string" ? event_data.prompt : null;
  const lastMessage = typeof event_data?.last_assistant_message === "string" ? event_data.last_assistant_message : null;
  const teammateName = typeof event_data?.teammate_name === "string" ? event_data.teammate_name : null;
  const taskSubject = typeof event_data?.task_subject === "string" ? event_data.task_subject : null;
  // Tool input fields
  const command = typeof tool_input.command === "string" ? tool_input.command : null;
  const oldString = typeof tool_input.old_string === "string" ? tool_input.old_string : null;
  const newString = typeof tool_input.new_string === "string" ? tool_input.new_string : null;
  const filePath = typeof tool_input.file_path === "string" ? tool_input.file_path : null;
  const content = typeof tool_input.content === "string" ? tool_input.content : null;

  return (
    <div className="detail-view">
      <div className="detail-header">
        <h2>{tool_name}</h2>
        <span className="event-badge">{hook_event_name}</span>
      </div>

      <div className="detail-meta">
        <div><strong>Agent:</strong> {agent_id}</div>
        <div><strong>Project:</strong> {project}</div>
        <div><strong>Time:</strong> {new Date(timestamp).toLocaleTimeString()}</div>
      </div>

      {/* UserPromptSubmit: show the submitted prompt */}
      {promptText && (
        <div className="detail-section">
          <h3>Submitted Prompt</h3>
          <pre className="code-block">{promptText}</pre>
        </div>
      )}

      {/* Stop: show last assistant message */}
      {lastMessage && (
        <div className="detail-section">
          <h3>Last Message</h3>
          <pre className="plan-content">{lastMessage}</pre>
        </div>
      )}

      {/* TeammateIdle */}
      {teammateName && (
        <div className="detail-section">
          <h3>Teammate Status</h3>
          <p>Teammate <strong>{teammateName}</strong> is idle.</p>
        </div>
      )}

      {/* TaskCompleted */}
      {taskSubject && (
        <div className="detail-section">
          <h3>Task Completed</h3>
          <p><strong>{taskSubject}</strong></p>
          {typeof event_data?.task_description === "string" && (
            <pre className="code-block">{event_data.task_description as string}</pre>
          )}
        </div>
      )}

      {/* ExitPlanMode: plan content */}
      {planContent && (
        <div className="detail-section">
          <h3>Plan</h3>
          <div className="markdown-content" dangerouslySetInnerHTML={{ __html: renderMarkdown(planContent) }} />
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

      {command && (
        <div className="detail-section">
          <h3>Command</h3>
          <pre className="code-block">{command}</pre>
        </div>
      )}

      {(oldString || newString) && (
        <div className="detail-section">
          <h3>Changes</h3>
          {filePath && <div className="file-path">{filePath}</div>}
          <DiffView oldStr={oldString || ""} newStr={newString || ""} />
        </div>
      )}

      {content && tool_name === "Write" && (
        <div className="detail-section">
          <h3>Content (new file)</h3>
          {filePath && <div className="file-path">{filePath}</div>}
          <pre className="code-block">{content}</pre>
        </div>
      )}

      {!command && !oldString && !content && !Array.isArray(tool_input.questions) &&
       !planContent && !promptText && !lastMessage && !teammateName && !taskSubject && (
        <div className="detail-section">
          <h3>Tool Input</h3>
          <pre className="code-block">
            {Object.keys(tool_input).length > 0
              ? JSON.stringify(tool_input, null, 2)
              : event_data
                ? JSON.stringify(event_data, null, 2)
                : "(no data)"}
          </pre>
        </div>
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
