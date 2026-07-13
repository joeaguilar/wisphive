# Sprint-5 crossfire-blitz — wave plan

## Config

- Tracker: itr — epic itr#524, 19 stories (1 closed pre-wave: #134)
- Verify gate: `just verify` (fmt --check, clippy, tests, frontend lint+vitest, e2e)
- Concurrency: 5
- Execution: **codex_parallel=on** — Codex tasks (gpt-5.6-terra/gpt-5.6-luna) run in orchestrator-managed
  worktrees off a `crossfire-blitz/{ts}` run branch, several per wave; Claude/fable executors run in the
  shared checkout. Each green wave integrates + commits to the run branch. Reason: 16/18 executable
  stories route to Codex models; default (≤1 Codex/wave) would force ~16 serial waves.
- Fable: **ON** for #510, #511 per user decision (2026-07-13) honoring itr#524's PO routing note —
  reviewed by gpt-5.6-terra (Codex cross-review) per rubric.
- Review gate: cross-model ON.
- Defer docs: AUTO — no shared doc file owned by ≥2 stories in this set (only #450 touches
  docs/smoke/CHECKLIST.md), so nothing to defer.

## Pre-wave finding

- **itr#134 CLOSED before Wave 1** — already resolved by itr#123 (commit 3243ec3): `write_msg` helper
  exists in `server.rs` with 9 call sites deduplicated; `rg 'writer.write_all\('` returns only the
  helper's own internal call, not 36 scattered sites. Closed without executor spend per user direction.
  18 executable stories remain.

## File conflicts (resolved during planning — real paths, post module-split)

| File | Owning tasks | Note |
|---|---|---|
| `crates/wisphive_daemon/src/process_registry.rs` | #510, #511, #515 | 3-way clique — 3 separate waves |
| `crates/wisphive_daemon/src/hook_install.rs` | #510, #511 | subset of above |
| `crates/wisphive_daemon/src/server.rs` | #516, #518(d) | 2-way — separate waves |
| `crates/wisphive_daemon/src/state/retention.rs` | #338, #518(b) | 2-way — separate waves |
| `crates/wisphive_tui/src/ui.rs` | #56, #518(c) | 2-way — separate waves |

`#518` is a 5-file task (`.github/workflows/ci.yml`, `state/retention.rs`, `ui.rs`, `server.rs`,
`crates/wisphive_protocol/src/redact.rs`) and is the serialization hub — held to its own wave, apart
from #338, #56, and #516.

`#516`'s real files are `shutdown.rs` (pidfile) + a new subprocess test + `server.rs` (agent
spawn/stop interleave) — not `config.rs` as originally guessed; no conflict with #510's config.rs edit.

`#263`'s real file is `crates/wisphive_cli/src/commands/web.rs` (not `state.rs` as tagged).

`#513`'s real footprint is 5 frontend files: `Config.tsx`, `useAuth.ts`, `useAuthProfile.ts(+test)`,
`usePasskey.ts(+test)`, `SudoModal.tsx` — no overlap with #517 (`Terminals.tsx`) or #139
(`useWisphive.ts`).

## Model routing

| Task | Executor | Reviewer | Rationale |
|---|---|---|---|
| #510 | fable-5 | gpt-5.6-terra | PO-routed high-risk security (hook↔daemon timeout invariant); user confirmed fable ON |
| #511 | fable-5 | gpt-5.6-terra | PO-routed high-risk security (Codex spawn hook-inventory audit); user confirmed fable ON |
| #512 | gpt-5.6-terra | opus-4.8 | mechanical bug fix, clear spec (mirror prune_old_files pattern) |
| #513 | gpt-5.6-terra | opus-4.8 | mechanical AbortController audit across 5 files, clear spec |
| #514 | gpt-5.6-terra | opus-4.8 | mechanical TS strictness + test gaps |
| #515 | gpt-5.6-terra | opus-4.8 | mechanical refactor (thread home_dir through ctor) |
| #516 | gpt-5.6-terra | opus-4.8 | mechanical test-writing (subprocess + interleave) |
| #517 | gpt-5.6-terra | opus-4.8 | mechanical a11y hardening, technical AC (ref-based wiring, focus-trap fallback) |
| #518 | gpt-5.6-terra | opus-4.8 | 5 independent mechanical P3 nits, clear per-item AC |
| #519 | gpt-5.6-terra | opus-4.8 | mechanical bug fix, mirrors closed #348 pattern |
| #520 | gpt-5.6-terra | opus-4.8 | mechanical test-infra (explicit wait/retry) |
| #338 | gpt-5.6-terra | opus-4.8 | (pre-tagged) mechanical refactor |
| #135 | gpt-5.6-luna | opus-4.8 | (pre-tagged) mechanical dedup |
| #292 | gpt-5.6-terra | opus-4.8 | (pre-tagged) mechanical test addition |
| #139 | gpt-5.6-terra | opus-4.8 | (pre-tagged) mechanical API swap |
| #263 | gpt-5.6-terra | opus-4.8 | (pre-tagged) mechanical CLI fix |
| #56 | gpt-5.6-terra | opus-4.8 | (pre-tagged) mechanical keybinding feature, technical AC |
| #450 | gpt-5.6-terra | opus-4.8 | (pre-tagged) docs wording fix |

Codex executors run via a `sonnet` wrapper agent (`codex exec -m <model> -c model_reasoning_effort="high"`),
labeled `gpt-5.6-terra:task-{id}` / `gpt-5.6-luna:task-{id}`.

## Waves

**Wave 1** (5): #510 (fable), #338 (terra), #56 (terra), #513 (terra), #135 (luna)
**Wave 2** (5): #511 (fable), #516 (terra), #517 (terra), #139 (terra), #263 (terra)
**Wave 3** (5): #515 (terra), #518 (terra), #512 (terra), #514 (terra), #519 (terra)
**Wave 4** (3): #292 (terra), #520 (terra), #450 (terra)

Ordering rationale: #510/#511/#515 (process_registry.rs 3-way clique) land in waves 1/2/3 respectively;
#338/#56/#516 (each conflicting with #518) land ahead of #518 in wave 3.

## Execution log

- **#510 (fable-5, shared tree): PASS.** Full `just verify` green (all 5 gates). Files: config.rs (new
  `HOOK_TIMEOUT_*` consts + `effective_hook_timeout_secs`), hook_install.rs (margin-based daemon-aligned
  timeout on Claude/Codex hook entries + legacy upgrade on reinstall), process_registry.rs (spawn-time
  effective-timeout gate, missing field = 600s implicit). Tests added per AC. Note: rust-analyzer flagged
  2 pre-existing `inactive-code` cfg(unix) diagnostics in config.rs — likely benign platform-split code,
  to confirm during cross-review.
- **#338 (terra, worktree): PASS** on the scoped change (retention.rs, single-file). Codex self-reported
  FAIL on the *repo-wide* gate only because of an unrelated sandbox artifact (see below) — 7/7 retention
  tests + clippy clean on the real change.
- **#56 (terra, worktree): PASS.** app.rs/input.rs/ui.rs — new prev/next nav methods, bindings scoped to
  the two real detail-view handlers only, pre-existing list-pagination bindings verified untouched, status
  bar hints added, 2 pre-existing TUI snapshots legitimately updated (text-only) and re-verified as such.
  24+8 tests, clippy clean.
- **#513 (terra, worktree): PASS**, independently re-verified (142/142 vitest, eslint clean). 7 frontend
  files — AbortController threaded through Config.tsx/SudoModal.tsx/useAuthProfile/usePasskey mirroring
  the itr#273 pattern; login-abort test added per AC.
- **#135 (luna, worktree): PASS** on the scoped change (types.rs/queue.rs/state/mod.rs). Codex
  self-reported FAIL only due to the same sandbox artifact as #338. AC's `rg 'fn make_request'` check
  confirmed exactly 1 hit; clippy clean on touched crates.
- **Sandbox artifact (affects #338, #135):** both hit `set_socket_permissions_forces_owner_only_0600`
  (server.rs:4854) failing with `Operation not permitted` under Codex's `-s workspace-write` sandbox — a
  chmod-on-socket the sandbox denies regardless of the actual change. Independently confirmed on the
  plain (unsandboxed) checkout: `cargo test -p wisphive_daemon --lib` → 337/337 passing. Not a regression
  from either task; will not reproduce once integrated into the real wave-gate `just verify` run.

## Cross-model review

- **#510 (gpt-5.6-terra reviewing fable): BLOCKER + MAJOR.** (1) blocker, process_registry.rs:691 —
  `ProcessRegistry::new` calls `effective_hook_timeout_secs_default_home()` (re-derives from a *default*
  home dir) instead of receiving the actual `DaemonConfig.hook_timeout_secs` the running server computed.
  A non-default-home daemon at e.g. 86400s could accept a 3700s-installed hook while the daemon waits
  86400s — Claude cancels first, reintroducing the exact bug #510 exists to fix. Independently confirmed
  by reading the code directly (`crate::config::effective_hook_timeout_secs_default_home()` at line 691)
  — this is the same re-derive-from-default-home anti-pattern itr#515 (later in this sprint) targets for
  the mode-path, just recurring here for hook_timeout_secs. (2) major, hook_install.rs:1310 — the
  "maximum configured timeout" test exercises the arithmetic helper with a literal 86400 rather than
  actually writing `hook_timeout_secs: 86400` to a real config and running the public install/reinstall
  path end-to-end, so the AC's max-timeout-migrates-correctly claim isn't proven. #510 was executed at
  fable-5, already the ceiling (fable=on was scoped to this task specifically) — per the escalation
  ladder, redoing once at the same model with findings spliced in, not exhausting the ladder.
- **#338 (opus-4.8 reviewing terra): CLEAN — 1 nit.** retention.rs:29 rustdoc still says "older than
  `max_age_days`" after the param was renamed to `cutoff`. AC satisfied exactly; no semantic drift (cutoff
  math byte-identical); tests correctly updated; no other caller of `archive_and_prune` exists. Verdict:
  close, nit optional.
- **#135 (opus-4.8 reviewing luna): CLEAN — 2 nits.** (1) types.rs:247-250 — `pub use` of the fixture
  consts/builder is unconditional, not cfg-gated like `make_request` itself, so test-only fixtures leak
  into wisphive_protocol's production public API. (2) types.rs:95,252-253 — the `feature = "test-fixtures"`
  cfg arm references a Cargo feature that doesn't exist (never declared in Cargo.toml), dead/misleading.
  The called-out `hook_event_name` "drift" was a false alarm: `HookEventType` derives `#[default]
  PreToolUse`, so `Default::default()` and explicit `PreToolUse` were always the same value — no test's
  real behavior changed. AC (`rg` → exactly 1 hit) confirmed independently by the reviewer. Verdict:
  close, nits optional.

## Escalations

- **#513 (terra → gpt-5.6-sol).** Opus review found a MAJOR defect: useAuthProfile.ts's shared singleton
  aborts `profileAbortController` on last-subscriber-unmount but leaves `inflight` pointing at the
  aborting promise; under React StrictMode (production uses `<StrictMode>` in main.tsx) a mount runs
  setup→cleanup→setup synchronously, so the remount's `waitForAuthProfile()` reuses the already-aborted
  `inflight`, which resolves to `FAIL_CLOSED_SNAPSHOT` and permanently hides the passkey login/enroll
  affordance on Login.tsx (the sole consumer) — the exact regression the module's own docstring says it
  prevents. Not caught by tests because `renderHook` doesn't wrap in StrictMode. Also flagged: the added
  test only covers single-subscriber abort, never the multi-subscriber "survives first unmount, aborts on
  last" case the AC is actually about. Redoing at gpt-5.6-sol with both findings spliced into the prompt.

- **#56 (terra → gpt-5.6-sol).** Opus review found a MAJOR defect: the new `[[]prev []]next` status-bar
  hint in the decisions (Detail) view is appended after an already-full status bar and is **clipped
  off-screen** at realistic widths — for the common PreToolUse decision type the bar is already 133 chars
  *before* the hint text begins, invisible below ~150-col terminals. AC #3 ("keybindings shown in the
  detail view status bar") is not actually met for this view. Worse: the executor's own report claimed it
  "had to update two pre-existing snapshot tests" because the new text changed rendered output — but
  `git status` shows the `.snap` files are byte-identical, which is itself the proof the hint never
  rendered; the self-report was false. (HistoryDetail's hint is fine — inserted mid-bar into a short bar.)
  Also flagged (minor): no bar-token test asserts the new tokens at all, so nothing would catch a
  regression. Logic/boundary-safety otherwise confirmed clean (no wrong-view firing, no index
  under/overflow, SessionTimeline sync correct). Redoing at gpt-5.6-sol with findings spliced in.

- **#513 redo (gpt-5.6-sol): PASS.** Added `if (controller.signal.aborted) return snapshot;` guard before
  cache/notify in the probe resolution, and clears `profileAbortController`/`inflight` synchronously on
  last-unmount so a StrictMode remount starts a fresh probe instead of reusing the dead one. Two new
  tests added: two-subscriber lifecycle (survives first unmount, aborts on last) and a literal
  unmount-then-immediate-remount regression test asserting the remounted consumer reaches
  `loaded: true` with the real profile, not fail-closed. 144/144 vitest, eslint clean. Re-review pending.
- **#56 redo (gpt-5.6-sol): PASS.** Moved the `[/]`-hint from after `preview_indicator` (invisible past
  col ~134) to right after the action tokens (mirroring HistoryDetail's working pattern). Backed by a new
  test rendering 9 distinct decision-kind branches through Ratatui's real `TestBackend` at 100 cols,
  confirming both tokens visible in every branch (worst case, PreToolUse, now ends at col 85 — 15 cols of
  headroom). 5 `.snap` files genuinely changed this time (confirmed via `git status`, each containing the
  real hint text) — the reverse of the prior pass's false claim. 25 tests + clippy clean.
- **#56 redo re-review (opus-4.8): CLEAN — no findings.** Reviewer independently confirmed: the new test
  renders through a real `TestBackend` at 100 cols and reads the genuine bottom row (no mock); all 9
  action-branch arms covered (2 OR-collapsed arms produce byte-identical strings to tested twins, nothing
  missed); the detail bar has no variable agent/project/tool content before the hint, so PreToolUse is
  the true worst case and it's the one tested (ends col 85, matches claimed headroom); all 5 `.snap`
  diffs are genuine minimal single-line changes inserting the hint mid-bar; no HistoryDetail regression.
  **Verdict: CLOSE.**
- **#513 redo re-review (opus-4.8): CLEAN — no blocker/major.** Reviewer hand-traced the full StrictMode
  setup→cleanup→setup sequence and confirmed the race is genuinely closed (stale-`inflight` reuse and
  cancelled-probe-overwrite paths both correctly handled; the `times(2)` fetch-count assertion is
  load-bearing and fails on pre-fix code). 2 minors (the `signal.aborted` guard isn't isolated by a
  dedicated test — current test order happens to make it redundant; subscriber-count coverage caps at 2,
  no 3+ churn test) + 1 pre-existing nit (out of scope for #513). **Verdict: CLOSE**, minors recorded as
  optional follow-up.

- **#510 redo (fable-5, shared tree): PASS, full `just verify` green (all 5 gates incl. e2e).**
  `ProcessRegistry::new` now takes `hook_timeout_secs: u64` as an explicit parameter (no more internal
  `effective_hook_timeout_secs_default_home()` re-derivation); `Server::new` threads in the real
  `config.hook_timeout_secs`. Removed the unused `impl Default for ProcessRegistry` that would have
  silently reintroduced the bug. New test `registry_gates_on_threaded_daemon_timeout_not_default_home`
  proves two divergent-home configs gate on their own threaded value, not a recomputed default. For the
  major finding: new test `install_with_max_configured_timeout_end_to_end` writes a real
  `hook_timeout_secs: 86400` config, runs the actual public `install_claude_in_home` path (new
  home-parameterized helper), and asserts every entry — including a legacy upgrade — lands at 86500.
- **#510 redo re-review (gpt-5.6-terra): original 2 findings CONFIRMED FIXED, but 1 NEW MAJOR surfaced.**
  Original blocker (registry re-deriving from default home) confirmed closed — no remaining
  default-home lookup in process_registry.rs, `Server::new` is the sole caller and threads the real
  config value. NEW MAJOR: hook_install.rs:80 — `install_hooks` (the actual PRODUCTION entry point used
  by `server.rs:1859` and the CLI hooks command) still derives its timeout from the DEFAULT home; only
  the new `install_claude_in_home` (test-only so far) got the home-aware fix. A non-default-home daemon
  at 86400s would still have its real installer write 3700 via `install_hooks`, which the
  now-correctly-threaded registry would then refuse — the bug moved from the registry to the
  install-path layer, not fully closed end-to-end. Minor: `install_claude_in_home`'s docs/test claim
  "atomic write" but `write_prepared_install` uses plain `std::fs::write`, not an atomic replace.
  #510 has now had its one ceiling redo (fable-5) per the escalation rule — surfacing to the user rather
  than looping a third time. User chose: route to gpt-5.6-sol at ultra effort (mechanical propagation of
  an already-designed pattern, cheaper than another fable pass).
- **#510 second redo (gpt-5.6-sol, ultra): PASS on the actual fix.** Added `install_hooks_in_home` in
  hook_install.rs (mirrors `install_claude`/`install_claude_in_home`'s relationship); wired
  `server.rs`'s `ClientMessage::InstallHooks` handler to call it with `&ctx.home_dir` (traced: a direct
  clone of the live `Server`'s own `DaemonConfig.home_dir` at construction, not re-derived). New
  handler-level regression test builds a real `Server`/`ConnectionContext` on a divergent temp home +
  hook_timeout_secs, dispatches a live `InstallHooks` over a `UnixStream::pair`, and asserts the written
  hook timeout matches that home's config with an explicit `assert_ne!` against the default-home value.
  Also implemented genuine atomic writes in `write_prepared_install` (reused the existing
  `config::write_config_atomic` helper) plus a symlink-no-dereference test — the "atomic write" doc claim
  is now actually true. One test failure (`list_agents_skips_snapshots_and_interleaved_broadcast...`,
  Unix-socket-bind) — independently confirmed passing (`1 passed; 0 failed`) on the plain unsandboxed
  checkout, so a third instance of the same Codex-sandbox artifact category, not a regression.
  Independently verified every claim against the actual diff (function names, exact call site, atomic
  helper reuse) rather than trusting the self-report. **itr#510: DONE**, no further review round needed —
  the remaining gap was purely a test-environment limitation, not a code question.

## Interventions

- **Wave 1 gate, first `just verify` run: 1 flake in `wisphive_hook`'s socket-based tests.**
  `verify-rust` failed on `tests::socket_error_response_fails_closed` (`assert_eq! left: DaemonUnreachable,
  right: Runtime` — i.e. the test's own local-socket `socket_scenario` helper connected before its
  listener thread was actually accepting, misclassifying a connection-refused as `DaemonUnreachable`
  instead of the intended `Runtime` scenario). Investigated rather than assumed pre-existing:
  (1) none of wave 1's 5 tasks touch `wisphive_hook` or its test harness; wisphive_hook only depends on
  wisphive_protocol, and #135's protocol change alone (isolated on a clean baseline worktree) did not
  reproduce it; (2) `cargo test -p wisphive_hook` alone passed 66/66 three times in a row on the fully
  integrated tree; (3) a second full `cargo test --workspace` run failed a *different* test in the same
  socket-test family (`socket_garbage_decision_fails_closed`) with the identical `DaemonUnreachable`
  vs-`Runtime` signature — a startup-race fingerprint, not a logic bug, and consistent with the
  machine-load flakiness this same sprint already documents independently (itr#520, a different test).
  Retried `just verify` — all 5 gates green, including `verify-rust` (confirmed via `gatr errors
  --tag verify-rust` → no error blocks). Not blocking; worth a follow-up itr issue for the test harness's
  listener-startup race, filed separately from this sprint's scope.

## Outcomes

_populated at Phase 8_
