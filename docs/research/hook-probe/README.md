# Hook-probe: does a deferred native prompt signal back when answered?

**Date:** 2026-07-04 · **Spike:** itr#442 (closed GO) · **Epic:** itr#440

## Question

When wisphive defers an `AskUserQuestion` / `ExitPlanMode` to the agent's native
prompt (ADR-0002 always-defer), does Claude Code fire any hook when the human
**answers** that prompt — a signal the daemon could use to clear the "waiting in
your terminal" inbox row? Prior belief (stale comment at `Inbox.tsx` and the
itr#440 framing) was "no signal comes back."

## Method

A pure **logging** hook (`probe-hook.sh`) registered on all events
(PreToolUse / PermissionRequest / PostToolUse / Elicitation / Notification /
UserPromptSubmit — see the settings snippet in the probe's `.claude/`) in a
real interactive Claude Code session. It emits **no decision** (never alters
behavior) and appends one summary line per event to `probe.log` plus the full
raw stdin to `probe-raw.jsonl`.

## Result — GO. PostToolUse is a usable, correlatable resolution signal.

From `probe.log`:

- **ExitPlanMode** — PreToolUse + PermissionRequest @ 04:18:33 → **PostToolUse @ 04:19:01** (28 s = the human deciding).
- **AskUserQuestion** — PreToolUse + PermissionRequest @ 04:19:35 → **PostToolUse @ 04:19:39**.

From `probe-raw.jsonl`, the PostToolUse payload:

- carries the **same `tool_use_id`** as the deferring PreToolUse (exact
  correlation, no heuristics), and
- carries the answer: `tool_response.answers = { "<question>": "<chosen option>" }`.

## Why this matters (the infra already exists)

The daemon **already stamps the answer onto the deferred row today**:

1. `wisphive hooks install` registers PostToolUse with an empty matcher (all
   tools) — `crates/wisphive_daemon/src/project_audit.rs`.
2. On PostToolUse the hook sends `ClientMessage::ToolResult{ tool_use_id,
   tool_result }` — `crates/wisphive_hook/src/main.rs` (`handle_post_tool_use`).
3. The deferred `decision_log` row carries `tool_use_id` under a **unique index**
   (`state/migrate.rs`); `attach_tool_result` matches by it and `UPDATE`s
   `tool_result` — `crates/wisphive_daemon/src/state/decisions.rs`.

So an `ask` row with a non-NULL `tool_result` **is** an answered deferral. The
update is just never broadcast or surfaced, so the inbox row never clears.

## What's left (itr#440 subtree)

- **#462** protocol: add `tool_use_id` to `AuditDecision` + define a
  `DeferredResolved` `ServerMessage`.
- **#461** daemon: broadcast the resolution when `attach_tool_result` fills an
  `ask` row; make the reconnect snapshot resolved-aware.
- **#463** frontend: clear the "waiting in terminal" row on resolution —
  **Queue-consistent** (splice immediately, à la `Queue.tsx`); the answer shows
  in the audit/History feed, not as a lingering greyed row.
- **#464** dead-session fade: abandoned prompts (session killed mid-prompt, no
  PostToolUse) never resolve — relabel via the agent-registry liveness the
  daemon already computes. Ties off itr#449.

## Files

- `probe.log` — one line per hook event (timeline above).
- `probe-raw.jsonl` — full raw stdin per event (forensics; the `tool_response`
  payloads live here).
- `probe-hook.sh` — the pure-logging hook used (provenance; paths are the
  original temp scratchpad).
