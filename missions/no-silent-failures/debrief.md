# Mission debrief — no-silent-failures

**Verdict: COMPLETE.** All three acceptance oracles green against integrated main
(`oracle-rust` 46.6s · `oracle-frontend` 7.9s · `oracle-e2e` 129.9s, 19/19), every chain
terminal. Closed 2026-07-19. Baseline `41c14ee` → final `25432ff`.

## Outcomes vs contract

- **O1 — the web UI never swallows a failure: DELIVERED** (chain 602, sealed complete).
  Error-surfacing substrate (banner + correlation ids + typed `CommandError` for web-origin
  callers) live; the PO-reported "Spawn Agent form does nothing" regression root-caused and
  fixed with real-exec e2e proof (`AgentSpawned` producer + Spawned Processes UI).
- **O2 — no silent fail-open on the managed-spawn path: DELIVERED** (chain 603, sealed
  complete, 6 links). Capacity rejection fails closed + audited (#560); explicitly-unknown
  provider refuses loudly, audited and retroactively queryable (#562 + #607); headless
  intrinsic asks resolve as deterministic audited denies, not silent blocks (#559); agents
  blocked on a human decision are never falsely reaped (#568); stopping an agent kills the
  whole process group with confirmed-gone semantics (#561). ADR-0001 and ADR-0002 amended
  in-link.
- **O3 — sessions survive a daemon restart (plan A): DELIVERED** (chain 588 plan-A slice,
  sealed complete). Pinned sessions respawn on daemon start as live shells with seeded
  scrollback and an honesty banner, crash- and graceful-path, proven by the first-ever
  Terminals Playwright spec with a real daemon-restart fixture. Epic #588 stays open for
  plan B/C.

## Per-chain terminal state

| Chain | Links | Councils | Reworks | State |
|---|---|---|---|---|
| 602 | 2/2 landed | 2 (2-1 proceed ×2) | 2 (single-round) | sealed complete; epic closed |
| 603 | 6/6 landed (incl. ratified L2.5 extension #607) | 3 (2-1, 3-0, 3-0) | 5 (single-round) | sealed complete; epic closed |
| 588 (plan-A slice) | 3/3 landed | 0 | 3 (single-round) | sealed complete; epic stays open for plan B/C |

Zero quarantines. Zero cancellations. **Cancellation-traceable-to-defaulted-assumption
rate: 0/0** — every intake default (Q1–Q4, X-A1/X-A2, A-A4/A-A6, B-A7/B-A8) survived the
mission; none was falsified. X-A1 (serial landings absorb same-file adjacency) was tested
by three real server.rs crossings and held every time.

## What the process caught (why the ceremony paid)

Every chain shipped at least one defect to review that the builder's green gates missed —
the reviewer, not the builder, is the safety margin:

- CLI-hang on refused spawn (602/L1 — correlation-keyed reply split would have hung the CLI).
- Cleartext env-secret leak class (588/L1 — key-rule probe gap, 5 confirmed cases).
- Command-line fabrication on corrupt respawn spec (588/L3 MUST-FIX — silent argv
  fabrication audited as success).
- Stop-deny semantic inversion (603/L3 — a promptless deny on Stop means "keep working").
- Resolve-instant reap race (603/L4) and two mutation-proven oracle gaps incl. the
  unpinned whole-tree-confirmed gate (603/L5 — only the asymmetric leader-dies/
  grandchild-lingers tree pins it).
- Evidence-integrity correction (588/L3): a builder claim of "kill -9 gate re-entry
  PROVEN" had no oracle in the tree — withdrawn in journal, structural argument ratified
  instead.

Councils: 5 total; the first three split 2-1 (evidence-seat rescope pattern → operator
brief, default executed), the last two unanimous proceed. Council-imposed derived
assumptions that now bind future work: **B-A9** ("audited" = end-to-end queryable — write
→ ingest → decision_log → query surface → render) and **B-A10** (registered, unratified:
lifecycle stop outcomes are reply-only with no audit trail — scoped into itr#620 for PO
ratification or reversal).

## FACTS learned this mission (durable, in missions/FACTS.jsonl)

Worktrees can spawn stale (verify base at step zero) · build frontend dist in-worktree,
never copy · verify-frontend never typechecks — production build broke invisibly for two
landings (#616) · daemon reply shapes are shared with the CLI — key variant splits on
`device_id`, never `correlation_id` · read-side tolerance can silently mutate what the
write side guaranteed · the `last_seen` map was wrong twice (4 refresh sites, not 2/3);
the mandated independent terrain re-count then produced the mission's first fully-correct
map (9/9 sites, L5).

## Budget reconciliation

No `--budget` set; burn criteria off throughout. 11 landed links + 1 extension link,
~17 follow-up tickets, 5 councils, 12 reviews/reworks — all inside one session plus one
freeze/thaw cycle.

## Residue (open, tracked)

- **#616 (HIGH)** tsc sub-gate for verify-frontend (+ e2e outside tsconfig; satisfies-check).
- **#608 (HIGH)** dropdown-only project picker (dissent-flagged PO-hint match) · #609 TUI
  parity for agent_spawned · #610 preflight hooks hint · #606 dup-id visibility.
- **#604** two-stage admission (reserved interactive floor) · #605 shed-storm latched alert.
- **#611** pidfile flake (3 occurrences, root-caused) · #612 ingest invalid-JSON writer
  (high) · #613 archive drops agent_type · #614 missing agent_type→claude_code · #615
  clippy --all-targets drift.
- **#617** respawn log level · #618 resize dims persistence · **#619** respawn/pin
  observability (web_audit unreadable, refusals unaudited, failed respawn invisible).
- **#620** emergency-stop split + stop-outcome audit trail (carries B-A10 ratification) ·
  **#621** natural-exit orphaned grandchildren.
- Human smoke items (post-`./install.sh`, operator-only): pinned-respawn on the real HOME;
  whole-tree stop — added to `docs/smoke/CHECKLIST.md`.

## Flag-flip review

Nothing to flip. 602's and 588's user-visible work went live at their terminal links by
design; 603's enforcement changes carry inherent dormancy — they activate only when the
operator installs the new binaries (`./install.sh` is operator-only per the 2026-07-15
incident rule). The smoke-checklist items above are the activation follow-through.
