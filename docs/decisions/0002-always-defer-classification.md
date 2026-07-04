# ADR-0002: Always-defer classification for questions / plan-mode / elicitations

- **Status:** Accepted (amended 2026-07-03 — see Amendments)
- **Date:** 2026-06-14
- **Deciders:** Josef (PO)
- **itr:** #380, #388
- **Related:** ADR-0001

## Context

Some Claude Code hook events are not real "tool calls" — they are *prompts back to the human*:
`AskUserQuestion`, `EnterPlanMode`, `ExitPlanMode`, `Elicitation`. Their answer comes back **only**
through the agent's native prompt (`PermissionRequest` / `Elicitation`), never through the
PreToolUse hook's allow/deny verdict. An operator running with `auto_approve_level` set to
anything but `off` was silently auto-approving the PreToolUse event for these tools — which
pre-empts the native prompt and resolves the question with **no selection** ("did not answer").
Auto-approving a question was never a real approval; it was a silent dead-end. (See
`~/.claude` memory `reference_askuserquestion_hooks.md`: AskUserQuestion must be answered via
PermissionRequest, not PreToolUse.)

## Decision

Add an **always-defer guard** that runs *before* the tiered auto-approve logic. For any tool in
the effective always-defer set it returns `Decision::Ask` (defer to the agent's native prompt)
**regardless of `auto_approve_level`**. The built-in set `DEFAULT_ALWAYS_ASK` =
`{AskUserQuestion, EnterPlanMode, ExitPlanMode, Elicitation}` lives in `wisphive_protocol` and is
shared by the hook and the CLI. The effective set is `DEFAULT_ALWAYS_ASK ∪ always_ask −
always_ask_remove`. The **only** thing that bypasses the guard is the `auto_approve_dangerous`
posture (the "dangerous" preset); no `auto_approve_level` value may bypass it. `PermissionRequest`
is never deferred by the guard — it *is* the native-answer path and must reach the daemon.

## Rationale

The guard has to win even at `auto_approve_level=all`, so it must sit ahead of the tiered logic
and short-circuit; putting it after would let `all` swallow the question first and reintroduce the
silent-no-answer bug. Sharing the set via `wisphive_protocol` (rather than forking copies into the
hook and CLI) means `config auto-approve status` shows the operator exactly what the hook will do.
Keeping the escape hatch a single coarse bool makes "I turned off the safety net" an explicit,
auditable one-liner rather than a scatter of per-tool opt-outs; fine-grained intent is still
expressible via `always_ask` / `always_ask_remove`.

## Consequences

- The guard's position (before the auto-approve tiers) is load-bearing and must not be reordered.
- `DEFAULT_ALWAYS_ASK` is a hard-coded list: any new question/plan/elicitation-shaped Claude event
  whose answer only returns through a native prompt must be added, or it falls back into the tiers
  and hits the same bug. Cross-check against `HookEventType` whenever Claude's event roster changes.
- The `dangerous` posture is genuinely dangerous and has no extra confirmation gate at set time;
  an operator can footgun into auto-answering questions with no selection (a confirmation prompt
  is the obvious future mitigation).

## Amendments

Two statements in the Decision section above were tightened by later bug fixes; the original text
is preserved for history, but the **current** semantics are:

1. **The guard applies to `PermissionRequest` too** (itr#388, commit `10e78f5`, 2026-06-14).
   The original "`PermissionRequest` is never deferred by the guard" was itself the bug: with the
   daemon down, the fail-open path emitted `{"behavior":"allow"}`, silently resolving the native
   prompt with no selection. `Decision::Ask` on `PermissionRequest` emits **no decision object**,
   which is what lets Claude's native dialog render the question/plan and capture the answer. The
   guard therefore fires on **both** `PreToolUse` and `PermissionRequest`.
2. **Intrinsic entries defer unconditionally** (commit `0530ef1`, 2026-07-01). The original "the
   only thing that bypasses the guard is the `auto_approve_dangerous` posture" let `dangerous`
   (and an `always_ask_remove` entry) re-swallow a question's answer — the same "did not answer"
   dead-end this ADR exists to prevent. The `DEFAULT_ALWAYS_ASK` check now runs ahead of every
   posture and override: **nothing** can un-defer the intrinsic set. `auto_approve_dangerous` and
   `always_ask_remove` release only operator-added `always_ask` tools.

Consequence confirmed 2026-07-03 (itr#249/#250/#253 closed as obsoleted): because the intrinsic
tools always defer before the daemon connection, they can never appear in the daemon decision
queue or the TUI/web detail views via the shipped hook — UI work targeting those views for these
tools is dead code (commit `4462bfa`, reverted in `e7ccb5e`). Deferrals are still audited to
`events.jsonl` (`decided_by: always_ask:intrinsic`, itr#397), which is the correct feed for any
inbox surface that wants to *show* pending questions (deep-link, not in-console answer — itr#399).

## Alternatives considered

- **Run the guard after the auto-approve tiers** — rejected: `all` would swallow the question
  first, exactly the bug being fixed.
- **Per-tool `auto_approve_dangerous` override** — rejected: a posture, not a fine-grained knob;
  per-tool intent already lives in `always_ask` / `always_ask_remove`.
- **Duplicate the defer set in the hook and CLI** — rejected: they could drift and `status` would
  lie to the operator.

## Links

- Code: `crates/wisphive_hook/src/main.rs` (`is_always_deferred`),
  `crates/wisphive_protocol/src/types.rs` (`DEFAULT_ALWAYS_ASK`),
  `crates/wisphive_daemon/src/config.rs` (`always_ask` / `auto_approve_dangerous`),
  `crates/wisphive_cli/src/commands/config.rs` (`mode {balanced|dangerous}`, `defer`/`undefer`)
- itr: #380
- Handoff: `docs/handoff/2026-06-14-always-defer-posture-modes.md`
- Memory: `reference_askuserquestion_hooks.md`
