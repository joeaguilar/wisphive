import { useEffect, useRef, useState } from "react";
import { Modal } from "./Modal";
import type { SpawnStatus } from "../hooks/useWisphive";

/** How long a submit may sit unanswered before the modal stops claiming
 * progress (itr#567). The daemon replies to spawn_agent synchronously
 * (queued ack or refusal) well within this. */
const PENDING_TIMEOUT_MS = 10_000;

interface SpawnModalProps {
  projects: string[];
  defaultProject?: string;
  /** Correlated status of the submit (useWisphive `state.spawn`, itr#567):
   * null before the first submit, then pending → queued | refused. */
  status: SpawnStatus | null;
  onSpawn: (req: {
    agent_type?: "claude_code" | "codex";
    project: string;
    prompt: string;
    model?: string;
    reasoning?: string;
    max_turns?: number;
  }) => void;
  onClose: () => void;
}

export function SpawnModal({ projects, defaultProject, status, onSpawn, onClose }: SpawnModalProps) {
  const [agentType, setAgentType] = useState<"claude_code" | "codex">("claude_code");
  const [project, setProject] = useState(defaultProject || "");
  const [prompt, setPrompt] = useState("");
  const [model, setModel] = useState("");
  const [reasoning, setReasoning] = useState("");
  const [maxTurns, setMaxTurns] = useState("");
  // The correlation id of a submit whose pending wait expired. Deriving
  // `timedOut` from a match against the CURRENT submit means a fresh submit
  // (new correlation id) implicitly clears the stale flag — no synchronous
  // setState-in-effect needed.
  const [timedOutFor, setTimedOutFor] = useState<string | null>(null);
  const promptRef = useRef<HTMLTextAreaElement>(null);

  useEffect(() => {
    promptRef.current?.focus();
  }, []);

  // Never spin forever on a reply that is not coming: if the daemon has not
  // answered this submit (keyed by its correlation id) within the timeout,
  // stop claiming progress and let the operator retry. A late queued/refused
  // reply still lands — the phase change re-runs this effect and re-renders.
  const phase = status?.phase ?? null;
  const correlationId = status?.correlationId ?? null;
  useEffect(() => {
    if (phase !== "pending" || correlationId === null) return;
    const timer = setTimeout(() => setTimedOutFor(correlationId), PENDING_TIMEOUT_MS);
    return () => clearTimeout(timer);
  }, [phase, correlationId]);

  const timedOut = phase === "pending" && timedOutFor === correlationId;
  const pending = phase === "pending" && !timedOut;
  const queued = phase === "queued";

  const handleSubmit = () => {
    if (!project.trim() || !prompt.trim() || pending || queued) return;
    onSpawn({
      agent_type: agentType,
      project: project.trim(),
      prompt: prompt.trim(),
      model: model.trim() || undefined,
      reasoning: reasoning.trim() || undefined,
      max_turns: maxTurns ? parseInt(maxTurns, 10) : undefined,
    });
  };

  return (
    <Modal title="Spawn Agent" onClose={onClose}>
      <div className="spawn-form">
        <label>
          <span>Project</span>
          {projects.length > 0 ? (
            <select value={project} onChange={(e) => setProject(e.target.value)}>
              <option value="">Select a project...</option>
              {projects.map((p) => (
                <option key={p} value={p}>{p}</option>
              ))}
            </select>
          ) : (
            <input type="text" value={project} onChange={(e) => setProject(e.target.value)} placeholder="/path/to/project" />
          )}
        </label>

        <label>
          <span>Prompt</span>
          <textarea
            ref={promptRef}
            value={prompt}
            onChange={(e) => setPrompt(e.target.value)}
            placeholder="What should the agent do?"
            rows={3}
          />
        </label>

        <div className="spawn-options">
          <label>
            <span>Agent</span>
            <select value={agentType} onChange={(e) => setAgentType(e.target.value as "claude_code" | "codex")}>
              <option value="claude_code">Claude Code</option>
              <option value="codex">Codex</option>
            </select>
          </label>

          <label>
            <span>Model</span>
            <select value={model} onChange={(e) => setModel(e.target.value)}>
              <option value="">Default</option>
              <option value="sonnet">Sonnet</option>
              <option value="opus">Opus</option>
              <option value="haiku">Haiku</option>
            </select>
          </label>

          <label>
            <span>Reasoning</span>
            <select value={reasoning} onChange={(e) => setReasoning(e.target.value)}>
              <option value="">Default</option>
              <option value="low">Low</option>
              <option value="medium">Medium</option>
              <option value="high">High</option>
            </select>
          </label>

          <label>
            <span>Max Turns</span>
            <input type="number" value={maxTurns} onChange={(e) => setMaxTurns(e.target.value)} placeholder="∞" min="1" />
          </label>
        </div>

        {queued && (
          <div className="spawn-status spawn-status-queued" role="status">
            Spawn queued for approval — review and approve it from the Inbox.
          </div>
        )}
        {status?.phase === "refused" && (
          // Daemon-authored refusal text: untrusted display data, rendered as
          // an inert text node. The message names the exact refusal cause
          // (mode off, invalid request, hook gate, deny, expiry, …).
          <div className="spawn-status spawn-status-refused" role="alert">
            {status.message}
          </div>
        )}
        {phase === "pending" && timedOut && (
          <div className="spawn-status spawn-status-refused" role="alert">
            No response from the daemon after {PENDING_TIMEOUT_MS / 1000}s — the spawn has not
            been confirmed. Check the connection and try again.
          </div>
        )}

        <div className="modal-actions">
          {queued ? (
            <button className="btn-approve" onClick={onClose}>Close</button>
          ) : (
            <button
              className="btn-approve"
              onClick={handleSubmit}
              disabled={!project.trim() || !prompt.trim() || pending}
            >
              {pending ? "Spawning…" : phase === "refused" || timedOut ? "Retry Spawn" : "Spawn"}
            </button>
          )}
          <button className="btn-cancel" onClick={onClose}>Cancel</button>
        </div>
      </div>
    </Modal>
  );
}
