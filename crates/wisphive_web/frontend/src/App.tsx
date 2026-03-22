import { useEffect, useState } from "react";
import { useWisphive } from "./hooks/useWisphive";
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

  const selectedRequest = queue.find((r) => r.id === selectedId);

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
