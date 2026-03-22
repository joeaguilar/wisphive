import { useState } from "react";
import { useWisphive } from "./hooks/useWisphive";
import { Queue } from "./components/Queue";
import { DetailView } from "./components/DetailView";
import "./app.css";

type View = "queue" | "history" | "sessions";

function App() {
  const { connected, queue, agents, approve, deny } = useWisphive();
  const [selectedId, setSelectedId] = useState<string | null>(null);
  const [view, setView] = useState<View>("queue");

  const selectedRequest = queue.find((r) => r.id === selectedId);

  return (
    <div className="app">
      <nav className="sidebar">
        <div className="sidebar-header">
          <h1>wisphive</h1>
          <span className={`status-dot ${connected ? "connected" : "disconnected"}`} />
        </div>
        <button className={view === "queue" ? "active" : ""} onClick={() => setView("queue")}>
          Queue {queue.length > 0 && <span className="badge">{queue.length}</span>}
        </button>
        <button className={view === "history" ? "active" : ""} onClick={() => setView("history")}>
          History
        </button>
        <button className={view === "sessions" ? "active" : ""} onClick={() => setView("sessions")}>
          Sessions
        </button>
        <div className="sidebar-agents">
          <h3>Agents ({agents.length})</h3>
          {agents.map((a) => (
            <div key={a.agent_id} className="agent-item">
              {a.agent_id.slice(0, 12)}
            </div>
          ))}
        </div>
      </nav>

      <main className="content">
        {view === "queue" && (
          <div className="queue-layout">
            <Queue
              items={queue}
              selectedId={selectedId}
              onSelect={setSelectedId}
              onApprove={(id) => { approve(id); setSelectedId(null); }}
              onDeny={(id) => { deny(id); setSelectedId(null); }}
            />
            {selectedRequest && (
              <DetailView
                request={selectedRequest}
                onApprove={(id) => { approve(id); setSelectedId(null); }}
                onDeny={(id, msg) => { deny(id, msg); setSelectedId(null); }}
              />
            )}
          </div>
        )}
        {view === "history" && (
          <div className="placeholder">
            <p>History view — coming soon</p>
          </div>
        )}
        {view === "sessions" && (
          <div className="placeholder">
            <p>Sessions view — coming soon</p>
          </div>
        )}
      </main>
    </div>
  );
}

export default App;
