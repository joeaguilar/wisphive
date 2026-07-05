# Glossary — Wisphive

_Last reviewed: 2026-07-05_

> **Purpose.** One canonical definition per Wisphive-specific term, so humans and agents mean
> the same thing by the same word. When a term is overloaded (e.g. **transcript**), this file is
> the tie-breaker. If you coin or redefine a term in an ADR, plan, or issue, add/point it here.
>
> Scope: terms whose Wisphive meaning is **non-obvious** or **collision-prone**. Generic
> Rust/React/Tokio vocabulary is out of scope — assume the reader is fluent. Authoritative source
> for each term is `CLAUDE.md` unless a file / ADR / `itr#` is cited.

---

## Architecture & components

- **Control plane** — Wisphive as a whole: the layer that sits between AI agents and the tools
  they invoke, gating tool calls through a central daemon for human review. Not a proxy of the
  agent's model traffic — it mediates **tool calls only**.
- **Daemon** — The long-lived async Tokio server (`wisphive_daemon`) on `~/.wisphive/wisphive.sock`.
  Owns the decision queue, SQLite state, terminal PTYs, spawned agents, and notifications. Single
  source of truth for live state.
- **Hook** — The `wisphive-hook` binary, run by Claude Code / Codex as a subprocess per tool call.
  Ephemeral: makes a local decision (mode / auto-approve / defer) or connects to the daemon and
  **blocks** until a human resolves it. Distinct from the daemon.
- **TUI** — Specifically the **Wisphive dashboard** (`wisphive_tui`, Ratatui): one of the two human
  **review frontends** onto the daemon, the terminal-native counterpart of the **Web UI**. A
  streaming daemon client with panels (queue, agents, projects, terminals). **Only the TUI or Web UI
  resolves decisions** — notifications do not. It is **not** a PTY and **not** "a terminal" in the
  byte-stream sense — see the disambiguation below.
- **Web UI** — The Axum + React surface (`wisphive_web`); embeds the Vite build via `rust-embed`
  and bridges the browser to the daemon over `/ws`. The browser-native peer of the **TUI** — same
  role (review frontend), different medium.

  > **TUI vs terminal vs Web UI — don't conflate.** Three text-ish surfaces, three meanings:
  > **TUI** = the Wisphive Ratatui *dashboard application* (a review frontend, like the Web UI).
  > **Terminal / PTY** = a **terminal session** (§ Agents & sessions) — a shell/agent byte stream
  > the daemon owns and records. **Terminals panel / embedded terminal** = a *view of* a terminal
  > session **rendered inside** the TUI or the Web UI (via `xterm.js` on the web). So "the TUI" is
  > the app; "a terminal" is a byte stream; an embedded terminal is the app *showing* the byte
  > stream. The TUI running in your terminal emulator does **not** make it "a terminal" in this
  > glossary's sense.
- **Adapter** — An `AgentAdapter` implementation (`wisphive_adapters`). Claude Code and Codex are
  **hook-based / passive**; Red and LocalLLM are stubs.
- **Command Center** — The web UI's primary review surface; its **inbox** shows pending decisions
  plus a seeded feed of recently auto-answered/deferred rows (itr#399/#434). See
  [`command-center-spec.md`](command-center-spec.md).

## Agents & sessions

- **Passive / hook-based agent** — An agent the **user** launches (Claude Code, Codex). Wisphive
  never owns its process or terminal; it sees only the stream of gated tool calls via the hook.
  This includes `claude -p`.
- **Headless / spawned agent** — An agent **Wisphive** launches via the process registry
  (`wisphive agent start`), running inside a Wisphive-owned **PTY**. Wisphive owns its lifecycle
  and byte stream.
- **`claude -p`** — Claude Code's non-interactive / "print" mode. Fires `PreToolUse` hooks
  identically to interactive — **even under `--permission-mode bypassPermissions`** (bypass skips
  Claude's *native* prompts, not hooks). So `-p` tool calls are gated like any other. It has **no
  interactive PTY** and exits when done, but still writes a full session JSONL (see
  **conversation transcript**). Verified 2026-07, Claude Code 2.1.201.
- **Session** — One agent run, identified by `session_id` (from the hook stdin). The join key for a
  **conversation transcript** and for the decisions made during that run.
- **Terminal session** — A Wisphive-owned PTY (spawned agent or `wisphive term new`), identified by
  a `terminal_session_id`. Its byte history persists to `terminal_events`. Distinct from a
  **session** — a passive agent has a `session_id` but **no** terminal session.

## Transcript — the overloaded word (read this before using it)

"Transcript" refers to **two different artifacts from two different sources**. Always qualify it.

- **Terminal transcript** — A plain-text (or asciinema `.cast`) rendering of a **terminal
  session's byte stream** (`terminal_events`: Input/Output/Resize), produced offline through a
  vt100 parser. Exists **only** when Wisphive owned the PTY. Export tracked in **itr#486**; gated on
  **itr#98** (TermReplay authz) because byte history can hold typed sudo passwords / `cat`'d keys.
- **Conversation transcript** — A structured render of **Claude Code's session JSONL**
  (`transcript_path`: `user` / `assistant` / `tool_use` / `tool_result` records). Exists for **any
  hooked session, including `claude -p`** (which has no PTY, hence no terminal transcript). View
  tracked in **itr#483**. Contains `tool_input`/`tool_result` → must pass the **redaction** scrubber
  before any surface.

They are **complementary, not competing**: a PTY-owned interactive session can have *both*; a `-p`
session has *only* the conversation transcript. Both are bulk-exfiltration surfaces and share
**itr#98**'s authz+audit model. Do not conflate them in UI, CLI, or prose.

- **`transcript_path`** — Hook-stdin field pointing to Claude's session JSONL under
  `~/.claude/projects/<munged-cwd>/<session-id>.jsonl`. Attacker-influenceable input — reads must be
  path-validated (itr#483 AC-5). Today used narrowly to extract plan text for `ExitPlanMode`
  (`wisphive_hook/src/main.rs`).

## Decision flow & gating

- **Gate / gating** — Interposing on a tool call so a human (or a rule) approves, denies, or defers
  it before it runs. The core verb of the product.
- **Decision** — The outcome: `Approve`, `Deny`, or `Ask`. Also the `wisphive_protocol` type.
- **Ask / defer** — Return the call to the **agent's native prompt** instead of resolving it in
  Wisphive. Deferred calls never reach the daemon queue.
- **Always-defer / always-ask** — The classification that runs **first**, before any level check
  (ADR-0002, itr#380). `DEFAULT_ALWAYS_ASK` (`AskUserQuestion`, `EnterPlanMode`, `ExitPlanMode`,
  `Elicitation`) defers **unconditionally** — no posture or override un-defers it (since `0530ef1`).
  Operator-added `always_ask` tools are releasable by the **dangerous** posture or `always_ask_remove`.
- **Auto-approve level** — Tiered posture in `config.json`: `off` / `read` / `write` / `execute` /
  `all`. Resolves **in the hook**, never touching the daemon. Plus per-tool `auto_approve_add` /
  `auto_approve_remove` and content-aware `tool_rules`.
- **`tool_rules`** — Per-tool `deny_patterns` / `allow_patterns` (case-insensitive substring match
  on tool input) in `config.json`.
- **Posture** — The preset selected by `wisphive config auto-approve mode`: **balanced** (keeps the
  full always-defer set) or **dangerous** (releases operator-added `always_ask` entries;
  intrinsic questions/plan-mode still defer). Both default `auto_approve_level=all`.
- **`decided_by`** — The attribution string stamped on every resolution: which layer/rule resolved
  it (`level:all`, `auto_approve_add`, `tool_rules:<tool>:allow_pattern`, `always_ask:intrinsic`,
  `event_toggle:<key>`, `human`, `timeout:approve`, `daemon_restart:failopen`, …). Queryable via
  `wisphive audit` (itr#397).
- **Self-modification protection** — With `allow_self_modification=false` (default), any
  `Write`/`Edit`/`MultiEdit`/`NotebookEdit`/`Bash` targeting `~/.wisphive/**` is forced past every
  auto-approve layer to **human review** at any level — a gated agent can't rewrite its own gate
  (itr#425). Routes to the human queue, **not** a `Decision::Ask`.

## Fail behavior

- **Tiered fail posture** — ADR-0001. A **daemon-unreachable** failure (socket refused/absent)
  **always fails open** (approve) so a dead control plane can't brick every agent. Other runtime
  errors honor `fail-mode`.
- **`fail-mode`** — `~/.wisphive/fail-mode`, default **`closed`** (deny). Set `open` for
  availability-first.
- **DaemonUnreachable** — The specific error class that unconditionally fails open, including an
  EOF-mid-wait (daemon died while the hook blocked).
- **Mode file / kill switch** — `~/.wisphive/mode`. Gating is active **iff** it contains exactly
  `active`; anything else (missing/empty/`off`) disables gating globally. `wisphive emergency-off`
  writes `off`.

## Recording & audit surfaces

- **Decision queue** — The daemon's live, in-memory set of pending decisions awaiting human review.
  Holds the **un-redacted** tool input in memory for review only.
- **`events.jsonl`** — Append-only log the hook writes for daemon ingestion (`auto_approved`,
  `deferred`, `denied` records with `decided_by` + `config_hash`). Redacted before persist. Rotates
  into `logs/events-<ts>.jsonl`.
- **`decision_log`** — The durable SQLite table (+ `logs/decision_log.jsonl` archive sink) of
  resolved decisions. **Audit data is never auto-deleted** (itr#340) — instead a
  **resource alert** fires when the archive or free disk crosses a threshold.
- **Redaction** — The shared secret scrubber (`wisphive_protocol::redact`, itr#89) that replaces
  suspected secrets with `***REDACTED***` before any persist/notify/UI surface. The raw Claude
  transcript is **not** redacted at source — itr#483 must run it through this on ingest.
- **`config_hash`** — Truncated SHA-256 of `config.json` at decision time, stamped on hook records
  for the audit trail.
- **Resource alert** — A non-destructive, latched TUI/web banner (`disk_alert.rs`, itr#340) raised
  when the audit archive exceeds `archive_alert_max_mb` or free disk drops below
  `disk_alert_free_mb`. Never deletes data — prompts the operator to act.

## Config & runtime files

- **`~/.wisphive/`** — Runtime root: `wisphive.sock`, `wisphive.pid`, `wisphive.db`, `mode`,
  `config.json`, `events.jsonl`, `logs/`, `web.cert.pem`/`web.key.pem`. See CLAUDE.md "Runtime Files".
- **`config.json`** — Canonical hook config (auto-approve levels/overrides, `tool_rules`,
  posture, event toggles, daemon retention knobs). `auto-approve.json` is a **legacy** fallback.

## Auth & web

- **AuthProfile** — The auth/security posture selected at startup (itr#310): **LocalLAN** (default)
  or **Enterprise**. Not a single locked posture — chosen per run. Enterprise requires TLS flags and
  is **non-functional until itr#270** lands.
- **Device token** — A per-device bearer token issued by `/api/auth/login` or first-run
  `set-password`, stored in the SPA's `localStorage` (`wisphive-web-token`). The server stores only
  its SHA-256 hash. Any XSS = device compromise. There is **no** `~/.wisphive/web.token` file.
- **RP ID** — WebAuthn Relying Party ID (a domain) for passkeys; central to mobile pairing
  (itr#283, [`plan-mobile-device-pairing.md`](plan-mobile-device-pairing.md)).

## Docs & process vocabulary

- **ADR** — Architecture Decision Record under [`docs/decisions/`](decisions/README.md). Git-tracked
  (unlike `~/.claude` memory) so the *why* survives a fresh clone. Never deleted — superseded ADRs
  flip status and link their successor.
- **Handoff** — A dated, append-only milestone breadcrumb under `docs/handoff/`. Never rewritten in
  place — a new dated handoff links back.
- **Smoke checklist** — [`docs/smoke/CHECKLIST.md`](smoke/CHECKLIST.md): human-only verification
  (notification perception, real-device passkeys, TUI feel) batched at phase boundaries, never
  blocking per-issue.
- **Verify gate** — `just verify`: the close-with-evidence suite (fmt, clippy, tests, frontend
  lint/vitest, e2e) run under `gatr` tags. `just e2e` is the Playwright smoke suite and is **not**
  in the per-story gate — run it after user-visible changes.

---

_See also:_ [`DOCUMENTATION.md`](DOCUMENTATION.md) (the index of all doc surfaces) ·
[`CLAUDE.md`](../CLAUDE.md) (canonical architecture/build/guidance) ·
[`docs/decisions/`](decisions/README.md) (the *why* behind these terms).
