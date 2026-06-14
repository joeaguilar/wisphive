# Daemon, Hook, CLI, and Adapter Findings

## High: CLI agent commands consume the wrong daemon response

Affected code:

- `crates/wisphive_cli/src/commands/agent.rs`
- `crates/wisphive_daemon/src/server.rs`

Evidence:

- `handle_tui` sends `AgentsSnapshot` and then `QueueSnapshot` immediately after `Welcome`.
- `commands::agent::connect_to_daemon` reads only `Welcome`.
- `commands::agent::send_and_recv` drains only one extra line before sending the actual command.
- That one drained line is `AgentsSnapshot`, so the next line read after `SpawnAgent`, `ListAgents`, or `StopAgent` is usually the stale `QueueSnapshot`, not the command response.
- `crates/wisphive_cli/src/commands/term.rs` correctly drains three startup messages, which confirms the agent client is out of sync with the daemon protocol.

Impact:

- `wisphive agent start`, `wisphive agent list`, and `wisphive agent stop` can print `Unexpected response: QueueSnapshot` and miss the real daemon response.
- This can make agent management look broken even though the daemon performed the action.

Recommended fix:

- Update `commands::agent::connect_to_daemon` or `send_and_recv` to drain both `AgentsSnapshot` and `QueueSnapshot` after `Welcome`, mirroring `commands::term::connect`.
- Prefer sharing one small daemon-client handshake helper across TUI-like CLI commands.

Test suggestion:

- Add CLI-level tests with a fake daemon that sends `Welcome`, `AgentsSnapshot`, `QueueSnapshot`, then an `AgentList` response. Assert `send_and_recv(ListAgents)` returns `AgentList`.

## High: PermissionRequest responses appear to use the wrong hook JSON shape

Affected code:

- `crates/wisphive_hook/src/main.rs`
- `CLAUDE.md`

Evidence:

- `CLAUDE.md` documents PermissionRequest as accepting `updatedPermissions` in the hook response.
- `format_permission_response` emits:
  - `hookSpecificOutput.hookEventName = "PermissionRequest"`
  - `hookSpecificOutput.decision = { behavior, updatedPermissions, ... }`
- The documented PreToolUse response style puts event-specific fields directly under `hookSpecificOutput`, not under a nested `decision` object.

Impact:

- Selecting a permission suggestion in the TUI may not actually grant the permission in Claude Code.
- The operator can believe a permission was accepted while Claude ignores the response and falls back to native behavior or no-op behavior.

Recommended fix:

- Re-check Claude Code's current PermissionRequest response schema and encode exactly that structure.
- Add golden tests for `format_permission_response` using captured real hook payloads and expected stdout JSON.

Test suggestion:

- Unit test `format_permission_response` for approve/deny/ask, including a selected suggestion, and validate the output against the documented hook schema.

## High: Spawned Claude agents can deadlock because stdout/stderr are piped but never drained

Affected code:

- `crates/wisphive_daemon/src/process_registry.rs`

Evidence:

- `spawn_agent` sets `cmd.stdout(Stdio::piped())` and `cmd.stderr(Stdio::piped())`.
- The resulting `Child` is stored, but no task reads either pipe.

Impact:

- A verbose `claude -p` process can fill the OS pipe buffer and block forever.
- `reap_exited` will keep seeing the child as running, and shutdown/stop can hang longer than expected.
- The user loses the spawned agent's output anyway, so piping does not currently provide value.

Recommended fix:

- If output is not needed, use `Stdio::null()` or inherit stderr for operator visibility.
- If output is needed, spawn drain tasks that persist or broadcast stdout/stderr, and ensure those tasks are cleaned up when the child exits.

Test suggestion:

- Spawn a test command that writes more than a pipe buffer to stdout/stderr and exits. Assert the registry observes the exit instead of hanging.

## Medium: Invalid PermissionRequest suggestion indexes still approve the request

Affected code:

- `crates/wisphive_daemon/src/server.rs`

Evidence:

- The `ApprovePermission` arm looks up `permission_suggestions[suggestion_index]`.
- If the index is missing, `selected` becomes `None`.
- The daemon still constructs `RichDecision { decision: Approve, selected_permission: None, ... }`, resolves the queue item, and persists the approval.

Impact:

- A malformed or malicious local client can resolve a PermissionRequest as approved without selecting a permission.
- Depending on Claude's response semantics, this can produce a confusing no-op approval or accidentally allow behavior that should have remained pending.

Recommended fix:

- If the index is invalid, send `ServerMessage::Error` and leave the request pending.
- Apply the sudo reauth gate to `ApprovePermission` as well if PermissionRequest can grant write/execute permissions from the web bridge.

Test suggestion:

- Add an integration test that sends `ApprovePermission { suggestion_index: 999 }`, asserts an error response, and asserts the request remains in the queue.

## Medium: Pending decisions are persisted but never restored into the live queue

Affected code:

- `crates/wisphive_daemon/src/server.rs`
- `crates/wisphive_daemon/src/state.rs`
- `crates/wisphive_daemon/src/queue.rs`

Evidence:

- `handle_hook` calls `state_db.persist_pending(&req)` with a comment saying "Persist for crash recovery".
- `Server::new` creates an empty `DecisionQueue`.
- There is no `load_pending`, recovery pass, or queue seeding from `pending_decisions`.

Impact:

- The `pending_decisions` table does not provide actual crash recovery.
- After a daemon restart, persisted pending rows are invisible to TUI/web clients.
- The table can accumulate stale rows and mislead future recovery work.

Recommended fix:

- Decide the intended semantics explicitly:
  - If hooks are fail-open on daemon crash, remove or rename "crash recovery" claims and aggressively clean stale pending rows.
  - If pending decisions should be recoverable, add a recovery path that marks old requests as timed out/approved/abandoned rather than trying to recreate dead oneshot senders.

Test suggestion:

- Persist a pending row, restart/open a new server, and assert the expected recovery behavior.

## Medium: Ask/defer leaves stale rows in `pending_decisions`

Affected code:

- `crates/wisphive_daemon/src/server.rs`
- `crates/wisphive_daemon/src/state.rs`

Evidence:

- `ClientMessage::Ask` resolves the in-memory queue with `Decision::Ask`.
- The code intentionally skips `state_db.resolve_pending` because Ask decisions are not logged.
- There is no alternate delete call for the pending row.

Impact:

- Every defer leaves a stale row in SQLite.
- If pending recovery is later implemented, old asks can resurface incorrectly.
- Until then, retention does not clean these rows because they never move to `decision_log`.

Recommended fix:

- Add `StateDb::delete_pending(id)` and call it for Ask/defer.
- If Ask should be auditable, log it as an `ask` decision instead and update UI/history semantics accordingly.

Test suggestion:

- Integration test an Ask flow and assert `pending_decisions` has no row afterward.

## Medium: `permission_suggestions` are not persisted despite a database column

Affected code:

- `crates/wisphive_daemon/src/state.rs`

Evidence:

- The migration adds `pending_decisions.permission_suggestions`.
- `persist_pending` inserts only `id`, `agent_id`, `agent_type`, `project`, `tool_name`, `tool_input`, `timestamp`, `tool_use_id`, `hook_event_name`, and `terminal_session_id`.
- `permission_suggestions` is never bound.

Impact:

- Even if pending recovery is added, PermissionRequest entries will recover without their selectable suggestions.
- Debugging persisted pending rows loses the most important PermissionRequest context.

Recommended fix:

- Serialize `req.permission_suggestions` into the column.
- Include it in any pending-row read model.

Test suggestion:

- Persist a PermissionRequest with two suggestions and assert the stored row contains both.

## Medium: Auto-approved events written before daemon startup are skipped

Affected code:

- `crates/wisphive_daemon/src/event_ingest.rs`
- `crates/wisphive_hook/src/main.rs`

Evidence:

- `run_ingest` opens `events.jsonl` and immediately seeks to EOF.
- It only processes new lines after the watcher starts.
- `wisphive_hook` logs auto-approved calls to `events.jsonl` regardless of whether the daemon is currently ingesting.

Impact:

- Auto-approved calls that happen while the daemon is down are never added to `decision_log`.
- The audit trail silently loses allowed tool activity.

Recommended fix:

- Run `reimport_all` once at daemon startup before seeking to EOF, relying on existing deduplication.
- Alternatively persist and resume an offset, but startup reimport is simpler and safer for this scale.

Test suggestion:

- Write an auto-approved event to `events.jsonl`, start the daemon/ingester, and assert history contains it without manual `ReimportEvents`.

## Medium: Auto-approved `PostToolUse` results can be lost in an ingest race

Affected code:

- `crates/wisphive_daemon/src/server.rs`
- `crates/wisphive_daemon/src/event_ingest.rs`
- `crates/wisphive_daemon/src/state.rs`

Evidence:

- `ToolResult` handling calls `attach_tool_result`.
- If no decision row exists yet, it logs `no matching decision yet (may be pending ingest)` and drops the result.
- Auto-approved decisions are inserted asynchronously by the JSONL ingester.

Impact:

- Fast tools can emit `PostToolUse` before the auto-approved row is ingested.
- The tool result is then permanently missing from history even though it arrived.

Recommended fix:

- Buffer unmatched tool results by `tool_use_id` for a short TTL and retry after ingest.
- Or write auto-approved decision rows synchronously from the hook path before accepting result attachment.

Test suggestion:

- Send a `ToolResult` for a tool_use_id before calling `ingest_line`, then ingest the event and assert the result is eventually attached.

## Medium: Terminal close ignores the `kill` flag

Affected code:

- `crates/wisphive_daemon/src/terminal.rs`
- `crates/wisphive_protocol/src/wire.rs`
- `crates/wisphive_cli/src/commands/term.rs`

Evidence:

- The protocol says `TermClose { kill }` can distinguish close behavior.
- `TerminalSessionManager::close(&self, id, _kill)` ignores the flag and always calls the child killer.

Impact:

- `wisphive term close <id>` and `wisphive term close <id> --kill` behave the same.
- Users cannot request a graceful close even though the CLI/protocol suggests they can.

Recommended fix:

- Either implement a graceful path or remove/rename the flag so the protocol and CLI do not promise behavior that does not exist.

Test suggestion:

- Add a terminal-manager test that passes `kill = false` and asserts the intended graceful behavior, or assert the flag is no longer part of the public protocol.

## Low: Adapter stubs report success

Affected code:

- `crates/wisphive_adapters/src/red.rs`
- `crates/wisphive_adapters/src/local_llm.rs`

Evidence:

- `RedAdapter::start`, `RedAdapter::respond`, `LocalLlmAdapter::start`, and `LocalLlmAdapter::respond` return `Ok(())` while comments say the integrations are TODO/stubbed.

Impact:

- If these adapters are accidentally wired into user-facing code, callers see successful starts/responses even though no agent is connected and no decisions are forwarded.

Recommended fix:

- Return an explicit `Err(anyhow!("... not implemented"))` until the adapters are functional.
- Or hide stub adapters behind a feature flag.

Test suggestion:

- Add tests asserting unimplemented adapters fail loudly until real behavior lands.
