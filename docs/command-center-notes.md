# Command-center investigation — how Josef actually operates

**Date:** 2026-07-02
**Question:** What should a personal command center / dashboard look like, investigated fresh — not from the "what does Claude keep re-scripting" theory of the earlier audits, but from "what does Josef repeatedly need to see and decide?"
**Method:** Five parallel subagents: (A) the 15 PNGs in this repo, (B) all genuinely-typed user prompts across ~/.claude/projects (351 messages after noise filtering), (C) interruptions/corrections (462 turns, 19 interrupt stubs), (D) on-disk state-artifact inventory across ~200 project dirs, (E) session rhythm/concurrency from transcript metadata (86 sessions, 30-day retention window).

---

## Stream findings

### A. The 15 PNGs are a methodology, not mockups
Frame-grabs from a YouTube tutorial on building **autonomous, self-verifying recurring loops on Claude Code skills** ("personal OS"): audit workspace for repeated tasks → 4-Condition Test (repeats; rule-decidable done; wasted runs affordable; AI has data+tools) → one Orchestration Skill per loop with goal/steps/done-rule baked in → **Loop Training Mode** toggle (ON = pause per step for approval, skip passing steps, retry cap; OFF = autonomous with checks kept) → final verification by a **separate fresh-context subagent** scoring 1–10 against a threshold → every run writes Output + a Memory file (what worked/failed) → after N clean supervised runs, flip Training Mode OFF. Plus a "quick check before I burn the tokens" human checkpoint before expensive multi-agent runs. This is werkit's apparent direction: a loop control plane, and the command center is its operator console.

### B. Recurring information needs (from typed prompts, ranked)
1. **"What's next / what's left"** — dominant session opener (~15×, 5 projects), re-asked after every `/clear` (45 uses).
2. **Manual session continuity** — pastes prior session's closing status into new sessions (3× verbatim in Darkroom); handoff-doc ceremony (~8×).
3. **Commit-message ritual** — "one line conventional commit for this work?" (~9×, near-verbatim).
4. **Promised-vs-delivered audits** (~12×) — "compare what was expected versus what was delivered"; "EXPLICIT requests that were dropped — where are they?"
5. itr backlog hygiene (~10×); agent/daemon attribution ("Is any of the uncommitted work yours?", "is Codex committing as Claude?") (~9×); **pending-questions visibility** (~10 messages — wisphive auto-approve swallowed AskUserQuestion prompts); cross-project juggling (~8 active projects in a week); roadmap/sprint ceremony state; cost awareness ("you blew through the credits of Fable and delivered the results of Opus").

### C. Interventions: he interrupts for visibility, not direction
Ranked by frequency: (1) git/commit ceremony ~25×; (2) session-boundary state transfer ~20×; (3) **liveness-probe interrupts** — ~half of the 19 interrupt stubs are interrupt→bare "continue", checking for a pulse; (4) swallowed questions/permission prompts (12 hits, one lost afternoon on 06-13/14); (5) dropped explicit requests / delivered-vs-promised distrust (~10, highest emotion; a feedback list evaporated between sessions and was re-sent verbatim); (6) itr filing chores; (7) narrating external state to Claude ("Codex side work will appear", "I cargo cleaned some dirs"); (8) investigation thrash with no decision record; (9) spend-without-output ("blew through the credits… got nothing in return").
**Cross-cutting:** mid-work messages are overwhelmingly about missing state visibility and repeated closure ceremonies. The command center should be a **state mirror and ritual automator, not a steering wheel**.

### D. On-disk reality: federation-ready, but surfaces rot
- `itr`: per-project `.itr.db` (15 exist), **global `-f json` on every subcommand + `--db <path>`** → cross-project queries need no cd. `itr ui` (local browser UI), `stats`, `summary`, `ready`, `wip`, `next`, `export`, `log`, `graph`, `doctor` all exist.
- `wisphive` = "Agent control plane for multiplexed AI workflows" (and already intercepts permission events — that's how the auto-approve bug happened). `clawdia` appears to be a TUI. `kgr orient`/`hotspots` are dashboard-tile-ready.
- ~10 genuinely active projects (Panthexia, wisphive, red, TimelineClock, RetroGames, Harness, Darkroom, skills, werkit, BigGlichur) among ~200 dormant dirs across two roots.
- State files rot: `sprint/CURRENT` stale in 2 of 3 projects that have it; red's roadmap lags commits by a month; only wisphive follows `docs/handoff/YYYY-MM-DD-slug.md`; several active projects have **no discoverable gate command**. Git recency + porcelain dirty count are the trustworthy signals; a dashboard must flag stale pointers, not render them as truth.
- Campaign artifacts (`campaign/*/campaign.json`, `evidence.json`, `ledger.json`, `queue.json`, `reports/*.html` + `campaign/CURRENT`) in Panthexia/wisphive/CodeNexus are the richest machine-readable feed.
- Most active repos sit dirty; werkit/BigGlichur have zero commits on main.

### E. Rhythm: a night-shift fleet operator in bursts
- **Hub + satellites:** every sitting touches 3–5 projects concurrently (never 1); one anchor era (rustglichur → Darkroom → BigGlichur/werkit) with satellite touches. Parallel sessions within one project are routine (3 overlapping werkit sessions on 07-02).
- **Burst-and-silence:** 2–4 day bursts (weekends included, ~6pm–2am Central), then 3–6 days off → re-entry after a gap is a first-class scenario.
- **85% of sessions involve RemoteTrigger**; ~4.8 subagent transcripts per main session; `good` had 80 subagents under one session.
- **Retention hazard:** ~30-day transcript cleanup has already erased ~21 projects' history. Durable state must live in itr/campaign/handoff files, not JSONL.

---

## Synthesis — the command center design

The earlier audits' theory ("Claude re-scripts things → build small tools") produced a pull-based `sitrep` CLI. The fresh evidence changes the shape in three ways:

1. **It must be live, not on-demand.** The liveness-probe interrupts and the swallowed-questions saga are push problems: things must find *him* across 3–5 concurrent sessions. A regenerate-on-command report can't do that.
2. **The centerpiece is the waiting-on-you inbox**, not the project table. He is the approval bottleneck by design (dozens of one-word "proceed" gates); when prompts can't reach him, afternoons are lost.
3. **The natural home is wisphive**, not a new tool. It's already the agent control plane, already sees the permission/question event stream, already has a webui ("Is the webui running?"). The command center is wisphive's face; federation of existing feeds (itr `--db -f json`, campaign JSON, git porcelain, workflow journals), not new plumbing.

### Two layers + one direction

**Layer 1 — Ops console (live; during a burst).** Wisphive surface:
- **Waiting-on-you inbox:** every AskUserQuestion/permission/plan-approval across all sessions, with an audit line for anything auto-answered and by which rule.
- **Agent lanes:** per project × session × subagent — working / waiting-on-input / stalled (600s no-progress), current task label, wave progress, 429 retries. Kills probe interrupts.
- **Working-tree strip per active repo:** dirty count, diff one-liner, **pre-generated conventional commit message**, attribution (Claude / Codex / human lane).
- **Burn meter:** tokens/credits per run next to concrete artifacts produced (commits, issues closed w/ evidence, files); dead-run alert when spend accrues with zero artifacts.

**Layer 2 — State of play (durable; re-entry after silence).** Auto-written, script-rendered:
- Per-project **state-of-play file written at session end by a Stop hook** (replaces the paste-back ritual and manual handoffs), rendered first at session start.
- Cross-project digest over the ~10 active repos: next task (`itr next/ready`), in-progress + stale claims, sprint/campaign CURRENT (flagged when pointer mtime lags git), roadmap drift, last gate result, latest handoff.
- **Promise ledger:** explicit requests captured as tracked items at utterance time, mapped request → issue → commit → evidence; post-blitz "expected vs delivered" diff. Addresses the highest-emotion failure directly.

**Direction — Loop console (the PNG methodology, deferred).** As recurring loops get built per the tutorial (done-rule, training mode, separate verifier, run memory), the ops console grows a loops panel: per loop — last run verdict/score, training-mode status, consecutive-success count toward graduation, next fire time. Architecture should leave room; don't build yet.

### What the old audits got right vs. missed
- Confirmed: whats-next reconciliation, evidence/drift inbox, itr-json prerequisites, fix-wisphive-daemon-first.
- Missed (wrong theory): the questions inbox as centerpiece; live liveness board; commit widget with attribution; auto-handoff at Stop; wisphive as the home; that state files rot and git is ground truth; the burst/re-entry duality; the loop-console direction.
