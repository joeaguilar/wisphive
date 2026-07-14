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

## Wave 2 execution log

- **#511 (fable-5, shared tree): PASS.** Full `just verify` green (all 5 gates). New
  `audit_codex_effective_hooks` in process_registry.rs inspects the full effective hook inventory (user
  hooks.json, inline `[hooks]` TOML in user+project config.toml, project file, plugin-bundled hooks,
  managed knobs, `features.hooks`, persisted `/hooks` state, `allow_managed_hooks_only`) rather than just
  the project file. Fails closed on any TOML layer it can't positively parse (no `toml` crate in
  ownership, so a purpose-built line scanner errs toward over-refusing). `features.hooks≠true` and
  persisted disablement are hard refusals the `codex_allow_foreign_hooks` opt-in cannot release. 15 new
  tests + a genuine runtime proof (spawns a real `codex exec`, confirms the Wisphive gate's events.jsonl
  recorded the auto-approved PreToolUse — env-gated behind `WISPHIVE_CODEX_RUNTIME_PROOF=1` to keep
  `just verify` deterministic, but actually executed and passed during this task).
- **#139 (terra, worktree): PASS.** useWisphive.ts's decode/encodeBase64 try native
  `Uint8Array.fromBase64()`/`toBase64()` first (feature-detected via `typeof ctor.fromBase64 ===
  "function"`, since TS lib target is ES2023 and jsdom/Node test env can't guarantee ES2024 methods),
  falling back to the original atob/per-byte-loop/btoa implementation. 2 new tests: one deletes the
  native methods and proves the fallback round-trips ASCII + arbitrary binary bytes (0x00/0xff/0x80/...);
  one stubs native methods and asserts they're actually dispatched to when present. 146/146 vitest,
  eslint clean.
- **#517 (terra, worktree): PASS.** Chose ref-based fix (not integration test) — App.tsx already owns the
  sidebar DOM node, so a `backgroundRef` prop threaded down to Terminals.tsx eliminates the
  `.app > .sidebar` string-selector coupling entirely rather than just detecting its breakage after the
  fact. Added a capturing Tab/Shift+Tab keydown trap on the mobile dialog (independent of native `inert`,
  mirrors Modal.tsx's existing FOCUSABLE pattern) with a new test that bypasses jsdom's lack of `inert`
  focus-suppression semantics to prove the trap holds focus inside the dialog in both directions. 145/145
  vitest, eslint clean.
- **#263 (terra, worktree): PASS on the actual change.** `revoke_web_device` now inspects
  `rows_affected()`; on 0 rows does a follow-up `SELECT 1 FROM web_devices WHERE id = ?` to distinguish
  already-revoked (idempotent, exit 0) from unknown id (`WebAuthError::NotFound` → CLI prints "unknown
  device id: <id>", nonzero exit). 3 new tests cover unknown/already-revoked/fresh-revoke, all pass
  (`3 passed; 0 failed` isolated). Codex self-reported FAIL only due to the same familiar sandbox
  socket-permission artifact seen throughout this run (2 unrelated tests); clippy clean, 376/378
  workspace tests pass when those 2 are skipped.
- **#516 (terra, worktree): PASS, independently re-verified outside Codex's sandbox** (which prohibits
  AF_UNIX binds outright — Codex's own transcript only ever saw the trivial skip-branch execute). Wrapper
  agent rebuilt/retested itself: real `wisphive daemon start`/`stop` subprocess lifecycle test (isolated
  temp HOME/PATH, confirmed different real PIDs across runs, confirmed `~/.wisphive/wisphive.pid`
  untouched) + 2 new interleave tests in server.rs (stop-path, spawn-path). **Self-flagged caveat for
  review**: the new interleave tests hand-roll a client reader at the wire-protocol level rather than
  calling the actual production `send_and_recv_on`/`is_matching_agent_reply` skip-loop the way the
  existing list-path interleave test (wisphive_cli/commands/agent.rs) does — proves wire round-trip
  shape, not the same production-code guarantee for stop/spawn. Also: one flaky run (10s wait_for_path
  timeout under concurrent build load) out of 3, consistent with this sprint's known machine-load
  sensitivity (itr#520 precedent) — always passed standalone. 343+1+27 tests, clippy clean.

## Wave 2 cross-model review

- **#263 (opus-4.8 reviewing terra): CLEAN — 1 harmless nit.** TOCTOU with `reset_web_password` (the only
  bulk-delete path): if UPDATE hits 0 rows and a concurrent password reset wipes all devices before the
  follow-up SELECT, reports NotFound for an id that briefly existed — deemed harmless since after a full
  reset the device really is gone, message isn't misleading. All 4 requested axes verified correct: same-id
  race handled right (UPDATE-affects-1 short-circuits before SELECT), NotFound never conflated with other
  DB errors (only constructed on `exists.is_none()`), exit code genuinely propagates nonzero via
  `Termination`, SELECT uses a bound parameter (no injection risk). **Verdict: CLOSE.**
- **#139 (opus-4.8 reviewing terra): CLEAN — 1 minor, 1 nit.** Minor: AC's "throughput bench improves"
  claim is unverified — no bench added, and native-path coverage is stub-only in CI (Node 22.12 lacks both
  methods, so `frontend-test` only exercises fallback + stubbed-dispatch tests; the real win only
  materializes on browsers shipping ES2024). Nit: fallback still uses the flagged per-byte
  `String.fromCharCode` hot path — unchanged perf on non-native environments, expected/acceptable as a
  fallback. All 4 requested correctness axes clean: binary semantics match old atob/btoa contract exactly
  (default alphabet/lastChunkHandling), feature detection is per-function and safe under partial
  implementations, casts don't mask any option mismatch, dispatch test genuinely distinguishes
  encode-vs-decode by return type + call args. **Verdict: CLOSE**, minor/nit optional follow-up.

## Wave 2 escalation

- **#511 (fable-5 → redo at fable-5, ceiling).** Codex cross-review found 3 BLOCKERS + 1 MAJOR — this
  security-critical ticket's first pass does not actually close the gap:
  1. blocker, process_registry.rs:861 — `parse_toml_key_path` doesn't decode TOML basic-string `\uXXXX`
     escapes; `"hooks" = false` parses as `features.u0068ooks` not `features.hooks`, so a
     legitimately-disabling TOML entry (or a foreign inline hook under an escaped key) evades detection
     entirely — disproves the "provable superset" claim outright, a concrete bypass even without the
     foreign-hooks opt-in.
  2. blocker, process_registry.rs:998 — plugin enumeration only admits files literally named
     `hooks.json`, silently skips symlinks, and has no session-source enumeration at all despite the AC
     requiring it — a plugin manifest relocating its hook file evades the audit entirely.
  3. blocker, process_registry.rs:1430 — TOCTOU: the audit reads config/hook files, then the spawned
     child re-reads them later; nothing ties the audited bytes to what the child actually consumes, so a
     swap between audit and spawn can run the child ungated.
  4. major, process_registry.rs:2913 — the runtime-proof test is skipped unless
     `WISPHIVE_CODEX_RUNTIME_PROOF` is set, so it provides zero evidence in normal `just verify` runs, and
     even when run it calls `audit`/`build_agent_command` directly rather than the real
     `ProcessRegistry::spawn_agent` path, covering only a clean happy path.
  #511 was executed at fable-5, already the ceiling — redoing once at the same model with all 4 findings
  spliced in, per the escalation rule for ceiling tasks.
- **#511 redo (fable-5): PASS, exceptionally thorough.** Replaced the hand-rolled TOML scanner with the
  real `toml` crate (checked Cargo.lock first, confirmed nothing transitively vendored a parser before
  adding one; `cargo deny check advisories bans sources` passes) — closes finding #1 structurally, not
  via more escape-pattern whack-a-mole. Plugin manifest resolution now follows symlinks (empirically
  verified against this machine's real `~/.codex/plugins` tree, finding actual production symlinks) with
  cycle-breaking + fail-closed on unresolvable paths; added `audit_codex_session_argv` for the
  daemon-managed-spawn session source. Finding #3 (TOCTOU) honestly NOT fully closed — added
  `AuditSnapshot` (SHA-256 hash-on-read) reverified before AND after `cmd.spawn()` (killing the child on
  post-spawn mismatch), narrowing the window to the instants around the spawn syscall while explicitly
  documenting the residual can't be eliminated without a Codex-side API that doesn't exist. Finding #4
  closed with an always-on offline runtime-proof test (no env gate) exercising the real `spawn_agent`
  path with a stand-in `codex` binary, including a negative case (foreign hook without opt-in → refused,
  no child launches). ~25 new tests, file grew 1400→3824 lines. Full `just verify` green (370+27 daemon
  tests). Self-filed itr#527 for a newly-discovered flake in the same test family as itr#525 — merged as
  duplicate, root-cause hypothesis sharpened. Re-review dispatched given the scope and criticality.
- **#511 redo re-review (gpt-5.6-terra): 2 BLOCKERS + 2 MAJORS + 2 minors — round 2 miss after the one
  ceiling redo.**
  1. blocker, process_registry.rs:773 — AST walk only checks `features.hooks`; Codex also accepts the
     deprecated `features.codex_hooks` alias per its config reference. `[features]\ncodex_hooks = false`
     disables the gate but evades this audit entirely.
  2. blocker, process_registry.rs:1725 — the TOCTOU mitigation's own test coverage has a gap in the
     window it disclosed: pre-spawn hash check passes, `spawn()` starts a concurrently-scheduled child,
     and the child can read swapped config and make an ungated tool call BEFORE the post-spawn re-hash/
     kill runs. Existing mutation tests only mutate before the pre-spawn check, not in this window.
  3. major, process_registry.rs:950 — manifest `hooks` field parsing only accepts a bare string; Codex's
     real manifest schema also supports an array of paths, an inline hooks object, and an array of inline
     objects. A correctly-configured plugin using any non-string form now gets EVERY managed Codex spawn
     refused — an availability regression, not just a security gap.
  4. major, process_registry.rs:3556 — the always-on offline runtime-proof's stand-in `codex` script only
     seds/evals the project hooks.json; doesn't model config-layer merging, features aliases, persisted
     disablement, profiles, or plugin enablement — so it would pass even with blocker #1 present and
     doesn't substantiate the claimed effective-inventory behavior.
  5. minor, process_registry.rs:953 — `dir.join(declared)` before `canonicalize` accepts absolute paths
     (e.g. `"hooks": "/tmp/x.json"`), letting a manifest point outside the plugin root; Codex requires
     paths to start `./` and stay inside the plugin root.
  6. minor, process_registry.rs:1053 — the new session-argv audit is raw substring matching (`contains
     ("feature")`), not key-path-aware parsing — same escape-bypass class as the ORIGINAL finding #1, just
     in new code. Not yet exploitable (today's builder doesn't emit arbitrary `-c`), but fragile.
  Per the escalation rule: #511 already had its one ceiling redo (fable-5, this pass) — a second miss
  means surfacing to the user rather than looping a third fable pass, same situation as #510 earlier in
  this run. User chose: route to gpt-5.6-sol at ultra effort (same choice as #510).
- **#511 second redo (gpt-5.6-sol, ultra): all 6 findings addressed.** Alias auditing for
  `features.hooks`/`features.codex_hooks` sharing canonical-key precedence; full 4-shape manifest union
  parsing (path string/array/inline object/array-of-inline) tested individually for false-refusal;
  absolute-path + traversal + symlink-escape rejection with canonical-root confinement; structured
  `-c`/`--config` TOML-key-path parsing replacing substring matching. Finding #2 (TOCTOU) honestly
  narrowed further, not claimed eliminated: pre-check is now the literal last op before `spawn()`,
  post-check the literal first op after — new test lands a mutation via a start/release rendezvous file
  exactly after the child starts and proves the recheck detects it and force-terminates; code comment
  states the remaining scheduler-level residual precisely rather than overclaiming. Codex self-reported
  FAIL only because of the same `UnixListener::bind` sandbox artifact seen throughout this run (#263,
  #516) — independently confirmed passing outside the sandbox, and I ran the FULL `just verify` myself on
  the integrated tree: all 5 gates genuinely green (fmt, clippy, 371+ tests, frontend, e2e). 68/68 new
  security-focused tests pass. Cumulative diff across 3 passes: +4543/-1009 in process_registry.rs.
  Fable-5 review dispatched per user instruction (in place of the usual Codex/opus reviewer, given fable
  is already the model that did the deepest passes on this ticket).
- **#511 fable review (round 3): 4 findings, none proven bugs.** 3 major (contingent — genuinely
  undecidable from outside Codex's source: `disableAllHooks` per-file vs global scope, `features.hooks`
  vs `codex_hooks` alias precedence, persisted-disable key-format assumption) + 1 major structural
  (~3000-line audit reimplements Codex's config-resolution internals — inherently fragile long-term
  regardless of specific bugs; filed as ADR-track itr#528, not attempted now) + 2 concrete (unbounded
  recursion DoS on agent-writable nested TOML; unvalidated `profile` string path-injection). Confirmed
  what held up from rounds 1-2: TOCTOU adjacency claim accurate (verify_unchanged is the literal
  last/first op around spawn in non-test builds), `-c`/`--config` argv parser covers every syntax variant
  tried, 4-shape manifest parity holds, escaped-key TOML decoding genuinely closes round-1's bug. User
  chose: fix the 2 concrete issues + resolve the 3 judgment calls toward fail-closed (not a redesign).
- **#511 round 4 (gpt-5.6-sol, ultra): small, targeted, all 5 changes applied.** Depth cap (32) on both
  recursive TOML scanners, fail-closed on exceeding it. `profile` string validated as a bare
  ASCII-identifier before path join. `disableAllHooks` now refuses the ENTIRE spawn regardless of which
  file it appeared in (not just masking that file's own hooks). Feature-alias check now ORs both disable
  signals (`hooks`/`codex_hooks`) instead of letting canonical-key precedence override an alias-disable.
  Persisted `hooks.state` now fails closed on an unrecognized-format key while still passing through
  empty/absent tables normally. Wrapper independently re-ran the 8 new tests + full daemon suite
  (394/394) + clippy, all clean. I ran the full `just verify` myself: all 5 gates genuinely green (fmt,
  clippy, workspace tests, frontend, e2e).
- **#511 round 4 review (opus-4.8): NO FINDINGS.** All 5 changes verified correct, complete, non-vacuous:
  depth cap enforced at function entry in both scanners with a genuine two-part test hitting each cap
  independently; profile validation runs before every path-join site (confirmed no second unvalidated
  call site); `disableAllHooks` bail lives in the shared `inspect_hook_value` gated on
  `HookSettingsKind::Codex` so it fires for every Codex source (project/user/plugin), genuinely refusing
  the whole spawn not just masking one file; alias OR-logic confirmed via all 3 cases
  (disagree-each-direction + neither-present-default) with no regression to round-1's escaped-key
  parsing; unrecognized-`hooks.state`-key check confirmed to never misclassify a correctly-formatted key
  (traced the `rsplitn` parsing against what `gate_state_keys` actually produces) while still passing
  empty/absent tables. No regressions to any prior round's fixes. **itr#511: DONE after 4 rounds** — 2
  concrete bugs (recursion DoS, path injection) fixed, 3 genuinely-undecidable judgment calls resolved
  toward fail-closed per PO direction, structural redesign concern filed as ADR-track itr#528.

- **#516 (terra → gpt-5.6-sol).** Opus review found a MAJOR defect, worse than the executor's own
  self-flagged caveat: the new stop/spawn interleave tests in server.rs are tautological — the fake
  daemon writes `[AgentExited, correlated_reply]`, the hand-rolled client reads exactly 2 lines and
  asserts their order, but never calls the real production skip-loop (`send_and_recv_on`/
  `is_matching_agent_reply` in wisphive_cli's agent.rs) or drives the real `handle_agent_command`'s
  per-variant correlation stamping. These tests would still pass if the skip-loop were deleted entirely —
  exactly the hard-coded-count anti-pattern that caused itr#468. Root cause: the tests are in the daemon
  crate, which structurally can't reach `send_and_recv_on` (private to wisphive_cli) — that crate
  placement is what forced the hand-rolling. Reviewer's fix: move the stop/spawn cases into wisphive_cli
  alongside the existing list-path test, reusing the real `connect_to_socket`/`send_and_recv_on`.
  The pidfile subprocess test itself is CONFIRMED SOLID (no finding) — traced through code to prove it
  genuinely exercises the itr#372 drop-before-exit fix, would fail if that fix were reverted. 2 minors on
  it, folding into the redo: bump the flaky `wait_for_path` 10s timeout to ~30s; `FrontendDistGuard`
  mutates a shared path in the real checkout rather than an isolated dir (SIGKILL-survival / concurrent-run
  race risk) — consider gating on pre-existing dir instead of fabricate-then-delete.
- **#517 (terra → gpt-5.6-sol).** Opus review found a MAJOR defect: the focus-trap's "focus began outside
  dialog" branch unconditionally pulls focus into the terminal dialog from anywhere outside it — including
  OTHER App-level modals that legitimately stack on top. Concrete reachable path: approving a sudo-class
  tool from the mobile terminal dock's `TerminalQueueDock` while the terminal dialog is open triggers
  `web_reauth_required` → `SudoModal` mounts over it; now two document-level capturing Tab traps are live,
  and the Terminals handler fires first, sees the SudoModal's password field isn't inside `.terminals-main`,
  and yanks focus back into the occluded terminal — the keyboard user can no longer Tab within the reauth
  modal at all. Modal.tsx (the pattern this was supposed to mirror) has no such outside-pull branch.
  Reviewer's fix: scope the pull-in to `backgroundRef.current?.contains(active)` instead of "anything
  outside the dialog" — still satisfies the existing test. Also minor: the new test passes through a
  jsdom quirk (`offsetParent` always null → `focusables` always empty → falls into the zero-focusable
  `dialog.focus()` fallback) rather than exercising the intended first/last edge logic, so it can't
  distinguish the buggy broad trap from a correctly-scoped one. Redoing at gpt-5.6-sol with both findings
  spliced in.
- **#517 redo (gpt-5.6-sol): PASS.** Scoped the "focus outside dialog" recovery branch to
  `backgroundRef.current?.contains(active)` — only pulls focus back when it escaped into the occluded
  background, leaves it alone (no `preventDefault`) when it's in another stacked modal. New regression
  test mounts a second stacked dialog, focuses its password field, asserts Tab doesn't steal focus.
  Rigorously verified falsifiable: reverted to the old unconditional condition and confirmed the new test
  actually FAILS (`1 failed`), then restored the fix and confirmed it PASSES (`1 passed`) — not just a
  self-report. Second new test patches jsdom's `offsetParent` on real controls to exercise the actual
  first/last Tab-wrap edge logic instead of the empty-list fallback. 147/147 vitest, eslint clean.
  Re-review pending.
- **#516 redo (gpt-5.6-sol): PASS.** Removed the tautological server.rs interleave tests entirely. Added
  `stop_agent_skips_interleaved_exit_and_other_correlated_reply` /
  `spawn_agent_skips_interleaved_exit_and_other_correlated_reply` in wisphive_cli's agent.rs, mirroring
  the existing list-path test's pattern; extracted `connect_on_stream` from production `connect_to_socket`
  so tests drive the real handshake over `UnixStream::pair()` and genuinely call the real
  `send_and_recv_on` skip-loop — verified in the diff, gap #1 closed. Gap #2 (daemon-side correlation
  stamping) honestly left open and documented in a comment: `handle_agent_command`/`ConnectionContext`
  are private to wisphive_daemon, structurally inaccessible from the CLI test module. `wait_for_path`
  bumped 10s→30s (real run took 25.69s — legitimate flake risk, not over-fixing). `FrontendDistGuard`
  replaced with a skip-with-actionable-message check instead of fabricate-then-delete. Wrapper
  independently re-ran everything outside Codex's sandbox: 341+27+1 tests pass, clippy clean. Re-review
  pending.
- **#517 redo re-review (opus-4.8): CLEAN — 2 nits.** Verified the null-ref case correctly does nothing
  (no fallthrough to stealing everywhere); confirmed the original itr#488 protection is intact (narrowed
  condition still matches the exact inert scope, no other occluded focusable region leaks through — traced
  CSS + banner components). Nit: the stacked-modal test's mock dialog has no keydown listener, so it
  proves "Terminals doesn't steal" but not the real two-capture-listener interaction with Modal.tsx's
  actual trap. Nit: recommend a code comment tying the fix's correctness to "modals render as siblings
  of, never descendants of, backgroundRef" for future-refactor robustness. Mentally re-ran the pre-fix
  code against the new test and confirmed it would genuinely fail. **Verdict: CLOSE.**
- **#516 redo re-review (opus-4.8): 1 minor, 1 nit, no blocker.** Traced all 3 verification points as
  genuinely closed: real `send_and_recv_on` called (not a re-hand-rolled equivalent), `connect_on_stream`
  extraction is a faithful refactor (production `connect_to_socket` byte-for-byte unchanged), and the new
  tests force a genuine skip (3 snapshots + unrelated AgentExited + same-variant-wrong-correlation reply
  before the real match — 3 distinct plausible regressions all traced to fail against these tests).
  Confirmed tautological server.rs tests fully removed via grep. Minor: the gap-#2 "structurally
  unreachable" framing overstates it — daemon crate already has precedent (`dispatch_command` driven over
  `UnixStream::pair()` at server.rs:4680) for closing this cheaply in-crate; reviewer notes `StopAgent`'s
  Some/None branch (AgentStopResponse vs AgentExited) is a real untested inversion risk. Explicitly "not a
  blocker for a C1 ticket." Nit: new tests use `UnixStream::pair()` without the list-test's
  `unix_sockets_are_available()` skip-guard (defensible — pair() is more permissive than bind(), not a
  real coverage gap). **Verdict: CLOSE**, filing the daemon-side dispatch_command stamping test as a
  fast-follow (same treatment as other minor findings this run).

## Wave 3 execution log

- **#518 (terra, worktree): all 5 sub-items fixed.** (a) ci.yml — rg exit-code branch (0=violation,
  1=pass, 2=hard-failure with distinct message). (b) retention.rs — BufWriter + serde_json::to_vec +
  explicit flush before the existing fsync (preserves the audit-never-lost invariant). (c) ui.rs —
  `u16::try_from(...).unwrap_or(u16::MAX)` clamp instead of raw `as u16` wrap. (d) server.rs — removed
  the dead `AgentSpawned` construction on the spawn success path (protocol variant retained, CLI still
  matches it). (e) redact.rs — new `push_log_safe` helper escaping C0 control chars (except `\t`/`\n`) as
  `\xHH`; new test asserts exact escaped output. Build+clippy clean; retention (7) + redact (6) unit
  tests pass directly. Codex self-reported FAIL only because all 27 daemon integration tests fail at
  socket-permission setup under the sandbox (chmod denied) — same known artifact category as this run's
  earlier findings, not yet independently re-verified outside the sandbox as of this log entry.
- **#514 (terra, worktree): PASS.** Added `ViteTypeOptions.strictImportMetaEnv` interface (drops the
  permissive index signature from ImportMetaEnv). Proved the typo now errors via a standalone
  TypeScript-compiler-API probe injecting a synthetic source file into the real tsconfig program —
  confirmed diagnostic TS2339 fires, not just "added the interface and assumed." 2 new tests (omitted env
  var accepted; blank VITE_API_URL throws, mirroring the existing VITE_WS_URL pattern). 151/151 vitest,
  eslint + tsc --noEmit clean.
- **#519 (terra, worktree): PASS.** `--host` changed from `String` with a default to `Option<String>` (no
  default), so `daemon_web_requested` can distinguish "explicitly passed 127.0.0.1" from "omitted
  entirely" — mirrors itr#348's existing `Option`-based pattern for `--port`. Default bind behavior
  preserved (unwrapped to "127.0.0.1" after the web-requested check). New regression test confirms
  explicit `--host 127.0.0.1` now triggers web. 40/40 tests, clippy clean.
- **#512 (terra, worktree): fix applied, same known sandbox artifact.** Mirrored the pruner's tolerant
  pattern: `read_dir(log_dir)?.flatten()` for the initial listing (directory-level readability stays a
  startup precondition) + `let Ok(file_type) = entry.file_type() else { warn!(...); continue }` for
  per-entry errors, instead of `?`-propagating a single broken dirent into a fatal startup abort. New
  regression test `reimport_rotated_segments_skips_dangling_symlink` (dangling symlink + one valid
  segment, asserts the valid one still ingests). `cargo test -p wisphive_daemon --lib`: 395/395 pass
  (including the new test), clippy clean. Same Unix-socket-bind sandbox artifact as #518/#516/#263/#511
  blocks the `server_integration` test binary specifically (confirmed by Codex itself: same 27 failures
  reproduce against the unmodified file in the same sandbox — not caused by this change).
- **#515 (terra, worktree): fix applied, same known sandbox artifact.** Deleted `default_mode_path()`;
  `ProcessRegistry::new` gained a third `home_dir: PathBuf` param, builds the mode path via
  `home_dir.join("mode")` instead of re-deriving from `$HOME`. Server.rs's sole call site now passes
  `config.home_dir.clone()`. New regression test constructs two registries against synthetic homes with
  `mode=active` vs `mode=off`, proving the check follows the injected home_dir. Did not touch itr#511's
  hook-audit/TOCTOU/TOML code (correctly scoped). `cargo test -p wisphive_daemon --lib`: 395/395 pass,
  clippy clean. Same `server_integration` sandbox artifact as #512/#516/#518/#263/#511.
- **All 5 Wave 3 tasks reported.** None yet integrated into the shared checkout, none yet cross-reviewed,
  none yet independently re-verified outside the sandbox. **This is the resume point** — see the handoff
  doc for exact next steps.

## Wave 3 review + commit / Wave 4 (resumed 2026-07-14)

Resumed from the handoff (`docs/handoff/2026-07-14-sprint5-crossfire-blitz-wave3-wave4.md`).

- **Wave 3 cross-review (opus-4.8, independent): SHIP.** All 5 tickets verified to do what they claim —
  no vacuous tests, no no-op refactors, no deleted coverage. Two P2 nits raised on #512: (A) rotated-events
  recovery used `read_dir()?.flatten()`, which silently drops a mid-iteration readdir `Err` — inconsistent
  with the new `file_type()`-error `warn!`+skip branch; **folded into the commit** (explicit `Err` arm now
  logs+skips). (B) the `skips_dangling_symlink` test exercises the `!is_file()` path, not the new
  `file_type()`-error branch (a dangling symlink is `Ok(symlink)` on Unix, not `Err`); left as-is — the
  test is honestly named and the error branch isn't portably forceable. All 5 sandbox-artifact FAILs from
  the execution log re-verified benign under a full local `just verify` (all gates green).
- **Wave 3 committed:** `02c7d7f` (11 files). Closed #515, #518, #512, #514, #519.
- **Wave 4 executed (opus-4.8) + cross-reviewed (independent): SHIP, no findings.**
  - **#292** — `logging.rs`: `envfilter_floor_and_stderr_ceiling_are_independent`, a non-global subscriber
    mirroring `init()`'s stack; asserts the registry EnvFilter floor (DEBUG reaches store) and per-layer
    stderr WARN ceiling (DEBUG clamped off stderr; WARN reaches both) independently. Positive `warn-line`
    assertion guards against a vacuous pass. Documents why global `try_init` is untestable.
  - **#520** — inbox e2e: header count timeout 15s→30s (`toHaveText` already polls) and the `wisphive audit`
    oracle wrapped in `expect.poll(...).toBe(1)` to retry the CLI read. Both exact assertions preserved —
    over-count still fails. Reviewer confirmed poll retries the read, not the threshold.
  - **#450** — `docs/smoke/CHECKLIST.md`: corrected inbox smoke provenance — AC2/AC3 via the real
    `wisphive-hook` binary, AC1 via the real-wire socket fixture (`e2e/fixtures/hook-client.ts`); removed
    the overstated "all via the hook binary" claim. Matches the spec header exactly.
- **Wave 4 committed:** `f7dd6de` (3 files). Closed #292, #520, #450. Full `just verify` green (incl. the
  new logging test and the hardened inbox e2e spec, 11/11 e2e pass).
- **All 19 sprint-5 member issues now closed.** Epic #524's acceptance is met. Follow-ups filed during the
  run remain open by design and are NOT members: #528 (ADR-track, Codex hook-audit redesign), #526
  (daemon-side dispatch_command correlation-stamping test), #525 (flaky hook socket-test harness).
  Remaining step: `/sprint-review` (Outcomes/Demo/Retro + epic close).

## Outcomes

_populated at Phase 8_
