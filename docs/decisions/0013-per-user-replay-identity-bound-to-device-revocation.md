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

**See Amendment 1 below** — authorization additionally requires *recent* authentication when
crossing into a session the device is not already attached to. Revocation alone is reactive; the
step-up requirement is the proactive half of this decision and is not optional.

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

## Amendment 1 (2026-07-19): step-up re-authentication on cross-session access

**Decided by:** Product Owner, same day, before implementation began.

### What changes

A remote (web) device must have authenticated **within a recent window (default 2–3 hours,
configurable)** to view or request a session it is **not already attached to**. Inside that window,
and for sessions the device is already in, access is frictionless. Outside it, crossing into
unfamiliar history requires re-entering the password or satisfying a passkey.

Session **authorship is demoted from an authorization boundary to a step-up trigger.** `created_by`
and current attachment answer "does this need a challenge?", never "is this permitted?". The
permission answer remains per-user + live revocation, per the main decision above.

Scope: **remote surfaces only.** The TUI/CLI runs as the operator's UID and is already trusted per
ADR-0008; it holds no auth session to step up, so applying this there would be theater.

A failed or abandoned step-up is **audited and raises an operator notification.** A challenge that
fails is not merely a denial — a party holding a valid device token who cannot authenticate is a
high-signal compromise indicator, and this gate is the cheapest place in the system to catch it.

Step-up failure, unavailability, or an unreadable auth timestamp **fails closed** (deny), consistent
with the project's posture in ADR-0010.

### Why

The main decision's accepted cost was that *"a compromised, not-yet-revoked device now reaches the
operator's history."* Revocation is **reactive** — it protects only after the operator notices a
compromise and acts, leaving the entire detection window uncovered. Step-up is the **proactive**
complement: an attacker holding an exfiltrated token but lacking the password or passkey cannot
cross into unfamiliar sessions *at all*, whether or not the operator has noticed yet.

The two controls compose along different axes. Revocation answers "this device is no longer mine";
step-up answers "prove you are still the human, right now." Neither subsumes the other, and the
residual risk each leaves is largely covered by the other.

This also gives session authorship an honest job. As a security wall it was accidental and produced
a defect its own author misread as a broken scroll wheel (itr#624). As a friction heuristic —
"you're already in this session, no challenge needed" — it is exactly the right signal.

### Honest limits

- **This does not defend against a live XSS in the operator's own browser.** Such an attacker shares
  the page's origin and can wait for, or ride, a legitimate re-authentication. Step-up's real
  strength is against an **exfiltrated token replayed out-of-band** — copied from `localStorage` and
  used later or from another machine. Do not let this control justify relaxing XSS discipline; per
  `CLAUDE.md`, agent-rendered output stays untrusted and unsanitized HTML stays forbidden.
- **Re-auth friction is real on mobile**, where this boundary is crossed most. The window must be
  configurable, and passkey/biometric step-up should be the default path so the challenge costs a
  fingerprint rather than a typed password.
- **A clock the attacker controls is not a security boundary.** The recency timestamp must be
  server-side state, never a client-supplied or client-adjustable value.

### Consequences of the amendment

Adds a `last_authenticated_at` per device to the web auth state, a recency predicate on the replay/
attach authorization path, and a step-up challenge flow in the SPA. The recency check joins the live
revocation check as **per-request** state — same non-caching constraint. Failed challenges become a
new audited event class and a notification source.

## Links

- Code: `crates/wisphive_daemon/src/server.rs` (`evaluate_replay_access`, `ReplayAccess::allowed`,
  the `TermAttach` and `TermReplay` arms), `crates/wisphive_web/src/auth.rs` (device registry /
  revocation)
- itr: #623 (implementation), #624 (the defect that exposed it), #284 (attach scrollback epic)
- ADR: ADR-0012 (attach seed gated by this ACL), ADR-0008 (same-uid tamper evidence, not
  tamper-proofing)
