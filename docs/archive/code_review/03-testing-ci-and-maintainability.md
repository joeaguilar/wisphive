# Testing, CI, and Maintainability Findings

## Medium: CI currently fails because frontend lint fails

Affected code:

- `.github/workflows/ci.yml`
- `crates/wisphive_web/frontend`

Evidence:

- CI runs `npm run lint`.
- Local `npm run lint` fails with 7 errors.

Impact:

- Pull requests will fail the frontend job even though `npm run build` succeeds.
- Developers may spend time debugging CI because the local production build does not surface the same failures.

Recommended fix:

- Fix the lint errors listed in `02-web-frontend.md`.
- Optionally run lint before build in CI for faster feedback.

Test suggestion:

- Keep `npm run lint` as a required CI step after the current errors are fixed.

## Medium: Integration tests cover happy-path socket behavior but not recovery semantics

Affected code:

- `crates/wisphive_daemon/tests/server_integration.rs`
- `crates/wisphive_daemon/src/state.rs`
- `crates/wisphive_daemon/src/server.rs`

Evidence:

- Tests cover handshakes, queue snapshots, approvals, denials, protocol versions, and sudo-gate behavior.
- There are no tests that restart the daemon with rows already present in `pending_decisions`.
- There are no tests for the Ask/defer persistence path.

Impact:

- Persistence comments say pending decisions support crash recovery, but tests do not define the recovery contract.
- Stale-pending bugs can survive because most tests exercise only the in-memory queue.

Recommended fix:

- Add explicit tests for daemon restart behavior with pending rows.
- Add tests for Ask/defer cleanup.
- If the intended behavior is fail-open/no recovery, encode that in tests by verifying stale pending rows are pruned or ignored intentionally.

Test suggestion:

- Seed `pending_decisions`, start a fresh server, connect a TUI, and assert the expected snapshot.
- Send Ask for a pending request, then query SQLite and assert the row is gone or logged as ask.

## Medium: No tests cover the CLI agent client framing

Affected code:

- `crates/wisphive_cli/src/commands/agent.rs`
- `crates/wisphive_daemon/src/server.rs`

Evidence:

- The term CLI drains `Welcome + AgentsSnapshot + QueueSnapshot`.
- The agent CLI drains fewer messages and is therefore likely broken.
- Existing CLI tests focus heavily on hooks and web password helpers, not agent socket command framing.

Impact:

- Protocol changes in daemon startup snapshots can silently break non-TUI clients.
- The bug can pass Rust unit/integration tests because the daemon tests use direct TUI/hook clients, not the CLI agent helper.

Recommended fix:

- Extract a reusable TUI-like connection helper and test it once.
- Add fake-daemon tests for agent CLI commands.

Test suggestion:

- Fake daemon sends the normal handshake prelude and then `AgentList`; assert the CLI helper returns `AgentList`.

## Low: Documentation and code disagree on some shipped behavior

Affected code:

- `CLAUDE.md`
- `crates/wisphive_daemon/src/terminal.rs`
- `crates/wisphive_protocol/src/wire.rs`
- `crates/wisphive_hook/src/main.rs`

Evidence:

- The protocol comments describe `TermClose.kill` as meaningful, while the implementation ignores it.
- The hook documentation describes PermissionRequest response behavior differently from the current nested JSON shape.
- The state layer describes pending-decision persistence as crash recovery, while the daemon does not restore it.

Impact:

- Future contributors can implement against the docs and create incompatible behavior.
- Reviewers may miss bugs because the comments assert that the safety property already exists.

Recommended fix:

- Update docs/comments in the same PR as behavior fixes.
- Where behavior is intentionally not implemented, say so explicitly rather than describing the planned behavior as current behavior.

Test suggestion:

- Add regression tests that encode the documented behavior for `TermClose`, PermissionRequest responses, and pending recovery/cleanup.

## Low: Frontend protocol types are manually mirrored and already stale

Affected code:

- `crates/wisphive_protocol/src/types.rs`
- `crates/wisphive_protocol/src/wire.rs`
- `crates/wisphive_web/frontend/src/types/protocol.ts`

Evidence:

- Rust `ProjectSummary` includes fields that the TypeScript mirror omits.
- The TypeScript `ClientMessage` union omits some protocol variants that can still arrive through the web bridge if sent manually.

Impact:

- TypeScript can compile while the UI ignores newer backend fields.
- Manual protocol drift makes frontend behavior dependent on untyped assumptions.

Recommended fix:

- Generate TypeScript protocol types from Rust with a tool such as `ts-rs`, or add strict schema fixtures shared between Rust and TypeScript.
- At minimum, add a documented checklist requiring TypeScript mirror updates whenever protocol structs change.

Test suggestion:

- Add JSON fixtures for representative `ServerMessage` variants and validate them against frontend types/handlers.
