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

### W3.pre-intervention — Vitest harness bootstrap

Discovered before spawning Wave 3: `crates/wisphive_web/frontend` had zero Vitest infra (no devDeps, no config, no `test` script, no `.test.ts(x)` files) but #312's AC explicitly requires a 7+ test Vitest matrix (useAuthProfile probe, usePasskey enroll/login error taxonomy, Login.tsx render gating, skip/retry/success transitions). User picked "orchestrator bootstraps pre-Wave-3" per the AskUserQuestion menu — keeps the agent's declared file list scoped to hooks + Login.tsx + api.ts.

Committed as `92b9379` ("chore(frontend): bootstrap Vitest harness for sprint-1 wave-3"):
- devDeps: vitest 4.1, @testing-library/react 16.3 (React 19 support), @testing-library/jest-dom 6.9, @testing-library/user-event 14.6, jsdom 28.1
- `vitest.config.ts` (jsdom env, `globals: false`, setupFiles)
- `src/setupTests.ts` + `src/setupTests.test.ts` (3-test smoke — proves harness end-to-end; agent deletes once real tests land)
- `package.json`: `test` + `test:watch` scripts
- `tsconfig.node.json`: includes vitest.config.ts
- `justfile`: new `frontend-test` recipe
- CLAUDE.md + AGENTS.md: document the recipe

Gate after bootstrap: cargo test 352 ✅ | clippy ✅ | fmt --check ✅ | npm lint ✅ | npm test 3 ✅.

### Wave 3 — #312 (foreground spawn, completed 2026-05-17)

- **Status:** `closed` in itr (close-reason captured in tracker)
- **Commits:** `21eb009` (initial implementation) + `b6662b2` (review fix bundle). 9 + 6 files; +2044 / +720 insertions.
- **Tests:** 33 new Vitest tests at initial commit (replacing 3 placeholder smoke); 42 after the review-fix bundle added 9 more. Cargo workspace tests unchanged at 352.
- **New files:** `hooks/useAuthProfile.ts`, `hooks/usePasskey.ts`, `hooks/useAuthProfile.test.ts`, `hooks/usePasskey.test.ts`, `components/Login.test.tsx`
- **Modified:** `hooks/useAuth.ts`, `components/Login.tsx`, `app.css`, `App.tsx`
- **Deleted:** `src/setupTests.test.ts` (placeholder smoke from W3.pre-intervention)
- **Unblocked:** #220 (out-of-sprint), #315 (Wave 4 ready), and #269 (Wave 5 once #315 closes)
- **Agent decisions worth noting (initial commit):**
  - `useAuth.loginWithPasskey` wrapper added per spec but Login.tsx calls `usePasskey().loginWithPasskey` directly (CQ reviewer flagged dead code; deleted in fix bundle per S3).
  - `app.css` edit outside declared file list (agent flagged): new `.login-passkey-cta` + `.login-divider` rules; scoped to `.login-*`, mobile-responsive inherited from existing `@media (max-width: 480px)`. Accepted — matches `feedback_mobile_responsive` rule.
  - Counter-regression 401 plain-text body surfaces as `server_rejected` with the verbatim message (no `counter_regression` discriminant added — task taxonomy was locked at 7 kinds).
  - base64url helpers feature-detect ES2024 `Uint8Array.fromBase64/toBase64` and fall back to atob/btoa shim for jsdom (Node 22.12 baseline). Production browsers take the native path.
  - In the fix bundle: M1 fix introduced a new `AuthPhase` variant `"authed-pending-enroll"` rather than `EnrollGate` wrapper or `enrollOpportunity` flag. Cleanest of the three options the reviewer offered — Login stays the owner of the enroll UI, App.tsx gate stays the simple `phase !== "authed"`.

### W3.intervention-1 — review-driven MUST-FIX applied to #312 commit chain

Two parallel reviewers (`backend-security-auditor` + `strict-react-reviewer`) on commit `21eb009`. Both verdicted `ship-with-must-fix`. Triage per `feedback_review_workflow` (action cheap, file rest as itr):

- **MUST-FIX applied** (committed as `b6662b2`):
  - **M1 (security):** Post-set-password enroll card never rendered in production — React 19 batched `setPhase("authed")` + Login's `setPendingEnroll(true)` into one render; App.tsx unmounted Login before the card could render. **Fix:** new `"authed-pending-enroll"` AuthPhase + `useAuth.completeEnrollGate()`; Login replaces local pendingEnroll with `phase === "authed-pending-enroll"`. **Regression test** added in `Login.test.tsx` "M1 regression" suite — mounts a real useAuth via AuthHarness (not the previously-mocked onSetPassword that hid the bug).
  - **M2 (security):** Enterprise sudo 403 rendered raw JSON, leaked internal itr#313 reference. **Fix:** `classifyHttpError` now JSON-parses both 400 AND 403; new `sudo_required` PasskeyErrorKind; Login renders "coming soon (tracked as itr#313)". Two new tests pin the JSON and the rendered text.
  - **M3 (code-quality):** Throttle countdown reset on every 429 error identity change (pre-existing; reachable via passkey path). **Fix:** effect now depends on `retryAfter` value, seeds via `max(currentCountdown, retryAfter)`.

- **SHOULD-FIX applied** (same commit, all from the review):
  - **S(sec)1:** try/catch around `browserOpts` construction (enroll + login paths) → malformed server response maps to `server_rejected`.
  - **S(sec)2:** validate `finishBodyJson.token` is a non-empty string before `setWebToken`; new regression test.
  - **S(sec)3:** disable password submit + inputs during `passkeyBusy !== false` (race-able against shared throttle).
  - **S(cq)2:** `useEffect` clears `passkeyError` on `phase` transitions.
  - **S(cq)3:** delete dead `useAuth.loginWithPasskey` wrapper + `PasskeyLoginResult` re-export (YAGNI per reviewer; #220 adds the right shape).
  - **S(cq)6:** userHandle non-null base64url round-trip test (closes a real coverage hole — fixtures always set `userHandle: null`).
  - **S(cq)9:** extract `bufferToB64u(ArrayBuffer)` helper; replaces 7 repetitions (WET threshold per project standards).
  - **M(cq)2:** rename local `session_id` → `sessionId` via destructure rename in 2 spots; wire field stays snake_case on finish bodies.

- **Filed as itr** (deferred):
  - **itr#321** (medium): web security follow-ups from #312 review — retry-after on PasskeyError shape; client-side observability for named PasskeyError kinds; browserOpts whitelist (not spread).
  - **itr#322** (low): code-quality follow-ups from #312 review (8 items bundled) — ES2024 base64 cast hardening; AbortController pattern; integration test for Skip-then-dashboard; passkeyRequired field deletion; InvalidStateError taxonomy refinement; 6 R-items (`navigator.userAgentData`, encode-loop OOM comment, profile-probe console.warn, `getClientExtensionResults` defensive null, client correlation ID, className interpolation helper, Error/has-name disjunction, double-bang defensive layer, shared `RegisterStartResponse`/etc types module, `withPasskeyBusy` helper).

### Wave 3 → Wave 4 gate status

After interventions: `cargo test` ✅ (352 passing) | `cargo clippy` ✅ | `cargo fmt --check` ✅ | `npm run lint` ✅ | `npm test` ✅ (42 passing).

Three new commits on `main` ready for Wave 4 (#315 doc) to land against:
- `92b9379` — chore(frontend): Vitest bootstrap
- `21eb009` — feat(web): frontend passkey hooks + Login.tsx (#312)
- `b6662b2` — fix(web): #312 review M1/M2/M3 + cheap SHOULD-FIX

Total commits in this blitz so far: 8 (a0d6128, 4630abc, dd70016, 8357500, 4e67206, 92b9379, 21eb009, b6662b2).

### W4.pre-intervention — `docs/` rescue (untrack + backfill)

Wave 4 agent wrote 225 lines to `docs/plan-mobile-device-pairing.md` cleanly and reported success. Verify gate green. BUT `git status` showed a clean tree — `/docs` was the FIRST line of `.gitignore`. Every prior "docs updated" claim across the sprint history (including itr#310's commit message, every Sprint-1 DoD entry) was a no-op against version control. Surfaced to user via AskUserQuestion; user picked "remove /docs from .gitignore + commit Wave 4 + backfill" (the Recommended option).

Split into 2 commits:
- `e8817e8` — chore(docs): untrack /docs + backfill the 5 CLAUDE.md-referenced planning docs (open-source-path, plan-cross-agent-conflict-gate, plan-decision-plugins, plan-mobile-device-pairing in its **pre-Wave-4 state**, plan-policy-learning-engine). Intentionally NOT backfilled: `code_reivew/`, `composers_code_review/`, `securty_walkthrough_demo.html` — local-only typo'd / one-off artifacts.
- `c8cbcaa` — docs(plan): LocalLAN browser smoke procedure for sprint-1 wave-4 (itr#315). Clean +224-line diff against the now-tracked pre-Wave-4 state.

Future "docs updated" DoD entries are now load-bearing against version control.

### Wave 4 — #315 (foreground doc spawn, completed 2026-05-17; awaits manual smoke)

- **Status:** code complete; itr#315 stays `open` pending the user-driven LocalLAN browser smoke that fulfills the remaining AC. Blitz **pauses** here per design.
- **Commit:** `c8cbcaa` (docs/plan-mobile-device-pairing.md +224 lines; new section inserted between Acceptance matrix > Trusted-cert path and Milestone sequencing).
- **Tests:** unchanged (doc-only commit). 352 cargo / 42 vitest / clippy + fmt + lint all green.
- **Section structure:** 0 Prerequisites · 1 Daemon startup · 2 First-run set-password · 3 Post-set-password enroll (with M1 callout) · 4 Logout + login-with-passkey · 5 Edge cases (LAN-IP origin, throttle, skip, re-enroll UX gap → itr#322 CQ S8) · 6 Per-browser quirks (Chrome/Brave/Firefox; Safari OUT per itr#283) · 7 Common failure modes & fixes (TLS warning per-browser recipes, missing-button diagnostic, Touch ID/Hello/USB key recovery, itr#319 device-row 401 gap) · 8 Result-capture markdown table · 9 Closing itr#219 + itr#269.
- **Reusable:** itr#316 (Enterprise smoke matrix, blocked-by itr#270) can copy and extend the same shape once user-cert flags ship.

### Wave 4 → Wave 5 gate status

After interventions: `cargo test` ✅ (352 passing) | `cargo clippy` ✅ | `cargo fmt --check` ✅ | `npm run lint` ✅ | `npm test` ✅ (42 passing). Sprint DoD's "Docs updated when user-facing behavior changes" criterion now meaningful against version control.

Two new commits on `main` since Wave 3 close:
- `e8817e8` — chore(docs): untrack /docs + backfill planning docs (W4.pre-intervention)
- `c8cbcaa` — docs(plan): #315 LocalLAN smoke procedure

Total commits in this blitz so far: 10.

### Blitz pause point (between Wave 4 and Wave 5)

User actions required to unblock Wave 5:

1. Execute the "LocalLAN browser smoke procedure (itr#315)" from `docs/plan-mobile-device-pairing.md` across Chrome / Firefox / Brave. Capture the result table per §8.
2. `itr close 315 "<smoke results>"` — paste the captured table.
3. `itr close 219 "<smoke results>; Enterprise smoke deferred to itr#316 pending itr#270>"` — same table, plus the Enterprise deferral note.

When both close, Wave 5 fires:

4. Orchestrator runs `itr close 269 "Closed mechanically — #312 + #315 fulfilled passkey onboarding acceptance."` (folded into Wave 4 wrap-up per user direction at Phase 0).



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

### Wave 5 — itr#269 (folded into Wave 4 wrap-up, completed 2026-05-17)

Per Product Owner direction at /blitz Phase 0, Wave 5's mechanical close of itr#269 was folded into Wave 4 wrap-up rather than spawning an agent for a one-line `itr close`. itr#269 closed after Chrome happy-path smoke verification confirmed #312 + #315's acceptance — no code change against #269 itself per its own bookkeeping note.

### Sprint close (post-manual-smoke)

Five issues closed in sequence after Product Owner's "looks good" on the Chrome happy-path verification:

- itr#315 — LocalLAN smoke procedure doc + Chrome happy-path table in close-reason
- itr#219 — WebAuthn umbrella; v1 scope cut documented (LocalLAN only; Firefox/Brave deferred to itr#324; Safari/Android out; USB-key webauthn-rs limitation tracked under itr#321)
- itr#269 — Mechanical bookkeeping (Wave 5)
- itr#314 — Sprint epic with all 5 stories + interventions accounted for

AC text reconciliation (review item #4) applied to #219 and #314 BEFORE closing — both contained stale "both profiles" / "Android smoke" wording from the original plan that the LocalLAN-only v1 scope superseded. New ACs explicitly cite the Enterprise → itr#316 + multi-browser → itr#324 deferrals.

itr#324 filed (medium) for the deferred Firefox + Brave matrix + per-browser §5 edge cases. Chrome happy-path covered in #315 close.

### Sprint blitz final gate state (after all wave-4 smoke interventions)

| Gate | Start of sprint | End of sprint |
|------|-----------------|---------------|
| cargo test --workspace | 309 passing | 367 passing (+58 net new across waves) |
| cargo clippy --workspace -- -D warnings | clean | clean |
| cargo fmt --all -- --check | red (10 pre-existing) | clean |
| npm run lint (frontend) | red (7 pre-existing) | clean |
| npm test (Vitest) | n/a (no infra) | 47 passing |

### Total commits in this blitz: 13

Waves 1+2 (already on `main` at handoff): `a0d6128`, `4630abc`, `dd70016`, `8357500`, `4e67206`, `96d1718`.

This session:
- `92b9379` — chore(frontend): Vitest bootstrap (W3.pre-intervention)
- `21eb009` — feat(web): #312 frontend passkey hooks + Login.tsx
- `b6662b2` — fix(web): #312 review M1/M2/M3 + cheap SHOULD-FIX
- `e8817e8` — chore(docs): /docs untrack + backfill (W4.pre-intervention)
- `c8cbcaa` — docs(plan): #315 LocalLAN smoke procedure
- `40b58ef` — docs(sprint): Wave 3+4 outcome
- `76a4536` — fix(web): /api/auth/profile Host fallback (W4.intervention)
- `c3913cb` — fix(web): /api/auth/profile HTTP/2 URI authority (W4.intervention follow-up)
- `081b9d8` — fix(web): useAuthProfile singleton + waitForAuthProfile barrier (W4.intervention-2)
- `f0ae293` — docs(plan): USB-key webauthn-rs limitation callout
- `caf896d` — fix(web): IP-literal None for rp_id + 308 redirect (W4.intervention-3)
- (this commit) — docs(sprint): wave-1.md final outcome + Wave 5 close

### itr ledger this sprint

Closed: 310, 311, 312, 315, 269, 219, 314.

Filed as follow-ups (deferred):
- itr#316 (Enterprise smoke matrix; blocked-by 270)
- itr#317 (rate-limit /api/auth/profile + /api/auth/status)
- itr#318 (#310 review code-quality bundle; 9 items)
- itr#319 (#311 review security bundle; device-row semantics, LAN port-mapping, blob versioning)
- itr#320 (#311 review code-quality bundle; 7 items)
- itr#321 (#312 review security bundle + wave-4 review items 2/3; retry-after taxonomy, observability, browserOpts whitelist, resident-key upgrade, ChallengeStore rate-limit + size cap)
- itr#322 (#312 review code-quality bundle; 8 items)
- itr#324 (full Firefox + Brave LocalLAN smoke matrix + §5 edge cases per browser)

### Recommend next session

Run `/sprint-review` — the sprint had 5 interventions (W3.pre, W4.pre, W4, W4-2, W4-3), which is above the friction-signal threshold for Adaptive Retro. Sprint epic is already closed; `/sprint-review` would formalize the retro + update `sprint/CURRENT`.



## Quarantine triage notes

(Empty — appended if quarantine triage fires. None fired during sprint-1.)
