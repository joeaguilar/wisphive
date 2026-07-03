# ADR-0007: Loop supervisor fails toward stop

- **Status:** Proposed
- **Date:** 2026-07-03
- **Deciders:** Josef Aguilar (PO), Claude (loop-supervisor spec, itr#421)
- **itr:** #421
- **Related:** ADR-0001 (tiered fail posture), ADR-0005 (I1 — human monopoly on widening)

## Context

The loop supervisor (`docs/plan-loop-supervisor.md`) re-invokes a stopped agent with
verify-gate feedback until the gate is green or a rail fires. A supervisor-internal
failure (gate spawn error, unreadable state, protocol error) needs a posture: keep the
loop going on best effort, or stop it. ADR-0001 resolved the superficially similar hook
question toward **fail-open** — a crashed control plane must not brick every interactive
agent, because a human is present to notice and recover.

## Decision

Any supervisor-internal error moves the loop to **Aborted** with an OS notification and an
audit record — never to another iteration. Loops also abort (all of them, atomically) on
`wisphive emergency-off`. Rails (iteration cap, wall-clock cap, no-progress detection)
escalate to the human queue rather than silently extending.

## Rationale

ADR-0001's fail-open rationale does not transfer: an interactive session has a human at
the keyboard who sees the stall; an autonomous loop's entire premise is that nobody is
watching. The failure asymmetry is stark — a stopped loop costs one restart, while a
blind loop re-invoking an agent without a working gate can burn budget indefinitely and,
worse, close work as "verified" on evidence that was never actually collected.
Stopped-and-loud beats running-and-blind wherever no human is in the loop by design.

## Consequences

- Easier: loop states are trustworthy — a Complete loop always means the supervisor's own
  gate run exited zero; incident forensics reduce to reading the audit stream.
- Harder / costs: transient supervisor errors (e.g. a gate binary briefly missing) kill
  loops that could have survived a retry; operators of long unattended runs must watch
  for Abort notifications. A bounded internal retry for the gate *spawn* (not the gate
  *result*) may be added later without violating this ADR, provided exhaustion still
  aborts.
- Sets precedent: components that act autonomously (supervisor, future schedulers) default
  fail-closed/stop; components that gate interactive humans default per ADR-0001's tiers.

## Alternatives considered

- **Fail-open like ADR-0001** — rejected: the no-human-present premise inverts the trade.
- **Fail to Escalated (human queue) instead of Aborted** — rejected as the *general* rule:
  escalation assumes the supervisor is healthy enough to enqueue, render, and resolve;
  when its own internals are failing, the honest terminal state is Aborted + notification.
  Rails (budget, no-progress) do escalate — those fire while the supervisor is healthy.
- **Configurable posture (`loop_fail_mode`)** — rejected for v1: no user need yet, and a
  quiet misconfiguration here is exactly the running-and-blind failure this ADR exists to
  prevent. Revisit only with dogfood evidence.

## Links

- Plan: `docs/plan-loop-supervisor.md` (Safety rails, rail 5)
- itr: #421
