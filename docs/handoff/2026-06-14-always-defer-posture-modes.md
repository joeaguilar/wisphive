# Always-defer questions/plan-mode + CLI posture modes — handoff & next steps

**Date:** 2026-06-14
**Branch:** `main` @ `d9cd3c0`
**Epic / itr:** itr#380
**Closed this session:** itr#380
**Decisions:** [ADR-0002](../decisions/0002-always-defer-classification.md) — always-defer classification (filed for this change); sits inside the tiered fail posture of [ADR-0001](../decisions/0001-tiered-fail-posture.md).
**Predecessor handoff:** none — this is the first durable handoff under `docs/handoff/`. (The tradition previously lived only in conversation + machine-local `~/.claude` memory; see `project_handoff_tradition.md`.)

If you only have 60 seconds: an operator running with `auto_approve_level` set to anything but `off` was silently auto-approving `AskUserQuestion` / `EnterPlanMode` / `ExitPlanMode` / `Elicitation` PreToolUse events, which pre-empts the agent's native prompt and resolves the question with **no selection** ("did not answer"). This change adds an **always-defer guard** that runs *before* the auto-approve tiers and returns `ask` for those tools regardless of level. The one escape hatch is the new `auto_approve_dangerous` posture. If you touch the hook's decision pipeline, scroll to **§ Hard rules** before you reorder anything.

## What just shipped

A question/plan/elicitation tool now defers to the agent's native prompt no matter how aggressive the auto-approve level is — unless the operator has explicitly opted into the "dangerous" posture. The answer to these tools can only come back through the native prompt (`PermissionRequest` / `Elicitation`), so auto-approving their PreToolUse was never a real approval; it was a silent dead-end that resolved the prompt with no answer. The always-defer set is now the single source of truth (these tools were removed from `DEFAULT_AUTO_APPROVE` and the Read tier), shared between the hook and the CLI via `wisphive_protocol`.

Operators get a posture-level control surface for this: `wisphive config auto-approve mode <balanced|dangerous>` (both default to `auto_approve_level=all`; `balanced` keeps always-defer on, `dangerous` turns it off), plus `defer`/`undefer <tool>` to add a harmful tool to / opt a built-in question tool out of the defer set, and a `status` that prints the effective posture and defer set.

```
d9cd3c0  feat(hook): always-defer questions/plan-mode + CLI auto-approve posture modes (itr#380)
```

| Surface | Anchor |
|---|---|
| `is_always_deferred()` guard (runs before auto-approve layers) | `crates/wisphive_hook/src/main.rs` |
| `DEFAULT_ALWAYS_ASK` constant (shared) | `crates/wisphive_protocol/src/types.rs` |
| New config keys `always_ask` / `always_ask_remove` / `auto_approve_dangerous` | `crates/wisphive_daemon/src/config.rs` |
| `config auto-approve mode {balanced\|dangerous}`, `defer`/`undefer`, `status`/`list`/`reset` | `crates/wisphive_cli/src/commands/config.rs`, `crates/wisphive_cli/src/main.rs` |
| Convention doc update | `CLAUDE.md` (wisphive_hook crate entry + config.json runtime-files entry) |

`DEFAULT_ALWAYS_ASK` = `AskUserQuestion`, `EnterPlanMode`, `ExitPlanMode`, `Elicitation`. Effective defer set = `DEFAULT_ALWAYS_ASK` ∪ config `always_ask` − config `always_ask_remove`.

## Trade-offs made

- **Guard runs before the auto-approve tiers, not after.** The defer classification has to win even at `auto_approve_level=all`, so it sits ahead of the tiered logic and short-circuits with `Decision::Ask`. Putting it after the tiers would have let `all` swallow the question first. This is the load-bearing ordering decision — see Hard rules.
- **`PermissionRequest` stays exempt from the guard.** It *is* the native-answer path and must reach the daemon for human review; deferring it would loop. The guard only applies to the PreToolUse-shaped question/plan/elicitation events.
- **`auto_approve_dangerous` is a single bool, not per-tool.** It's a posture, not a fine-grained override — when true, the entire always-defer set is ignored and even questions auto-approve per the level. Per-tool intent is expressed through `always_ask` / `always_ask_remove` instead. Keeping the escape hatch coarse makes "I have turned off the safety net" an explicit, auditable one-liner rather than a scatter of per-tool opt-outs.
- **The defer set lives in `wisphive_protocol`, not duplicated in the hook and CLI.** Both binaries compute the effective set the same way, so `config auto-approve status` can show the operator exactly what the hook will do.

## What's NOT shipped — explicit scope gaps

No follow-up itr issues were filed against this change; it closed itr#380 cleanly. Two things a future session should keep in mind:

1. **`DEFAULT_ALWAYS_ASK` is a hard-coded list.** If Claude Code introduces a new question/plan/elicitation-shaped event whose answer only returns through a native prompt, it must be added to `DEFAULT_ALWAYS_ASK` or it will fall back into the auto-approve tiers and hit the same silent-dead-end bug. Cross-check against `HookEventType` whenever Claude's hook event roster changes.
- **The `dangerous` posture is genuinely dangerous.** It is documented but has no extra confirmation gate at set time; an operator can footgun themselves into auto-answering questions with no selection. If that bites someone, a confirmation prompt on `mode dangerous` is the obvious mitigation.

## Hard rules established this session

1. **The always-defer guard must run BEFORE the auto-approve tiers.** Reordering it after the tiers reintroduces the silent-no-answer bug at `level != off`. (This is the whole point of itr#380.)
2. **`PermissionRequest` is never deferred by the guard** — it is the answer path and must reach the daemon.
3. **`DEFAULT_ALWAYS_ASK` is shared via `wisphive_protocol`** — do not fork a second copy into the hook or CLI; they must agree, or `status` lies to the operator.
4. **`auto_approve_dangerous` is the only thing that bypasses the guard.** No level value (`read`/`write`/`execute`/`all`) may bypass it.

## Where to start next

This was a self-contained bugfix-plus-control-surface; there is no forced next step. Two natural directions if you are picking up the auto-approve surface:

1. **Confirmation gate on `mode dangerous`** (small). The posture is a footgun by design; a one-line "are you sure" at set time closes the obvious operator-error path. Anchor: `crates/wisphive_cli/src/commands/config.rs`.
2. **`HookEventType` audit pass** (small, do whenever Claude's hook roster changes). Confirm every new question/plan/elicitation-shaped event is in `DEFAULT_ALWAYS_ASK`. Anchor: `crates/wisphive_protocol/src/types.rs` + the "Known Claude Code hook events" list in `CLAUDE.md`.

## Memory / docs to read for context

- `CLAUDE.md` → **wisphive_hook** crate entry — the canonical three-layer decision logic, including the always-defer guard and `auto_approve_dangerous` bypass.
- `CLAUDE.md` → **Runtime Files** → `config.json` entry — full key list (`always_ask`, `always_ask_remove`, `auto_approve_dangerous`, the posture presets).
- `CLAUDE.md` → **Key Design Decisions** — the tiered fail posture and blocking-hook model the guard sits inside.
- `~/.claude` memory: `reference_askuserquestion_hooks.md` (AskUserQuestion must be answered via PermissionRequest, not PreToolUse — the root cause this change addresses) and `feedback_hook_response_schema.md`.
