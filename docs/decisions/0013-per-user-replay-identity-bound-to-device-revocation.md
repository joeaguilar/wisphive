# ADR-0013: Terminal replay authorization is per-user, bound to live device revocation

- **Status:** Accepted
- **Date:** 2026-07-19
- **Deciders:** Product Owner (Josef Aguilar)
- **itr:** #623, #624, #284
- **Related:** ADR-0012, ADR-0008

## Context

Terminal replay (and, since ADR-0012, the scrollback seed attached to a `TermAttach` catchup) is
authorized by `evaluate_replay_access` in `crates/wisphive_daemon/src/server.rs`:

```rust
let authored = author.as_deref() == Some(requester);
let explicit_access = replay_acl.iter().any(|e| e == requester);
// allowed = session_known && (authored || explicit_access)
```

The `requester` string is a **per-device** identity — `human:web:<device_id>` for a browser,
`human:tui` for the TUI/CLI. `replay_acl` defaults empty and has **no grant path** anywhere in the
CLI, TUI, or web UI. The practical result is that the operator's own surfaces are strangers to one
another: a terminal created from the CLI is authored `human:tui` and can never be replayed from the
browser; a terminal created in one browser cannot be replayed from another.

This was latent while nothing received scrollback on attach. ADR-0012 made it **visible and
asymmetric** — the creating device now gets history and every other surface silently falls back to a
screen-only catchup. A 2026-07-19 smoke test produced this matrix (session created on one surface,
`seq -w 1 300` run, then scrolled from each):

| created on | viewed on web | viewed on phone |
|---|---|---|
| web | full scrollback | live-forwarded output only |
| phone | **denied**, screen-only + `terminal replay denied` | full scrollback |

The Product Owner — who wrote this codebase — first experienced the denial as *"the scroll wheel is
broken on web."* A permission decision that reads as a broken feature is a design defect, not a UX
polish item.

The forces that shape the option space:

- **There is exactly one human.** Web devices authenticate against a single operator password; a
  device token is an issuance detail, not a distinct principal. Per ADR-0008 we provide
  tamper-evidence, not tamper-proofing, against code already running as the operator's UID.
- **Session authorship is an accidental boundary.** Nothing designed `created_by` as a security
  control; it is provenance metadata that `allowed()` happens to consult.
- **A stolen device token is already serious.** Tokens live in browser `localStorage`, so per
  `CLAUDE.md` an XSS is device compromise regardless of what replay does.
- **But revocation must mean something.** `wisphive web devices revoke <id>` exists, and history
  access is exactly the kind of capability an operator revokes a device *in order to* cut off.

## Decision

Terminal replay authorization becomes **per-user, not per-device**: the TUI/CLI and every web device
belonging to the operator resolve to a single principal, so the operator can reach their own terminal
history from any of their own surfaces.

Authorization is **bound to the live device-revocation check at request time**, not to session
authorship. A request is authorized when the requesting principal is the operator *and* the
presenting device is currently valid in the device registry. Revoking a device immediately removes
its access to all history, including sessions it created itself.

`replay_acl` is retained for genuinely foreign principals. Denials remain audited, and must surface
as actionable UI naming the remedy — never a raw `terminal replay denied` string.

## Rationale

Per-device identity was buying almost no security while imposing a real cost. The only actor it
excluded was the operator's own second browser; it did not exclude an attacker holding a valid token,
because such an attacker already *is* the operator as far as every other surface is concerned. We
were paying a daily usability tax for a boundary that stopped nobody.

The genuine risk — that a widened principal lets a stolen token reach more history than before — is
real, and authorship was accidentally mitigating it. So we replace the accidental firebreak with a
deliberate one that is *strictly better*: live revocation. Authorship is a static fact fixed at
creation time and cannot be withdrawn; device validity is dynamic state the operator controls. Under
the old model, revoking a compromised device did **not** stop it replaying sessions it had created.
Under the new model, revocation is immediate and total. The operator gains a control that actually
responds to the threat, in exchange for one that merely limited its blast radius by accident.

Binding to revocation also puts the security story where an operator can reason about it: "my
devices can see my history; revoke a device and it sees nothing," rather than "each device sees
whatever it happened to create."

## Consequences

**Easier.** The operator reaches their own scrollback from any of their own surfaces — the ADR-0012
seed stops being creator-only, and the asymmetry that reads as a bug disappears. CLI/TUI-created
sessions become replayable in the browser, which is the common case for this project's own workflow.

**Harder, and honestly costly:**

- **A compromised, not-yet-revoked device reaches more.** Previously it saw sessions it created; now
  it sees the operator's terminal history. This is a deliberate widening. It is bounded by the
  ADR-0012 seed cap and by how fast the operator revokes.
- **Revocation becomes load-bearing.** It moves from a convenience to a security control on the hot
  path. The check must be evaluated **live per request** — never cached for the life of a
  connection, or a revoked device retains access until it reconnects. Any caching added later must
  treat this as a correctness constraint.
- **A single principal is now a single point of failure.** There is no per-device compartmentaliza-
  tion left to fall back on; revocation is the only lever.
- **Multi-operator is now an explicit future decision.** Should Wisphive ever support more than one
  human, "the operator" must become a real user identity rather than an implicit singleton. This ADR
  must be revisited then, not quietly extended.

**Constraints imposed on future work.** Replay authorization must not be re-keyed on authorship;
`created_by` returns to being provenance metadata only. Any new history-bearing surface (mobile
pairing, remote access) inherits this model and must consult the same live revocation check.

## Alternatives considered

- **Per-device grant affordance (populate `replay_acl` from the UI)** — rejected as the primary
  mechanism. It makes the operator perform ceremony to grant themselves access to their own history,
  on every new device, forever. It encodes the accidental boundary as an intentional one. Retained
  only for genuinely foreign principals, where explicit grant is the right semantic.
- **Status quo (authorship as the boundary)** — rejected. It locks the operator out of their own
  data, produced a defect that its own author misread as a broken scroll wheel, and its security
  benefit is illusory against a valid-token attacker. It also cannot be revoked.
- **Per-user with authorship retained as an additional firebreak** — rejected as the worst of both:
  it keeps the confusing asymmetry for CLI-created sessions while still not surviving a stolen token,
  and it leaves two overlapping authorization concepts to reason about.
- **Unbounded per-user with no revocation binding** — rejected. This is the widening without the
  compensating control, and would leave the operator no way to cut off a compromised device's access
  to history.

## Links

- Code: `crates/wisphive_daemon/src/server.rs` (`evaluate_replay_access`, `ReplayAccess::allowed`,
  the `TermAttach` and `TermReplay` arms), `crates/wisphive_web/src/auth.rs` (device registry /
  revocation)
- itr: #623 (implementation), #624 (the defect that exposed it), #284 (attach scrollback epic)
- ADR: ADR-0012 (attach seed gated by this ACL), ADR-0008 (same-uid tamper evidence, not
  tamper-proofing)
