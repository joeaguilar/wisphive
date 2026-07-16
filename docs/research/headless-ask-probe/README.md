# Headless-ask probe: what does `claude -p` do with `permissionDecision: "ask"`?

**Date:** 2026-07-15 · **Issue:** itr#559 · **Claude Code:** 2.1.211 · **Binary:** `target/release/wisphive-hook`

Companion to [`../hook-probe/`](../hook-probe/README.md) (itr#442), which probed the
**interactive** case. This one probes the **headless** case that ADR-0002 never considered.

## Question

ADR-0002 always-defer returns `Decision::Ask` for the intrinsic `DEFAULT_ALWAYS_ASK` set
(`AskUserQuestion` / `EnterPlanMode` / `ExitPlanMode` / `Elicitation`) to defer to the agent's
**native prompt**. Daemon-managed spawns run `claude -p` headless with stdio nulled
(`process_registry.rs` ~2150) and `--dangerously-skip-permissions` (`:2162`) — where **no native
prompt exists**. What actually happens?

The hypothesis under test was that bypass would turn the `ask` into an **effective silent
approve**, un-deferring the intrinsic set that CLAUDE.md says no posture or override can un-defer.

## Result — hypothesis REFUTED. `ask` is a silent *block*, not a silent approve.

| run | tool ran? (sentinel on disk) | PostToolUse fired? | exit | elapsed |
|---|---|---|---|---|
| A) `ask` + `--dangerously-skip-permissions` — today's managed config | **no** | no | 0 | 6s |
| B) `ask`, no bypass | **no** | no | 0 | 8s |
| C) control: no `ask`, + bypass — proves the rig can run Bash | **yes** | yes | 0 | 6s |

- **No silent approve.** The fail-closed posture holds. The tool does not execute.
- **No hang.** Exit 0 in seconds; the timeout (100s) was never approached.
- **Bypass is irrelevant to `ask`.** A and B are identical. An explicit hook `ask` forces the
  prompt path that `--dangerously-skip-permissions` would otherwise skip, so bypass does not
  suppress it. (This also means removing bypass does **not** fix this on its own.)
- Claude narrates the hole rather than failing: *"a `PreToolUse` hook intercepted it and asked for
  confirmation, but this session is non-interactive so there's no way to approve the prompt."*

## Emission verified against the real binary (no stub involved)

Isolated HOME, `mode=active` 0700/0600, the exact env `process_registry.rs:2244-2245` sets on a
managed child (`WISPHIVE_AGENT_TYPE=claude_code`, `WISPHIVE_AGENT_ID=agent-…`):

```
ExitPlanMode    → {"hookSpecificOutput":{"hookEventName":"PreToolUse","permissionDecision":"ask"}}
AskUserQuestion → {"hookSpecificOutput":{"hookEventName":"PreToolUse","permissionDecision":"ask"}}
```

Bare `ask`, **no `permissionDecisionReason`**. Contrast, same event, `WISPHIVE_AGENT_TYPE=codex`:

```
{"hookSpecificOutput":{"hookEventName":"PreToolUse","permissionDecision":"deny",
 "permissionDecisionReason":"Wisphive cannot defer to a native prompt on Codex; re-run after
  explicit approval in the Wisphive TUI/web UI."}}
```

Two members of one class ("no native prompt to defer to"), treated two different ways — because
the guard at `main.rs:731-743` branches on `agent_type == Codex` rather than on whether a native
prompt exists.

## So what is the actual defect?

Not a security hole — a **silent capability hole with zero observability**:

1. The tool never runs; the agent burns a turn explaining it cannot proceed.
2. The job **exits 0 = success** having accomplished nothing.
3. Always-defer resolves inside the hook and **never reaches the daemon**, so there is no
   `decision_log` row and no inbox entry. The operator gets **no signal at all**.

`permission_mode: "plan"` is explicitly permitted by `validate_spawn_request`
(`process_registry.rs:100-104`) and exists to make the agent call `ExitPlanMode` — an intrinsic
always-defer member. So a supported, validated spawn config routes into this by construction.

## Method notes (for whoever re-runs this)

- **Two-step, so the expensive step is minimal.** Step 1 drives the real `wisphive-hook` with
  synthetic stdin in an isolated HOME to confirm the emitted bytes — free, no quota. Step 2
  reproduces those exact bytes from a stub inside a live `claude -p` to observe Claude's reaction.
  Only step 2 spends quota.
- **Never run `./install.sh`.** The probe uses `target/release/wisphive-hook` against an isolated
  temp HOME (`$P/.wisphive`, `mode=active`, 0700/0600), per the CLAUDE.md rule.
- **Claude keeps the real `$HOME`** (it needs its own credentials); only the *hook subprocess* gets
  `HOME=$P`. That split is what lets the probe run without touching `~/.wisphive`.
- **`--setting-sources project`** + a temp project's `.claude/settings.json` keeps the probe hook
  isolated from the operator's real hooks.
- **`ExitPlanMode` is not reliably inducible under `-p`.** A first attempt prompting for a plan in
  `--permission-mode plan` produced `num_turns:1` with **no tool call at all** — Claude answered in
  prose. Forcing the `ask` on `Bash` is the controllable substitute; Claude's handling of `ask`
  does not depend on which tool it is for.
- **Do not detect execution by grepping the result text.** A first pass searched the transcript for
  the echoed string and got a false positive — Claude quotes the command back in its prose while
  saying it did *not* run. Use a filesystem sentinel plus `PostToolUse` presence; narration cannot
  fake either.

## Files

- `probe-hook.sh` — the stub used in step 2 (emits the exact bytes verified in step 1; `MODE=control` suppresses the `ask` for the control run).
