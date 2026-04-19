import { useEffect, useState } from "react";
import type { HistoryEntry } from "../types/protocol";
import { HistoryEntryItem } from "./HistoryEntryItem";

interface HistoryProps {
  entries: HistoryEntry[];
  onLoad: (agentId?: string) => void;
  onSearch: (query: string) => void;
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
        <button className="btn-secondary" onClick={() => onLoad(agentFilter ?? undefined)}>Refresh</button>
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
            <HistoryEntryItem
              key={entry.id}
              entry={entry}
              expanded={expandedId === entry.id}
              onToggle={() => setExpandedId(expandedId === entry.id ? null : entry.id)}
              onAgentClick={(aid) => setAgentFilter(aid)}
            />
          ))}
        </div>
      )}
    </div>
  );
}
