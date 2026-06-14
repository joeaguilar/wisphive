# Wisphive Code Review Summary

Review date: 2026-05-04

Scope reviewed:

- Rust workspace: `wisphive_protocol`, `wisphive_daemon`, `wisphive_hook`, `wisphive_tui`, `wisphive_cli`, `wisphive_adapters`, `wisphive_web`
- React frontend: `crates/wisphive_web/frontend/src`
- Build, lint, and CI configuration

Validation commands run:

- `cargo check --workspace` - passed
- `cargo clippy --workspace -- -D warnings` - passed
- `cargo test --workspace` - passed when rerun outside the sandbox; the first sandboxed run failed because the integration tests could not bind Unix sockets
- `npm run build` in `crates/wisphive_web/frontend` - passed, with a large bundle warning
- `npm run lint` in `crates/wisphive_web/frontend` - failed with 7 errors

## Issue Index

### High

1. CLI agent commands read the wrong daemon response after the TUI handshake.
   See `01-daemon-hook-cli.md`.
2. PermissionRequest approvals appear to emit the wrong Claude hook response shape.
   See `01-daemon-hook-cli.md`.
3. Spawned Claude agents pipe stdout/stderr but never drain them, so chatty agents can deadlock.
   See `01-daemon-hook-cli.md`.
4. The frontend markdown renderer can inject attacker-controlled HTML attributes.
   See `02-web-frontend.md`.
5. Terminal output side effects run inside a React state updater, which can duplicate output in React strict/concurrent paths.
   See `02-web-frontend.md`.
6. Multiple sudo-gated web approvals can strand older approvals after reauth.
   See `02-web-frontend.md`.

### Medium

1. `ApprovePermission` accepts an invalid suggestion index and resolves the request anyway.
2. Pending decisions are persisted but never loaded back into the live queue.
3. `Ask`/defer decisions are removed from memory but left in `pending_decisions`.
4. `permission_suggestions` has a database column but is never written by `persist_pending`.
5. Auto-approved events written before daemon startup are skipped by the ingest tailer.
6. Auto-approved `PostToolUse` results can be permanently lost when they arrive before JSONL ingest catches up.
7. `term close --kill=false` is ignored; all terminal closes kill the child.
8. Frontend lint is currently failing in CI.
9. Project live-status data is emitted by Rust but omitted/misused in frontend types and UI.

### Low

1. Red and Local LLM adapters return success from stubbed `start`/`respond` methods.
2. The frontend has no automated unit/component tests for high-risk protocol and rendering logic.
3. The production frontend bundle exceeds Vite's default 500 kB warning threshold.

## Notes

The review found no Rust compiler or Clippy failures. The most actionable backend bugs are not type-system issues; they are protocol sequencing, persistence semantics, and runtime I/O behavior.

The requested output path is `docs/code_reivew`, matching the spelling in the request.
