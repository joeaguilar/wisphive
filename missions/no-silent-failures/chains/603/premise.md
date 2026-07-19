# Chain 603 premise — managed-spawn gate integrity: no silent fail-open

**IMMUTABLE.** Rescope = supersede via a new epic + `itr relate --type supersedes`.

## Ordering-claim

Classification before behavior: the links that define how failures are *classified and
audited* (#560 hook-side live-rejection taxonomy, #562 explicit-unknown provider routing,
#559 headless defer predicate) must land before the lifecycle/behavior links (#568
last_seen touch, #561 process-group stop), because the later fixes emit through the failure
taxonomy the earlier ones establish, and all five walk the same three files
(hook `main.rs`, daemon `server.rs`/`process_registry.rs`) — serial order is also
collision avoidance. #568→#561 is territory-order, not semantic dependency (declared, not
claimed). Falsifier: a later link is shown to need none of the earlier taxonomy AND its
serialization costs a council-visible delay.

## Value-claim

After the chain lands, no failure on the managed-spawn path resolves silently: capacity
rejection fails closed and audited (not silent approve), an explicitly-unknown provider
routes to `response_for_failure` (not another provider's JSON), headless always-defer
resolves deterministically fail-closed with a reason (not a silent block), a hook blocked
on a human decision is not falsely reaped at ~300s, and stopping an agent verifiably stops
its whole process tree. Falsifier: red-team probe after landing still finds a silent
resolution path reachable by a wave-spawner.

## Links (serial; blocked-by encoded in itr)

1. **L1 = itr#560** (critical) — hook handshake treats pre-Welcome `Error` as a LIVE
   rejection → fail-closed + audited; daemon's typed/structured rejection is secondary
   hardening. NOT the `MAX_PENDING_SPAWNS` path.
2. **L2 = itr#562** — explicitly-set unrecognized agent-type env → unknown →
   `response_for_failure`; unset env keeps tail-default + Codex heuristic (4 pinned tests
   stay green).
3. **L3 = itr#559** — prompt-existence predicate (posture-aware, #564-compatible);
   headless managed spawn → deterministic fail-closed with reason + audit. ADR-0002 amended.
4. **L4 = itr#568** — touch `last_seen` while a decision is pending; no false
   `AgentDisconnected` at ~300s; record the L4-cap fail-open constraint.
5. **L5 = itr#561** — process-group spawn; SIGTERM → grace → SIGKILL to the group; fix
   lying comments; grandchild-gone test.

Dormancy: all links are enforcement-semantics changes exercised only via tests/isolated
temp-HOME probes — nothing here touches the installed binary (`./install.sh` is
operator-only; CLAUDE.md standing rule), so "dormant until terminal" is inherent.

## Assumptions

| id | text | tag |
|---|---|---|
| B-A1 | daemon at capacity accepts-then-sends `Error{CONNECTION_LIMIT_ERROR}` (`server.rs:557-564`, test `:5010`) — a live rejection is distinguishable in-band | verified-at-intake |
| B-A2 | hook collapses pre-Welcome `Error` into `DaemonUnreachable` → approve (`main.rs:1513-1520`, `:673-674`) | verified-at-intake |
| B-A3 | #562 as filed (wildcard arm) — **refuted at intake**; re-scoped to explicit-unknown path; 4 pinned tests (`:3186,:3194,:3204,:3210`) must stay green | verified-at-intake (correction noted on issue) |
| B-A4 | #559 silent-approve framing — **refuted** by `docs/research/headless-ask-probe` (8193b04); defect is silent-block availability; fail-closed posture holds | verified-at-intake |
| B-A5 | `last_seen` only touched post-resolution (`server.rs:930-934`, `:951-954`); reaper default 300s | verified-at-intake |
| B-A6 | `stop_agent` = `kill()` (SIGKILL) on direct PID only; comments claim SIGTERM (`process_registry.rs:2585-2594`); no pgid anywhere in file | verified-at-intake |
| B-A7 | ADR-0001/0002 amendments land in-link with the behavior change (intake Q4 default) | assumed-by-default |
| B-A8 | changing `DaemonUnreachable` classification for post-connect errors does NOT regress the sacred socket-down fail-open (ADR-0001: refused/absent socket still approves) | assumed-by-default — reviewer must verdict this every chain-B link |

## Kill criteria

- Link rework ≥ 2 → council.
- Reviewer `contradicts` on any assumption (esp. B-A8) → council.
- Every 5 landed links → council (fires at L5 if reached).
- Any link found to require weakening a fail-closed default to pass its own tests →
  immediate council (min verdict rescope).

## Territory

`crates/wisphive_hook/src/main.rs`, `crates/wisphive_daemon/src/server.rs` (accept-loop
`:476-564`, reaper `:387-414`, touch sites `:930-955` regions),
`crates/wisphive_daemon/src/process_registry.rs`, `crates/wisphive_daemon/src/registry.rs`,
`crates/wisphive_daemon/src/config.rs`, `crates/wisphive_protocol/src/wire.rs` (typed
variant — shared surface with chain 602, X-A1), `docs/decisions/0001-*.md`,
`docs/decisions/0002-*.md`, `docs/plan-loop-supervisor.md` (#568 constraint note).

## Budget

Mission budget unset; no per-chain allocation. Rework/contradicts criteria still fire.
