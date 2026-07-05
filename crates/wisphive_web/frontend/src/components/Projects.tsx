import { useEffect, useState } from "react";
import type { ProjectHookStatus, ProjectSummary } from "../types/protocol";
import { ConfirmModal } from "./Modal";
import { activate } from "./a11y";

interface ProjectsProps {
  projects: ProjectSummary[];
  /** Per-project hook install state, keyed by absolute project path (itr#460). */
  hookStatus: Record<string, ProjectHookStatus>;
  /** Per-project install error strings, keyed by project path. Inert text. */
  hookErrors: Record<string, string>;
  onLoad: () => void;
  onSpawnInProject: (project: string) => void;
  onDrillDown: (project: string) => void;
  /** Send an `install_hooks` for the given absolute path (itr#460). */
  onInstallHooks: (project: string) => void;
  /** Lazily fetch a project's hook status (itr#460). */
  onQueryHookStatus: (project: string) => void;
}

type GateState = "gated" | "needs-repair" | "not-gated" | "unknown";

/** Derive the gating badge state from a project's hook status. A project is
 * "gated" only when every expected hook is installed AND the global mode is
 * active. Anything partially installed or installed-but-disabled needs repair;
 * a project with nothing installed is simply not gated. */
function deriveGateState(status: ProjectHookStatus | undefined): GateState {
  if (!status) return "unknown";
  if (status.all_installed && status.mode === "active") return "gated";
  if (status.claude_installed || status.codex_installed || status.all_installed) {
    return "needs-repair";
  }
  return "not-gated";
}

const BADGE_LABEL: Record<GateState, string> = {
  gated: "Gated",
  "needs-repair": "Needs repair",
  "not-gated": "Not gated",
  unknown: "Checking…",
};

function duration(first: string, last: string): string {
  const start = new Date(first).getTime();
  const end = new Date(last).getTime();
  if (isNaN(start) || isNaN(end)) return "—";
  const seconds = Math.floor((end - start) / 1000);
  if (seconds < 60) return `${seconds}s`;
  if (seconds < 3600) return `${Math.floor(seconds / 60)}m`;
  return `${Math.floor(seconds / 3600)}h ${Math.floor((seconds % 3600) / 60)}m`;
}

export function Projects({
  projects,
  hookStatus,
  hookErrors,
  onLoad,
  onSpawnInProject,
  onDrillDown,
  onInstallHooks,
  onQueryHookStatus,
}: ProjectsProps) {
  useEffect(() => { onLoad(); }, [onLoad]);

  // Lazily fetch hook status for every listed project. Re-runs when the list
  // changes; the daemon answers each with a `project_hook_status`.
  useEffect(() => {
    for (const p of projects) onQueryHookStatus(p.project);
  }, [projects, onQueryHookStatus]);

  // Free-form path the user wants to gate (a project not yet in the list).
  const [gatePath, setGatePath] = useState("");
  // The project pending confirm-before-write; null when no modal is open.
  const [pendingInstall, setPendingInstall] = useState<string | null>(null);

  const submitGatePath = () => {
    const trimmed = gatePath.trim();
    if (trimmed) setPendingInstall(trimmed);
  };

  const confirmInstall = () => {
    if (pendingInstall) {
      onInstallHooks(pendingInstall);
      // If this was the free-form path, clear the input on confirm.
      if (pendingInstall === gatePath.trim()) setGatePath("");
    }
    setPendingInstall(null);
  };

  return (
    <div className="projects-view">
      <div className="sessions-toolbar">
        <h2>Projects ({projects.length})</h2>
        <button className="btn-secondary" onClick={onLoad}>Refresh</button>
      </div>

      <div className="gate-path-bar">
        <input
          className="gate-path-input"
          type="text"
          aria-label="Absolute path of the project to gate with Wisphive hooks"
          placeholder="/absolute/path/to/project"
          value={gatePath}
          onChange={(e) => setGatePath(e.target.value)}
          onKeyDown={(e) => {
            if (e.key === "Enter") {
              e.preventDefault();
              submitGatePath();
            }
          }}
        />
        <button
          className="btn-secondary gate-path-btn"
          onClick={submitGatePath}
          disabled={!gatePath.trim()}
        >
          Gate project
        </button>
      </div>

      {projects.length === 0 ? (
        <div className="history-empty">No projects</div>
      ) : (
        <div className="sessions-list">
          {projects.map((p) => {
            const status = hookStatus[p.project];
            const gateState = deriveGateState(status);
            const error = hookErrors[p.project];
            return (
              <div
                key={p.project}
                className="session-item"
                aria-label={`Project ${p.project.split("/").pop()} — ${p.agent_count > 0 ? "active" : "idle"}. View history.`}
                onClick={() => onDrillDown(p.project)}
                {...activate(() => onDrillDown(p.project))}
              >
                <div className="session-header">
                  <span
                    className={`status-indicator ${p.agent_count > 0 ? "live" : "ended"}`}
                    role="img"
                    aria-label={p.agent_count > 0 ? "active" : "idle"}
                  >
                    {p.agent_count > 0 ? "●" : "○"}
                  </span>
                  <span className="project-name-lg">{p.project.split("/").pop()}</span>
                  <span className={`gate-badge gate-badge-${gateState}`}>{BADGE_LABEL[gateState]}</span>
                  <span className="time-ago">{duration(p.first_seen, p.last_seen)}</span>
                </div>
                <div className="session-meta">
                  <span className="session-stats">
                    {p.agent_count} agents · {p.total_calls} calls · {p.approved} approved · {p.denied} denied
                  </span>
                  {(gateState === "not-gated" || gateState === "needs-repair") && (
                    <button className="btn-secondary gate-repair-btn" onClick={(e) => {
                      e.stopPropagation();
                      setPendingInstall(p.project);
                    }}>
                      {gateState === "needs-repair" ? "Repair" : "Gate"}
                    </button>
                  )}
                  <button className="btn-secondary spawn-project-btn" onClick={(e) => {
                    e.stopPropagation();
                    onSpawnInProject(p.project);
                  }}>
                    + Agent
                  </button>
                </div>
                {error && (
                  <div className="gate-error" role="alert">{error}</div>
                )}
              </div>
            );
          })}
        </div>
      )}

      {pendingInstall && (
        <ConfirmModal
          title="Gate this project?"
          message={
            `Wisphive will add its hooks to ${pendingInstall}/.claude/settings.json ` +
            `(PreToolUse/… + Bash/Edit/Write/NotebookEdit allowlist) and ` +
            `${pendingInstall}/.codex/hooks.json.`
          }
          confirmLabel="Install hooks"
          onConfirm={confirmInstall}
          onClose={() => setPendingInstall(null)}
        />
      )}
    </div>
  );
}
