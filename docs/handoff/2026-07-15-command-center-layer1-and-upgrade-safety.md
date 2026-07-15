# Handoff — Command Center Layer 1 complete + upgrade-safety epic (incident-driven)

**Date:** 2026-07-15 · **Campaign:** `campaign/campaign-003-2026-07-14-command-center-layer1/` · **Predecessor:** [2026-07-03-decision-plane-integrity-phase2.md](2026-07-03-decision-plane-integrity-phase2.md)

## If you only have 60 seconds

Epic **#398 (Command Center Layer 1) is CLOSED** — all seven children done with runtime evidence: Inbox (#399, Sprint-2), **Board** view 8 (#400), **Worktrees strip** view 9 (#401), **Burn meter** (#402), plus answer-path #249/#250/#253. Mid-campaign, a worker's `./install.sh` bricked every gated agent machine-wide (strict-perms hardening + legacy `~/.wisphive` + fail-closed): the PO ruled the stop **desired** security, and epic **#533** turned that into ADR-0010 + actionable denials + `wisphive on/off` + `doctor --fix-perms` + `scripts/wisphive-rescue.sh` + an atomic preflighted `install.sh` + a brick detector — proven by `just redteam` (17/17 + 24/24 vs release binaries). **The installed binaries predate most of this: the operator's next step is running `./install.sh` themselves (smoke item S-7).** Next program work: campaign-004 draft at `campaign/campaign-004-DRAFT-scrollback-layer1-tail/draft.md` (scrollback #284 vs werkit#6 tracker question pending PO).

## What shipped (commits on `crossfire-blitz/20260712-230112`)

| Commit | What |
|---|---|
| 34d05ee, cc33ed8, 14d2274, f96eca6, e9282ac | CLAUDE.md: install policy (#540), CLI on/off + doctor --fix-perms, `query_worktrees`/`query_burn` IPC, BRICKED marker, atomic install, sqlite `-readonly` caution |
| 66ebaba | #473 dup deferred-audit E2E proof (source fix was pre-existing `bc1a192`; the missing piece was live-drive verification) |
| 0b27594 | #535 actionable repair-path denials + **ADR-0010** (fail-closed everywhere is deliberate) |
| 5a3742b, 6251d16 | #541 rescue script + strict `wisphive on/off`; #537 `doctor --fix-perms` |
| a61e7d9 | #400 liveness board (frontend-only; lanes derive client-side; `STALL_THRESHOLD_MS=600_000`) |
| faa32b0 (+36fb881) | #401 working-tree strip (read-only git allowlist, deterministic CC generator, attribution) |
| 88bcaf0 (+d2f0446) | #402 burn meter (labeled activity proxy — wisphive can't see tokens; dead-run ≥10 calls/≥10 min/0 artifacts) |
| b9daa0c | #449 stale deferred deep-link notice (test-first proof it was still live) |
| fe3c1ae, ce85d35, 2f3c1fc | #536/#534 atomic install + `--statecheck`; #538 brick detector; #539 upgrade-safety red-team + decision-plane script repair |

## Hard rules established

1. **ADR-0010:** fail-closed everywhere (incl. UserPromptSubmit) is deliberate, PO-endorsed. Never "fix" toward fail-open; invest in strict entry (`wisphive on`), actionable denials, and binary-independent exits (`scripts/wisphive-rescue.sh`).
2. **`./install.sh` is human-supervised** (#540, CLAUDE.md): agents never swap live gated binaries; they verify via `./target/release` + isolated strict-perms HOME (dir 0700, mode 0600).
3. **Spec §5 held everywhere:** all three new views are state mirrors with zero write affordances, test-enforced by button enumeration.
4. **Never embed raw control bytes in source** — two P1s this campaign (NUL-as-separator made .ts/.tsx files git-binary, hiding diffs from review). Gate pending: itr#551 (high).
5. Inspect a live daemon's `wisphive.db` only with `sqlite3 -readonly` (#555).

## Where to start next

- **PO async:** `reports/smoke-test.html` (S-1..S-7; S-7 = the install moment), `reports/roadmap-update.html` (Phase 2 stale-row correction + Phase 3 → ✅ — apply via `/roadmap --update`).
- **Campaign-004:** draft at `campaign/campaign-004-DRAFT-scrollback-layer1-tail/draft.md`; blocking PO question = tracker boundary (werkit#6 Phase-4 vs wisphive scrollback #284).
- **Open tails:** epic #533 refinements (#543–#545, #556–#557), review follow-ups (#544–#555, #558), flaky #542, NUL gate #551.

## Follow-ups filed this campaign

#542 flaky pidfile test · #543 shared validator module · #544 doctor exit code · #545 head -c POSIX · #546 deferredKey test import · #547 css design-debt · #548 stream output cap · #549 Bash false-attribution ranking · #550 allowlist width · #551 NUL-byte gate (high) · #552 quoting-edge classifier · #553 human-approved-row test · #554 BRICKED surfacing in doctor/TUI · #555 WAL mechanism · #556 preflight harness commit · #557 redteam TMPDIR isolation · #558 stale-notice truncation
