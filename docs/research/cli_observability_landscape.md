# CLI agent observability and tool-call gating: the landscape in 2026

**No existing tool fully solves the problem of a local, sub-millisecond, daemon-based control plane for AI coding agent tool calls — but the pieces exist.** OpenTelemetry provides the best instrumentation foundation at ~35μs overhead per span; Claude Code's 21 lifecycle hooks offer the richest extensibility surface of any CLI agent; and a simple JSONL-append + inotify-tail + SQLite-WAL architecture can achieve **~5–10μs total producer overhead**, orders of magnitude below the sub-millisecond target. The most architecturally relevant open-source project is Aegis (AgentLify Labs), a Go daemon with OPA policy and append-only audit logging, though it remains early-stage. The ecosystem is young but growing fast — the leading Claude Code observability project already has 1,100+ GitHub stars.

---

## Observability platforms mostly miss the mark for local CLI agents

The major AI agent observability platforms — LangSmith, Langfuse, Weave, Helicone, Braintrust, AgentOps — are designed for cloud-first, web-UI-first workflows monitoring LLM API calls. Most require heavy infrastructure (Postgres, ClickHouse, Redis, Kubernetes) that makes them unsuitable for a lightweight local daemon. Only two stand out for this use case.

**Arize Phoenix** is the strongest full-featured option. It runs with a single `pip install arize-phoenix && phoenix serve`, stores data in **SQLite by default** (in `~/.phoenix/`), and accepts traces over standard OTLP gRPC. Its OpenInference schema distinguishes agent, LLM, and tool spans with typed `kind` columns. The tradeoff: Phoenix is primarily a debugging UI, not a hot-path logging layer, and the OTEL collector adds overhead compared to direct file writes.

**OpenTelemetry itself** — not any particular platform — is the best foundation layer. Raw OTEL span creation costs **~35μs in Python** (sub-microsecond in Go/Rust), export is fully async via `BatchSpanProcessor`, and it works with any backend or no backend at all. The context propagation model naturally correlates parent-child spans, making PreToolUse → tool execution → PostToolUse correlation trivial. You can pair OTEL with a local file exporter, Phoenix, or a custom SQLite writer, and later forward spans to Langfuse, Datadog, or Grafana without changing instrumentation code. Helicone, by contrast, is proxy-based with **50–80ms overhead** and only captures HTTP API calls — entirely unsuitable. LangSmith and Langfuse both support tool-call-level granularity but require cloud or heavy self-hosted infrastructure.

| Platform | Local/offline | Write overhead | Tool-call granularity | CLI-first |
|---|---|---|---|---|
| OpenTelemetry (raw) | ✅ Library-only | **~35μs** (Python) | ✅ Custom spans | ✅ |
| Arize Phoenix | ✅ SQLite default | ~35μs + collector | ✅ Typed tool spans | Partial |
| Langfuse | ⚠️ PG+CH+Redis+S3 | Near-zero async | ✅ OTEL-native | ⚠️ |
| LangSmith | ❌ Enterprise self-host | Low async | ✅ RunTree model | ❌ |
| Helicone | ❌ Proxy architecture | **50–80ms** | ❌ LLM calls only | ❌ |
| Weave (W&B) | ❌ Requires API key | Async | ✅ `@weave.op` | ❌ |
| AgentOps | ❌ Cloud dashboard | Async | ✅ Session waterfall | ⚠️ |

---

## Claude Code hooks are the richest extensibility surface available

Claude Code provides **21 lifecycle events** — the most comprehensive hook system of any CLI coding agent. The core events for a control plane are `PreToolUse` (fires before tool execution, can approve/deny/modify input) and `PostToolUse` (fires after, receives tool response). Hooks are **synchronous and blocking** with a 10-minute timeout, and receive structured JSON via stdin containing `session_id`, `tool_name`, `tool_input`, `tool_response`, and critically, `tool_use_id` — a unique identifier that enables pre/post correlation without any additional infrastructure.

Four handler types exist: **command** (shell scripts receiving JSON on stdin), **HTTP** (POST to an endpoint), **prompt** (single-turn Claude evaluation), and **agent** (spawns subagents with Read/Grep/Glob access). Configuration lives in `~/.claude/settings.json` (user-wide), `.claude/settings.json` (project-level), or `.claude/settings.local.json` (local overrides), with regex matchers filtering by tool name.

The ecosystem around these hooks is surprisingly active. **disler/claude-code-hooks-multi-agent-observability** (1,100+ stars) provides the closest existing implementation to the described architecture: hook scripts POST events to a local Bun server that stores them in SQLite and streams via WebSocket to a Vue dashboard. Other notable projects include **ColeMurray/claude-code-otel** (Grafana-based OTEL dashboards), **TechNickAI/claude_telemetry** (drop-in `claudia` wrapper with <10ms overhead), and **blackwell-systems/claudewatch** (uniquely exposes MCP tools so Claude can query its *own* performance metrics). For security gating specifically, **lasso-security/claude-hooks** scans tool outputs for prompt injection across 5 attack categories, and **karanb192/claude-code-hooks** provides ready-made gates for dangerous commands and secret file access.

Among competing agents, **Cursor gained hooks in v1.7** (beta) with events like `afterFileEdit`, `beforeShellExecution`, and `stop` — simpler than Claude Code's system. **Aider, GitHub Copilot's Coding Agent, and OpenAI Codex CLI have no hook systems.** This makes Claude Code the clear leader in extensibility for building external control planes.

---

## The optimal logging architecture is deceptively simple

The fastest path for logging auto-approved tool calls without blocking the agent is a three-stage pipeline: **JSONL file append → inotify-based daemon tailing → SQLite WAL batch insert**. This architecture achieves sub-microsecond producer overhead, durable buffering, and full Unix-tool compatibility.

**Producer side (~0.1–1μs per event):** POSIX guarantees that `O_APPEND` writes are atomic — the seek-to-end and write happen as a single operation. On Linux ext4, writes up to at least 1MB are empirically atomic for regular files. A single `write()` syscall with a pre-serialized JSON line costs approximately **0.1–1μs** (kernel buffer memcpy, no disk I/O wait). The producer has zero coupling to the daemon — if the daemon is down, events accumulate in the file and are processed on restart. This is ideal for the "no latency on the critical path" requirement.

**Daemon ingestion:** The `notify` crate (used by rust-analyzer, cargo-watch, Deno) provides inotify/kqueue file watching with **sub-millisecond event delivery**. When the JSONL file is modified, the daemon reads new lines from a tracked offset, parses JSON, and batch-inserts into SQLite. The `linemux` crate wraps this into an async line-tailing API. File offset checkpointing (stored in SQLite itself) enables crash-recovery.

**SQLite WAL mode** allows unlimited concurrent readers plus one writer simultaneously — perfect for a TUI reading while the daemon writes. With `PRAGMA synchronous = NORMAL` and batch inserts of 50–100 rows per transaction, throughput reaches **50,000–100,000 events/sec** easily — orders of magnitude beyond what an AI coding agent generates (~10–100 events/sec). The recommended configuration:

```sql
PRAGMA journal_mode = WAL;
PRAGMA synchronous = NORMAL;
PRAGMA cache_size = -64000;
PRAGMA busy_timeout = 5000;
PRAGMA temp_store = MEMORY;
PRAGMA mmap_size = 268435456;
```

Alternatives like Unix domain sockets (~1.4μs p50), named pipes (~4μs p50), or shared-memory ring buffers (~20ns) offer no meaningful advantage over file-append for this workload — and lose the durability and inspectability benefits. Vector and Fluent Bit both lack native SQLite output plugins, making a custom daemon (~200–400 lines of Rust) the pragmatic choice.

---

## Agent gating tools are emerging but none fully solve the problem

Several open-source projects now address AI agent action gating with audit trails, though the space is less than a year old.

**Aegis (AgentLify Labs)** is architecturally closest to the described system: a Go daemon (`aegisd`) using an embedded OPA/Rego policy engine for sub-millisecond decisions, with a **SHA-256 hash-chained append-only event log** stored in SQLite. It supports MCP proxy mode, capability manifests per agent, deterministic session replay, and a CLI (`aegisctl`). It's early-stage but well-designed.

**Microsoft's Agent Governance Toolkit** (285 stars, MIT) achieves the most impressive latency numbers: **<0.1ms per action** using deterministic, non-LLM policy evaluation. It supports 14+ agent frameworks, Ed25519 identity with trust scoring, and append-only audit logs. Production evidence: "473 denials across 11 agents over 11 days, each caught deterministically in <8ms." The tradeoff: it's Python middleware (same-process), not an external daemon.

**SafeClaw (AUTHENSOR)** most directly addresses the "auto-approve without blocking" pattern — read-only tools (`Read`, `Glob`, `Grep`) are **auto-allowed locally without any control plane call**, while non-read operations go through the gate. Its audit ledger uses append-only JSONL with SHA-256 hash chaining. **NVIDIA's OpenShell/NemoClaw** (March 2026 preview) takes a different approach with Landlock + seccomp + network namespace sandboxing, operating at the filesystem/network level rather than individual tool calls.

MCP (Model Context Protocol) itself has **no built-in audit or gating mechanisms** — it is "neutral on governance." This gap has spawned MCP gateways like Lasso MCP Gateway (plugin-based interception with structured JSON logging) and Peta MCP Gateway (tamper-evident per-call logging).

- **Aegis** — Go daemon, OPA policy, hash-chained audit, MCP proxy, CLI-first
- **MS Governance Toolkit** — <0.1ms Python middleware, Ed25519 identity, broadest framework support
- **SafeClaw** — Auto-allows read-only tools without blocking, JSONL+SHA-256 audit
- **AEGIS Firewall** — Academic paper (arXiv:2603.12621), 8.3ms median latency, 14 framework support
- **AgentWard** — Permission control plane, scan/setup/inspect modes, very early stage

---

## Rust crates provide a battle-tested stack for the daemon

The Rust ecosystem has mature, high-performance crates for every component of the described architecture. Total producer-side overhead using these crates: **~5–10μs per event**.

**Event ID generation** uses `memmap2::MmapRaw` to create a shared memory-mapped `AtomicU64` counter backed by a file in `/dev/shm/`. Cross-process `fetch_add` costs **~5–20ns** — negligible. The crate is actively maintained (fork of memmap-rs by RazrFalcon), with `MmapRaw` providing raw pointer access that avoids handing out references to externally-modifiable memory. For simpler correlation, `{pid}-{monotonic_counter}` as a correlation ID requires no shared state at all, since each agent session is a single process.

**JSON serialization** with `serde_json` is **faster than simd-json for small payloads** — a counterintuitive result confirmed by independent benchmarks. For typical 200–500 byte tool-call events, `serde_json::to_string()` takes **~1–2μs**. The SIMD instruction overhead in simd-json only pays off above ~1KB. With 26,916 dependents, serde_json's ecosystem compatibility is unmatched.

**File watching** via the `notify` crate (v7.x, 19M+ downloads) provides cross-platform inotify/kqueue/FSEvents support. In raw mode (no debouncing), events arrive within microseconds of the file write. The `linemux` crate wraps notify + file position tracking into an async tailing API: `MuxedLines::new()?.add_file("events.jsonl").await?`. Expected end-to-end latency from file write to daemon seeing the new line: **~1–5ms**.

**SQLite access** through `rusqlite` with WAL mode and `prepare_cached()` statements achieves **3,000,000 rows/sec** in aggressive benchmarks. For the realistic case (WAL mode, `synchronous = NORMAL`, batches of 50–100 rows), expect **50,000–100,000 events/sec** sustained. Use `r2d2-sqlite` (93K downloads/month) for connection pooling with one dedicated writer connection and 3–4 reader connections for the TUI. Critical: disable r2d2's `idle_timeout` to prevent WAL file removal.

**The TUI dashboard** uses `ratatui` (19,100 stars, successor to tui-rs) with the `crossterm` backend. The pattern: a main thread runs the render loop at 10–30 FPS while a background thread polls SQLite and sends updates via `mpsc::channel`. Table widgets display event streams, Sparkline widgets show latency distributions, and Gauge widgets track correlation status.

| Crate | Purpose | Overhead | Maturity |
|---|---|---|---|
| `memmap2` | Shared counter via mmap | ~5–20ns | ★★★★★ Core ecosystem |
| `serde_json` | Event serialization | ~1–2μs | ★★★★★ 26K dependents |
| `notify` 7.x | File change detection | Sub-ms delivery | ★★★★★ Used by rust-analyzer |
| `linemux` | Async file tailing | ~1–5ms end-to-end | ★★★★ Stable |
| `rusqlite` | SQLite WAL writes | 50K–100K events/sec | ★★★★★ Standard choice |
| `r2d2-sqlite` | Connection pooling | Negligible | ★★★★ 93K downloads/mo |
| `ratatui` 0.30+ | TUI dashboard | Sub-ms rendering | ★★★★★ 19K stars |

---

## Conclusion: build it yourself, from proven primitives

The landscape reveals a clear gap: no existing tool combines local-first operation, sub-millisecond tool-call gating, append-only audit logging, pre/post event correlation, and a TUI dashboard in a single Unix-philosophy package. The observability platforms are too cloud-heavy; the gating tools are too early-stage; the Claude Code hook ecosystem provides excellent building blocks but no integrated control plane.

The strongest architectural insight from this research is that **the JSONL file is the ideal IPC mechanism** — not a Unix domain socket, not a shared memory ring buffer, not a direct SQLite write. O_APPEND file writes give sub-microsecond producer latency, durable buffering independent of daemon state, and full Unix-tool inspectability (`tail -f events.jsonl | jq`). This is a genuinely better design than what any existing observability platform uses.

Three implementation decisions stand out as non-obvious. First, **serde_json outperforms simd-json for small payloads** — don't optimize prematurely. Second, **`tool_use_id` from Claude Code's hook payload eliminates the need for shared-memory counters** for event correlation in the single-agent case; `memmap2`-based atomic counters become necessary only for multi-agent session coordination. Third, **the Microsoft Agent Governance Toolkit's <0.1ms deterministic policy evaluation** proves that even synchronous gating can be fast enough to be effectively non-blocking — the "async observability" requirement may be over-engineering for the policy decision itself, though it remains essential for the audit logging path.

For the near-term, the recommended approach is: fork the architecture pattern from disler's observability project (hooks → local server → SQLite → dashboard), rewrite the hot path in Rust using the crate stack above, and integrate OPA/Rego-style policy evaluation from Aegis for the gating layer. The resulting system would be the first truly Unix-philosophy, daemon-based control plane for AI coding agents — a tool the ecosystem clearly needs but doesn't yet have.