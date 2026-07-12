# Sprint-4 crossfire-blitz — tail (32 remaining stories)

Second-pass model-routed blitz over the 33 stories left open after the Codex lead run
closed 26 (commits `2102fb7`..`a3279b0` on `main`). This run uses `codex_parallel=on`.
(#96 re-decisioned back in via ADR-0008 → 32 tail + #96 = 33.)

## Config

- Mode: `codex_parallel=on` — Codex tasks in orchestrator-made git worktrees, committed per green wave.
- Run branch: `crossfire-blitz/20260712T020836Z` (base `a3279b0`; never `main`, never pushed).
- Executor: **gpt-5.6-terra** for all 33 (none clear the taste bar; all mechanical/clear-AC robustness or prescriptive hand-off-able specs). Effort `ultra` for risk:high, `high` otherwise.
- Reviewer: **opus-4.8** for all 33 (cross-model; a different model than executed). Satisfies the plan's per-lane + whole-diff `/crossfire-review` checkpoints.
- Fable: OFF. No task requested it; escalation ladder tops at opus (then recommend fable/gpt-5.6-sol@ultra if a ceiling task still fails).
- Verify gate (per wave): `cargo fmt --check && cargo clippy --workspace -D warnings && cargo test --workspace`; Lane-E-touching waves also `frontend-lint` + `frontend-test`. `just e2e` once at the whole-diff review.
- Concurrency: 5. Gating: `~/.wisphive/mode=active`, `auto_approve_level=all` → workers auto-approve at hook layer (no daemon queue).
- Excluded: **#509** (Codex blitz epic); **#510/#511** (Codex follow-ups, not #508 children). (#96 re-included via ADR-0008.)
- Dirty-tree: never stage the 9 sprint-2 evidence PNGs or `sprint/CURRENT`.

## Waves (routing: all executor=gpt-5.6-terra, reviewer=opus-4.8)

| Wave | Stories (effort) | Hot-file rationale |
|------|------------------|--------------------|
| 1 | #410(ultra) #472(ultra) #91(high) #348(high) #118(high) | server.rs, process_registry, state.rs, cli/main.rs, fe/useWisphive — all disjoint |
| 2 | #101(high) #470(high) #137(high) #371(high) #273(high) | server.rs, agent+protocol, state.rs, cli/main.rs, fe/api+useAuth |
| 3 | #97(high) #281(ultra) #318(high) #378(high) #265(high) | server.rs, web/lib+state, cli/main+auth_profile, fe/App, history.rs |
| 4 | #336(high) #495(ultra) #372(high) #482(med→high) #373(high) | server.rs, web/lib+config, cli/daemon+shutdown, fe/TerminalView, tui/app |
| 5 | #409(high) #504(ultra) #374(high) #488(high) #261(ultra) | server.rs+modal, web/lib+auth+web_auth, tui/input+ui, fe/Terminals, cli/web |
| 6 | #365(high) #408(high) #505(high) #377(high) #248(high) | terminal+server.rs, web/lib, fe/Login, tui/input, ci.yml |
| 7 | #102(ultra) #277(high) | hook/main+server.rs, web/lib+cli/main+cli/daemon |
| 8 | #96(ultra, **solo**) | server.rs+config+notify+wire+tui(ui/app)+fe(useWisphive/App/protocol) — collides with all 7 waves → lands last on top of everything. Story C likely already done by #407 → executor MUST reconcile, not redo. |

Serialized hot files (each forks from prior wave's commit): `daemon/server.rs` W1→W7 · `web/src/lib.rs` W3→W7 · `daemon/state.rs` W1→W3 · `cli/main.rs` W1→W7 · `tui/input.rs` W5→W6 · `fe/useWisphive.ts` W1→W2.

## File conflicts (resolved by wave separation)

- `daemon/src/server.rs`: 410,101,97,336,409,365,102 — 7-clique + mutual neighbors → 7 waves.
- `web/src/lib.rs`: 504,495,281,408,277 — 5-clique.
- `cli/src/main.rs`: 348,371,318,277.
- `daemon/src/state.rs`: 91,137 (both `archive_rows_by_ids`) + 281 (new method/neighbor).
- `tui/src/input.rs`: 374,377. `fe/hooks/useWisphive.ts`: 118,273. `daemon`↔`web` neighbor 504↔281↔505.

## Semantic warnings

Under codex_parallel, worktrees are blind to wave-mates — every neighbor pair above is split across waves (verified: no within-wave neighbor pairs). #470 edits `protocol/src/lib.rs` (wire type) — reviewer must confirm behavior-preserving on the public protocol.

## Cross-model review

### Wave 1 (executor gpt-5.6-terra → reviewer opus-4.8)
- **#410 → SHIP.** Gate byte-identical to single-Approve; fails closed on unknown device; no strand/double-resolve; regression genuinely fails pre-fix. Nits only: pre-existing `eager_persist`-without-`resolved`-guard (not introduced by diff, itr#363-adjacent); optional fresh/TUI-origin test coverage.
- **#348 → SHIP.** Genuine `Option<u16>` fix (not another no-op); no `None` reaches a bind/banner site; `web serve` untouched; tests hit the real failure mode via the production predicate. FOLLOW-UP: sibling footgun — explicit `--host 127.0.0.1` still doesn't enable web (pre-existing, out of scope) → file crossfire-review/sprint-4-followup.
- **#472 → SHIP.** Fail-secure (same descriptor-based secure mode-read as hook; read-error → refuse); TOCTOU narrowed by pre-`spawn()` recheck; test proves refusal-before-hook-validation + empty registry. FOLLOW-UP (minor): `default_mode_path()` re-derives `$HOME/.wisphive/mode` from env — a 3rd path derivation; thread `config.home_dir` into `ProcessRegistry::new` (diverges only under a custom-home override) → file crossfire-review/sprint-4-followup.
- **#118 → SHIP.** Hard AC met; blank-`VITE_WS_URL` fatal-at-startup genuinely wired; throws at module load, omitted→origin fallback preserved. FOLLOW-UPS (P2, file don't expand scope): F1 `api.ts` read site still reads `import.meta.env` directly (`environment.apiUrl` dead — but api.ts is #273's Wave-2 file); F2 `vite-env.d.ts` lacks `ViteTypeOptions.strictImportMetaEnv` so typos aren't TS errors; F3 test omits the omitted-accepted + blank-`VITE_API_URL` cases → file crossfire-review/sprint-4-followup.
- **#91 → HOLD → escalated to sol** (see Escalations). Security core clean; 2 MEDIUM defects (substring regression, retention format!).

### Wave 4 — commit f1c630d — 4/4 closed (#336 deferred). Gate: rust + frontend all green.
- **#372** closed — drop(pid_guard) before process::exit; stale/unparseable PID→no-daemon, can't nuke a live pidfile (terra; opus SHIP). Follow-up P2: subprocess start→stop integration test.
- **#482** closed — terminal touch ignores pre-paint moves (no 17px guess); all 6 prior touch tests retained (terra; opus SHIP).
- **#495** closed — reconciled: #407 already serializes put_config; added 8-way concurrent-PUT regression (terra ultra; opus SHIP TRUE reconciliation).
- **#373** closed — TUI config-save errors surfaced to status bar (app+cli/tui+ui.rs); panic already gone via #407 (terra; opus SHIP; 3-file scope after 2 correct refusals).

### Deferred to tail solo waves (files collide with the `server.rs`/`cli-daemon.rs` serializers)
- **#470** → re-scoped `protocol/src/wire.rs` + `daemon/src/server.rs` + `cli/commands/agent.rs`.
- **#336** (W4 defer) → needs `cli/commands/daemon.rs` (prune-ordering, collides #372) + `server.rs`; re-scoped `event_ingest.rs` + `server.rs` + `cli/commands/daemon.rs`. Worker correctly refused.
- Tail plan: run #96 (W8), then #336, #470 as sequential solo waves (each forks from prior commit → clean server.rs serialization).
- **#373** (W4) re-scoped mid-wave: #407 moved TUI save to daemon's safe `update_config_json`; panic gone, remaining work = surface the error to the TUI status bar → owns `tui/app.rs` + `cli/commands/tui.rs` (disjoint from W4 peers). Re-run in-wave.

### Wave 5 — commit 9eb329a — 5/5 closed. Gate: rust + frontend all green.
- **#504** closed — rehash-on-verify (login/reauth/device-revoke) via fail-safe CAS (terra ultra; opus security review **NO FINDINGS**). Flagship security carryover.
- **#409** closed — daemon→resolving-TUI persist-failure feedback, decision outcome preserved (terra; opus SHIP; re-scoped to server.rs+cli/tui.rs reusing #373).
- **#374** closed — detail-view `G` clamp via `Cell<u16>`, no wrap/trap (terra; opus SHIP; re-scoped to app+input+ui, no signature change).
- **#488** closed — mobile terminal dialog semantics + reversible inert occlusion (terra; opus SHIP; 3 P3 polish).
- **#261** closed — zeroize plaintext on all 7 paths (terra; opus SHIP).

### Wave 6 — commit ee948ed — 5/5 closed. Gate: rust + frontend all green.
- **#365** closed — remove ended terminal sessions from live map (PTY fd leak) + reject input/resize (terra; opus SHIP; ownership audit confirms real drop).
- **#408** closed — nested-null merge-patch deletes one tool_rules entry, siblings survive (terra; opus SHIP; locked path, no abuse).
- **#505** closed — relax Login native minLength so custom below-floor copy renders (terra; opus SHIP; JS floor still enforced).
- **#377** closed — spawn modal sends full multi-line prompt (terra; opus SHIP).
- **#248** closed — CI cargo-deny/audit + format! DML deny-list gate (completes #91 invariant). opus **HOLD** on the grep (over-escaped→vacuous, missed `{}`) → **orchestrator corrected the regex inline** + verified it flags interpolated DML, ignores logs, passes current tree. Follow-up: rg exit-2 handling.

### Wave 7 — commit 3d97c87 — 2/2 closed. Gate: rust green.
- **#102** closed — O_EXCL hook marker (no TOCTOU), EEXIST idempotent, exactly-one AgentConnected, orphan cleanup (terra ultra; opus SHIP; symlink handling net-safer than old fs::write).
- **#277** closed — readiness oneshot after TCP bind replaces 400ms sleep; serve() signature preserved (terra; opus SHIP; dropped-sender no-hang).

### Wave 8 — commit 652929b — #96 closed (solo, ultra). Gate: full workspace + frontend green.
- **#96** closed — ADR-0008 config tamper-evidence, 3 sub-stories in ~1400 LOC/27 files: A `fs_trust` trusted-read (opus security review **NO FINDINGS**), C reconciled vs #407 (8-thread test, no 2nd lock), B `config_watch` widening alerts + `ConfigAlert` wire + TUI/web banners + SQLite restart baseline. 3-angle opus review all SHIP; **P2 fixed inline** (untrusted banner re-asserted as a level on restart — was lost as an edge) + **P3** (aria-live=assertive for untrusted). 25 named tests.

### Wave 9 — commit 34bcc2b — #336 closed (solo). Gate green.
- **#336** closed — startup re-ingest of orphaned/failed event segments before prune (structurally enforced in Server::new); idempotent dedup; no data loss (terra; opus SHIP). Follow-up #512 (read_dir dirent error fatal to startup).

### Wave 10 — commit 9dda782 — #470 closed (final, solo). Gate green.
- **#470** closed — additive correlation_id disambiguates agent command replies from concurrent broadcasts; strictly backward-decodable; #294 drain preserved (terra; opus SHIP). Follow-ups #516 (spawn/stop interleave tests, dead arm).

## FINAL — sprint-4 tail complete (33/33 stories closed across 10 waves)
Run branch `crossfire-blitz/20260712T020836Z`: 10 commits `b58b6db`→`9dda782` (never pushed; never on main).
- **Executors:** all gpt-5.6-terra (1 sol escalation on #91). **Reviewers:** opus-4.8 (every story, cross-model). No fable spent (fable=off).
- **Crossfire caught (green self-report + green gate, but wrong):** #91 substring→whole-token search regression (sol-fixed); #348 no-op refactor (redone); #318 deleted a live test (restored inline); #248 vacuous CI regex (corrected inline); #96 untrusted-banner-lost-on-restart P2 (fixed inline). Plus many worker OUT-OF-SCOPE refusals that prevented wave collisions (planner under-scoped `server.rs`, `cli/commands/{tui,daemon,agent}.rs`, `protocol/wire.rs`, `state/*` submodules).
- **Deferrals:** #470, #336 → tail solo waves (both needed `server.rs` + a `cli` file that every wave serializes).
- **Delegated invariants completed across stories:** #91's "no format! SQL" → #137 (archive) + #248 (CI grep); #118-F1 → #273.
- **Follow-ups filed:** #512–#520 (tag `crossfire-review`+`sprint-4-followup`) for /sprint-review triage.

### Whole-diff e2e checkpoint (commit 5fadaba) — 11/11 green
`just e2e` initially showed 4 consistent failures (queue approve/deny, card-badge, inbox — all hook-fixture-over-socket tests; web-only tests passed). **Fable diagnosis: NOT a crossfire-tail regression.** The failures traced to two main-side hardening gates (Codex run 2026-07-11): `is_valid_hook_agent_id` (#86, commit 3851024) rejected the fixture's `e2e-hook-fixture` prefix, and `hook_decision_mode_denial` (#95, commit b6a1551) required an active 0600 mode file the fixtures never wrote. On main these were MASKED — the daemon fixture died at boot 5/5 (SQLite-preflight race, the "(0ms)" flake); **this branch fixed that boot race**, unmasking the (correct) rejections as 15s timeouts. Fix: e2e fixtures only (active mode file + `cc-` agent-id prefix), **zero product-code change** — the Rust gates are correct. Result: `just e2e` 11/11, `cargo test --workspace` 12/12. Residual: inbox auto-answered-count assertion flakes under concurrent machine load (timing, not a defect) → #520.

### Run branch: crossfire-blitz/20260712T020836Z — 11 commits, 71 files, +4881/-572. Full gate (fmt/clippy/test/frontend/e2e) GREEN. Never pushed; never on main. User to squash/merge/discard.

### Recurring planner scope-gap (workers correctly refuse, orchestrator re-scopes)
The planner's coarse file lists miss cross-crate WIRING files. Corrections mid-wave:
- **#409** (W5): TUI-feedback needs `cli/commands/tui.rs` (receive loop routes daemon `ServerMessage::Error`) → re-scoped `daemon/server.rs` + `cli/commands/tui.rs`, reusing #373's `app.status_error`+`ui.rs`.
- **#374** (W5): render-time clamp can't take `&mut App` (ripples to unowned callers) → re-scoped to a `Cell<u16>` clamp across `app.rs`+`input.rs`+`ui.rs`.
- Pattern seen also at #470, #336, #373: `server.rs` (7-way serializer), `cli/commands/{tui,daemon}.rs`, and `protocol/src/wire.rs` are the load-bearing wiring files the planner under-scoped.

### Scope carried into later waves (from W1 reviews)
- **#137 (Wave 2):** + convert `archive_rows_by_ids` `format!` SELECT/DELETE → QueryBuilder (from #91 review).
- **#248 (later):** + add CI `format!.*SELECT|INSERT|UPDATE|DELETE` deny-list grep (from #91 review).

## Escalations

- **#91 terra → gpt-5.6-sol (ultra).** Opus review HOLD on 2 MEDIUM defects: (1) substring→whole-token FTS regression breaks per-keystroke type-ahead, violating AC's "substring search"; (2) `archive_rows_by_ids` `format!` SELECT/DELETE still present. Redo on sol: FTS5 **trigram tokenizer** to preserve substring + <50ms; fix empty-query→match-all and per-boot backfill. Security core (bound values, sync triggers) was correct and preserved. Re-review after.
  - **Delegated (AC spans >1 story's files):** `archive_rows_by_ids` format!→QueryBuilder → **#137** (owns retention.rs, Wave 2). CI `format!.*SELECT|INSERT|UPDATE|DELETE` deny-list grep → **#248** (owns ci.yml). #91's daemon-wide "no format! SELECT" invariant completes across #91+#137+#248 within the sprint.

## Interventions

- **W1 path correction (planner error):** planner resolved `wisphive_daemon/src/state.rs`, but `state` is a directory module. #91 correctly refused rather than edit out-of-scope. Corrected ownership: #91→`state/{decisions,decisions_tests,migrate,mod}.rs`; #137→`state/retention.rs`; #281→`state/{web_auth,web_auth_tests}.rs`. These are DIFFERENT submodules → no mutual conflict (wave assignment unchanged). #91 re-run.
- **Report-write sandbox fix:** codex `workspace-write` rejects writes to `$SCRATCH/reports/` (`/private/tmp` ≠ its `/tmp` allowlist). Workers now write `./BLITZ_REPORT.md` in-worktree; the `RESULT:` line is also in each worker's stdout `.log`. `BLITZ_REPORT.md` is excluded from the integration patch.
- **Worktree build-infra fix:** fresh worktrees lack gitignored `frontend/dist/` (rust-embed needs it → any `wisphive_cli`/`wisphive_web` cargo build fails) and `frontend/node_modules/` (vitest missing; sandbox has no npm network). Both symlinked from the main tree into every worktree at creation (gitignored → absent from harvest). Daemon-only workers (#410/#472/#91) are unaffected — `wisphive_daemon` doesn't depend on `wisphive_web`.
- **W1 #348 no-op catch (crossfire value at harvest):** first attempt extracted the identical `port != 3100` expression into a helper with UNCHANGED logic + a test asserting `--web --port 3100` (which already worked) — did not fix the footgun. Re-run with precise guidance: detect explicitly-provided `--port` (Clap `Option<u16>`/`ValueSource`) so `--port 3100` enables web like any other explicit port; test must exercise `--port 3100` without `--web`. #118 accepted (code correct; vitest deferred to integration gate).

## Outcomes

### Wave 1 — commit b58b6db — 5/5 closed. Gate: clippy/test/fmt + frontend lint/vitest all green.
- **#410** closed — sudo-gate web ApprovePermission (terra; opus SHIP).
- **#472** closed — SpawnAgent kill-switch refuse (terra; opus SHIP). Follow-up: mode_path env-derivation.
- **#91** closed — search_history QueryBuilder + FTS5 trigram substring<50ms (terra→**sol** escalation; opus HOLD→SHIP). Delegated: archive→#137, CI grep→#248.
- **#348** closed — explicit `--port` enables web (terra, 1 no-op redo; opus SHIP). Follow-up: `--host` sibling footgun.
- **#118** closed — env module-load validation (terra; opus SHIP). Follow-ups: F1 api.ts read site (→#273), F2 strict ImportMetaEnv, F3 test gap.
- Infra lessons banked: `< /dev/null` stdin (hang fix), worktree `dist/`+`node_modules/` symlinks, worker RESULT headers unreliable under sandbox (judge by diff+focused tests+integration gate).

### Wave 2 — commit 9dee3e0 — 4/4 closed (5th, #470, deferred). Gate: rust + frontend all green.
- **#101** closed — per-peer Hello-mismatch rate-limit + log (terra; opus SHIP; bounded map verified). 
- **#137** closed — retention writer→tokio::fs + archive_rows_by_ids format!→QueryBuilder (terra; opus SHIP). Completes #91 invariant (CI grep still owed by #248). Follow-up P3: serde_json::to_writer/BufWriter batching.
- **#371** closed — malformed `--host` rejected via Ipv4Addr::parse (terra; opus SHIP).
- **#273** closed — AbortController threaded + api.ts consumes environment.apiUrl (#118-F1) (terra; opus SHIP). Follow-ups: weak login-abort test; Config.tsx loader still unthreaded (+audit useAuthProfile/usePasskey/SudoModal).
### Wave 3 — commit 7ddb3a5 — 5/5 closed. Gate: rust + frontend all green.
- **#97** closed — control-char/ANSI sanitize at every log+notify sink, boundary-only (terra; opus SHIP; grep-verified no raw sink). INFO: space-sub vs `\xHH`.
- **#281** closed — atomic set-password+device txn, rollback empirically verified (terra ultra; opus SHIP).
- **#318** closed — all 9 #310 code-quality items (terra; opus SHIP). **P2 coverage-regression FIXED INLINE**: worker deleted a live drift smoke test (replaced by ignored `todo!()` skeleton) → orchestrator restored it alongside the skeleton (additive-not-destructive). Restored test passes.
- **#378** closed — key `7`→onViewTerminals + help row (terra; opus SHIP).
- **#265** closed — char-aware history truncate (terra; opus SHIP; test hits exact 😀-boundary panic).

### Wave 2 addendum
- **#470 DEFERRED** → final solo wave. Planner mislabeled its files (`protocol/src/lib.rs` → actually `wire.rs`) and it needs `daemon/src/server.rs` to echo the correlation id / route a non-subscribed CLI client kind — which collides with the `server.rs` story in EVERY wave. Re-scoped to `protocol/src/wire.rs` + `daemon/src/server.rs` + `cli/commands/agent.rs`; runs last, forking from the final commit so all server.rs work is in its base. Worker correctly refused rather than edit out-of-scope.
