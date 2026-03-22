import { useEffect, useRef, useState } from "react";
import { Modal } from "./Modal";

interface SpawnModalProps {
  projects: string[];
  defaultProject?: string;
  onSpawn: (req: {
    project: string;
    prompt: string;
    model?: string;
    reasoning?: string;
    max_turns?: number;
  }) => void;
  onClose: () => void;
}

export function SpawnModal({ projects, defaultProject, onSpawn, onClose }: SpawnModalProps) {
  const [project, setProject] = useState(defaultProject || "");
  const [prompt, setPrompt] = useState("");
  const [model, setModel] = useState("");
  const [reasoning, setReasoning] = useState("");
  const [maxTurns, setMaxTurns] = useState("");
  const promptRef = useRef<HTMLTextAreaElement>(null);

  useEffect(() => {
    promptRef.current?.focus();
  }, []);

  const handleSubmit = () => {
    if (!project.trim() || !prompt.trim()) return;
    onSpawn({
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

        <div className="modal-actions">
          <button className="btn-approve" onClick={handleSubmit} disabled={!project.trim() || !prompt.trim()}>
            Spawn
          </button>
          <button className="btn-cancel" onClick={onClose}>Cancel</button>
        </div>
      </div>
    </Modal>
  );
}
