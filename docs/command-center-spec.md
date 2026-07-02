# Command Center — Steering Spec

**Status:** Draft v1 · 2026-07-02
**Evidence base:** `command-center-notes.md` (5-agent investigation), `reflection-notes.md`, `claude-reflection-notes.md`
**Trackers:** program epic + Layer-2 stories in `werkit/.itr.db`; decision-plane hardening + Layer-1 console stories in `wisphive/.itr.db`

---

## 1. Vision

A command center for a **night-shift fleet operator**: one person running 3–5 concurrent projects per sitting, ~5 subagents per session, 85% remote-triggered, in 2–4 day bursts separated by multi-day silences.

**Theory of operation (the steering principle):** the transcripts show Josef almost never intervenes to change *direction* — interventions are overwhelmingly about **missing state visibility** (what's running, what got done, what was dropped, what was auto-answered) and **repeated closure ceremonies** (commit, handoff, file, close). Therefore:

> The command center is a **state mirror and ritual automator, not a steering wheel.**

Every feature must pass this test: *does it eliminate a documented visibility gap or automate a documented ritual?* If it's a new control surface for steering agents, it's out of scope.

## 2. The two operating modes it must serve

| Mode | When | Question it answers | Latency need |
|---|---|---|---|
| **Burst (live)** | during a working sitting, N sessions live | "what needs *me* right now; is anything stalled?" | push, seconds |
| **Re-entry (durable)** | first session after 3–6 days off, or after `/clear` | "where did I leave off, across everything?" | pull, on demand |

These are different products sharing feeds. Layer 1 serves Burst; Layer 2 serves Re-entry.

## 3. Architecture

```
feeds (existing, federated — no new databases)
├─ wisphive daemon event stream   (PreToolUse/PermissionRequest/Elicitation/Stop, decisions)
├─ itr per-project .itr.db        (itr <cmd> -f json --db <path>; 15 dbs discoverable via find)
├─ git porcelain                  (dirty count, last commit, branch — the ONLY trusted freshness signal)
├─ campaign/*/                    (campaign.json, evidence.json, ledger.json, queue.json, CURRENT)
├─ sprint/CURRENT, docs/ROADMAP.md (consumed WITH staleness flags — these files rot)
├─ workflow journals               (journal.jsonl, agent-*.jsonl under session dirs)
└─ state-of-play files             (NEW — written by Stop hook, §6.1)

surfaces
├─ Layer 1: wisphive web UI (+ TUI parity where cheap)   — live ops console
└─ Layer 2: script-rendered digest (CLI + static HTML)   — re-entry state of play
```

**Host decision:** Layer 1 lives in **wisphive** (it already ingests the hook event stream, has web + TUI crates, and is self-described as the agent control plane). No new daemon. Layer 2 is a renderer over files + `itr -f json` and may live in werkit as scripts/skill; it must not require the daemon to be up.

**Ground-truth rule:** git recency + porcelain beat artifact mtimes. `sprint/CURRENT` and `ROADMAP.md` are *displayed with a staleness badge* when their mtime lags the repo's last commit by >7 days — never rendered as silent truth (observed: 2 of 3 CURRENT pointers stale; red's roadmap lags commits by a month).

**Active-project rule:** a project appears iff it has a commit or `.itr.db` mtime within 21 days. Everything else (≈200 dormant dirs) is filtered, with a count shown ("+193 dormant").

**Durability rule:** transcripts evaporate on a ~30-day window (already lost 21 projects' history). Nothing load-bearing may live only in JSONL; anything worth keeping is written to a file in the repo or an itr issue.

## 4. Phase 0 — Decision-plane trust (P0, blocks everything)

The waiting-on-you inbox is the centerpiece (§5.1), and it is built on wisphive's decision plane — which currently **cannot be trusted**. #380 (closed, live-verified) made questions/plan-mode always defer to the native prompt. Five open bugs remain, all of which can silently weaken or misroute gating policy:

- **wisphive#358** — web Config save full-replaces config.json, silently wiping `tool_rules.deny_patterns`, event toggles, retention keys.
- **wisphive#361** — CLI `config set` round-trips through lossy `UserConfig`, silently reverting `auto_approve_user_prompt:false` etc.
- **wisphive#360** — TUI "Always Allow" writes a legacy file the hook never reads once a level is set (logs success, does nothing).
- **wisphive#366** — Codex `Decision::Ask` fail-opens into silent approval (no native prompt to defer to).
- **wisphive#308** — corrupt config files silently fall back to defaults and may be overwritten.

Plus one genuinely new requirement:

- **Auto-answer audit trail:** every decision the hook/daemon resolves without a human (auto-approve tier, rule, always-defer bypass in dangerous mode, Codex path) is recorded as an event: `{ts, project, session, agent, tool, decision, decided_by: <layer/rule id>, config_snapshot_hash}`. Queryable via CLI and rendered in the inbox (§5.1). Acceptance is behavioral: reproduce the 06-13/14 scenario (a rule auto-answering) and show the audit line names the rule.

**Exit criterion for Phase 0:** a red-team check — attempt each of the five silent-weakening paths; each either works correctly or fails loudly; every auto-answer is visible in the audit trail. Until then, Layer 1 renders fiction.

## 5. Layer 1 — Live ops console (wisphive)

### 5.1 Waiting-on-you inbox (centerpiece)
One queue, across all projects and sessions, of everything blocked on the human:
- AskUserQuestion / Elicitation (with the question text and options)
- PermissionRequest items in the daemon decision queue
- Plan-mode approvals (ExitPlanMode)
- Optionally: "quick check before I burn the tokens" checkpoints (§7, loop methodology)

Each item: project · session · agent · age · the actual question · answer affordance (respond from the console where the protocol allows; otherwise deep-link/focus the session). Below the queue, the **auto-answer audit feed** (§4) — what was decided *without* you, and by which rule. Empty-state must be explicit: "0 waiting · 14 auto-answered in last hour (view)".

*Evidence:* ~10 messages across 3 projects on 06-13/14 debugging invisible prompts blind; dozens of one-word "proceed" gates; he is the bottleneck by design.

### 5.2 Agent lanes / liveness board
Per project → session → subagent lanes: state **working / waiting-on-input / stalled / done / failed**, current task label (tool name or wave/story id), wave progress (n/m done), retry/429 counts. Stall = no event for 600s (matches the existing "Agent stalled" notification) → visually loud + optional notification. Codex lanes included (the daemon already sees Codex hooks) so "is any of this work yours?" is answerable at a glance.

*Evidence:* ~half of 19 interrupt stubs are pulse checks (interrupt → "continue"); "Is wisphive running?"; blitz agents dying silently after 600s.

### 5.3 Working-tree strip
Per active repo: branch · dirty file count · ahead/behind · one-line diff summary · **pre-generated Conventional Commit message** (regenerated when the tree changes; conforms to `type(scope): summary`) · attribution per change where derivable (which agent lane touched the file, from the event stream; else "human/unknown"). Read-only + copy affordance; committing stays in the session (per "you own git").

*Evidence:* the commit-message ask is the single most repeated ritual (~25×); "Is any of the uncommitted work yours?"; "is Codex committing as Claude?"; most active repos sit dirty.

### 5.4 Burn meter
Per session/run: tokens or credits consumed vs **artifacts produced** (commits, itr issues closed-with-evidence, files written, reports). A **dead-run alert** when spend crosses a threshold with zero artifacts. Model name shown per run.

*Evidence:* "you blew through the credits of Fable and delivered the results of Opus… I gave this another shot and got nothing in return."

## 6. Layer 2 — Durable state of play

### 6.1 State-of-play file (the handoff, automated)
A per-project file, `docs/state-of-play.md` (with `docs/handoff/` archive per the existing wisphive convention), **written by a Stop hook** at session end and **rendered at session start**. Schema (YAML frontmatter + short prose):

```yaml
session: <id>            # and start/end timestamps
in_progress: [<itr ids>] # claimed, not closed
next: <itr id + title>   # from itr next at time of writing
uncommitted: <n files, one-line summary, suggested commit>
gate: <cmd> · <pass|fail|not-run> · <when>
promises_open: [<explicit requests not yet issues/closed>]  # §6.3
notes: <≤5 lines of "what just happened / what to remember">
```

Rules: append-only archive (never destroys prior handoffs); write must be cheap (<2s) and never block session exit; if the hook can't determine a field it writes `unknown`, not a guess. **Sequencing caution:** this is a new hook on the same control plane as Phase 0 — do not ship until Phase 0 lands (layered nondeterminism was the 06-13/14 failure mode).

*Evidence:* pasted-back closing summaries (3× verbatim), "create a handoff for the next session" (~8×), an archive script already requested once.

### 6.2 Cross-project re-entry digest
One command (and static HTML) over the ~10 active projects: per project — next task (`itr next`/`ready` counts) · in-progress + **stale claims** (in-progress with no event >48h) · sprint/campaign CURRENT with staleness badge · roadmap drift (✅ marks contradicted by open itr issues) · last gate result (or "no gate configured" — a finding in itself) · latest state-of-play/handoff date · git dirty/last-commit. Reads only files + `itr --db -f json`; works with the daemon down.

*Evidence:* "what's next / what's left" is the dominant opener (~15×, re-asked after each of 45 `/clear`s); burst-and-silence cadence makes re-entry a first-class scenario.

### 6.3 Promise ledger
Explicit user requests become tracked items at utterance time (a skill-DoD/convention: when the PO states a requirement mid-session, file it to itr with tag `promise` before proceeding). The ledger view traces **request → itr issue → commit(s) → verification evidence**, and renders a post-blitz **expected-vs-delivered diff** (sprint/queue contents vs closed-with-evidence). Dropped items (promise with no issue, issue closed with no evidence) are the alert condition.

*Evidence:* the angriest interventions — "EXPLICIT requests that were dropped — where are they?"; a feedback list evaporating between sessions and being re-sent verbatim; "compare what was expected versus what was delivered" (~12×). Builds on the existing evidence-on-close DoD direction and campaign `evidence.json`.

## 7. Direction (explicitly deferred) — Loop console

The 15 PNGs in this repo document the target methodology: recurring **orchestration loops** with baked done-rules, Loop Training Mode (supervised → autonomous after N clean runs), separate fresh-context verifiers scoring against a threshold, per-run Output + Memory files, and human checkpoints before expensive runs. When loops exist, the console grows a panel: per loop — last verdict/score, training-mode state, consecutive-success count toward graduation, next fire, last run's memory-file summary.

**Do not build now.** But the layers above are its substrate by design: the inbox (§5.1) *is* Training Mode's approval channel and the checkpoint surface; the audit trail (§4) *is* the record of what ran unsupervised; the promise ledger (§6.3) *is* the done-rule audit. Nothing in Layers 1–2 may preclude this.

## 8. Non-goals

- **Not a steering wheel:** no start/stop/retarget-agent controls beyond answering queued decisions (v1).
- **Not a metrics wall:** no charts for their own sake; every tile traces to a documented intervention or ritual.
- **Not a new tracker/db:** federation of existing feeds only.
- **No transcript mining at runtime:** JSONL is a melting foundation; runtime reads are events + files + itr.
- **No display of dormant projects** beyond a count.
- **No global PreToolUse close-gates** (would stall non-UI blitz waves — enforcement stays in skill DoD).

## 9. Sequencing & priorities

| # | Work | Tracker | Pri | Depends on |
|---|---|---|---|---|
| P0 | Decision-plane trust: #358, #361, #360, #366, #308 + audit trail | wisphive | critical | — |
| P1 | §5.1 inbox (incl. audit feed) | wisphive | high | P0 |
| P1 | §5.2 liveness board | wisphive | high | P0 (event trust) |
| P2 | §6.1 state-of-play Stop hook + start render | werkit | high | P0 (hook safety) |
| P2 | §5.3 working-tree strip | wisphive | medium | — |
| P2 | §6.2 re-entry digest | werkit | medium | §6.1 (consumes its files) |
| P3 | §6.3 promise ledger | werkit | medium | evidence-on-close DoD |
| P3 | §5.4 burn meter | wisphive | low | — |
| — | §7 loop console | — | deferred | all of the above |

## 10. Verification standard (Definition of Done, per feature)

Per the global working agreement — *a written value ≠ a wired feature*:
- Layer 1 features: exercised end-to-end against a **real running session** (not fixtures alone) with a screenshot or driven-flow evidence attached to the closing itr note. The inbox specifically must demonstrate a live AskUserQuestion appearing in the queue and being answered.
- Phase 0: the §4 red-team check, evidenced per path.
- Layer 2 features: run against the real ~10-project portfolio; the digest's staleness badges verified against a known-stale pointer (red's `sprint/CURRENT` is a natural fixture).
- Every close: evidence linked in the itr note.

## 11. Open questions (decide when reached, not now)

1. Inbox answer path: respond from the web console vs deep-link to the owning terminal session — depends on what the daemon decision protocol already supports (ClientMessage::Ask is wired; scope of answering AskUserQuestion remotely TBD).
2. Notification channel for stalls/inbox (push notification vs TUI bell vs none) — decide after observing real prompt load post-Phase-0.
3. State-of-play writer: wisphive Stop-hook handler vs Claude Code Stop hook in settings — pick whichever survives the Phase-0 hook-safety review.
4. Commit-message generation engine (template from diff stat vs small model call) — start with the cheapest that produces conforming messages.
