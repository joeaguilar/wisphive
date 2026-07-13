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
<!-- Populated by /sprint-review after /blitz runs. -->

## Demo
<!-- Populated by /sprint-review. -->

## Retro
<!-- Populated by /sprint-review. -->
