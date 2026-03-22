import { useEffect } from "react";
import type { ProjectSummary } from "../types/protocol";

interface ProjectsProps {
  projects: ProjectSummary[];
  onLoad: () => void;
  onSpawnInProject: (project: string) => void;
  onDrillDown: (project: string) => void;
}

function duration(first: string, last: string): string {
  const ms = new Date(last).getTime() - new Date(first).getTime();
  const seconds = Math.floor(ms / 1000);
  if (seconds < 60) return `${seconds}s`;
  if (seconds < 3600) return `${Math.floor(seconds / 60)}m`;
  return `${Math.floor(seconds / 3600)}h ${Math.floor((seconds % 3600) / 60)}m`;
}

export function Projects({ projects, onLoad, onSpawnInProject, onDrillDown }: ProjectsProps) {
  useEffect(() => { onLoad(); }, [onLoad]);

  return (
    <div className="projects-view">
      <div className="sessions-toolbar">
        <h2>Projects ({projects.length})</h2>
      </div>
      {projects.length === 0 ? (
        <div className="history-empty">No projects</div>
      ) : (
        <div className="sessions-list">
          {projects.map((p) => (
            <div key={p.project} className="session-item" onClick={() => onDrillDown(p.project)}>
              <div className="session-header">
                <span className={`status-indicator ${p.agent_count > 0 ? "live" : "ended"}`}>
                  {p.agent_count > 0 ? "●" : "○"}
                </span>
                <span className="project-name-lg">{p.project.split("/").pop()}</span>
                <span className="time-ago">{duration(p.first_seen, p.last_seen)}</span>
              </div>
              <div className="session-meta">
                <span className="session-stats">
                  {p.agent_count} agents · {p.total_calls} calls · {p.approved} approved · {p.denied} denied
                </span>
                <button className="btn-secondary spawn-project-btn" onClick={(e) => {
                  e.stopPropagation();
                  onSpawnInProject(p.project);
                }}>
                  + Agent
                </button>
              </div>
            </div>
          ))}
        </div>
      )}
    </div>
  );
}
