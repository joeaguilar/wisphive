# Architecture Decision Records

Durable, shared records of **why** Wisphive is built the way it is. Each ADR captures one
decision: its context, the decision itself, the rationale, the consequences, and the alternatives
weighed. ADRs are git-tracked — unlike `~/.claude` memory, which is machine-local — so a fresh
clone, a collaborator, or a reviewing agent on any machine can reconstruct the reasoning. Several
of Wisphive's security-critical decisions previously lived only as prose in `CLAUDE.md`; these
ADRs give that reasoning a durable home.

**Filing a new ADR:** copy [`0000-template.md`](0000-template.md) to `NNNN-short-title.md` (next
number, 4-digit zero-padded, monotonic), fill it in, and add a row to the index below. See
[`../DOCUMENTATION.md`](../DOCUMENTATION.md) for when an ADR is warranted and the cross-linking
rules that keep decisions findable.

**File an ADR when a decision** (a) constrains future work, (b) was non-obvious / had real
alternatives, or (c) someone will later ask "why is it done this way." When in doubt, copy
`0000-template.md` and write the half-page; a recorded decision is cheaper than a re-litigated one.

## Index

| ADR | Title | Status | Date | itr |
|-----|-------|--------|------|-----|
| [0001](0001-tiered-fail-posture.md) | Tiered fail posture for the hook decision path | Accepted | 2026-06-14 | — |
| [0002](0002-always-defer-classification.md) | Always-defer classification for questions / plan-mode / elicitations | Accepted (amended 2026-07-03) | 2026-06-14 | #380, #388 |
| [0003](0003-enterprise-profile-non-functional-until-tls.md) | Enterprise auth profile non-functional until user-cert TLS lands | Accepted | 2026-06-14 | #310, #270 |
| [0004](0004-deterministic-agent-analytics-first.md) | Deterministic agent analytics before generated summaries | Accepted | 2026-06-15 | #390, #391, #392, #393, #394, #395 |
| [0005](0005-policy-learning-security-invariants.md) | Policy-learning security invariants (I1–I10) | Proposed | 2026-07-03 | #422 |
| [0006](0006-plugin-trust-model.md) | Decision-plugin trust model — observer-only, redacted, env-var shell hooks | Proposed | 2026-07-03 | #423, #425 |
| [0007](0007-loop-supervisor-fails-toward-stop.md) | Loop supervisor fails toward stop | Proposed | 2026-07-03 | #421 |
| [0008](0008-same-uid-tamper-evidence-not-tamper-proofing.md) | Same-UID config tampering — tamper-evidence, not tamper-proofing | Accepted | 2026-07-11 | #96, #508 |
| [0009](0009-isolated-codex-home-over-config-emulation.md) | Isolated daemon-controlled CODEX_HOME instead of emulating Codex config resolution | Proposed | 2026-07-14 | #528, #511, #471 |

## Status lifecycle

`Proposed` → `Accepted` → `Superseded by ADR-XXXX` / `Deprecated`. **Never delete a superseded
ADR**; flip its status and link the successor. The reasoning history — including the path not
taken — is the point.

## Candidate decisions not yet written up

Real decisions still living only in prose, handoffs, or memory. File an ADR for each when the area
is next touched:

- **Blocking hooks via oneshot channels** (1-hour timeout, defaults to approve) — `CLAUDE.md`
  "Key Design Decisions".
- **Audit data is never auto-deleted; resource alerts instead of pruning** (itr#340) — `CLAUDE.md`
  Runtime Files (`logs/decision_log.jsonl`), `crates/wisphive_daemon/src/disk_alert.rs`.
- **`hooks install` adds Claude permissions to eliminate the double-prompt** — `CLAUDE.md`
  "Key Design Decisions" → Permissions management.
