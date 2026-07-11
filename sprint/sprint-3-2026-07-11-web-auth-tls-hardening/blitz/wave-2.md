# Blitz log — Sprint-3 Wave 8 (crossfire-review follow-ups)

Second blitz run for sprint-3. Waves 1–7 (the original 22 stories) closed in commit `a38ea02`.
This run clears **Wave 8** — the `/crossfire-review` follow-ups #497–#503.

## Config
- **Tracker:** `itr get <id>` / `itr close <id>` (source of truth)
- **Dep graph:** kgr present — pre-existing `wisphive_daemon/state/*` cycles only, unrelated
- **Verify gate:** `cargo test --workspace && cargo clippy --workspace -- -D warnings && cargo fmt --check`
  - `cargo deny check advisories bans sources` run once by orchestrator at final gate (no Cargo.lock changes expected)
- **Concurrency:** 5 (max 3 agents/wave here)
- **Models:** opus inherited by all; #498 & #501 (C3, security-critical concurrency) pinned to opus
- **Starting commit:** `a38ea02`
- **Runtime-evidence gate:** EXEMPT — all six stories are backend-only Rust security logic, no UI/behavioral surface
- **Final-wave adversarial review:** #502 (Wave 3) gets independent opus + codex gpt-5.6-terra (ultra) reviews before close

## Waves

### Wave 1 (3 agents, disjoint files)
| Task | File | Route | Notes |
|------|------|-------|-------|
| #497 | security.rs | C2 | password_set cache never invalidates on live reset-password |
| #498 | auth.rs | C3 / opus | record_success wipes whole per-IP entry, breaks max_in_flight>1 |
| #500 | tls.rs | C1 | DER NotBefore age check trips ~24h early |

### Wave 2 (2 agents, disjoint files)
| Task | File | Route | Notes |
|------|------|-------|-------|
| #499 | auth.rs | C0 | locked_until doesn't advance for concurrent failures under cap>1 |
| #501 | tls.rs | C3 / opus | randomized tmp filenames leak key material on crash (no sweep) |

### Wave 3 (1 agent + adversarial review)
| Task | File | Route | Notes |
|------|------|-------|-------|
| #502 | auth.rs | C2 | Argon2 param-floor rejection is a one-way ratchet |

### Deferred (not executed this run)
| Task | File | Reason |
|------|------|--------|
| #503 | tls.rs | Needs maintainer decision (self-heal vs hard-fail on corrupt key/cert) → confirmed in /sprint-review |

## File conflicts
- `auth.rs` × 3 (#498, #499, #502) → serialized across Waves 1→2→3. Later waves see prior on-disk edits.
- `tls.rs` × 2 executable (#500, #501) → serialized across Waves 1→2. (#503 also tls.rs but deferred.)
- `security.rs` × 1 (#497) → standalone.

## Semantic warnings
- auth.rs chain touches **different functions**: #498 `record_success`, #499 `apply_failure`, #502 `verify_password`. Minimal semantic overlap, but same-file so serialized.
- #498 and #499 both concern the `max_in_flight>1` NAT-office scenario (siblings on one IP); #499's fix reads the throttle map that #498 restructures — Wave 2's #499 agent must build on #498's Wave-1 edit already on disk.

## Interventions
- **W1 gate (clippy --all-targets):** agents ran clippy without `--all-targets`, so two lints in their new test code slipped through. Fixed by orchestrator: `auth.rs:1318` `map.get(&key).is_none()` → `!map.contains_key(&key)` (#498's test); `tls.rs:1103` OpenOptions missing `.truncate(true)` (#500's test). Re-ran gate → green (exit 0).

## Outcomes
- **Wave 1:** #497 closed · #498 closed · #500 closed. Gate green after 1 clippy-lint intervention.
- **Wave 2:** #499 closed · #501 closed. Gate green, no intervention (agents ran `--all-targets` clippy themselves).
- **Wave 3:** #502 CLOSE-PENDING → adversarial review → **closed**. Both reviewers (opus + codex gpt-5.6-terra xhigh) independently confirmed the security substance holds (argon2-params-from-PHC claim verified against `password-hash 0.5.0` source, ratchet closed, no oracle/regression/panic). Both flagged the same gap: `OkRehashNeeded` unwired (opus P3 / codex P1 HOLD — the doc at auth.rs:84 oversold "transparent migration"). Reconciled: corrected the doc in-scope (auth.rs, #502-owned) to state the lockout ratchet is closed but rehash-on-login is NOT yet wired; filed the out-of-scope wiring (lib.rs login handlers + DB write-back + HTTP test) as **itr#504**. Final full-workspace gate + `cargo deny check advisories bans sources` both green.
- **Deferred:** #503 (tls.rs self-heal-vs-hard-fail) held for maintainer decision → **/sprint-review**.
- **Follow-up filed:** #504 (wire OkRehashNeeded rehash-on-verify migration).

## Adversarial review (Wave 3 / #502)
- **opus:** SHIP. P3: OkRehashNeeded unconsumed → follow-up.
- **codex gpt-5.6-terra (effort xhigh):** HOLD → P1: doc oversold un-wired migration; else all checks held. Required-before-ship = wire the rehash sink.
- **Orchestrator reconciliation:** the P1 was about the doc lying, not the security fix. Fixed the doc; the real wiring is a legitimately separate story (#504, different file ownership). #502's AC (doc-only path groom-approved) is satisfied honestly → closed.
