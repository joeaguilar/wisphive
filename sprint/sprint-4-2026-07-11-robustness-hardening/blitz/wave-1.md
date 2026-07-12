# Sprint-4 blitz wave plan

## Config

- Created: `2026-07-11T21:17:27Z`
- Tracker scope: `itr list --parent 508 --status open --include-blocked --format json`
- Tracker record: `itr#509`, a high-priority child epic of sprint epic `itr#508`.
- Repository: `.`
- Starting commit: `bce236c968b6803d5d10b7b5fa98f586606b03df`
- Dependency audit: `kgr check --format json --no-progress .`; no rule violations, three pre-existing module/test cycles, and partial coverage for unsupported snapshots/CSS/docs artifacts.
- Verify gate: `cargo test && cargo clippy -- -D warnings && cargo fmt --check`
- Frontend supplement: frontend stories also run the repository's frontend test/lint gate required by the sprint Definition of Done.
- Concurrency: 3 workers (four total agent slots including the orchestrator).
- Stop conditions: sprint backlog empty; two no-progress waves; foundational quarantine; or a red wave gate that cannot be safely repaired.
- Commit policy: user explicitly requested periodic commits. The orchestrator commits only green, reviewed wave batches using the required Josef Aguilar identity, Conventional Commits, and the Codex trailer. Workers do not commit.
- Dirty-tree exclusions: the nine modified sprint-2 evidence PNGs predate this run and must never be staged. `sprint/CURRENT` and the original sprint-4 `plan.md` also predate this run; only deliberate blitz-log/plan updates are owned here.

## Waves

### Wave 1

- **#94 — Validate SpawnAgent flags + queue for human approval**
  - Files: `crates/wisphive_daemon/src/process_registry.rs`, `crates/wisphive_daemon/src/server.rs`, `crates/wisphive_daemon/src/sudo_gate.rs`, `crates/wisphive_daemon/src/state/decisions.rs`, `crates/wisphive_daemon/src/state/decisions_tests.rs`, `crates/wisphive_daemon/src/queue.rs`, `crates/wisphive_cli/src/commands/agent.rs`
- **#105 — Restructure ServerMessage union as proper discriminated union**
  - Files: `crates/wisphive_web/frontend/src/types/protocol.ts`, `crates/wisphive_web/frontend/src/hooks/useWisphive.ts`, `crates/wisphive_web/frontend/src/hooks/useWisphive.test.ts`
- **#106 — Replace component casts with type guards**
  - Files: `crates/wisphive_web/frontend/src/components/DetailView.tsx`, `crates/wisphive_web/frontend/src/components/Queue.tsx`, `crates/wisphive_web/frontend/src/components/Agents.tsx`, `crates/wisphive_web/frontend/src/components/ToolContent.tsx`, `crates/wisphive_web/frontend/src/components/DetailView.test.tsx`, `crates/wisphive_web/frontend/src/components/toolInput.ts`, `crates/wisphive_web/frontend/src/components/queueUtils.ts`

### Wave 2

- **#294 — Agent commands read the wrong post-handshake response**
  - Files: `crates/wisphive_cli/src/commands/agent.rs`, `crates/wisphive_daemon/src/server.rs`
- **#295 — Multiple sudo-gated approvals strand older requests**
  - Files: `crates/wisphive_web/frontend/src/hooks/useWisphive.ts`, `crates/wisphive_web/frontend/src/hooks/useWisphive.test.ts`
- **#362 — TUI byte-slice truncation panics on Unicode**
  - Files: `crates/wisphive_tui/src/panels.rs`, `crates/wisphive_tui/src/ui.rs`, `crates/wisphive_tui/tests/ui_snapshots.rs`

### Wave 3

- **#296 — Move terminal-output side effects out of the reducer**
  - Files: `crates/wisphive_web/frontend/src/hooks/useWisphive.ts`, `crates/wisphive_web/frontend/src/hooks/useWisphive.test.ts`
- **#411 — Use precise Wisphive hook matching in project audit**
  - Files: `crates/wisphive_daemon/src/project_audit.rs`, `crates/wisphive_cli/src/commands/hooks.rs`
- **#86 — Validate agent IDs before filesystem access**
  - Files: `crates/wisphive_hook/src/main.rs`, `crates/wisphive_daemon/src/server.rs`

### Wave 4

- **#99 — Cap concurrent Unix-socket connections**
  - Files: `crates/wisphive_daemon/src/server.rs`
- **#111 — Memoize keyboard actions**
  - Files: `crates/wisphive_web/frontend/src/hooks/useKeyboard.ts`, `crates/wisphive_web/frontend/src/hooks/useKeyboard.test.ts`, `crates/wisphive_web/frontend/src/App.tsx`
- **#116 — Use stable React keys for questions and options**
  - Files: `crates/wisphive_web/frontend/src/components/DetailView.tsx`, `crates/wisphive_web/frontend/src/components/DetailView.test.tsx`

### Wave 5

- **#114 — Move title/notification side effects out of the reducer**
  - Files: `crates/wisphive_web/frontend/src/hooks/useWisphive.ts`, `crates/wisphive_web/frontend/src/hooks/useWisphive.test.ts`
- **#503 — Improve fail-closed TLS corruption remediation**
  - Files: `crates/wisphive_web/src/tls.rs`

### Wave 6

- **#95 — Make mode-file reads fail secure and verify permissions**
  - Files: `crates/wisphive_hook/src/main.rs`, `crates/wisphive_hook/Cargo.toml`, `crates/wisphive_hook/tests/mode_failclosed.rs`, `crates/wisphive_cli/src/commands/hooks.rs`, `crates/wisphive_daemon/src/config.rs`, `crates/wisphive_daemon/src/server.rs`, `crates/wisphive_daemon/tests/server_integration.rs`, `AGENTS.md`
- **#205 — Add Copy controls to History details**
  - Files: `crates/wisphive_web/frontend/src/components/DetailView.tsx`, `crates/wisphive_web/frontend/src/components/ToolContent.tsx`, `crates/wisphive_web/frontend/src/components/CopyButton.tsx`, `crates/wisphive_web/frontend/src/components/HistoryEntryItem.tsx`, `crates/wisphive_web/frontend/src/components/HistoryEntryItem.test.tsx`
- **#303 — Remove the unsupported terminal-close kill distinction**
  - Files: `crates/wisphive_daemon/src/terminal.rs`, `crates/wisphive_daemon/src/server.rs`, `crates/wisphive_protocol/src/wire.rs`, `crates/wisphive_cli/src/commands/term.rs`, `crates/wisphive_cli/src/commands/tui.rs`, `crates/wisphive_cli/src/main.rs`, `crates/wisphive_web/frontend/src/hooks/useWisphive.ts`, `crates/wisphive_web/frontend/src/types/protocol.ts`, `docs/ROADMAP.md`

### Wave 7

- **#306 — Reject invalid daemon/web host values**
  - Files: `crates/wisphive_cli/src/main.rs`
- **#307 — Handle non-object Claude hook settings without panic**
  - Files: `crates/wisphive_cli/src/commands/hooks.rs`, `crates/wisphive_daemon/src/hook_install.rs`
- **#262 — Enforce mode 0600 on SQLite database sidecars**
  - Files: `crates/wisphive_daemon/src/state/mod.rs`, `AGENTS.md`

### Wave 8

- **#367 — Skip unknown server messages without disconnecting**
  - Files: `crates/wisphive_tui/src/connection.rs`, `crates/wisphive_cli/src/commands/tui.rs`, `crates/wisphive_protocol/src/wire.rs`
- **#369 — Scroll selected TUI list rows into view**
  - Files: `crates/wisphive_tui/src/ui.rs`, `crates/wisphive_tui/tests/ui_snapshots.rs`
- **#375 — Detach web terminal streams on replay/view changes**
  - Files: `crates/wisphive_web/frontend/src/components/Terminals.tsx`, `crates/wisphive_web/frontend/src/components/Terminals.test.tsx`, `crates/wisphive_web/frontend/src/hooks/useWisphive.ts`, `crates/wisphive_web/frontend/src/hooks/useWisphive.test.ts`

### Wave 9

- **#376 — Guard browsers without the Notification API**
  - Files: `crates/wisphive_web/frontend/src/hooks/useWisphive.ts`, `crates/wisphive_web/frontend/src/hooks/useWisphive.test.ts`
- **#407 — Serialize every config.json writer**
  - Files: `crates/wisphive_daemon/src/config.rs`, `crates/wisphive_daemon/src/server.rs`, `crates/wisphive_web/src/lib.rs`, `crates/wisphive_web/src/http_tests.rs`, `crates/wisphive_cli/src/commands/config.rs`, `crates/wisphive_cli/src/commands/tui.rs`, `crates/wisphive_tui/src/app.rs`, `crates/wisphive_tui/src/input.rs`, `crates/wisphive_tui/Cargo.toml`, `Cargo.lock`, `AGENTS.md`
- **#412 — Detect partial hook installations in doctor/agent preflight**
  - Files: `crates/wisphive_cli/src/commands/doctor.rs`, `crates/wisphive_cli/src/commands/agent.rs`

### Wave 10

- **#470 — Ignore interleaved broadcasts in CLI agent commands**
  - Files: `crates/wisphive_cli/src/commands/agent.rs`
- **#488 — Add dialog semantics/inert background to mobile terminals**
  - Files: `crates/wisphive_web/frontend/src/components/Terminals.tsx`, `crates/wisphive_web/frontend/src/components/Terminals.test.tsx`
- **#504 — Persist upgraded Argon2 hashes after successful login**
  - Files: `crates/wisphive_web/src/lib.rs`, `crates/wisphive_web/src/auth.rs`, `crates/wisphive_web/src/http_tests.rs`, `crates/wisphive_daemon/src/state/web_auth.rs`, `crates/wisphive_daemon/src/state/web_auth_tests.rs`

### Wave 11

- **#495 — Serialize web config read-modify-write updates**
  - Files: `crates/wisphive_web/src/lib.rs`, `crates/wisphive_daemon/src/config.rs`
- **#91 — Replace dynamic history SQL and add FTS**
  - Files: `crates/wisphive_daemon/src/state/decisions.rs`, `crates/wisphive_daemon/src/state/decisions_tests.rs`, `crates/wisphive_daemon/src/state/migrate.rs`
- **#97 — Sanitize control characters in logs and notifications**
  - Files: `crates/wisphive_daemon/src/notify.rs`, `crates/wisphive_daemon/src/server.rs`, `crates/wisphive_daemon/src/queue.rs`

### Wave 12

- **#101 — Log and rate-limit invalid protocol-version hellos**
  - Files: `crates/wisphive_daemon/src/server.rs`
- **#118 — Validate frontend endpoint environment variables**
  - Files: `crates/wisphive_web/frontend/src/hooks/useWisphive.ts`, `crates/wisphive_web/frontend/src/components/Config.tsx`
- **#137 — Move retention archive writes off the Tokio reactor**
  - Files: `crates/wisphive_daemon/src/state/retention.rs`

### Wave 13

- **#102 — Create hook marker files with O_EXCL semantics**
  - Files: `crates/wisphive_hook/src/main.rs`, `crates/wisphive_daemon/src/server.rs`
- **#265 — Make history truncation Unicode-safe**
  - Files: `crates/wisphive_cli/src/commands/history.rs`
- **#348 — Decouple web enablement from the default port sentinel**
  - Files: `crates/wisphive_cli/src/main.rs`

### Wave 14

- **#336 — Re-ingest orphaned event-log segments at startup**
  - Files: `crates/wisphive_daemon/src/event_ingest.rs`, `crates/wisphive_daemon/src/server.rs`
- **#371 — Parse host values as strict IPv4 addresses**
  - Files: `crates/wisphive_cli/src/main.rs`
- **#372 — Remove stale PID files on clean shutdown**
  - Files: `crates/wisphive_cli/src/commands/daemon.rs`, `crates/wisphive_daemon/src/shutdown.rs`

### Wave 15

- **#373 — Report non-object/corrupt TUI config saves**
  - Files: `crates/wisphive_tui/src/app.rs`
- **#374 — Clamp detail-view jump-to-bottom scrolling**
  - Files: `crates/wisphive_tui/src/input.rs`, `crates/wisphive_tui/src/ui.rs`
- **#378 — Wire and document the Terminals keyboard shortcut**
  - Files: `crates/wisphive_web/frontend/src/App.tsx`, `crates/wisphive_web/frontend/src/hooks/useKeyboard.ts`

### Wave 16

- **#377 — Preserve multiline spawn-agent prompts**
  - Files: `crates/wisphive_tui/src/input.rs`
- **#408 — Support nested-null deletion in config merge patches**
  - Files: `crates/wisphive_web/src/lib.rs`
- **#409 — Surface Always Allow persistence failures in the TUI**
  - Files: `crates/wisphive_daemon/src/server.rs`, `crates/wisphive_tui/src/modal.rs`

### Wave 17

- **#482 — Remove hard-coded terminal touch row-height fallback**
  - Files: `crates/wisphive_web/frontend/src/components/TerminalView.tsx`
- **#261 — Zeroize plaintext CLI web passwords**
  - Files: `crates/wisphive_cli/src/commands/web.rs`, `crates/wisphive_cli/Cargo.toml`, `Cargo.toml`, `Cargo.lock`
- **#273 — Abort frontend API requests when hooks unmount**
  - Files: `crates/wisphive_web/frontend/src/api.ts`, `crates/wisphive_web/frontend/src/hooks/useAuth.ts`

### Wave 18

- **#277 — Replace browser-open sleeps with a web-ready signal**
  - Files: `crates/wisphive_web/src/lib.rs`, `crates/wisphive_cli/src/main.rs`, `crates/wisphive_cli/src/commands/daemon.rs`
- **#505 — Make the below-floor password error reachable**
  - Files: `crates/wisphive_web/frontend/src/components/Login.tsx`, `crates/wisphive_web/frontend/src/components/Login.test.tsx`
- **#248 — Run cargo-deny in CI**
  - Files: `.github/workflows/ci.yml`

### Wave 19

- **#281 — Make first-run password/device provisioning atomic**
  - Files: `crates/wisphive_web/src/lib.rs`, `crates/wisphive_web/src/http_tests.rs`, `crates/wisphive_daemon/src/state/web_auth.rs`, `crates/wisphive_daemon/src/state/web_auth_tests.rs`
- **#318 — Close the auth-profile review tail**
  - Files: `crates/wisphive_web/src/auth_profile.rs`, `crates/wisphive_cli/src/main.rs`, `crates/wisphive_web/Cargo.toml`, `Cargo.toml`
- **#410 — Sudo-gate web-origin ApprovePermission**
  - Files: `crates/wisphive_daemon/src/server.rs`

### Wave 20

- **#472 — Align daemon SpawnAgent behavior with kill-switch preflight**
  - Files: `crates/wisphive_daemon/src/process_registry.rs`
- **#365 — Remove ended terminal sessions from the live map**
  - Files: `crates/wisphive_daemon/src/terminal.rs`

## File conflicts

- `crates/wisphive_daemon/src/server.rs`: #94 → #294 → #86 → #99 → #95 → #303 → #407 → #97 → #101 → #102 → #336 → #409 → #410.
- `crates/wisphive_web/frontend/src/hooks/useWisphive.ts`: #105 → #295 → #296 → #114 → #303 → #375 → #376 → #118.
- `crates/wisphive_web/frontend/src/types/protocol.ts`: #105 → #303.
- `crates/wisphive_web/src/lib.rs`: #407 → #504 → #495 → #408 → #277 → #281.
- `crates/wisphive_web/src/http_tests.rs`: #407 → #504 → #281.
- `crates/wisphive_web/frontend/src/components/Terminals.test.tsx`: #375 → #488.
- `crates/wisphive_daemon/src/state/web_auth.rs`: #504 → #281.
- `crates/wisphive_cli/src/main.rs`: #303 → #306 → #348 → #371 → #277 → #318.
- `crates/wisphive_hook/src/main.rs`: #86 → #95 → #102.
- `crates/wisphive_cli/src/commands/agent.rs`: #94 → #294 → #412 → #470.
- `crates/wisphive_cli/src/commands/hooks.rs`: #411 → #95 → #307.
- `crates/wisphive_daemon/src/config.rs`: #95 → #407 → #495.
- `crates/wisphive_tui/src/ui.rs`: #362 → #369 → #374.
- `crates/wisphive_web/frontend/src/components/DetailView.tsx`: #106 → #116 → #205.
- `crates/wisphive_cli/src/commands/daemon.rs`: #372 → #277.
- `crates/wisphive_daemon/src/process_registry.rs`: #94 → #472.
- `crates/wisphive_daemon/src/terminal.rs`: #303 → #365.
- `crates/wisphive_protocol/src/wire.rs`: #303 → #367.
- `crates/wisphive_cli/src/commands/tui.rs`: #303 → #367.
- `crates/wisphive_tui/src/input.rs`: #407 → #374 → #377.
- `crates/wisphive_cli/src/commands/tui.rs`: #303 → #367 → #407.
- `crates/wisphive_tui/src/app.rs`: #407 → #373.
- `Cargo.lock`: #407 → #261.
- `crates/wisphive_web/frontend/src/App.tsx` and `useKeyboard.ts`: #111 → #378.
- `crates/wisphive_web/frontend/src/components/Terminals.tsx`: #375 → #488.
- `Cargo.toml`: #261 → #318.
- `ToolContent.tsx`: #106 → #205.
- `crates/wisphive_daemon/src/queue.rs`: #94 → #97.
- `crates/wisphive_daemon/src/state/decisions.rs`: #94 → #91.
- `AGENTS.md`: #95 → #262 → #407.

No wave contains a shared owned file. Co-located tests inherit the same ownership chain as their source; a worker must request an ownership expansion before touching any additional test file.

## Semantic warnings

- **#407 / #495 overlap:** #407 is the superset and lands first. In Wave 11, #495 should inspect the landed behavior; if its acceptance is already fully proven, add only missing regression evidence and close it without duplicating the lock design.
- **#105 is a frontend protocol foundation:** later `useWisphive.ts` stories must preserve its discriminated-union validation and avoid reintroducing broad casts.
- **#114 / #376 are one side-effect seam:** #114 performs the extraction first; #376 must harden the extracted notification path rather than recreating reducer-side behavior.
- **#294 / #470 share response selection:** #294 fixes startup snapshots; #470 later proves unrelated live broadcasts cannot satisfy a command reply.
- **#306 / #371 share host parsing:** #306 establishes non-zero error propagation; #371 replaces permissive parsing without weakening that behavior.
- **#303 used its documented safe alternative:** nested review proved a truthful force/graceful distinction unsafe with the current PTY ownership model, so the unsupported flag was removed end-to-end while legacy payloads remain decodable.
- **#472 contains a policy branch:** prefer the existing CLI-safe refusal semantics so daemon and CLI behavior align; stop for PO input if the current architecture makes the alternative materially safer.
- **Stale `state.rs` ownership normalized:** #91 maps to `state/decisions.rs`, tests, and migration code; #137 maps to `state/retention.rs`; #281 maps to web-auth state and HTTP tests. This removes a false conflict introduced by the pre-split tracker paths.
- **Directory ownership normalized:** #248 owns only `.github/workflows/ci.yml`, not the whole workflow directory.
- **Review checkpoints:** honor the sprint plan's dual adversarial review after each lane's final landed wave and once over the whole sprint diff. Survivors are filed as `crossfire-review` / `sprint-4-followup`; they are not fixed inline.

## Interventions

- **Pre-Wave 1 ownership expansion:** added `daemon/server.rs` to #94 because its existing tracker note identifies `handle_tui` as the required human-approval dispatch point; added the existing `useWisphive.test.ts` and `DetailView.test.tsx` regression surfaces to #105/#106. Wave 1 remains conflict-free.
- **Wave 1 / #106 ownership grant:** added new non-component helper `frontend/src/components/toolInput.ts`; exporting the shared parser from `ToolContent.tsx` would violate `react-refresh/only-export-components`. No neighbor owns the new path.
- **Wave 1 / #106 review repair:** independent review found that `Queue.tsx` delegates its unsafe parsing to `queueUtils.ts`, which still contained the original unchecked assertions and bypassed the new single guard. Reopened #106, durably added `queueUtils.ts` to tracker/plan ownership, and returned it to the original worker for a narrow repair plus both gates.
- **Wave 1 / #105 review repair:** cross-language audit found the parser rejected legal scalar/array `serde_json::Value` payloads and accepted malformed Rust enum/UUID/timestamp/integer domains. Reopened #105 with a durable tracker note and returned it to the original worker to accept all JSON shapes, validate representable wire domains, document JavaScript safe-integer limits, and rerun both gates.
- **Wave 1 / #94 security retry:** independent review blocked commit on ignored reviewer edits, a pending/audit race, Codex options accepted but ignored, unbounded abandoned approvals, missing web sudo reauth, fabricated restart approvals, and a broken CLI queued-response path. Reopened #94 with durable tracker note #180; expanded ownership to `sudo_gate.rs`, state decision/recovery files, `queue.rs`, and CLI `agent.rs`; returned it to the original worker for one consolidated retry with lifecycle tests and full gate.
- **Wave 1 / #105 type-widening intervention:** honest recursive `JsonValue` types exposed four errors in #106-owned consumers. The orchestrator changed the DetailView fixture parameter to `JsonValue` and added an explicit object guard before queue summary code reads `event_data`; no behavior or ownership beyond those mechanical compatibility fixes changed.
- **Wave 1 / #105 prototype-safety intervention:** repair recheck found `{}` plus indexed assignment mishandled an own `__proto__` JSON key. The orchestrator switched recursive object reconstruction to own-property-safe `Object.fromEntries` and added WebSocket-path regression coverage proving the key remains inert data with no inherited `command` field.
- **Wave 1 / #94 expanded retry:** the retry's scratch security review continued until PASS, repairing claim/expiry/bulk-deny races, exact edited-input validation, durable failure reconciliation, strict agent-specific argv and hook checks, server-owned provenance, active-mode checks, sudo reauth, restart fail-closed behavior, and CLI queued acknowledgments. Follow-ups #510 and #511 capture two pre-existing seams that require separate scope.
- **Wave 1 gate:** orchestrator reran both repository gates after all repairs: `GATR exit=0 dur=23.5s errors=0 warnings=0 adapter=generic tag=blitz-wave1-final-rust` and `GATR exit=0 dur=6.5s errors=0 warnings=0 adapter=generic tag=blitz-wave1-final-frontend` (13 files, 117 tests).
- **Pre-Wave 2 test ownership:** added the existing `useWisphive.test.ts` surface to #295 and `tui/tests/ui_snapshots.rs` to #362. Neither path conflicts with another Wave 2 worker.
- **Wave 2 / #295 transient gate red:** its first Rust gate overlapped #362's in-flight TUI snapshot edits and failed only on those two neighbor tests. No #295 rework was needed; after #362 converged, the required rerun passed.
- **Wave 2 gate:** orchestrator reran both repository gates after all workers settled: `GATR exit=0 dur=23.5s errors=0 warnings=0 adapter=generic tag=blitz-wave2-final-rust` and `GATR exit=0 dur=6.2s errors=0 warnings=0 adapter=generic tag=blitz-wave2-final-frontend` (13 files, 118 tests).
- **Wave 3 sandbox gate retry:** #296 and #86 each hit a sandbox-only `PermissionDenied` when the existing #294 fake-daemon regression bound a Unix socket. The exact test and full gates passed outside that restriction; no product-code repair was needed.
- **Wave 3 gate:** orchestrator reran both repository gates after all workers settled: `GATR exit=0 dur=23.4s errors=0 warnings=0 adapter=generic tag=blitz-wave3-final-rust` and `GATR exit=0 dur=6.4s errors=0 warnings=0 adapter=generic tag=blitz-wave3-final-frontend` (13 files, 119 tests).
- **Pre-Wave 4 re-pack:** #95's full body requires daemon-side mode enforcement in `server.rs`, which conflicts with #99. Expanded #95 to `server.rs` + `AGENTS.md` and moved it to Wave 6; moved #116 to Wave 4 and #262 to Wave 5. The three waves remain size 3 and conflict-free.
- **Pre-Wave 5 re-pack:** #96 changes documented runtime-file permission semantics and therefore also owns `AGENTS.md`, conflicting with #262. Moved #503 to Wave 5, #262 to Wave 7, and #365 to Wave 20. Wave count/capacity remain unchanged and all affected waves stay conflict-free.
- **Wave 5 / #96 hard quarantine:** exact same-UID provenance cannot be authenticated by owner/mode checks or a file-backed key readable by that UID. The worker made no edits. PO directed that #96 remain open for Fable review and be removed from this blitz; tracker tags `needs-fable-review` and `blitz-skipped` plus note #181 preserve the full required design expansion. Outcome: `failed-skipped`.
- **Wave 5 sandbox gate retry:** #114's first Rust run hit the existing fake-daemon Unix-socket sandbox restriction; its authorized rerun and the orchestrator gate passed without a product-code intervention.
- **Wave 5 gate:** orchestrator reran both repository gates after workers settled: `GATR exit=0 dur=24.6s errors=0 warnings=0 adapter=generic tag=blitz-wave5-final-rust` and `GATR exit=0 dur=5.8s errors=0 warnings=0 adapter=generic tag=blitz-wave5-final-frontend` (14 files, 124 tests).
- **Pre-Wave 6 test ownership:** #205's actual History render path is `HistoryEntryItem` → `ToolContent`, not only the stale tracker file list. Added `HistoryEntryItem.tsx` and a focused colocated test path; no Wave 6 conflict was introduced.
- **Wave 6 / #95 ownership grant:** added `crates/wisphive_hook/Cargo.toml` so descriptor-level `O_NOFOLLOW` and effective-UID checks can use the workspace `libc` dependency; no neighboring worker owns the manifest.
- **Wave 6 / #95 integration-fixture grant:** daemon integration fixtures must now create an active mode file before sending hook requests through the new daemon-side gate. Added `crates/wisphive_daemon/tests/server_integration.rs`; no other blitz issue owns that path.
- **Wave 6 adversarial review repair:** reopened all three stories before commit. #95's pre-stdin failure path emitted a hard-coded PreToolUse response for other hook events; it must fail closed generically with exit 2. #205's initial regression proved only two named blocks rather than the invariant that every History code block has an exact-copy control. #303 marked undelivered signals as killed, exposed a waiter/raw-PID reuse race, and lacked the required manager-level lifecycle test. Original owners received narrow repair tasks and must rerun their full gates.
- **Wave 6 / #95 process-test grant:** added `crates/wisphive_hook/tests/mode_failclosed.rs` to prove a missing-mode PermissionRequest exits 2 with stderr and no mismatched PreToolUse stdout; no neighboring worker owns the path.
- **Wave 6 / #303 safe-alternative pivot:** nested signal-race review demonstrated that raw-PID SIGKILL cannot be made truthful and reuse-safe with the current `portable-pty` ownership model, including zombie and inherited `SIGCHLD=SIG_IGN` cases. Directed the story to its explicit alternate acceptance: remove the unsupported kill distinction and expose one honest close behavior. Added the CLI parser plus all Rust/TypeScript TermClose senders and protocol types; every later owner was inactive, and #95 had already settled its shared server edits.
- **Wave 6 / #303 documentation re-review:** final review found the blitz warning and roadmap still described terminal-close work as pending. Updated both to the accepted single-close outcome and added `docs/ROADMAP.md` to ownership.
- **Wave 6 gate:** after all adversarial repairs and documentation reconciliation, the orchestrator reran both repository gates: `GATR exit=0 dur=25.8s errors=0 warnings=0 adapter=generic tag=blitz-wave6-repair-final-rust` and `GATR exit=0 dur=6.7s errors=0 warnings=0 adapter=generic tag=blitz-wave6-repair-final-frontend` (15 files, 125 tests). Final re-review: PASS.
- **Wave 7 / #307 shared-boundary repair:** independent review found the CLI-only prevalidation left the daemon/web `InstallHooks` path able to reach the same `hooks`-object `expect`. Reopened #307 and added `crates/wisphive_daemon/src/hook_install.rs`; malformed shapes must return `Result` from the shared library before either caller mutates files.
- **Wave 7 / #262 pre-existing-file repair:** independent security review found the fresh-file permission pass happened too late for existing WAL/SHM files and did not validate parent ownership/writeability or descriptor file type/effective UID. Reopened #262 for secure-parent validation, descriptor preflight of main/WAL/SHM before SQLite consumes them, postflight of newly-created sidecars, and loose-mode/symlink/nonregular regressions.
- **Wave 7 gate:** after both review repairs, the orchestrator reran both repository gates: `GATR exit=0 dur=34.0s errors=0 warnings=0 adapter=generic tag=blitz-wave7-final-rust` and `GATR exit=0 dur=6.6s errors=0 warnings=0 adapter=generic tag=blitz-wave7-final-frontend` (15 files, 125 tests). Final re-reviews for #262 and #307: PASS; #306 review: PASS.
- **Pre-Wave 8 test ownership:** added the existing TUI snapshot harness to #369 and `Terminals.test.tsx` to #375 so both behavioral changes have runtime evidence; the additions are conflict-free within the wave.
- **Wave 8 / #375 hook-test grant:** added `useWisphive.test.ts` to prove the shared handler registry drops live chunks in replay mode and a stale same-ID unregister cannot remove the replacement replay handler; no Wave 8 neighbor owns the path.
- **Wave 8 / #367 bounded-log repair:** cross-review confirmed framing/order/EOF behavior but found `%serde_json::Error` could echo an unbounded, newline-bearing unknown type into `tui.log`. Reopened #367 to log only bounded error category/location/frame-size metadata and add an adversarial unknown-tag regression.
- **Wave 8 / #375 direction-isolation repair:** cross-review confirmed detach ordering and cleanup but found replay registrations still accepted live catchup and live registrations still accepted replay chunks. Reopened #375 so live handlers accept only chunk/catchup, replay handlers accept only replay chunks, with symmetric routing regressions.
- **Wave 8 gate:** after both cross-review repairs, the orchestrator reran both repository gates: `GATR exit=0 dur=26.1s errors=0 warnings=0 adapter=generic tag=blitz-wave8-final-rust` and `GATR exit=0 dur=6.3s errors=0 warnings=0 adapter=generic tag=blitz-wave8-final-frontend` (15 files, 129 tests). Final cross-reviews for #367, #369, and #375: PASS.
- **Pre-Wave 9 test ownership:** added the existing `useWisphive.test.ts` surface to #376 so the missing-Notification hidden-tab crash is exercised without creating a parallel harness; no Wave 9 conflict was introduced.
- **Wave 9 / #407 HTTP-test grant:** added `crates/wisphive_web/src/http_tests.rs` for deterministic web-vs-daemon concurrent disjoint-update evidence; its later #281 owner is inactive.
- **Wave 9 / #407 runtime-doc grant:** the new durable `~/.wisphive/config.json.lock` falls under the repository's runtime-file documentation rule, so #407 also owns `AGENTS.md` for flock/permission/lifetime semantics.
- **Wave 9 / #407 TUI-writer grant:** self-audit found a fourth lossy writer in `wisphive_tui::App::save_config`. Added `app.rs`, the TUI manifest, and `Cargo.lock` so it can use the same daemon config primitive; later #373/#261 owners are inactive and must preserve this serialization.
- **Wave 9 / #407 async TUI routing grant:** direct flock I/O in the TUI select loop would block the runtime. Added `wisphive_tui/src/input.rs` to emit a save action and CLI `commands/tui.rs` to execute the shared mutation in `spawn_blocking` with visible failure handling; later #374/#377 are inactive.
- **Wave 9 / #407 authority-branch repair:** adversarial cross-review found `persist_auto_approve` checked for `config.json` before locking, so two legacy writers could still lose updates and a concurrent config creator could make the daemon write the now-nonauthoritative legacy file. Reopened #407 to take the same config lock before selecting config versus legacy, hold it through the chosen RMW without recursive locking, and add deterministic dual-legacy/config-creation plus concurrent same-tool TUI pattern tests.
- **Wave 9 gate:** after the #407 authority repair, the orchestrator reran both repository gates: `GATR exit=0 dur=43.5s errors=0 warnings=0 adapter=generic tag=blitz-wave9-final-rust` and `GATR exit=0 dur=6.0s errors=0 warnings=0 adapter=generic tag=blitz-wave9-final-frontend` (15 files, 130 tests). Final cross-reviews for #376, #407, and #412: PASS.
- **Pre-Wave 10 test/storage ownership:** added `Terminals.test.tsx` to #488. Expanded #504 to the HTTP regression surface plus web-auth state implementation/tests because compare-and-swap persistence belongs at the database boundary; all later web-auth owners are inactive.
- **Wave 4 / #111 lint repair:** its first frontend pass caught a React hooks rule violation from assigning the actions ref during render. The worker moved the ref refresh into `useLayoutEffect`; both required gates then passed.
- **Wave 4 gate:** orchestrator reran both repository gates after all workers settled: `GATR exit=0 dur=23.5s errors=0 warnings=0 adapter=generic tag=blitz-wave4-final-rust` and `GATR exit=0 dur=5.8s errors=0 warnings=0 adapter=generic tag=blitz-wave4-final-frontend` (14 files, 122 tests).

## Outcomes

- **Wave 1 — 2026-07-11T21:24Z–2026-07-11T22:49Z — 3 workers.**
  - **#94 closed:** managed spawns now validate at the process boundary, enter a bounded/persistent human-review lifecycle, honor only fully revalidated edited requests, fail closed on every timeout/restart/persistence/action path, require sudo-fresh web approval, and return an honest queued CLI acknowledgment. Independent security review: PASS.
  - **#105 closed:** every inbound daemon WebSocket variant is validated into a wrapped discriminated union; recursive JSON values are preserved safely (including inert `__proto__` keys), malformed frames log without poisoning later valid frames, and Rust enum/UUID/time/integer domains are checked. Independent cross-language review: PASS.
  - **#106 closed:** component/queue tool inputs use one explicit parser with no unsafe assertions; malformed AskUserQuestion payloads render a stable fallback. Runtime evidence: React/jsdom component and queue flows; 117 frontend tests green at final gate.
  - **Follow-ups filed:** #510 (Claude hook/daemon timeout invariant) and #511 (effective Codex multi-source hook inventory).
  - **Commit:** `2102fb7` — `fix(control-plane): harden spawn and web trust boundaries`.
- **Wave 2 — 2026-07-11T22:51Z–2026-07-11T22:59Z — 3 workers.**
  - **#294 closed:** agent CLI startup now deterministically drains Welcome/AgentsSnapshot/QueueSnapshot before reading list/start/stop responses; fake Unix-daemon regression proves AgentList is returned.
  - **#295 closed:** one successful sudo reauth drains the deduplicated batch of still-queued, tool-matching approvals exactly once; cancel abandons the whole batch; parsed-WebSocket Vitest covers two gates.
  - **#362 closed:** shared character-aware truncation replaces both panic-prone UTF-8 byte slices; real Ratatui queue and session renders cover emoji at the former split boundaries.
  - **Commit:** `b69c4b1` — `fix(clients): harden agent replies and approval UI`.
- **Wave 3 — 2026-07-11T23:00Z–2026-07-11T23:09Z — 3 workers.**
  - **#296 closed:** validated terminal live/catch-up/replay frames dispatch imperative xterm callbacks before the React state updater; StrictMode runtime test proves exactly one callback per frame. Related follow-up seam remains itr#114.
  - **#411 closed:** project audit now reuses the daemon's precise Wisphive hook-command matcher; regression fixtures prove paths that merely contain `wisphive` are not classified as installed.
  - **#86 closed:** hook and daemon boundaries reject traversal-capable agent IDs and malformed terminal UUIDs before marker/database work; marker cleanup revalidates stored IDs and project strings remain opaque metadata.
  - **Commit:** `3851024` — `fix(runtime): validate hook identity and terminal dispatch`.
- **Wave 4 — 2026-07-11T23:11Z–2026-07-11T23:16Z — 3 workers.**
  - **#99 closed:** a daemon-wide 256-permit semaphore bounds live socket handlers; over-cap clients receive a protocol error without a handler/context allocation and capacity recovers when permits drop.
  - **#111 closed:** one stable keyboard listener reads the latest actions through a layout-ref refresh; rerender tests prove no listener churn and no stale callback after action changes.
  - **#116 closed:** question and option keys derive from canonical content with duplicate occurrence disambiguation; rerender/click tests prove DOM identity follows reordered content and current request callbacks.
  - **Commit:** `d269d50` — `fix(runtime): cap connections and stabilize UI identity`.
- **Wave 5 — 2026-07-11T23:19Z–2026-07-11T23:25Z — 3 workers.**
  - **#114 closed:** title updates run in an effect, one validated decision frame creates one notification outside state updaters, permission prompting waits for a real click, and redundant socket error closing is removed. StrictMode/gesture tests pass.
  - **#503 closed:** readable-but-unparseable TLS cert/key material remains fail-closed with actionable path-specific remediation; tests prove no regeneration and no file mutation.
  - **#96 failed-skipped:** no edits; remains open with Fable-review tags and the PO-approved design diagnostic.
  - **Commit:** `9886856` — `fix(web): isolate notifications and TLS corruption`.
- **Wave 6 — 2026-07-11T23:27Z–2026-07-12T00:10Z — 3 workers + adversarial repair reviews.**
  - **#95 closed:** mode reads now use descriptor-based no-follow validation of the effective owner, exact `0700` state directory, exact `0600` regular file, and bounded contents; writes are atomic and owner-only; the daemon independently requires secure active mode before hook decisions or managed spawns. Missing/unsafe pre-parse state emits stderr and exits 2 without fabricating an event response. Real-binary PermissionRequest and daemon-boundary regressions pass.
  - **#205 closed:** the existing shared History renderer already supplied Copy controls; focused runtime coverage now proves every rendered History code block has a colocated control and copies exact Command/Tool Result content, including stdout, stderr, and raw output.
  - **#303 closed:** after adversarial review rejected an unsafe raw-PID force/graceful implementation, the accepted alternate removed the unsupported `kill` distinction across Rust/TypeScript protocol, CLI, TUI, web, and daemon. Legacy payloads with the retired field still decode, and a real PTY manager test proves the single close behavior.
  - **Commit:** `b6a1551` — `fix(runtime): secure mode and simplify terminal close`.
- **Wave 7 — 2026-07-12T00:11Z–2026-07-12T00:38Z — 3 workers + adversarial repair reviews.**
  - **#306 closed:** both daemon-web start and standalone web-serve hosts are validated by Clap before side effects and invalid input exits 2 with an actionable error; runtime parsing also propagates errors if called outside Clap. Regression tests cover both affected commands and valid IPv4/localhost behavior remains.
  - **#307 closed:** the shared Claude/Codex installer now parses and validates both user configs before writing either one, returns contextual errors for malformed roots or non-object hooks, and is used by CLI plus daemon/web paths. Tests cover array/string/number/bool hooks, malformed document roots, unchanged rejected files, and cross-agent no-partial-mutation behavior.
  - **#262 closed:** daemon and CLI database opens hold a no-follow, effective-user-owned, non-group/world-writable parent descriptor; preflight existing main/WAL/SHM entries by descriptor, securely create a missing main file, enforce exact `0600`, and postflight parent identity plus newly-created sidecars. Six adversarial tests cover fresh and `06777` files, symlinks, nonregular entries, and unsafe parents.
  - **Commit:** `b8a39f8` — `fix(runtime): harden startup configuration boundaries`.
- **Wave 8 — 2026-07-12T00:40Z–2026-07-12T01:03Z — 3 workers + cross-review repairs.**
  - **#367 closed:** the Rust TUI connection consumes and skips unknown or malformed complete frames, preserves later known-message order, returns `None` only on EOF, and propagates real socket I/O failures. Diagnostics expose only fixed decode categories and numeric location/frame-size metadata; a 64 KiB newline-bearing unknown tag proves bounded, injection-safe logging and continued ordered processing.
  - **#369 closed:** all nine selectable TUI lists now use `ListState` and stateful rendering; a constrained-height approval-queue regression proves the selected 24th request scrolls into view while the first row leaves the viewport. Existing visual snapshots and Unicode coverage remain green.
  - **#375 closed:** live terminal forwarders detach before every reattach/replay (including same ID) and exactly once on live-view unmount; replay-only selections do not detach. Handler identity protects replacements from stale cleanup, and routing is symmetric: live accepts chunk/catchup only, replay accepts replay chunks only. StrictMode and all-frame tests pass.
  - **Commit:** `dade8b5` — `fix(clients): preserve TUI and terminal stream state`.
- **Wave 9 — 2026-07-12T01:04Z–2026-07-12T01:54Z — 3 workers + adversarial repair reviews.**
  - **#376 closed:** every Notification API access is guarded at use time, including the deferred click callback. A hidden-document test deletes the API, processes a new decision without crashing or notifying, then proves a later resolution frame still clears state on the same socket.
  - **#407 closed:** every daemon, web, CLI, and TUI config mutation now uses one owner-only persistent `config.json.lock` transaction covering authority selection, raw-JSON read, precise mutation, and atomic rename. TUI saves execute off-runtime as per-setting/per-pattern mutations; typed CLI updates preserve future nested fields. Deterministic tests cover 16 concurrent tools, HTTP-vs-daemon, dual legacy writers, concurrent config creation/recheck, same-tool allow/deny patterns, corrupt inputs, symlinked locks, and panic release.
  - **#412 closed:** doctor reuses the exact hook audit to distinguish full installation from PreToolUse-only gating and names every missing Claude/Codex event. Agent preflight stays fail-closed when the minimum gate is absent, warns on partial coverage, and remains silent for full installs; both-agent absent/malformed/partial/full fixtures pass.
  - **Commit:** `a3279b0` — `fix(runtime): serialize config and audit hook coverage`.

## Quarantine triage notes

- **#96:** PO decision on 2026-07-11: mark for Fable review and remove from the blitz plan. No partial hardening should land under this story because it would not meet the stated same-UID provenance acceptance.
