import { useEffect } from "react";
import type { HistoryEntry, SessionSummary } from "../types/protocol";

interface SessionsProps {
  sessions: SessionSummary[];
  timeline: HistoryEntry[];
  selectedAgent: string | null;
  onLoad: () => void;
  onSelectAgent: (agentId: string | null) => void;
  onLoadTimeline: (agentId: string) => void;
}

function duration(first: string, last: string): string {
  const ms = new Date(last).getTime() - new Date(first).getTime();
  const seconds = Math.floor(ms / 1000);
  if (seconds < 60) return `${seconds}s`;
  if (seconds < 3600) return `${Math.floor(seconds / 60)}m`;
  return `${Math.floor(seconds / 3600)}h ${Math.floor((seconds % 3600) / 60)}m`;
}

export function Sessions({ sessions, timeline, selectedAgent, onLoad, onSelectAgent, onLoadTimeline }: SessionsProps) {
  useEffect(() => { onLoad(); }, [onLoad]);

  if (selectedAgent) {
    return (
      <div className="sessions-view">
        <div className="sessions-toolbar">
          <button className="btn-secondary" onClick={() => onSelectAgent(null)}>
            ← Back to sessions
          </button>
          <span className="filter-tag">Agent: {selectedAgent.slice(0, 20)}</span>
        </div>
        {timeline.length === 0 ? (
          <div className="history-empty">No timeline entries</div>
        ) : (
          <div className="history-list">
            {timeline.map((entry) => {
              const d = entry.decision.replace(/"/g, "");
              const cls = d === "approve" ? "badge-approve" : d === "deny" ? "badge-deny" : "badge-defer";
              return (
                <div key={entry.id} className="history-item">
                  <div className="history-item-row">
                    <span className={`decision-badge ${cls}`}>{d.toUpperCase()}</span>
                    <span className="tool-name">{entry.tool_name}</span>
                    <span className="time-ago">{new Date(entry.resolved_at).toLocaleTimeString()}</span>
                    {entry.tool_result && <span className="result-indicator">+</span>}
                  </div>
                </div>
              );
            })}
          </div>
        )}
      </div>
    );
  }

  return (
    <div className="sessions-view">
      <div className="sessions-toolbar">
        <h2>Sessions ({sessions.length})</h2>
      </div>
      {sessions.length === 0 ? (
        <div className="history-empty">No sessions</div>
      ) : (
        <div className="sessions-list">
          {sessions.map((s) => (
            <div key={s.agent_id} className="session-item" onClick={() => {
              onSelectAgent(s.agent_id);
              onLoadTimeline(s.agent_id);
            }}>
              <div className="session-header">
                <span className={`status-indicator ${s.is_live ? "live" : "ended"}`}>
                  {s.is_live ? "●" : "○"}
                </span>
                <span className="agent-id">{s.agent_id.slice(0, 20)}</span>
                <span className="time-ago">{duration(s.first_seen, s.last_seen)}</span>
                {s.pending_count > 0 && <span className="badge badge-pending">{s.pending_count}</span>}
              </div>
              <div className="session-meta">
                <span className="project-name">{s.project.split("/").pop()}</span>
                <span className="session-stats">
                  {s.total_calls} calls · {s.approved} approved · {s.denied} denied
                </span>
              </div>
            </div>
          ))}
        </div>
      )}
    </div>
  );
}
