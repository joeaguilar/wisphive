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
import { DeferredDetailView } from "./DetailView";
import { TextModal } from "./Modal";

interface InboxProps {
  items: DecisionRequest[];
  auditDecisions: AuditDecision[];
  selectedId: string | null;
  onSelect: (id: string) => void;
  onApprove: (id: string) => void;
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

  const ordered = useMemo(() => orderByAge(items), [items]);
  const recentAutoCount = auditDecisions.filter(
    (audit) => audit.kind === "auto_approved" && now - new Date(audit.ts).getTime() <= HOUR_MS,
  ).length;

  // Always-deferred native prompts (ADR-0002) never reach the in-console
  // queue — they arrive only as `deferred` AuditDecision events. Surface the
  // recent ones as their own "waiting in your terminal" section, distinct from
  // the daemon-queued rows that CAN be approved/denied here. Bounded to the
  // last hour because wisphive gets no signal when a native prompt is answered
  // (itr#440), so stale rows would otherwise pile up.
  const deferred = useMemo(
    () =>
      auditDecisions
        .filter((a) => a.kind === "deferred" && now - new Date(a.ts).getTime() <= HOUR_MS)
        .sort((a, b) => new Date(b.ts).getTime() - new Date(a.ts).getTime()),
    [auditDecisions, now],
  );
  const deferredGroups = useMemo(() => groupDeferred(deferred), [deferred]);

  const nothingWaiting = ordered.length === 0 && deferred.length === 0;

  return (
    <section className="inbox" aria-label="Waiting on you inbox">
      <div className="inbox-header">
        <div>
          <h2>Inbox</h2>
          <p className="inbox-count">
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

      {showFeed && <AutoAnswerFeed decisions={auditDecisions} now={now} />}
    </section>
  );
}

interface InboxRowProps {
  item: DecisionRequest;
  now: number;
  isOldest: boolean;
  selected: boolean;
  onSelect: (id: string) => void;
  onApprove: (id: string) => void;
  onDeny: (id: string, message?: string) => void;
}

function InboxRow({ item, now, isOldest, selected, onSelect, onApprove, onDeny }: InboxRowProps) {
  const [showDenyMsg, setShowDenyMsg] = useState(false);
  const prefix = eventPrefix(item.hook_event_name);
  const summary = inputSummary(item);
  const full = fullInput(item);
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
      {/* Collapsed: an ellipsised one-liner. Selected: the untruncated input so
          the full command/question is always reachable in-place (no
          single-place truncation). */}
      {!selected && summary && <div className="inbox-summary">{summary}</div>}
      {selected && full && (
        <div className="inbox-detail">
          <div className="inbox-detail-label">Full input</div>
          <pre className="inbox-detail-input">{full}</pre>
        </div>
      )}
      <div className="inbox-actions" aria-label={`Actions for ${item.tool_name}`}>
        <button className="btn-approve" onClick={(e) => { e.stopPropagation(); onApprove(item.id); }}>
          Approve
        </button>
        <button className="btn-deny" onClick={(e) => { e.stopPropagation(); onDeny(item.id); }}>
          Deny
        </button>
        {selected && (
          <button
            className="btn-secondary"
            onClick={(e) => { e.stopPropagation(); setShowDenyMsg(true); }}
          >
            Deny + Message
          </button>
        )}
      </div>
      {showDenyMsg && (
        <TextModal
          title="Deny with Message"
          placeholder="Claude will see this as feedback..."
          submitLabel="Deny"
          submitClass="btn-deny"
          onSubmit={(msg) => { onDeny(item.id, msg); setShowDenyMsg(false); }}
          onClose={() => setShowDenyMsg(false)}
        />
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
      onClick={onToggle}
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

// Full, untruncated tool input for the expanded row. Mirrors inputSummary's
// field preferences but never slices; falls back to pretty JSON so nothing is
// hidden (no single-place truncation).
function fullInput(item: DecisionRequest): string | null {
  const input = item.tool_input;
  if (!input) return null;

  if (typeof input.command === "string") return input.command;
  if (typeof input.file_path === "string") return input.file_path;
  if (typeof input.pattern === "string") return `/${input.pattern as string}/`;

  if (Array.isArray(input.questions)) {
    const lines: string[] = [];
    for (const raw of input.questions as Array<Record<string, unknown>>) {
      if (typeof raw.question === "string") lines.push(raw.question);
      if (Array.isArray(raw.options)) {
        for (const opt of raw.options as Array<Record<string, unknown>>) {
          const label = typeof opt.label === "string" ? opt.label : "";
          const desc = typeof opt.description === "string" ? ` — ${opt.description}` : "";
          lines.push(`  • ${label}${desc}`);
        }
      }
    }
    if (lines.length > 0) return lines.join("\n");
  }

  if (item.event_data && typeof item.event_data.plan_content === "string") {
    return item.event_data.plan_content as string;
  }
  if (item.event_data && typeof item.event_data.last_assistant_message === "string") {
    return item.event_data.last_assistant_message as string;
  }

  return JSON.stringify(input, null, 2);
}
