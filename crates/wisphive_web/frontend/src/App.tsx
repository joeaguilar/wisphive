import { useCallback, useEffect, useMemo, useState } from "react";
import { useWisphive } from "./hooks/useWisphive";
import { useKeyboard } from "./hooks/useKeyboard";
import { useAuth } from "./hooks/useAuth";
import { Queue } from "./components/Queue";
import { DetailView } from "./components/DetailView";
import { History } from "./components/History";
import { Sessions } from "./components/Sessions";
import { Projects } from "./components/Projects";
import { Agents } from "./components/Agents";
import { SpawnModal } from "./components/SpawnModal";
import { ConfigView } from "./components/Config";
import { Terminals } from "./components/Terminals";
import { Login } from "./components/Login";
import { SudoModal } from "./components/SudoModal";
import "./app.css";

type View = "queue" | "history" | "sessions" | "projects" | "agents" | "config" | "terminals";

function App() {
  const auth = useAuth();

  // Gate the entire shell behind auth — mounting useWisphive before we
  // have a token means the WS hook churns against the bearer-gated /ws
  // endpoint for no reason. Hooks must run unconditionally, so split the
  // authenticated shell into its own component.
  if (auth.phase === "loading") {
    return <div className="app-loading">Loading…</div>;
  }
  if (auth.phase !== "authed") {
    return (
      <Login
        phase={auth.phase}
        error={auth.error}
        onLogin={auth.login}
        onClearError={auth.clearError}
        onRefreshStatus={auth.refreshStatus}
      />
    );
  }
  return <AuthedApp onLogout={auth.logout} />;
}

function AuthedApp({ onLogout }: { onLogout: () => Promise<void> }) {
  const {
    connected, queue, agents, projects, history, agentTimeline, sessionTimeline, sessions, terminals,
    pendingReauth, approve, deny, dismissReauth, retryPendingApprove,
    spawnAgent, queryProjects, queryHistory, queryAgentTimeline, querySessionTimeline, searchHistory, querySessions,
    termList, termCreate, termAttach, termDetach, termInput, termResize, termClose, termReplay, termSetGroup, termReorder, registerTerminalHandler,
  } = useWisphive();
  const [selectedId, setSelectedId] = useState<string | null>(null);
  const [view, setView] = useState<View>("queue");
  const [showSpawn, setShowSpawn] = useState(false);
  const [spawnDefaultProject, setSpawnDefaultProject] = useState<string | undefined>();
  const [sessionAgent, setSessionAgent] = useState<string | null>(null);
  const [agentDrilldown, setAgentDrilldown] = useState<string | null>(null);
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
      if (agentDrilldown) { setAgentDrilldown(null); return; }
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
    onViewAgents: () => setView("agents"),
    onViewConfig: () => setView("config"),
    onViewTerminals: () => setView("terminals"),
    onSpawn: () => setShowSpawn(true),
    onHelp: () => setShowHelp((v) => !v),
  }), [handleNext, handlePrev, selectedId, view, queue, approve, deny, showHelp, showSpawn, agentDrilldown, sessionAgent]);

  useKeyboard(keyActions);

  // Fetch projects when spawn modal opens
  useEffect(() => {
    if (showSpawn) queryProjects();
  }, [showSpawn, queryProjects]);

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
        <button className={view === "agents" ? "active" : ""} onClick={() => setView("agents")}>
          Agents {agents.length > 0 && <span className="badge">{agents.length}</span>}
        </button>
        <button className={view === "terminals" ? "active" : ""} onClick={() => setView("terminals")}>
          Terminals {terminals.filter((t) => t.status === "running").length > 0 && <span className="badge">{terminals.filter((t) => t.status === "running").length}</span>}
        </button>
        <button className={view === "config" ? "active" : ""} onClick={() => setView("config")}>
          Config
        </button>
        <button className="spawn-btn" onClick={() => setShowSpawn(true)}>
          + Spawn Agent
        </button>
        <button className="logout-btn" onClick={() => void onLogout()}>
          Sign out
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
            onLoadTimeline={querySessionTimeline}
            onRefreshTimeline={querySessionTimeline}
          />
        )}
        {view === "agents" && (
          <Agents
            agents={agents}
            queue={queue}
            timeline={agentTimeline}
            selectedAgent={agentDrilldown}
            onSelectAgent={setAgentDrilldown}
            onLoadTimeline={queryAgentTimeline}
            onRefreshTimeline={queryAgentTimeline}
            onApprove={(id) => approve(id)}
            onDeny={(id) => deny(id)}
            onSpawn={() => setShowSpawn(true)}
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
        {view === "terminals" && (
          <Terminals
            terminals={terminals}
            queue={queue}
            projects={projects}
            onRefresh={termList}
            onRefreshProjects={queryProjects}
            onCreate={termCreate}
            onAttach={termAttach}
            onDetach={termDetach}
            onClose={termClose}
            onReplay={termReplay}
            onInput={termInput}
            onResize={termResize}
            onSetGroup={termSetGroup}
            onReorder={termReorder}
            onApprove={(id, opts) => approve(id, opts)}
            onDeny={(id, msg) => deny(id, msg)}
            onJumpToQueue={() => setView("queue")}
            registerHandler={registerTerminalHandler}
          />
        )}
        {view === "config" && <ConfigView />}
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
                <div className="help-row"><kbd>5</kbd> Agents</div>
                <div className="help-row"><kbd>6</kbd> Config</div>
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

      {pendingReauth && (
        <SudoModal
          toolName={pendingReauth.tool_name}
          onCancel={dismissReauth}
          onSuccess={retryPendingApprove}
        />
      )}
    </div>
  );
}

export default App;
