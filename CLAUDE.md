# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

## What is Wisphive

Wisphive is a multiplexed AI agent control plane that gates tool calls from AI agents (Claude Code, Codex, Red, local LLMs) through a centralized daemon. Agents request approval before executing tools; humans review and approve/deny via a TUI dashboard. Passive OS notifications alert the user when decisions are pending.

## Build & Test Commands

```bash
cargo build --workspace          # Build all crates (debug)
cargo build --release            # Build release binaries
cargo test --workspace           # Run all tests
cargo test -p wisphive_daemon    # Test a single crate
cargo test server_cleans_up      # Run a single test by name
cargo clippy --workspace -- -D warnings   # Lint (must be warning-free)
cargo fmt --all                  # Format
./install.sh                     # Build release + install to ~/.cargo/bin + codesign on macOS
```

Prefer `just <task>` for common workflows — see `justfile` for the full list (`build`, `test`, `clippy`, `daemon`, `tui`, `web`, `web-dev`, `frontend-dev`, `frontend-build`, `bootstrap`, `reinstall`, `doctor`, `off`, etc.).

Two binaries are produced: `wisphive` (CLI/daemon/TUI/web) and `wisphive-hook` (Claude Code/Codex hook subprocess).

### Frontend

The web UI lives in `crates/wisphive_web/frontend` (React 19 + TypeScript + Vite, with `xterm.js` for embedded terminals).

```bash
just frontend-install    # npm install
just frontend-dev        # Vite dev server on :5173 — pair with `just web-dev`
just frontend-build      # Production build → frontend/dist/ (embedded via rust-embed)
just frontend-lint       # ESLint
just frontend-test       # Vitest (jsdom + @testing-library/react)
```

In production (`wisphive web serve` or `wisphive daemon start --web`) the Rust binary serves the embedded `dist/` assets and the WebSocket bridge from one process. In dev (`--dev`), it serves only `/ws` and expects Vite to serve the UI.

Rendered agent/tool output is untrusted. Markdown-like text must be rendered as React nodes or through an audited sanitizer with a URL protocol allowlist; do not use `dangerouslySetInnerHTML` for agent-controlled content. Web device bearer tokens live in browser `localStorage`, so any XSS is device compromise.

### Safety / Dependency Audit

```bash
cargo deny check advisories bans sources
cd crates/wisphive_web/frontend && npm audit
cd crates/wisphive_web/frontend && npm audit --omit=dev
```

Run these when touching `Cargo.lock`, frontend lockfiles, TLS/web auth, markdown rendering, or dev-server dependencies. Treat Vite dev-server advisories as relevant even though production serves embedded assets: `just frontend-dev` exposes source files if a vulnerable dev server is bound beyond loopback.

## Architecture

```
Claude Code / Codex → wisphive-hook (subprocess) → Unix socket → wisphive daemon → TUI + web UI + passive notification
```

`wisphive hooks install` installs Claude hooks in `.claude/settings.json` and Codex hooks in `.codex/hooks.json`.

Seven workspace crates with clear dependency flow:

- **wisphive_protocol** — Shared types and newline-delimited JSON wire protocol. `DecisionRequest`, `Decision`, `ClientMessage`/`ServerMessage`, `SpawnAgentRequest`, terminal events. All other crates depend on this.
- **wisphive_daemon** — Async Tokio server on `~/.wisphive/wisphive.sock`. Accepts hook connections (blocking until decision), TUI/web connections (bidirectional streaming via broadcast channel), persists state to SQLite (`~/.wisphive/wisphive.db`), spawns headless agents via the process registry, manages `portable-pty` terminal sessions, sends platform notifications.
- **wisphive_hook** — Lightweight binary that runs as a Claude Code or Codex hook subprocess. Three-layer decision logic: (1) check `~/.wisphive/mode` file, (2) auto-approve safe tools via `~/.wisphive/auto-approve.json`, (3) connect to daemon for human review. Exit codes: 0=approve, 2=deny, 1=error (fail-open).
- **wisphive_tui** — Ratatui terminal UI. Connects to daemon as a streaming client. Panels include queue, agents, projects, terminals. Keys: `a`/`d` approve/deny, `A`/`D` bulk, `/` filter, Tab switch panels.
- **wisphive_web** — Axum HTTP/WebSocket server. Embeds the Vite-built React frontend via `rust-embed` and bridges browser ↔ daemon over `/ws`. Optional TLS via `rustls`/`rcgen` self-signed certs. Auth primitives in `auth.rs` (Argon2id passwords, SHA-256-hashed device tokens, per-IP login throttle, `webauthn-rs` for passkeys); request gating in `security.rs` (bearer token + Origin/Host allowlist). Can run standalone (`wisphive web serve`) or in-process with the daemon (`wisphive daemon start --web`).
- **wisphive_cli** — Clap-based CLI (`wisphive` binary). Subcommands: `daemon {start [--web --host --port --web-dev --no-open --auth-profile --auth-rp-id], stop, status}`, `hooks {install, uninstall, enable, disable, status}`, `tui`, `web {serve [--host --port --dev --no-open --auth-profile --auth-rp-id], set-password, reset-password, devices {list, revoke <id>}, fingerprint}`, `agent {start [--agent-type claude_code|codex], list, stop}`, `history {search, recent}`, `config {list, get, set, auto-approve {status, level, add, remove, reset}}`, `term {new, list, attach, replay, close}`, `doctor`, `emergency-off`. Web UI default port is `3100` (CLI) — note `justfile` uses `8080` for the `web` recipe. On first-run (no web password set), `daemon start --web` and `web serve` auto-open the default browser onto the SPA; `--no-open` suppresses this for headless servers / CI. `--auth-profile {local-lan|enterprise}` (default `local-lan`, itr#310) selects the daemon's auth/security posture; `enterprise` additionally requires `--auth-rp-id <domain>` plus user-provided TLS cert (the latter pending itr#270).
- **wisphive_adapters** — `AgentAdapter` trait and implementations (Claude Code and Codex are hook-based/passive; Red and LocalLLM are stubs).

## Key Design Decisions

- **Fail-open**: Hook errors always approve (exit 0) to avoid blocking agents.
- **Blocking hooks via oneshot channels**: Each hook connection gets a `tokio::sync::oneshot` receiver; it blocks until a human resolves the decision or timeout (1 hour, defaults to approve).
- **Broadcast fan-out**: TUI clients subscribe to a `tokio::sync::broadcast` channel for real-time events.
- **SQLite WAL crash recovery**: Pending decisions persist to disk; audit log tracks all resolutions.
- **Passive notifications**: macOS uses `osascript display notification` (non-intrusive banner); Linux uses `notify-send`. Notifications are informational only — all tool input fields are shown so users have context when switching to the TUI to respond. Notifications do NOT resolve decisions; only the TUI does.
- **Permissions management**: `wisphive hooks install` adds Claude Code permissions (Bash, Edit, Write, NotebookEdit) to `.claude/settings.json` so Claude Code auto-allows tools that Wisphive gates (eliminates double-prompt). Codex hooks are installed in `.codex/hooks.json`; Codex `PermissionRequest` is used for native approvals instead of a permissions allowlist.

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
- `web.cert.pem` / `web.key.pem` — Self-signed TLS cert/key for the web UI (key is mode 0600). Validity is capped at 397 days; rotation writes atomically under `web.cert.lock` (flock) with metadata in `web.cert.meta.json`. See `crates/wisphive_web/src/tls.rs`.

Web auth no longer uses a `~/.wisphive/web.token` file. Raw per-device bearer tokens are issued by `/api/auth/login` or first-run `/api/auth/set-password`, stored client-side in the SPA's `localStorage` under `wisphive-web-token`, and sent as `Authorization: Bearer` for `/api/*` or `?token=` for `/ws` because browser WebSocket constructors cannot set auth headers. The server stores only SHA-256 token hashes in `web_devices`; revoked or unknown tokens both return 401. The retired `/api/web-token` route should stay 404.

## Reference Documentation

- [tui-textarea reference](claude/tui-textarea-reference.md) — API reference, key bindings, and integration notes for the TUI text editing widget
- [investigation-empty-detail-views](claude/investigation-empty-detail-views.md) — notes on why `ExitPlanMode` and `AskUserQuestion` rendered empty detail views in the TUI
- [docs/plan-cross-agent-conflict-gate.md](docs/plan-cross-agent-conflict-gate.md), [docs/plan-decision-plugins.md](docs/plan-decision-plugins.md), [docs/plan-policy-learning-engine.md](docs/plan-policy-learning-engine.md) — design docs for upcoming workstreams
- [docs/plan-mobile-device-pairing.md](docs/plan-mobile-device-pairing.md) — critical path, sizing, and RP ID design for the phone-pairing milestone (itr#283 epic)
- [docs/open-source-path.md](docs/open-source-path.md) — OSS positioning and roadmap

## Rust Edition

The workspace uses Rust **edition 2024**. Requires Rust **nightly** (per `CONTRIBUTING.md`); a recent stable toolchain that supports edition 2024 also works.

## When to Update This File

Keep `CLAUDE.md` aligned with reality — a stale entry here misleads every future Claude Code session. Update it in the same PR as the change whenever you:

- **Add, remove, or rename a workspace crate** (update the architecture section, dependency flow, and crate count).
- **Add or rename a top-level CLI subcommand or change a default flag value** (the CLI subcommand list is hand-maintained from `crates/wisphive_cli/src/main.rs`).
- **Add, remove, or rename a runtime file under `~/.wisphive/`** (sockets, PID, DB, mode, certs, tokens, config). Include permissions/locking semantics when non-obvious.
- **Change the IPC wire protocol** (new client kinds, new framing, breaking message changes).
- **Add a new Claude Code hook event handler** in `wisphive_hook`, or learn a new fact about hook stdin/stdout schema (the "Claude Code Hook Response Format" section is the canonical reference for the project).
- **Change a fail-open / fail-closed default, timeout, or other safety-critical default** (the "Key Design Decisions" section).
- **Add a new build/test/lint command** that contributors will need (or change an existing one).
- **Add reference docs under `claude/` or `docs/`** that future sessions should know exist.

Do **not** add to `CLAUDE.md`:

- Per-task notes, in-progress work, or transient TODOs (use `itr` issues or commit messages).
- File-by-file or line-by-line inventories that `git ls-files` / `Glob` can derive on demand.
- Generic Rust/React/Tokio guidance — assume the reader is fluent.
- Counts that drift (test counts, LOC, issue counts). Prefer the command that produces the count.

If you're unsure whether something belongs here, ask: *"Would the next Claude Code session waste time or make a wrong assumption without this?"* If yes, add it. If no, leave it out.
