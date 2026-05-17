# Blitz log — Sprint-1 (started 2026-05-16)

## Config

- **Tracker:** itr (db: `.itr.db`); list: `itr list --tag sprint-1 --include-blocked --all`; close: `itr close <id> "<reason>"`
- **Verify gate:** `cargo test --workspace && cargo clippy --workspace -- -D warnings && cargo fmt --all -- --check && (cd crates/wisphive_web/frontend && npm run lint)`
- **Concurrency:** 5 (effective 1 per wave — strict dep chain)
- **Sprint context:** `sprint/CURRENT` → `sprint-1-2026-05-16-auth-profile-passkey-localLAN`
- **Dep graph:** `kgr` present; baseline check = 4 orphans, 0 cycles, 0 rule violations
- **Stop when:** backlog empty | 2 no-progress waves | foundational quarantine | manual smoke handoff (Wave 4)

## Waves

| Wave | Task   | Title (short)                                    | Files                                                                                          | Blocks |
|------|--------|--------------------------------------------------|-------------------------------------------------------------------------------------------------|--------|
| 1    | #310   | AuthProfile module + presets + `GET /api/auth/profile` | `auth_profile.rs` (new), `lib.rs`, `security.rs`, `cli/main.rs`, `commands/daemon.rs`, `commands/web.rs`, plan doc, **CLAUDE.md**, **AGENTS.md** | — |
| 2    | #311   | WebAuthn backend handlers (#219 PR-4)            | `auth.rs`, `passkey.rs` (new), `lib.rs`, `state.rs`                                            | #310 |
| 3    | #312   | Frontend hooks + Login.tsx (#219 PR-5)           | `useAuth.ts`, `useAuthProfile.ts` (new), `usePasskey.ts` (new), `Login.tsx`, `api.ts`           | #311 |
| 4    | #315   | LocalLAN smoke procedure doc + handoff           | plan doc (smoke procedure section)                                                              | #312 |
| 5    | #269   | Bookkeeping close                                | (none — verify #312 + #315 closed, then close #269)                                            | #312, #315 |

## File conflicts (resolved by dep ordering)

- `crates/wisphive_web/src/lib.rs` — owned by Wave 1 (#310) and Wave 2 (#311). Sequential.
- `docs/plan-mobile-device-pairing.md` — owned by Wave 1 (#310) and Wave 4 (#315). Sequential.

## Semantic warnings

None. Each downstream story consumes the upstream's output as designed.

## Wave 4 special handling

#315's AC requires manual browser execution that an agent cannot do. Wave 4 agent will:

1. Write the smoke procedure section into `docs/plan-mobile-device-pairing.md` (commands, expected screens, common failure modes for LocalLAN with self-signed cert across Chrome/Firefox/Brave).
2. Update CLAUDE.md + AGENTS.md if any procedure step touches CLI behavior the agent learned during the doc write.
3. Run the verify gate.
4. Report "doc complete; manual smoke runs required to fulfill remaining AC".

Blitz then **pauses** before Wave 5. The user runs the smoke manually and closes #315 with results in the close-reason. Wave 5 (the #269 bookkeeping close) can proceed once #315 is closed.

## Interventions

### W1.intervention-1 — fmt cleanup before commit

`cargo fmt --all -- --check` reported 10 spots of pre-existing drift (8 in `wisphive_daemon/src/logging.rs`, 1 each in `http_tests.rs:998` + `lib.rs:853`). All pre-existing on `main` per Wave 1 agent's report. Orchestrator ran `cargo fmt --all` between waves (safe — no agents running), staged the 3 files, committed as `4630abc` ("chore(fmt): clear pre-existing fmt drift surfaced by sprint-1 wave-1 gate"). No behavior change.

### W1.intervention-2 — review-driven SHOULD-FIX applied to #310 commit chain

Two parallel reviewers (`backend-security-auditor` + `general-purpose` Rust review) both verdicted `ship-ready` on commit `a0d6128`. 0 MUST-FIX. 6 SHOULD-FIX deduplicated. Per project's `feedback_review_workflow` tradition (action cheap, file rest as itr):

- **Applied** (committed as `dd70016` "fix(web): address itr#310 review feedback"):
  - C: `is_missing_column_error` now matches on `sqlx::Error::Database` discriminant (robust to sqlx version bumps)
  - D: `lib.rs:499` uses locally re-exported `Url` instead of `webauthn_rs::prelude::Url::parse` (preserves abstraction)
  - E: `validate_enterprise_config` + `EnterpriseValidationError` re-exported at `wisphive_web` crate root (consistency with `AuthProfile`/`AuthPolicy`); CLI call site updated to short path
- **Filed as itr** (deferred):
  - itr#317 — Web security: rate-limit `/api/auth/profile` + `/api/auth/status`; pass parsed Origin through middleware extensions
  - itr#318 — Code-quality follow-ups from #310 review (9 items bundled)
- **#311 context updated** with full handoff notes from the review (challenge_ttl plumbing, RP ID hard contract, OnceCell expectation, LocalLAN localhost-vs-127.0.0.1 subtlety, etc.)

### Wave 1 → Wave 2 gate status

After interventions: `cargo test` ✅ | `cargo clippy` ✅ | `cargo fmt --check` ✅ | `npm run lint` ❌ (7 pre-existing frontend errors deferred — #312 inherits)

Three clean commits on `main` ready for Wave 2 (#311) to land against:
- `a0d6128` — feat(web): AuthProfile (#310)
- `4630abc` — chore(fmt): pre-existing drift cleanup
- `dd70016` — fix(web): #310 review SHOULD-FIX C/D/E



## Outcomes

### Wave 1 — #310 (foreground spawn, completed)

- **Status:** `closed` in itr (close-reason captured in tracker)
- **Tests:** 332 workspace passing (+23 new: 18 unit in `auth_profile`, 5 integration for `/api/auth/profile`)
- **New files:** `crates/wisphive_web/src/auth_profile.rs` (types + presets + `rp_id_for_origin` + `validate_enterprise_config` + `scan_passkey_rp_id_drift` stub + 13 unit tests)
- **Modified:** `lib.rs`, `security.rs`, `http_tests.rs`, `Cargo.toml` (wisphive_web), `cli/main.rs`, `cli/commands/daemon.rs`, `docs/plan-mobile-device-pairing.md`, `CLAUDE.md`, `AGENTS.md`
- **Unblocked:** #311 (Wave 2 ready to spawn)
- **Agent decisions worth noting:**
  - Added `url` and `sqlx` as direct deps to `crates/wisphive_web/Cargo.toml` (both already transitive); justified for explicit use in `auth_profile`.
  - Re-exported `webauthn_rs::prelude::Url` as `wisphive_web::Url` so CLI doesn't need `webauthn-rs` dep.
  - `/api/auth/profile` bypasses BOTH device-token gate AND setup-required gate (mirrors `/api/auth/status` per spec).
  - `can_enroll_passkey_on_this_origin` defaults to `false` when `Origin` header missing/unparseable (fail-closed).
  - Enterprise `rp_origin` auto-derived from `--auth-rp-id` as `https://<rp_id>` (no separate `--rp-origin` flag; #270 can refine later).
  - `TODO(itr#311)` left in `scan_passkey_rp_id_drift` early-return — #311 must delete once it adds the `rp_id` column.
- **Follow-ups surfaced (pre-existing, not Wave 1's fault):**
  - 10 spots of `cargo fmt --check` drift: 8 in `wisphive_daemon/src/logging.rs`, 1 each in `http_tests.rs:998` + `lib.rs:853`.
  - 7 frontend `react-hooks/set-state-in-effect` errors in `Sessions.tsx` + `TerminalQueueDock.tsx`.

### Wave 1 gate (Phase 6) — RED ON PRE-EXISTING

`cargo test` ✅ green (332 passing) | `cargo clippy` ✅ clean | `cargo fmt --all -- --check` ❌ 10 spots drift (all pre-existing) | `npm run lint` ❌ 7 errors (all pre-existing)

Wave 1's contribution is clean — it did not introduce any of the red. The pre-existing red would have prevented Wave 1 from ever running if we'd gated entry; the agent ran the gate as instructed and reported the drift without auto-fixing (per the prohibition rule). Decision on how to clear the gate before Wave 2 is captured in `Interventions` below.



## Quarantine triage notes

(Empty — appended if quarantine triage fires.)
