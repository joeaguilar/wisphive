# ADR-0010: Fail-closed everywhere is deliberate — repair channels are messages and scripts, not fail-open holes

- **Status:** Accepted
- **Date:** 2026-07-15
- **Deciders:** Product Owner (posture ruling, 2026-07-15), implementing agents (epic #533)
- **itr:** #533, #535, #534, #536, #538, #539, #541
- **Related:** ADR-0001 (extends the tiered fail posture), ADR-0008 (same-UID trust model)

## Context

**The incident (2026-07-15, epic #533).** A campaign worker rebuilt and installed the hook
binary mid-session via `./install.sh`, per what was then repo convention. The freshly installed
hook carried new strict-perms validators for `~/.wisphive` (state dir must be a non-symlink
directory, owner-only 0700; `mode` must be a non-symlink regular file, owner-only 0600, checked
descriptor-side with `O_NOFOLLOW`). The operator's *legacy* state predated those requirements —
so the new validators rejected it, and the fail-closed default did exactly what it says: **every
hook event on the machine denied**, including human-origin events like `UserPromptSubmit`.
Every gated agent (Claude Code and Codex alike) was bricked until the operator hand-`chmod`ed
`0700`/`0600`. The worker was killed mid-deadlock.

The post-incident question was pointed: should human-origin events (`UserPromptSubmit`,
session-lifecycle events) fail *open* when the control plane's own config is unsafe, so a
perms regression can never lock the human out of their own prompt?

**The PO ruling:** no. The total stop — including `UserPromptSubmit` — is security baked in,
not a bug. An unsafe `~/.wisphive` means the gate itself cannot be trusted; the correct behavior
is to stop everything and say so. What failed in the incident was not the posture but the
*clarity of the way out*: the denial told the operator almost nothing about which file was
wrong, what state it needed to be in, or what command would fix it.

## Decision

Fail-closed stays, everywhere, for every hook event type — and the repair channel is the
**denial message and out-of-band tooling**, never a fail-open code path. Every config/perms
denial the hook emits must carry three elements: (1) the failing file/path, (2) the required
state (mode bits / ownership / non-symlink), and (3) the exact repair commands — the literal
`chmod`/`chown` line, `wisphive doctor --fix-perms`, `scripts/wisphive-rescue.sh`, and
`wisphive emergency-off` as the emergency exit.

## Rationale

- **An event-type exemption is a bypass primitive.** Any "human-origin events fail open" carve-out
  becomes the path an attacker (or a confused agent) shapes traffic through. Event names arrive on
  untrusted stdin; classifying them as "safe to fail open" hands control of the fail posture to the
  payload author.
- **Unsafe state means the gate is untrustworthy, not inconvenient.** A group-writable `mode` file
  or a symlinked state dir is exactly the tampering surface ADR-0008 worries about. Waving some
  events through while the gate's own integrity is in question inverts the product's core promise.
- **The incident's real gap was diagnosability.** The operator fixed the brick in two `chmod`s once
  they knew which files and which bits. A denial that carries the path, the required state, and the
  literal repair command turns a machine-wide deadlock into a one-line fix — without weakening
  anything.
- **Out-of-band repair cannot be hook-shaped.** The hook runs *inside* the gated loop; the repair
  tools (`doctor --fix-perms`, the rescue script, `emergency-off`) run outside it, invoked by the
  human. That separation is what keeps "the way out" from also being "the way around."

## Consequences

- Every future validator added to the hook's config/perms path must ship with an actionable
  denial message (path + required state + repair commands) — the itr#535 matrix tests
  (`crates/wisphive_hook/tests/actionable_denials.rs`) enforce the three elements across
  event types × failure classes and will fail any cell that stops denying.
- A perms/config regression still stops the machine until repaired. That cost is accepted and
  mitigated by tooling, not softened in code: #535 (actionable denials, this ADR), #541
  (`scripts/wisphive-rescue.sh` + strict on/off), #536 (install preflight so a new binary's
  validators are checked against existing state *before* it goes live), #534 (deliberate repair
  of legacy state), #538 (brick detector), #539 (red-team of the posture).
- Future sessions must **not** "fix" a total-stop report toward fail-open — the stop is the
  designed behavior. Improve the message, ship a repair tool, or fix the state.
- `wisphive emergency-off` remains the documented last resort: it disables gating entirely,
  visibly, by the operator's hand — a deliberate act, not a silent degradation.

## Alternatives considered

- **Fail open for human-origin events (`UserPromptSubmit`, session lifecycle)** — rejected by the
  PO. It converts an integrity failure into a partial bypass, keyed off attacker-controllable
  event names, and teaches operators that a broken gate is survivable-by-default.
- **Auto-repair from inside the hook (self-`chmod` on denial)** — rejected. The hook mutating the
  state that gates the hook is self-modification of the control plane (cf. itr#425); repair must
  be a human-invoked, out-of-band action.
- **Grandfather legacy perms (accept 0644/0755 with a warning)** — rejected. A warning nobody sees
  is a widened trust boundary forever; the validators exist precisely to refuse loose state.

## Links

- Code: `crates/wisphive_hook/src/main.rs` (`read_mode_file`, `mode_failure`,
  `format_pre_parse_deny`); tests: `crates/wisphive_hook/tests/actionable_denials.rs`,
  `crates/wisphive_hook/tests/mode_failclosed.rs`
- itr: #533 (incident epic), #535 (this ADR + actionable denials), #534, #536, #538, #539, #541
- ADR-0001 — tiered fail posture (daemon-*unreachable* still fails open; that posture is about a
  dead control plane, not an untrustworthy one, and is unchanged by this ADR)
