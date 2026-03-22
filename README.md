# Wisphive

A multiplexed AI agent control plane that gates tool calls through a centralized daemon. When AI coding agents (Claude Code, etc.) try to execute tools like Bash, Edit, or Write, Wisphive intercepts the call and routes it to a terminal dashboard where you approve, deny, or modify it before execution proceeds.

```
Claude Code ──→ wisphive-hook ──→ Unix socket ──→ wisphive daemon ──→ TUI dashboard
                (subprocess)        (~1μs)          (queues + blocks)    (human reviews)
```

## Why

AI coding agents are powerful but opaque. They execute shell commands, edit files, and make HTTP requests — often faster than you can read. Wisphive gives you a single pane of glass to:

- **Review every tool call** before it executes (or auto-approve safe ones)
- **Monitor multiple agents** working across different projects simultaneously
- **Audit everything** — full history with tool inputs, outputs, and decisions
- **Set granular policies** — auto-approve `cargo test` but block `rm -rf`, per-tool deny/allow patterns
- **Handle all Claude Code events** — not just tool calls, but permission requests, stop signals, MCP elicitation, prompt review, and more

## Quick Start

### Install

```bash
git clone https://github.com/your-org/wisphive.git
cd wisphive
./install.sh    # builds release, installs to ~/.cargo/bin, codesigns on macOS
```

Requires Rust nightly (edition 2024). Two binaries are produced: `wisphive` (CLI/daemon/TUI) and `wisphive-hook` (Claude Code hook subprocess).

### Setup

```bash
# 1. Start the daemon (runs in foreground — use a dedicated terminal or tmux pane)
wisphive daemon start

# 2. Install hooks into your project (run from the project directory)
wisphive hooks install

# 3. Enable hooks (activates the control plane)
wisphive hooks enable

# 4. Open the TUI dashboard (another terminal)
wisphive tui
```

Now when Claude Code runs in that project, every tool call routes through Wisphive.

### Teardown

```bash
wisphive hooks disable     # instant pass-through, agents keep running
wisphive hooks uninstall   # remove hooks from .claude/settings.json
wisphive daemon stop       # stop the daemon
wisphive emergency-off     # panic button — disables everything instantly
```

## TUI Dashboard

The dashboard shows three panels: pending queue, connected agents, and projects.

### Navigation

| Key | Action |
|-----|--------|
| `j`/`k` or arrows | Navigate lists |
| `Tab` | Cycle panels |
| `Enter` | Open detail view |
| `q` | Back / quit |
| `Q` | Quit immediately |
| `/` | Filter queue |
| `h` | History browser |
| `s` | Session explorer |
| `p` | Project explorer |
| `c` | Auto-approve config |
| `n` | Spawn new agent |
| `P` | Pick project + spawn |

### Decision Actions (detail view)

| Key | Action |
|-----|--------|
| `Y` | Approve |
| `N` | Deny |
| `M` | Deny with message (Claude sees the feedback) |
| `!` | Always allow this tool |
| `E` | Edit input before approving |
| `C` | Approve with additional context |
| `?` | Defer to Claude's native prompt |

For **PermissionRequest** events, number keys `1-9` select from the dynamic suggestion list.

## Auto-Approve Configuration

Not every tool call needs human review. Wisphive has tiered auto-approve levels and content-aware rules.

### Levels

| Level | Tools Auto-Approved |
|-------|-------------------|
| `off` | Nothing — every call goes to TUI |
| `read` | Read, Grep, Glob, Agent, ToolSearch, etc. (default) |
| `write` | + Edit, Write, NotebookEdit |
| `execute` | + Bash |
| `all` | Everything — TUI is monitoring-only |

### Content-Aware Rules

Auto-approve Bash but block dangerous commands:

```json
{
  "auto_approve_level": "execute",
  "tool_rules": {
    "Bash": {
      "deny_patterns": ["rm -rf", "DROP TABLE", "mkfs"]
    }
  }
}
```

Or keep Bash gated but auto-approve specific safe commands:

```json
{
  "auto_approve_level": "write",
  "tool_rules": {
    "Bash": {
      "allow_patterns": ["cargo test", "cargo build", "git status"]
    }
  }
}
```

Patterns are case-insensitive substrings matched against the command text. Configure via `wisphive tui` → `c` (config panel), or edit `~/.wisphive/config.json` directly.

## Hook Events

Wisphive handles all blocking Claude Code hook events:

| Event | TUI Behavior |
|-------|-------------|
| **PreToolUse** | Approve/deny/edit tool calls |
| **PostToolUse** | Captures execution results for audit |
| **PermissionRequest** | Dynamic suggestion list — select permissions to grant |
| **Elicitation** | MCP server requests user input (form/URL) |
| **Stop / SubagentStop** | Agent wants to stop — let it or tell it to continue |
| **UserPromptSubmit** | Review/block user prompts |
| **ConfigChange** | Veto config changes |
| **TeammateIdle** | Continue teammate with feedback, or stop |
| **TaskCompleted** | Accept or reject with feedback |

## Event Logging & Audit

Every tool call is logged — not just ones reviewed by a human.

- **Auto-approved calls** → appended to `~/.wisphive/events.jsonl` (~1μs, zero daemon coupling)
- **Human-reviewed decisions** → stored in SQLite `decision_log`
- **PostToolUse results** → correlated by `tool_use_id` and attached to the matching log entry
- **Daemon ingests** `events.jsonl` in the background and batch-inserts to SQLite

Browse history in the TUI (`h`), search with `/`, filter by agent (`f`), or query programmatically:

```bash
wisphive history list                    # recent decisions
wisphive history search "cargo test"     # full-text search
tail -f ~/.wisphive/events.jsonl | jq    # live event stream
```

### Retention Policy

The `decision_log` table is pruned automatically:

| Setting | Default | Description |
|---------|---------|-------------|
| `retention_max_rows` | 50,000 | Max rows in SQLite |
| `retention_max_age_days` | 30 | Max age before archival |

Pruned entries are archived to `~/.wisphive/logs/decision_log.jsonl` before deletion. Runs on daemon startup and hourly.

## CLI Reference

```
wisphive daemon start              # start background daemon
wisphive daemon stop               # stop daemon
wisphive daemon status             # check if daemon is running
wisphive tui                       # open TUI dashboard
wisphive hooks install             # install hooks in current project
wisphive hooks uninstall           # remove hooks from current project
wisphive hooks enable              # set mode to active
wisphive hooks disable             # set mode to off (pass-through)
wisphive hooks status              # show hook/daemon status
wisphive emergency-off             # kill switch — disables everything
wisphive config list               # show all config
wisphive config set <key> <value>  # set a config value
wisphive doctor                    # diagnose setup issues
wisphive agent spawn               # spawn a new Claude Code agent
wisphive history list              # browse audit history
wisphive history search <query>    # search history
```

## Configuration

All config lives in `~/.wisphive/config.json`:

```json
{
  "notifications": true,
  "hook_timeout_secs": 3600,
  "agent_timeout_secs": 300,
  "auto_approve_level": "read",
  "auto_approve_add": ["WebSearch"],
  "auto_approve_remove": [],
  "auto_approve_stop": false,
  "tool_rules": {},
  "retention_max_rows": 50000,
  "retention_max_age_days": 30
}
```

## Runtime Files

All under `~/.wisphive/`:

| File | Purpose |
|------|---------|
| `wisphive.sock` | Unix domain socket (daemon ↔ hooks/TUI) |
| `wisphive.pid` | Daemon PID file |
| `wisphive.db` | SQLite state + audit database |
| `mode` | `active` or `off` (global kill switch) |
| `config.json` | User configuration |
| `events.jsonl` | Auto-approved tool call event stream |
| `logs/decision_log.jsonl` | Archived audit entries |
| `logs/daemon.log` | Daemon log |
| `sessions/` | Per-session marker files |

## Architecture

Six workspace crates:

```
wisphive_protocol   ← shared types + wire protocol (all crates depend on this)
wisphive_hook       ← lightweight hook binary (runs as Claude Code subprocess)
wisphive_daemon     ← async Tokio server (queue, SQLite, notifications, event ingest)
wisphive_tui        ← Ratatui terminal dashboard
wisphive_cli        ← Clap CLI (ties everything together)
wisphive_adapters   ← agent adapter trait + implementations
```

### Key Design Decisions

- **Fail-open** — hook errors always approve (exit 0) to avoid blocking agents
- **Blocking via oneshot channels** — each hook blocks on a `tokio::sync::oneshot` until a human responds or timeout (1 hour, defaults to approve)
- **JSONL for the hot path** — auto-approved tools write to `events.jsonl` via `O_APPEND` (~1μs) instead of connecting to the daemon
- **`tool_use_id` correlation** — Claude Code's unique call ID enables deterministic pre/post matching instead of fuzzy agent+tool+recency
- **SQLite WAL + performance pragmas** — `synchronous=NORMAL`, 64MB cache, 5s busy timeout for concurrent reads during TUI browsing
- **Passive notifications** — macOS `terminal-notifier` (click-to-focus) or `osascript`; Linux `notify-send`. Informational only — decisions are made in the TUI

## Building from Source

```bash
cargo build --workspace           # debug build
cargo build --release             # release build
cargo test --workspace            # run all tests
cargo clippy --workspace          # lint
```

Requires Rust nightly or a recent stable toolchain that supports edition 2024.

## License

MIT
