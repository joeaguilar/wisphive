# Plan: Loop Supervisor (daemon-native agent loops with verify-gate feedback)

_Last reviewed: 2026-07-03 (initial spec, itr#421 — interfaces pinned, final design gated on dogfood data; see "Awaiting dogfood data")_

## Problem

Wisphive can spawn headless agents (`wisphive agent start`, `ProcessRegistry`), gate every
tool call, and audit every decision — but a spawned agent is fire-and-forget. The loop that
makes autonomous engineering actually converge lives outside the control plane today:
something must notice the agent stopped, run a verify gate, decide whether the work is
done, and re-invoke the agent with the gate's feedback. Right now that something is a
human, or an orchestrating Claude session (blitz/proof-campaign skills).

The loop supervisor is the daemon component that closes this loop natively:

```
spawn agent → agent stops → run verify gate → gate green? ──yes→ complete
                   ▲                              │no
                   └── re-prompt with gate feedback ┘   (bounded by budget/iterations)
```

Wisphive already owns every hard piece — process registry, Stop/SubagentStop hook events,
the decision queue for human escalation, the audit stream, and `gatr` on the operator's
PATH. The supervisor is mostly wiring plus policy.

## What this plan pins now vs. defers

**Pinned now (this document):** component boundaries, lifecycle state machine, the verify-
gate contract, safety rails, fail posture (ADR-0007), protocol/CLI surface shape.

**Deferred until dogfood data exists:** everything in "Awaiting dogfood data" below. The
requirements input for those sections is friction observed while running real campaigns
(blitz / proof-campaign) *gated by wisphive*, mined from the audit stream and the
deterministic analytics substrate (ADR-0004). Guessing at re-prompt ergonomics before that
data exists would be spec theater.

## Design

### LoopSpec

A loop is configured by a `LoopSpec`, supplied at start (CLI flags or JSON):

```rust
// wisphive_protocol/src/types.rs (future)
pub struct LoopSpec {
    /// What the agent is trying to achieve — becomes the base prompt.
    pub goal: String,
    /// Verify-gate command, run from `project` root after each agent stop.
    /// Exit 0 = green. Run via the operator's shell; `gatr run --tag loop-<id> -- ...`
    /// is the recommended form so evidence lands on disk.
    pub verify: String,
    pub project: PathBuf,
    pub agent_type: AgentType,          // claude_code | codex (existing enum)
    /// Hard rails — see Safety rails.
    pub max_iterations: u32,            // default 5
    pub max_wall_clock_secs: u64,       // default 3600
    pub escalate_on_repeat_failure: bool, // default true
}
```

### Lifecycle state machine

```
        ┌──────────────────────────────────────────────────────┐
        ▼                                                      │
Idle → Spawned → Running → Stopped → Verifying ─green→ Complete│
                    │                   │red                   │
                    │                   ├─ budget left & progress → RePrompting ─┘
                    │                   ├─ repeat failure / no budget → Escalated
                    │                   └─ supervisor error → Aborted
                    └─ agent crash/reap → Verifying (crash is just an early stop)
```

- **Stopped** is detected from the existing hook event stream (`Stop` / `SessionEnd` /
  process reap), not by polling.
- **Escalated** enqueues a `DecisionRequest`-shaped item on the existing review queue —
  the human resolves it in the TUI/web exactly like a tool-call decision. Resolution
  options: retry with operator note (splices into the next prompt), abort loop, or mark
  complete-as-is. Attribution: `decided_by: human` on the loop audit record.
- **Aborted/Complete/Escalated-resolved** are terminal; every transition is stamped into
  the audit stream with the loop id, iteration, gate exit code, and `config_hash`.

### Verify-gate contract

- The gate runs **in the supervisor, not the agent** — the agent cannot skip it, and its
  output is evidence the agent never authored. (Agents may run the same command themselves
  mid-work; only the supervisor's run counts.)
- **Gate integrity is a separate, unsolved problem (see ADR-0007).** Supervisor-side
  execution stops the agent *skipping* the gate; it does not stop the agent *neutering* it
  by editing the tests, the `justfile`, or `.cargo/config` in the same repo it is working
  in. A green gate means "the gate as the agent left it passed," not "the work is correct."
  Closing this needs the gate definition + corpus in agent-unwritable scope, or a review of
  the diff touching gate-adjacent files — tracked as an open question below. Until then the
  human diff-review remains the real correctness gate.
- Captured per run: exit code, duration, last N lines of stderr/stdout (default 100),
  and — when the command is a `gatr run` — the gatr log path.
- Gate output feeds the next prompt on red. The exact re-prompt template is dogfood-gated
  (see below); the contract is only that it includes the gate tail verbatim plus the
  iteration count ("attempt 3 of 5").

### Safety rails (non-negotiable)

1. **Iteration cap** (`max_iterations`) and **wall-clock cap** — whichever fires first
   moves the loop to Escalated, never silently to more attempts.
2. **No-progress detection**: if two consecutive gate runs fail with an identical error
   signature (normalized tail hash), escalate immediately — re-prompting an agent into the
   same wall is the canonical runaway failure.
3. **Human monopoly on widening**: the supervisor never changes gating config, never
   raises auto-approve levels, never resolves its own escalations. It is a client of the
   decision plane, not an owner. (Same posture family as ADR-0005 I1.)
4. **`wisphive emergency-off`** aborts all running loops in the same write that disables
   gating — a control plane in emergency stop must not keep re-invoking agents.
5. **Fail toward stop** (ADR-0007): any supervisor-internal error — gate spawn failure,
   unreadable state, protocol error — moves the loop to Aborted with a notification.
   A broken supervisor must never keep an agent looping unsupervised; contrast
   deliberately with ADR-0001's hook fail-open, whose rationale (don't brick every agent
   when the control plane dies) protects *interactive* sessions. A loop has no human at
   the keyboard to notice; stopped-and-loud beats running-and-blind.

### Surface (shape only; names may shift at implementation)

- CLI: `wisphive loop start --goal ... --verify ... [--project --agent-type --max-iterations --budget-secs]`,
  `wisphive loop list`, `wisphive loop stop <id>`, `wisphive loop show <id>` (iteration
  history with gate results).
- Protocol: `ClientMessage::{StartLoop, StopLoop, QueryLoops}`;
  `ServerMessage::{LoopStatus, LoopEscalation}` broadcast to TUI/web.
- TUI/web: loops appear in the agents panel with iteration count and last gate result;
  escalations land in the normal review queue.

### Relationship to the other plans

- **Deterministic analytics (ADR-0004)** supplies the work journal the supervisor's
  status/re-prompt surfaces read from — build order: analytics substrate first.
- **Policy learning (ADR-0005)** reduces per-iteration human interrupts; the supervisor
  works without it (every gated call just goes to the queue as today).
- **Conflict gate (itr#424 semantics)** is what makes *parallel* loops safe; single loops
  don't need it.
- **Decision plugins (ADR-0006)** observer hooks can notify on loop transitions
  (`on_loop_escalated` is a natural addition to the T1 observer event set).

## Awaiting dogfood data — open questions, deliberately unanswered

Run ≥2 real campaigns (blitz or proof-campaign) gated by wisphive; mine the audit stream,
`gatr` records, and session transcripts (`ccq`) for friction. Then answer:

1. **Re-prompt template** — how much gate output helps vs. drowns? Structured
   (JSON block) or prose? Does including the *diff since last iteration* help?
2. **Context strategy** — fresh session per iteration vs. `--resume` the same session?
   (Cost, drift, and context-poisoning trade-offs cut both ways.)
3. **Stop-vs-stall detection** — is the Stop hook event reliable enough across
   claude_code/codex, or does the supervisor need an idle-timeout heuristic?
   > **Codex gating constraint (proved 2026-07-04, itr#467).** A loop that drives
   > Codex must spawn it such that the Wisphive hook actually runs. Codex
   > *silently skips* hooks it has not been granted persisted trust for (via an
   > interactive `/hooks` step), so a naive `codex exec` spawn runs the agent
   > **completely ungated**. The managed-spawn path now (a) fails closed unless the
   > project has the Wisphive Codex hook installed and (b) passes
   > `--dangerously-bypass-hook-trust` so the daemon-vetted hook runs headlessly.
   > Any Codex-backed loop inherits this: gating is only real when both hold.
   > Blast radius (itr#471, resolved): bypassing trust runs *every* hook in that
   > project's `.codex/hooks.json`, not only Wisphive's. The managed spawn now
   > detects non-Wisphive hook commands and **refuses by default** (opt-in
   > `codex_allow_foreign_hooks` in `config.json`), always `warn!`-ing what would
   > run. Per-hook trust provisioning (writing only Wisphive's `[hooks.state]`
   > trusted-hash and dropping the blanket bypass) would be tighter still, but is
   > codex-version-brittle and a hash mismatch would silently un-gate — rejected
   > for now in favour of the reliable bypass + fail-safe refusal.
4. **Escalation UX** — what does the human actually need in the queue item to make a
   30-second decision? (Candidate: goal, iteration, gate tail, diff stat.)
5. **itr integration** — should a loop claim/close an itr issue as its unit of work
   (proof-campaign-style), or stay tracker-agnostic with the goal string?
6. **Budget defaults** — are 5 iterations / 1h the right rails for real work?
7. **Multi-loop scheduling** — serialize loops per project, or lean on the conflict gate
   once it exists?
8. **Gate integrity** (ADR-0007) — how to stop the agent neutering its own verify gate
   (editing tests/`justfile`/`.cargo`): gate definition + corpus in agent-unwritable scope,
   a hash/allowlist of gate-adjacent files checked before accepting green, or a mandatory
   human diff-review of those files? Until answered, "Complete" is gate-green-as-left, and
   the human diff-review is the real correctness gate.

## Implementation order (post-dogfood)

| Phase | What | Depends on |
|---|---|---|
| 0 | Dogfood: ≥2 wisphive-gated campaigns; write findings into this doc | verification harness (itr#413) |
| 1 | `loop.rs` state machine + LoopSpec, no re-prompt (single-shot + verify + report) | Stop-event plumbing |
| 2 | Escalation via decision queue | Phase 1 |
| 3 | Re-prompt loop with rails 1–2 | Phases 1–2, dogfood answers 1–3 |
| 4 | CLI/TUI/web surfaces | Phase 3 |
| 5 | Parallel loops | conflict gate |

## Non-goals

- Replacing blitz/proof-campaign orchestration (multi-task planning, file-ownership
  waves) — the supervisor runs ONE goal against ONE verify gate. Orchestrators can stack
  on top by starting many loops.
- Judging work quality beyond the gate — the gate is the contract; richer evidence
  (screenshots, runtime checks) belongs in the gate command itself.
- Learning/adjusting its own rails — rails are config, changed by humans.
