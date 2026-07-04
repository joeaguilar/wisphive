# Sprint-2 — Command Center Layer 1: Waiting-on-you inbox

**Sprint Goal:** Deliver the Waiting-on-you inbox so that every human-blocked decision across all concurrent sessions — plus the auto-answered audit feed — surfaces in one answerable web queue, eliminating invisible-prompt debugging.
**Epic:** itr#433
**Created:** 2026-07-03
**Story style:** base default (no STORY_STYLE.md)
**Program context:** Command Center Phase 3 (itr#398) centerpiece = itr#399. Phases 1 (itr#396) + 2 (itr#403) closed; Phase 3 is the next sequential slice per docs/ROADMAP.md. Scope = inbox-only (option A), chosen for highest chance of success.
**Spec:** docs/command-center-spec.md §5.1 (inbox) + §10 (verification standard)

## Non-Goals
- Agent liveness board / working-tree strip / burn meter (#400/#401/#402 — later Phase 3 sprints)
- Any Phase 5 work: Devices UI #220, scrollback, mobile pairing
- Steering controls (start/stop/retarget agents) — spec §5 locks Layer 1 to state-mirror, not steering-wheel
- Reworking the existing History view (audit stays there; feed is live+recent only)
- In-console answering of always-deferred tools (AskUserQuestion/ExitPlanMode) — deferred to epic itr#439 + a new ADR

## Definition of Done (sprint-level)
- cargo test --workspace green; cargo clippy --workspace -- -D warnings clean; cargo fmt --all -- --check no diff
- just frontend-lint clean; just frontend-test (vitest) green
- User-visible increment exercised against a REAL running session (spec §10) — screenshot/driven-flow evidence in the closing itr note
- Docs updated when user-facing behavior changes (CLAUDE.md web surface / new ServerMessage / new route)

## Sprint Backlog
| ID | Title | Pri | Risk | Files | Blocked-by |
|----|-------|-----|------|-------|------------|
| itr#434 | S1 Stream auto-answered + deferred audit events to clients live | high | high | wire.rs, event_ingest.rs, server.rs, protocol.ts | — |
| itr#435 | S2 Inbox view: unified waiting-on-you queue (project/session/agent/age) | high | med | Inbox.tsx(new), App.tsx, queueUtils.ts, useWisphive.ts | — |
| itr#436 | S3 Auto-answer audit feed panel + explicit empty-state | med | med | AutoAnswerFeed.tsx(new), Inbox.tsx, useWisphive.ts | #434, #435 |
| itr#437 | S4 Deferred-item answer affordance (deep-link) | med | med | Inbox.tsx, useWisphive.ts, DetailView.tsx | #434, #435, #436 |
| itr#438 | S5 Runtime evidence + close #399 (dynamic smoke test) | high | high | docs/smoke/CHECKLIST.md | #434, #435, #436, #437 |

**Wave plan note:** frontend stories S3→S4 are chained after S2 because they share `Inbox.tsx` + `useWisphive.ts`; serializing them trades parallelism for zero merge conflicts (deliberate, for a highest-chance-of-success sprint). S1 (backend) can run fully in parallel with S2 (frontend). S5 is the final gating evidence story.

## Spillover → Product Backlog
- itr#439 — EPIC: In-console remote answering of AskUserQuestion/ExitPlanMode (needs a new ADR; reverses always-defer for remote answer) — from alignment Q1
- itr#440 — EPIC: Auto-clear deferred inbox items on native-prompt resolution — from alignment Q2
  - itr#442 — SPIKE: detect deferred-item resolution signal (PostToolUse / follow-up correlation)
- itr#441 — Fast-follow: auto-answer feed as full searchable audit surface — from alignment Q3

## Open Assumptions
- **ADR-0002 boundary (captured):** the inbox surfaces deferred tools (AskUserQuestion/ExitPlanMode/Elicitation) via deep-link/focus-session, NOT in-console. Always-defer semantics unchanged this sprint. In-console answering lives in itr#439 and needs its own ADR. #399 AC allows "in-console OR via deep-link", so this satisfies the AC.
- **Deferred auto-clear limitation (known v1):** wisphive gets no event when a deferred prompt is answered natively, so deferred rows may not clear promptly — they age out / clear on next snapshot. Resolution detection tracked in itr#440.
- **No new aggregation plumbing:** the daemon decision queue is already cross-session (flat `DecisionRequest[]` with project/agent/terminal_session_id/age on every item); the inbox reshapes it rather than re-aggregating.
- **Deep-link scope:** focus works only for wisphive-spawned terminal sessions (terminal_session_id present); hook-only sessions get a "go to your terminal" pointer.
- **S5 correctness oracle:** because live-session queue content is non-deterministic, S5 judges correctness by invariants cross-checked against `wisphive audit --since 10m` (deterministic oracle over live data).

## Outcomes
<!-- Populated by /sprint-review after /blitz runs. -->

## Demo
<!-- Populated by /sprint-review. -->

## Retro
<!-- Populated by /sprint-review. -->
