# Plan: Deterministic Agent Analytics

_Last reviewed: 2026-06-15_

Decision record: ADR-0004. Backlog: itr#390 umbrella; itr#391 analytics
substrate, itr#392 session work journal, itr#393 risk digest, itr#394
operations dashboard, itr#395 conflict/overlap analytics.

## Problem

Wisphive already captures a large amount of agent activity: decision requests,
tool inputs, approvals and denials, post-use tool results, timestamps, session
IDs, project paths, auto-approved events, and terminal-session correlation. That
data is useful only if the operator can turn it into clear facts after an agent
session ends.

The first product goal is not "AI summaries." The first goal is a deterministic
analytics layer that can answer: what happened, when it happened, which files and
commands were involved, what failed, what looked risky, and where multiple
agents overlapped.

LLM-written summaries can come later, but only as a narrative layer over a
typed fact bundle.

## Product Boundary

### In Scope

- Typed, deterministic facts derived from existing Wisphive history.
- Session-level work journals.
- Risk digests over agent actions.
- Agent and project operations dashboards.
- Historical conflict and overlap analytics.
- CLI and web surfaces that link every summary claim back to source history.

### Out of Scope for the First Slice

- Sending raw session logs to an LLM by default.
- Automated policy changes or learned auto-approve rules.
- Live conflict blocking. Historical overlap reports can feed the existing
  cross-agent conflict gate plan, but they do not replace it.
- Inferring actions Wisphive did not observe.

## Design Principle: Facts First

Every user-facing summary starts from a reproducible fact bundle:

```rust
pub struct SessionFact {
    pub source_id: uuid::Uuid,
    pub agent_id: String,
    pub project: PathBuf,
    pub at: DateTime<Utc>,
    pub action: ActionKind,
    pub decision: Decision,
    pub confidence: FactConfidence,
}

pub enum ActionKind {
    CommandRun { command: String, outcome: CommandOutcome },
    FileRead { path: PathBuf },
    FileMutated { path: PathBuf, operation: FileOperation },
    SearchRun { pattern: String },
    PermissionPrompt { tool_name: String },
    UserPrompt { prompt: String },
    StopEvent { message: Option<String> },
    UnknownTool { tool_name: String },
}
```

The exact shape can change during implementation, but the contract should hold:

- facts are typed;
- facts cite the source history row;
- facts never hide missing data;
- facts are safe to aggregate without calling an LLM.

## Existing Data Sources

- `decision_log` in SQLite: primary source for resolved decisions.
- `events.jsonl`: source for auto-approved events before daemon ingestion.
- `tool_result` attached from `PostToolUse`: command output, tool responses, and
  failure clues when available.
- `terminal_session_id`: bridge from tool actions into daemon-managed terminal
  sessions.
- `terminal_events`: replay data, useful later for session replay enrichment.

## Known Data Gaps

- `Ask` decisions are not currently persisted to `decision_log`.
- Some agent/tool paths are not intercepted by Codex hooks.
- Auto-approved writes bypass live daemon review and arrive later through ingest.
- `tool_result` can be absent or attached by fuzzy fallback for older/no-ID
  flows.
- Retention may move older `decision_log` rows into JSONL archives.

Analytics must represent these as lower-confidence or missing facts rather than
guessing.

## Epic 0: Analytics Substrate

Build the shared fact extraction layer used by every downstream feature.

### Deliverables

- `StateDb` query for a full session timeline by `agent_id`, oldest-first, with
  pagination.
- Shared extractor module for common tool inputs and results:
  - `Bash` command, outcome, and error hints.
  - `Read`, `Write`, `Edit`, and `NotebookEdit` paths.
  - `Grep`/search patterns.
  - `PermissionRequest`, `UserPromptSubmit`, and `Stop` event data.
- Typed fact structs in `wisphive_protocol` or a daemon-local module with a
  stable web/CLI projection.
- Tests for missing result data, unknown tool names, auto-approved rows, and
  malformed JSON.

### Acceptance Criteria

- A session can be converted into ordered typed facts without external services.
- Every fact links back to a source history entry.
- Missing or lower-confidence facts are explicit.
- Existing History and Sessions behavior is unchanged.

## Epic 1: Session Work Journal

Generate a deterministic post-session journal for one agent session.

### Deliverables

- CLI: `wisphive history summarize --agent <id>`.
- Web: Summary tab in the existing Sessions detail view.
- Sections for:
  - session header and duration;
  - files touched;
  - commands run;
  - tests/builds detected;
  - failures and denied actions;
  - user prompts and stop events;
  - unresolved/missing data warnings.
- Optional persisted `session_summaries` cache after the first live version.

### Acceptance Criteria

- The summary is useful without any LLM.
- Every section is reproducible from deterministic facts.
- The UI can jump from each summary section back to matching history entries.

## Epic 2: Risk Digest

Surface review-worthy actions across sessions and projects.

### Deliverables

- Deterministic risk taxonomy:
  - destructive filesystem;
  - privileged/sudo-like;
  - network access;
  - secret-adjacent reads or writes;
  - dependency or CI configuration changes;
  - publish/deploy commands;
  - denied actions;
  - unknown tool behavior.
- CLI: `wisphive history risk --since <range>` and project/session filters.
- Web risk badges on History, Sessions, and the session summary.
- Tests for high-signal patterns and false-positive boundaries.

### Acceptance Criteria

- A user can answer "what should I review from agent activity this week?"
- The digest changes no policy and adds no auto-approve rules.
- Each risk item cites source rows and the deterministic rule that matched.

## Epic 3: Agent Operations Dashboard

Turn history into operational observability for agent supervision.

### Deliverables

- Project/session aggregates:
  - approvals, denials, asks, auto-approvals;
  - tool-result coverage;
  - failed command/result counts;
  - decision latency;
  - top commands and top touched files.
- Web dashboard over project and session activity.
- CLI export for machine-readable aggregate data.

### Acceptance Criteria

- The dashboard answers which projects are busiest, which sessions look stuck,
  and where denials or failures cluster.
- Metrics are explainable aggregations, not opaque quality scores.
- Dashboard queries remain bounded on large histories.

## Epic 4: Conflict and Overlap Analytics

Report historical overlap between agents before live conflict blocking exists.

### Deliverables

- Path normalization shared with the future live conflict gate.
- Historical report for:
  - same file touched by multiple agents;
  - concurrent edits within a configurable time window;
  - repeated edits to the same file by one session;
  - hot files by project.
- Session and project overlap warnings.
- Explicit lower-confidence labels for auto-approved writes.

### Acceptance Criteria

- A user can see which sessions overlapped on the same files.
- The report does not block or modify live decisions.
- The implementation can feed `docs/plan-cross-agent-conflict-gate.md`.

## Relationship to Existing Plans

- `docs/plan-policy-learning-engine.md`: risk and pattern extraction can reuse
  deterministic classifiers, but analytics must ship before learned policy.
- `docs/plan-cross-agent-conflict-gate.md`: overlap analytics should share path
  normalization concepts and later provide evidence for live conflict UX.
- `docs/ROADMAP.md`: this workstream is tracked as deterministic analytics and
  remains downstream of the audit/history substrate.
- `docs/decisions/0004-deterministic-agent-analytics-first.md`: locks the
  ordering of deterministic facts first, optional generated summaries later, and
  policy automation last.

## Rollout

1. Analytics substrate.
2. Session work journal.
3. Risk digest.
4. Conflict and overlap analytics.
5. Operations dashboard.
6. Optional LLM narrative layer over the deterministic fact bundle.
7. Policy learning or automation only after the deterministic layer is proven.

## Open Questions

- Should `Ask` decisions be persisted as history entries before summary work
  starts, or should summaries call them out as currently invisible?
- Should JSONL archives be queryable by the first implementation, or should
  analytics initially target active SQLite retention only?
- Should session summaries be cached immediately, or only after the fact schema
  has settled?
- What redaction layer is required before any optional LLM narrative feature?
