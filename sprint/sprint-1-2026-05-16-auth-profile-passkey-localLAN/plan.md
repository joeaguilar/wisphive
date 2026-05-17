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

<!-- Populated by /sprint-review after /blitz runs. -->

## Demo

<!-- Populated by /sprint-review. -->

## Retro

<!-- Populated by /sprint-review. -->
