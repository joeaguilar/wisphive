import { useState } from "react";
import type { AgentInfo, DecisionRequest, HistoryEntry } from "../types/protocol";
import { activate } from "./a11y";
import { parseToolInput } from "./toolInput";

interface AgentsProps {
  agents: AgentInfo[];
  queue: DecisionRequest[];
  timeline: HistoryEntry[];
  selectedAgent: string | null;
  onSelectAgent: (agentId: string | null) => void;
  onLoadTimeline: (agentId: string) => void;
  onRefreshTimeline: (agentId: string) => void;
  onApprove: (id: string) => void;
  onDeny: (id: string) => void;
  onSpawn: () => void;
}

function duration(first: string, last: string): string {
  const start = new Date(first).getTime();
  const end = new Date(last).getTime();
  if (isNaN(start) || isNaN(end)) return "—";
  const seconds = Math.floor((end - start) / 1000);
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

function inputSummary(toolName: string, input: unknown): string {
  const parsed = parseToolInput(toolName, input);
  if (parsed.kind === "bash" && parsed.input.command) {
    const cmd = parsed.input.command;
    return cmd.length > 60 ? cmd.slice(0, 57) + "..." : cmd;
  }
  if (parsed.fields.filePath) return parsed.fields.filePath;
  if (parsed.fields.pattern) return `/${parsed.fields.pattern}/`;
  if (parsed.kind === "ask-user-question" && parsed.input) {
    return parsed.input.questions[0]?.question ?? "";
  }
  return "";
}

export function Agents({ agents, queue, timeline, selectedAgent, onSelectAgent, onLoadTimeline, onRefreshTimeline, onApprove, onDeny, onSpawn }: AgentsProps) {
  const [expandedEntry, setExpandedEntry] = useState<string | null>(null);

  // Drilldown view: agent's pending decisions + timeline
  if (selectedAgent) {
    const agentPending = queue.filter((r) => r.agent_id === selectedAgent);
    const agent = agents.find((a) => a.agent_id === selectedAgent);

    return (
      <div className="agents-view">
        <div className="agents-toolbar">
          <button className="btn-secondary" onClick={() => onSelectAgent(null)}>
            ← Back to agents
          </button>
          <button className="btn-secondary" onClick={() => onRefreshTimeline(selectedAgent)}>
            Refresh
          </button>
        </div>

        {/* Agent header */}
        {agent && (
          <div className="agent-detail-header">
            <h2 className="agent-detail-id">
              <span className="status-indicator live" aria-hidden="true">●</span>
              <span>{agent.agent_id}</span>
              <span className="agent-card-type">{agent.agent_type}</span>
            </h2>
            <div className="agent-detail-meta">
              <span>Project: {agent.project}</span>
              <span>Connected {timeAgo(agent.connected_at)}</span>
              <span>Active for {duration(agent.connected_at, agent.last_seen)}</span>
            </div>
          </div>
        )}

        {/* Pending decisions — what the agent is blocked on */}
        {agentPending.length > 0 && (
          <div className="agent-section">
            <h3>Waiting for Decision ({agentPending.length})</h3>
            <div className="agent-pending-list">
              {agentPending.map((req) => (
                <div key={req.id} className="agent-pending-item">
                  <div className="agent-pending-header">
                    <span className="tool-name">{req.tool_name}</span>
                    <span className="event-badge">{req.hook_event_name}</span>
                    <span className="time-ago">{timeAgo(req.timestamp)}</span>
                  </div>
                  {inputSummary(req.tool_name, req.tool_input) && (
                    <div className="agent-pending-summary">{inputSummary(req.tool_name, req.tool_input)}</div>
                  )}
                  <div className="agent-pending-actions">
                    <button className="btn-approve btn-sm" onClick={() => onApprove(req.id)}>Approve</button>
                    <button className="btn-deny btn-sm" onClick={() => onDeny(req.id)}>Deny</button>
                  </div>
                </div>
              ))}
            </div>
          </div>
        )}

        {agentPending.length === 0 && (
          <div className="agent-section">
            <h3>Status</h3>
            <div className="agent-status-idle">
              <span className="status-indicator live" aria-hidden="true">●</span>
              Agent is working — no pending decisions
            </div>
          </div>
        )}

        {/* Activity timeline */}
        <div className="agent-section">
          <h3>Recent Activity ({timeline.length})</h3>
          {timeline.length === 0 ? (
            <div className="history-empty">No activity recorded yet</div>
          ) : (
            <div className="history-list">
              {timeline.map((entry) => {
                const d = entry.decision.replace(/"/g, "");
                const cls = d === "approve" ? "badge-approve" : d === "deny" ? "badge-deny" : "badge-defer";
                const isExpanded = expandedEntry === entry.id;
                return (
                  <div
                    key={entry.id}
                    className="history-item"
                    aria-expanded={isExpanded}
                    aria-label={`${entry.tool_name} — ${isExpanded ? "collapse" : "expand"} detail`}
                    onClick={() => setExpandedEntry(isExpanded ? null : entry.id)}
                    {...activate(() => setExpandedEntry(isExpanded ? null : entry.id))}
                  >
                    <div className="history-item-row">
                      <span className={`decision-badge ${cls}`}>{d.toUpperCase()}</span>
                      <span className="tool-name">{entry.tool_name}</span>
                      <span className="time-ago">{new Date(entry.resolved_at).toLocaleTimeString()}</span>
                      {entry.tool_result !== null && entry.tool_result !== undefined && (
                        <span className="result-indicator">+</span>
                      )}
                    </div>
                    {inputSummary(entry.tool_name, entry.tool_input) && !isExpanded && (
                      <div className="queue-item-summary">{inputSummary(entry.tool_name, entry.tool_input)}</div>
                    )}
                    {isExpanded && (
                      <div className="history-detail">
                        <div className="detail-section">
                          <h3>Tool Input</h3>
                          <pre className="code-block">{JSON.stringify(entry.tool_input, null, 2)}</pre>
                        </div>
                        {entry.tool_result !== null && entry.tool_result !== undefined && (
                          <div className="detail-section">
                            <h3>Tool Result</h3>
                            <pre className="code-block">{JSON.stringify(entry.tool_result, null, 2)}</pre>
                          </div>
                        )}
                      </div>
                    )}
                  </div>
                );
              })}
            </div>
          )}
        </div>
      </div>
    );
  }

  // Agent list view
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
          {agents.map((a) => {
            const pending = queue.filter((r) => r.agent_id === a.agent_id);
            const lastPending = pending[pending.length - 1];
            return (
              <div
                key={a.agent_id}
                className="agent-card"
                aria-label={`Agent ${a.agent_id.slice(0, 24)}${pending.length > 0 ? ` — ${pending.length} pending` : " — working"}`}
                onClick={() => {
                  onSelectAgent(a.agent_id);
                  onLoadTimeline(a.agent_id);
                }}
                {...activate(() => {
                  onSelectAgent(a.agent_id);
                  onLoadTimeline(a.agent_id);
                })}
              >
                <div className="agent-card-header">
                  <span className="status-indicator live" role="img" aria-label="connected">●</span>
                  <span className="agent-card-id">{a.agent_id.slice(0, 24)}</span>
                  <span className="agent-card-type">{a.agent_type}</span>
                  <span className="time-ago">{duration(a.connected_at, a.last_seen)}</span>
                </div>
                <div className="agent-card-meta">
                  <span className="project-name">{a.project.split("/").pop()}</span>
                  {pending.length > 0 && (
                    <span className="badge badge-pending">{pending.length} pending</span>
                  )}
                </div>
                {/* Show what the agent is blocked on */}
                {lastPending && (
                  <div className="agent-card-current">
                    <span className="agent-card-blocked">⏳ Waiting:</span>
                    <span className="tool-name">{lastPending.tool_name}</span>
                    {inputSummary(lastPending.tool_name, lastPending.tool_input) && (
                      <span className="agent-card-input">{inputSummary(lastPending.tool_name, lastPending.tool_input)}</span>
                    )}
                  </div>
                )}
                {pending.length === 0 && (
                  <div className="agent-card-current">
                    <span className="agent-card-working">● Working</span>
                  </div>
                )}
              </div>
            );
          })}
        </div>
      )}
    </div>
  );
}
