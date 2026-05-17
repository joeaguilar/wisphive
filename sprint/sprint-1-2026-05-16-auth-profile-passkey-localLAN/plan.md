# Sprint-1 — Desktop passkey login under LocalLAN profile

**Sprint Goal:** Deliver desktop passkey login under the LocalLAN profile by shipping the AuthProfile module, backend WebAuthn handlers, and Login.tsx integration, so first-run desktop users can enroll a passkey at onboarding and one-click-login afterward.

**Epic:** itr#314
**Created:** 2026-05-16
**Story style:** base default (no `STORY_STYLE.md` — consider running `/story-style` to capture project conventions)
**Source:** /alignment session 2026-05-16; design decisions locked across itr#310/#311/#312/#313 + updates to #219/#220/#271/#272/#283 + `docs/plan-mobile-device-pairing.md` "Profiles" section

## Non-Goals

- Enterprise device-enroll flow (itr#313 — deferred; needs #220 first)
- Devices.tsx passkey list / remove / "enroll another" UI (itr#220 — deferred)
- Phone / mobile pairing flows (itr#271, #272 — blocked by AuthProfile but out of this sprint's goal)
- Cross-device passkey portability (architectural lock from 2026-05-16 /alignment: out of v1)
- iOS / Safari support (explicitly OUT per itr#283 epic)
- Android browser smoke (that's #272 territory; this sprint is desktop-only)
- **Enterprise browser smoke** (moved to itr#316 — blocked by #310 + #270; #270 is not in Sprint-1)

## Definition of Done (sprint-level)

Appended to every story's acceptance criteria.

- All story-level acceptance criteria pass
- `cargo test --workspace` green
- `cargo clippy --workspace -- -D warnings` clean
- `cargo fmt --all` produces no diff
- `just frontend-lint` clean (for stories touching frontend)
- User-visible increment observable: set password → enroll passkey → logout → login-with-passkey, in a real browser
- Docs updated: `CLAUDE.md` AND `AGENTS.md` (Codex-facing mirror) when CLI flags / defaults change; `docs/plan-mobile-device-pairing.md` per #310's existing acceptance

## Sprint Backlog

| # | ID       | Title                                                                                            | Pri  | Risk | Blocked-by  | Files (declared)                                                                                                                              |
|---|----------|--------------------------------------------------------------------------------------------------|------|------|-------------|------------------------------------------------------------------------------------------------------------------------------------------------|
| 1 | itr#310  | AuthProfile module + LocalLAN/Enterprise presets + `GET /api/auth/profile`                       | high | med  | —           | `crates/wisphive_web/src/auth_profile.rs` (new), `lib.rs`, `security.rs`, `crates/wisphive_cli/src/main.rs`, `commands/daemon.rs`, `commands/web.rs`, `docs/plan-mobile-device-pairing.md` |
| 2 | itr#311  | #219 PR-4: WebAuthn backend handlers (register/login start/finish) consuming AuthPolicy          | high | high | itr#310     | `crates/wisphive_web/src/auth.rs`, `passkey.rs` (new), `lib.rs`, `crates/wisphive_daemon/src/state.rs`                                         |
| 3 | itr#312  | #219 PR-5: Frontend passkey hooks + Login.tsx enroll-after-set-password + login-with-passkey     | high | med  | itr#311     | `frontend/src/hooks/useAuthProfile.ts` (new), `usePasskey.ts` (new), `useAuth.ts`, `components/Login.tsx`, `api.ts`                            |
| 4 | itr#315  | Desktop browser smoke matrix **(LocalLAN only)**: Chrome/Firefox/Brave register+login + close #219 / #269 | high | med  | itr#312     | `docs/plan-mobile-device-pairing.md` (smoke procedure section)                                                                                |
| 5 | itr#269  | Stage 1c: frontend /onboarding route (closes mechanically when #312 + #315 land; **closure-only bookkeeping — do not start work**) | high | low  | itr#312, #315 | —                                                                                                                                            |

## Discovery

To enumerate this sprint's backlog:

```
itr list --tag sprint-1 --include-blocked --all
```

`itr list` hides blocked issues by default; without `--include-blocked` only the foundation story (#310) + the epic surface. `itr list --parent 314` returns only #315 + the epic — #311/#312 stay parented to feature epic #219, #269 stays parented to #266. Sprint membership is tag-based by design (feature epics outlive sprints).

## Spillover → Product Backlog

Tagged `product-backlog,needs-sprint,risk:<tier>`. Eligible for future `/sprint` adoption.

- **itr#313** — Enterprise device-enroll flow (risk:high; reason: out-of-sprint-goal — focuses on Enterprise profile; also blocked by #220)
- **itr#220** — Devices.tsx UI + TUI event surfacing (risk:med; reason: out-of-sprint-goal — first-enroll handled by Login.tsx, doesn't need Devices)
- **itr#271** — Pairing token module + ephemeral LAN listener (risk:high; reason: out-of-sprint-goal — phone pairing)
- **itr#272** — Frontend add-device QR flow + mobile /pair route (risk:med; reason: out-of-sprint-goal — phone pairing)

## Open Assumptions

Revisit at `/sprint-review`.

- webauthn-rs `SoftPasskey` test authenticator behavior matches real native-API authenticator round-trip closely enough that automated tests catch the bulk of bugs (manual LocalLAN smoke in #315 covers the gap; Enterprise smoke deferred to #316)
- No `webauthn-rs` API breakage between current workspace pin (0.5) and sprint completion
- All sprint work fits a single dev's focused capacity (~4-6 days); the dep chain serializes execution regardless of parallelism
- **Enterprise profile selection requires itr#270** (TLS user-cert flags `--tls-cert` / `--tls-key`) which is NOT in Sprint-1. #310 ships Enterprise *policy* + fail-fast validation that errors clearly when #270 is absent; end-to-end Enterprise behavior validates separately via #316 after #270 ships.

## Execution shape

The dependency chain is strictly sequential — wave-parallelism is low by design:

```
#310 ──► #311 ──► #312 ──► #315 (LocalLAN smoke) ──► #269 closes (bookkeeping)
                                                  └──► closes #219 umbrella (against LocalLAN-only smoke)

[future, post-Sprint-1]
#310 + #270 ──► #316 (Enterprise smoke) ─── validates Enterprise vertical end-to-end
```

`/blitz` will spawn one wave-agent per story, in order. If #310 lands cleanly, #311 can start immediately; same for the chain down.

## Cross-issue context (preserved from /alignment 2026-05-16)

- **AuthProfile reframes the original "strategy C" lock** for WebAuthn. LocalLAN says "no passkey on LAN-IP origin" (phone uses device bearer instead); Enterprise requires a real registrable domain (sidesteps the IP-RP-ID gap).
- **#311 includes a schema migration**: adds `aaguid` + `rp_id` columns to `web_passkeys`. The `rp_id` column powers #310's profile-switch detection.
- **Resident keys always required** (modern UX; single "Login with passkey" button via `start_discoverable_authentication`).
- **Login throttle shared** between password + passkey login (per source IP, existing `LoginThrottle`).
- **Enroll binds to current device.** Cross-device enroll deferred.

## Outcomes

**Goal achievement:** partial — desktop passkey login under LocalLAN ships and is observable end-to-end in Chrome (set-password → enroll Touch ID → logout → login-with-passkey → dashboard, PO-verified 2026-05-17). The cross-browser matrix declared in #315's original AC was de-scoped mid-sprint to itr#323 with PO approval, hence partial rather than yes.
**Reviewed:** 2026-05-17
**Stories:** 5/5 closed, 0 quarantined, 0 open

| ID | Title | Status | Closed | Notes |
|----|-------|--------|--------|-------|
| itr#310 | AuthProfile module + LocalLAN/Enterprise presets + `GET /api/auth/profile` | closed | 2026-05-16 | Review SHOULD-FIX C/D/E applied in-wave (`dd70016`); itr#317/#318 deferred |
| itr#311 | WebAuthn backend handlers (#219 PR-4) | closed | 2026-05-17 | M1 throttle bypass caught + fixed in review (`4e67206`); itr#319/#320 deferred |
| itr#312 | Frontend hooks + Login.tsx (#219 PR-5) | closed | 2026-05-17 | M1 enroll-gate race + M2 sudo JSON leak + M3 countdown reset all fixed in review (`b6662b2`); Vitest infra bootstrapped W3.pre (`92b9379`); itr#321/#322 deferred |
| itr#315 | LocalLAN browser smoke matrix + close #219 / #269 | closed | 2026-05-17 | Doc shipped (`c8cbcaa`); **Chrome happy-path only** + 4 W4 smoke fixes on `main` (`76a4536`, `c3913cb`, `081b9d8`, `caf896d`); Firefox/Brave matrix deferred to itr#323 |
| itr#269 | Stage 1c: frontend /onboarding route (bookkeeping) | closed | 2026-05-17 | Mechanical close per #312 + #315; Wave 5 folded into Wave 4 wrap-up |

**Sprint gate progression**

| Gate | Start | End |
|------|-------|-----|
| cargo test --workspace | 309 passing | 367 passing (+58 net new) |
| cargo clippy --workspace -- -D warnings | clean | clean |
| cargo fmt --all -- --check | red (10 pre-existing) | clean |
| npm run lint (frontend) | red (7 pre-existing) | clean |
| npm test (Vitest) | n/a (no infra) | 47 passing |

**Untracked changes (in git diff but not in itr):**
- `f869fba feat: documents + security` (2026-05-17) — bundled sprint kickoff artifacts (CURRENT, plan.md, wave-1.md) with frontend security hardening: `MarkdownText.tsx` rewrite, `Queue.tsx`, `Sessions.tsx`, `TerminalQueueDock.tsx`, new `queueUtils.ts`. The frontend security piece (markdown XSS hardening per CLAUDE.md's `dangerouslySetInnerHTML` rule) is real work that lived alongside the sprint but outside the sprint backlog. Flagged for PO awareness; no follow-up filed (the work is on `main` and verified by existing tests).
- Working-tree untracked at sprint close: `docs/code_reivew/`, `docs/composers_code_review/`, `docs/securty_walkthrough_demo.html` — local-only typo'd artifacts the W4.pre-intervention explicitly chose not to backfill. Intentional.

## Demo

| ID | Title | PO Decision | Notes |
|----|-------|-------------|-------|
| itr#310 | AuthProfile module + `GET /api/auth/profile` | accepted | Review fixes in-wave; #317/#318 follow-ups filed |
| itr#311 | WebAuthn backend handlers (#219 PR-4) | accepted | M1 throttle bypass fixed with regression test; #319/#320 follow-ups filed |
| itr#312 | Frontend hooks + Login.tsx (#219 PR-5) | accepted | M1/M2/M3 all fixed in-wave; Vitest 0→42 passing; #321/#322 follow-ups filed |
| itr#315 | LocalLAN browser smoke matrix | **conditional** | Chrome happy-path verified; Firefox/Brave matrix deferred to itr#323 per PO call in W4 |
| itr#269 | Stage 1c onboarding (bookkeeping) | accepted | Mechanical close — Wave 5 folded into Wave 4 wrap-up |

**Bugs surfaced during demo:** none new — all four W4 smoke fixes (Origin/Host fallback, HTTP/2 URI authority, useAuthProfile singleton, IP-literal rp_id) were caught + landed in-sprint; review tails captured by itr#316–#322.

## Retro

**Triggered by:**
- 10 named interventions across waves 1–4 (W1.int-1/2, W2.int-1/2, W3.pre, W3.int-1, W4.pre, W4.int-1/2/3)
- 4 post-code-complete smoke fixes hit `main` after #312 was already "done"
- W4.pre-intervention `/docs` gitignored incident invalidated every prior "docs updated" sprint-DoD claim until 2026-05-17

### Plan vs. actual

- **Strict dep chain held** — #310 → #311 → #312 → #315 → #269 executed serially without parallel-wave conflicts. Wave parallelism was 1 by design and that prediction was accurate.
- **AC scope drift on #315** — original AC: 3 browsers × 1 profile. Actual: 1 browser × 1 profile + itr#323 deferral. PO-approved mid-sprint but a sign the AC was over-scoped at planning time given LocalLAN's IP-origin quirks meant browser variation mattered more than expected.
- **Scope creep that paid off:** Vitest infra (0 → 47 tests) wasn't in any story's declared files but was load-bearing for #312's AC. W3.pre-intervention bootstrap was the right call.
- **Scope creep that surprised:** `app.css` edit on #312 (login-passkey-cta + login-divider rules) — outside declared file list; agent flagged; accepted under `feedback_mobile_responsive` rule. Planning gap.

### Friction log

| Event | Source | Root cause |
|-------|--------|------------|
| W1.int-1 — 10 spots of pre-existing fmt drift on `main` | wave-1 verify gate | Pre-existing drift not gated at sprint start; Wave 1 absorbed the cleanup. |
| W1.int-2 — 6 SHOULD-FIX from parallel review | security + Rust review on `a0d6128` | Normal review yield; process working as designed. |
| W2.int-1 — stale doc comments referencing pre-#311 stub state | pre-commit review | Agent updated function body but missed doc text — common LLM near-miss. |
| W2.int-2 — M1 throttle bypass (`record_success` wiped failure history) | security review on `8357500` | Subtle semantic bug invisible without targeted test; review caught it. |
| W3.pre — Vitest infra missing | pre-Wave-3 discovery | Sprint planning didn't audit dev infra against story AC. |
| W3.int-1 — three MUST-FIX (M1 enroll race / M2 sudo JSON leak / M3 throttle reset) | review on `21eb009` | M1: React 19 batching collapsed multi-setState into one render; mocked tests didn't exercise it. M2: leaked internal itr ref to users. M3: pre-existing but reachable via passkey. |
| W4.pre — `/docs` gitignored | pre-Wave-4 `git status` check | `.gitignore` had `/docs` as line 1, predating the planning-doc culture. Sprint plan declared the file but verify gate never checked git tracking. |
| W4.int-1 — `/api/auth/profile` failed on first request because Chrome navigation had no Origin header | manual smoke | Spec read fail-closed too strictly; happy path uncrossable. Fix: Host fallback. |
| W4.int-1 follow-up — HTTP/2 strips Host into URI authority | manual smoke retest | rustls + HTTP/2 path bypassed the Host header. |
| W4.int-2 — useAuthProfile parallel-probe race | manual smoke | Hook wasn't a singleton; two callers each fired a probe. Fix: singleton + `waitForAuthProfile` barrier. |
| W4.int-3 — IP-literal loopback rp_id broke enrollment | manual smoke | Spec said "localhost only" but 127.0.0.1 path wasn't covered. Fix: None for IP loopbacks + 308 redirect. |

### Process improvements (filed as retro action items)

- itr#324 — Pre-sprint verify-gate baseline check before first wave spawns
- itr#325 — /sprint AC dev-infra precondition audit (verify named tools/frameworks exist)
- itr#326 — `.gitignore` sanity check during /sprint planning (high-priority — class of bug silently invalidates sprint claims)
- itr#327 — Manual-smoke prerequisite checklist (Origin/Host/HTTP-2/IP-literal/hook-race patterns) → extends `docs/plan-mobile-device-pairing.md` §0
- itr#328 — React 19 hook-driven UI tests must mount real hook, not mocked hook
- itr#329 — Split MUST-FIX vs SHOULD-FIX in review-fix commits for bisectability

### Agent-specific learnings

- **AC drift on multi-environment claims** — when a story says "Chrome AND Firefox AND Brave", split it. Each browser is a distinct environment with distinct quirks. Sprint-2 stories that claim "works in browsers X/Y/Z" should be N separate substories.
- **PasskeyError taxonomy lesson** — locking taxonomy size early (7 kinds) was right; new edge cases got mapped to `server_rejected` with verbatim message rather than expanding the enum. Pattern reusable for next sprint's auth-adjacent work.
- **Strict-dep-chain sprints have a planning tax** — the sprint executed serially as planned but every wave had to absorb the previous wave's late-surfacing review issues. Sprint-2's plan should budget review-pass time per wave, not just code-spawn time.
- **AuthProfile module pattern worked** — the LocalLAN/Enterprise preset abstraction held up across 5 stories without leaks. Reusable architecture pattern for future profile-style features (mobile-pair profile, headless profile, etc.).
