import type { DecisionRequest } from "../types/protocol";

interface DetailViewProps {
  request: DecisionRequest;
  onApprove: (id: string) => void;
  onDeny: (id: string, message?: string) => void;
}

// Safe string extraction from unknown values
const str = (v: unknown): string => (typeof v === "string" ? v : String(v ?? ""));

export function DetailView({ request, onApprove, onDeny }: DetailViewProps) {
  const { tool_name, tool_input: rawInput, agent_id, project, timestamp, hook_event_name, event_data } = request;
  const tool_input = rawInput ?? {};
  const planContent = typeof event_data?.plan_content === "string" ? event_data.plan_content : null;
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

      {planContent && (
        <div className="detail-section">
          <h3>Plan</h3>
          <pre className="plan-content">{planContent}</pre>
        </div>
      )}

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
          <div className="diff">
            {oldString && <pre className="diff-remove">- {oldString}</pre>}
            {newString && <pre className="diff-add">+ {newString}</pre>}
          </div>
        </div>
      )}

      {content && tool_name === "Write" && (
        <div className="detail-section">
          <h3>Content (new file)</h3>
          {filePath && <div className="file-path">{filePath}</div>}
          <pre className="code-block">{content}</pre>
        </div>
      )}

      {!command && !oldString && !content && !Array.isArray(tool_input.questions) && !planContent && (
        <div className="detail-section">
          <h3>Tool Input</h3>
          <pre className="code-block">{JSON.stringify(tool_input, null, 2)}</pre>
        </div>
      )}

      <div className="detail-actions">
        <button className="btn-approve" onClick={() => onApprove(request.id)}>
          Approve
        </button>
        <button className="btn-deny" onClick={() => onDeny(request.id)}>
          Deny
        </button>
      </div>
    </div>
  );
}
