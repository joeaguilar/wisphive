# ADR-0002: Always-defer classification for questions / plan-mode / elicitations

- **Status:** Accepted (amended 2026-07-18 — see Amendments)
- **Date:** 2026-06-14
- **Deciders:** Josef (PO)
- **itr:** #380, #388, #559
- **Related:** ADR-0001, ADR-0010

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

Statements in the Decision section above were tightened by later bug fixes; the original text
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

3. **`ask` presumes a native prompt; promptless origins fail closed** (itr#559, 2026-07-18).
   The Decision above silently presumed that a native prompt EXISTS to receive the deferred
   question. Two origins have none: Codex's `PreToolUse` path (its native approval surface is
   `PermissionRequest`; itr#366 already mapped Ask → deny there in the response formatter), and
   **daemon-managed headless spawns** (`claude -p` / `codex exec` with stdio nulled and
   `--dangerously-skip-permissions`), where a probe (`docs/research/headless-ask-probe/`, commit
   `8193b04`) showed an `ask` is a **silent block**: the tool never runs, no `PostToolUse` fires,
   the process exits 0 — the spawn burns its run invisibly. The guard's predicate is therefore
   **native-prompt existence** — `native_prompt_exists(surface, provider, event)` in
   `wisphive_hook` — not `agent_type == Codex`; the Codex arm is now a member of that predicate.

   - The prompt **surface** is classified first (`PromptSurface`): managed spawns export
     `WISPHIVE_PROMPT_SURFACE=headless` (set in `build_agent_command`,
     `wisphive_daemon::process_registry`). `WISPHIVE_AGENT_ID` without a marker (a pre-marker
     daemon's spawn — version skew) also classifies as headless, since every managed spawn such a
     daemon produces is unattended headless. Absent both, the session is interactive. An
     **unrecognized marker value is never presumed interactive** and fails closed.
   - **Interactive sessions are unchanged**: intrinsic entries still defer unconditionally to the
     real native prompt (amendments 1–2 stand in full). The itr#559 tests pin this
     (`interactive_defer_contract_unchanged_by_surface_guard`).
   - On a promptless origin an always-defer tool resolves a **deterministic fail-closed deny**
     with an operator-readable reason (ADR-0010 repair-via-message: the message names the origin,
     the rule, and the way out), audited to `events.jsonl` as `denied` with
     `decided_by: always_ask:headless_no_prompt:{intrinsic|operator}` (managed headless),
     `always_ask:unrecognized_surface:{intrinsic|operator}` (unimplemented marker value), or the
     pre-existing `codex_ask_fail_closed:always_ask:*` (interactive Codex `PreToolUse`) — all
     reaching `wisphive audit` via the normal ingestion path. A **daemon-resolved** Ask converts
     the same way (`headless_no_prompt:daemon_ask` / `unrecognized_surface:daemon_ask` /
     `codex_ask_fail_closed:daemon_ask`, see `convert_promptless_daemon_ask`).
   - **The attendance seam** (binding scope note, 2026-07-16): the upcoming Chat surface
     (itr#564 spike) is headless-SHAPED but ATTENDED — the Wisphive Inbox is its native gate
     surface — and must NOT be hard-denied by this rule. The seam is the marker env: an attended
     surface declares a new recognized `WISPHIVE_PROMPT_SURFACE` value and adds a
     `PromptSurface` variant whose routing sends intrinsic asks to that surface instead of
     denying. Only the seam exists today; the Inbox routing is deliberately not built here.
   - **`permission_mode='plan'` stays a permitted managed-spawn config** (recommendation
     recorded per itr#559 AC6; the orchestrator ratifies). A plan-mode spawn calls
     `ExitPlanMode` by construction and will hit this deny — but the failure is now loud,
     attributed, and self-explaining (the deny message names plan-mode as the likely cause and
     the restructuring options), and the planning output remains retrievable from the
     conversation transcript, so plan-mode recon spawns retain value. Refusing `'plan'` in
     `validate_spawn_request` would be a behavior change beyond itr#559's scope and was not made.

   See also `docs/GLOSSARY.md` ("Headless / spawned agent", "Ask / defer", "Always-defer /
   always-ask").

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

- Code: `crates/wisphive_hook/src/main.rs` (`is_always_deferred`, and since itr#559:
  `PromptSurface`, `native_prompt_exists`, `resolve_always_defer`,
  `convert_promptless_daemon_ask`),
  `crates/wisphive_protocol/src/types.rs` (`DEFAULT_ALWAYS_ASK`),
  `crates/wisphive_daemon/src/config.rs` (`always_ask` / `auto_approve_dangerous`),
  `crates/wisphive_daemon/src/process_registry.rs` (`build_agent_command` sets
  `WISPHIVE_PROMPT_SURFACE=headless`),
  `crates/wisphive_cli/src/commands/config.rs` (`mode {balanced|dangerous}`, `defer`/`undefer`)
- Probe: `docs/research/headless-ask-probe/` (itr#559 AC1 — headless `ask` is a silent block)
- itr: #380, #559
- Handoff: `docs/handoff/2026-06-14-always-defer-posture-modes.md`
- Memory: `reference_askuserquestion_hooks.md`
