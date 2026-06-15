# ADR-0004: Deterministic agent analytics before generated summaries

- **Status:** Accepted
- **Date:** 2026-06-15
- **Deciders:** Product Owner, Codex
- **itr:** #390, #391, #392, #393, #394, #395
- **Related:** ADR-0002

## Context

Wisphive collects enough agent activity to reconstruct much of an agent session:
tool requests, inputs, human decisions, post-use tool results, timestamps,
session IDs, project paths, auto-approved events, and terminal-session
correlation. That data can support summaries, risk reviews, dashboards, policy
learning, and conflict detection.

The tempting path is to send a whole session to an LLM and ask for a summary.
That would be fast to prototype, but the raw data can contain source code,
secrets, prompts, command output, and sensitive project paths. It would also
make user-facing claims difficult to audit and could blur the line between
observability and automated policy decisions.

## Decision

Wisphive will build deterministic analytics first: typed, reproducible session
facts extracted from stored history. Optional LLM narrative summaries may only
run later over that fact bundle, and automated policy changes remain a separate,
later workstream.

## Rationale

Deterministic facts give operators useful value immediately: work journals, risk
digests, overlap reports, and dashboards can cite the exact source history rows
behind each claim. This keeps the first implementation local, testable, and
privacy-conscious.

Separating fact extraction from narrative generation also gives future LLM
features a safer input boundary. Instead of shipping raw logs to a model, a
future summarizer can consume redacted, structured facts and label generated
text as narrative rather than source-of-truth.

Keeping policy learning separate avoids a dangerous feedback loop where
"observed frequently" becomes "safe to auto-approve." Risk analytics can inform
the operator, but it must not silently create or apply policy rules.

## Consequences

- Session summaries must be backed by typed facts and source row references.
- Missing data must be represented explicitly instead of inferred.
- The first analytics implementation should not require any external model
  provider.
- Optional LLM summaries need a redaction and provenance boundary before they
  are added.
- Policy learning remains governed by `docs/plan-policy-learning-engine.md`,
  not by analytics dashboard work.
- Historical overlap reports can feed the cross-agent conflict gate, but they do
  not replace live decision-time conflict detection.

## Alternatives considered

- **LLM-first session summarization** - rejected because it is harder to audit,
  leaks more raw data by default, and makes privacy posture dependent on model
  configuration before deterministic value exists.
- **Policy learning first** - rejected because reducing approval friction before
  facts and risk signals are reliable increases safety risk.
- **Dashboard-only aggregates** - rejected because aggregate metrics without a
  typed fact layer would duplicate extraction logic across CLI, web, and future
  policy features.

## Links

- Plan: `docs/plan-deterministic-agent-analytics.md`
- Roadmap: `docs/ROADMAP.md`
- Related plans: `docs/plan-policy-learning-engine.md`,
  `docs/plan-cross-agent-conflict-gate.md`
- itr: #390, #391, #392, #393, #394, #395
