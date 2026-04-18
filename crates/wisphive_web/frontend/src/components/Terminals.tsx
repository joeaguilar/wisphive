import { useEffect, useMemo, useState } from "react";
import type { ProjectSummary, TerminalSessionMeta } from "../types/protocol";
import { TerminalView } from "./TerminalView";

interface TerminalsProps {
  terminals: TerminalSessionMeta[];
  projects: ProjectSummary[];
  onRefresh: () => void;
  onRefreshProjects: () => void;
  onCreate: (opts: { label?: string; cwd?: string; cols: number; rows: number }) => void;
  onAttach: (id: string) => void;
  onDetach: (id: string) => void;
  onClose: (id: string) => void;
  onReplay: (id: string, fromSeq?: number) => void;
  onInput: (id: string, data: string) => void;
  onResize: (id: string, cols: number, rows: number) => void;
  registerHandler: (
    id: string,
    handler: (id: string, direction: "chunk" | "catchup" | "replay_chunk", bytes: Uint8Array) => void,
  ) => () => void;
}

export function Terminals(props: TerminalsProps) {
  const {
    terminals, projects, onRefresh, onRefreshProjects, onCreate, onAttach, onDetach, onClose, onReplay, onInput, onResize,
    registerHandler,
  } = props;
  const [selectedId, setSelectedId] = useState<string | null>(null);
  const [replayMode, setReplayMode] = useState(false);
  const [showProjectPicker, setShowProjectPicker] = useState(false);

  useEffect(() => {
    onRefresh();
    onRefreshProjects();
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, []);

  const selected = useMemo(
    () => terminals.find((t) => t.id === selectedId) ?? null,
    [terminals, selectedId],
  );

  const handleCreate = () => {
    const label = prompt("Label for the new terminal session?") ?? undefined;
    onCreate({ label: label || undefined, cols: 120, rows: 32 });
  };

  const handleCreateInProject = (project: ProjectSummary) => {
    const projectName = project.project.split("/").filter(Boolean).pop() ?? project.project;
    onCreate({ label: projectName, cwd: project.project, cols: 120, rows: 32 });
    setShowProjectPicker(false);
  };

  const handleSelect = (t: TerminalSessionMeta, replay: boolean) => {
    // Detach from any previously selected terminal so we don't leak forwarders.
    if (selectedId && selectedId !== t.id) {
      onDetach(selectedId);
    }
    setSelectedId(t.id);
    setReplayMode(replay);
    if (replay) {
      onReplay(t.id);
    } else {
      onAttach(t.id);
    }
  };

  return (
    <div className="terminals-layout">
      <div className="terminals-sidebar">
        <div className="terminals-sidebar-toolbar">
          <button onClick={handleCreate}>+ New terminal</button>
          <button onClick={() => { onRefreshProjects(); setShowProjectPicker(true); }}>
            + In project…
          </button>
          <button onClick={onRefresh}>Refresh</button>
        </div>
        {showProjectPicker && (
          <div className="terminals-project-picker">
            <div className="terminals-project-picker-header">
              <strong>Open terminal in project</strong>
              <button onClick={() => setShowProjectPicker(false)}>×</button>
            </div>
            {projects.length === 0 && (
              <div className="terminals-project-picker-empty">
                No known projects yet — start an agent or run a tool in one first.
              </div>
            )}
            {projects.map((p) => (
              <div
                key={p.project}
                className="terminals-project-picker-item"
                onClick={() => handleCreateInProject(p)}
              >
                <strong>{p.project.split("/").filter(Boolean).pop() ?? p.project}</strong>
                <div className="path">{p.project}</div>
              </div>
            ))}
          </div>
        )}
        <div className="terminals-sidebar-list">
          {terminals.length === 0 && (
            <div className="terminals-sidebar-empty">
              No terminal sessions yet. Press "New terminal" to spawn one.
            </div>
          )}
          {terminals.map((t) => (
            <div
              key={t.id}
              onClick={() => handleSelect(t, t.status !== "running")}
              className={`terminals-sidebar-item${t.id === selectedId ? " selected" : ""}`}
            >
              <div className="row">
                <strong>{t.label ?? "(no label)"}</strong>
                <span className={`term-status term-status-${t.status}`}>{t.status}</span>
              </div>
              <div className="cmd">{t.command} {t.args.join(" ")}</div>
              <div className="meta">
                {t.cwd} · {t.cols}×{t.rows} · {new Date(t.started_at).toLocaleString()}
              </div>
              <div className="actions">
                {t.status === "running" && (
                  <>
                    <button onClick={(e) => { e.stopPropagation(); handleSelect(t, false); }}>Attach</button>
                    <button onClick={(e) => { e.stopPropagation(); onClose(t.id); }}>Kill</button>
                  </>
                )}
                <button onClick={(e) => { e.stopPropagation(); handleSelect(t, true); }}>Replay</button>
              </div>
            </div>
          ))}
        </div>
      </div>

      <div className="terminals-main">
        {selected ? (
          <TerminalView
            key={`${selected.id}-${replayMode ? "replay" : "live"}`}
            session={selected}
            replayMode={replayMode}
            onInput={onInput}
            onResize={onResize}
            registerHandler={registerHandler}
          />
        ) : (
          <div className="terminals-empty-state">
            Select a terminal to view it live, or press "New terminal".
          </div>
        )}
      </div>
    </div>
  );
}
