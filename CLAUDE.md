# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

## What is Wisphive

Wisphive is a multiplexed AI agent control plane that gates tool calls from AI agents (Claude Code, Codex, Red, local LLMs) through a centralized daemon. Agents request approval before executing tools; humans review and approve/deny via a TUI dashboard. Passive OS notifications alert the user when decisions are pending.

## Documentation Map

**Start here when you need context this file doesn't carry:** [`docs/DOCUMENTATION.md`](docs/DOCUMENTATION.md) — the index of every documentation surface and how they link together. Quick links:

- **Why a design is the way it is** → ADRs at [`docs/decisions/`](docs/decisions/README.md)
- **What's done / in flight / next** → [`docs/ROADMAP.md`](docs/ROADMAP.md) + `itr ready`
- **A task to work on** → `itr` (`itr ready`, `itr next`, `itr get <ID>`)
- **What happened in a past milestone** → [`docs/handoff/`](docs/handoff/)
- **Upcoming-workstream designs** → [`docs/plan-*.md`](docs/) (conflict gate, deterministic analytics, decision plugins, policy learning, loop supervisor, mobile pairing, red)
- **Exploratory research** → [`docs/research/`](docs/research/) · **Reference notes** → [`claude/`](claude/)

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
just verify                      # Full verify gate suite via gatr (fmt --check, clippy, tests, frontend lint+vitest, e2e)
just e2e                         # Playwright e2e smoke suite (isolated temp HOME — never touches ~/.wisphive)
```

`just verify` is the close-with-evidence gate: it runs every sub-gate under its own gatr tag (`verify-fmt`, `verify-clippy`, `verify-rust`, `verify-frontend`, `verify-e2e`), fails fast on the first red gate, and `gatr last` / `gatr errors` reproduce each tag's result afterward. TUI snapshot tests run inside `verify-rust` (`cargo test --workspace`).

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
- **wisphive_hook** — Lightweight binary that runs as a Claude Code or Codex hook subprocess. Three-layer decision logic: (1) check `~/.wisphive/mode` file, (2) an **always-defer** classification runs first — questions/plan-mode/elicitations (`DEFAULT_ALWAYS_ASK`: `AskUserQuestion`, `EnterPlanMode`, `ExitPlanMode`, `Elicitation`) plus operator `always_ask` tools return `ask` (defer to the agent's native prompt) regardless of level, since their answer comes back only through the native prompt and auto-approving them silently resolves the prompt with no selection; this is bypassed only by the `auto_approve_dangerous` posture (itr#380) — then auto-approve via tiered `auto_approve_level` in `~/.wisphive/config.json` (off/read/write/execute/all, plus per-tool `auto_approve_add`/`auto_approve_remove` and content-aware `tool_rules` with `deny_patterns`/`allow_patterns`); legacy `~/.wisphive/auto-approve.json` (`{"auto_approve": [...]}`) is still read as a fallback, (3) connect to daemon for human review. Exit codes: the `permissionDecision` field in the stdout JSON — not the exit code — is authoritative for allow/deny/ask. `2` is a bare-exit deny; everything else exits `0` (including a message-bearing deny and a fail-open approve). There is no exit-`1` error path: a runtime error resolves per `fail-mode` (deny→exit 2, or approve→exit 0), never a bare error exit.
- **wisphive_tui** — Ratatui terminal UI. Connects to daemon as a streaming client. Panels include queue, agents, projects, terminals. Keys: `a`/`d` approve/deny, `A`/`D` bulk, `/` filter, Tab switch panels.
- **wisphive_web** — Axum HTTP/WebSocket server. Embeds the Vite-built React frontend via `rust-embed` and bridges browser ↔ daemon over `/ws`. Optional TLS via `rustls`/`rcgen` self-signed certs. Auth primitives in `auth.rs` (Argon2id passwords, SHA-256-hashed device tokens, per-IP login throttle, `webauthn-rs` for passkeys); request gating in `security.rs` (bearer token + Origin/Host allowlist). Can run standalone (`wisphive web serve`) or in-process with the daemon (`wisphive daemon start --web`). The web crate depends on `wisphive_daemon` for web auth state and, in embedded mode, reads the daemon's in-memory `LogStore` for `/api/logs`; standalone web returns 503 for that endpoint until daemon-log IPC streaming lands.
- **wisphive_cli** — Clap-based CLI (`wisphive` binary). Subcommands: `daemon {start [--web --host --port --web-dev --no-open --auth-profile --auth-rp-id], stop, status}`, `hooks {install, uninstall, enable, disable, status}`, `projects {audit, scan, list, seed}`, `tui`, `web {serve [--host --port --dev --no-open --auth-profile --auth-rp-id], set-password, reset-password, devices {list, revoke <id>}, fingerprint}`, `agent {start [--agent-type claude_code|codex], list, stop}`, `history {search, recent}`, `audit [--since 1h --project X --decided-by <rule> --limit N]` (decision audit trail: every decision with the layer/rule that resolved it, itr#397), `config {list, get, set, auto-approve {status, level, add, remove, mode <balanced|dangerous>, defer <tool>, undefer <tool>, reset}}`, `term {new, list, attach, replay, close}`, `doctor`, `emergency-off`. Web UI default port is `3100` (CLI) — note `justfile` uses `8080` for the `web` recipe. On first-run (no web password set), `daemon start --web` and `web serve` auto-open the default browser onto the SPA; `--no-open` suppresses this for headless servers / CI. `--auth-profile {local-lan|enterprise}` (default `local-lan`, itr#310) selects the daemon's auth/security posture; `enterprise` requires `--auth-rp-id <domain>` plus a user-provided TLS cert. Until the TLS wiring lands (itr#270) the `enterprise` profile is **non-functional regardless of `--auth-rp-id`**: config validation checks the TLS flags first and always fails with `MissingTlsFlags`.
- **wisphive_adapters** — `AgentAdapter` trait and implementations (Claude Code and Codex are hook-based/passive; Red and LocalLLM are stubs).

## Key Design Decisions

- **Tiered fail posture** (active mode): a *daemon-unreachable* failure (refused/absent socket — the daemon is down) **always fails open** so a crashed control plane can't brick every agent. Other runtime errors (read/parse/protocol) honor `~/.wisphive/fail-mode`, which **defaults to `closed`** (deny) per the security posture in AGENTS.md; set `fail-mode=open` for availability-first. Oversized hook stdin always denies. `PostToolUse` reporting failures always approve (telemetry only). See `response_for_failure` in `wisphive_hook`.
- **Blocking hooks via oneshot channels**: Each hook connection gets a `tokio::sync::oneshot` receiver; the daemon `select!`s over it, the timeout (1 hour, defaults to approve, attributed `timeout:approve`), and the hook's socket (a dead hook abandons the decision immediately as a deny, itr#363). A dropped sender (daemon teardown mid-wait) intentionally fails open per ADR-0001, attributed `channel_dropped:approve` (itr#345).
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

**PermissionRequest hook** (separate event): fires when Claude's permission dialog would show. Input includes `permission_suggestions` array — the dynamic options the user would see in the native dialog. Each suggestion is a permission update entry (`addRules`/`setMode`/etc) with `behavior`, `destination`, `rules`. A hook can echo any suggestion back as `updatedPermissions` in its response.

**Claude PermissionRequest response** (stdout, exit 0):
```json
{
  "hookSpecificOutput": {
    "hookEventName": "PermissionRequest",
    "decision": {
      "behavior": "allow",
      "updatedPermissions": [/* echoed permission_suggestions entry */],
      "updatedInput": { "command": "sanitized version" }
    }
  }
}
```

**Known Claude Code hook events recognized by `HookEventType`**: `PreToolUse`, `PostToolUse`, `PostToolUseFailure`, `PermissionRequest`, `Elicitation`, `ElicitationResult`, `InstructionsLoaded`, `UserPromptSubmit`, `Stop`, `SubagentStop`, `SubagentStart`, `StopFailure`, `ConfigChange`, `TeammateIdle`, `TaskCompleted`, `WorktreeCreate`, `WorktreeRemove`, `PreCompact`, `PostCompact`, `SessionStart`, `SessionEnd`, `Notification`, plus `Unknown`. The formatter must explicitly route each known variant so future or unsupported events do not silently emit PreToolUse-shaped JSON. `wisphive hooks install` intentionally registers a narrower Claude hook set in `crates/wisphive_cli/src/commands/hooks.rs`.

## IPC Protocol

Unix socket at `~/.wisphive/wisphive.sock`. Newline-delimited JSON. Two client types:
- **Hook**: ephemeral — sends Hello + DecisionRequest, blocks for DecisionResponse, exits.
- **TUI**: long-lived — sends Hello, receives QueueSnapshot, then bidirectional streaming of commands and events.

## Runtime Files

All under `~/.wisphive/`:
- `wisphive.sock` — Unix domain socket
- `wisphive.pid` — Daemon PID file
- `wisphive.db` — SQLite state/audit database
- `mode` — `active` (hook compares `contents.trim() == "active"`); anything else (missing, empty, `off`) disables gating. Global kill switch.
- `config.json` — Canonical hook config. Keys: `auto_approve_level` (`off`/`read`/`write`/`execute`/`all`), `auto_approve_add` / `auto_approve_remove` (tool-name overrides), `always_ask` / `always_ask_remove` (tool-name overrides for the always-defer set — adds harmful tools, or opts a built-in question/plan tool out), `auto_approve_dangerous` (bool, default false — the "dangerous" posture: when true, the always-defer set is ignored and even questions auto-approve per the level), `tool_rules.<ToolName>.{deny_patterns,allow_patterns}` (case-insensitive substring rules on tool input), `allow_self_modification` (bool, default false — control-plane self-protection, itr#425: while false, any `Write`/`Edit`/`MultiEdit`/`NotebookEdit`/`Bash` tool call targeting `~/.wisphive/**` is forced past every auto-approve layer to daemon human review at **any** level including `all`, so a gated agent can't rewrite its own gate; set `true` to opt out. Not a `Decision::Ask` defer — Ask would hand the call to Claude's `hooks install` permission allowlist, so it routes to the human queue instead), plus event-level toggles like `auto_approve_stop` / `auto_approve_user_prompt`. `wisphive config auto-approve mode {balanced|dangerous}` sets the posture preset (both default to `auto_approve_level=all`; `balanced` keeps always-defer on, `dangerous` turns it off). Daemon-side retention knobs also live here: `retention_max_rows`, `retention_max_age_days`, `log_retention_days`, and `retention_vacuum_max_mb` (default 256 — the DB-size ceiling above which retention skips the full `VACUUM` to avoid hang/OOM, doing only a WAL checkpoint). Two non-destructive resource-alert thresholds also live here: `archive_alert_max_mb` (default 10240 = 10 GiB) and `disk_alert_free_mb` (default 10240 = 10 GiB free); `0` disables either. These never delete data — they raise a TUI/web banner (see `disk_alert.rs` / itr#340). Managed via `wisphive config ...`.
- `auto-approve.json` — **Legacy** auto-approve file (`{"auto_approve": ["Read", ...]}`). Still read as a fallback when `config.json` is missing, before `DEFAULT_AUTO_APPROVE` kicks in.
- `events.jsonl` — Append-only log written by the hook for daemon ingestion: `auto_approved`, `deferred` (always-defer), and `denied` (Codex fail-closed) records, each carrying `decided_by` (the layer/rule that resolved it — e.g. `level:all`, `auto_approve_add`, `tool_rules:<tool>:allow_pattern`, `always_ask:intrinsic`, `event_toggle:<key>`) and `config_hash` (truncated SHA-256 of config.json at decision time) for the itr#397 audit trail. `tool_input`/`tool_result` are secret-redacted (`***REDACTED***`, shared scrubber in `wisphive_protocol::redact`, itr#89) before any persist/notify surface — the live TUI review queue keeps the full input in memory only. Daemon-side resolutions stamp `decided_by` too (`human`, `timeout:approve`). Query via `wisphive audit`. `O_APPEND` atomic writes; fail-open on error. The daemon (sole consumer) tails it, tolerates truncation/rotation by reseeking, and once it grows past ~16 MiB rotates it into `logs/events-<ts>.jsonl`. Non-lossy **on the success path**: the rotated segment is re-ingested into `decision_log`, then reaped by `log_retention_days`. If re-ingest fails the segment is renamed `logs/events-<ts>.failed.jsonl` and **retained** for manual/startup re-import (itr#336); the pruner never reaps `*.failed.jsonl`.
- `logs/decision_log.jsonl` — Durable archive sink for `decision_log` rows pruned from SQLite (rows are written here before deletion). Rotated at 32 MiB into `decision_log.jsonl.<ts>` siblings. **Audit data is never auto-deleted** (itr#340): the pruner never reaps `decision_log.jsonl*`, so the archive is retained indefinitely. Instead the daemon raises a non-destructive **resource alert** (TUI/web banner + `warn!`) when the archive exceeds `archive_alert_max_mb` (default 10 GiB) or free space on the state filesystem drops below `disk_alert_free_mb` (default 10 GiB); the operator then moves/compresses the archive or frees disk. Alerts are latched (raised once per threshold crossing, cleared when back under) and probed on startup + each hourly retention tick. See `crates/wisphive_daemon/src/disk_alert.rs`.
- `web.cert.pem` / `web.key.pem` — Self-signed TLS cert/key for the web UI (key is mode 0600). Validity is capped at 397 days; rotation writes atomically under `web.cert.lock` (flock) with metadata in `web.cert.meta.json`. See `crates/wisphive_web/src/tls.rs`.

Web auth no longer uses a `~/.wisphive/web.token` file. Raw per-device bearer tokens are issued by `/api/auth/login` or first-run `/api/auth/set-password`, stored client-side in the SPA's `localStorage` under `wisphive-web-token`, and sent as `Authorization: Bearer` for `/api/*` or `?token=` for `/ws` because browser WebSocket constructors cannot set auth headers. The server stores only SHA-256 token hashes in `web_devices`; revoked or unknown tokens both return 401. The retired `/api/web-token` route should stay 404.

## Reference Documentation

- [tui-textarea reference](claude/tui-textarea-reference.md) — API reference, key bindings, and integration notes for the TUI text editing widget
- [investigation-empty-detail-views](claude/investigation-empty-detail-views.md) — notes on why `ExitPlanMode` and `AskUserQuestion` rendered empty detail views in the TUI
- [docs/plan-cross-agent-conflict-gate.md](docs/plan-cross-agent-conflict-gate.md), [docs/plan-deterministic-agent-analytics.md](docs/plan-deterministic-agent-analytics.md), [docs/plan-decision-plugins.md](docs/plan-decision-plugins.md), [docs/plan-policy-learning-engine.md](docs/plan-policy-learning-engine.md), [docs/plan-loop-supervisor.md](docs/plan-loop-supervisor.md) — design docs for upcoming workstreams; the policy-learning, plugin, conflict-gate, and loop-supervisor plans carry **normative** invariant/trust-model/semantics sections backed by ADR-0005–0007
- [docs/plan-mobile-device-pairing.md](docs/plan-mobile-device-pairing.md) — critical path, sizing, and RP ID design for the phone-pairing milestone (itr#283 epic)
- [docs/open-source-path.md](docs/open-source-path.md) — OSS positioning and roadmap

## Session Handoffs

Substantial sessions end with a durable handoff at `docs/handoff/YYYY-MM-DD-<topic>.md` — an append-only milestone breadcrumb for the next implementer (a fresh clone, a collaborator, or a reviewing agent on another machine). Write one when you close an epic/phase **or** when you hand off mid-stream. Each handoff records what shipped, the trade-offs made, the hard rules established, and where to start next, with an "if you only have 60 seconds" pointer and a link to its predecessor. **Handoffs are never rewritten in place** — if the situation changes, write a new dated handoff that links back. Copy `docs/handoff/TEMPLATE.md` to start; get the facts from git (`git show --stat <sha>`), not from memory.

**Human smoke checklist** ([`docs/smoke/CHECKLIST.md`](docs/smoke/CHECKLIST.md)): human verification is batched at phase boundaries, never blocking per-issue. When work has human-only verification residue (notification perception, real-device passkeys, phone pairing, TUI feel), close the issue on the automated gate and append an item to the checklist (steps, expected result, evidence slot, dated sign-off line) in the right phase section; the human burns pending items down in one session per phase and records it in the burn-down log.

## Architecture Decision Records (ADRs)

Non-obvious, security-critical decisions live as durable ADRs under `docs/decisions/`, not just as prose here — `~/.claude` memory is machine-local, but ADRs are git-tracked, so a fresh clone or a reviewing agent on another machine can reconstruct the reasoning and the alternatives weighed. The index is [`docs/decisions/README.md`](docs/decisions/README.md); the template is [`docs/decisions/0000-template.md`](docs/decisions/0000-template.md). **File an ADR when a decision constrains future work, was non-obvious / had real alternatives, or someone will later ask "why is it done this way" — copy `docs/decisions/0000-template.md` to `NNNN-short-title.md`, fill it in, and add a row to the index.** Status lifecycle is `Proposed` → `Accepted` → `Superseded by ADR-XXXX` / `Deprecated`; never delete a superseded ADR — flip its status and link the successor. The tiered fail posture (ADR-0001) and the always-defer classification (ADR-0002, itr#380) are already backfilled.

## Rust Edition

The workspace uses Rust **edition 2024**. Requires Rust **nightly** (per `CONTRIBUTING.md`); a recent stable toolchain that supports edition 2024 also works.

## When to Update This File

Keep `CLAUDE.md` aligned with reality — a stale entry here misleads every future Claude Code session. Update it in the same PR as the change whenever you:

- **Add, remove, or rename a workspace crate** (update the architecture section, dependency flow, and crate count).
- **Add or rename a top-level CLI subcommand or change a default flag value** (the CLI subcommand list is hand-maintained from `crates/wisphive_cli/src/main.rs`).
- **Add, remove, or rename a runtime file under `~/.wisphive/`** (sockets, PID, DB, mode, certs, tokens, config). Include permissions/locking semantics when non-obvious.
- **Change the IPC wire protocol** (new client kinds, new framing, breaking message changes).
- **Add a new Claude Code hook event handler** in `wisphive_hook`, or learn a new fact about hook stdin/stdout schema (the "Claude Code Hook Response Format" section is the canonical reference for the project).
- **Change a fail-open / fail-closed default, timeout, or other safety-critical default** (the "Key Design Decisions" section). If the decision was non-obvious or had real alternatives, also file/refresh an ADR under `docs/decisions/`.
- **Add a new build/test/lint command** that contributors will need (or change an existing one).
- **Add reference docs under `claude/` or `docs/`** that future sessions should know exist.

Do **not** add to `CLAUDE.md`:

- Per-task notes, in-progress work, or transient TODOs (use `itr` issues or commit messages).
- File-by-file or line-by-line inventories that `git ls-files` / `Glob` can derive on demand.
- Generic Rust/React/Tokio guidance — assume the reader is fluent.
- Counts that drift (test counts, LOC, issue counts). Prefer the command that produces the count.

If you're unsure whether something belongs here, ask: *"Would the next Claude Code session waste time or make a wrong assumption without this?"* If yes, add it. If no, leave it out.
