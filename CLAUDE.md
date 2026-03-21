# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

## What is Wisphive

Wisphive is a multiplexed AI agent control plane that gates tool calls from AI agents (Claude Code, Red, local LLMs) through a centralized daemon. Agents request approval before executing tools; humans review and approve/deny via a TUI dashboard. Passive OS notifications alert the user when decisions are pending.

## Build & Test Commands

```bash
cargo build --workspace          # Build all crates (debug)
cargo build --release            # Build release binaries
cargo test --workspace           # Run all tests (15 total: 12 integration, 3 unit)
cargo test -p wisphive_daemon    # Test a single crate
cargo test server_cleans_up      # Run a single test by name
cargo clippy --workspace         # Lint
./install.sh                     # Build release + install to ~/.cargo/bin + codesign on macOS
```

Two binaries are produced: `wisphive` (CLI/daemon/TUI) and `wisphive-hook` (Claude Code hook subprocess).

## Architecture

```
Claude Code → wisphive-hook (subprocess) → Unix socket → wisphive daemon → TUI + passive notification
```

Six workspace crates with clear dependency flow:

- **wisphive_protocol** — Shared types and newline-delimited JSON wire protocol. `DecisionRequest`, `Decision`, `ClientMessage`/`ServerMessage`. All other crates depend on this.
- **wisphive_daemon** — Async Tokio server on `~/.wisphive/wisphive.sock`. Accepts hook connections (blocking until decision), TUI connections (bidirectional streaming via broadcast channel), persists state to SQLite (`~/.wisphive/wisphive.db`), sends platform notifications.
- **wisphive_hook** — Lightweight binary that runs as a Claude Code `PreToolUse` hook. Three-layer decision logic: (1) check `~/.wisphive/mode` file, (2) auto-approve safe tools via `~/.wisphive/auto-approve.json`, (3) connect to daemon for human review. Exit codes: 0=approve, 2=deny, 1=error (fail-open).
- **wisphive_tui** — Ratatui terminal UI. Connects to daemon as a streaming client. Three panels: queue, agents, projects. Keys: `a`/`d` approve/deny, `A`/`D` bulk, `/` filter, Tab switch panels.
- **wisphive_cli** — Clap-based CLI (`wisphive` binary). Subcommands: `daemon {start,stop,status}`, `hooks {install,uninstall,enable,disable,status}`, `tui`, `doctor`, `emergency-off`.
- **wisphive_adapters** — `AgentAdapter` trait and implementations (ClaudeCode is hook-based/passive; Red and LocalLLM are stubs).

## Key Design Decisions

- **Fail-open**: Hook errors always approve (exit 0) to avoid blocking agents.
- **Blocking hooks via oneshot channels**: Each hook connection gets a `tokio::sync::oneshot` receiver; it blocks until a human resolves the decision or timeout (1 hour, defaults to approve).
- **Broadcast fan-out**: TUI clients subscribe to a `tokio::sync::broadcast` channel for real-time events.
- **SQLite WAL crash recovery**: Pending decisions persist to disk; audit log tracks all resolutions.
- **Passive notifications**: macOS uses `osascript display notification` (non-intrusive banner); Linux uses `notify-send`. Notifications are informational only — all tool input fields are shown so users have context when switching to the TUI to respond. Notifications do NOT resolve decisions; only the TUI does.
- **Permissions management**: `wisphive hooks install` adds Claude Code permissions (Bash, Edit, Write, NotebookEdit) to `.claude/settings.json` so Claude Code auto-allows tools that Wisphive gates (eliminates double-prompt). `wisphive hooks uninstall` removes them.

## Claude Code Hook Response Format

The `wisphive-hook` binary runs as both `PreToolUse` and `PostToolUse` hook. Claude Code supports rich JSON responses on stdout (exit 0), not just exit codes.

**PreToolUse stdin fields**: `session_id`, `tool_name`, `tool_use_id`, `tool_input`, `cwd`, `permission_mode`, `hook_event_name`, `transcript_path`

**PostToolUse additional field**: `tool_response` (the tool's execution output — NOT `tool_result`)

**Structured JSON response** (stdout, exit 0):
```json
{
  "hookSpecificOutput": {
    "hookEventName": "PreToolUse",
    "permissionDecision": "allow|deny|ask",
    "permissionDecisionReason": "text shown to Claude",
    "updatedInput": { "command": "sanitized version" },
    "additionalContext": "guidance injected into Claude's context"
  }
}
```

**Key capabilities**: `permissionDecision: "deny"` + `permissionDecisionReason` gives Claude feedback on why. `updatedInput` lets hooks sanitize tool input before execution. `"ask"` defers to Claude's native permission prompt. Stderr on exit 2 becomes Claude feedback.

**PermissionRequest hook** (separate event): fires when Claude's permission dialog would show. Input includes `permission_suggestions` array — the dynamic options the user would see in the native dialog. Each suggestion is a permission update entry (`addRules`/`setMode`/etc) with `behavior`, `destination`, `rules`. A hook can echo any suggestion back as `updatedPermissions` in its response. Does NOT fire in `-p` (print) mode.

**All Claude Code hook events** (22 total): `SessionStart`, `SessionEnd`, `InstructionsLoaded`, `UserPromptSubmit` (blocking), `PreToolUse` (blocking), `PermissionRequest` (blocking), `PostToolUse`, `PostToolUseFailure`, `Notification`, `SubagentStart`, `SubagentStop` (blocking), `Stop` (blocking), `StopFailure`, `TeammateIdle` (blocking), `TaskCompleted` (blocking), `ConfigChange` (blocking), `PreCompact`, `PostCompact`, `WorktreeCreate` (blocking), `WorktreeRemove`, `Elicitation` (blocking — MCP form/URL input), `ElicitationResult` (blocking). Wisphive currently handles: `PreToolUse`, `PostToolUse`, `PermissionRequest` (planned).

## IPC Protocol

Unix socket at `~/.wisphive/wisphive.sock`. Newline-delimited JSON. Two client types:
- **Hook**: ephemeral — sends Hello + DecisionRequest, blocks for DecisionResponse, exits.
- **TUI**: long-lived — sends Hello, receives QueueSnapshot, then bidirectional streaming of commands and events.

## Runtime Files

All under `~/.wisphive/`:
- `wisphive.sock` — Unix domain socket
- `wisphive.pid` — Daemon PID file
- `wisphive.db` — SQLite state/audit database
- `mode` — "active" or "off" (global kill switch)
- `auto-approve.json` — List of tool names that skip daemon review

## Reference Documentation

- [tui-textarea reference](docs/tui-textarea-reference.md) — API reference, key bindings, and integration notes for the TUI text editing widget

## Rust Edition

The workspace uses Rust **edition 2024**. Requires nightly or a recent stable toolchain that supports it.
