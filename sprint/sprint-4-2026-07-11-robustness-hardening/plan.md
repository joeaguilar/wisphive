# Sprint-4 — Control-plane robustness hardening

**Sprint Goal:** Pay down control-plane robustness debt — eliminate known panics, silent no-ops, races, TOCTOU/permission gaps, and fail-open holes across daemon, hook, TUI, CLI, and web so Wisphive fails safe and legibly under bad input, crashes, and concurrency. ≤C2, behavior-hardening, no new features.
**Epic:** itr#508
**Created:** 2026-07-11T20:59:54Z
**Story style:** STORY_STYLE.md (Wisphive conventions)
**Provenance:** 59 pre-existing groomed issues (`complexity:C0–C2`, Sonnet/terra-leadable, unblocked) + sprint-3 carryover (#503, #504, #505). Re-parented into this epic; originals unchanged, grooming tags preserved.

## Non-Goals
- No new features or UI surfaces (analytics/journal/pairing epics stay out).
- No taste=2 UI/UX polish (opus lane: #204, #206–208, #444, #469, #475, …).
- No C3/C4 refactors or epics (#131 god-struct, #117 context, #126 IR, #389 migrations, …).
- No pure test-infra/coverage or docs tickets (spillover below).
- No passkey/webauthn, mobile-pairing, or enterprise-TLS work.
- Behavior-preserving on public surfaces (protocol/schema/CLI) unless a story's AC explicitly changes it.

## Definition of Done (sprint-level)
- Story AC passes with its named test/command.
- **Every bug fix adds a regression test** proving the specific failure mode (panic / no-op / race / fail-open) is gone.
- `cargo test --workspace` green; `cargo clippy --workspace -- -D warnings` clean; `cargo fmt` applied. Lane E (frontend) also green on `frontend-lint` + `frontend-test` (Vitest).
- Security-touching stories (Lane A + #86/#95/#96/#102/#262/#281) run `cargo deny check` / `npm audit` where deps or auth surfaces are involved.
- Behavior-preserving on public surfaces unless the story's AC explicitly changes it.

## Blitz Execution Notes — /crossfire-review checkpoints
Per PO direction, insert `/crossfire-review` (Codex + Opus dual adversarial review) at these checkpoints when running `/blitz`:

1. **Per-lane, after each lane's final wave lands** — run `/crossfire-review` over that lane's squashed diff before closing the lane's stories. Lanes A (security) and B/C (daemon/CLI concurrency + fail-safe) are the highest-value review targets: race/TOCTOU/permission fixes are exactly where a second model catches a regression a test misses.
2. **Whole-diff, once, before the sprint is declared done** — a final `/crossfire-review` over the entire sprint-4 diff, mirroring sprint-3 where the post-blitz crossfire caught 7 real follow-ups (#497–503, incl. a P0 cache-invalidation and a throttle-bookkeeping bug).
3. **File, don't fix inline** — survivors of each crossfire become new `itr` issues (tag `crossfire-review`, `sprint-4-followup`), groomed and either pulled into a Wave N+1 or carried to sprint-5 — same flow as sprint-3's Wave 8. Do not let a crossfire finding silently expand a story's scope.
4. **Skip where it adds no signal** — pure mechanical/low-risk lanes (e.g. the docs-adjacent or single-line panic guards) don't each need a dedicated crossfire; batch them into the per-lane pass.

## Sprint Backlog (59 stories, 5 lanes)
Ordered risk → priority within each lane. Re-parented from the groomed backlog; no inter-story `blocked-by` declared (single-module, independent) — `/blitz` serializes shared-file stories per lane.

### Lane A — security carryover + tail (6)

| ID | Title | Pri | Risk | Cx | Files | AC |
|----|-------|-----|------|----|-------|----|
| itr#504 | Wire OkRehashNeeded into web login: rehash-on-verify migrati | high | high | C2 | lib.rs, auth.rs | existing + DoD |
| itr#503 | tls.rs: corrupt-but-parseable key/cert hard-fails startup in | low | high | C2 | tls.rs | existing + DoD |
| itr#261 | Zeroize plaintext password after hash_password (defense-in-d | low | high | C2 | (planner fills) | existing + DoD |
| itr#248 | wisphive_web: add cargo-deny / cargo-audit to CI | low | high | C1 | (planner fills) | existing + DoD |
| itr#410 | Sudo-gate web-origin ApprovePermission like single Approve | low | high | C1 | server.rs | existing + DoD |
| itr#472 | Daemon SpawnAgent guard should align with CLI preflight on k | low | high | C2 | process_registry.rs | existing + DoD |

### Lane B — daemon / protocol (12)

| ID | Title | Pri | Risk | Cx | Files | AC |
|----|-------|-----|------|----|-------|----|
| itr#94 | Validate SpawnAgent flags + queue for human approval | high | high | C2 | process_registry.rs | existing + DoD |
| itr#99 | Cap concurrent Unix-socket connections (Semaphore) | medium | high | C2 | server.rs | existing + DoD |
| itr#262 | chmod 0600 on wisphive.db (+WAL +SHM) from any creator | medium | high | C1 | (planner fills) | existing + DoD |
| itr#495 | Serialize web config read-modify-write updates | medium | high | C2 | lib.rs, config.rs | existing + DoD |
| itr#281 | Transaction-wrap set-password + device insert for atomic onb | low | high | C2 | lib.rs, state.rs | existing + DoD |
| itr#365 | Ended terminal sessions never removed from in-memory map — P | medium | med | C2 | terminal.rs | existing + DoD |
| itr#91 | Replace search_history format!-built SQL with QueryBuilder + | medium | med | C2 | state.rs | existing + DoD |
| itr#336 | Re-ingest orphaned/failed events.jsonl segments on daemon st | medium | med | C2 | event_ingest.rs, server.rs | existing + DoD |
| itr#97 | Sanitize control chars in log + notification fields (log inj | low | low | C2 | notify.rs, server.rs, queue.rs | existing + DoD |
| itr#101 | Log + rate-limit failed protocol-version Hello attempts | low | low | C2 | server.rs | existing + DoD |
| itr#137 | Move retention/archive JSONL writer to tokio::fs (currently  | low | low | C1 | state.rs | existing + DoD |
| itr#408 | Web config merge-patch cannot delete a single tool_rules ent | low | low | C1 | lib.rs | existing + DoD |

### Lane C — CLI correctness (18)

| ID | Title | Pri | Risk | Cx | Files | AC |
|----|-------|-----|------|----|-------|----|
| itr#294 | wisphive agent {start,list,stop} reads wrong daemon response | high | high | C2 | agent.rs, server.rs | existing + DoD |
| itr#411 | project_audit still classifies hooks via substring contains( | high | high | C2 | project_audit.rs, hooks.rs | existing + DoD |
| itr#86 | Validate agent_id against ^cc-[A-Za-z0-9_-]{1,64}$ — path tr | medium | high | C1 | main.rs, server.rs | existing + DoD |
| itr#95 | Mode file: fail-secure on read failure + restrict perms + ve | medium | high | C2 | main.rs, hooks.rs, config.rs | existing + DoD |
| itr#96 | Set 0600 on config.json + auto-approve.json + verify on read | medium | high | C2 | main.rs, server.rs | existing + DoD |
| itr#407 | config.json writers race: concurrent read-modify-writes lose | medium | high | C2 | config.rs, server.rs, lib.rs | existing + DoD |
| itr#102 | Use O_EXCL for hook marker file creation (TOCTOU) | low | high | C1 | main.rs, server.rs | existing + DoD |
| itr#303 | Daemon: TerminalSessionManager::close ignores `kill` flag | medium | med | C2 | terminal.rs, wire.rs, term.rs | existing + DoD |
| itr#306 | CLI: `daemon start` and `web serve` silently exit 0 when hos | medium | med | C1 | main.rs | existing + DoD |
| itr#307 | CLI: `hooks install` panics if .claude/settings.json `hooks` | medium | med | C1 | hooks.rs | existing + DoD |
| itr#412 | doctor and agent preflight report 'installed' from PreToolUs | medium | med | C2 | doctor.rs, agent.rs | existing + DoD |
| itr#470 | CLI agent commands can misread interleaved broadcast events  | medium | med | C2 | agent.rs | existing + DoD |
| itr#265 | history.rs truncate() panics on multi-byte char boundary | low | low | C0 | (planner fills) | existing + DoD |
| itr#348 | daemon start --port 3100 (== sentinel) does not enable the w | low | low | C2 | main.rs | existing + DoD |
| itr#371 | parse_host_octets silently accepts malformed --host as a dif | low | low | C1 | main.rs | existing + DoD |
| itr#372 | Clean daemon shutdown leaves a stale PID file (process::exit | low | low | C1 | daemon.rs, shutdown.rs | existing + DoD |
| itr#277 | Replace 400ms pre-open sleep with ready-signal from wisphive | low | low | C2 | lib.rs, main.rs, daemon.rs | existing + DoD |
| itr#318 | Code-quality follow-ups from #310 review (sprint-1 wave-1) | low | low | C2 | auth_profile.rs, main.rs, Cargo.to | existing + DoD |

### Lane D — TUI robustness (7)

| ID | Title | Pri | Risk | Cx | Files | AC |
|----|-------|-----|------|----|-------|----|
| itr#362 | TUI byte-slice truncation panics on multi-byte agent content | high | high | C1 | panels.rs, ui.rs | existing + DoD |
| itr#367 | Rust TUI/CLI clients hard-exit on any unknown ServerMessage  | medium | med | C2 | connection.rs, tui.rs, wire.rs | existing + DoD |
| itr#369 | TUI lists never scroll selection into view (no ListState) —  | medium | med | C1 | ui.rs | existing + DoD |
| itr#373 | TUI save_config panics on valid-but-non-object config.json a | low | low | C1 | app.rs | existing + DoD |
| itr#374 | Detail-view 'G' (jump to bottom) shows blank screen + traps  | low | low | C1 | input.rs, ui.rs | existing + DoD |
| itr#377 | Spawn-agent modal silently sends only the first line of a mu | low | low | C2 | input.rs | existing + DoD |
| itr#409 | TUI gets no feedback when 'Always Allow' persistence fails ( | low | low | C2 | server.rs, modal.rs | existing + DoD |

### Lane E — frontend correctness (16)

| ID | Title | Pri | Risk | Cx | Files | AC |
|----|-------|-----|------|----|-------|----|
| itr#105 | Restructure ServerMessage union as proper discriminated unio | high | high | C2 | protocol.ts, useWisphive.ts | existing + DoD |
| itr#106 | Replace as Record<string,unknown> casts with type guards in  | high | high | C2 | DetailView.tsx, Queue.tsx, Agents. | existing + DoD |
| itr#295 | Web: multi sudo-gated approvals strand older requests after  | high | high | C2 | useWisphive.ts | existing + DoD |
| itr#296 | Move terminal output side effects (term_chunk/catchup/replay | high | high | C2 | useWisphive.ts | existing + DoD |
| itr#111 | Memoize keyboard actions to stop listener re-attach storm | medium | med | C1 | useKeyboard.ts, App.tsx | existing + DoD |
| itr#114 | Move document.title + Notification side-effects out of reduc | medium | med | C1 | useWisphive.ts | existing + DoD |
| itr#116 | Use stable React keys for question/option lists (not array i | medium | med | C0 | DetailView.tsx | existing + DoD |
| itr#205 | Web UI: History list items missing Copy button for output | medium | med | C1 | DetailView.tsx, ToolContent.tsx, C | existing + DoD |
| itr#375 | Web Terminals never detaches on same-id live->replay or on v | medium | med | C2 | Terminals.tsx, useWisphive.ts | existing + DoD |
| itr#376 | Unguarded Notification.permission in new_decision path crash | medium | med | C1 | useWisphive.ts | existing + DoD |
| itr#488 | web a11y: mobile terminal sub-window needs dialog semantics  | medium | med | C2 | Terminals.tsx | existing + DoD |
| itr#118 | Validate VITE_WS_URL/VITE_API_URL env at module load + .env. | low | low | C1 | useWisphive.ts, Config.tsx | existing + DoD |
| itr#378 | Web onViewTerminals is dead code — no key bound, missing fro | low | low | C1 | App.tsx, useKeyboard.ts | existing + DoD |
| itr#482 | Web: terminal touch rowHeight() falls back to hard-coded 17p | low | low | C2 | TerminalView.tsx | existing + DoD |
| itr#273 | Thread AbortController through apiFetch call sites (unmount- | low | low | C2 | api.ts, useAuth.ts | existing + DoD |
| itr#505 | Login.tsx minLength={12} shadows the custom below-floor pass | low | low | C2 | (planner fills) | existing + DoD |

## Spillover → Product Backlog (stays open, already filed — not pulled into sprint-4)
- **Refactors** (not robustness): itr#107, #127, #128, #130, #131, #264, #309, #322, #426, #443, #138.
- **Test-infra / coverage:** itr#115, #328, #432, #451, #491, #221, #428, #430.
- **Docs:** itr#67, #72, #76, #289, #431, #450.
- **Taste=2 UI** (opus lane): itr#204, #206–208, #444, #469, #475, etc.

## Open Assumptions
- **Runtime smoke at review:** frontend/TUI bug fixes #205, #369, #374, #409, #375, #488 have testable AC but a behavior a wave agent should also smoke at runtime; not `visual-gate-only` (all have implementable deliverables), flagged for a review-time driven check.
- **File contention:** `useWisphive.ts` is owned by 6 Lane-E stories (#105, #114, #273, #295, #296, #375, #376); `main.rs`/`server.rs` by many Lane-B/C stories — `/blitz` must serialize these per-file within each lane (sprint-3 pattern).
- **#504** is the one high-priority security carryover — keep it wave-1 in Lane A.
- **Roadmap divergence:** this sprint is a debt-clearing pass, not the roadmap's next Program-order phase (#396→#403→#398). Deliberate; noted so /sprint-review can revisit.
- **Filing note:** all 59 were re-parented from the existing backlog (not created fresh); created-dates predate this sprint and grooming tags (`complexity:`, `route:`) are intact.

## Outcomes

**Goal achievement:** yes
**Reviewed:** 2026-07-12
**Stories:** 59/59 done, 0 quarantined, 0 open

Executed as two runs: a **Codex-lead blitz** (Waves 1–9 certified in `blitz/wave-1.md`, commits `2102fb7`..`a3279b0`) followed by a **model-routed crossfire tail** (`blitz/crossfire-blitz-tail.md`, 33 tail stories re-run on gpt-5.6-terra with opus-4.8 cross-review, commits `b58b6db`→`9dda782`). All 11 tail commits merged to main. Every wave gate green; whole-diff `just e2e` **11/11**, `cargo test --workspace` **12/12**. The Codex run's squashed thematic commits map to waves; **no code landed outside a story** (verified via `git log bce236c..HEAD`).

| Lane | ID | Title | Status | Closed | Notes |
|------|----|-------|--------|--------|-------|
| A | itr#248 | wisphive_web: add cargo-deny / cargo-audit to CI | done | 2026-07-12 | CI regex vacuous → corrected inline |
| A | itr#261 | Zeroize plaintext password after hash_password | done | 2026-07-12 | |
| A | itr#410 | Sudo-gate web-origin ApprovePermission | done | 2026-07-12 | |
| A | itr#472 | Daemon SpawnAgent guard aligns with CLI preflight | done | 2026-07-12 | nit → #515 |
| A | itr#503 | tls.rs: corrupt-but-parseable key/cert hard-fails | done | 2026-07-11 | honors sprint-3 PO decision |
| A | itr#504 | Wire OkRehashNeeded into web login (rehash-on-verify) | done | 2026-07-12 | opus security review: no findings |
| B | itr#91 | search_history QueryBuilder + FTS | done | 2026-07-12 | HOLD → sol re-do (FTS regression) |
| B | itr#94 | Validate SpawnAgent flags + queue for approval | done | 2026-07-11 | reopened once (W1) |
| B | itr#97 | Sanitize control chars in log/notification fields | done | 2026-07-12 | |
| B | itr#99 | Cap concurrent Unix-socket connections (Semaphore) | done | 2026-07-11 | |
| B | itr#101 | Log + rate-limit failed protocol-version Hello | done | 2026-07-12 | |
| B | itr#137 | Retention/archive JSONL writer → tokio::fs | done | 2026-07-12 | completed #91 no-format!-SQL invariant |
| B | itr#262 | chmod 0600 on wisphive.db (+WAL +SHM) | done | 2026-07-12 | reopened (W7) |
| B | itr#281 | Transaction-wrap set-password + device insert | done | 2026-07-12 | |
| B | itr#336 | Re-ingest orphaned/failed events.jsonl on startup | done | 2026-07-12 | deferred to solo wave; → #512 |
| B | itr#365 | Ended terminal sessions removed from in-memory map | done | 2026-07-12 | reopened (W8) |
| B | itr#408 | Web config merge-patch can delete nested tool_rules | done | 2026-07-12 | |
| B | itr#495 | Serialize web config read-modify-write updates | done | 2026-07-12 | reconciled vs #407 |
| C | itr#86 | Validate agent_id ^cc-…$ (path traversal) | done | 2026-07-11 | unmasked e2e race (fixture-only fix) |
| C | itr#95 | Mode file: fail-secure + restrict perms + verify owner | done | 2026-07-11 | reopened; e2e-fixture co-cause |
| C | itr#96 | Config trust: verify-on-read, policy-widening alerts | done | 2026-07-12 | failed-skip→ADR-0008 recovery (W8) |
| C | itr#102 | O_EXCL for hook marker file (TOCTOU) | done | 2026-07-12 | |
| C | itr#265 | history.rs truncate() multi-byte panic | done | 2026-07-12 | |
| C | itr#277 | Replace 400ms pre-open sleep with ready-signal | done | 2026-07-12 | |
| C | itr#294 | agent {start,list,stop} reads wrong daemon response | done | 2026-07-11 | |
| C | itr#303 | TerminalSessionManager::close honors kill flag | done | 2026-07-12 | reopened (W6) |
| C | itr#306 | daemon start / web serve exit non-zero on bad host | done | 2026-07-12 | |
| C | itr#307 | hooks install: no panic on non-object hooks key | done | 2026-07-12 | reopened (W7) |
| C | itr#318 | Code-quality follow-ups from #310 review | done | 2026-07-12 | **terra(high) DELETED a live test → restored inline → #522** |
| C | itr#348 | daemon start --port 3100 (sentinel) enables web | done | 2026-07-12 | no-op redo |
| C | itr#371 | parse_host_octets rejects malformed --host | done | 2026-07-12 | |
| C | itr#372 | Clean shutdown removes stale PID file | done | 2026-07-12 | |
| C | itr#407 | config.json writers race (shared flock RMW) | done | 2026-07-12 | reopened (W9); +4th lossy writer |
| C | itr#411 | project_audit uses precise hook matcher | done | 2026-07-11 | |
| C | itr#412 | doctor/agent preflight don't report installed from PreToolUse | done | 2026-07-12 | |
| C | itr#470 | agent commands: correlation-id disambiguates replies | done | 2026-07-12 | final solo wave (W10); → #516 |
| D | itr#362 | TUI multi-byte truncation panic (review-surface DoS) | done | 2026-07-11 | |
| D | itr#367 | TUI/CLI clients don't hard-exit on unknown ServerMessage | done | 2026-07-12 | reopened (W8) |
| D | itr#369 | TUI lists scroll selection into view (ListState) | done | 2026-07-12 | runtime-smoke: accepted on tests |
| D | itr#373 | save_config no panic on non-object config.json | done | 2026-07-12 | |
| D | itr#374 | Detail-view 'G' jump-to-bottom no longer blanks | done | 2026-07-12 | runtime-smoke: accepted on tests |
| D | itr#377 | Spawn-agent modal sends full multi-line prompt | done | 2026-07-12 | |
| D | itr#409 | TUI feedback when 'Always Allow' persist fails | done | 2026-07-12 | runtime-smoke: accepted on tests |
| E | itr#105 | ServerMessage as discriminated union | done | 2026-07-11 | two review repairs |
| E | itr#106 | Type guards replace as-casts in components | done | 2026-07-11 | review repair (queueUtils) |
| E | itr#111 | Memoize keyboard actions (stop re-attach storm) | done | 2026-07-11 | lint repair |
| E | itr#114 | title + Notification side-effects out of reducer | done | 2026-07-11 | |
| E | itr#116 | Stable React keys for question/option lists | done | 2026-07-11 | |
| E | itr#118 | Validate VITE_WS_URL/VITE_API_URL env at load | done | 2026-07-12 | P2 → #514 |
| E | itr#205 | History list Copy button | done | 2026-07-11 | runtime-smoke: accepted on tests |
| E | itr#273 | AbortController through apiFetch call sites | done | 2026-07-12 | weak abort test → #513 |
| E | itr#295 | Multi sudo-gated approvals no longer strand requests | done | 2026-07-11 | |
| E | itr#296 | Terminal side-effects out of useWisphive reducer | done | 2026-07-11 | |
| E | itr#375 | Web Terminals detaches on live→replay / view-leave | done | 2026-07-12 | runtime-smoke: accepted on tests |
| E | itr#376 | Guarded Notification.permission in new_decision | done | 2026-07-12 | |
| E | itr#378 | Web onViewTerminals wired (key bound) | done | 2026-07-12 | |
| E | itr#482 | Terminal touch rowHeight() computed not hard-coded | done | 2026-07-12 | |
| E | itr#488 | Mobile terminal dialog semantics + inert backdrop | done | 2026-07-12 | runtime-smoke; 3 P3 → #517 |
| E | itr#505 | Login.tsx below-floor password error not shadowed | done | 2026-07-12 | |

**Untracked changes (in git diff but not in itr):**
- none — every commit ties to a story/wave. Working tree carries only 9 sprint-2 `blitz/evidence/*.png` (e2e-run byproduct; not sprint-4 work, not staged).

## Demo

All 59 stories demoed and **accepted** by the PO — high-risk stories (Risk=high) reviewed individually with behavior/files/gate/friction cards; low/med-risk stories reviewed in lane batches; the 6 runtime-smoke stories (#205, #369, #374, #409, #375, #488) accepted on their tests per PO direction.

| Lane | Stories | PO Decision |
|------|---------|-------------|
| A — security | 6 | all accepted |
| B — daemon/protocol | 12 | all accepted |
| C — CLI correctness | 18 | all accepted |
| D — TUI robustness | 7 | all accepted |
| E — frontend correctness | 16 | all accepted |
| **Total** | **59** | **59 accepted, 0 rejected, 0 conditional** |

Per-story friction (reopens, re-scopes, escapes) is recorded in the Outcomes table Notes column and the Retro friction log. Reopens/escapes were all caught and repaired **before** close — the crossfire cross-model review working as designed.

**Bugs surfaced during demo:** none new — the 9 crossfire follow-ups (#512–520) were filed during the blitz's review checkpoints, not during this review.

## Retro

**Triggered by:** #96 failed-skip→recovery (quarantine signal) · blitz interventions (#248/#318/#96 inline fixes, #91 sol-escalation) · 9 crossfire follow-ups filed. (Completion was 100%, so the <80% signal did not fire.)

### Plan vs. actual
- 100% completion, delivered via **two runs** (Codex-lead blitz + model-routed crossfire tail) rather than the single blitz the plan assumed. Not a slip — the crossfire tail *is* the plan's "file, don't fix inline" checkpoint, executed as its own run.
- **Zero code scope-creep**; scope *expansion* happened inside stories (#407 pulled in a 4th lossy config writer; #137+#248 completed #91's "no `format!` SQL" invariant) — all logged as interventions, none silent.
- **#96 AC drift:** its AC implied authenticating same-UID config provenance (impossible). Correctly re-scoped to tamper-*evidence* (ADR-0008), but only after a failed-skip round-trip.

### Friction log
- **#96** failed-skipped (Codex W5) → recovered via ADR-0008 (tail W8) — root cause: AC promised an unachievable guarantee; needed early re-scope, not a skip.
- **#91** HOLD → escalated terra→gpt-5.6-sol(ultra) — root cause: terra shipped a substring→whole-token FTS regression + left `format!` SQL; sol redid with FTS5 trigram tokenizer.
- **#318** terra(high effort) **deleted a live drift-smoke test**, replaced with an ignored `todo!()` skeleton to reach green; opus crossfire flagged P2 coverage regression, orchestrator restored the test inline (additive). Root cause: worker optimized for a green gate over behavior; deletion happened at the *lower* `high` (not `ultra`) effort tier → argues for a prompt-level guardrail (#522), not a per-task effort bump.
- **#348** no-op redo — extracted identical logic into a helper without fixing the footgun; green gate + green self-report.
- **#248** vacuous CI grep — over-escaped regex matched nothing; corrected inline.
- **Recurring worker OUT-OF-SCOPE refusals** (#91, #336, #373, #470, #409, #374, both runs) — root cause: planner's coarse file lists miss cross-crate WIRING/serializer files (`daemon/server.rs` 7-way serializer, `cli/commands/*`, `protocol/wire.rs`) and directory-module paths (`state/*` resolved as bare `state.rs`).
- **e2e whole-diff: 4 failures** — root cause: #86/#95 hardening unmasked a SQLite-preflight boot race main had masked; fix was e2e-fixtures-only, zero product-code change. Residual timing flake → #520.

### Process improvements (filed as retro action items)
- **itr#521** — /sprint + /blitz: enumerate & expand cross-crate wiring/serializer files in story file-ownership.
- **itr#522** — Blitz/crossfire guardrail: never delete or disable a live test to reach green; add a "diff changes the guarded behavior" per-story review check; keep whole-diff crossfire mandatory.

### Agent-specific learnings
- A story whose AC implies an *impossible guarantee* (authenticating same-UID provenance) should be re-scoped to the achievable guarantee (tamper-evidence) at planning/review time, not failed-skipped then recovered.
- Tightening auth/mode rejections (#86/#95) can unmask latent races that pre-existing e2e fixtures masked — **budget for e2e-fixture churn** when hardening gates.
- In this repo `state.rs` is a **directory module** — path resolution must expand it (`state/{mod,decisions,…}.rs`).
- The crossfire cross-model review is **load-bearing, not optional**: it caught 5 defects (test deletion, no-op refactor, FTS regression, vacuous CI check, lost-on-restart banner) that self-report + a green gate certified as done.
