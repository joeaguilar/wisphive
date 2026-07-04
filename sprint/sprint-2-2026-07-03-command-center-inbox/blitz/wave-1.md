# Blitz log — Sprint-2 Command Center Inbox (finish)

Started against HEAD `b1a42cf`. Backlog: #436, #437, #438 (chained; #434/#435 already done).

## Config
- Tracker: `itr` (epic #433). List: `itr get <id>`. Close: `itr close <id>`.
- Dep graph: kgr present — `kgr check` clean (no cycles/rule violations).
- Verify gate: `cargo test --workspace && cargo clippy --workspace -- -D warnings && cargo fmt --all -- --check && just frontend-lint && just frontend-test`
- Concurrency: 5 (effective 1 — serial backlog).
- Repos: `.`
- Stop: backlog empty | max_waves=3.

## Waves
- **Wave 1 — #436** S3 auto-answer feed panel. Owns `AutoAnswerFeed.tsx`(new), `Inbox.tsx`, `useWisphive.ts`. No neighbors.
- **Wave 2 — #437** S4 deferred affordance (+absorbed #435 review findings: untruncated question, project·session grouping). Owns `Inbox.tsx`, `useWisphive.ts`, `DetailView.tsx`. Serial after #436 (shared files).
- **Wave 3 — #438** S5 runtime smoke + close #399. Owns `docs/smoke/CHECKLIST.md`. Orchestrator/human-driven runtime evidence.

## File conflicts
- `Inbox.tsx`, `useWisphive.ts` owned by both #436 and #437 → serialized into consecutive waves.

## Semantic warnings
- #437 depends on #436's `AutoAnswerFeed` + `useWisphive` audit slice; #437 agent must read #436's committed-to-disk changes first.

## Interventions
- **Wave 2 — #437 out-of-owned-file wiring (ACCEPTED):** agent made additive edits to `App.tsx` (+`focusTerminalId` state, `onFocusTerminal`, widened `onDeny(id,msg)`) and `Terminals.tsx` (optional `focusSessionId`/`onFocusHandled` props + auto-select effect) to make the deep-link a real feature, not a dead button. #435(App)/Terminals stories closed, no concurrent wave → zero conflict. Reviewed diff: clean, commented, tagged itr#437. Kept.
- **Wave 2 — #437 wire gap (REPORTED by agent, not patched):** `AuditDecision` wire carries no `tool_input`, so deferred rows show tool name + honest "answer in your terminal" pointer, NOT the literal AskUserQuestion question text/options. Agent obeyed stop-and-report (did not touch backend/protocol). Affects #437 AC ("showing question text/options") and #438 smoke AC#2 ("deferred row with question text"). Decision pending with PO.
- **Wave 2.5 — wire fix (PO chose option 1, DONE):** Added `tool_input: Option<serde_json::Value>` to `AuditDecision` (dropped `Eq` — `serde_json::Value` is `PartialEq` not `Eq`; `ServerMessage` derives neither, no hash-key users → safe). `ingest_line` forwards the **already-redacted** input (`redact::redact_value`, hook main.rs:1647) **only for `kind==Deferred`** (auto/denied stay `None`, wire lean; null-input elicitations → `Some(Null)` gracefully). `protocol.ts` mirrors `tool_input?: Record<string,unknown>|null`. New `parseDeferredPrompt`/`deferredPromptSummary` (queueUtils) → one-line row summary + full untruncated question/options (or plan text, or pretty-JSON fallback) in `DeferredDetailView`, read-only, deep-link CTA, no fake approve/deny, inert React text nodes. Patched sibling literals `decisions.rs:475` (snapshot seed `None`), `wire.rs:811/842`. Orchestrator wave gate: 161 protocol tests + clippy clean + fmt clean + 80/80 vitest. #437 AC + #438 AC#2 now satisfiable on live data. (Stale LSP diagnostic on wire.rs:811 confirmed false — literal has the field.)

## Outcomes
- **Wave 1 — #436: CLOSED.** New `AutoAnswerFeed.tsx` + `AutoAnswerFeed.test.tsx`; `Inbox.tsx` gained `(view)`/`(hide)` toggle + exact header `0 waiting · N auto-answered in last hour (view)`; `app.css` feed styling. `useWisphive.ts` needed no change (audit slice already present from #434/#435 — agent owned it, confirmed sufficient, did not touch protocol.ts/Rust). Wave gate re-run by orchestrator: RUST green + frontend lint clean + 66/66 vitest. Live runtime evidence deferred to #438.
- **Wave 2 — #437: CLOSED.** Deferred waiting-on-you rows (deferred label, deep-link "Focus terminal" for wisphive sessions / go-to-terminal pointer for hook-only), `DeferredDetailView` read-only detail, daemon-queued rows expand to untruncated input + deny-with-message, project·session colour grouping. Wave gate re-run by orchestrator: all rust tests + clippy clean + fmt clean + 74/74 vitest. Interventions logged above (out-of-owned wiring accepted; wire gap → Wave 2.5).
- **Wave 3 — #438: CLOSED + #399/#433 CLOSED.** Runtime §10 evidence via new `e2e/inbox-command-center.spec.ts` — REAL `wisphive daemon start --web` (isolated HOME) + REAL `wisphive-hook` binary authoring genuine deferred (AskUserQuestion→`always_ask:intrinsic`) + auto-approved (Read→`level:all`) `events.jsonl` records across projects alpha/bravo, + socket hook-client for a blocking daemon-queued `Grep` decision. All 5 ACs proven + `wisphive audit` oracle cross-check. 6 screenshots in `blitz/evidence/`. Human perception residue appended to `docs/smoke/CHECKLIST.md`. #399 (centerpiece) + epic #433 closed.

### Wave 3 interventions
- **Sudo-gate discovery:** first smoke run hung on approving a `Bash` decision — `Bash/Write/Edit/MultiEdit/NotebookEdit` are sudo-class (`sudo_gate.rs`) and pop a reauth modal instead of resolving. Switched AC1's queued tool to non-sudo `Grep` (that reauth path is core-flows' job). Also raised the test timeout to 180s for 3 real-hook cold-spawns.
- **#446 filed + fixed (e2e regression from #435):** full e2e suite was red — `core-flows.spec.ts` + `smoke.spec.ts` assert the old default `.queue-layout`, but #435 made **Inbox** the default view (uncaught: e2e isn't in the per-story gate). Filed #446, fixed inline (specs now click the Queue nav first). Full e2e now **8/8 green**.
- **Stray removed:** deleted untracked `e2e/inbox-smoke.spec.ts` — a prior-session #438 attempt that hand-appended events.jsonl records (less faithful) and failed; superseded by the passing real-hook `inbox-command-center.spec.ts`.
