# Handoff — mission no-silent-failures (chains 602 / 603 / 588-plan-A)

**Date:** 2026-07-19 · **Predecessor:** [2026-07-15-command-center-layer1-and-upgrade-safety.md](2026-07-15-command-center-layer1-and-upgrade-safety.md)

**If you only have 60 seconds:** a chain-based mission landed 12 links across 3 epics —
web error surfacing + the dead Spawn form (602, closed), no-silent-fail-open on the
managed-spawn path (603, closed: capacity/provider/headless/reaper/process-group-stop),
and pinned-session respawn after daemon restart (588 plan-A slice; epic stays open for
plan B/C). All oracles green at `25432ff`. Full story: `missions/no-silent-failures/`
(contract · per-chain premise/journal · debrief). The operator has NOT yet installed the
new binaries — `docs/smoke/CHECKLIST.md` carries the two post-install items.

## What just shipped

Eleven `Chain-Link:` commits, in landing order: `fd2d3f9` (#560 capacity rejection
fail-closed+audited), `cbe43be` (#567 error-surfacing substrate), `8b260e3` (#562 unknown
provider loud refusal), `20b0ad7` (#565 AgentSpawned producer + UI), `d484e59` (#607
tolerant audit read layer), `2a85eae` (#590 redaction-safe respawn spec), `d787fab`
(#589 pin flag), `9c1c82e` (#559 headless loud deny), `3cd565f` (#568 reaper keep-alive
exemption), `0378792` (#591 pinned respawn live, first Terminals e2e spec), `25432ff`
(#561 process-group stop ladder). ADR-0001/0002 amended in-link; CLAUDE.md updated at
each landing.

## Trade-offs made

- Frozen-DoD discipline over redesign: #561 kept the TERM→grace→KILL ladder despite the
  recon's setsid-escape critique — residuals documented in `STOP_TERM_GRACE` rationale +
  plan-loop-supervisor rail 7; the two-control emergency stop is itr#620.
- Stop outcomes are reply-only (no audit rows) — registered as unratified assumption
  B-A10 (journal 603, L5/review), PO decision rides with #620.
- `#591` fixed two cross-chain tsc errors in-link (production build was silently broken;
  verify-frontend never typechecks — durable fix is itr#616).

## What's NOT shipped — explicit scope gaps

Session-survival plans B/C (FD-handoff, VIP PTY host — epic #588 open) · herdr UX ·
#601 (solo-agent-only, untouched by design) · the residue tickets #604–#621 (triaged in
the debrief; #608 and #616 are the HIGH ones).

## Hard rules established this session

- **"Audited" means end-to-end queryable** (B-A9): a record that can't be read back via
  `wisphive audit`/UI doesn't count. Review against the full write→ingest→query→render path.
- **Terrain re-counts over premise maps:** the chain's site-counts were wrong three times;
  the only fully-correct map came from a mandated independent re-enumeration. Reviewers of
  process/registry code must re-count, not trust.
- Build frontend dist **in-worktree** (`npm run build`), never copy; verify worktree base
  at step zero; daemon reply-variant splits key on `device_id`, never `correlation_id`.
- The 4s stop-ladder budget is compile-time pinned under the CLI's 5s socket timeout —
  raising the grace requires raising the CLI timeout in the same change.

## Where to start next

1. Operator: run `./install.sh`, then burn down the two new smoke-checklist items.
2. itr#616 (typecheck gate) and #608 (project picker) are the highest-leverage opens.
3. #620 carries the B-A10 ratification decision (stop-outcome auditability).
4. Plan B/C session survival resumes from epic #588 (spike #581 queued).

## Memory / docs to read for context

`missions/no-silent-failures/debrief.md` (the full reconciliation) · chain journals under
`missions/no-silent-failures/chains/{602,603,588}/` · `missions/FACTS.jsonl` ·
ADR-0001/0002 amendments · `docs/plan-loop-supervisor.md` rails 6–7.
