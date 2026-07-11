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
  - Files: `crates/wisphive_hook/src/main.rs`, `crates/wisphive_cli/src/commands/hooks.rs`, `crates/wisphive_daemon/src/config.rs`, `crates/wisphive_daemon/src/server.rs`, `AGENTS.md`
- **#205 — Add Copy controls to History details**
  - Files: `crates/wisphive_web/frontend/src/components/DetailView.tsx`, `crates/wisphive_web/frontend/src/components/ToolContent.tsx`, `crates/wisphive_web/frontend/src/components/CopyButton.tsx`
- **#303 — Honor the terminal-close kill flag**
  - Files: `crates/wisphive_daemon/src/terminal.rs`, `crates/wisphive_protocol/src/wire.rs`, `crates/wisphive_cli/src/commands/term.rs`

### Wave 7

- **#306 — Reject invalid daemon/web host values**
  - Files: `crates/wisphive_cli/src/main.rs`
- **#307 — Handle non-object Claude hook settings without panic**
  - Files: `crates/wisphive_cli/src/commands/hooks.rs`
- **#262 — Enforce mode 0600 on SQLite database sidecars**
  - Files: `crates/wisphive_daemon/src/state/mod.rs`, `AGENTS.md`

### Wave 8

- **#367 — Skip unknown server messages without disconnecting**
  - Files: `crates/wisphive_tui/src/connection.rs`, `crates/wisphive_cli/src/commands/tui.rs`, `crates/wisphive_protocol/src/wire.rs`
- **#369 — Scroll selected TUI list rows into view**
  - Files: `crates/wisphive_tui/src/ui.rs`
- **#375 — Detach web terminal streams on replay/view changes**
  - Files: `crates/wisphive_web/frontend/src/components/Terminals.tsx`, `crates/wisphive_web/frontend/src/hooks/useWisphive.ts`

### Wave 9

- **#376 — Guard browsers without the Notification API**
  - Files: `crates/wisphive_web/frontend/src/hooks/useWisphive.ts`
- **#407 — Serialize every config.json writer**
  - Files: `crates/wisphive_daemon/src/config.rs`, `crates/wisphive_daemon/src/server.rs`, `crates/wisphive_web/src/lib.rs`, `crates/wisphive_cli/src/commands/config.rs`
- **#412 — Detect partial hook installations in doctor/agent preflight**
  - Files: `crates/wisphive_cli/src/commands/doctor.rs`, `crates/wisphive_cli/src/commands/agent.rs`

### Wave 10

- **#470 — Ignore interleaved broadcasts in CLI agent commands**
  - Files: `crates/wisphive_cli/src/commands/agent.rs`
- **#488 — Add dialog semantics/inert background to mobile terminals**
  - Files: `crates/wisphive_web/frontend/src/components/Terminals.tsx`
- **#504 — Persist upgraded Argon2 hashes after successful login**
  - Files: `crates/wisphive_web/src/lib.rs`, `crates/wisphive_web/src/auth.rs`

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

- `crates/wisphive_daemon/src/server.rs`: #94 → #294 → #86 → #99 → #95 → #407 → #97 → #101 → #102 → #336 → #409 → #410.
- `crates/wisphive_web/frontend/src/hooks/useWisphive.ts`: #105 → #295 → #296 → #114 → #375 → #376 → #118.
- `crates/wisphive_web/src/lib.rs`: #407 → #504 → #495 → #408 → #277 → #281.
- `crates/wisphive_cli/src/main.rs`: #306 → #348 → #371 → #277 → #318.
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
- `crates/wisphive_tui/src/input.rs`: #374 → #377.
- `crates/wisphive_web/frontend/src/App.tsx` and `useKeyboard.ts`: #111 → #378.
- `crates/wisphive_web/frontend/src/components/Terminals.tsx`: #375 → #488.
- `Cargo.toml`: #261 → #318.
- `ToolContent.tsx`: #106 → #205.
- `crates/wisphive_daemon/src/queue.rs`: #94 → #97.
- `crates/wisphive_daemon/src/state/decisions.rs`: #94 → #91.
- `AGENTS.md`: #95 → #262.

No wave contains a shared owned file. Co-located tests inherit the same ownership chain as their source; a worker must request an ownership expansion before touching any additional test file.

## Semantic warnings

- **#407 / #495 overlap:** #407 is the superset and lands first. In Wave 11, #495 should inspect the landed behavior; if its acceptance is already fully proven, add only missing regression evidence and close it without duplicating the lock design.
- **#105 is a frontend protocol foundation:** later `useWisphive.ts` stories must preserve its discriminated-union validation and avoid reintroducing broad casts.
- **#114 / #376 are one side-effect seam:** #114 performs the extraction first; #376 must harden the extracted notification path rather than recreating reducer-side behavior.
- **#294 / #470 share response selection:** #294 fixes startup snapshots; #470 later proves unrelated live broadcasts cannot satisfy a command reply.
- **#306 / #371 share host parsing:** #306 establishes non-zero error propagation; #371 replaces permissive parsing without weakening that behavior.
- **#303 preserves the public wire shape:** honor `kill=false` rather than deleting the protocol flag unless implementation evidence makes that impossible.
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
  - **Commit:** pending immediately after this log update.

## Quarantine triage notes

- **#96:** PO decision on 2026-07-11: mark for Fable review and remove from the blitz plan. No partial hardening should land under this story because it would not meet the stated same-UID provenance acceptance.
