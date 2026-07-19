# Chain 588 premise — session survival plan A (respawn/reconcile) · QUEUED

**IMMUTABLE.** Rescope = supersede. Chain reuses existing epic itr#588 (plan-A slice only).
Status: queued — admitted when a lane frees. Re-verify FACTS diff at admission.

## Ordering-claim

Reconcile-on-start (itr#591) requires both the pin flag (itr#589) and a complete persisted
respawn spec (itr#590) — already encoded as itr blocked-by edges (591 ← 589,590). Within
the lane, #590 walks first (daemon-only, no UI surface, unblocks the riskiest schema work
early), then #589 (TUI/web pin affordance), then #591 (terminal: respawn actually happens).
Falsifier: reconcile is shown to work with a partial spec, or pinning is shown to be
prerequisite to persisting (inverted coupling).

## Value-claim

Plan A alone (no FD-handoff) delivers the operator-visible outcome "my pinned sessions
come back after a daemon restart" — independently shippable against the epic's plan-A
scope, without any plan-B machinery. Falsifier: respawned sessions are useless without
live-process continuity (e.g. lost child state makes respawn worthless to the PO) — that
would invalidate plan A's standalone value and route to plan B (outside this mission).

## Links (serial)

1. **L1 = itr#590** — persist full respawn spec (env via itr#89 redaction; `env_json`
   currently NULL) in `terminal_sessions`.
2. **L2 = itr#589** — per-session pin/keep-alive flag, TUI + web affordance, persisted.
3. **L3 = itr#591** — reconcile-on-start: respawn pinned sessions (same cwd/command/env);
   terminal link — user-visible, e2e-verified.

## Assumptions

| id | text | tag |
|---|---|---|
| C-A1 | epic #588's plan-A/B/C phasing (note 223) is current PO intent; plan A is "BUILD NOW" | verified-at-intake (epic body + notes) |
| C-A2 | #591's body (unread at formation) matches the reconcile scope assumed here | spike-scheduled — read at admission, before first claim |
| C-A3 | chain 602's frontend work and #589's TUI/web pin UI won't collide (different components) | assumed-by-default — check at admission |
| C-A4 | herdr UX chain (#574-578) stays unadmitted this mission; its dir-level overlap with #589 is moot | verified-at-intake (contract non-goal) |

## Kill criteria

- Link rework ≥ 2 → council.
- Reviewer `contradicts` → council.
- Admission-time FACTS diff touching C-A1..C-A4 → council before first claim.

## Territory

`crates/wisphive_daemon/src/state/terminals.rs`, `crates/wisphive_daemon/src/terminal.rs`,
`crates/wisphive_tui/src/**` (pin affordance), `crates/wisphive_web/frontend/src/**`
(pin affordance — coordinate with chain 602 landings), daemon startup path in
`crates/wisphive_daemon/src/server.rs`.

## Budget

Mission budget unset; no per-chain allocation.
