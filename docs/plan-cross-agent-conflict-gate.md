# Plan: Cross-Agent Conflict Gate

_Last reviewed: 2026-07-03 (semantics pass, itr#424 — see Semantics section)_

## Problem

Wisphive monitors multiple agents across multiple projects simultaneously. Each agent independently edits files — and Wisphive sees every Write and Edit tool call. But today the daemon treats each call in isolation. If Agent A writes `src/auth.rs` and Agent B tries to write `src/auth.rs` two seconds later, Wisphive approves both without warning. The human in the TUI has no visibility into the conflict.

Coven and Seance solve this proactively — they plan conflict-free waves *before* spawning agents. Wisphive can't do that because it doesn't control when agents run. Instead, Wisphive needs **reactive conflict detection**: detect conflicts as tool calls arrive and surface them to the human before approving.

## Design

### Core Concept: FileConflictMap

A new in-memory data structure in the daemon that tracks which agent "owns" which files based on approved Write/Edit calls.

```rust
// wisphive_daemon/src/conflict.rs

pub struct FileConflictMap {
    /// file path (canonicalized) → claim
    claims: HashMap<PathBuf, FileClaim>,
}

pub struct FileClaim {
    pub agent_id: String,
    pub project: PathBuf,
    pub claimed_at: DateTime<Utc>,
    pub tool_use_id: Option<String>,
    /// How many times this agent has written to this file in the current session
    pub write_count: u32,
}

pub struct ConflictInfo {
    pub file: PathBuf,
    pub holding_agent: String,
    pub holding_project: PathBuf,
    pub claimed_at: DateTime<Utc>,
    pub requesting_agent: String,
}
```

**Lifecycle of a claim:**
1. Agent A's `Write { file_path: "src/auth.rs" }` is approved → `FileConflictMap::claim("src/auth.rs", "agent-A")`
2. Agent B's `Write { file_path: "src/auth.rs" }` arrives → `FileConflictMap::check("src/auth.rs", "agent-B")` → returns `ConflictInfo`
3. Conflict is attached to the `DecisionRequest` and highlighted in TUI
4. Human decides: approve anyway, deny, or hold (new decision variant)
5. Claims auto-expire when: agent disconnects, configurable TTL elapses, or human manually releases

### What Changes

#### 1. New file: `wisphive_daemon/src/conflict.rs`

```rust
impl FileConflictMap {
    pub fn new() -> Self;

    /// Record a file claim after a Write/Edit is approved.
    pub fn claim(&mut self, file: &Path, agent_id: &str, project: &Path) -> Option<FileClaim>;

    /// Check if a file is claimed by another agent. Returns conflict info if so.
    pub fn check(&self, file: &Path, agent_id: &str) -> Option<ConflictInfo>;

    /// Release all claims for an agent (called on disconnect/reap).
    pub fn release_agent(&mut self, agent_id: &str) -> Vec<PathBuf>;

    /// Release claims older than TTL.
    pub fn reap_expired(&mut self, ttl: Duration) -> Vec<(PathBuf, FileClaim)>;

    /// Release a specific file claim (manual release from TUI).
    pub fn release_file(&mut self, file: &Path) -> Option<FileClaim>;

    /// Snapshot of all active claims (for TUI display).
    pub fn snapshot(&self) -> Vec<(PathBuf, FileClaim)>;
}
```

**Path normalization:** Files arrive as relative or absolute paths from different working directories. Before checking/claiming, canonicalize: resolve `..`, strip trailing slashes, and prepend the project's cwd if relative. Two agents in the same project writing `src/auth.rs` and `./src/auth.rs` must resolve to the same key.

```rust
fn normalize(file: &Path, project: &Path) -> PathBuf {
    let full = if file.is_absolute() {
        file.to_path_buf()
    } else {
        project.join(file)
    };
    // Best-effort canonicalize; fall back to cleaned path if file doesn't exist yet
    full.canonicalize().unwrap_or_else(|_| clean_path(&full))
}
```

#### 2. Wire into `Server` (server.rs)

Add `FileConflictMap` as a new `Arc<Mutex<FileConflictMap>>` field on `Server`, alongside the existing `queue`, `agent_registry`, and `process_registry`.

```rust
pub struct Server {
    // ... existing fields ...
    conflict_map: Arc<Mutex<FileConflictMap>>,
}
```

Pass it to `handle_connection` → `handle_hook` → used at two points:
- **On enqueue** (before adding to queue): check for conflicts, attach to request
- **On resolve** (after approval): record the claim

#### 3. Conflict check in `handle_hook` (server.rs:248)

After constructing the `DecisionRequest` but before calling `q.enqueue(req)`:

```rust
// Only check file-mutating tools
let dominated_tools = ["Write", "Edit", "NotebookEdit"];
if dominated_tools.contains(&req.tool_name.as_str()) {
    if let Some(file_path) = extract_file_path(&req.tool_name, &req.tool_input) {
        let conflict_map = conflict_map.lock().await;
        if let Some(conflict) = conflict_map.check(&file_path, &req.agent_id) {
            // Attach conflict info to the request for TUI display
            req.conflict = Some(conflict);
        }
    }
}
```

**Extracting file paths from tool input:**

```rust
fn extract_file_path(tool_name: &str, tool_input: &Value) -> Option<PathBuf> {
    match tool_name {
        "Write" | "Read" => tool_input.get("file_path")?.as_str().map(PathBuf::from),
        "Edit" => tool_input.get("file_path")?.as_str().map(PathBuf::from),
        "NotebookEdit" => tool_input.get("notebook_path")?.as_str().map(PathBuf::from),
        _ => None,
    }
}
```

#### 4. Claim recording after approval (server.rs:~295)

After the hook receives an Approve decision and before sending the response:

```rust
if rich.decision == Decision::Approve {
    if let Some(file_path) = extract_file_path(&req_tool_name, &req_tool_input) {
        let mut cm = conflict_map.lock().await;
        cm.claim(&file_path, &agent_id, &req_project);
    }
}
```

#### 5. Claim release on agent disconnect/reap (server.rs:~92)

In the existing `reap_interval` tick handler, after `reg.reap_inactive()`:

```rust
for agent_id in &reaped {
    let mut cm = conflict_map.lock().await;
    let released = cm.release_agent(agent_id);
    if !released.is_empty() {
        info!(agent_id, files = released.len(), "released file claims");
    }
}
```

Also add a TTL reap in the same interval:

```rust
{
    let mut cm = conflict_map.lock().await;
    let expired = cm.reap_expired(Duration::from_secs(config.conflict_ttl_secs));
    for (path, claim) in &expired {
        info!(file = %path.display(), agent = %claim.agent_id, "claim expired");
    }
}
```

#### 6. Protocol additions (wisphive_protocol/src/types.rs)

```rust
/// Conflict information attached to a DecisionRequest.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ConflictInfo {
    pub file: PathBuf,
    pub holding_agent: String,
    pub holding_project: PathBuf,
    pub claimed_at: DateTime<Utc>,
}
```

Add to `DecisionRequest`:
```rust
pub struct DecisionRequest {
    // ... existing fields ...
    /// If this tool call conflicts with another agent's file claim.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub conflict: Option<ConflictInfo>,
}
```

New `ServerMessage` variants:
```rust
pub enum ServerMessage {
    // ... existing variants ...
    /// Snapshot of all active file claims (sent to TUI on connect).
    ConflictSnapshot { claims: Vec<(PathBuf, FileClaim)> },
    /// A file claim was recorded (broadcast after approval).
    FileClaimed { file: PathBuf, agent_id: String },
    /// A file claim was released.
    FileReleased { file: PathBuf, agent_id: String },
}
```

New `ClientMessage` variant:
```rust
pub enum ClientMessage {
    // ... existing variants ...
    /// Manually release a file claim (from TUI).
    ReleaseClaim { file: PathBuf },
    /// Query current conflict map.
    QueryConflicts,
}
```

#### 7. Config additions (config.json)

```json
{
    "conflict_detection": true,
    "conflict_ttl_secs": 300,
    "conflict_mode": "warn"
}
```

Three modes:
- `"off"` — no conflict detection (existing behavior)
- `"warn"` — attach conflict info to request, highlight in TUI, but don't block (default)
- `"block"` — auto-deny conflicting writes (aggressive; user can override per-call)

#### 8. TUI changes (wisphive_tui)

**Detail view:** When a `DecisionRequest` has a `conflict` field:
- Show a warning banner: `⚠ CONFLICT: src/auth.rs is claimed by agent-A (2m ago)`
- Add a `[R]elease claim` action alongside Approve/Deny
- Color the tool name in the queue list (yellow for conflict)

**New panel (optional, lower priority):** "Active Claims" panel showing all current file ownership. Navigate with `f` key. Shows: file path, owning agent, claimed duration, write count.

**Queue list:** Conflict items get a `⚠` prefix and sort to the top of the queue (they need human attention more urgently).

#### 9. Web UI changes (wisphive_web)

- Decision detail shows conflict warning with holding agent info
- Claims dashboard (new tab or section in Projects view)
- Release button per claim

### Semantics (normative — itr#424 pass, 2026-07-03)

The mechanics above are sound; this section pins the definitions and the race analysis an
implementer would otherwise have to guess at. Where this section and the sketches above
disagree, this section wins.

#### S1 — Precise conflict definition

A **conflict** exists when a Write/Edit/NotebookEdit request's normalized path equals the
normalized path of (a) an unexpired claim held by a *different claim key*, or (b) another
**pending** request in the queue from a different claim key (see S3, Race A). Equality is
canonical-path equality only — no directory or prefix containment in v1. The **claim key**
is `agent_id`; note that distinct subagents of one orchestrating session carry distinct
ids, so an agent's own fan-out conflicts with itself — that is a feature, not a bug (it is
exactly the parallel-blitz case this gate exists for).

**Known blind spots, accepted and documented, not silently shipped:** Bash file mutations
(`sed -i`, `mv`, `rm`, `>`) are invisible to the gate in v1 (only the three structured
file tools participate). Symlinked paths to not-yet-existing files can evade key equality
(canonicalize falls back to lexical cleaning). On macOS, default-APFS is case-insensitive
— normalization lowercases path keys on macOS so `Foo.rs`/`foo.rs` collide as they do on
disk.

#### S2 — Design bias: over-claim, never under-claim

A false positive costs a yellow banner (warn mode) or one human override (block mode). A
false negative is two agents silently interleaving writes — the exact failure the gate
exists to prevent. Every ambiguous choice below resolves toward claiming: claims are
recorded even if the write may never execute, TTLs err long, and abandoned decisions still
claim briefly rather than risk a miss.

#### S3 — TOCTOU analysis (check-to-execution window)

There are four distinct races between "conflict checked" (enqueue), "claim recorded"
(approval), and "write actually executes" (whenever the agent runs the tool — unbounded):

- **Race A — pending vs pending.** A and B both enqueue writes to `auth.rs` before either
  is approved; neither holds a claim, so neither shows a conflict. **Resolution:** the
  enqueue-time check also scans the pending queue (definition in S1b), and conflict status
  is **recomputed at resolution time** — the TUI approve action re-checks the map so a
  claim recorded after enqueue still surfaces before the human commits.
- **Race B — claim expiry before execution.** The decision timeout is 1 hour; the claim
  TTL default is 300s. An approval that sat in the queue can post a claim that expires
  before the agent even executes the write. **Resolution:** TTL is measured from claim
  recording (approval), default raised to `conflict_ttl_secs: 900`, and a PostToolUse
  observation (see S4) refreshes the claim — execution proves the claim was live.
- **Race C — approval delivered to a dead hook.** itr#363 abandons the decision as a deny
  when the hook socket dies; no write will happen. **Resolution:** claims are recorded
  only on *successfully delivered* approvals; an abandoned decision records nothing. If
  delivery-vs-death races anyway, the stale claim expires by TTL — over-claiming is the
  accepted direction per S2.
- **Race D — approve B while A's approved write hasn't executed yet.** The human sees A's
  claim but has no way to know whether A's write already happened. **Resolution:** claims
  carry a state, `granted → executed`, flipped when the corresponding PostToolUse event
  (already reported by the hook) is ingested. The TUI banner distinguishes "claimed,
  write not yet observed" from "written 2m ago" — that is the information the human
  actually needs to sequence approvals.

#### S4 — Precedence versus the auto-approve layers

The gate can only **narrow**, never widen: a conflict can turn an approval into a warn or
a deny; it never approves anything. Interaction table:

| Path | Sees the map? | Behavior |
|---|---|---|
| Daemon-reviewed request (hook deferred to human) | yes | full warn/block semantics |
| Hook auto-approved (level/tool_rules/legacy) | no — hook has no map access | not gated in-line; the `events.jsonl` ingest records a **retroactive claim** (lag: seconds), so later daemon-reviewed requests still conflict against auto-approved writes |
| Future learned rules (ADR-0005) | no | same as auto-approved; ADR-0005 I3's decision-time constraints are about input safety, not conflicts — cross-agent coordination at `write`+ auto-approve tiers remains explicitly out of scope |
| `conflict_mode: "block"` auto-deny | n/a | applies only to daemon-reviewed requests; attributed `decided_by: conflict_gate:block` in the audit stream, overridable by the human re-approving from the TUI |

The plan's existing note stands: users who want in-line conflict gating for writes must
keep writes below the auto-approve tier. Retroactive claims close most of the practical
gap for mixed fleets.

#### S5 — Deny-message contract (block mode)

A blocked agent must be able to act on the denial without a human explaining it. The
`permissionDecisionReason` is stable and machine-readable, first line exactly:

```
WISPHIVE_CONFLICT file=<canonical path> holder=<agent_id> claimed_at=<ISO8601> state=<granted|executed>
```

followed by a prose line: "Another agent currently owns this file. Do not edit it now —
work on something else and retry after the claim releases (TTL <n>s) or ask the operator
to release it." Agents pattern-matching the first line can reschedule work; humans reading
the queue see the second.

#### S6 — Scope cuts made explicit

- The "hold" decision variant mentioned in the lifecycle sketch is **cut from v1** — a
  hold is just an unresolved pending decision; no new variant needed.
- `write_count` is display metadata only; it must never feed policy (it is trivially
  inflatable by the holding agent).
- Claims are in-memory only (existing non-goal): a daemon restart drops all claims, which
  is over-forgetting rather than over-blocking — acceptable, but the restart log notes how
  many claims were dropped so a mid-blitz restart is diagnosable.

#### Edge-case outcomes (delta to the list below)

| Case | Outcome |
|---|---|
| Two pending writes, same file, neither approved | Both flagged via pending-scan (Race A); approving either re-checks |
| Approve delivered, hook already dead | No claim (itr#363 deny); TTL cleans any race remnant |
| Claim TTL expires between approval and execution | PostToolUse ingest re-establishes `executed` claim (Race B/D) |
| Same file, case-differing paths on macOS | Conflict (lowercased keys) |
| Auto-approved write then daemon-reviewed write, same file | Conflict via retroactive ingest claim (lag seconds) |
| Bash `sed -i` then Write, same file | **Not detected** — documented v1 blind spot (S1) |

### Implementation Order

| Phase | What | Files | Estimated Size |
|-------|------|-------|---------------|
| **1** | `FileConflictMap` struct + unit tests | `conflict.rs` (new) | ~200 LOC + ~200 LOC tests |
| **2** | Protocol types (`ConflictInfo`, new messages) | `types.rs` (edit) | ~40 LOC |
| **3** | Wire into `Server` (check on enqueue, claim on approve, release on reap) | `server.rs` (edit) | ~60 LOC |
| **4** | Config support (`conflict_detection`, `conflict_ttl_secs`, `conflict_mode`) | `config.rs` (edit), hook `main.rs` (no change — hook doesn't check conflicts) | ~20 LOC |
| **5** | TUI conflict display (warning banner in detail, ⚠ in queue list) | TUI detail/queue files (edit) | ~80 LOC |
| **6** | TUI claims panel + release action | TUI new panel | ~150 LOC |
| **7** | Web UI conflict indicators + claims dashboard | React components (edit/new) | ~200 LOC |

**Total estimate:** ~950 LOC (including tests). No new crates. No new dependencies.

### Edge Cases

**Same agent, same file:** Agent A writes `auth.rs` twice. No conflict — the claim is refreshed with a new timestamp and `write_count` incremented. This is normal iterative behavior.

**Cross-project conflicts:** Agent A in `/project-a` writes `/shared/config.rs`. Agent B in `/project-b` writes `/shared/config.rs`. This IS a conflict because the canonical path is the same. The normalization function handles this via `canonicalize()`.

**Reads don't claim:** Only Write, Edit, and NotebookEdit create claims. Read/Grep/Glob don't. An agent reading a file another agent is editing is fine.

**Auto-approved writes:** Writes at the `write` auto-approve level bypass the daemon. The hook approves them directly and logs to `events.jsonl`. The daemon doesn't see them until ingested. **This means auto-approved writes won't have conflict checks.** This is acceptable at the `write` tier — if you've auto-approved writes, you've accepted the risk. Document this clearly. Users who want conflict detection should use `read` tier for auto-approve.

**Agent crash with held claims:** Claims have a TTL (`conflict_ttl_secs`, default 300s). After an agent crashes and is reaped (5s reap interval + `agent_timeout_secs`), `release_agent()` is called. Even without reaping, the TTL expires claims. Belt and suspenders.

**File doesn't exist yet:** Agent A creates a new file. `canonicalize()` fails because the file doesn't exist. Fall back to the cleaned path (resolve `..`, normalize separators). This is still deterministic — two agents creating the same new file will hit the same key.

### Testing Strategy

Unit tests for `FileConflictMap`:
- `claim_and_check_same_agent_no_conflict`
- `claim_and_check_different_agent_returns_conflict`
- `release_agent_clears_all_claims`
- `reap_expired_removes_old_claims`
- `path_normalization_relative_and_absolute`
- `cross_project_same_canonical_path`
- `write_count_increments_on_reclaim`
- `release_file_manual`

Integration test for the full flow:
- Spawn two mock hook connections
- First approves a Write to `foo.rs`
- Second sends a Write to `foo.rs`
- Verify the second DecisionRequest has `conflict` populated

### Non-Goals

- **Automatic merging** — Wisphive doesn't understand file contents. It flags conflicts; humans decide.
- **Pre-planning waves** — That's Coven/Seance's job. Wisphive reacts to tool calls as they arrive.
- **Blocking reads on written files** — Reads are always safe. Only file-mutating tools participate in conflict detection.
- **Distributed conflict state** — The map lives in the daemon's memory. If the daemon restarts, claims are lost. This is fine — claims are short-lived and the TTL handles stale state.
