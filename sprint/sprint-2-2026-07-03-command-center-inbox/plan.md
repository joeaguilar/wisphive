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

**Goal achievement:** yes
**Reviewed:** 2026-07-04
**Stories:** 5/5 closed, 0 quarantined, 0 open

| ID | Title | Status | Closed | Notes |
|----|-------|--------|--------|-------|
| itr#434 | S1 Stream auto-answered + deferred audit events to clients | closed | 2026-07-04 | All 6 AC; reimport resilience regression caught by /code-review + fixed; redaction preserved. |
| itr#435 | S2 Inbox view: unified waiting-on-you queue | closed | 2026-07-04 | Shipped partial first (truncation + keyboard bug); /code-review caught it; keyboard fixed, truncation/grouping re-homed to #437. |
| itr#436 | S3 Auto-answer feed panel + empty-state | closed | 2026-07-04 | Feed shows decided_by behind `(view)`; live; no over-build. |
| itr#437 | S4 Deferred affordance + (absorbed) untruncated detail + grouping | closed | 2026-07-04 | Deep-link chain real; absorbed #435 truncation gap genuinely fixed (`<pre>` full input + DeferredDetailView); colour grouping. Required mid-blitz wire change (Wave 2.5). |
| itr#438 | S5 Runtime evidence + close #399 (dynamic smoke) | closed | 2026-07-04 | Real daemon+web+hook-binary e2e (inbox-command-center.spec.ts); all 5 AC + `wisphive audit` oracle; 6 screenshots. Centerpiece #399 + epic #433 closed. |

**Centerpiece:** itr#399 (Waiting-on-you inbox) delivered and closed with §10 runtime evidence.

**Untracked changes (in git diff but not tied to a story):**
- `Terminals.tsx` (+19) — deep-link "Focus terminal" target wiring for #437 (accepted scope expansion, logged as a Wave 2 intervention).
- `core-flows.spec.ts` / `smoke.spec.ts` (minor) — e2e selector updates because Inbox became the default view (itr#446, filed + fixed inline).

## Demo

| ID | Title | PO Decision | Notes |
|----|-------|-------------|-------|
| itr#434 | Audit stream (S1) | accepted | — |
| itr#435 | Inbox view (S2) | accepted | truncation/grouping delivered via #437 |
| itr#436 | Auto-answer feed (S3) | accepted | — |
| itr#437 | Deferred affordance + detail + grouping (S4) | accepted | NIT → itr#449 |
| itr#438 | Runtime evidence (S5) | accepted | checklist wording → itr#450 |

**Bugs surfaced during demo/verification:**
- itr#449 — deferred deep-link silently no-ops for a stale/non-embedded terminal session (#437 NIT).
- itr#450 — smoke CHECKLIST overstates AC1 (real-wire fixture, not hook binary) — transparency fix (#438).
- itr#446 — e2e regression from #435's default-view change (filed + fixed inline during the blitz).

## Retro

**Triggered by:** blitz interventions (out-of-owned-file wiring, mid-blitz wire gap → Wave 2.5, sudo-gate discovery) + a bug filed during the sprint (itr#446).

### Plan vs. actual
- 5/5 closed (100%), goal achieved.
- #435 shipped partial first (green vitest ≠ spec-complete); caught by /code-review, fixed, gaps re-homed to #437.
- Mid-blitz wire-protocol change (Wave 2.5): #434 AC excluded `tool_input` from the wire; #437 AC required showing the deferred question — a plannable contradiction that surfaced at execution time.
- Accepted scope expansion: #437 touched `App.tsx` + `Terminals.tsx` (undeclared) to make the deep-link real.

### Friction log
| Event | Source | Root cause |
|-------|--------|------------|
| #437 wire gap → Wave 2.5 protocol change | blitz Wave 2 | #434 AC (no `tool_input` on wire) contradicts #437 AC (show question text); not caught at plan time. |
| itr#446 e2e regression | blitz Wave 3 | #435 made Inbox the default view; e2e specs asserted old `.queue-layout`; e2e not in the per-story verify gate. |
| #435 partial-ship | /code-review | Green vitest ≠ spec-complete; truncation + invisible-selection keyboard bug had no test coverage. |
| Sudo-gate hang in smoke | blitz Wave 3 | AC1 picked a sudo-class tool (Bash); reauth modal intercepts. Switched to Grep. |

### Process improvements (filed as retro action items)
- itr#451 — Add the e2e suite to the per-story verify gate for view/routing/nav changes (root cause of #446).
- itr#452 — Plan-time AC-contradiction check across dependent data→UI stories (root cause of Wave 2.5).
- itr#453 — Require a "full X reachable" test when a story AC says "user sees X" (root cause of #435; promoted to global ~/.claude/CLAUDE.md).

### Agent-specific learnings
- The blitz self-closed the sprint epic + #399 without running /sprint-review — correct work, wrong authority: epic close + PO acceptance is the review's job.
- Stop-and-report worked well — the #437 agent hit the wire gap and escalated instead of silently patching the backend out of scope. Keep this pattern.
- Out-of-owned-file additive wiring (App/Terminals) was handled well: logged, diff-reviewed, tagged to the story, kept.
