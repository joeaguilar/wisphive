# FAQ

## Where does Wisphive save its database?

Wisphive stores its SQLite state and audit database at `~/.wisphive/wisphive.db`.

It runs in WAL mode for crash recovery — pending decisions persist across restarts, and every resolution (approve/deny, who, when) is appended to the audit log.

Related runtime files in the same directory:

- `wisphive.sock` — Unix domain socket the daemon listens on
- `wisphive.pid` — daemon PID file
- `mode` — global kill switch (`active` or `off`)
- `auto-approve.json` — tool names that skip daemon review
- `web.cert.pem` / `web.key.pem` — self-signed TLS cert/key for the web UI (key mode `0600`)
- `web.cert.meta.json` / `web.cert.lock` — TLS rotation metadata and flock

## Where does Wisphive's Unix socket live?

The daemon listens on `~/.wisphive/wisphive.sock`.

All clients connect there:

- `wisphive-hook` subprocesses (spawned by Claude Code / Codex per tool call) — ephemeral; send a Hello + `DecisionRequest`, block for the response, exit.
- The TUI (`wisphive tui`) — long-lived streaming client; sends a Hello, receives a `QueueSnapshot`, then exchanges commands and events over the same connection.
- The web bridge (`wisphive web serve` or `wisphive daemon start --web`) — connects on behalf of browser clients and relays over `/ws`.

The wire protocol is newline-delimited JSON (see `wisphive_protocol`). If the daemon isn't running the socket file won't exist; `wisphive daemon start` (re)creates it, and `wisphive daemon stop` removes it.

## Where is the daemon's PID file?

The daemon writes its process ID to `~/.wisphive/wisphive.pid` on startup.

It's used by `wisphive daemon status` to report whether the daemon is running and by `wisphive daemon stop` to signal the right process. If the daemon exits cleanly the file is removed; a stale PID file left behind by a crash is detected and cleared on the next `daemon start`.

## What is the mode file?

`~/.wisphive/mode` is the global kill switch read by `wisphive-hook` on every invocation, before it does anything else.

- Contents are `active` (case- and whitespace-sensitive — the hook compares `contents.trim() == "active"`).
- Anything else — including a missing file, an empty file, or the literal `off` — disables Wisphive gating: the hook short-circuits and approves the tool call.

CLI surface:

- `wisphive hooks enable` writes `active`.
- `wisphive hooks disable` writes `off`.
- `wisphive emergency-off` is the panic button — writes `off` to halt all gating immediately.
- `wisphive hooks status` and `wisphive doctor` both read this file to report the current mode.

This is intentionally a single tiny file with no daemon dependency so the kill switch keeps working even if the daemon is wedged.

## What is `auto-approve.json`?

`~/.wisphive/auto-approve.json` is the **legacy** auto-approve file. Tool calls listed there bypass the daemon entirely — the hook approves them in-process and only logs an `auto_approved` event to `~/.wisphive/events.jsonl`.

Legacy format (still read for backwards compatibility):

```json
{ "auto_approve": ["Read", "Glob", "Grep", "LS"] }
```

The current configuration surface is `~/.wisphive/config.json`, managed via `wisphive config auto-approve …`:

- `auto_approve_level` — tiered preset, lowest to highest:
  - `off` — nothing auto-approved; every tool queues in the TUI.
  - `read` *(default)* — read-only and orchestration tools (`Read`, `Glob`, `Grep`, `LS`, `LSP`, `NotebookRead`, `WebSearch`, `WebFetch`, `Agent`, `Skill`, `ToolSearch`, `AskUserQuestion`, plan/worktree enter/exit, `Task*`, `TodoRead`, `CronList`).
  - `write` — adds `Edit`, `Write`, `NotebookEdit`, `CronCreate`, `CronDelete`.
  - `execute` — adds `Bash`.
  - `all` — auto-approve everything (TUI becomes monitoring-only).
- `auto_approve_add` — extra tool names always approved on top of the level.
- `auto_approve_remove` — tool names removed from the level's defaults (queued instead).
- `tool_rules.<ToolName>.deny_patterns` / `.allow_patterns` — case-insensitive substrings matched against the tool's input. `deny_patterns` revoke approval on a normally-approved tool; `allow_patterns` add approval to a normally-queued tool.

CLI:

```bash
wisphive config auto-approve status            # show current level, add/remove, rules
wisphive config auto-approve level <off|read|write|execute|all>
wisphive config auto-approve add <ToolName>
wisphive config auto-approve remove <ToolName>
wisphive config auto-approve reset             # clear all overrides
```

Resolution order on each hook invocation: explicit `auto_approve_remove` → explicit `auto_approve_add` → tiered level → `auto-approve.json` legacy list → built-in `DEFAULT_AUTO_APPROVE`. Content-aware `deny_patterns` / `allow_patterns` are applied last to flip the decision.
