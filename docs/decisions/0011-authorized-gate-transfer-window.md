# ADR-0011: A live-upgrade enforcement gap is permitted only if it is operator-authorized, surfaced, and audited

- **Status:** Proposed
- **Date:** 2026-07-17
- **Deciders:** Product Owner (posture direction, 2026-07-17), planning agent (epic #588)
- **itr:** #588 (epic), #596, #599, #597, #595
- **Related:** ADR-0001 (tiered fail posture — daemon-unreachable fails open), ADR-0010 (fail-closed everywhere is deliberate), ADR-0008 (same-UID tamper-evidence), ADR-0006/itr#425 (control-plane self-protection)

## Context

**Plan B of the session-survival epic (#588, [`docs/research/terminal-session-survival.md`](../research/terminal-session-survival.md))** wants live terminal/agent processes — a long build, a stateful REPL — to survive a *planned* wisphive daemon upgrade. The mechanism, modeled on herdr's opt-in `--handoff`, is an in-process live handoff: the outgoing daemon spawns its successor, passes each live PTY master fd via `sendmsg`/`SCM_RIGHTS`, and transfers ownership through a multi-phase commit handshake (`validated → restored → ready → committed → owned`). During that handshake **two daemons are briefly alive and enforcement ownership is in transit** — there is a short window in which no single daemon is the authoritative gate.

That window is a real, unavoidable property of a live handoff. Pretending it is zero-gap would be dishonest. Two facts bound its danger:

1. **ADR-0001 already fails open when the daemon is unreachable.** A tool call whose hook can't reach an authoritative daemon during the transfer resolves the same way it would if the daemon had simply crashed: fail-open (approve), because a dead/absent control plane must not brick every agent. The handoff window is, from the hook's point of view, a brief planned instance of that same condition.
2. **The agent's native terminal prompt is a second backstop.** When wisphive is unavailable, most agents still surface their own permission prompt in the terminal — so a human is not blind during the gap.

But this collides head-on with **ADR-0010: fail-closed everywhere is deliberate.** ADR-0010 ruled that an *untrustworthy* gate (bad perms, symlinked state) must stop everything, including human-origin events — no fail-open carve-outs, because an event-type exemption is a bypass primitive. If we now introduce a window where gating is permissive, we owe a precise account of *why this is not the hole ADR-0010 forbids*. The distinguishing question is not "is the gate down?" but **"who chose to lower it, and can that choice be abused?"**

The exploit to defeat is obvious once named: **"trigger an upgrade to slip a call through the transfer window."** If any caller — especially a gated agent — can initiate a handoff, the disclosed gap becomes an on-demand bypass primitive.

## Decision

A live-upgrade handoff **may** open a brief enforcement-transfer window in which gating falls back to the ADR-0001 daemon-unreachable posture (fail-open) plus the agent's native prompt — **but only** when *all* of the following hold. Absent any one, the upgrade is refused and no window opens:

1. **Operator-authorized via step-up auth.** Initiating a handoff requires a fresh authorization — an Argon2id password re-auth or a WebAuthn passkey assertion (`auth.rs`), a sudo-style step-up, **not** merely a valid device bearer token. The upgrade is gated by wisphive's own auth model (itr#599).
2. **Never initiable by a gated agent.** Triggering an upgrade is a self-modification of the gate; it routes to human authorization and is never auto-approved at any `auto_approve_level`, extending the `allow_self_modification=false` posture (ADR-0006, itr#425). An agent-origin attempt to initiate a handoff is refused and audited, never served.
3. **Surfaced.** The TUI and web show an explicit, transient notice while the window is open — "gate transferring; decisions momentarily rely on the agent's native prompt" — so the gap is legible, never silent (itr#596).
4. **Bounded.** The window has a maximum duration; if the commit handshake exceeds it, the handoff **aborts and rolls back** (the outgoing daemon keeps its children and its gate) rather than extending the permissive window.
5. **Audited.** The authorized upgrade is recorded to the decision/audit trail — who authorized, when, from which device — so the window is attributable after the fact.

Only the commit path opens a window: a handoff that fails validation before commit rolls back with the old daemon still authoritative, so **no gap is opened on the failure path**. If live handoff is unavailable, the upgrade falls back to Plan A respawn (a fresh daemon, sessions reconciled, no live-process preservation) — never to a silent permissive gap.

## Rationale

- **ADR-0010's rule is about *involuntary* integrity failure; this is *voluntary, authenticated, disclosed* transfer.** ADR-0010 stops the machine when the gate's own state can't be trusted and no operator intent is present. Here the operator has *deliberately and provably* chosen to upgrade, the successor gate is trusted, and the transfer is bounded and attributable. The distinguishing property is operator intent proven by step-up auth — exactly the thing ADR-0010's involuntary-failure scenario lacks.
- **Authorization is what keeps the disclosed gap from being a bypass.** ADR-0010 warns that a fail-open carve-out keyed off attacker-controllable input is a bypass primitive. The defense is symmetric: the *only* way to open this window is an action that a gated agent cannot take and an unauthenticated caller cannot reach. Disclosure (surfacing) and attribution (audit) are necessary but not sufficient; **authorization of the initiating action is the load-bearing control.**
- **The chicken-and-egg forces the authz out-of-band.** The daemon that would normally gate a tool call is the daemon being replaced — it cannot gate its own replacement through the ordinary decision queue. So authorization must be an out-of-band step-up, mirroring ADR-0010's out-of-band repair channel: the way in (upgrade) is deliberately not reachable through the way the gate normally serves agents.
- **Consistency with ADR-0001, not a new hole.** The permissive behavior *inside* the window is already what ADR-0001 prescribes for an unreachable daemon. This ADR does not invent a new fail-open path; it governs *when an operator is allowed to induce* the existing one, and forces that inducement to be authenticated, bounded, and visible.
- **Honesty over false guarantee.** A "zero-gap" claim would require holding/queueing every in-flight tool call across the transfer — which risks the very brick ADR-0001/0010 exist to avoid. A short, disclosed, authorized, backstopped gap is the honest and safer posture.

## Consequences

- **#597 (wire handoff into the upgrade path) depends on #599 (authorize the action).** The upgrade path cannot ship before the step-up authorization gate exists — encoded as an itr dependency, not left to discipline.
- Every entry point that can initiate a handoff (CLI, web, any future API) must route through the step-up check; a gated-agent origin is refused at every one. Red-team coverage (cf. `just redteam`, epic #533) should include "agent attempts to induce a gate-transfer window."
- The window must be **instrumented and bounded**: a max-duration abort, a surfaced notice, and an audit record are acceptance criteria for Plan B, not polish.
- Future sessions must **not** relax any of the five conditions independently — e.g. "allow an unauthenticated LAN client to upgrade for convenience" reopens the bypass. Changing the posture means superseding this ADR, not editing around it.
- **Plan C may make this ADR moot for terminal sessions.** If the #598 VIP spike moves PTY ownership to a host process that outlives the daemon, a daemon upgrade needs no fd-handoff and opens no window at all. If Plan C lands and subsumes Plan B, revisit this ADR's status. Until then, this posture governs Plan B.
- Accepts a real, bounded cost: for the duration of an authorized upgrade, enforcement leans on ADR-0001 fail-open plus native prompts. That cost is the price of preserving live processes across an upgrade, and it is paid only by an authenticated operator who chose it.

## Alternatives considered

- **Zero-gap handoff — hold/queue all in-flight tool calls across the transfer.** Rejected for now. Blocking calls while the control plane is mid-swap risks the same deadlock ADR-0001/0010 avoid, and adds significant complexity. May be revisited if a strictly bounded queue with a fail-open timeout proves safe — but it is not required to ship Plan B.
- **Route the upgrade through the normal decision queue like any other gated tool call.** Rejected. The daemon that would gate the call is the one being replaced (chicken-and-egg), and the queue is an agent-reachable surface; authorization for lowering the gate must be operator-only and out-of-band.
- **Disclosure without authorization — surface the window but let any caller trigger it.** Rejected. A surfaced-but-unauthorized gap is exactly the "trigger an upgrade to slip a call through" bypass primitive. Surfacing informs; it does not protect.
- **No live handoff at all — only Plan A respawn.** Rejected as the *default* but retained as the *fallback*. Respawn opens no window but does not preserve live processes, which is the entire point of Plan B. The two coexist: respawn when unauthorized/unavailable, authorized handoff when the operator chooses to keep processes alive.
- **A standing "maintenance mode" the operator toggles once and leaves on.** Rejected. A durable permissive mode is an always-open hole; the authorization must be per-upgrade and the window must close automatically on commit or abort.

## Links

- Research / plan: [`docs/research/terminal-session-survival.md`](../research/terminal-session-survival.md) (Plan B, §"Gating during the handoff window" + §"Authorize the action itself")
- itr: #588 (epic), #599 (authorize the upgrade action), #596 (enforcement-continuity + surfaced notice), #597 (wire into upgrade path — depends on #599), #595 (SCM_RIGHTS transport)
- ADR-0001 — tiered fail posture (the daemon-unreachable fail-open this window falls back to)
- ADR-0010 — fail-closed everywhere (the posture this ADR reconciles against: involuntary integrity failure vs. voluntary authorized transfer)
- ADR-0006 / itr#425 — control-plane self-protection (an agent cannot self-modify the gate, extended here to "cannot induce a gate-transfer window")
- Code (future): `crates/wisphive_web/src/auth.rs` (step-up auth), `crates/wisphive_daemon/src/` (handoff transport + commit), `install.sh` / `crates/wisphive_cli/src/commands/daemon.rs` (upgrade entry points)
