# Mission digest — no-silent-failures

## Launch digest — 2026-07-18

`WALK · landed 0/10 · burn n/a (no budget) · lanes 0/2 (admitting)`

**Chains formed (backlog mode, 186 ready mined):**
- **602** (active, lane A): web error surfacing → spawn regression. Links: #567 → #565. 2 links.
- **603** (active, lane B): managed-spawn gate integrity. Links: #560 → #562 → #559 → #568 → #561. 5 links.
- **588** (queued): session-survival plan A. Links: #590 → #589 → #591. Admitted when a lane frees.

**Premise corrections at intake (red-team, file-level evidence):**
- #560 re-aimed: fail-open is hook-side (`main.rs:1513`→approve), daemon rejection already typed-ish + test-pinned.
- #562 re-scoped: described wildcard arm doesn't exist; only explicitly-set-unknown env changes behavior; 4 pinned tests stay green.
- #559 reframed: in-repo probe proves silent BLOCK not approve; availability defect, fail-closed holds.
- #567 shrunk: banner infra exists; route errors into it + correlation_id.
- #565 gained a candidate root cause: `send()` no-ops when ws not OPEN.

**assumed-by-default register (the contract for future cancellations):**
- Q1 criticals-first over ROADMAP program order (roadmap 6 days stale, predates incident findings)
- Q2 #568/#561 ride chain 603 as territory-order links
- Q3 e2e at chain-602 terminal link + closure only
- Q4 ADR amendments land in-link
- X-A1 server.rs/wire.rs cross-chain collisions manageable by serial landing
- X-A2 no concurrent foreign session mutates chain territory
- A-A4 #565 root cause among 3 traced candidates · A-A6 e2e harness adequate for error-surfacing proof
- B-A7 ADR-in-link · B-A8 socket-down fail-open (ADR-0001) untouched by hook reclassification

**Budget:** unset — burn criteria off; rework/contradicts/5-link councils armed.
**Spike order:** none filed; C-A2 (#591 body unread) resolves by read-at-admission.
**Excluded:** #601 (solo-agent-only), plan-B/C survival, herdr UX, ADR-0009 spikes, program phases (successor candidates).

NEXT: claim 602/L1 (#567) lane A · claim 603/L1 (#560) lane B.
