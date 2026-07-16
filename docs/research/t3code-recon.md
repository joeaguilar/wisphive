# Recon: t3code vs. Wisphive agents mode

_Last reviewed: 2026-07-15 (multi-agent recon of `inspiration/t3code` against Wisphive; every
learning below survived an evidence lens and a security lens. Claims that were narrowed by a
verifier appear here in their narrowed form only.)_

## The answer up front

t3code never spawns an agent the way Wisphive does. For Claude it does not spawn at all — it
imports `@anthropic-ai/claude-agent-sdk` and lets the SDK own the child. For Codex it spawns a
long-lived `codex app-server` and speaks JSON-RPC over its stdio. For opencode it spawns an HTTP
server and talks to it over loopback TCP. In all three cases **t3code owns the session transport,
and the approval gate is a function of that transport** — it can only gate sessions it launched.

Wisphive's gate is the opposite by construction: `wisphive-hook` is a subprocess that the *provider*
invokes from its own config, so an operator's own interactive `claude` in a terminal is gated
identically to a daemon-spawned one. That is a capability t3code structurally cannot have, and it is
the single most important thing not to trade away.

So the strategic answer is: **own the observation model, not the session.** Almost everything
t3code buys with transport ownership that Wisphive actually wants — completion reason, error text,
turn counts, "is agent 17 stuck or thinking" — is reachable by un-nulling a file descriptor
Wisphive already asked the child to write to. The rest is either already solved better here, or is a
gate regression wearing an isolation costume.

The single highest-value finding in this recon is not from t3code at all. It surfaced while a
verifier was trying to refute a t3code-derived claim: **the daemon's connection semaphore fails
OPEN at capacity, silently, and a wave is the workload that reaches it.** See L1.

## Mechanical spawn, per provider

### t3code → Claude Code

No spawn. `apps/server/package.json:25` depends on `@anthropic-ai/claude-agent-sdk ^0.3.170`;
`ClaudeAdapter.ts:22` imports it and `:1364` calls `query({prompt, options})`, consuming an
`AsyncIterable<SDKMessage>`. The SDK owns the child; t3code only names the binary via
`pathToClaudeCodeExecutable` (`:3446`, defaulting to bare `"claude"` on PATH).

- **Gate:** the `canUseTool` promise callback (`:3405-3406`, passed at `:3464`), returning a
  `PermissionResult`. Recon confirmed zero occurrences of `allowedTools`/`disallowedTools`/
  `maxTurns`/`hooks`/`appendSystemPrompt` in the adapter — `canUseTool` is the whole gate.
- **Settings:** `settingSources: ["user","project","local"]` (`:3448`).
- **Bypass retained:** `permissionMode: "bypassPermissions"` ⇒ `allowDangerouslySkipPermissions:
  true` (`:3457-3459`). `canUseTool` still fires under it — the `AskUserQuestion` (`:3266`) and
  `ExitPlanMode` (`:3270`) handlers sit *above* the full-access short-circuit at `:3292-3298`, and
  the comment at `:3263-3265` says "regardless of runtime mode".
- **Free-text flag passthrough:** `launchArgs` → `parseCliArgs(...).flags` → spread as `extraArgs`
  (`:3409`, `:3467`).
- **Control channel:** `setModel` (`:3663`), `setPermissionMode` (`:3680`/`:3685`),
  `getContextUsage?()` (`:1783`), `interrupt()` (`:3748`), `close()` (`:2990`).

### t3code → Codex

`ChildProcess.make("codex", ["app-server", ...])` (`CodexSessionRuntime.ts:716-736`), long-lived,
NDJSON-RPC over the child's stdio (`packages/effect-codex-app-server/src/_internal/stdio.ts:13-22`).
Handshake `initialize` → `initialized` (`:1202-1203`). `CODEX_HOME` is spread **last** into the env
(`:717-720`), clobbering the operator's value.

- **Gate:** in-process JSON-RPC server→client calls — `item/commandExecution/requestApproval`
  (`:952`), `item/fileChange/requestApproval` (`:1008`), `item/tool/requestUserInput` (`:1066`).
  Each mints a `Deferred`, registers it in a pending-approvals map, emits a UI event, then blocks on
  `Deferred.await` and returns the decision as the RPC response — structurally the same shape as
  Wisphive's oneshot.
- **Posture is in-band:** `approvalPolicy` / `sandbox` carried on `thread/start` params
  (`:289-303`), re-asserted per turn (`:381-389`).
- **Unknown method → `methodNotFound`** (`:1116-1118`). Correct fail-closed posture; steal this.
- **Teardown:** `CODEX_APP_SERVER_FORCE_KILL_AFTER = "2 seconds"` (`:56`) as `forceKillAfter`
  (`:733`) — SIGTERM, then SIGKILL 2s later. `close` settles every pending approval with `cancel`
  (`:1241-1260`).
- **Silent resume→fresh fallback:** a `thread/resume` failing a substring-matched
  "recoverable" check falls back to `thread/start` under the same canonical ThreadId
  (`:463-478`). Logged as a warning, but the caller sees success.

### t3code → opencode

`["serve", "--hostname=127.0.0.1", "--port=<ephemeral>"]` (`opencodeRuntime.ts:342`), `detached:
true` on non-Windows (`:348`) purely to enable a process-**group** kill (`process.kill(-pid, sig)`,
`:374`). Port chosen by a reserve-then-release TOCTOU probe (`Net.ts:151-175`).

- **Config blanked deliberately:** `OPENCODE_CONFIG_CONTENT: "{}"` (`:37`, `:352`) — the child is
  deterministic and policy flows in-band via `permission:` at `session.create`
  (`Layers/OpenCodeAdapter.ts:1076`), surfacing as `permission.asked`/`permission.replied` events.
- **Unauthenticated:** no `--password`. The Basic-auth builder (`:505-517`) is wired only for
  **external** servers (`Layers/OpenCodeAdapter.ts:1055`; mirrored `OpenCodeProvider.ts:423-425`,
  `OpenCodeTextGeneration.ts:388`). A spawned server's `permission.reply` is reachable by any local
  UID.
- **Teardown:** `killpg(SIGTERM)` → `sleep 1s` → `killpg(SIGKILL)`, `Effect.ignore`d, registered via
  `Scope.addFinalizer` (`:381-386`), plus a layer-level finalizer stopping every session
  (`Layers/OpenCodeAdapter.ts:490-510`).

### Wisphive → both

`build_agent_command` (`process_registry.rs:2150-2252`) constructs a closed argv from a typed
`SpawnAgentRequest` — no free-text passthrough anywhere:

```
claude -p --setting-sources project --dangerously-skip-permissions --session-id <uuid4> \
       [--model|--name|--effort|--max-turns|--permission-mode|--system-prompt|--tools|
        --output-format|--verbose] -- <prompt>

codex exec --sandbox workspace-write --skip-git-repo-check --dangerously-bypass-hook-trust \
       -C <project> [--model] [--config model_reasoning_effort="…"] [--json] <prompt>
```

Then, unconditionally (`:2246-2250`):

```rust
// Managed children must not inherit the daemon's terminal input. Any
// interactive bytes would bypass the reviewed SpawnAgent request.
cmd.stdin(Stdio::null());
cmd.stdout(Stdio::null());
cmd.stderr(Stdio::null());
```

The comment justifies **stdin only**. `git log -S` shows the stdout/stderr nulls predate it: before
`2102fb7` the line read *"Managed output is not surfaced yet; avoid child pipe backpressure"* — a
true and still-load-bearing constraint whose removal is exactly why "just pipe it" now looks free.

Everything else is gate proof: `require_managed_spawn_mode` (`:2348`, re-checked `:2498`),
`validate_spawn_request` (`:60-161`), the Claude project-hook audit (`:2366-2407`) or the Codex
effective-inventory walk (`:2426-2467`), `audit_codex_session_argv` (`:1719-1779`), and a
verify→spawn→verify TOCTOU sandwich (`:2505-2549`). Post-launch the daemon sees exactly two things:
hook events, and an exit code from `reap_exited` (`:2611-2632`).

## Delta table

| Axis | t3code | Wisphive | Who's right |
|---|---|---|---|
| Gate location | In the transport it owns | External subprocess the provider invokes | **Wisphive.** Gates sessions it didn't launch. |
| Gate coverage | Sessions t3code launched | Every gated session on the box | **Wisphive**, decisively. |
| Fail posture | Undefined; decided by the vendor | ADR-0001 tiered, ADR-0010 fail-closed validators, keyed on `mode` with zero processes running | **Wisphive.** |
| argv | Free-text `launchArgs` → `extraArgs` | Closed typed constructor, jailbreak-scanned, byte-bounded | **Wisphive.** |
| Child config | Blanked (`OPENCODE_CONFIG_CONTENT="{}"`), policy in-band | Re-derived from the operator's live `~/.codex` (~900 impl lines) | **t3code's direction** — this is ADR-0009's thesis, independently confirmed. |
| Child stdout | Consumed, normalized, persisted | `/dev/null` | **t3code.** |
| Teardown | SIGTERM → grace → SIGKILL, process **group** | `child.kill()` = SIGKILL, direct PID only, doc comment says SIGTERM | **t3code**, with caveats (L6). |
| Session records | Persisted (`004_ProviderSessionRuntime`), reaper sweeps | In-memory `HashMap`, no table, no crash reconcile | **t3code.** |
| Concurrency cap | None (unbounded `Map`) | None on *running* agents (8 on *pending approvals*) | **Neither.** |
| Unknown method/type | Parse-must-succeed → "unavailable" shadow | Closed enum → hard bail | **Wisphive.** Unknown gate semantics must never launch. |
| Correlation | Transport owns it (`Deferred` per request) | `queue.rs:16` `pending_senders: HashMap<Uuid, oneshot::Sender<RichDecision>>` | **Tie** — same shape, independently. |
| Reconnect | Cursor replay (`afterSequence`) | Full re-snapshot | **Wisphive** for decisions (see Rejected). |
| Client dedupe | Monotonic cursor guard | None on queue/agents (audit arm has one) | **t3code.** |
| Adapter seam | Real: 14-member `ProviderAdapterShape`, injectable `createQuery` | `AgentAdapter` — orphan crate, zero consumers | **t3code**, or delete ours. |

## Ranked learnings

Ordered by value ÷ effort. Everything below survived both lenses; where a lens narrowed a claim,
the narrowed form is what appears.

### Tier 1 — do now, small, low risk

**L1. The connection semaphore fails OPEN at capacity — a wave is the workload that reaches it.**
`MAX_CONCURRENT_CONNECTIONS = 256` (`server.rs:30`) is shared by every client kind, and the permit
is taken in the accept loop (`:477`) *before* Hello. Blocking hooks hold theirs for up to
`hook_timeout_secs = 3600` (`config.rs:28`). At capacity, `reject_connection_at_capacity`
(`:557-563`) writes `ServerMessage::Error` **at the welcome position** and closes — it never
constructs a handler, so it never sends a `Welcome` (`:681`). The hook reads that as its welcome,
fails the `matches!(welcome, ServerMessage::Welcome{..})` check (`main.rs:1512-1519`), classifies it
`HookFailureKind::DaemonUnreachable`, and `response_for_failure` (`:673-675`) returns `approve()`
*before* the fail-mode branch at `:679`. The 257th tool call is silently allowed regardless of
`fail-mode=closed`.

This was proved empirically against release binaries in an isolated HOME: 260 hooks spawned, 256
blocked holding the pool, the excess exited with a silent allow; a probe tool call while saturated
returned approve with no human review and no `decision_log` row. The existing unit test
`socket_error_response_fails_closed` (`main.rs:3722-3747`) sends the Error *after* a Welcome, so it
models a live session and gives false confidence about this path.

This is an over-broad application of ADR-0001. CLAUDE.md scopes fail-open to *"refused/absent socket
— the daemon is down."* An at-capacity daemon is up, reachable, and deliberately refusing — the
reachable-but-refusing case ADR-0001 assigns to fail-mode. Fix is two-part: classify pre-Welcome
`Error` as `Runtime` (not `DaemonUnreachable`) so it honors fail-mode, and reserve a slice of
permits only non-hook clients may draw from so a wave cannot evict the operator's ability to gate
it. Note this finding *emerged from refuting* a t3code-derived learning that asserted the cliff was
fail-closed — see Rejected.

**Value: very high. Effort: small-medium. Security: closes a volume-reachable gate bypass.**

**L2. `stop_agent`'s doc comment lies about the signal.** `process_registry.rs:2585` says "Stop an
agent process by sending SIGTERM"; `:2593` says "Try graceful kill first"; `:2594` calls
`proc.child.kill().await`, which is SIGKILL. Grep for `SIGTERM|libc::kill|signal::kill|process_group|
setsid|killpg|kill_on_drop` across `crates/**/*.rs` returns exactly two hits: this lying comment, and
an unrelated `kill_on_drop(true)` in `worktree.rs:76`. Fix the comment even if the behavior never
changes — its falsity is how the missing grace window and the missing process group stayed invisible.
Leave `start_kill()` at `:2537` as SIGKILL; it is the ADR-0009 TOCTOU-tamper abort and must keep zero
grace.

**Value: medium. Effort: trivial. Security: none.**

**L3. Delete `wisphive_adapters`.** Verified orphan: zero references to `AgentAdapter` anywhere
outside the crate, and no crate lists it as a dependency — only the workspace member entry
(`Cargo.toml:9`) and an unconsumed `[workspace.dependencies]` line (`:30`). `AdapterEvent` isn't even
re-exported (`lib.rs:1` `mod adapter;` is private), so the trait is unimplementable externally. All
four impls are no-ops; `claude_code.rs:52-56` is a literal `Ok(())`.

The reason to delete rather than fix is default gravity. `respond(&self, agent_id, decision)`
(`adapter.rs:27`) is addressed to an **agent**, not a **request** — it cannot answer one of several
in-flight approvals. Someone tasked with "add ACP support" will find a trait named `AgentAdapter`
with a `RedAdapter` stub in it and hang a correlation side-table off `respond()`, hand-rebuilding
what `queue.rs:16` already does correctly. That is not hypothetical: `queue.rs:61-64` records that
the decision id is hook-supplied and "attacker-influenced over the local socket," that silently
overwriting drops the victim's oneshot sender — *"an instant fail-open approve"* — and that the
daemon therefore rejects duplicate ids fail-closed (itr#370, `server.rs:814`). A hand-rolled
correlation table rebuilds precisely the hazard already paid for. Deleting forces the work into
`process_registry.rs` + a bridge module, which is where `docs/plan-red-support.md:140-144` already
says it belongs. Fix CLAUDE.md's "seven workspace crates with clear dependency flow" in the same PR.

**Value: high. Effort: small. Security: none directly; removes an attractive nuisance.**

**L4. Cap running agents, not just pending approvals.** `MAX_PENDING_SPAWNS = 8` (`server.rs:50`)
bounds the pending-approval queue only — its own comment scopes it to *"lifetime and queue
cardinality."* Approve 8, they launch and drain, approve 8 more: 16 live. `spawn_agent` inserts into
`processes` (`:2574`) with no check against `len()` (`:2645`), which is used only by a shutdown log
line (`server.rs:527`). t3code is not the model — it has the identical hole (unbounded
`Map<ThreadId, SessionContext>`, `CodexAdapter.ts:1367`); it is evidence that the hole is what you
get by default. Add `max_concurrent_agents` to `config.json`, enforced inside `spawn_agent` under the
existing registry mutex (race-free there), refusing with a first-class named error so "at capacity"
is distinguishable from "gate broken" (the itr#538 BRICKED distinction). Reap before comparing —
`reap_exited` runs on a tick, so `len()` counts dead-but-unreaped agents.

This composes with L1: uncapped agents are how you *reach* 256 blocked hooks, so the cap is a
gate-integrity control, not housekeeping. It is necessary but not sufficient — hand-run Claude
sessions draw from the same 256 permits.

**Value: high. Effort: small. Security: narrowing only; a cap can only refuse.**

**L5. Client-side dedupe on the two blind-append arms.** Wisphive independently got t3code's
non-obvious half — `server.rs:1173` subscribes *before* the snapshot writes, with the same rationale
t3code states at `ws.ts:1174-1180`. It skipped the easy half. A decision enqueued between
`subscribe()` (`:1173`, no lock) and `q.snapshot()` (`:1189-1192`, takes the lock) lands in both the
broadcast buffer and the snapshot, and `useWisphive.ts:297-301` blind-appends
(`[...prev.queue, req]`), as does `:344-352` for agents. The audit arm in the same file already
dedupes via `mergeAuditDecisions` (`:948-961`) — three streams share one connect path, one dedupes,
two don't.

Dedupe by `req.id` / `info.agent_id`, **not** by content — `auditKey` (`:931-946`) is content-keyed
and copying that shape would collapse two genuinely distinct decisions. Keying on `id` is safe
precisely because itr#370 guarantees daemon-side pending-id uniqueness. Duplicates self-heal on
resolve (`:303-308` filters by id), which is exactly why this will never be noticed and never fixed.

**Value: medium. Effort: small (~6 lines). Security: none — display artifact only.**

### Tier 2 — high value, medium effort

**L6. Kill the process group, with the escape window understood.** There is no `setsid`/
`process_group`/`pre_exec` on the managed spawn path, so children inherit the daemon's process group
and `child.kill()` signals only the direct PID. `claude -p` spawns Bash tool subprocesses; any alive
at the instant of `StopAgent` is **orphaned and survives**. `shutdown_all` (`:2635-2642`) delegates
to `stop_agent`, so daemon teardown leaks them too. This is the real defect — worse than the
half-written-file story usually told, because a "stop the wave" button that reports 30 agents dead
while their grandchildren keep writing to 30 worktrees is worse than one that truncates a file.

Four constraints the naive fix misses:

1. **Use `process_group(0)`, not `setsid()`.** `setsid` detaches the child from the daemon's
   controlling terminal, destroying the terminal-SIGHUP backstop in the one path the in-memory
   registry cannot cover (daemon SIGKILL/OOM/panic). `process_group(0)` gets a killable group and
   keeps the backstop.
2. **Set it before any `killpg`.** Children currently share the daemon's pgid; a `killpg` against
   the inherited value signals the daemon itself.
3. **`killpg` strictly before reaping.** The un-reaped zombie leader is what pins the pgid against
   OS reuse; `reap_exited` already `try_wait`s children out from under the registry on a timer, so a
   recorded pgid can be reaped-but-recorded and a `killpg` on a recycled pgid hits an unrelated
   group.
4. **Do not adopt t3code's TERM→1s→KILL ladder unmodified for the operator's stop button.** SIGTERM
   is catchable and `killpg` broadcasts it, giving every member a 1s window to `setsid` out of the
   group and survive the follow-up SIGKILL entirely. t3code's ladder is tuned for a cooperative
   first-party `opencode serve`; the managed agent is the party ADR-0008 explicitly does not trust.
   Worse, under the shipped default (`auto_approve_level=all` in *both* presets) every tool call in
   a grace window is auto-approved with no human in the loop — the grace turns an emergency stop into
   "keep running, unsupervised, for N seconds." Either offer two controls (graceful stop with grace;
   emergency stop = SIGKILL now, wired to the wave button), or flip `mode` before the window.

Also: `stop_agent` holds the registry mutex (`server.rs:2566-2569`) and `shutdown_all` loops
serially — 20 agents × 2s is 40s of lock-held shutdown. Any grace must not hold the mutex and must
issue SIGTERM concurrently.

**Value: high. Effort: medium. Security: low — no gate bypass, but it touches an
incident-response control and sits 50 lines from an ADR-0009 enforcement kill.**

**L7. Persist a managed-agent record and reconcile on restart.** `processes` is a bare in-memory
`HashMap` (`:2256`) holding a tokio `Child`; `state/migrate.rs` has no agents table. PTY sessions
*are* persisted (`terminal_sessions`, `:106`) and *are* reconciled at startup
(`mark_running_terminals_orphaned`, `state/terminals.rs:276-281`) — the pattern is already accepted
here. `shutdown_all` runs only on the graceful path (`server.rs:523-530`); `Server::new` drains
decisions (`:210`) and nothing for processes.

Compose the facts: a daemon SIGKILL/OOM/panic mid-wave leaves N `claude -p
--dangerously-skip-permissions` children whose only gate — per the code's own comment at
`:2362-2365` — is a hook that now takes the `DaemonUnreachable` branch and approves
(`main.rs:670-675`). Two narrowings the lenses insisted on, both of which matter:

- **The exposure is bounded, not permanent.** The hook reconnects fresh on every invocation
  (`main.rs:1417-1425`), so orphans are re-gated the moment a daemon is back. The window is
  crash→restart. The risk is "unsupervised for the outage window," not "a permanently ungated fleet."
- **The primary fix may not be persistence at all.** The daemon already stamps `WISPHIVE_AGENT_ID`
  into every managed child (`:2244`) and the hook *already reads it* (`main.rs:357`) — today only as
  an attribution fallback. That is an existing in-band signal for "daemon-managed headless spawn,"
  available at `response_for_failure`, the exact chokepoint ADR-0001 designates as the classifier.
  Making the `DaemonUnreachable` carve-out managed-spawn-aware (approve for interactive; deny or
  honor `fail-mode` for managed) *prevents* the ungated window rather than reaping after it — and
  it is squarely within ADR-0001's own rationale, which justifies fail-open solely by "don't brick
  the fleet," a concern about the operator's interactive agents, not about a headless agent whose
  supervisor is dead and who has no human to unblock it. The ADR-0001 amendment is the primary
  deliverable; persistence is the complementary observability/hygiene fix.

If persisted: `(agent_id, pid, start_time, agent_type, project)`, verify `start_time` before acting
so PID reuse can't kill an innocent process, write an intent row keyed by `agent_id` *before* spawn
and fill the PID after (the fork→write window is irreducible). Do **not** propose `pgid` until L6
lands. Do **not** claim "re-adopt into the registry" — a restarted daemon isn't the parent, so no
`waitpid`, and `ManagedProcess`'s tokio `Child` cannot be rebuilt from a PID; the reconciler can only
verify, kill, and record. Never re-launch: a silently respawned agent is an unreviewed spawn. Default
should be re-adopt/report rather than kill, since a restarted daemon re-gates automatically and
default-kill destroys in-flight work.

**Value: high. Effort: medium. Security: interacts with ADR-0001; the fail-open itself is correct
and must not be touched for interactive sessions.**

**L8. Un-null stderr first, then stdout, with a drain task.** `output_format` is a real user-facing
CLI flag (`wisphive_cli/src/main.rs:386-391`), validated against `text|json|stream-json`
(`:129-133`), given a coupled invariant (`stream-json ⇒ verbose`, `:135-140`), threaded into argv for
Claude (`:2201-2203`) and collapsed to `--json` for Codex (`:2228-2234`) — and then discarded at
`:2249`. The field *has* read sites; what it lacks is a consumption site for the output those flags
produce. That is the actual defect, and it is dead configuration surface that reads as a capability.
It is rendered into the human approval payload too: `spawn_approval_request` (`server.rs:1935-1949`)
serializes the whole request into `tool_input`, so an operator approving a spawn sees
`output_format: "stream-json"` doing nothing.

Scope the claim honestly. The daemon is **not** blind — it sees every gated tool call, every
`PostToolUse` result, and the burn meter already derives real signal from `decision_log`. The gap is
the narrative/usage layer: assistant text, turn counts, token usage, the terminal `result` record,
and the child's own error text. And a status column is buildable today (`AgentStatus` Running/Exited,
`reap_exited` yields the exit code). What stdout uniquely unlocks is **discriminating a stuck agent
from a thinking one** — both emit zero tool calls. That is the justification, not the status column.

Constraints, all necessary:

- **`stdin` stays `Stdio::null()`.** That is the half of `:2248-2250` that is security-load-bearing.
  Un-nulling stdout must not be cover to touch it.
- **`Stdio::piped()` alone is a regression.** With no reader the child blocks at the ~64KB pipe
  buffer and `try_wait()` returns `Ok(None)` forever — a hung, unreapable agent. `Stdio::null()` is a
  *correct default* absent a drain, not an oversight. Pipe + drain land as one unit.
- **Bound by draining, never by closing.** Dropping the read end gives the child EPIPE mid-wave.
  Retain ~N bytes, discard past the cap, keep reading to EOF for the process lifetime. t3code's own
  `collectUint8StreamText.ts` says it: *"keep draining after truncation so the child process can exit
  normally."* Note t3code bounds only its update path (`providerMaintenanceRunner.ts:32`,
  `UPDATE_OUTPUT_MAX_BYTES = 10_000`); its probe path is unbounded — be stricter than t3code here.
- **Never into `events.jsonl`.** That file's trust rests on a sole-writer invariant
  (`event_ingest.rs:13-14`) enforced by itr#425 (`main.rs:1253-1257` names it explicitly), and
  `event_ingest.rs:461` lifts `decided_by` verbatim into `decision_log` (`:471-482`) and broadcasts
  it (`:512`). Piping child stdout there lets a gated agent forge audit records with a `println!`.
  Redaction does not mitigate this — `redact` scrubs secret *values*, not *provenance*. Use a
  separate, structurally distinct sink stamped daemon-side as agent-origin/untrusted, which can never
  parse as a decision record.
- **Redaction is necessary but not sufficient.** `redact.rs` is explicitly regex-free — a token
  scanner over three rules tuned for structured tool inputs. It will not catch secrets in free-form
  prose (PEM bodies, `postgres://user:pass@host`, paraphrased credentials). Either restrict the
  persisted sink to structured record types where its assumptions hold, or extend the scrubber for
  prose — and that extension is its own itr issue with its own review.
- **Gate-neutral only under a display contract.** Wisphive's decision function is a human. Rendering
  agent-authored prose next to pending decisions is an injection channel to the approver ("this next
  Bash call is routine cleanup already approved by the operator"). The sink is write-only forensics
  by default, queryable post-hoc, never auto-rendered adjacent to an undecided queue row; the
  structured `result` record is daemon-parseable and safe to surface, free-text assistant content is
  not.
- **Cap and honor `archive_alert_max_mb` / `disk_alert_free_mb`.** The state dir's audit archive is
  never auto-deleted (itr#340), so an uncapped agent-controlled writer is a disk-DoS — and per
  ADR-0010 a full filesystem denies every hook event, i.e. a self-inflicted brick.
- **Steal t3code's unknown-message discipline verbatim.** `handleSdkMessage`'s `default:` branch
  warns via `describeUnknownSdkMessage` (`ClaudeAdapter.ts:2877-2883`) instead of dropping. Matches
  the rule CLAUDE.md already states for `HookEventType` routing, and matters *more* here because a
  Rust daemon has no SDK and owns the schema drift.
- **Non-JSON lines are expected, not exceptional.** t3code's own probe notes record that at least one
  agent CLI's stream-json degrades to plain text on the same fd under startup failure and rate-limit.
  Probe Claude/Codex directly before committing to a framer — t3code's considered move was *away*
  from line-oriented stream-json toward JSON-RPC.

**Stderr is the higher-value, lower-risk half**: it is CLI-authored rather than model-authored, so it
sidesteps most of the prose-redaction and injection concerns while delivering the login prompt, the
bad-model error, and the version complaint. Ship it first.

Note `itr#22`'s CONTEXT is stale — it says stdout is `Stdio::piped()` "but nothing reads them"; the
code is `Stdio::null()`. Update it before routing.

**Value: high. Effort: medium. Security: low — new untrusted surface, no gate impact, given the
constraints above.**

**L9. Re-attach the lost comment to `:2249` now.** Independent of L8 and free. Today nothing on disk
explains why stdout/stderr are null, which is precisely what makes "just pipe it" look like a
one-liner. This is the highest value-per-character line in the recon.

### Tier 3 — structural, larger, decision-shaped

**L10. Prefer per-agent worktrees for waves; keep the conflict gate for what worktrees can't
isolate.** `docs/plan-cross-agent-conflict-gate.md` concedes the gap in its own semantics section:
hook auto-approved writes are *"not gated in-line; the hook has no map access"* (`:332`, repeated
`:397`). Waves are only fast at `auto_approve_level=write|all` — so for a pure wave, where every
agent auto-approves and no daemon-reviewed request survives to consume a retroactive claim, the
gate's in-line semantics are inert. t3code avoids the problem structurally instead: per-thread
worktrees (`ThreadEnvMode = ["local","worktree"]`, `settings.ts:102`), `git worktree add -b`
(`GitVcsDriverCore.ts:2251-2254`) into a server-owned dir, 1:1 bootstrap dispatch with a compensating
rollback (`ws.ts:691-703`).

Do not oversell it. Worktrees **defer** write conflicts to merge time rather than eliminating them,
and they do nothing for the plan's own `:393` case (two agents writing a shared absolute path), for
Bash mutations (`sed -i`, `mv`, `>`), or for shared DBs/ports. Keep the gate for those and for its
retroactive-claim observability, which survives even at `level=all` and is what feeds the banner.
The gate is deaf for *enforcement* at auto-approve tiers, not blind for *observability*.

Two hard constraints:

1. **Worktree creation is a NEW, separately gated mutating path.** Do not relax `worktree.rs`'s
   allowlist (`:36`) — its read-only claim is structural, module-documented, and load-bearing for the
   strip's zero-write-affordance guarantee (runtime refusal `:64-66`, `GIT_OPTIONAL_LOCKS=0` `:73`).
   Adopt t3code's rollback discipline, mirroring the kill-child-on-stale-audit precedent at `:2537`.
2. **Budget a second mutating path: per-worktree hook provisioning.** Hooks are per-project-directory
   (`hooks.rs:92`), and `audit_codex_effective_hooks` refuses the spawn when the project's hooks file
   is unreadable. A worktree inherits gating only if the hook files are committed — true in wisphive,
   *not* general (`.claude/settings.json` is commonly gitignored). Posture is fail-closed, so no
   ungated-agent hole, but the effort estimate is worse than it looks. And note `allow_self_modification`
   protects only `~/.wisphive/**`, not a project's `.claude/settings.json` — worktree isolation must
   not be sold as closing that gap.

Keep the plan's S1 insight untouched: key ownership on **daemon connection identity**, never on an
agent-authored `agent_id` (`:274-281`). A wave-spawner that keys on anything the agent authors ships
a silent-interleave bug.

**Value: high. Effort: large. Security: see constraints; the CODEX_HOME rejection below is
load-bearing.**

**L11. If ADR-0009's isolated home lands, root it at `~/.wisphive/codex-home/` — but harden it
explicitly.** ADR-0009 specifies *what* the controlled home contains and never *where* it lives, and
that choice is load-bearing. `path_in_dir` (`main.rs:2258-2275`) ends in
`target.starts_with(&dir_resolved)`, so a subdir of `~/.wisphive` falls inside `targets_control_plane`
(`:2220-2240`) and any matching tool call skips both auto-approve layers to human review
(`:1265-1266`). Rooting it anywhere else — `~/.wisphive-codex`, `/tmp/...`, an operator-configured
path à la t3code's free-form `shadowHomePath` (`settings.ts:182-193`) — creates a trust root no
existing guard covers, re-opening the itr#425 hole one directory over while the ADR advertises the
move as a posture improvement.

But the location buys **partial** coverage, not free coverage:

- Coverage is limited to `{Write, Edit, MultiEdit, NotebookEdit, Bash}` — `targets_control_plane`
  returns false for every other name (`_ => None`, `:2235`), while `AutoApproveLevel::includes`
  returns true for *any* name at `level=all`. **Probe a real Codex hook payload for its `tool_name`
  values before claiming coverage**: t3code's generated Codex schema shows the taxonomy is
  `execve`/`applyPatch`/`mcpToolCall`, not Claude's names, and the hook does no Codex tool-name
  normalization. If they don't match, the guard fires for none of the managed Codex child's calls.
- **Do not claim it "inherits ADR-0010's validation."** `read_mode_file` (`main.rs:504-570`)
  validates only `~/.wisphive` and `mode`; no validator inspects subdirs. Provision `codex-home`
  with an explicit `0700` (never bare `create_dir_all` — umask would fabricate `0755`, the exact
  t3code defect, now inside the state dir), seed `auth.json` `0600` per ADR-0009:114, and give the
  home its own O_NOFOLLOW + euid + mode check at provision *and* inside `verify_unchanged`
  (`:739-777`) — because `resolve_existing_ancestor` follows symlinks, a symlinked `codex-home`
  canonicalizes out of `wisphive_dir`, `starts_with` goes false, and the guard silently stops firing
  while the strict validators still report green.

Reject t3code's `authOverlay` share-heavy-dirs trade-off: shared `sessions`/`sqlite`/`cache` across
gated agents is shared mutable state under ADR-0008's same-UID model, and `KNOWN_SHARED_DIRECTORIES`
(`CodexHomeLayout.ts:19-29`) also shares plugins/skills/cache — the surfaces ADR-0009 exists to
delete. Treat "one shared managed home serves a wave" as a hypothesis to test with concurrent
children, not a settled call. Also correct the ADR's own framing while you're there: the emulation
surface is ~900 impl lines (`audit_codex_effective_hooks` at `:1875-2148` plus helpers from ~`:1374`),
not 5,604 — `mod tests` starts at `:2660`.

**Value: high. Effort: small-medium (as an ADR amendment + provisioning hardening). Security:
strengthens only if paired with the three items above.**

**L12. For fleet visibility, prefer itr#483 over itr#22.** These look like alternatives for "see what
the agent is doing" and the priorities already encode the right answer (#483 high/C4 vs #22 low/C2) —
the point is not to let the wave-spawner reverse it because #22 looks cheaper. `transcript_path`
arrives via the hook, so it covers **every gated Claude Code session** including un-spawned
interactive ones; stdout covers only the subset the daemon spawned. #483's AC-2 (stitch `tool_use` →
decision + `decided_by`) delivers the narrative a stream-json parser would, sourced from a channel
already joined to the authoritative gate outcome.

Three narrowings:

- **#483 does not cover Codex.** `transcript_path` is a Claude Code field and AC-5 anchors path-trust
  to `~/.claude/projects/`; Codex rollouts live under `~/.codex/sessions`. A daemon-spawned Codex
  fleet is visible through *neither* issue today. File the sibling Codex-rollout story before
  building the fleet view on #483's substrate.
- **#483 relocates the trust problem, it doesn't sidestep it.** The transcript JSONL is agent-authored
  at the same UID under a directory itr#425 does *not* guard, so at `level=all` a Write there is
  auto-approved and never reviewed. AC-5's path validation is an arbitrary-file-read defense, not an
  integrity control. Only the stitched decision (`decided_by` from `decision_log`) is authoritative.
  Render it that way: authoritative gate column, non-authoritative agent-authored narrative.
- **"Already redaction-designed" ≠ "redaction-adequate."** Same `redact.rs` prose gap as L8;
  `cat ~/.ssh/id_rsa` in a `tool_result` survives AC-3 untouched. File that as an explicit blocker.

**Value: medium. Effort: large. Security: no gate bypass; a real widening of the persistent store
and web surface, where per CLAUDE.md any XSS is device compromise.**

**L13 (constraint, not work). Any streamed-content model must redact at segment finalization.** This
kills the obvious design — `ServerMessage::ContentDelta` + `redact_value` per chunk — before someone
builds it. `redact_text` (`redact.rs:159-177`) resets `prev_word = ""` on every call (`:161`) and
finds word boundaries with `rest.find(char::is_whitespace)` (`:170`). Its Rule 2 (`:134-139`) redacts
a token *only* because the previous word was `Bearer`/`password:`, so
`redact_text("Authorization: Bearer ") + redact_text("abc.def.ghi")` emits the token in cleartext
while the joined form redacts (test pinned `:234-239`). The hazard is broader than `prev_word`: an
intra-token split defeats Rule 3 too — `redact_text("sk-proj-") + redact_text("abcdef123456")` yields
cleartext, and worse, `"export API_KEY=sk-abc" + "123def456"` emits `API_KEY=***REDACTED***123def456`,
which *looks* scrubbed so a grep-for-cleartext audit passes over it. And `push_log_safe`
self-escapes backslashes (`:100-101`) with the module declaring the pass **non-idempotent**
(`:94-95`), so you cannot redact each chunk *and* the joined segment without breaking the
injectivity guarantee proven at `:302-348`.

The rule: run `redact_text` exactly once over a unit **guaranteed whitespace-terminated**. t3code's
own accumulator is bounded (`MAX_BUFFERED_ASSISTANT_CHARS = 24_000`, flushing mid-stream at
`ProviderRuntimeIngestion.ts:815-817` as a "safety valve") — and a flush at an arbitrary character
offset reintroduces the leak at every flush point, which at 40 concurrent agents is exactly when the
bound binds. So a bounded accumulator must snap its flush boundary back to the last whitespace and
carry the trailing partial word plus one word of lookback forward. Note also that `redact.rs`'s own
doc scopes it to "shell commands, notification bodies, file contents" — assistant *prose* evades
Rule 2 entirely (`"the password is hunter2"` — `prev_word` is `is`), so piping stdout implies a
redactor work item, not just a call site.

**Value: high (as a prohibition). Effort: none to record; medium to honor.**

### Speculative — do not build on these yet

**S1. Daemon-issued resume handles.** `validate_spawn_request:119-127` refuses `resume`/
`continue_session` with a comment that already names the unblock: *"Fail closed until the protocol
carries an ownership-bound session capability."* The daemon already mints the id
(`:2353` `uuid::Uuid::new_v4()`) and passes it (`:2163` `--session-id`) — and discards it;
`ManagedAgent` (`types.rs:511-527`) has no `session_id` field and no table stores it. The shape
change: keep `resume`/`continue_session` rejected **forever** as caller-supplied fields (they are
unfalsifiable), and add a daemon-issued `resume_agent: Option<String>` carrying a prior `agent_id`,
resolved server-side to the `(session_id, project, agent_type)` recorded at spawn.

Why this stays speculative:

- **Claude Code only.** `:2163` is inside the `AgentType::ClaudeCode` arm; the Codex arm builds
  `codex exec` and ignores `session_id` entirely. `codex exec` exposes no flag to set a
  caller-supplied session id — Codex mints its own. Codex wave agents stay one-shot regardless.
- **It is authorization-by-lookup, not a bearer capability.** `agent_id` is published to every
  connected client, so it is enumerable. The property that makes it safe is that the daemon derives
  the target from its own record, not the caller's claim.
- **Ownership of the id is not integrity of the transcript.** `--resume` replays a JSONL the daemon
  never wrote and never screens. `SYSTEM_PROMPT_DENY_PATTERNS` applies only to caller-supplied
  request fields, so a gated-but-productive agent can launder override-class text into its own
  transcript across approved turns and resume replays it unscreened.
- **Resume is *less* reviewable than the cold spawn it replaces.** A human approving
  `resume_agent: agent-abc123` approves content they cannot see. Prior prompt ≠ accumulated
  transcript.

Effort is medium-**high**, not medium: it needs a new table, restart semantics for handles minted by
a dead daemon, transcript screening or hashing, and a review-rendering story.

**S2. ACP.** Zero ACP code, ADR, plan, or itr issue exists. If it ever lands, three rules must be
written down *first* — see Rejected.

## SECURITY: does Wisphive-as-provider-client duplicate or bypass the hook gate?

**It duplicates. It cannot bypass — as long as the hook stays installed and the client never becomes
the gate.** This is the load-bearing question and the answer has a specific mechanism.

Wisphive's own production spawn config is the proof. `build_agent_command` passes
`--dangerously-skip-permissions` (`:2162`) and the code's comment at `:2362-2363` states the
consequence plainly: *"Claude receives `--dangerously-skip-permissions`, so its Wisphive PreToolUse
hook is the only remaining control-plane gate."* The hook still fires. The registry holds `mode_path`
as, verbatim, *"the local kill switch that makes the managed agent's bypassed native permission
prompts safe to use"* (`:2262-2264`). The hook is invoked by the provider from a settings source and
is transport-agnostic **provided the provider loads the settings source carrying it**. So a
transport-owning Wisphive that also keeps `hooks install` intact gets a redundant second channel, not
a replacement one.

The bypass vector is therefore **settingSources, not transport**. `--setting-sources project`
(`:2161`) exists with the comment *"User/local settings can disable or replace its hooks"*
(`:2159-2160`). Widening it to t3code's `["user","project","local"]` (`ClaudeAdapter.ts:3448`) would
void the one-file audit at `:2366-2407` and hand a gated agent — which can write
`~/.claude/settings.json` at the same UID — a path to disabling its own gate. That is the actual
hole, and it has nothing to do with owning a socket.

Three specific things would convert duplication into bypass:

1. **Making the client's approval callback the gate.** Adopting `canUseTool` or app-server
   `requestApproval` *instead of* the hook relocates enforcement from an external binary Wisphive
   proves present pre-spawn into an in-process library callback whose universal coverage is a
   property of the vendor. Note the honest caveat: this is not about the presence of a bypass flag —
   the SDK retains `allowDangerouslySkipPermissions` (`:3457-3459`) and Wisphive already passes
   `--dangerously-skip-permissions` unconditionally. The distinction is **auditability**: an
   enumerated, verified claim ("we listed every hook that can run, with a verify→spawn→verify
   sandwich and a `mode` recheck") versus an unverifiable trust assumption ("the vendor routes every
   approval to us"). Unknown methods are precisely what a vendor upgrade adds — which is why
   t3code's `methodNotFound` hard-deny (`CodexSessionRuntime.ts:1116-1118`) is the one principle
   worth stealing verbatim.
2. **Accepting a request-supplied `CODEX_HOME` or a free-text flag passthrough.** t3code's
   `extraArgs` (`:3409`/`:3467`) would let a settings string inject `--dangerously-skip-permissions`
   past every validator. A request-chosen home is subtler: `audit_codex_effective_hooks` takes
   `codex_home` as a parameter and the scan follows the home, so the requester cannot select *past*
   the kill-switch check — but a field addition alone creates exactly the audit-one-home/spawn-another
   divergence the `:2478` pin exists to prevent. If per-agent homes are wanted, they must be
   operator-configured named entries in `config.json` selected by name from an allowlist, threaded
   through audit *and* spawn atomically. That is a design change, not a field addition.
3. **Treating a "lifecycle-only" transport as security-neutral.** It isn't. `thread/start`,
   `thread/resume`, and `thread/fork` are session-**config** methods wearing lifecycle names: each
   carries `config?: Record<String, Unknown>` — the JSON-RPC twin of the `-c key=value` channel
   `hook_relevant_codex_override` (`:1708-1717`) refuses — plus `approvalPolicy` (incl. `"never"`),
   `sandbox` (incl. `"danger-full-access"`), and `approvalsReviewer` (incl. `"auto_review"`, an LLM
   subagent that approves autonomously). An app-server child's argv is just `["app-server"]`, so
   `audit_codex_session_argv` passes **vacuously** while the entire steering surface moves onto the
   pipe. A lifecycle transport therefore *carries* the itr#511 bypass rather than avoiding it — it
   relocates config steering from an audited channel to an unaudited one.

If a dual-plane design is ever pursued, the invariant is **not** "deny transport-borne approvals and
let the hook adjudicate" — that is incoherent, since a transport decline suppresses the command
before execution and the hook never fires, blanket-denying every call instead of deferring. The
correct invariant is that the approval plane is **never engaged**: configure Codex so approvals are
not generated, and treat any inbound `item/*/requestApproval` as an unexpected-configuration signal —
log a security event, tear the thread down, alert — because its arrival *proves* the hook gate was
not consulted for that call.

One more, for whoever writes the ACP ADR: `session/request_permission` is **advisory** — an agent
that doesn't call it executes anyway. The CLIENT-method boundary (`fs/write_text_file`,
`terminal/create`) is a *better place* to gate, because within one cooperating binary those are
mandatory-to-send to obtain the effect while `request_permission` is optional-to-send. But it is
enforcing **only if the agent process is confined**. An unsandboxed ACP agent runs at the operator's
UID and can call `open(2)`/`fork`/`exec` directly; `fs/write_text_file` exists so the agent can see
unsaved editor buffers, not because the client holds a filesystem monopoly. t3code's `CursorAdapter`
has zero sandbox references. Per ADR-0008 an unsandboxed same-UID ACP agent can only be
tamper-*evidenced*, so **ACP without a sandbox is strictly weaker than the hook**, which the trusted
host invokes and the model cannot skip. Write that inversion down explicitly — "ACP lets us execute,
so ACP is stronger" is the second naive reading a future implementer arrives at after discarding the
first.

Finally, a real gap the recon surfaced while checking this: `detect_agent_type_from_env`
(`main.rs:267-284`) matches `WISPHIVE_AGENT_TYPE` with a wildcard `_ => {}` arm and falls through to
an unconditional `AgentType::ClaudeCode` default (pinned by `defaults_to_claude_without_codex_signal`,
`:3209-3216`). Since response formatting branches on agent type (`:731`, `:770`), an unknown provider
gets Claude-shaped JSON it cannot parse — it may ignore the deny and proceed ungated. Do **not** cite
the closed `AgentType` enum as the fail-closed mechanism for unknown providers; it never participates
(the hook doesn't deserialize it from stdin, and both deser sites carry
`#[serde(default = "default_agent_type")]`). File: make `detect_agent_type_from_env` return
`Option<AgentType>` and route `None` through `response_for_failure`.

## The wave-spawner

**t3code has nothing to teach about waves.** Recon found zero hits for `maxConcurrent`/`maxSessions`/
`concurrencyLimit` across `apps/server/src`; sessions live in an unbounded `Map`
(`CodexAdapter.ts:1367`); every provider-path queue and pubsub is unbounded
(`CodexSessionRuntime.ts:706`, `Layers/ProviderService.ts:215`); the only bounded concurrency in the
whole server is `{concurrency: 4}` on project-event enrichment (`ws.ts:615`), unrelated to sessions.
Its only resource accounting is an idle reaper, which reclaims dead sessions but cannot stop you
opening a 51st. t3code never has N agents contending for one human, so it has no admission model to
copy. That is itself the finding: **the uncapped design is what you get by default, not a design
anyone chose.**

What this recon changes about the plan:

**Four things are blocking, and one is a live bug.**

1. **L1 (fail-open at capacity) must be fixed before the first wave.** A wave is the only workload
   that plausibly reaches 256 simultaneously-blocked hooks, and the failure is not a refusal — it is
   silent auto-approval of every call past the cap, with no `decision_log` row. The queue never even
   builds past 256, because the pressure relieves itself by letting everything through. Reserve
   operator permits *and* fix the pre-Welcome classification.
2. **L4 (running-agent cap) must land before batch-approve exists.** It is the only place blast
   radius is bounded, and `MAX_PENDING_SPAWNS=8` gives a false sense that it already is.
3. **L6 (process group) must land before a "stop the wave" button ships.** Blast radius scales
   linearly: 30 direct PIDs reached, N tool-subprocess grandchildren orphaned across N worktrees, and
   the operator told the wave stopped.
4. **L7 (ADR-0001 amendment) must land before unattended waves.** One crash × 30 in-flight agents is
   30 unsupervised `--dangerously-skip-permissions` children for the outage window, with no
   enumerable blast radius.

**Two things reframe the architecture.**

- **Build isolation at spawn time (L10), not detection at decision time.** The conflict gate cannot
  be the wave's safety mechanism at the auto-approve tier waves run at. Pre-assign worktrees; keep
  the gate for shared absolute paths, Bash mutations, and observability.
- **Batch approval is unnecessary and worse.** `approve_all` already exists (`wire.rs:100-103`,
  itr#88; TUI `A`/`D` via `input.rs:285` with explicit confirmation for the unfiltered case). It
  clears a wave in one keystroke while each spawn keeps its own decision id, oneshot, and
  `decision_log` row. A "one decision covering N spawns" primitive would collapse N audit rows into
  one, destroying per-spawn `decided_by` attribution against itr#397 — and would cap N nowhere
  (~50,000 small requests fit in one 8 MiB line), turning one approve click into a fork bomb the
  operator authorized. It would also collide with `run_after_spawn_approval`'s `updated_input`
  rewrite path (`server.rs:2285-2312`), where approve-with-modification is a designed affordance.

**One thing is cheaper than it looks.** Wave preflight: resolve the binary once and classify
not-installed/installed before burning 50 full fail-closed audits to emit 50 copies of *"failed to
spawn claude — is it installed and on PATH?"* (`:2522-2527`). The codebase already matches
`std::io::ErrorKind::NotFound` in several other places — just not at the spawn site. Two constraints:
the preflight is an **additional** early refusal, never a replacement for the per-agent audit
(caching the hook-inventory audit across a wave reintroduces the itr#511 TOCTOU window); and it must
not execute `claude --version` — today `spawn_agent` is the only place the daemon executes a provider
binary, and every such execution is kill-switch-gated and logged with
`security_event="managed_agent_spawn"`. Use a non-executing PATH resolution, placed *after*
`require_managed_spawn_mode`.

**One thing the reaper story gets wrong.** If a wave-spawner adds an idle reaper (hung agents
otherwise pin the L4 cap forever), it must not key on the clock that looks ready to wire up.
`agent_registry.reap_inactive` fires on the same 5s tick (`server.rs:386-389`) with a 300s default
(`config.rs:191`), but `last_seen` is never refreshed while a hook blocks — and `hook_timeout_secs`
defaults to 3600. An agent legitimately waiting on a pending human decision goes "inactive" 12×
before the human's own approval window closes. In a wave, **human approval latency IS the dominant
idle time** — a naive reaper would kill the wave from the front while the operator works the back of
the queue. t3code's guard is the right shape (`ProviderSessionReaper.ts:64-71` skips any thread with
`activeTurnId != null`), but it must be **bounded**, not copied: t3code has no adversarial agent
model, whereas a hung-but-chatty Wisphive agent could evade the reaper forever by parking one
`DecisionRequest`. Skip only while the pending row is younger than `hook_timeout_secs`, and count
park-and-timeout cycles. Do adopt t3code's sweep-loop hardening verbatim
(`ProviderSessionReaper.ts:106-122`, catch + catchDefect around `Effect.repeat`) — a reaper whose
loop dies silently is the one genuinely fail-open shape in the t3code code, and it ports directly to
any tokio interval task.

## Rejected ideas

Recorded so they are not mistaken for gaps later.

| Idea | Why rejected |
|---|---|
| **`codex app-server` as the gate** | Makes enforcement depend on daemon liveness, replacing ADR-0001's tiering with a vendor-decided fallback; substitutes an unverifiable trust assumption for the itr#511 enumerated claim. Also carries the bypass rather than avoiding it (see SECURITY §3). Adopt `methodNotFound` only. |
| **Claude Agent SDK / in-process `canUseTool`** | Relocates enforcement into an unauditable third-party library that *retains* its own bypass. Recon confirms `canUseTool` provides only `updatedInput` + permission suggestions — which the hook already does. It buys the control channel (`setModel`/`interrupt`/`getContextUsage`), none of which is load-bearing for a batch runner. |
| **`extraArgs` free-text flag passthrough** (`ClaudeAdapter.ts:3409`/`:3467`) | The exact anti-pattern Wisphive's closed argv constructor exists to prevent — a settings string would inject `--dangerously-skip-permissions` past every validator. |
| **ACP `session/request_permission` as the gate** | Advisory: an agent that doesn't call it executes anyway. Also imports t3code's `runtimeMode === "full-access"` in-process pre-queue return (`CursorAdapter.ts:674-684`), contradicting itr#425. And its human path hardcodes option-id literals (`AcpAdapterSupport.ts:46-56`) while only its *auto* path discovers by `kind` (`:297-311`) — a vendor rename would yield a reply of unverifiable effect logged as `decided_by: human`/`Deny`, the audit lie itr#299 refused to write. Discover from `request.options[].kind` on **both** paths; note `optionId` is vendor free-text while `PermissionOptionKind` is protocol-normative, and there is **no** id-independent deny fallback (`cancelled` is reserved for `session/cancel`) — so if no `reject_*` kind is offered, that is itself a fail-mode condition. |
| **Unauthenticated loopback provider server** (`opencodeRuntime.ts:342`) | Loopback TCP is reachable by every local UID and supports neither of the daemon's two other-UID defenses — 0600 perms (`server.rs:3551-3561`) *and* the per-accept `peer_cred` check (`:445-460`, itr#81, whose comment reads "the check is the second layer"). ADR-0008 scopes its residual to same-UID and is silent on other UIDs. Prefer a 0600 Unix socket or a passed listener fd; where a provider only speaks TCP, invert t3code's `server.external &&` guard and make a per-session password **mandatory** — which is what `wisphive_web` already does for its own TCP surface. Reject `findAvailablePort(0)`'s reserve-then-release TOCTOU wholesale. |
| **Request-supplied `CODEX_HOME`** | See SECURITY §2. Operator-configured named entries selected from an allowlist, threaded through audit and spawn atomically — never a `SpawnAgentRequest` field. |
| **`authOverlay` shared heavy dirs** (`CodexHomeLayout.ts:19-29`) | Shared mutable `sessions`/`sqlite`/`cache` across gated agents under a same-UID tamper-evidence model; also shares plugins/skills/cache, the surfaces ADR-0009 exists to delete. |
| **Symlinking `auth.json` → the real home** | Codex writes credentials via `NamedTempFile::persist()` → `rename(2)`, which **replaces** the symlink with a regular file. Write-through never happens; it degrades silently to copy semantics minus the re-seed. Also destroys ADR-0009:116's containment invariant ("real home stays canonical"), races OAuth refresh-token rotation across N children, and takes a new hard dependency on Codex's undocumented credential write strategy (0.144.x has a pluggable keyring store — `auth.json` may not even be the location). |
| **Cursor-replay reconnect for the decision queue** | A pending decision's authority lives in `pending_senders` (`queue.rs:16`), an in-memory oneshot map — the snapshot is a *liveness projection*, so its semantics are "replace," not "append after cursor." Cursor replay would need a durable sequenced decision log, colliding with `persist_pending` (`decisions.rs:114-122`), which redacts `tool_input` before disk precisely because the live queue keeps the full input. You'd either persist cleartext (the itr#300 leak reasoning) or replay `***REDACTED***` rows and make the operator approve blind. The tell is the sequence: reject any PR adding a cursor to the decision-queue wire type. `TermReplay { from_seq }` (`wire.rs:255-258`) is the one legitimate cursor — PTY bytes are an append-only log with no liveness semantics. |
| **Silent resume inheritance / silent resume→fresh fallback** | t3code inherits persisted `resumeCursor` *and* `cwd` when the caller supplies none (`ProviderService.ts:562-572`), recording the divergence only as a telemetry span attribute — and `ProviderSessionStartInput` has no `forceNew` flag, so a caller literally cannot express "fresh session on this bound thread." Separately, `CodexSessionRuntime.ts:463-478` downgrades a failed resume to a fresh start under the same ThreadId. Both make the approved artifact differ from the launched one. Subordinate to the existing ownership refusal (`:119-127`), which is stricter: explicitness satisfies reviewed==launched while failing *ownership*. |
| **Open `AgentType` / parse-must-always-succeed** (`providerInstance.ts:16-28`) | Right for t3code (unknown driver = degraded UX), wrong here (unknown agent_type = unknown gate semantics). Keep the closed enum and the bail at `:61`. Note the enforcement is *already* structural via exhaustive `match` — do **not** "improve" it into a runtime spawnable-map lookup, which trades a compile-time guarantee for a runtime miss. |
| **Opaque `Schema.Unknown` resume cursor** (`ProviderSessionRuntime.ts:48-49`) | Not because it escapes redaction (`redact_value` is schema-independent), but because a closed typed enum makes redaction and validation provable per field, forces a new driver's fields to be reviewed to exist, and matches the closed-allowlist posture at `server.rs:1952-1988`. itr#300's real precedent is data minimization: don't persist what nothing reads. |
| **Self-stamped idleness + NaN gate** (`ProviderSessionDirectory.ts:117`, `ProviderSessionReaper.ts:46-54`) | The flaw isn't self-stamping — Wisphive's `touch()` fires on real agent contact and is the good version. It's that t3code's `upsert` stamps on *any* write, so a pure metadata merge refreshes the idleness clock. The NaN gate is a `Date.parse` artifact a Rust `DateTime<Utc>` cannot reproduce. Do not adopt "reap on unknown" as a security posture: reaping evicts a HashMap row and kills nothing, so an immortal row is a stale-UI wart, not an unsupervised agent — and a blind reap-on-unknown would mass-kill 40 live wave agents on one clock glitch. |
| **Batch spawn approval** | See wave-spawner section. `approve_all` already exists. |

**Killed for an interesting reason.** A learning claimed the 256-connection cap was a hard
fail-**closed** cliff — "availability-shaped with a security tail." The verifier refuted it and, in
doing so, proved empirically against release binaries that the path fails **open**. That inversion is
now L1, the highest-value item in this recon. Worth recording as a method note: the claim was wrong
in the safe-sounding direction, and only an adversarial check found it.

## Non-goals

- Owning any provider session transport as the **gate**. The hook gates agents Wisphive did not
  spawn; a transport-owning gate would not. Own the observation model, not the session.
- Relaxing `--setting-sources project`, `worktree.rs`'s read-only allowlist, or the
  `resume`/`continue_session` refusal.
- Building auto-restart. A silently respawned agent is an unreviewed spawn.
- Treating this document as a decision. Every Tier-3 item needs its own ADR or plan; the ADR-0009
  amendment (L11) and the ADR-0001 managed-spawn carve-out (L7) are the two that constrain future
  work hardest and should be written first.

---

# CORRECTIONS (2026-07-16, ADR-mapping pass)

Four agents re-checked this recon's claims against each `Proposed` ADR and the real source. They
found six errors. **This section is appended, not merged into the text above** — the original claims
stay visible so the reasoning trail survives. Where they conflict, **this section wins**.

## C1 — The ADR-0009 corroboration claim is wrong: a provider conflation. **This is the big one.**

The delta table (:133) and the `coreDifference` claim that t3code "blanks the child's config and
passes policy in-band, which is exactly ADR-0009's thesis — independent confirmation" **does not
survive contact with the source.**

`OPENCODE_CONFIG_CONTENT="{}"` occurs at exactly **one** site — `opencodeRuntime.ts:352` — in the
**OpenCode** driver. It never touches the Codex driver. The recon compared *t3code's OpenCode driver*
to *Wisphive's Codex audit*: different providers, and OpenCode is under no gate-integrity requirement.

**t3code's actual Codex driver does the opposite of ADR-0009.** `CodexHomeLayout.ts:354-359`
symlinks every shared-home entry into the shadow home except `auth.json`/`models_cache.json` (`:31`)
and `log`/`memories`/`tmp` (`:32`) — the shadow home's `config.toml` **is a symlink to the operator's
real one**, asserted at `CodexHomeLayout.test.ts:126`. Its motive is **per-instance auth
(multi-account)**, not gate integrity — the different-threat-model trap. Not convergent design.

"CODEX_HOME spread LAST… clobbering the operator's value" is also misleading: the spread is
**conditional** (`CodexSessionRuntime.ts:716-719`). With no configured `homePath` the operator's
`CODEX_HOME` passes through untouched; when it does win it sets the operator's own configured path —
precedence resolution, not daemon enforcement.

t3code performs no hook audit and no Codex config emulation, so it is **silent** on ADR-0009's
problem. Absence of emulation in a system with no gate is not a vote for isolation.

**But chasing this produced the session's most valuable finding** (itr#569): t3code's generated
protocol bindings surfaced **`hooks/list`**, verified against the operator's installed codex-cli
0.144.5 via `codex app-server generate-json-schema`. It returns the effective per-project hook
inventory — `enabled`, `source`, `sourcePath`, `isManaged`, `pluginId`, `trustStatus`, `eventName`,
`matcher`, `command`, `timeoutSec`, `currentHash` — i.e. what `audit_codex_effective_hooks`
re-derives in ~800 lines. Its `HookSource` enum enumerates the `mdm`/`cloudManagedConfig` layers
ADR-0009:155-157 calls **"non-enumerable"**, and those live *outside* `CODEX_HOME`, so isolation
cannot remove them — **(a) and (b) are complementary, not competing.** ADR-0009 rejected option (a)
on "an authoritative non-interactive surface does not exist" (:80-82) and plans to *file a feature
request* for it (:176-177). **The ask already ships.** The ADR's facts are scoped ":42 from `--help`
surfaces only" — it never opened the JSON-RPC protocol it lists at :47.

## C2 — "The one genuinely fail-open shape in the t3code code" is wrong (:704). :725 is the right half.

This document contradicts itself; :725 wins. A silently-dead reaper is **not** fail-open in Wisphive:
`reap_inactive` only does `HashMap::remove` (`registry.rs:70-85`), kills no process, and nothing
gates on the registry. A dead tick is a **stale agents panel**. And conditionally it *inverts* — if a
future L4 cap counts registry rows, a dead reaper fails **closed** (immortal rows fill the cap).
Adopt the loop hardening on **robustness** grounds, not fail-open grounds.

## C3 — "Adopt t3code's sweep-loop hardening verbatim" is wrong for the supervisor (:704).

The hardening is real and correctly ordered (`ProviderSessionReaper.ts:109-120` puts `catch` +
`catchDefect` *inside* the `repeat`). But its **handler body is `logWarning` — swallow and continue**,
which is precisely what ADR-0007:20-23 forbids. Adopt the **structure** (the loop task cannot die
silently); the handler body must route to **Abort**, not a log line. Verbatim adoption imports a
janitor's posture into a spender.

## C4 — The "reap on unknown" rejection (:726) is a strawman.

t3code has no reap-on-unknown to reject. `ProviderSessionReaper.ts:47-54` does `continue` on a NaN
`lastSeenAt` — it **skips**. The recon rejects a posture t3code never held, then lands on t3code's
actual behavior. The sub-claims stand and are worth keeping: the `Date.parse` artifact genuinely
cannot occur in Rust (Wisphive stores a typed `DateTime<Utc>`, `registry.rs:43` — no parse step, no
NaN state, correct behavior for free), and the self-stamping critique is right (`upsert` stamps on
*any* write, `ProviderSessionDirectory.ts:117`; Wisphive's `touch()` fires on real agent contact and
is the better version).

## C5 — The reaper-bounding requirement is real but not presently load-bearing (:704).

`ProviderSessionReaper.ts:64-71` skipping unbounded on `activeTurnId != null` is confirmed. But
"a hung-but-chatty agent could evade the reaper forever" must answer: evade to **what end**? Today
reaping buys the agent nothing — it kills nothing and gates nothing. This is a design constraint on a
**future** L4 cap, not a present need. Keep the advice; drop the urgency.

## C6 — The wave reaper warning points the wrong way (:704). **The real direction is fail-open.**

This document warns a naive reaper "would kill the wave from the front while the operator works the
back of the queue." **Inverted.** Verified (itr#568): nothing refreshes `last_seen` while a hook
blocks — both touch sites fire *after* resolution (`server.rs:930-934`, `:950-955`) and
`register_agent_once` is `O_EXCL`-guarded to once per session (`main.rs:1684-1750`). With
`agent_timeout_secs`=300 and `hook_timeout_secs`=3600, an agent waiting on a human goes "inactive"
**12× over** before the approval window closes, and at ~t=305 the daemon broadcasts a **false
`AgentDisconnected`** while the hook is still blocked and the decision still queued.

So if a future L4 cap counts registry rows, agents blocked on the human fall **out** of the count and
the cap admits **more** agents — **fail-open on admission**, under-counting exactly the population
blocked on the human. *The cap stops binding precisely at peak human latency.* Any L4 cap must count
blocked-on-human agents, or key on the queue (as `MAX_PENDING_SPAWNS` already does,
`server.rs:2011`).

## What survived

The security analysis (settingSources-not-transport as the bypass vector; a transport decline
suppressing the command before the hook fires; ACP `request_permission` being advisory and strictly
weaker than the hook absent a sandbox), the `methodNotFound` hard-deny recommendation, the
process-group finding (itr#561), the `detect_agent_type_from_env` finding (itr#562), the
capacity fail-open (itr#560, the highest-value item and itself the product of a refutation), the
`authOverlay` and `auth.json`-symlink rejections, and "t3code has nothing to teach about waves."

**Method note.** Every error above is in the *corroborating* direction — claims that made an
appealing conclusion look better-supported than it was. C1 flattered an ADR we wanted to accept. The
adversarial verify caught inversions inside dimensions but could not catch a conflation *across*
dimensions, because no single verifier held both drivers at once. Cross-dimension claims need a
verifier that owns the comparison, not two that each own a side.
