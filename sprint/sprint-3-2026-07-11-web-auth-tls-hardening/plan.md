# Sprint-3 — Web-auth / TLS hardening

**Sprint Goal:** Close the known hardening gaps across the web auth, TLS transport, and `/api/auth` surfaces so a LocalLAN-exposed Wisphive daemon resists credential, transport, and rate-limit attacks — Sonnet-led, ≤C2, behavior-preserving.
**Epic:** itr#496
**Created:** 2026-07-11T17:17:07Z
**Story style:** STORY_STYLE.md (Wisphive conventions)
**Provenance:** 22 pre-existing issues selected during the 2026-07-11 backlog grooming (`complexity:C0–C2`, `route:sonnet-5` or cheaper, unblocked). Re-parented into this epic; originals unchanged, grooming tags preserved.

## Non-Goals
- No new auth features — no passkey/webauthn (C3/C4 lane: #319, #427, #321).
- No enterprise TLS / user-cert wiring (#270 is C4); self-signed-cert hardening only.
- No device-management UI / TUI surfacing (#220 is taste=2 → opus lane).
- No protocol or schema changes — behavior-preserving hardening only.
- No throttle calibration from real UX telemetry (#246 lands the knob; tuning deferred).
- No mobile-pairing / QR onboarding (epic #283).

## Definition of Done (sprint-level)
- Story AC passes with its named test/command.
- `cargo test --workspace` green; `cargo clippy --workspace -- -D warnings` clean; `cargo fmt` applied.
- Security dep gate: `cargo deny check advisories bans sources` (Rust stories); `npm audit` incl. `--omit=dev` for frontend stories (#275, #280, #492).
- Behavior-preserving: no protocol/schema change; existing auth/TLS flows still pass.
- Client-observable stories (#275, #280, #494) carry runtime/HTTP-test proof, not just a build.

## Sprint Backlog
Ordered risk → value. All stories `route:sonnet-5` or cheaper (grooming); Sonnet leads end-to-end. No inter-story `blocked-by` — every ticket is single-module and independent.

| ID | Title | Pri | Risk | Files | Blocked-by | AC |
|----|-------|-----|------|-------|------------|----|
| itr#494 | Reject bearer tokens in API query strings | high | high | security.rs, http_tests.rs | — | existing + DoD |
| itr#245 | auth.rs: add verify-deadline so attacker can't hold in_flight | med | high | security.rs | — | existing + DoD |
| itr#256 | Web security: cache password_set + rate-limit /api/auth/status | med | high | security.rs, lib.rs | — | existing + DoD |
| itr#317 | Web security: rate-limit /api/auth/profile + /api/auth/status | med | high | lib.rs, security.rs, auth.rs | — | existing + DoD |
| itr#228 | tls.rs: verify loaded private key matches cert SPKI before reuse | low | high | tls.rs | — | existing + DoD |
| itr#237 | tls.rs: assert cert public key matches on-disk private key | low | high | tls.rs | — | existing + DoD |
| itr#275 | Harden web approve stash: only track sudo-class tools | low | high | useWisphive.ts | — | existing + DoD |
| itr#492 | Frontend dev dependency audit — 4 vulnerabilities | high | med | package.json, package-lock.json | — | existing + DoD |
| itr#226 | tls.rs: cross-check cert age via DER NotBefore | med | med | tls.rs | — | existing + DoD |
| itr#227 | tls.rs: filter SAN/LAN enumeration (skip Docker/VPN/utun) | med | med | tls.rs | — | existing + DoD |
| itr#232 | auth.rs: validate Argon2 params/algorithm on verify | low | med | auth.rs | — | existing + DoD |
| itr#233 | auth.rs: don't extend locked_until on failures during lockout | low | med | auth.rs | — | existing + DoD |
| itr#235 | tls.rs: randomize tmp filename (symlink pre-create TOCTOU) | low | med | tls.rs | — | existing + DoD |
| itr#236 | tls.rs: cross-process flock test (spawn helper binary) | low | med | tls.rs | — | existing + DoD |
| itr#243 | auth.rs: make MAX_IN_FLIGHT_PER_IP configurable | low | med | auth.rs | — | existing + DoD |
| itr#246 | auth.rs: backoff cap configurable (⚠ calibration deferred) | low | med | auth.rs | — | existing + DoD |
| itr#234 | tls.rs: doc NFS caveat + in-process Mutex around flock | low | low | tls.rs | — | existing + DoD |
| itr#244 | auth.rs: held-await variant of parallel_attempts test | low | low | auth.rs | — | existing + DoD |
| itr#247 | auth.rs: hex byte loop — use write! for verifier encoding | low | low | auth.rs | — | existing + DoD |
| itr#258 | Web security: JSON-format web_audit detail column | low | low | lib.rs, state.rs | — | existing + DoD |
| itr#259 | Web security: hide last_ip for peer devices in /api/devices | low | low | lib.rs | — | existing + DoD |
| itr#280 | Bump MIN_PASSWORD_LEN 8→12 (NIST SP 800-63B Rev.4) | low | low | lib.rs, Login.tsx | — | existing + DoD |

### Wave 8 — Crossfire-review follow-ups (added 2026-07-11, post-blitz)

Source: `/crossfire-review` (Codex adversarial-review + Opus, independent) of commit `a38ea02` — the squashed close of Waves 1–7 above. Both lanes independently found itr#497 (strongest signal: corroborated by both). Groomed via `/groom` immediately after filing; **not** all ≤C2 like the original 22 — two land at C3/opus-4.8 given security-critical concurrency stakes and missing verify gates. `itr#503` additionally needs a maintainer decision before it's dispatchable (see Open Assumptions).

| ID | Title | Pri | Risk | Files | Complexity/Route | AC |
|----|-------|-----|------|-------|-------------------|----|
| itr#497 | security.rs: password_set cache never invalidates on live reset-password | high | high | security.rs | C2 / gpt-5.6-terra | existing + DoD |
| itr#498 | auth.rs: record_success wipes whole per-IP throttle entry, breaking max_in_flight>1 | high | high | auth.rs | C3 / opus-4.8 | existing + DoD |
| itr#499 | auth.rs: locked_until doesn't advance for concurrent failures under max_in_flight>1 | med | med | auth.rs | C0 / gpt-5.5 | existing + DoD |
| itr#500 | tls.rs: DER NotBefore age check trips ~24h early | med | med | tls.rs | C1 / gpt-5.6-terra | existing + DoD |
| itr#501 | tls.rs: randomized tmp filenames leak key material on crash (no sweep) | med | med | tls.rs | C3 / opus-4.8 | existing + DoD |
| itr#502 | auth.rs: Argon2 param-floor rejection is a one-way ratchet | low | low | auth.rs | C2 / gpt-5.6-terra | existing + DoD |
| itr#503 | tls.rs: corrupt-but-parseable key/cert hard-fails instead of self-healing | low | low | tls.rs | C2 / gpt-5.6-terra ⚠ needs decision | existing + DoD |

**File contention within Wave 8** (unlike Waves 1–7, this table is not yet blitz-wave-partitioned): `auth.rs` × 3 (itr#498, #499, #502) and `tls.rs` × 3 (itr#500, #501, #503) — `/blitz` will need to serialize these across sub-waves by file ownership, same as the original 22. `itr#497` (security.rs) is standalone.

## Spillover → Product Backlog
None. Every candidate serves the goal and fits ≤C2. The adjacent C3/C4 security work (#319, #427, #321, #364, #493, #223, #270, #220) stays out per Non-Goals and was never in this set.

## Open Assumptions
- **#246** lands the *configurable* backoff-cap knob only; tuning it to a measured value is a declared Non-Goal. Confirm its AC reads "make configurable," not "raise to N" — if it demands a tuned value, it is under-specified and should defer.
- **#492** fixing the Vite dev-server advisories may force a Vite major bump → build/HMR risk. If the bump breaks `frontend-build`, split the upgrade to a follow-up rather than stalling the sprint.
- **#275** is the one story touching `useWisphive.ts` (frontend hook) rather than pure Rust — the Sonnet lane owns it, but closure needs the Vitest matrix green, not just `cargo`.
- **Filing note:** the 22 stories were re-parented from the existing backlog (not created fresh), so their created-dates predate this sprint and their grooming tags (`complexity:`, `route:`) are intact.
- **Wave 8 (itr#503):** its AC is literally "confirm intent with the maintainer" — self-heal-on-corruption vs. hard-fail-on-corruption for `tls.rs::try_load_existing`. Needs a PO/maintainer call before dispatch; not blocking the rest of Wave 8.
- **Wave 8 scope note:** itr#498 and itr#501 landed at C3 (opus-4.8) via `/groom`'s "bump when torn" rule — both lack an explicit verify-gate test in their AC, and itr#498 additionally touches security-critical concurrent bookkeeping shared by every login/reauth/passkey call site. This breaks the original sprint's "≤C2, sonnet-5 or cheaper" framing (Sprint Goal, line 3) for this wave only — the original 22 are unaffected.

## Known Issues & Follow-ups (surfaced during execution — for /sprint-review triage)

Findings and process notes from Waves 1–7 execution and the post-blitz `/crossfire-review`, not yet triaged. Full detail lives in `blitz/wave-1.md`; this is the scannable summary.

**Not yet filed — needs a PO call at review:**
- **Login.tsx `minLength` shadows the custom error message.** The native HTML `minLength={12}` attribute intercepts form submission before the custom JS "Password must be at least 12 characters." message can ever render — it's effectively dead code via mouse/keyboard submit. Pre-existing pattern (same shape existed at the old `MIN_PASSWORD_LEN=8`), not introduced or worsened by itr#280. Low-priority UX polish; file as a follow-up only if the team wants the custom copy to actually surface (e.g. for browsers that suppress native validation UI).

**Bookkeeping gap found (already resolved, but a retro candidate):**
- **itr#245 was already implemented before this sprint started** (shipped under itr#213's `VERIFY_DEADLINE` timeout wrapping in `lib.rs`) but was never closed — the ticket just sat open. The blitz agent caught this via investigation, made no code change, and closed it once the gate was green. This is the same stale-closed-ticket bookkeeping gap `/sprint`'s Phase 0 preflight checks for (`git log --grep='closes #'` cross-referenced against open `itr` status) — worth asking at retro whether that preflight should have caught this one, or whether itr#245's implementing commit predates the lookback window / didn't use a recognized closing verb.

**Process deviation (retro material, no user-facing impact):**
- **itr#227's wave agent closed its task despite its own verify-gate run reporting `cargo fmt --all --check` red** — the drift was 100% inside a same-wave neighbor's (itr#256) in-flight file, which itr#227 correctly never touched, but the agent should have held per its instructions (as itr#237/#245/#243/#235 correctly did in identical situations) rather than closing anyway. No harm resulted — the wave-gate re-run after itr#256 landed confirmed everything green — but worth a retro note on tightening the "stop and report, don't close on red" instruction adherence.

**Noted, no action needed:**
- One flaky, pre-existing, unrelated test (`wisphive_hook::tests::socket_garbage_decision_fails_closed`) failed on a single isolated run during Wave 3 and passed on retry and in the full wave-gate. Not caused by this sprint's changes; non-reproducing, no follow-up filed.
- itr#227 (SAN/LAN interface filtering) unblocked itr#270 (enterprise TLS / user-cert wiring, C4, out of this sprint's scope per Non-Goals) per the closing agent's own note — informational for roadmap sequencing, not actionable here.

**Operational caution until Wave 8 lands** (see Wave 8 table above — itr#497 P0, itr#498 P1): don't document or rely on `wisphive web reset-password` against a *live* server (it won't take effect until restart — itr#497), and don't recommend `WISPHIVE_MAX_IN_FLIGHT_PER_IP > 1` in any deployment guidance yet (concurrent successes can undercount in-flight reservations — itr#498).

## Outcomes

**Goal achievement:** yes
**Reviewed:** 2026-07-11
**Stories:** 28/30 done, 0 quarantined, 2 open (carried to sprint-4)

The 22 planned stories all shipped; a post-blitz `/crossfire-review` added Wave 8 (itr#497–503) and itr#504 was filed mid-run. Of Wave 8, itr#497–502 shipped; itr#503 and itr#504 remain open **by design** — decision-resolved at review and carried forward.

| ID | Cohort | Status | Notes |
|----|--------|--------|-------|
| itr#226–494 (22) | Original sprint-3 | done | All test/http/vitest-gated. Landed in a38ea02. |
| itr#497–502 (6) | Wave 8 crossfire | done | Real bugs found by adversarial review; landed in c518b4b. |
| itr#503 | Wave 8 crossfire | **open → sprint-4** | PO decision: keep fail-closed hard-fail + add operator remediation message + doc. AC rewritten to that spec. |
| itr#504 | Mid-run follow-up | **open → sprint-4 (high)** | PO decision: wire OkRehashNeeded (rehash below-floor Argon2 hashes on verify). |

**Notable bookkeeping:** itr#245 was already implemented under itr#213 but sat open; the blitz agent no-op-closed it after confirming the gate green (retro action itr#507).

**Untracked changes (in git diff but not in itr):** none. The two sprint commits (a38ea02, c518b4b) map cleanly to stories. Working-tree noise (sprint-2 evidence PNGs, `.playwright-mcp/`) is unrelated e2e churn, intentionally not staged.

## Demo

28 done stories batch-accepted by the PO (all carry test evidence in the two sprint commits). The two open items were decision-resolved:

| ID | Title | PO Decision | Notes |
|----|-------|-------------|-------|
| itr#226–502 (28) | Original 22 + Wave 8 #497–502 | accepted | Batch — test-gated, committed |
| itr#503 | corrupt-cert hard-fail vs self-heal | conditional → carryover | Keep fail-closed + remediation message + doc; sprint-4-candidate |
| itr#504 | wire OkRehashNeeded rehash-on-verify | conditional → carryover | Implement at high priority; sprint-4-candidate |

**Bugs surfaced during demo:**
- itr#505 — Login.tsx `minLength={12}` shadows the custom below-floor password error copy (pre-existing; low).

**#503 smoke runbook** (for when the sprint-4 work lands — corrupt-cert startup behavior): boot an isolated daemon under a short scratch `HOME`, let it mint `web.cert.pem`/`web.key.pem`, then replace the key body with valid-PEM-but-unparseable base64 and restart. Expected (decided policy): fail-closed startup with a clear remediation message naming the file and telling the operator to delete the cert files to regenerate — **not** a panic and **not** a silent regen. Repeat for a corrupt `cert.pem`; both new checks (`key_matches_cert_spki`, `der_not_before_unix`) must behave identically. Guardrail: a genuinely mismatched sidecar still regenerates.

## Retro

**Triggered by:** carryover (itr#503, itr#504), a bug surfaced during demo (itr#505), and a process deviation (itr#227 closed on a red gate).

### Plan vs. actual
- Sprint grew 22 → 30 via `/crossfire-review` (Wave 8) — adversarial review legitimately surfaced 7 more real issues, 2 of them harder (C3/opus) than the original "≤C2, sonnet-only" framing. Scope growth was value, not drift.
- 28/30 (93%) closed; the 2 open are decision-gated carryover, not failures.

### Friction log
- **itr#227 closed on a red `cargo fmt --check`** — drift was inside same-wave neighbor itr#256's in-flight file (correctly untouched by #227); post-#256 wave-gate re-run was green, so no harm. Root cause: "stop on red, don't close" adherence when an agent judges the drift to be a neighbor's. → itr#506.
- **itr#245 already shipped but left open** (under itr#213) — `/sprint` Phase 0 stale-ticket preflight didn't catch it; implementing commit likely predates the lookback or lacked a closing verb. → itr#507.
- One flaky pre-existing unrelated test (`wisphive_hook socket_garbage_decision_fails_closed`) failed once in Wave 3, passed on retry — no action.

### Process improvements (filed as retro action items)
- itr#506 — Blitz agent must not close a task on a red verify gate even when drift is a same-wave neighbor's file.
- itr#507 — `/sprint` stale-ticket preflight: widen lookback window / verb detection so already-shipped-but-open tickets are caught at planning time.

### Agent-specific learnings
- Adversarial `/crossfire-review` after a "done" blitz paid for itself here (7 real follow-ups, 2 high/critical). Worth keeping as a standing post-blitz step for security-surface sprints.
- Grooming's "≤C2" framing is a planning estimate, not a ceiling — review found genuinely C3 work hiding under C2 tickets. Fine, as long as the escalation is surfaced (it was, in the Wave 8 scope note).
