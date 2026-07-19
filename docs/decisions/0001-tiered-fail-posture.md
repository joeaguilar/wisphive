# ADR-0001: Tiered fail posture for the hook decision path

- **Status:** Accepted (amended 2026-07-18, itr#560; amended 2026-07-18, itr#562)
- **Date:** 2026-06-14
- **Deciders:** Josef (PO)
- **itr:** #560, #562 (amendments)
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
  Unreachable covers control-plane **absence** only: connect refused/absent socket, or an
  EOF/garbled reply where no well-formed message from a live daemon ever arrived. It **never**
  covers a live daemon's deliberate rejection (see the live-rejection tier below).
- **Live protocol-level rejection** (amendment 2026-07-18, itr#560): a well-formed non-Welcome
  reply at the Welcome position — the typed `ServerMessage::Overloaded` connection-capacity
  load-shed, or a legacy daemon's bare `Error` (capacity, protocol-version skew, handshake
  refusal) — proves the daemon is **alive and answering**. It resolves per
  `~/.wisphive/fail-mode` (**default `closed`**, deny) and is **audited**: the hook writes an
  events.jsonl record whose `decided_by` names the rejection (e.g. `daemon_overloaded:capacity`),
  so a shed decision is distinguishable from a human one and never silent. Before this
  amendment the hook collapsed any pre-Welcome non-Welcome reply into daemon-unreachable,
  turning a saturated daemon's load-shed into a silent unaudited auto-approve.
- **Unrecognized provider identity** (amendment 2026-07-18, itr#562): a set, **non-empty**
  `WISPHIVE_AGENT_TYPE` that is not byte-exactly a provider the hook can serve (one with both an
  `AgentType` variant *and* a response-formatter arm: `claude_code`/`claude`/`codex`) makes the
  hook **refuse rather than guess**: response formatting branches on provider, and a guessed
  (Claude-shaped) reply can be silently ignored by a provider that cannot parse it — an
  unparseable deny is an effective allow. The refusal resolves per `fail-mode` (**default
  `closed`**), is **audited** to events.jsonl as `decided_by: agent_type:unrecognized`, and is
  emitted only through provider-agnostic channels: a deny is the bare exit 2 + stderr message,
  an approve (`fail-mode=open`) is exit 0 with empty stdout. Absence or an empty-but-set
  variable is the plain interactive case and keeps the payload-shape heuristic (Codex
  `model`/`turn_id`) with the ClaudeCode default. Adding a provider means adding the variant
  AND the formatter arm, then recognizing its value in `detect_agent_type_from_env`.
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

The live-rejection tier (itr#560) exists because absence and load-shedding are different risk
profiles wearing the same socket: hooks hold their connection permit for the whole human wait
(up to 3600 s), so a wave of concurrent gated calls can saturate the shared connection cap —
and the "unreachable fails open" carve-out would then wave through exactly the flood that caused
the saturation, silently and without an audit row. A daemon that *answers* — even with a
rejection — is by definition not absent, so the fail-open rationale (no path to a human
decision anywhere) does not apply: the control plane is up, only this session was refused.

## Consequences

- The hook's error handling must classify failures by kind first, then apply the matching
  posture — `response_for_failure` in `wisphive_hook` is the single chokepoint and must stay so.
- The handshake reader must treat any *well-formed* non-Welcome reply at the Welcome position as
  a live rejection (`HookFailureKind::DaemonRejected`), including a legacy bare `Error` from an
  old daemon coexisting with a new hook mid-upgrade. Only refused/absent sockets and
  EOF/unparseable replies may classify as unreachable. A consequence for mid-upgrade version
  skew: a live daemon's `Error{unsupported protocol version}` now denies by default (loudly,
  with the daemon's message) instead of silently approving — the repair channel is the denial
  message, per the ADR-0010 posture. One honest carve-out: the daemon throttles bad-version
  Hellos (10 per 60 s per uid) and, once the budget is exhausted, closes the connection
  *without* sending the `Error` — the hook then sees EOF at the Welcome position, which is
  absence semantics and fails open. Sustained version skew therefore alternates: the first
  throttle-window rejections deny loudly, the rest fail open until the window resets — skew
  does **not** always deny.
- The live-rejection audit record goes to events.jsonl with the resolution outcome (`denied`, or
  `auto_approved` under `fail-mode=open`) and a `decided_by` naming the rejection rule; the
  daemon that shed the connection is alive and ingests it into `decision_log` and the TUI/web
  audit feed, and the accept loop `warn!`s each shed.
- Anyone changing the fail-open/fail-closed default, the daemon-unreachable carve-out, the
  oversized-stdin deny, the PostToolUse approve, or the unrecognized-provider identity refusal
  (itr#562: refuse-and-audit instead of guessing a provider's JSON dialect, including its
  bare-channel formatting for pre-parse failures) is changing a security-critical default and
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
