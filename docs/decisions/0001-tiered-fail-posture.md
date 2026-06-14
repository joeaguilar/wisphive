# ADR-0001: Tiered fail posture for the hook decision path

- **Status:** Accepted
- **Date:** 2026-06-14
- **Deciders:** Josef (PO)
- **itr:** —
- **Related:** ADR-0002

## Context

`wisphive-hook` runs as a Claude Code / Codex subprocess that gates every tool call. When the
mode file says `active`, the hook must decide what to do when something goes wrong *before* a
human ever sees the request: the daemon socket is refused, stdin fails to parse, the wire
protocol breaks, or the incoming payload is implausibly large. Two failure modes pull in
opposite directions. Fail-closed (deny on error) is the secure default — an unparseable request
should not slip through ungated. But fail-closed has a catastrophic edge: if the daemon itself is
*down*, fail-closing would deny **every** tool call from **every** agent, bricking the whole
fleet whenever the control plane crashes. A single global posture cannot satisfy both.

## Decision

Split the failure posture by failure *kind* rather than picking one global policy:

- **Daemon-unreachable** (refused/absent socket — the control plane is down) **always fails
  open** (approve), regardless of any config. A crashed daemon must never brick agents.
- **Other runtime errors** (read/parse/protocol) honor `~/.wisphive/fail-mode`, which **defaults
  to `closed`** (deny). `fail-mode=open` is the explicit availability-first override.
- **Oversized hook stdin** always denies (a DoS guard, independent of `fail-mode`).
- **`PostToolUse` reporting failures** always approve — that path is telemetry only and must not
  block the agent.

## Rationale

Each failure has a different risk profile, so each gets the posture that matches it. A down
daemon is an availability problem the operator can see and fix; silently bricking agents would be
a worse outcome than a brief gap in gating. A malformed or oversized request, by contrast, is
exactly the case where denying-by-default protects the user, so it fails closed unless they
opt out. PostToolUse carries no gating authority, so a failure there has no security meaning and
must not stall the agent. Encoding this as one global switch would force a wrong answer for at
least one failure kind.

## Consequences

- The hook's error handling must classify failures by kind first, then apply the matching
  posture — `response_for_failure` in `wisphive_hook` is the single chokepoint and must stay so.
- Anyone changing the fail-open/fail-closed default, the daemon-unreachable carve-out, the
  oversized-stdin deny, or the PostToolUse approve is changing a security-critical default and
  must update this ADR + the "Key Design Decisions" section of `CLAUDE.md`/`AGENTS.md`.
- The default is deny-on-error, which can surprise an operator who expected availability-first
  behavior; they must set `fail-mode=open` deliberately.

## Alternatives considered

- **Single global fail-closed** — rejected: bricks every agent when the daemon crashes.
- **Single global fail-open** — rejected: silently lets malformed/ungated requests through, which
  is the opposite of the product's purpose.
- **Treat oversized stdin via `fail-mode`** — rejected: an oversized payload is a DoS signal, not
  a routine runtime error; it should deny even under `fail-mode=open`.

## Links

- Code: `crates/wisphive_hook/src/main.rs` (`response_for_failure`)
- Runtime files: `~/.wisphive/mode`, `~/.wisphive/fail-mode`
- Spec: `CLAUDE.md` → "Key Design Decisions" → Tiered fail posture
