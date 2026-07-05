import { useEffect, useMemo, useState } from "react";
import type { AuditDecision, DecisionRequest } from "../types/protocol";
import {
  deferredPromptSummary,
  eventPrefix,
  inputSummary,
  orderByAge,
  shortProject,
  timeAgo,
} from "./queueUtils";
import { AutoAnswerFeed } from "./AutoAnswerFeed";
import { DetailView, DeferredDetailView } from "./DetailView";
import { activate } from "./a11y";

interface InboxProps {
  items: DecisionRequest[];
  auditDecisions: AuditDecision[];
  /** Agent ids whose session the daemon has reported gone (itr#464). A deferred
   * prompt from a dead session can never be answered in its terminal, so it is
   * dropped from the waiting list rather than left as a dead pointer. */
  endedAgentIds?: string[];
  selectedId: string | null;
  onSelect: (id: string) => void;
  onApprove: (id: string, opts?: { additional_context?: string; always_allow?: boolean }) => void;
  onDeny: (id: string, message?: string) => void;
  /** Deep-link into a wisphive-spawned terminal session so the human can
   * answer an always-deferred native prompt (AskUserQuestion / ExitPlanMode /
   * Elicitation) where it actually lives. Only wired for deferred rows that
   * carry a `terminal_session_id`; hook-only sessions get a text pointer
   * instead (there is no embedded terminal to focus). */
  onFocusTerminal: (terminalSessionId: string) => void;
}

const HOUR_MS = 60 * 60 * 1000;

export function Inbox({
  items,
  auditDecisions,
  endedAgentIds,
  selectedId,
  onSelect,
  onApprove,
  onDeny,
  onFocusTerminal,
}: InboxProps) {
  const [now, setNow] = useState(() => Date.now());
  // The "decided without you" feed starts collapsed; '(view)' reveals it
  // (spec §5.1). Nothing to view means no toggle is rendered.
  const [showFeed, setShowFeed] = useState(false);
  // Which deferred row is expanded to its read-only detail. Deferred items are
  // AuditDecisions (not DecisionRequests) so they never join `selectedId` /
  // keyboard nav — they can't be answered in-console.
  const [expandedDeferred, setExpandedDeferred] = useState<string | null>(null);

  useEffect(() => {
    const id = window.setInterval(() => setNow(Date.now()), 1000);
    return () => window.clearInterval(id);
  }, []);

  // The 1s `now` keeps the small pending list's ages live. The hour-windowed
  // audit work below, though, scans the whole auditDecisions array (up to
  // `audit_snapshot_limit`, ≤10k rows) and only needs coarse freshness — so
  // bucket time to 15s and key the memos/feed on that. This stops the O(n)
  // filters and the "decided without you" feed from re-running on every 1s
  // tick, while pending ages stay per-second live.
  const coarseNow = Math.floor(now / 15_000) * 15_000;

  const ordered = useMemo(() => orderByAge(items), [items]);
  const recentAutoCount = useMemo(
    () =>
      auditDecisions.filter(
        (audit) => audit.kind === "auto_approved" && coarseNow - new Date(audit.ts).getTime() <= HOUR_MS,
      ).length,
    [auditDecisions, coarseNow],
  );

  // Always-deferred native prompts (ADR-0002) never reach the in-console
  // queue — they arrive only as `deferred` AuditDecision events. Surface the
  // recent ones as their own "waiting in your terminal" section, distinct from
  // the daemon-queued rows that CAN be approved/denied here.
  //
  // Bounded to the last hour as a backstop: a prompt answered in the terminal
  // DOES signal back — PostToolUse fires and the daemon stamps the answer onto
  // the deferred row via attach_tool_result (spike itr#442, GO) — but the
  // resolution broadcast + row-clear is not wired yet (itr#440 → #461/#462/#463).
  // Until it lands, the hour window keeps answered/abandoned rows from piling up.
  // (Abandoned prompts — session killed mid-prompt, no PostToolUse — are handled
  // by the dead-session fade, itr#464.)
  const endedAgents = useMemo(() => new Set(endedAgentIds ?? []), [endedAgentIds]);
  const deferred = useMemo(
    () =>
      auditDecisions
        .filter(
          (a) =>
            a.kind === "deferred" &&
            // A reconnect snapshot marks already-answered deferrals resolved
            // (itr#461); those are no longer "waiting" — keep them out of this
            // list so a refresh/second device doesn't re-surface answered rows.
            !a.resolved &&
            // The originating session is gone (itr#464): the prompt can never be
            // answered in its terminal, so drop the row rather than leave a dead
            // "Answer in your terminal" pointer.
            !endedAgents.has(a.agent_id) &&
            coarseNow - new Date(a.ts).getTime() <= HOUR_MS,
        )
        .sort((a, b) => new Date(b.ts).getTime() - new Date(a.ts).getTime()),
    [auditDecisions, coarseNow, endedAgents],
  );
  const deferredGroups = useMemo(() => groupDeferred(deferred), [deferred]);

  const nothingWaiting = ordered.length === 0 && deferred.length === 0;

  return (
    <section className="inbox" aria-label="Waiting on you inbox">
      <div className="inbox-header">
        <div>
          <h2>Inbox</h2>
          <p className="inbox-count" role="status" aria-live="polite">
            {items.length} waiting
            {deferred.length > 0 && ` · ${deferred.length} in your terminal`}
            {" · "}
            {recentAutoCount} auto-answered in last hour
            {recentAutoCount > 0 && (
              <>
                {" "}
                <button
                  type="button"
                  className="inbox-view-toggle"
                  aria-expanded={showFeed}
                  onClick={() => setShowFeed((v) => !v)}
                >
                  {showFeed ? "(hide)" : "(view)"}
                </button>
              </>
            )}
          </p>
        </div>
      </div>

      {nothingWaiting ? (
        <div className="inbox-empty">No pending decisions</div>
      ) : (
        <>
          {ordered.length > 0 && (
            <div className="inbox-list">
              {ordered.map((item, index) => (
                <InboxRow
                  key={item.id}
                  item={item}
                  now={now}
                  isOldest={index === 0 && ordered.length > 1}
                  selected={item.id === selectedId}
                  onSelect={onSelect}
                  onApprove={onApprove}
                  onDeny={onDeny}
                />
              ))}
            </div>
          )}

          {deferred.length > 0 && (
            <div className="inbox-deferred" aria-label="Waiting in your terminal">
              <h3 className="inbox-subhead">Waiting in your terminal</h3>
              {deferredGroups.map(([key, rows]) => (
                <div className="inbox-group" key={key}>
                  <header className="inbox-group-header" style={{ color: groupColor(key) }}>
                    <span className="inbox-group-name">{key}</span>
                    <span className="inbox-group-count">{rows.length}</span>
                  </header>
                  {rows.map((decision) => {
                    const dkey = deferredKey(decision);
                    return (
                      <DeferredRow
                        key={dkey}
                        decision={decision}
                        now={now}
                        expanded={expandedDeferred === dkey}
                        onToggle={() =>
                          setExpandedDeferred((cur) => (cur === dkey ? null : dkey))
                        }
                        onFocusTerminal={onFocusTerminal}
                      />
                    );
                  })}
                </div>
              ))}
            </div>
          )}
        </>
      )}

      {showFeed && <AutoAnswerFeed decisions={auditDecisions} now={coarseNow} />}
    </section>
  );
}

interface InboxRowProps {
  item: DecisionRequest;
  now: number;
  isOldest: boolean;
  selected: boolean;
  onSelect: (id: string) => void;
  onApprove: (id: string, opts?: { additional_context?: string; always_allow?: boolean }) => void;
  onDeny: (id: string, message?: string) => void;
}

function InboxRow({ item, now, isOldest, selected, onSelect, onApprove, onDeny }: InboxRowProps) {
  const prefix = eventPrefix(item.hook_event_name);
  const summary = inputSummary(item);
  const sessionLabel = sessionLabelFor(item.terminal_session_id, item.agent_id);
  const gkey = groupKey(item.project, sessionLabel);
  const color = groupColor(gkey);

  return (
    <article
      className={`inbox-item ${isOldest ? "oldest" : ""} ${selected ? "selected" : ""}`}
      style={{ borderLeftColor: color }}
      aria-current={selected}
      onClick={() => onSelect(item.id)}
    >
      <div className="inbox-item-topline">
        {prefix && <span className="event-prefix">{prefix}</span>}
        <span className="tool-name">{item.tool_name}</span>
        <span className="inbox-age">{timeAgo(item.timestamp, now)}</span>
      </div>
      <div className="inbox-route">
        <span className="inbox-group-chip" style={{ color }}>
          {gkey}
        </span>
        <span>{item.agent_id.slice(0, 20)}</span>
      </div>
      {/* Collapsed: an ellipsised one-liner + quick approve/deny for fast triage.
          Selected: the full DetailView — EVERY tool_input field (command, file
          content, edit diff, all params), project/agent/event metadata, Copy
          All, and the complete action set (+Context, Deny+Message, Always
          Allow) — so nothing needed to decide is hidden. */}
      {!selected ? (
        <>
          {summary && <div className="inbox-summary">{summary}</div>}
          <div className="inbox-actions" aria-label={`Actions for ${item.tool_name}`}>
            <button className="btn-approve" onClick={(e) => { e.stopPropagation(); onApprove(item.id); }}>
              Approve
            </button>
            <button className="btn-deny" onClick={(e) => { e.stopPropagation(); onDeny(item.id); }}>
              Deny
            </button>
          </div>
        </>
      ) : (
        <div className="inbox-detail-full" onClick={(e) => e.stopPropagation()}>
          <DetailView request={item} onApprove={onApprove} onDeny={onDeny} />
        </div>
      )}
    </article>
  );
}

interface DeferredRowProps {
  decision: AuditDecision;
  now: number;
  expanded: boolean;
  onToggle: () => void;
  onFocusTerminal: (terminalSessionId: string) => void;
}

function DeferredRow({ decision, now, expanded, onToggle, onFocusTerminal }: DeferredRowProps) {
  // Colour rail matches this row's group header (the group name is shown once
  // in the header above, so it isn't repeated as a per-row chip here).
  const sessionLabel = sessionLabelFor(decision.terminal_session_id, decision.agent_id);
  const color = groupColor(groupKey(decision.project, sessionLabel));
  const terminalId = decision.terminal_session_id;
  // Short one-line preview of the literal question/plan; the full untruncated
  // prompt lives in the expanded DeferredDetailView (no single-place truncation).
  const summary = deferredPromptSummary(decision.tool_input);

  return (
    <article
      className="inbox-item inbox-deferred-item"
      style={{ borderLeftColor: color }}
      aria-expanded={expanded}
      aria-label={`${decision.tool_name} — waiting in your terminal`}
      onClick={onToggle}
      {...activate(onToggle)}
    >
      <div className="inbox-item-topline">
        <span className="inbox-deferred-badge">deferred</span>
        <span className="tool-name">{decision.tool_name}</span>
        <span className="inbox-age">{timeAgo(decision.ts, now)}</span>
      </div>
      {decision.decided_by && (
        <div className="inbox-route">
          <span>{decision.decided_by}</span>
        </div>
      )}
      {/* Agent output is UNTRUSTED — rendered as an inert React text node. */}
      {!expanded && summary && <div className="inbox-summary">{summary}</div>}
      <div className="inbox-deferred-note">
        Answer this in the agent&apos;s native prompt — it never reaches the in-console queue.
      </div>
      {/* Read-only detail; NO in-console answer control (ADR-0002). */}
      {expanded && <DeferredDetailView decision={decision} onFocusTerminal={onFocusTerminal} />}
      <div className="inbox-actions" aria-label={`Answer route for ${decision.tool_name}`}>
        {terminalId ? (
          <button
            className="btn-focus"
            onClick={(e) => { e.stopPropagation(); onFocusTerminal(terminalId); }}
          >
            Focus terminal
          </button>
        ) : (
          <span className="inbox-goto-pointer">
            Answer in your <strong>{shortProject(decision.project)}</strong> terminal
          </span>
        )}
      </div>
    </article>
  );
}

// ── helpers ─────────────────────────────────────────────────────────

function sessionLabelFor(terminalSessionId: string | undefined, agentId: string): string {
  return terminalSessionId ? `term ${terminalSessionId.slice(0, 8)}` : `session ${agentId.slice(0, 8)}`;
}

// Stable per-(project·session) group key used for the grouping header and the
// colour chip that visually distinguishes concurrent sessions (#435 AC4).
function groupKey(project: string, sessionLabel: string): string {
  return `${shortProject(project)} · ${sessionLabel}`;
}

// Deterministic hue from the group key so the same session always paints the
// same colour across rows. Fixed saturation/lightness keeps contrast legible.
function groupColor(key: string): string {
  let hash = 0;
  for (let i = 0; i < key.length; i++) {
    hash = (hash * 31 + key.charCodeAt(i)) % 360;
  }
  return `hsl(${hash}, 60%, 60%)`;
}

function deferredKey(d: AuditDecision): string {
  return `${d.ts}|${d.agent_id}|${d.tool_name}|${d.terminal_session_id ?? ""}`;
}

function groupDeferred(items: AuditDecision[]): [string, AuditDecision[]][] {
  const map = new Map<string, AuditDecision[]>();
  for (const d of items) {
    const key = groupKey(d.project, sessionLabelFor(d.terminal_session_id, d.agent_id));
    const bucket = map.get(key);
    if (bucket) bucket.push(d);
    else map.set(key, [d]);
  }
  return [...map.entries()];
}

