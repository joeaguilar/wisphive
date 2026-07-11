/**
 * Shared tool-specific content renderer.
 * Used by both DetailView (live queue) and History (resolved entries).
 */
import { MarkdownText } from "./MarkdownText";
import { CopyButton } from "./CopyButton";
import { parseToolInput } from "./toolInput";

function isRecord(value: unknown): value is Record<string, unknown> {
  return typeof value === "object" && value !== null && !Array.isArray(value);
}

function stringField(value: unknown): string | null {
  return typeof value === "string" ? value : null;
}

function formatJson(value: unknown): string {
  const json = JSON.stringify(value, null, 2);
  return typeof json === "string" ? json : String(value ?? "");
}

function hasContent(value: unknown): boolean {
  if (value === null || value === undefined) return false;
  if (typeof value !== "object") return true;
  return Object.keys(value).length > 0;
}

function CodeBlock({ text, className }: { text: string; className?: string }) {
  return (
    <div className="code-block-wrap">
      <CopyButton value={text} className="copy-btn-overlay" />
      <pre className={className ?? "code-block"}>{text}</pre>
    </div>
  );
}

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
  toolInput: unknown;
  hookEventName?: string;
  eventData?: unknown;
  toolResult?: unknown;
}

export function ToolContent({ toolName, toolInput, hookEventName, eventData, toolResult }: ToolContentProps) {
  const parsed = parseToolInput(toolName, toolInput);
  const input = parsed.value;
  const name = toolName.toLowerCase();
  const { filePath, content, pattern } = parsed.fields;
  const command = parsed.kind === "bash" ? parsed.input.command : null;
  const description = parsed.kind === "bash" ? parsed.input.description : null;
  const oldString = parsed.kind === "edit" ? parsed.input.oldString : null;
  const newString = parsed.kind === "edit" ? parsed.input.newString : null;

  // Event data fields
  const event = isRecord(eventData) ? eventData : {};
  const lastMessage = stringField(event.last_assistant_message);
  const promptText = stringField(event.prompt);
  const stopHookActive = typeof event.stop_hook_active === "boolean" ? event.stop_hook_active : null;

  // Determine which view to render based on event type first, then tool name
  const eventType = hookEventName || "";

  // --- Event-specific views ---

  if (eventType === "Stop" || eventType === "SubagentStop") {
    return (
      <>
        <div className="detail-section">
          <h3>Stop Reason</h3>
          {(() => {
            const msg = lastMessage
              ?? parsed.fields.lastAssistantMessage;
            return msg ? <MarkdownText text={msg} /> : <CodeBlock text="(no message)" />;
          })()}
          {stopHookActive !== null && (
            <div className="field-row"><strong>Stop hook active:</strong> {String(stopHookActive)}</div>
          )}
        </div>
        <ToolResultSection result={toolResult} />
      </>
    );
  }

  if (eventType === "UserPromptSubmit") {
    const prompt = promptText || parsed.fields.prompt;
    return (
      <>
        {prompt && (
          <div className="detail-section">
            <h3>Submitted Prompt</h3>
            <MarkdownText text={prompt} />
          </div>
        )}
        <ToolResultSection result={toolResult} />
      </>
    );
  }

  if (eventType === "ConfigChange") {
    const cfgFile = stringField(event.file_path) ?? parsed.fields.filePath;
    const source = stringField(event.source) ?? parsed.fields.source;
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
    const teammateName = stringField(event.teammate_name);
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
    const taskSubject = stringField(event.task_subject);
    const taskDesc = stringField(event.task_description);
    return (
      <>
        <div className="detail-section">
          <h3>Task Completed</h3>
          {taskSubject && <p><strong>{taskSubject}</strong></p>}
          {taskDesc && <MarkdownText text={taskDesc} />}
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
          <CodeBlock text={command || formatJson(parsed.raw)} className="code-block code-bash" />
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
            <CodeBlock text={formatJson(parsed.raw)} />
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
          <CodeBlock text={content || formatJson(parsed.raw)} />
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
        {parsed.fields.path && <div className="field-row"><strong>Path:</strong> {parsed.fields.path}</div>}
        {parsed.fields.fileType && <div className="field-row"><strong>Type:</strong> {parsed.fields.fileType}</div>}
        {parsed.fields.glob && <div className="field-row"><strong>Glob:</strong> {parsed.fields.glob}</div>}
        {parsed.fields.outputMode && <div className="field-row"><strong>Output:</strong> {parsed.fields.outputMode}</div>}
        <ToolResultSection result={toolResult} />
      </>
    );
  }

  if (name === "glob") {
    return (
      <>
        {pattern && <div className="field-row"><strong>Pattern:</strong> <code>{pattern}</code></div>}
        {parsed.fields.path && <div className="field-row"><strong>Path:</strong> {parsed.fields.path}</div>}
        <ToolResultSection result={toolResult} />
      </>
    );
  }

  // --- Generic fallback ---
  const hasInput = hasContent(parsed.raw);
  return (
    <>
      {hasInput ? (
        <div className="detail-section">
          <h3>Tool Input</h3>
          <CodeBlock text={formatJson(parsed.raw)} />
        </div>
      ) : eventData ? (
        <div className="detail-section">
          <h3>Event Data</h3>
          <CodeBlock text={formatJson(eventData)} />
        </div>
      ) : null}
      <ToolResultSection result={toolResult} />
    </>
  );
}

function ToolResultSection({ result }: { result?: unknown }) {
  if (result === null || result === undefined) return null;
  return (
    <div className="detail-section">
      <h3>Tool Result</h3>
      <CodeBlock text={formatJson(result)} />
    </div>
  );
}
