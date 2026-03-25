/**
 * Shared tool-specific content renderer.
 * Used by both DetailView (live queue) and History (resolved entries).
 */

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

interface ToolContentProps {
  toolName: string;
  toolInput: Record<string, unknown> | null;
  hookEventName?: string;
  eventData?: Record<string, unknown>;
  toolResult?: Record<string, unknown> | null;
}

export function ToolContent({ toolName, toolInput, hookEventName, eventData, toolResult }: ToolContentProps) {
  const input = toolInput ?? {};
  const name = (toolName || "").toLowerCase();

  // Extract common fields
  const command = typeof input.command === "string" ? input.command : null;
  const description = typeof input.description === "string" ? input.description : null;
  const filePath = typeof input.file_path === "string" ? input.file_path : null;
  const oldString = typeof input.old_string === "string" ? input.old_string : null;
  const newString = typeof input.new_string === "string" ? input.new_string : null;
  const content = typeof input.content === "string" ? input.content : null;
  const pattern = typeof input.pattern === "string" ? input.pattern : null;

  // Event data fields
  const lastMessage = typeof eventData?.last_assistant_message === "string" ? eventData.last_assistant_message : null;
  const promptText = typeof eventData?.prompt === "string" ? eventData.prompt : null;
  const stopHookActive = typeof eventData?.stop_hook_active === "boolean" ? eventData.stop_hook_active : null;

  // Determine which view to render based on event type first, then tool name
  const eventType = hookEventName || "";

  // --- Event-specific views ---

  if (eventType === "Stop" || eventType === "SubagentStop") {
    return (
      <>
        <div className="detail-section">
          <h3>Stop Reason</h3>
          {lastMessage ? (
            <pre className="code-block">{lastMessage}</pre>
          ) : (
            // Fallback: try tool_input (which may contain event_data from DB)
            <pre className="code-block">
              {typeof input.last_assistant_message === "string"
                ? input.last_assistant_message as string
                : "(no message)"}
            </pre>
          )}
          {stopHookActive !== null && (
            <div className="field-row"><strong>Stop hook active:</strong> {String(stopHookActive)}</div>
          )}
        </div>
        <ToolResultSection result={toolResult} />
      </>
    );
  }

  if (eventType === "UserPromptSubmit") {
    const prompt = promptText || (typeof input.prompt === "string" ? input.prompt as string : null);
    return (
      <>
        {prompt && (
          <div className="detail-section">
            <h3>Submitted Prompt</h3>
            <pre className="code-block">{prompt}</pre>
          </div>
        )}
        <ToolResultSection result={toolResult} />
      </>
    );
  }

  if (eventType === "ConfigChange") {
    const cfgFile = typeof eventData?.file_path === "string" ? eventData.file_path : (typeof input.file_path === "string" ? input.file_path as string : null);
    const source = typeof eventData?.source === "string" ? eventData.source : (typeof input.source === "string" ? input.source as string : null);
    return (
      <>
        <div className="detail-section">
          <h3>Config Change</h3>
          {cfgFile && <div className="field-row"><strong>File:</strong> {cfgFile}</div>}
          {source && <div className="field-row"><strong>Source:</strong> {source}</div>}
        </div>
        <ToolResultSection result={toolResult} />
      </>
    );
  }

  if (eventType === "TeammateIdle") {
    const teammateName = typeof eventData?.teammate_name === "string" ? eventData.teammate_name : null;
    return (
      <>
        <div className="detail-section">
          <h3>Teammate Status</h3>
          <p>Teammate <strong>{teammateName || "unknown"}</strong> is idle.</p>
        </div>
        <ToolResultSection result={toolResult} />
      </>
    );
  }

  if (eventType === "TaskCompleted") {
    const taskSubject = typeof eventData?.task_subject === "string" ? eventData.task_subject : null;
    const taskDesc = typeof eventData?.task_description === "string" ? eventData.task_description : null;
    return (
      <>
        <div className="detail-section">
          <h3>Task Completed</h3>
          {taskSubject && <p><strong>{taskSubject}</strong></p>}
          {taskDesc && <pre className="code-block">{taskDesc}</pre>}
        </div>
        <ToolResultSection result={toolResult} />
      </>
    );
  }

  // --- Tool-specific views ---

  if (name === "bash") {
    return (
      <>
        {description && (
          <div className="detail-section">
            <div className="field-row"><strong>Description:</strong> {description}</div>
          </div>
        )}
        <div className="detail-section">
          <h3>Command</h3>
          <pre className="code-block code-bash">{command || JSON.stringify(input, null, 2)}</pre>
        </div>
        <ToolResultSection result={toolResult} />
      </>
    );
  }

  if (name === "edit" || name === "multiedit") {
    return (
      <>
        {filePath && <div className="file-path">{filePath}</div>}
        {(oldString || newString) ? (
          <div className="detail-section">
            <h3>Changes</h3>
            <DiffView oldStr={oldString || ""} newStr={newString || ""} />
          </div>
        ) : (
          <div className="detail-section">
            <h3>Tool Input</h3>
            <pre className="code-block">{JSON.stringify(input, null, 2)}</pre>
          </div>
        )}
        <ToolResultSection result={toolResult} />
      </>
    );
  }

  if (name === "write") {
    return (
      <>
        {filePath && <div className="file-path">{filePath}</div>}
        <div className="detail-section">
          <h3>Content (new file)</h3>
          <pre className="code-block">{content || JSON.stringify(input, null, 2)}</pre>
        </div>
        <ToolResultSection result={toolResult} />
      </>
    );
  }

  if (name === "read") {
    return (
      <>
        {filePath && <div className="file-path">{filePath}</div>}
        {input.limit && <div className="field-row"><strong>Limit:</strong> {String(input.limit)}</div>}
        {input.offset && <div className="field-row"><strong>Offset:</strong> {String(input.offset)}</div>}
        <ToolResultSection result={toolResult} />
      </>
    );
  }

  if (name === "grep") {
    return (
      <>
        {pattern && <div className="field-row"><strong>Pattern:</strong> <code>{pattern}</code></div>}
        {typeof input.path === "string" && <div className="field-row"><strong>Path:</strong> {input.path as string}</div>}
        {typeof input.type === "string" && <div className="field-row"><strong>Type:</strong> {input.type as string}</div>}
        {typeof input.glob === "string" && <div className="field-row"><strong>Glob:</strong> {input.glob as string}</div>}
        {typeof input.output_mode === "string" && <div className="field-row"><strong>Output:</strong> {input.output_mode as string}</div>}
        <ToolResultSection result={toolResult} />
      </>
    );
  }

  if (name === "glob") {
    return (
      <>
        {pattern && <div className="field-row"><strong>Pattern:</strong> <code>{pattern}</code></div>}
        {typeof input.path === "string" && <div className="field-row"><strong>Path:</strong> {input.path as string}</div>}
        <ToolResultSection result={toolResult} />
      </>
    );
  }

  // --- Generic fallback ---
  const hasInput = input && Object.keys(input).length > 0 && !isNullish(input);
  return (
    <>
      {hasInput ? (
        <div className="detail-section">
          <h3>Tool Input</h3>
          <pre className="code-block">{JSON.stringify(input, null, 2)}</pre>
        </div>
      ) : eventData ? (
        <div className="detail-section">
          <h3>Event Data</h3>
          <pre className="code-block">{JSON.stringify(eventData, null, 2)}</pre>
        </div>
      ) : null}
      <ToolResultSection result={toolResult} />
    </>
  );
}

function ToolResultSection({ result }: { result?: Record<string, unknown> | null }) {
  if (!result) return null;
  return (
    <div className="detail-section">
      <h3>Tool Result</h3>
      <pre className="code-block">{JSON.stringify(result, null, 2)}</pre>
    </div>
  );
}

function isNullish(v: unknown): boolean {
  return v === null || v === undefined || (typeof v === "object" && Object.keys(v as object).length === 0);
}
