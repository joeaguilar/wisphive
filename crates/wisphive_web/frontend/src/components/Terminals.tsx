import { useCallback, useEffect, useMemo, useRef, useState, type RefObject } from "react";
import type { DecisionRequest, ProjectSummary, TerminalSessionMeta } from "../types/protocol";
import { TerminalView } from "./TerminalView";
import { TerminalQueueDock } from "./TerminalQueueDock";
import { activate } from "./a11y";
import { useIsMobile } from "../hooks/useViewport";

interface TerminalsProps {
  terminals: TerminalSessionMeta[];
  queue: DecisionRequest[];
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
  onSetGroup: (id: string, group?: string) => void;
  onReorder: (id: string, sortOrder: number) => void;
  onApprove: (id: string, opts?: { additional_context?: string; always_allow?: boolean }) => void;
  onDeny: (id: string, message?: string) => void;
  onJumpToQueue: () => void;
  /** External deep-link focus (itr#437): when set to a live terminal's id the
   * Terminals view auto-selects + attaches it, so the inbox "Focus terminal"
   * CTA lands on the exact session. `onFocusHandled` clears it upstream. */
  focusSessionId?: string;
  onFocusHandled?: () => void;
  /** The application shell hidden behind the mobile terminal dialog. */
  backgroundRef: RefObject<HTMLElement | null>;
  registerHandler: (
    id: string,
    handler: (id: string, direction: "chunk" | "catchup" | "replay_chunk", bytes: Uint8Array) => void,
    options?: { replayMode?: boolean },
  ) => () => void;
}

// Fractional-index gap: start wide so many inserts fit between neighbors
// without requiring re-normalization.
const SORT_GAP = 1_000_000;
const UNGROUPED_KEY = "__ungrouped__";
// Bounded wait for a deep-link focus target to show up in `terminals` before
// the request is declared stale (itr#449). The view refreshes term_list on
// mount, so a live target arrives well inside this window; anything slower is
// a dead pointer (session ended, or never an embedded wisphive terminal).
const FOCUS_WAIT_MS = 2_000;
const FOCUSABLE =
  'a[href], button:not([disabled]), textarea:not([disabled]), input:not([disabled]), select:not([disabled]), [tabindex]:not([tabindex="-1"])';

interface DragPayload {
  id: string;
  fromGroup: string | null;
}

export function Terminals(props: TerminalsProps) {
  const {
    terminals, queue, projects, onRefresh, onRefreshProjects, onCreate, onAttach, onDetach,
    onClose, onReplay, onInput, onResize, onSetGroup, onReorder,
    onApprove, onDeny, onJumpToQueue, focusSessionId, onFocusHandled, backgroundRef, registerHandler,
  } = props;
  const [selectedId, setSelectedId] = useState<string | null>(null);
  const [replayMode, setReplayMode] = useState(false);
  // Feedback for a deep-link focus whose target never appeared (itr#449) —
  // rendered as a dismissible inline notice instead of a silent no-op.
  const [focusNotice, setFocusNotice] = useState<string | null>(null);
  // Deadline anchor for the pending focus request, keyed by session id so
  // unrelated `terminals` updates re-running the effect can't extend the wait.
  const focusWaitRef = useRef<{ id: string; deadline: number } | null>(null);
  const isMobile = useIsMobile();
  const [showProjectPicker, setShowProjectPicker] = useState(false);
  const [orphanedOpen, setOrphanedOpen] = useState<boolean>(() => readBool("wisphive.terminals.orphaned.open", true));
  const [archivedOpen, setArchivedOpen] = useState<boolean>(() => readBool("wisphive.terminals.archived.open", false));
  // Empty "virtual" groups created in the UI (no sessions yet) that should
  // still render as drop targets. Not persisted server-side until a session
  // lands in them.
  const [pendingGroups, setPendingGroups] = useState<string[]>([]);
  const [renamingGroup, setRenamingGroup] = useState<string | null>(null);
  const [renameDraft, setRenameDraft] = useState("");
  const [drag, setDrag] = useState<DragPayload | null>(null);
  const [dropHint, setDropHint] = useState<{ group: string; index: number } | null>(null);
  const mobileDialogRef = useRef<HTMLDivElement>(null);
  // The daemon owns one long-lived forwarder for each live attachment. Keep
  // that imperative resource in a ref so event handlers and unmount cleanup
  // always act on the current attachment rather than a stale render snapshot.
  const liveSelectionRef = useRef<string | null>(null);
  const onDetachRef = useRef(onDetach);

  useEffect(() => {
    onRefresh();
    onRefreshProjects();
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, []);

  useEffect(() => {
    onDetachRef.current = onDetach;
  }, [onDetach]);

  // Leaving the Terminals view must stop any live daemon forwarder. The ref
  // is cleared before sending so React StrictMode cleanup and any later stale
  // cleanup cannot detach the same attachment twice. Replay-only selections
  // never enter this ref and therefore do not emit a spurious detach.
  useEffect(() => () => {
    const liveId = liveSelectionRef.current;
    liveSelectionRef.current = null;
    if (liveId) onDetachRef.current(liveId);
  }, []);

  // Persist collapsed-section state.
  useEffect(() => { writeBool("wisphive.terminals.orphaned.open", orphanedOpen); }, [orphanedOpen]);
  useEffect(() => { writeBool("wisphive.terminals.archived.open", archivedOpen); }, [archivedOpen]);

  const selected = useMemo(
    () => terminals.find((t) => t.id === selectedId) ?? null,
    [terminals, selectedId],
  );
  const mobileSubwindowOpen = isMobile && selected !== null;

  // The fixed mobile terminal sub-window covers the app navigation, but that
  // navigation remains in the DOM. App owns the shell element, so it supplies
  // a ref rather than coupling this component to App's CSS class names.
  useEffect(() => {
    if (!mobileSubwindowOpen) return;
    const backgroundShell = backgroundRef.current;
    if (!backgroundShell) return;
    const wasInert = backgroundShell.hasAttribute("inert");
    backgroundShell.setAttribute("inert", "");
    return () => {
      if (!wasInert) backgroundShell.removeAttribute("inert");
    };
  }, [backgroundRef, mobileSubwindowOpen]);

  // Native inert keeps focus out of the background in current browsers, but
  // aria-modal alone does not protect keyboard users on older browsers. Keep
  // a small Tab trap active for the mobile dialog in every browser so it also
  // acts as the non-native-inert fallback.
  useEffect(() => {
    if (!mobileSubwindowOpen) return;
    const dialog = mobileDialogRef.current;
    if (!dialog) return;

    const handleKeyDown = (event: KeyboardEvent) => {
      if (event.key !== "Tab") return;
      const focusables = [...dialog.querySelectorAll<HTMLElement>(FOCUSABLE)].filter(
        (element) => element.offsetParent !== null || element === document.activeElement,
      );
      const active = document.activeElement;

      // If focus began in the occluded background (possible without native
      // inert), pull it into the appropriate edge of the dialog. Leave focus
      // alone when another App-level modal is stacked above this one.
      if (!dialog.contains(active)) {
        if (backgroundRef.current?.contains(active)) {
          event.preventDefault();
          ((event.shiftKey ? focusables.at(-1) : focusables[0]) ?? dialog).focus();
        }
        return;
      }

      if (focusables.length === 0) {
        event.preventDefault();
        dialog.focus();
        return;
      }

      const first = focusables[0];
      const last = focusables.at(-1)!;
      if (event.shiftKey ? active === first || active === dialog : active === last) {
        event.preventDefault();
        (event.shiftKey ? last : first).focus();
      }
    };

    document.addEventListener("keydown", handleKeyDown, true);
    return () => document.removeEventListener("keydown", handleKeyDown, true);
  }, [backgroundRef, mobileSubwindowOpen]);

  // Count pending approvals per terminal by joining the queue on terminal_session_id.
  const pendingByTerminal = useMemo(() => {
    const map = new Map<string, number>();
    for (const r of queue) {
      if (r.terminal_session_id) {
        map.set(r.terminal_session_id, (map.get(r.terminal_session_id) ?? 0) + 1);
      }
    }
    return map;
  }, [queue]);

  const terminalPending = useMemo(
    () => (selected ? queue.filter((r) => r.terminal_session_id === selected.id) : []),
    [queue, selected],
  );
  const otherPendingCount = queue.length - terminalPending.length;

  // Section partition: active = running; orphaned = own section; archived = exited + killed.
  const { active, orphaned, archived } = useMemo(() => {
    const active: TerminalSessionMeta[] = [];
    const orphaned: TerminalSessionMeta[] = [];
    const archived: TerminalSessionMeta[] = [];
    for (const t of terminals) {
      if (t.status === "running") active.push(t);
      else if (t.status === "orphaned") orphaned.push(t);
      else archived.push(t); // exited + killed
    }
    return { active, orphaned, archived };
  }, [terminals]);

  // Group active sessions by group_name (null → ungrouped bucket).
  const activeGroups = useMemo(() => {
    const groups = new Map<string, TerminalSessionMeta[]>();
    // Seed with any pending (empty) groups the user just created so they
    // render as drop targets.
    groups.set(UNGROUPED_KEY, []);
    for (const g of pendingGroups) groups.set(g, []);
    for (const t of active) {
      const key = t.group_name ?? UNGROUPED_KEY;
      if (!groups.has(key)) groups.set(key, []);
      groups.get(key)!.push(t);
    }
    // Sort each bucket by sort_order ASC (tiebreak started_at desc).
    for (const arr of groups.values()) {
      arr.sort((a, b) => a.sort_order - b.sort_order || b.started_at.localeCompare(a.started_at));
    }
    // Drop empty ungrouped bucket from display unless it's the only one.
    const entries = Array.from(groups.entries()).filter(([k, v]) => v.length > 0 || k !== UNGROUPED_KEY || groups.size === 1);
    // Custom groups sort alphabetically below ungrouped.
    entries.sort((a, b) => {
      if (a[0] === UNGROUPED_KEY) return -1;
      if (b[0] === UNGROUPED_KEY) return 1;
      return a[0].localeCompare(b[0]);
    });
    return entries;
  }, [active, pendingGroups]);

  const handleCreate = () => {
    const label = prompt("Label for the new terminal session?") ?? undefined;
    onCreate({ label: label || undefined, cols: 120, rows: 32 });
  };

  const handleCreateInProject = (project: ProjectSummary) => {
    const projectName = project.project.split("/").filter(Boolean).pop() ?? project.project;
    onCreate({ label: projectName, cwd: project.project, cols: 120, rows: 32 });
    setShowProjectPicker(false);
  };

  // Where the user was before the mobile sub-window opened, so closing it
  // can hand focus back instead of dropping it on <body> (the tapped list
  // item goes display:none while the sub-window is open).
  const returnFocusRef = useRef<HTMLElement | null>(null);

  const detachLiveSelection = () => {
    const liveId = liveSelectionRef.current;
    if (!liveId) return;
    liveSelectionRef.current = null;
    onDetach(liveId);
  };

  const handleSelect = (t: TerminalSessionMeta, replay: boolean) => {
    // Detach before issuing the next attach/replay command. This includes a
    // same-id live -> replay transition, whose old forwarder would otherwise
    // interleave live chunks into the replay stream.
    detachLiveSelection();
    returnFocusRef.current = document.activeElement instanceof HTMLElement ? document.activeElement : null;
    setSelectedId(t.id);
    setReplayMode(replay);
    if (replay) onReplay(t.id);
    else {
      liveSelectionRef.current = t.id;
      onAttach(t.id);
    }
  };

  // Register replay panes with the shared router in replay mode so a queued
  // live chunk cannot reach xterm even after the synchronous detach above.
  const registerSelectedHandler = useCallback(
    (
      id: string,
      handler: (
        id: string,
        direction: "chunk" | "catchup" | "replay_chunk",
        bytes: Uint8Array,
      ) => void,
    ) => registerHandler(id, handler, { replayMode }),
    [registerHandler, replayMode],
  );

  // Mobile sub-window back action (itr#487): below the breakpoint selecting a
  // terminal opens it full-screen (CSS keys on the `terminal-open` class);
  // "back" closes the sub-window by clearing the selection, returning to the
  // list. Detach so the daemon stops streaming to a pane nobody is viewing.
  // No Escape-to-close: inside a terminal, Escape belongs to the PTY.
  const handleBack = () => {
    detachLiveSelection();
    setSelectedId(null);
    setReplayMode(false);
    const el = returnFocusRef.current;
    returnFocusRef.current = null;
    if (el?.isConnected) el.focus();
  };

  // Honour an inbox deep-link focus (itr#437). Waits for the target session to
  // appear in `terminals` (term_list may not have arrived yet), then selects +
  // live-attaches it and clears the request upstream so it won't re-fire.
  // A target that never arrives within FOCUS_WAIT_MS is stale (itr#449): the
  // session ended, or the deferred row was mislabeled with a non-embedded
  // terminal id. Surface an inline notice instead of a silent no-op, and still
  // ack via onFocusHandled so the pending focus can't wedge the inbox.
  useEffect(() => {
    if (!focusSessionId) {
      focusWaitRef.current = null;
      return;
    }
    const target = terminals.find((t) => t.id === focusSessionId);
    if (target) {
      focusWaitRef.current = null;
      setFocusNotice(null);
      handleSelect(target, false);
      onFocusHandled?.();
      return;
    }
    if (focusWaitRef.current?.id !== focusSessionId) {
      focusWaitRef.current = { id: focusSessionId, deadline: Date.now() + FOCUS_WAIT_MS };
    }
    const giveUp = () => {
      focusWaitRef.current = null;
      setFocusNotice(
        `Couldn't focus terminal ${focusSessionId.slice(0, 8)}… — the session is no longer available. It may have ended, or it isn't an embedded wisphive terminal.`,
      );
      onFocusHandled?.();
    };
    const remaining = focusWaitRef.current.deadline - Date.now();
    if (remaining <= 0) {
      giveUp();
      return;
    }
    const timer = window.setTimeout(giveUp, remaining);
    return () => window.clearTimeout(timer);
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [focusSessionId, terminals]);

  const handleNewGroup = () => {
    const name = prompt("Name for the new group?");
    if (!name) return;
    const trimmed = name.trim();
    if (!trimmed) return;
    if (!pendingGroups.includes(trimmed) && !active.some((t) => t.group_name === trimmed)) {
      setPendingGroups((gs) => [...gs, trimmed]);
    }
  };

  const startRenameGroup = (group: string) => {
    if (group === UNGROUPED_KEY) return;
    setRenamingGroup(group);
    setRenameDraft(group);
  };

  const commitRename = () => {
    if (!renamingGroup) return;
    const next = renameDraft.trim() || null;
    const target = next === null ? undefined : next;
    // Rename = reassign group on every member; empty name → ungroup.
    const members = active.filter((t) => t.group_name === renamingGroup);
    for (const t of members) onSetGroup(t.id, target);
    // Replace in pending list as well.
    setPendingGroups((gs) => gs.flatMap((g) => {
      if (g !== renamingGroup) return [g];
      return target ? [target] : [];
    }));
    setRenamingGroup(null);
    setRenameDraft("");
  };

  const ungroupAll = (group: string) => {
    if (group === UNGROUPED_KEY) return;
    const members = active.filter((t) => t.group_name === group);
    for (const t of members) onSetGroup(t.id, undefined);
    setPendingGroups((gs) => gs.filter((g) => g !== group));
  };

  // Fractional reordering: compute a new sort_order that places `id` between
  // `before` and `after` in the destination bucket. If the bucket is empty or
  // the drop is at either edge, extrapolate by SORT_GAP.
  const computeSortOrder = (
    bucket: TerminalSessionMeta[],
    draggedId: string,
    dropIndex: number,
  ): number => {
    const filtered = bucket.filter((t) => t.id !== draggedId);
    const clamped = Math.max(0, Math.min(dropIndex, filtered.length));
    const before = clamped > 0 ? filtered[clamped - 1].sort_order : undefined;
    const after = clamped < filtered.length ? filtered[clamped].sort_order : undefined;
    if (before === undefined && after === undefined) return 0;
    if (before === undefined) return (after ?? 0) - SORT_GAP;
    if (after === undefined) return before + SORT_GAP;
    return Math.floor((before + after) / 2);
  };

  const handleDragStart = (t: TerminalSessionMeta) => (e: React.DragEvent) => {
    setDrag({ id: t.id, fromGroup: t.group_name ?? null });
    e.dataTransfer.effectAllowed = "move";
    e.dataTransfer.setData("text/plain", t.id);
  };

  const handleDragEnd = () => {
    setDrag(null);
    setDropHint(null);
  };

  const handleDragOverItem = (groupKey: string, index: number) => (e: React.DragEvent) => {
    if (!drag) return;
    e.preventDefault();
    e.dataTransfer.dropEffect = "move";
    setDropHint({ group: groupKey, index });
  };

  const handleDragOverGroupEnd = (groupKey: string, bucketLen: number) => (e: React.DragEvent) => {
    if (!drag) return;
    e.preventDefault();
    e.dataTransfer.dropEffect = "move";
    setDropHint({ group: groupKey, index: bucketLen });
  };

  const handleDrop = (groupKey: string) => (e: React.DragEvent) => {
    if (!drag) return;
    e.preventDefault();
    const destGroup = groupKey === UNGROUPED_KEY ? undefined : groupKey;
    const bucket = activeGroups.find(([k]) => k === groupKey)?.[1] ?? [];
    const index = dropHint?.group === groupKey ? dropHint.index : bucket.length;
    const newOrder = computeSortOrder(bucket, drag.id, index);
    // If crossing groups, update group first, then sort_order.
    const currentGroup = drag.fromGroup ?? undefined;
    if (destGroup !== currentGroup) onSetGroup(drag.id, destGroup);
    onReorder(drag.id, newOrder);
    // Created group is no longer pending once a session lands in it.
    if (destGroup) setPendingGroups((gs) => gs.filter((g) => g !== destGroup));
    setDrag(null);
    setDropHint(null);
  };

  return (
    <div className={`terminals-layout${selected ? " terminal-open" : ""}`}>
      <div className="terminals-sidebar">
        {focusNotice && (
          <div className="terminals-focus-notice" role="status">
            <span className="terminals-focus-notice-message">{focusNotice}</span>
            <button
              type="button"
              className="terminals-focus-notice-dismiss"
              aria-label="Dismiss terminal focus notice"
              onClick={() => setFocusNotice(null)}
            >
              ×
            </button>
          </div>
        )}
        <div className="terminals-sidebar-toolbar">
          <button onClick={handleCreate}>+ New terminal</button>
          <button onClick={() => { onRefreshProjects(); setShowProjectPicker(true); }}>+ In project…</button>
          <button onClick={handleNewGroup}>+ New group</button>
          <button onClick={onRefresh}>Refresh</button>
        </div>
        {showProjectPicker && (
          <div className="terminals-project-picker">
            <div className="terminals-project-picker-header">
              <strong>Open terminal in project</strong>
              <button onClick={() => setShowProjectPicker(false)} aria-label="Close project picker">×</button>
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
                aria-label={`Open terminal in ${p.project}`}
                onClick={() => handleCreateInProject(p)}
                {...activate(() => handleCreateInProject(p))}
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

          {/* ── Active ─────────────────────────────────────── */}
          {(active.length > 0 || pendingGroups.length > 0) && (
            <section className="terminals-section">
              <header className="terminals-section-header">
                <span className="terminals-section-title">Active</span>
                <span className="terminals-section-count">{active.length}</span>
              </header>
              <div className="terminals-section-body">
                {activeGroups.map(([groupKey, bucket]) => {
                  const isUngrouped = groupKey === UNGROUPED_KEY;
                  return (
                    <div
                      key={groupKey}
                      className={`terminals-group${drag && dropHint?.group === groupKey ? " drop-target" : ""}`}
                      onDragOver={handleDragOverGroupEnd(groupKey, bucket.length)}
                      onDrop={handleDrop(groupKey)}
                    >
                      {!isUngrouped && (
                        <div className="terminals-group-header">
                          {renamingGroup === groupKey ? (
                            <input
                              autoFocus
                              className="terminals-group-rename"
                              value={renameDraft}
                              onChange={(e) => setRenameDraft(e.target.value)}
                              onBlur={commitRename}
                              onKeyDown={(e) => {
                                if (e.key === "Enter") commitRename();
                                else if (e.key === "Escape") { setRenamingGroup(null); setRenameDraft(""); }
                              }}
                            />
                          ) : (
                            <>
                              <button
                                type="button"
                                className="terminals-group-name"
                                onClick={() => startRenameGroup(groupKey)}
                                title="Click to rename"
                                aria-label={`Rename group ${groupKey}`}
                              >
                                {groupKey}
                              </button>
                              <span className="terminals-group-count">{bucket.length}</span>
                              <button
                                className="terminals-group-ungroup"
                                onClick={() => ungroupAll(groupKey)}
                                title="Ungroup all"
                                aria-label={`Ungroup all in ${groupKey}`}
                              >×</button>
                            </>
                          )}
                        </div>
                      )}
                      {bucket.length === 0 && !isUngrouped && (
                        <div className="terminals-group-empty">Drop a terminal here</div>
                      )}
                      {bucket.map((t, idx) => (
                        <SidebarItem
                          key={t.id}
                          t={t}
                          selected={t.id === selectedId}
                          pending={pendingByTerminal.get(t.id) ?? 0}
                          draggable
                          dragHinted={drag !== null && dropHint?.group === groupKey && dropHint.index === idx}
                          onDragStart={handleDragStart(t)}
                          onDragEnd={handleDragEnd}
                          onDragOver={handleDragOverItem(groupKey, idx)}
                          onClick={() => handleSelect(t, false)}
                          onAttach={() => handleSelect(t, false)}
                          onClose={() => onClose(t.id)}
                          onReplay={() => handleSelect(t, true)}
                        />
                      ))}
                    </div>
                  );
                })}
              </div>
            </section>
          )}

          {/* ── Orphaned ───────────────────────────────────── */}
          {orphaned.length > 0 && (
            <section className="terminals-section">
              <header
                className="terminals-section-header clickable"
                aria-expanded={orphanedOpen}
                aria-label={`Orphaned terminals — ${orphanedOpen ? "collapse" : "expand"}`}
                onClick={() => setOrphanedOpen((o) => !o)}
                {...activate(() => setOrphanedOpen((o) => !o))}
              >
                <span className="terminals-section-caret" aria-hidden="true">{orphanedOpen ? "▼" : "▶"}</span>
                <span className="terminals-section-title">Orphaned</span>
                <span className="terminals-section-count">{orphaned.length}</span>
              </header>
              {orphanedOpen && (
                <div className="terminals-section-body">
                  {orphaned.map((t) => (
                    <SidebarItem
                      key={t.id}
                      t={t}
                      selected={t.id === selectedId}
                      pending={pendingByTerminal.get(t.id) ?? 0}
                      draggable={false}
                      dragHinted={false}
                      onClick={() => handleSelect(t, true)}
                      onReplay={() => handleSelect(t, true)}
                    />
                  ))}
                </div>
              )}
            </section>
          )}

          {/* ── Archived (collapsed by default) ────────────── */}
          {archived.length > 0 && (
            <section className="terminals-section">
              <header
                className="terminals-section-header clickable"
                aria-expanded={archivedOpen}
                aria-label={`Archived terminals — ${archivedOpen ? "collapse" : "expand"}`}
                onClick={() => setArchivedOpen((o) => !o)}
                {...activate(() => setArchivedOpen((o) => !o))}
              >
                <span className="terminals-section-caret" aria-hidden="true">{archivedOpen ? "▼" : "▶"}</span>
                <span className="terminals-section-title">Archived</span>
                <span className="terminals-section-count">{archived.length}</span>
              </header>
              {archivedOpen && (
                <div className="terminals-section-body">
                  {archived.map((t) => (
                    <SidebarItem
                      key={t.id}
                      t={t}
                      selected={t.id === selectedId}
                      pending={pendingByTerminal.get(t.id) ?? 0}
                      draggable={false}
                      dragHinted={false}
                      onClick={() => handleSelect(t, true)}
                      onReplay={() => handleSelect(t, true)}
                    />
                  ))}
                </div>
              )}
            </section>
          )}
        </div>
      </div>

      <div
        className="terminals-main"
        ref={mobileDialogRef}
        {...(mobileSubwindowOpen
          ? {
              role: "dialog",
              "aria-modal": true,
              "aria-labelledby": "terminals-mobile-dialog-title",
              tabIndex: -1,
            }
          : {})}
      >
        {selected ? (
          <>
            {/* Visible only inside the mobile sub-window (CSS hides it on
                desktop, where the sidebar stays alongside the terminal).
                Status/command/replay markers live in TerminalView's own strip
                right below — repeating them here would just double the chrome. */}
            <div className="terminals-mobile-header">
              <button
                type="button"
                className="terminals-mobile-back"
                onClick={handleBack}
                // Accessible name contains the visible text "Terminals"
                // (WCAG 2.5.3 Label in Name — voice-control users say what
                // they see).
                aria-label="Terminals — back to session list"
              >
                ‹ Terminals
              </button>
              <strong id="terminals-mobile-dialog-title" className="terminals-mobile-title">
                {selected.label || "(no label)"}
              </strong>
            </div>
            <TerminalQueueDock
              terminalPending={terminalPending}
              otherPendingCount={otherPendingCount}
              onApprove={onApprove}
              onDeny={onDeny}
              onJumpToQueue={onJumpToQueue}
            />
            <div className="terminals-terminal-slot">
              <TerminalView
                key={`${selected.id}-${replayMode ? "replay" : "live"}`}
                session={selected}
                replayMode={replayMode}
                onInput={onInput}
                onResize={onResize}
                registerHandler={registerSelectedHandler}
              />
            </div>
          </>
        ) : (
          <div className="terminals-empty-state">
            Select a terminal to view it live, or press "New terminal".
          </div>
        )}
      </div>
    </div>
  );
}

// ── Sidebar item ───────────────────────────────────────────────

interface SidebarItemProps {
  t: TerminalSessionMeta;
  selected: boolean;
  pending: number;
  draggable: boolean;
  dragHinted: boolean;
  onClick: () => void;
  onDragStart?: (e: React.DragEvent) => void;
  onDragEnd?: (e: React.DragEvent) => void;
  onDragOver?: (e: React.DragEvent) => void;
  onAttach?: () => void;
  onClose?: () => void;
  onReplay: () => void;
}

function SidebarItem(p: SidebarItemProps) {
  const { t, selected, pending, draggable, dragHinted, onClick, onDragStart, onDragEnd, onDragOver, onAttach, onClose, onReplay } = p;
  const classes = [
    "terminals-sidebar-item",
    selected ? "selected" : "",
    pending > 0 ? "has-pending" : "",
    dragHinted ? "drop-hint" : "",
  ].filter(Boolean).join(" ");
  return (
    <div
      className={classes}
      onClick={onClick}
      draggable={draggable}
      onDragStart={onDragStart}
      onDragEnd={onDragEnd}
      onDragOver={onDragOver}
    >
      <div className="row">
        {draggable && <span className="drag-handle" title="Drag to reorder" aria-hidden="true">⋮⋮</span>}
        <strong>{t.label ?? "(no label)"}</strong>
        <span className={`term-status term-status-${t.status}`}>{t.status}</span>
        {pending > 0 && (
          <span className="pending-badge" title={`${pending} pending approval${pending === 1 ? "" : "s"}`}>
            {pending}
          </span>
        )}
      </div>
      <div className="cmd">{t.command} {t.args.join(" ")}</div>
      <div className="meta">
        {t.cwd} · {t.cols}×{t.rows} · {new Date(t.started_at).toLocaleString()}
      </div>
      <div className="actions">
        {t.status === "running" && onAttach && onClose && (
          <>
            <button onClick={(e) => { e.stopPropagation(); onAttach(); }}>Attach</button>
            <button onClick={(e) => { e.stopPropagation(); onClose(); }}>Kill</button>
          </>
        )}
        <button onClick={(e) => { e.stopPropagation(); onReplay(); }}>Replay</button>
      </div>
    </div>
  );
}

// ── localStorage helpers ───────────────────────────────────────

function readBool(key: string, fallback: boolean): boolean {
  try {
    const v = localStorage.getItem(key);
    if (v === null) return fallback;
    return v === "1";
  } catch {
    return fallback;
  }
}

function writeBool(key: string, value: boolean) {
  try {
    localStorage.setItem(key, value ? "1" : "0");
  } catch {
    /* ignore quota errors */
  }
}
