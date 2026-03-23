import type { AgentInfo, HistoryEntry } from "../types/protocol";

interface AgentsProps {
  agents: AgentInfo[];
  timeline: HistoryEntry[];
  selectedAgent: string | null;
  onSelectAgent: (agentId: string | null) => void;
  onLoadTimeline: (agentId: string) => void;
  onSpawn: () => void;
}

function duration(first: string, last: string): string {
  const ms = new Date(last).getTime() - new Date(first).getTime();
  const seconds = Math.floor(ms / 1000);
  if (seconds < 60) return `${seconds}s`;
  if (seconds < 3600) return `${Math.floor(seconds / 60)}m`;
  return `${Math.floor(seconds / 3600)}h ${Math.floor((seconds % 3600) / 60)}m`;
}

function timeAgo(ts: string): string {
  const ms = Date.now() - new Date(ts).getTime();
  const seconds = Math.floor(ms / 1000);
  if (seconds < 60) return `${seconds}s ago`;
  if (seconds < 3600) return `${Math.floor(seconds / 60)}m ago`;
  return `${Math.floor(seconds / 3600)}h ${Math.floor((seconds % 3600) / 60)}m ago`;
}

export function Agents({ agents, timeline, selectedAgent, onSelectAgent, onLoadTimeline, onSpawn }: AgentsProps) {
  if (selectedAgent) {
    return (
      <div className="agents-view">
        <div className="agents-toolbar">
          <button className="btn-secondary" onClick={() => onSelectAgent(null)}>
            &larr; Back to agents
          </button>
          <span className="filter-tag">Agent: {selectedAgent}</span>
        </div>
        {timeline.length === 0 ? (
          <div className="history-empty">No activity recorded for this agent</div>
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
    <div className="agents-view">
      <div className="agents-toolbar">
        <h2>Connected Agents ({agents.length})</h2>
        <button className="btn-secondary spawn-btn-inline" onClick={onSpawn}>+ Spawn</button>
      </div>
      {agents.length === 0 ? (
        <div className="history-empty">No agents connected</div>
      ) : (
        <div className="agents-list">
          {agents.map((a) => (
            <div key={a.agent_id} className="agent-card" onClick={() => {
              onSelectAgent(a.agent_id);
              onLoadTimeline(a.agent_id);
            }}>
              <div className="agent-card-header">
                <span className="status-indicator live">&#9679;</span>
                <span className="agent-card-id">{a.agent_id}</span>
                <span className="agent-card-type">{a.agent_type}</span>
                <span className="time-ago">{duration(a.connected_at, a.last_seen)}</span>
              </div>
              <div className="agent-card-meta">
                <span className="project-name">{a.project.split("/").pop()}</span>
                <span className="agent-card-times">
                  Connected {timeAgo(a.connected_at)} &middot; Last seen {timeAgo(a.last_seen)}
                </span>
              </div>
            </div>
          ))}
        </div>
      )}
    </div>
  );
}
