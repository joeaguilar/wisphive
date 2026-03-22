import { useEffect, useState } from "react";
import type { HistoryEntry } from "../types/protocol";

interface HistoryProps {
  entries: HistoryEntry[];
  onLoad: (agentId?: string) => void;
  onSearch: (query: string) => void;
}

function decisionBadge(decision: string) {
  const d = decision.replace(/"/g, "");
  const cls = d === "approve" ? "badge-approve" : d === "deny" ? "badge-deny" : "badge-defer";
  return <span className={`decision-badge ${cls}`}>{d.toUpperCase()}</span>;
}

function formatTime(ts: string) {
  return new Date(ts).toLocaleString();
}

export function History({ entries, onLoad, onSearch }: HistoryProps) {
  const [search, setSearch] = useState("");
  const [agentFilter, setAgentFilter] = useState<string | null>(null);
  const [expandedId, setExpandedId] = useState<string | null>(null);

  useEffect(() => {
    onLoad(agentFilter ?? undefined);
  }, [onLoad, agentFilter]);

  const handleSearch = () => {
    if (search.trim()) {
      onSearch(search.trim());
    } else {
      onLoad(agentFilter ?? undefined);
    }
  };

  return (
    <div className="history-view">
      <div className="history-toolbar">
        <input
          type="text"
          className="history-search"
          placeholder="Search history..."
          value={search}
          onChange={(e) => setSearch(e.target.value)}
          onKeyDown={(e) => { if (e.key === "Enter") handleSearch(); }}
        />
        <button className="btn-secondary" onClick={handleSearch}>Search</button>
        {(agentFilter || search) && (
          <button className="btn-secondary" onClick={() => { setAgentFilter(null); setSearch(""); onLoad(); }}>
            Clear filters
          </button>
        )}
        {agentFilter && <span className="filter-tag">Agent: {agentFilter.slice(0, 16)}</span>}
      </div>

      {entries.length === 0 ? (
        <div className="history-empty">No history entries</div>
      ) : (
        <div className="history-list">
          {entries.map((entry) => (
            <div key={entry.id} className="history-item" onClick={() => setExpandedId(expandedId === entry.id ? null : entry.id)}>
              <div className="history-item-row">
                {decisionBadge(entry.decision)}
                <span className="tool-name">{entry.tool_name}</span>
                <span
                  className="agent-link"
                  onClick={(e) => { e.stopPropagation(); setAgentFilter(entry.agent_id); }}
                >
                  {entry.agent_id.slice(0, 16)}
                </span>
                <span className="time-ago">{formatTime(entry.resolved_at)}</span>
                {entry.tool_result && <span className="result-indicator">+</span>}
              </div>
              {expandedId === entry.id && (
                <div className="history-detail">
                  <div className="detail-meta">
                    <div><strong>Agent:</strong> {entry.agent_id}</div>
                    <div><strong>Project:</strong> {entry.project}</div>
                    <div><strong>Requested:</strong> {formatTime(entry.requested_at)}</div>
                    <div><strong>Resolved:</strong> {formatTime(entry.resolved_at)}</div>
                  </div>
                  <div className="detail-section">
                    <h3>Tool Input</h3>
                    <pre className="code-block">{JSON.stringify(entry.tool_input, null, 2)}</pre>
                  </div>
                  {entry.tool_result && (
                    <div className="detail-section">
                      <h3>Tool Result</h3>
                      <pre className="code-block">{JSON.stringify(entry.tool_result, null, 2)}</pre>
                    </div>
                  )}
                </div>
              )}
            </div>
          ))}
        </div>
      )}
    </div>
  );
}
