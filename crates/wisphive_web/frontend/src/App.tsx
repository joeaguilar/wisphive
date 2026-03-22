import { useCallback, useEffect, useMemo, useState } from "react";
import { useWisphive } from "./hooks/useWisphive";
import { useKeyboard } from "./hooks/useKeyboard";
import { Queue } from "./components/Queue";
import { DetailView } from "./components/DetailView";
import { History } from "./components/History";
import { Sessions } from "./components/Sessions";
import { Projects } from "./components/Projects";
import { SpawnModal } from "./components/SpawnModal";
import "./app.css";

type View = "queue" | "history" | "sessions" | "projects";

function App() {
  const { connected, queue, agents, projects, history, sessions, approve, deny, spawnAgent, queryProjects, queryHistory, searchHistory, querySessions } = useWisphive();
  const [selectedId, setSelectedId] = useState<string | null>(null);
  const [view, setView] = useState<View>("queue");
  const [showSpawn, setShowSpawn] = useState(false);
  const [spawnDefaultProject, setSpawnDefaultProject] = useState<string | undefined>();
  const [sessionAgent, setSessionAgent] = useState<string | null>(null);
  const [sessionTimeline, setSessionTimeline] = useState(history);
  const [showHelp, setShowHelp] = useState(false);

  const selectedRequest = queue.find((r) => r.id === selectedId);

  // Queue index for keyboard navigation
  const queueIndex = queue.findIndex((r) => r.id === selectedId);

  const handleNext = useCallback(() => {
    if (view === "queue" && queue.length > 0) {
      const next = Math.min(queueIndex + 1, queue.length - 1);
      setSelectedId(queue[next >= 0 ? next : 0].id);
    }
  }, [view, queue, queueIndex]);

  const handlePrev = useCallback(() => {
    if (view === "queue" && queue.length > 0) {
      const prev = Math.max(queueIndex - 1, 0);
      setSelectedId(queue[prev].id);
    }
  }, [view, queue, queueIndex]);

  const keyActions = useMemo(() => ({
    onNext: handleNext,
    onPrev: handlePrev,
    onApprove: () => {
      if (selectedId && view === "queue") { approve(selectedId); setSelectedId(null); }
    },
    onDeny: () => {
      if (selectedId && view === "queue") { deny(selectedId); setSelectedId(null); }
    },
    onBack: () => {
      if (showHelp) { setShowHelp(false); return; }
      if (showSpawn) { setShowSpawn(false); return; }
      if (selectedId) { setSelectedId(null); return; }
      if (sessionAgent) { setSessionAgent(null); return; }
    },
    onSelect: () => {
      if (view === "queue" && queue.length > 0 && !selectedId) {
        setSelectedId(queue[0].id);
      }
    },
    onViewQueue: () => setView("queue"),
    onViewHistory: () => setView("history"),
    onViewSessions: () => setView("sessions"),
    onViewProjects: () => setView("projects"),
    onSpawn: () => setShowSpawn(true),
    onHelp: () => setShowHelp((v) => !v),
  }), [handleNext, handlePrev, selectedId, view, queue, approve, deny, showHelp, showSpawn, sessionAgent]);

  useKeyboard(keyActions);

  // Fetch projects when spawn modal opens
  useEffect(() => {
    if (showSpawn) queryProjects();
  }, [showSpawn, queryProjects]);

  // Keep session timeline in sync when history updates and we're viewing a timeline
  useEffect(() => {
    if (sessionAgent) setSessionTimeline(history);
  }, [history, sessionAgent]);

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
        <button className={view === "projects" ? "active" : ""} onClick={() => setView("projects")}>
          Projects
        </button>
        <button className="spawn-btn" onClick={() => setShowSpawn(true)}>
          + Spawn Agent
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
                onApprove={(id, opts) => { approve(id, opts); setSelectedId(null); }}
                onDeny={(id, msg) => { deny(id, msg); setSelectedId(null); }}
              />
            )}
          </div>
        )}
        {view === "history" && (
          <History
            entries={history}
            onLoad={queryHistory}
            onSearch={searchHistory}
          />
        )}
        {view === "sessions" && (
          <Sessions
            sessions={sessions}
            timeline={sessionTimeline}
            selectedAgent={sessionAgent}
            onLoad={querySessions}
            onSelectAgent={setSessionAgent}
            onLoadTimeline={(agentId) => queryHistory(agentId)}
          />
        )}
        {view === "projects" && (
          <Projects
            projects={projects}
            onLoad={queryProjects}
            onSpawnInProject={(project) => { setSpawnDefaultProject(project); setShowSpawn(true); }}
            onDrillDown={(project) => { searchHistory(project); setView("history"); }}
          />
        )}
      </main>

      {showHelp && (
        <div className="modal-overlay" onClick={() => setShowHelp(false)}>
          <div className="modal-content help-modal" onClick={(e) => e.stopPropagation()}>
            <div className="modal-header">
              <h2>Keyboard Shortcuts</h2>
              <button className="modal-close" onClick={() => setShowHelp(false)}>×</button>
            </div>
            <div className="help-grid">
              <div className="help-section">
                <h3>Navigation</h3>
                <div className="help-row"><kbd>j</kbd> / <kbd>↓</kbd> Next item</div>
                <div className="help-row"><kbd>k</kbd> / <kbd>↑</kbd> Previous item</div>
                <div className="help-row"><kbd>Enter</kbd> Select / expand</div>
                <div className="help-row"><kbd>Esc</kbd> Back / close</div>
              </div>
              <div className="help-section">
                <h3>Actions</h3>
                <div className="help-row"><kbd>y</kbd> Approve selected</div>
                <div className="help-row"><kbd>n</kbd> Deny selected</div>
                <div className="help-row"><kbd>N</kbd> Spawn agent</div>
              </div>
              <div className="help-section">
                <h3>Views</h3>
                <div className="help-row"><kbd>1</kbd> Queue</div>
                <div className="help-row"><kbd>2</kbd> History</div>
                <div className="help-row"><kbd>3</kbd> Sessions</div>
                <div className="help-row"><kbd>4</kbd> Projects</div>
                <div className="help-row"><kbd>?</kbd> This help</div>
              </div>
            </div>
          </div>
        </div>
      )}

      {showSpawn && (
        <SpawnModal
          projects={projects.map((p) => p.project)}
          defaultProject={spawnDefaultProject}
          onSpawn={(req) => { spawnAgent(req); setShowSpawn(false); setSpawnDefaultProject(undefined); }}
          onClose={() => { setShowSpawn(false); setSpawnDefaultProject(undefined); }}
        />
      )}
    </div>
  );
}

export default App;
