# Decision-plane trust hardening (Command Center Phase 1) — handoff & next steps

**Date:** 2026-07-03
**Branch:** `main` @ `691b446` (+ docs commit after)
**Epic / itr:** itr#396 (Command Center P0) — CLOSED with red-team evidence
**Closed this session:** itr#396, #358, #360, #361, #366, #308, #301, #302, #397
**Filed this session:** itr#407 (config write lock), #408 (nested-null rule deletion), #409 (TUI feedback on persist failure)
**Predecessor handoff:** `docs/handoff/2026-06-15-daemon-refactors-and-security-band.md`

If you only have 60 seconds: Phase 1 of the Command Center program is done —
the five silent-weakening config bugs are fixed, the auto-answer audit trail
(itr#397) is live (`wisphive audit`), and the spec §4 red-team exit criterion
passed against installed binaries (evidence on itr#396). **Start next at Phase 2
(epic itr#403, decision-plane integrity)** — it blocks the inbox (#399). One
operational note: the daemon was not running on this machine at session end;
the next `wisphive daemon start` applies the decision_log migration and
startup-reimports the backlog (itr#301) automatically.

## What just shipped

Nine commits, in dependency order:

- `e5fea66` **fix(config)** — `UserConfig` gained a `#[serde(flatten)] extra`
  map so unknown keys (event toggles, future keys) survive every load→save
  (itr#361). New shared primitives in `wisphive_daemon::config`:
  `write_config_atomic` (tmp+fsync+rename, 0600) and `update_config_json`
  (raw-JSON read-modify-write that refuses corrupt files and can reject
  wrong-typed keys). CLI write paths refuse corrupt configs (itr#308). Also
  fixed: derived `Default` gave `notifications: false` vs the serde default
  `true`.
- `81a7803` **fix(daemon)** — TUI "Always Allow" writes `auto_approve_add` in
  config.json (the key the hook actually reads once a level is set), drops the
  tool from `auto_approve_remove` (checked first, would veto), uses
  `ctx.home_dir`, runs in `spawn_blocking` (itr#360).
- `edd5d13` **fix(web)** — `PUT /api/config` is an RFC 7386-style merge patch;
  a partial SPA body can no longer wipe `tool_rules`/toggles/retention keys
  (itr#358). Corrupt file → 409, never clobbered. Validation accepts every
  documented config key; `null` = merge-patch deletion.
- `61fd4e9` **fix(hook)** — Codex `Decision::Ask` fails closed with an explicit
  deny-with-reason instead of exit-0-empty-stdout silent approve (itr#366).
  PreToolUse formatting extracted to pure `pre_tool_use_stdout()`.
- `141fb49` **fix(config)** — post-review hardening: mutation-abort in
  `update_config_json`, `Cache-Control: no-cache` on the SPA shell (stale
  bundles can't speak the pre-merge-patch dialect), clearer CLI errors.
- `0e7b7d9` **fix(daemon)** — event ingest reimports the events.jsonl backlog
  at startup (itr#301; down-time auto-approvals are no longer an audit gap) and
  tool results retry on a bounded 250ms/1s/3s/10s schedule instead of being
  dropped when PostToolUse beats the ingester (itr#302).
- `691b446` **feat(audit)** — the itr#397 audit trail. Hook decision functions
  return the deciding rule; events.jsonl records carry `decided_by` +
  `config_hash` and gained `deferred`/`denied` kinds; `decision_log` gained
  `decided_by`/`config_hash` columns; daemon resolutions stamp
  `human`/`timeout:approve`/`channel_dropped:approve`; new
  `wisphive audit --since/--project/--decided-by/--limit`.

## Hard rules established (do not regress)

1. **One owner for "edit one key in config.json"** —
   `wisphive_daemon::config::update_config_json`. Never full-file-replace from
   a partial view; never round-trip through a struct that drops unknown keys.
2. **Corrupt config is never overwritten.** Every writer (daemon, CLI, web)
   refuses loudly. Read paths may fall back to defaults but must warn on both
   stderr and tracing.
3. **`Ask` must never silently approve.** Codex has no native PreToolUse
   prompt; Ask on that path is deny-with-reason, and the denial is audited
   (`codex_ask_fail_closed:*`).
4. **Every non-human decision is attributed.** New decision paths (plugins,
   policy learning) must emit `decided_by` + `config_hash` — the Command
   Center inbox (#399) will render this stream.
5. **events.jsonl now carries three record kinds** (`auto_approved`,
   `deferred`, `denied`) — consumers must switch on `event`, not assume
   auto_approved.

## Trade-offs / known gaps (filed, not hidden)

- Config writers have **no cross-process lock** — concurrent read-modify-writes
  can lose updates (itr#407, medium). Atomic rename prevents corruption, not
  lost updates.
- Merge-patch can't delete a **single** tool rule via nested null (itr#408).
- "Always Allow" persistence failure is loud only in daemon logs, not the TUI
  (itr#409).
- The `decided_by` retry/attribution does not yet flow into the **web UI**;
  only the CLI renders it. That's Layer 1 (#399) work by design.

## Where to start next

`itr ready` — the program order (docs/ROADMAP.md §Program order) says
**Phase 2: epic itr#403 (decision-plane integrity — audit correctness &
durability)**: #363 ghost approvals, #370 dup-id corruption, #368 fsync, #88
resolver identity, #347, secret redaction #89, hook fail-safety cluster,
pending-decision persistence #297–#300. It blocks the inbox (#399). #301/#302
from that epic are already done (this session).

Verification pattern that worked well here (recommended for Phase 2): unit
tests for each layer, then an end-to-end run of the **real binaries** with an
isolated short-path `HOME` (`/tmp/wh-*` — the scratchpad path exceeds
`SUN_LEN` for the Unix socket), then a consolidated red-team script against
the **installed** binaries before closing the epic.
