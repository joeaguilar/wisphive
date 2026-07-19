# Sprint-5 — Clear sprint-4 review debt + low-complexity backlog batch

**Sprint Goal:** Clear the sprint-4 review debt (crossfire follow-ups #510–520) and a batch of low-complexity backlog tasks, so the tracker reflects reality and easy wins stop accumulating. No new features; behavior-preserving on public surfaces.
**Epic:** itr#524
**Created:** 2026-07-12
**Story style:** STORY_STYLE.md (Wisphive conventions)
**Provenance:** 19 pre-existing groomed issues re-parented into this epic (Tier A = sprint-4 crossfire follow-ups; Tier B = `complexity:C1` low-complexity backlog). Originals unchanged; grooming tags preserved.

## Non-Goals
- No new features or UI surfaces (analytics/journal/pairing/Layer-1 epics stay out).
- No C2+ complexity or refactor epics — low-complexity (C1) tasks only in the easy-wins tier.
- No taste=2 UI/UX polish (opus lane).
- No adapters-crate decision (#125/#76) — contested; stays out until PO rules.
- Behavior-preserving on public surfaces (protocol/schema/CLI) unless a story's AC explicitly changes it.
- Process/housekeeping meta-tasks (#521/#522/#523) stay continuous, not in-sprint.

## Definition of Done (sprint-level)
- Story AC passes with its named test/command.
- **Every bug fix adds a regression test** proving the specific failure mode is gone.
- `cargo test --workspace` green; `cargo clippy --workspace -- -D warnings` clean; `cargo fmt` applied. Frontend stories also green on `frontend-lint` + `frontend-test`; e2e stories run `just e2e`.
- Behavior-preserving on public surfaces unless a story's AC explicitly changes it.

## Routing note (PO)
**#510 and #511** — high-risk hook↔daemon timeout invariant + Codex managed-spawn hook-inventory security — are routed to **fable-5** with a **codex cross-review** before close (tagged `route:fable-5`). All other stories are low-complexity and route per their grooming tags.

## Sprint Backlog (19 stories, 2 tiers)

### Tier A — sprint-4 review debt (11)

| ID | Title | Pri | Risk | Files | Blocked-by |
|----|-------|-----|------|-------|------------|
| itr#510 | Align Claude hook timeout with daemon approval timeout | high | high | daemon: hook_install.rs, config.rs, process_registry.rs | — |
| itr#511 | Audit Codex managed spawn against effective hook inventory | high | high | daemon: process_registry.rs, hook_install.rs | — |
| itr#512 | Daemon startup fatal on broken ~/.wisphive/logs dirent | med | med | daemon: event_ingest.rs | — |
| itr#513 | Config.tsx loader + auth hooks leak unmounted-setState | low | low | web: Config.tsx | — |
| itr#514 | Frontend env typo-detection + test gaps | low | low | web: vite-env.d.ts, env.test.ts | — |
| itr#515 | Thread daemon home_dir into ProcessRegistry | low | low | daemon: process_registry.rs | — |
| itr#516 | Test coverage: pidfile lifecycle + agent spawn/stop interleave | low | low | (planner fills) | — |
| itr#517 | Mobile terminal dialog a11y hardening | low | low | web: Terminals.tsx | — |
| itr#518 | Misc crossfire-review P3 nits (5 nits a–e, AC drafted) | low | low | (planner fills) | — |
| itr#519 | Explicit --host 127.0.0.1 still doesn't enable web | low | low | cli: main.rs | — |
| itr#520 | e2e inbox auto-answered-count assertion flakes under load | low | low | web e2e: inbox-command-center.spec.ts | — |

### Tier B — low-complexity backlog (`complexity:C1`, 8)

| ID | Title | Pri | Risk | Files | Blocked-by |
|----|-------|-----|------|-------|------------|
| itr#134 | Add write_msg helper to wire crate (36 write_all sites) | low | low | protocol: wire.rs · daemon: server.rs | — |
| itr#338 | Retention: single Utc::now() cutoff through archive_and_prune | low | low | daemon: state.rs | — |
| itr#135 | Consolidate make_request test fixtures (3 copies) | low | low | protocol: types.rs · daemon: queue.rs, state.rs | — |
| itr#292 | logging: RUST_LOG-vs-stderr-clamp integration test | low | low | daemon: logging.rs | — |
| itr#139 | Frontend base64 → native Uint8Array.fromBase64/toBase64 | low | low | web: useWisphive.ts | — |
| itr#263 | web devices revoke: distinguish unknown-id vs already-revoked | low | low | (planner fills, web) | — |
| itr#56 | Add `[`/`]` prev/next item keybindings in detail views | med | low | tui: input.rs, ui.rs | — |
| itr#450 | Correct smoke CHECKLIST wording (inbox AC1) | low | low | docs/smoke/CHECKLIST.md | — |

### Shared-file chains (/blitz serializes within a lane)
- `daemon/process_registry.rs` → #510, #511, #515
- `daemon/hook_install.rs` → #510, #511
- `daemon/state.rs` → #338, #135
- `daemon/server.rs` → #134
- `daemon/logging.rs` → #292

## Spillover → Product Backlog
- None new — this campaign re-parents existing issues. The remaining `complexity:C1` pool (~14) and the sprint-4 out-of-epic items stay in the product backlog for a future cycle.

## Open Assumptions
- **Roadmap divergence:** sprint-5 is a deliberate backlog-clearing interlude, NOT the roadmap's next Program-order phase (#403 decision-plane integrity → #398 Layer 1). Logged so `/sprint-review` revisits.
- **Adapters decision** (#125 delete adapters crate / #76 document status) deliberately excluded — contested (contradicts #4/#5 implement-adapters work); stays out until PO rules.
- **Process/housekeeping** #521/#522/#523 kept continuous (apply at each /sprint/blitz), not in-sprint.
- **#510/#511 routing:** fable-5 with codex cross-review per PO direction.

## Outcomes

**Goal achievement:** yes
**Reviewed:** 2026-07-18
**Stories:** 19/19 closed, 0 quarantined, 0 open (100% completion)

| ID | Title | Status | Notes |
|----|-------|--------|-------|
| itr#510 | Align Claude hook timeout with daemon timeout | closed | 3 rounds (fable→sol/ultra); bug moved registry→install layer between passes |
| itr#511 | Audit Codex managed spawn vs hook inventory | closed | 4 rounds (fable×2, sol×2, opus); spawned ADR-track #528; **first detonation of the research-gate class** |
| itr#512–520 | sprint-4 review debt (Tier A remainder) | closed | #513/#516/#517 each needed 1 escalation redo (all opus-CLOSE) |
| itr#56,134,135,139,263,292,338,450 | Tier B C1 backlog | closed | #56 needed 1 escalation (hint clipped off-screen); #134 pre-wave (already done by #123) |

**Untracked changes (in git diff, not tied to a sprint-5 story):**
- #134 closed pre-wave without executor spend — correctly logged, not scope creep.
- Follow-ups #525–532 filed *during* the run (flake harness, dispatch_command test, ADR-track #528, etc.) — all now closed; NOT epic members by design.

## Demo

Reviewed 2026-07-18. Per PO: 17 low-risk mechanical stories batch-accepted; #510/#511 deep-dived. **All 19 accepted — 0 rejected, 0 conditional.**

| ID | PO Decision | Notes |
|----|-------------|-------|
| #512–520, #56, #134, #135, #139, #263, #292, #338, #450 (17) | accepted (batch) | closed, cross-reviewed to CLOSE, behavior-preserving |
| #510 | accepted | "system working as designed" — cross-review caught every blocker |
| #511 | accepted | same; the 4-round cost is the retro subject, not a rejection |

**Bugs surfaced during demo:** none (code). The demo surfaced a **process** finding, not a defect — see Retro.

## Retro

**Triggered by:** interventions recorded (#510/#511 escalation ladders; wave-1 `just verify` socket-test flake). This retro went deep by PO direction — it became a data-backed process investigation.

### Plan vs. actual
- 100% completion, but 6/19 stories (32%) needed an escalation redo — every one caught by the cross-model review gate, never self-caught. The gate earns its cost; it operates *after* the spend.
- #511 alone = 4 rounds / +4543 lines to close one review-debt ticket. Cost-per-close, not close-rate, is the waste.

### Friction log (data-verified this session)
- **The review-debt circle is real and measured.** Sprint-5 was 58% cleanup of sprint-4's review output (9 `sprint-4-followup` + 2 `review-followup`); sprint-3 spawned 20 follow-ups and was itself 57% prior-review debt. Each sprint's review generates 8–20 follow-ups that become the majority of the next.
- **#511 root cause:** reverse-engineered OpenAI Codex's hook/config resolution across 4 rounds while the Apache-2.0 source sat unread at `inspiration/codex`. Cross-sprint audit proved this is a **first detonation**, not a recurring pattern (sprint-3 #502 and sprint-4 #303 read dependency source correctly).
- **Process retro items don't stick:** sprint-4's #521 ("stop guessing file ownership") failed to prevent sprint-5's own planner mis-guessing ownership on #516/#263/#513. A prose action item aimed at a future planner's memory rots within one sprint.

### Process improvements — EXECUTED, not filed (the loop-engineering reframe)
This retro produced the doctrine that a loop must contain a step that improves the loop, and in the agentic world **Retro executes that improvement inline (e2e-dry-run gated), never slices it into a future sprint** — because backlog is the queue that de-prioritized process work and rotted #521.
- **Design captured:** `docs/plan-agentic-discovery-and-retro-enforcement.md` (canonical; byte-identical copy in `AI_Projects/skills/docs/`). Produced by a Fable↔Codex debate, PO-adjudicated. Discovery gates (A–F) + Retro-executed enforcement (§4) + process e2e dry-run gate (§5).
- **Build authorized as the highest-priority work → itr#601** (SOLO AGENT ONLY, not a sprint — the loop machinery must be built in one coherent context; the features that later flow *through* it sprint normally).
- **#511 done-right → itr#600** (verify the audit against `inspiration/codex` source; re-scope #528).

### Agent-specific learnings
- Assertions must be data-backed: "this happens all the time" was disproven by the sprint-history audit (→ memory `feedback_no_claims_without_data`).
- The retro→action-item loop is itself the highest-leverage target; §4 replaces "file a follow-up" with "build + dry-run-verify inline."
- This retro is the first working example of its own doctrine — the loop-improvement step surfaced an improvement neither PO nor agent could have specified beforehand.
