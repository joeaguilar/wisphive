import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import { useWisphive } from "./hooks/useWisphive";
import { useKeyboard } from "./hooks/useKeyboard";
import { useAuth } from "./hooks/useAuth";
import { useIsMobile } from "./hooks/useViewport";
import { Queue } from "./components/Queue";
import { Inbox } from "./components/Inbox";
import { Board } from "./components/Board";
import type { InboxTarget } from "./components/liveness";
import { orderByAge } from "./components/queueUtils";
import { DetailView } from "./components/DetailView";
import { History } from "./components/History";
import { Sessions } from "./components/Sessions";
import { Projects } from "./components/Projects";
import { Worktrees } from "./components/Worktrees";
import { Burn } from "./components/Burn";
import { Agents } from "./components/Agents";
import { SpawnModal } from "./components/SpawnModal";
import { Modal } from "./components/Modal";
import { ConfigView } from "./components/Config";
import { Terminals } from "./components/Terminals";
import { Login } from "./components/Login";
import { SudoModal } from "./components/SudoModal";
import { DiskAlertBanner } from "./components/DiskAlertBanner";
import { ConfigAlertBanner } from "./components/ConfigAlertBanner";
import "./app.css";

type View = "inbox" | "board" | "queue" | "history" | "sessions" | "projects" | "worktrees" | "burn" | "agents" | "config" | "terminals";

function App() {
  const auth = useAuth();

  // Gate the entire shell behind auth — mounting useWisphive before we
  // have a token means the WS hook churns against the bearer-gated /ws
  // endpoint for no reason. Hooks must run unconditionally, so split the
  // authenticated shell into its own component.
  if (auth.phase === "loading") {
    return <div className="app-loading">Loading…</div>;
  }
  // Keep Login mounted across `unauthed`, `setup`, AND the transient
  // `authed-pending-enroll` state. The last one is what gives Login.tsx
  // a render window to show the optional passkey-enroll card after a
  // successful set-password (a synchronous setPhase + a local setState
  // in Login were previously batched by React 19, unmounting Login
  // before the enroll card could appear). Login.tsx drives the
  // transition out of `authed-pending-enroll` by calling
  // `auth.completeEnrollGate` once the user enrolls or skips.
  if (auth.phase !== "authed") {
    return (
      <Login
        phase={auth.phase}
        error={auth.error}
        onLogin={auth.login}
        onSetPassword={auth.setPassword}
        onCompleteEnrollGate={auth.completeEnrollGate}
        onClearError={auth.clearError}
        onRefreshStatus={auth.refreshStatus}
      />
    );
  }
  return <AuthedApp onLogout={auth.logout} />;
}

function AuthedApp({ onLogout }: { onLogout: () => Promise<void> }) {
  const {
    connected, queue, agents, projects, worktrees, burnTouches, hookStatus, hookErrors, auditDecisions, endedAgentIds, history, agentTimeline, sessionTimeline, sessions, terminals,
    pendingReauth, diskAlerts, configAlerts, approve, deny, dismissReauth, retryPendingApprove,
    spawnAgent, queryProjects, queryWorktrees, queryBurn, installHooks, queryProjectHookStatus, queryHistory, queryAgentTimeline, querySessionTimeline, searchHistory, querySessions,
    termList, termCreate, termAttach, termDetach, termInput, termResize, termClose, termReplay, termSetGroup, termReorder, registerTerminalHandler,
  } = useWisphive();
  const [selectedId, setSelectedId] = useState<string | null>(null);
  const [view, setView] = useState<View>("inbox");
  // App-lifetime breakpoint subscriber (itr#487): keeps the :root
  // `--is-mobile` / `data-viewport` flags synced for any CSS that keys on
  // them, independent of which view is mounted.
  useIsMobile();
  // Deep-link target for the inbox "Focus terminal" affordance on deferred
  // native prompts (itr#437): setting it navigates to the Terminals view and
  // tells Terminals which session to auto-select. Cleared once Terminals
  // honours it so the same session can be re-focused later.
  const [focusTerminalId, setFocusTerminalId] = useState<string | null>(null);
  // Deep-link target for the liveness board's "Answer in Inbox" cross-link on
  // a waiting-on-input lane (itr#400): a deferred row's stable key. Cleared
  // once the Inbox has expanded/scrolled the row so it can be re-focused.
  const [focusDeferredKey, setFocusDeferredKey] = useState<string | null>(null);
  const [showSpawn, setShowSpawn] = useState(false);
  const [spawnDefaultProject, setSpawnDefaultProject] = useState<string | undefined>();
  const [sessionAgent, setSessionAgent] = useState<string | null>(null);
  const [agentDrilldown, setAgentDrilldown] = useState<string | null>(null);
  const [showHelp, setShowHelp] = useState(false);
  const sidebarRef = useRef<HTMLElement>(null);

  const selectedRequest = queue.find((r) => r.id === selectedId);

  // Board → Inbox cross-link (itr#400): a queued decision selects its inbox
  // row (which renders the full DetailView); a deferred native prompt expands
  // its "waiting in your terminal" row via focusDeferredKey.
  const openInboxTarget = useCallback((target: InboxTarget) => {
    if (target.kind === "decision") {
      setSelectedId(target.requestId);
      setFocusDeferredKey(null);
    } else {
      setSelectedId(null);
      setFocusDeferredKey(target.deferredKey);
    }
    setView("inbox");
  }, []);

  // The burn meter's pull feeds: artifact-candidate rows (query_burn) plus the
  // session aggregates that type its tiles. Stable identity so the meter's
  // poll effect doesn't churn.
  const loadBurn = useCallback(() => {
    queryBurn();
    querySessions();
  }, [queryBurn, querySessions]);

  // The keyboard-navigation list must match the on-screen order of the active
  // view: the Inbox renders oldest-first (orderByAge), the Queue renders raw
  // insertion order. Walking the wrong order made j/k/y/n act on an item other
  // than the one highlighted.
  const navList = useMemo(
    () => (view === "inbox" ? orderByAge(queue) : queue),
    [view, queue],
  );
  const navIndex = navList.findIndex((r) => r.id === selectedId);

  const handleNext = useCallback(() => {
    if ((view === "queue" || view === "inbox") && navList.length > 0) {
      const next = Math.min(navIndex + 1, navList.length - 1);
      setSelectedId(navList[next >= 0 ? next : 0].id);
    }
  }, [view, navList, navIndex]);

  const handlePrev = useCallback(() => {
    if ((view === "queue" || view === "inbox") && navList.length > 0) {
      const prev = Math.max(navIndex - 1, 0);
      setSelectedId(navList[prev].id);
    }
  }, [view, navList, navIndex]);

  const keyActions = useMemo(() => ({
    onNext: handleNext,
    onPrev: handlePrev,
    onApprove: () => {
      if (selectedId && (view === "queue" || view === "inbox")) { approve(selectedId); setSelectedId(null); }
    },
    onDeny: () => {
      if (selectedId && (view === "queue" || view === "inbox")) { deny(selectedId); setSelectedId(null); }
    },
    onBack: () => {
      if (showHelp) { setShowHelp(false); return; }
      if (showSpawn) { setShowSpawn(false); return; }
      if (selectedId) { setSelectedId(null); return; }
      if (agentDrilldown) { setAgentDrilldown(null); return; }
      if (sessionAgent) { setSessionAgent(null); return; }
    },
    onSelect: () => {
      if ((view === "queue" || view === "inbox") && navList.length > 0 && !selectedId) {
        setSelectedId(navList[0].id);
      }
    },
    onViewQueue: () => setView("queue"),
    onViewBoard: () => setView("board"),
    onViewHistory: () => setView("history"),
    onViewSessions: () => setView("sessions"),
    onViewProjects: () => setView("projects"),
    onViewAgents: () => setView("agents"),
    onViewConfig: () => setView("config"),
    onViewTerminals: () => setView("terminals"),
    onViewWorktrees: () => setView("worktrees"),
    onViewBurn: () => setView("burn"),
    onSpawn: () => setShowSpawn(true),
    onHelp: () => setShowHelp((v) => !v),
  }), [handleNext, handlePrev, selectedId, view, navList, approve, deny, showHelp, showSpawn, agentDrilldown, sessionAgent]);

  useKeyboard(keyActions);

  // Fetch projects when spawn modal opens
  useEffect(() => {
    if (showSpawn) queryProjects();
  }, [showSpawn, queryProjects]);

  return (
    <div className="app">
      <nav ref={sidebarRef} className="sidebar">
        <div className="sidebar-header">
          <h1>wisphive</h1>
          <span
            className={`status-dot ${connected ? "connected" : "disconnected"}`}
            role="status"
            aria-label={connected ? "Daemon connected" : "Daemon disconnected — reconnecting"}
            title={connected ? "Daemon connected" : "Daemon disconnected — reconnecting"}
          />
        </div>
        <button className={view === "inbox" ? "active" : ""} onClick={() => setView("inbox")}>
          Inbox {queue.length > 0 && <span className="badge">{queue.length}</span>}
        </button>
        <button className={view === "board" ? "active" : ""} onClick={() => setView("board")}>
          Board
        </button>
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
        <button className={view === "worktrees" ? "active" : ""} onClick={() => setView("worktrees")}>
          Worktrees
        </button>
        <button className={view === "burn" ? "active" : ""} onClick={() => setView("burn")}>
          Burn
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
          <h2>Agents ({agents.length})</h2>
          {agents.map((a) => (
            <div key={a.agent_id} className="agent-item">
              {a.agent_id.slice(0, 12)}
            </div>
          ))}
        </div>
      </nav>

      <main className="content">
        <DiskAlertBanner alerts={diskAlerts} />
        <ConfigAlertBanner alerts={configAlerts} />
        {view === "inbox" && (
          <Inbox
            items={queue}
            auditDecisions={auditDecisions}
            endedAgentIds={endedAgentIds}
            selectedId={selectedId}
            onSelect={setSelectedId}
            onApprove={(id, opts) => { approve(id, opts); setSelectedId(null); }}
            onDeny={(id, msg) => { deny(id, msg); setSelectedId(null); }}
            onFocusTerminal={(termId) => { setFocusTerminalId(termId); setView("terminals"); }}
            focusDeferredKey={focusDeferredKey}
            onFocusDeferredHandled={() => setFocusDeferredKey(null)}
          />
        )}
        {view === "board" && (
          <Board
            sessions={sessions}
            agents={agents}
            queue={queue}
            auditDecisions={auditDecisions}
            endedAgentIds={endedAgentIds}
            onLoad={querySessions}
            onOpenInbox={openInboxTarget}
          />
        )}
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
            hookStatus={hookStatus}
            hookErrors={hookErrors}
            onLoad={queryProjects}
            onSpawnInProject={(project) => { setSpawnDefaultProject(project); setShowSpawn(true); }}
            onDrillDown={(project) => { searchHistory(project); setView("history"); }}
            onInstallHooks={installHooks}
            onQueryHookStatus={queryProjectHookStatus}
          />
        )}
        {view === "worktrees" && (
          <Worktrees worktrees={worktrees} onLoad={queryWorktrees} />
        )}
        {view === "burn" && (
          <Burn
            sessions={sessions}
            auditDecisions={auditDecisions}
            burnTouches={burnTouches}
            onLoad={loadBurn}
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
            focusSessionId={focusTerminalId ?? undefined}
            onFocusHandled={() => setFocusTerminalId(null)}
            backgroundRef={sidebarRef}
            registerHandler={registerTerminalHandler}
          />
        )}
        {view === "config" && <ConfigView />}
      </main>

      {showHelp && (
        <Modal title="Keyboard Shortcuts" className="help-modal" onClose={() => setShowHelp(false)}>
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
              <div className="help-row"><kbd>7</kbd> Terminals</div>
              <div className="help-row"><kbd>8</kbd> Board</div>
              <div className="help-row"><kbd>9</kbd> Worktrees</div>
              <div className="help-row"><kbd>0</kbd> Burn</div>
              <div className="help-row"><kbd>?</kbd> This help</div>
            </div>
          </div>
        </Modal>
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
