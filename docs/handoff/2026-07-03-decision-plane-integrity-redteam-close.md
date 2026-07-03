# Decision-plane integrity (#403) — red-team pass & epic close

**Date:** 2026-07-03
**Branch:** `main` @ `0bf1b73`
**Epic / itr:** itr#403 — **CLOSED** (acceptance met)
**Closed this session:** itr#403 (the epic; its 16 members were closed earlier — see predecessor)
**Filed this session:** none
**Predecessor handoff:** `docs/handoff/2026-07-03-decision-plane-integrity-phase2.md` (which listed this red-team pass as the remaining capstone — now done)

If you only have 60 seconds: the epic-level red-team pass ran green (17/17) against
the **release** binaries in a throwaway isolated HOME, so itr#403 is closed. It is
reproducible any time via `just redteam`. The inbox story (#399) is **no longer
blocked by #403**, but it still has three other blockers — see § Where to start next.

## What just shipped

`scripts/redteam-decision-plane.sh` (run it with `just redteam`) — a reproducible
red-team harness that drives the real `wisphive`/`wisphive-hook` release binaries
against a `mktemp` isolated `HOME` (never touches `~/.wisphive`) and asserts the three
audit-integrity properties in #403's acceptance:

1. **Ghost approval (itr#363)** — kill the hook mid-decision → the audit trail has
   exactly one terminal row (`deny` / `hook_disconnected:abandoned`), no leaked pending
   row, and no contradictory approve.
2. **Crash mid-stream (itr#299/#301)** — SIGKILL the daemon while a hook blocks → the
   hook fail-open approves (exit 0, silent allow); an auto-approve issued while the
   daemon is DOWN reaches `events.jsonl`; on restart the orphan is drained as
   `approve` / `daemon_restart:failopen` **and** the downtime auto-approve is reimported
   into `decision_log` (no loss).
3. **Secret redaction (itr#89)** — a `sk-`/`Bearer` secret in `tool_input` is
   `***REDACTED***` in `pending_decisions`, `events.jsonl`, and `decision_log`; the
   notify path shares the same `redact::redact_text` scrubber.

```
0bf1b73  test(redteam): decision-plane integrity harness for epic #403
```

| Surface | Anchor |
|---|---|
| red-team harness | `scripts/redteam-decision-plane.sh`, `just redteam` |
| evidence | itr#403 closing note (17/17 assertions) |

## How this was verified

Ran `just redteam` (release build → isolated HOME → three scenarios) to 17/17 green.
Two initial "failures" were harness-assertion bugs, not product bugs, and were corrected:
(1) a bare Claude-Code fail-open allow is **silent** (empty stdout, exit 0) — the proof is
the exit code, not a `"allow"` string; (2) the hook prefixes `cc-` on the agent_id, so the
downtime-reimport query had to match `cc-downtime-read`. Both fixed; the underlying product
behavior was correct from the first run (visible directly in `decision_log`).

## Trade-offs made

- **Release binaries + isolated `HOME`, not `./install.sh`.** Freshly-built release
  binaries match current HEAD (equivalent to installing) and testing them under
  `HOME=$(mktemp -d /tmp/wh-rt.XXXX)` avoids two hazards: corrupting the operator's live
  `~/.wisphive` audit DB, and swapping this dev session's own installed hook mid-session
  (the new #425 self-protection would then apply to the agent's own writes). The socket
  path must stay short (Unix `SUN_LEN` ~104), so the isolated HOME lives under `/tmp`,
  not the deep scratchpad path.
- **Notification redaction is code-verified, not screen-captured.** Capturing a live
  macOS `osascript` banner isn't automatable; the harness asserts every *persisted*
  surface is redacted and cites that `notify_decision` wraps the body in the same shared
  `redact::redact_text` (`crates/wisphive_daemon/src/notify.rs`). On Linux (`notify-send`)
  a future harness could capture the banner text directly.

## What's NOT shipped — explicit scope gaps

1. **Notification banner text not asserted end-to-end** (macOS osascript is opaque). Low
   risk — same scrubber as the persisted surfaces, which are asserted.
2. **The harness uses fixed sleeps** for async ingest/drain settling. Fine locally; if it
   ever flakes in CI, convert to poll-until-condition loops.

## Where to start next

itr#403 is done and the decision-plane audit stream is complete + durable for #397's
consumers. Next:

1. **The inbox (#399) is NOT yet fully unblocked.** Closing #403 removed it as a blocker,
   but `itr get 399` now shows `BLOCKED_BY: 249, 250, 253` — the Claude-API-alignment
   trio (web ExitPlanMode plan-specific options #249, web AskUserQuestion drops the
   selected answer #250, hook ExitPlanMode empty-plan fallback #253). Clear those three
   first, then #399 can start. They are all in `itr ready`.
2. **Policy-learning engine** — its default-deny blocker (ADR-0005 I9) was cleared by
   #425 this milestone; watch the I2 "no substring `allow_patterns`" invariant.

## Memory / docs to read for context

- Predecessor handoff `docs/handoff/2026-07-03-decision-plane-integrity-phase2.md` (the
  member-level work: #425 self-protection, #298/#299/#300 pending semantics, #337 harness).
- `scripts/redteam-decision-plane.sh` header — the exact scenarios and assertions.
- `~/.claude` memory `reference_wisphive_self_gating` — why the red-team runs against an
  isolated HOME rather than `./install.sh` mid-session.
