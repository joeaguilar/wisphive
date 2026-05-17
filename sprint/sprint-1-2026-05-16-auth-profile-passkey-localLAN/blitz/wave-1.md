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

### Wave 2 — #311 (foreground spawn, completed 2026-05-17)

- **Status:** `closed` in itr (close-reason captured in tracker). Reviewers caught one MUST-FIX (M1) corrected in review pass.
- **Tests:** 351 workspace passing on agent's final report → 352 after M1 regression test added in review pass.
- **New files:** `crates/wisphive_web/src/passkey.rs` (~800 lines: webauthn_for OnceCell cache + ChallengeStore + TTL reaper + local_lan_rp_origin helper + resolve_passkey_rp)
- **Modified:** `lib.rs` (4 passkey routes + AppState wiring), `security.rs` (passkey path classification), `auth_profile.rs` (scan_passkey_rp_id_drift now real, TODO removed), `http_tests.rs` (9 HTTP integration tests + AppState passkey_challenges field), `state.rs` (schema migration + insert_web_passkey sig update + find_web_passkey_by_credential_id + update_passkey_sign_count_and_last_used), `Cargo.toml` (workspace: webauthn-rs conditional-ui + uuid v5; wisphive_web: chrono)
- **Schema migration**: idempotent ALTER via new `try_add_column` helper; aaguid + rp_id columns added; pre-existing rows get `rp_id=''` and warn-on-startup via drift scan
- **Unblocked:** #312 (Wave 3 ready)
- **webauthn-rs 0.5 quirks discovered:** start_passkey_registration hardcodes `require_resident_key(false)` AND `UserVerificationPolicy::Required` (LocalLAN's spec'd Preferred is overridden — accepted because modern authenticators create resident credentials regardless); no public SoftPasskey test authenticator in 0.5 (end-to-end crypto round-trip tests skipped, handler-level tests cover non-crypto paths); start_discoverable_authentication behind conditional-ui feature; Passkey::aaguid() not public without danger-credential-internals (storing None for v1)
- **Sudo gate placeholder for Enterprise register**: returns 403 always with `sudo_required_for_passkey_register` discriminant. Full freshness check needs daemon IPC from #313 — handler is one match-arm change once that IPC ships.

### W2.intervention-1 — stale doc comments cleaned before commit

Two doc comments in `auth_profile.rs` and `lib.rs` still referenced the pre-#311 stub state of `scan_passkey_rp_id_drift` ("no-op stub until itr#311 adds the rp_id column"). The agent updated the function but missed the doc text. Orchestrator fixed both before committing #311 as `8357500`.

### W2.intervention-2 — review-driven SHOULD-FIX applied to #311 commit chain

Two parallel reviewers spawned on `8357500`. Security: `ship-with-must-fix` (M1). Rust code-quality: `ship-ready` (5 SHOULD-FIX, mostly polish).

- **MUST-FIX applied** (committed as `4e67206`):
  - **M1**: `/login/start` was calling `record_success` which wipes per-IP failure history. Added `AttemptGuard::release_slot()` (decrements `in_flight` only, preserves `failures`/`locked_until`). Replaced the bad call in `post_passkey_login_start`. New regression test `passkey_login_start_does_not_wipe_throttle_after_failures` proves second failure produces backoff_for(2)≈500ms not backoff_for(1)≈250ms.
- **SHOULD-FIX applied** (same commit):
  - **R1**: replaced stream-of-consciousness whiteboard comment in passkey login device-row minting with 5-line conclusion-only summary.
  - **S1**: added `passkey_register_failure` audit rows on `unknown_session` / `wrong_session_variant` paths in register/finish.
  - **S3**: added `passkey_register_start_ok` + `passkey_login_start_ok` audit events — completes the 6-event ceremony trace.
  - **R3**: `passkey.rs` tests previously shared cache key `(localhost, https://localhost:3100, 300s)` under parallel execution. New `unique_port()` (AtomicU16 starting at 35000) gives each test a distinct URL → no cross-test cache aliasing.
  - **R4**: added comment on `ChallengeStore::take()` clarifying post-expiry is destructive.
  - **R5**: rewrote `is_duplicate_column_error` doc explaining WHY message-match is the contract (SQLite code "1" too generic — catches disk-full, permission-denied too). Doc points at itr#320 for future tracking.
  - **Reaper docstring fix**: was wrong that "drop = abort" — corrected.
- **SHOULD-FIX partial** (S2/R2 device-row semantics): backend `LoginResponse.enrolling_device_id: Option<String>` field added so #312 can call `list_web_passkeys_for_device(enrolling_device_id)` correctly. Full design (N+1 rows per user, cascade shape) filed as itr#319.
- **Filed as itr**:
  - **itr#319** (medium): security follow-ups — device-row semantics full design, LAN port-mapping rp_origin fix, Passkey blob versioning.
  - **itr#320** (low): code-quality follow-ups bundle — 7 items including error-discriminant JSON normalization (which #312 will care about).
- **#312 context updated** with full inherited contract: response shapes (flattened session_id, enrolling_device_id), stable error discriminants (`passkey_unavailable_on_this_origin`, `sudo_required_for_passkey_register`), behavioral contracts (shared throttle with password login, counter-regression handling, 32 KiB body cap, /start consumes throttle slot — don't call on page-load), pre-existing issues, tests needed.

### Wave 2 → Wave 3 gate status

After interventions: `cargo test` ✅ (352 passing) | `cargo clippy` ✅ | `cargo fmt --check` ✅ | `npm run lint` ✅ (the pre-existing 7 errors resolved between sprint planning and now)

Three new commits on `main` ready for Wave 3 (#312 frontend) to land against:
- `8357500` — feat(web): WebAuthn passkey backend handlers (#311)
- `4e67206` — fix(web): #311 review M1 throttle bypass + audit gaps + cleanup

Total commits in this blitz so far: 5 (a0d6128, 4630abc, dd70016, 8357500, 4e67206).



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
