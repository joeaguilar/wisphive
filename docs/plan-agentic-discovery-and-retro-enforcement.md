# Plan — Agentic discovery gates + Retro-executed process enforcement

**Status:** Proposed (PO-resolved 2026-07-18; awaiting write-up review + skill wiring)
**Origin:** sprint-5 `/sprint-review` — a data-backed process retro. Produced by a Fable↔Codex design debate (3 rounds), adjudicated by the PO.
**Related:** itr#521 (the process rule that didn't stick), itr#600 (finish #511 right), itr#528 (Codex-audit ADR concern). Supersedes the "everything becomes a discovery-gate check" idea floated in debate round 1.

---

## 0. Doctrine — the loop-improvement step drives the work

This is **loop-engineering / software-factory** philosophy: a production loop must contain a step whose only job is to **improve the loop itself**, and that step is **as important as the work it produces, because it drives the work.** In agentic execution the Retro *is* that step — and unlike human Scrum it does not defer the improvement into a future sprint; it builds and verifies it inline (§4).

**Provenance note (why this doc exists at all):** the §4 reframe — *process improvement is executed in Retro, never re-queued* — was **not available to the PO or the agent beforehand.** It was learned *through* running the loop: a data-backed retro exposed that process retro items rot (itr#521), a Fable↔Codex debate surfaced the enforcement mechanics, and the PO's insight closed it. That is the doctrine demonstrating itself — the loop-improvement step produced a loop improvement neither party could have specified in advance. Because it was learned in-process, **it clears prior assumptions** about how process work should be scheduled (the human-Scrum "slice it into the next sprint" default is explicitly retired here).

---

## 1. The problem (measured, not asserted)

From the itr SQLite DB and sprint artifacts, verified this session (do not restate as vibe — these are the numbers):

- **The review-debt circle.** Each sprint's review generates 8–20 follow-up tickets that become the majority of the *next* sprint. Sprint-5 was **58% cleanup of sprint-4's review output** (9 `sprint-4-followup` + 2 `review-followup`). Sprint-3 spawned 20 follow-ups and was itself 57% prior-review debt.
- **Blockers surface during execution/review, not planning.** itr#511 (audit Codex's effective hook inventory) = 4 model-escalation rounds, +4543 lines. Its hardest unknowns were answerable from Codex source cloned on disk at `inspiration/codex` (Apache-2.0, HEAD `3151954`) — never attached to the ticket or read during planning. Cross-review could only *contain* it, never *prevent* it.
- **Process retro items don't stick.** sprint-4 filed itr#521 ("enumerate the REAL cross-crate files, stop guessing ownership"); sprint-5's planner then still mis-guessed ownership on #516/#263/#513 (`sprint-5/blitz/wave-plan.md:41-44`). A prose action item aimed at a future planner's memory rots within one sprint.
- **Waste is NOT failed stories.** Hard reopens = 12 ever; completion ~100% on sprints 3/4/5. The cost is (a) the self-feeding review circle and (b) enormous cost-per-close on a few stories that hit unknowns mid-execution.
- **Scope of the fix.** Discovery-addressable follow-ups are only ~30–50% of volume; 50–70% arise from code/design that must *exist* before review can expose the flaw. So upfront discovery alone cannot reach the majority class — a tighter execution/review loop must carry it.

**Root-cause note (also measured):** the reverse-engineer-instead-of-read-source failure is NOT a recurring pattern — the sprint-1→4 audit shows it absent, with positive counter-examples (sprint-3 #502 verified Argon2 against `password-hash 0.5.0` source; sprint-4 #303 reasoned from `portable-pty`'s model). #511 is a **first detonation** of a class the retros never modeled. The design below makes that class impossible to file blind.

---

## 2. Design principles (from the debate)

1. **Routing ≠ proof.** Mechanical triggers cheaply *route* risk; they never certify safety. False negatives are backstopped adversarially; false positives cost one cheap scout.
2. **Provenance, not self-certification.** A gate that checks a self-authored dossier string (`UNKNOWNS: 0 open`) is itr#521 one layer up. Enforcement must read **recorded tool activity**, not typed Markdown.
3. **Adversarial, cross-family.** A challenger or a fixture written and judged by the *same* model class is closure-laundering (this repo has documented `gpt-5.6-terra` writing vacuous green checks — see `reference_terra_low_effort_cheats`). Challenger and enforcement fixtures must be a **different model family** and cross-reviewed.
4. **Right batching boundary.** Not "before ALL execution" (over-serves the 30–50%, can't reach the 50–70%). Discovery is **story-scoped**; the review loop tightens to the **first implementation slice**.
5. **Process improvement is Retro's job, executed inline — never re-queued.** See §4. This is the load-bearing decision.

---

## 3. The discovery + execution pipeline (A–F)

Applies to `/sprint` (planning), `/blitz` · `/crossfire-blitz` (execution).

**A. Triage → routing (`/sprint` Phase 2.5, new).** Mechanical triggers stamp `recon:route` at filing — *routing, not proof*:
- **T1** — AC/context references external-system behavior not defined in this repo (Codex, Claude hooks, browser, OS).
- **T2** — `FILES` spans ≥2 crates, or was inferred without a `kgr refs` / `kgr query --who-imports` receipt.
- **T3** — AC contains audit / enumerate / effective / actual / real / "verify what X does".
- **T4** — a sibling story in the same epic previously escalated models or took >2 review rounds.
- **Catch-all** — complexity ≥C4, `risk:high`, or routed to fable/opus.

The rule decides, not the planner. Override = `recon:waived` + a reason line (auditable, never silent).

**B. Story-scoped recon (replaces a sprint-wide Wave 0).** Each `recon:route` story gets a read-only scout (no Edit/Write tools) as a `blocked-by` on **that story only** — the wave planner schedules it like any dependency, so unrouted stories start immediately. The scout stamps **its own session id** into the dossier header (`sprint/{folder}/discovery/<id>.md`).

**C. Provenance gate (`scripts/recon-gate.sh <id>`), machine-checked from the transcript.** Path-level, `ccq`-backed:
- Every SOURCES path/URL must appear as an actual `Read`/`Grep`/`WebFetch` target in the scout's session transcript. A cited-but-never-opened source **fails the gate**.
- Every authoritative-source candidate (the routing trigger names the domain — e.g. "Codex hooks" ⇒ `inspiration/codex` + official docs) is classified `used` / `rejected:<evidence>` / `unresolved`.
- Any `unresolved` that could change AC/behavior/ownership **blocks write capability** and fires the PO ruling event (F).
- The verified `FILES` list is written back onto the ticket from the dossier (kills the #516/#263/#513 mis-guess class).
- **[PO decision 2] Ship path-level now.** Output-vs-transcript matching (asserted command output == recorded output) is **advisory only, never gating** — it is not robustly checkable in this harness today. Do not delay the whole gate waiting for it.

**D. Independent challenger (unlocks write).** A **different-model-family** agent receives the story, the dossier, and the scout's recorded source-access list, and must either name a concrete omitted source/contradiction (→ scout iterates) or return no-objection before write capability unlocks. Also backstops trigger false-negatives: in `/blitz` Phase 2 the challenger may flag an *unrouted* story it believes needed recon.

**E. Slice-review gate (`/blitz`) — primary lever for the majority class.** For a `recon:route` story: after its first acceptance-relevant slice lands, dependent stories and same-surface neighbors are **blocked until cross-model review of the partial implementation passes** (wave planner encodes a synthetic `blocked-by`). This reaches the 50–70% of follow-ups that only exist after code exists.
- **[PO decision 3] Routed-only for sprint-6.** Scope slice-review to `recon:route` stories initially; expand to all-stories-with-dependents only if follow-up data shows the majority class leaking through. Make it falsifiable, not permanent.

**F. PO ruling channel (event-driven, async — [PO decision 1] KEEP).** Fires only on a gate-C `unresolved` unknown. PO disposition from a closed set: `fail-closed:<ruling>` / `descoped:<spike-id>` / `accept-risk:<note>`. This is the #511 human decision, delivered **day 0 instead of round 4**, with no scheduled ceremony. The human ruling on undecidable external behavior is irreplaceable and cheap when rare; loop-tightening cannot produce it earlier — only surfacing the unknown can.

**Smallest thing that would have stopped #511:** T1+T3 fire on its text. One read-only scout, before any execution token, enumerates Codex's effective hook-source merge order with `file:line` from `inspiration/codex`, lists what the source does not decide; the challenger checks for an omitted source; the residual policy choice goes to the PO at day 0. Four escalation rounds and most of +4543 lines collapse into one scout + one decision.

---

## 4. Retro-executed process enforcement (the load-bearing reframe)

**The mismatch this fixes:** in human Scrum, a Retro process improvement must be *sliced into a future sprint* because humans work in sprint cadence. Filing it as backlog — even "highest-priority Pending-enforcement" backlog — drops it into the same queue that never prioritizes process work. **That is precisely the shape that caused #521 to rot.** Moving process work "up front" into `/sprint` keeps it in that shape.

**In the agentic world, Retro does not defer — it executes.** Once Retro decides a process improvement is needed, agents **build and enforce it inline, during the ceremony**, and:

> **Retro cannot close until an end-to-end dry-run of the changed process passes.**

This deletes the "wait for a sprint" disposition entirely. A retro item resolves as exactly one of:

1. **Enforced (built + dry-run-verified in this Retro).** Requires: an observable event where the failure occurs, a blocking checker on that event, a failing regression fixture reproducing the old behavior, a passing fixture proving enforcement — **fixtures cross-model reviewed** (anti-vacuous-green). Then the Retro runs the **process e2e dry-run** (§5) and it must pass.
2. **Norm (PO-signed).** The PO explicitly rules it judgment-only (no cheap observable event exists — e.g. "planner asked closed questions instead of open prose"). Recorded in the invariant registry, revisited at every Retro, **never** given a fake regex to launder it into "enforced."

There is no third "assigned, will do next sprint" state. If a needed improvement can't be built and dry-run-verified in the Retro, that is itself the top blocking work — the ceremony stays open, it does not spill into the backlog.

**Conditional, adaptive depth:** Retro runs process improvement *only if it needed to occur* — a clean sprint (no friction signals) skips it, matching `/sprint-review`'s existing adaptive-retro rule.

**Registry home:** `docs/process/INVARIANTS.md` + fixtures. itr#521 itself lands as Enforced disposition-1 — event: a story filed with multi-crate `FILES` and no `kgr` receipt in the scout transcript; checker: gate C.

---

## 5. The process e2e dry-run gate (what makes §4 real, not theatre)

Before a Retro that produced an Enforced item may close, it runs the **changed workflow end-to-end against a fixture sprint** and asserts the new gate actually bites:

- A synthetic story that *should* trip the new checker is run through triage → recon → gate → challenger and **must be blocked/flagged** at the right step (failing fixture).
- A synthetic story that should pass is run through and **must proceed** (passing fixture, no false-positive deadlock).
- The dry-run uses an **isolated temp HOME / scratch sprint folder** — it never touches `~/.wisphive` or real sprint artifacts (same discipline as `just e2e`).
- Green dry-run = the improvement is enforced and non-blocking-on-good-input. Red = Retro stays open; the improvement is not done.

This is the mechanism that would have made #521 impossible: you cannot *say* "enumerate real files" and move on — the Retro must ship a checker and prove, e2e, that a file-mis-guess is now caught.

---

## 6. Skill/impl wiring (follow-on work — file as the top-priority process epic per PO rule)

- **`/sprint`** — add Phase 2.5 triage (A); emit `recon:route` / `recon:waived` tags.
- **`/blitz` · `/crossfire-blitz`** — recon scout as story-scoped `blocked-by` (B); `scripts/recon-gate.sh` as a BLOCKING preflight (C); challenger unlock (D); slice-review synthetic `blocked-by`, routed-only (E).
- **`/sprint-review`** — add the Retro-executes-process-improvement phase (§4) + the process e2e dry-run close-gate (§5); replace any "file a follow-up action item" step for process defects with inline build+verify.
- **New:** `scripts/recon-gate.sh` (ccq-backed transcript provenance), `docs/process/INVARIANTS.md` (registry), fixture sprint harness for §5.
- **Consider an ADR** for §4 (Retro-executed enforcement + no-defer rule) — it constrains future workflow work and had a real alternative (the human-Scrum "slice into next sprint" default) that was deliberately rejected.

---

## 7. Falsifiable success criteria (sprint-6)

- **Zero** mid-blitz model escalations caused by an unread on-disk/external source.
- **Zero** ownership mis-guesses on `recon:route` stories.
- Follow-up count ≤ **50%** of the sprint-3/4/5 trailing average.
- Any process retro item from sprint-6 is **Enforced (dry-run green) or PO-signed Norm** at Retro close — none carried as backlog.

If 2 of the first 3 miss, the discovery gate is ceremony — remove it. The registry/dry-run gate (§4–5) is judged separately: it passes iff no sprint-6 process item rots the way #521 did.

---

## 8. Open disagreements — all resolved by PO 2026-07-18

1. PO checkpoint shape → **KEEP F** (event-driven, blitz-pausing).
2. Provenance depth → **path-level now**, output-level advisory.
3. Slice-review scope → **routed-only for sprint-6**, expand on data.
4. Registry strictness → **process improvement is Retro's job, executed inline + e2e-dry-run-gated; no defer-to-sprint disposition** (§4). This resolves the mismatch: process work never re-enters the queue that de-prioritizes it.

---

## Provenance

Design debate: Fable (Claude, opening + synthesis) ↔ Codex/gpt-5.6 (adversarial critique + counter-enforcement). Key mutual catches: Codex killed the string-checked dossier (gaming moved, not defeated); Fable supplied the harness fact that path-level provenance IS checkable today via `ccq` transcripts, and turned Codex's own critique back on it (same-model challenger/fixtures = laundering). PO adjudicated all four value-calls.
