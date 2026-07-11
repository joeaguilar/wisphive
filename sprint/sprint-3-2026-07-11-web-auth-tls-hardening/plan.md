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

## Spillover → Product Backlog
None. Every candidate serves the goal and fits ≤C2. The adjacent C3/C4 security work (#319, #427, #321, #364, #493, #223, #270, #220) stays out per Non-Goals and was never in this set.

## Open Assumptions
- **#246** lands the *configurable* backoff-cap knob only; tuning it to a measured value is a declared Non-Goal. Confirm its AC reads "make configurable," not "raise to N" — if it demands a tuned value, it is under-specified and should defer.
- **#492** fixing the Vite dev-server advisories may force a Vite major bump → build/HMR risk. If the bump breaks `frontend-build`, split the upgrade to a follow-up rather than stalling the sprint.
- **#275** is the one story touching `useWisphive.ts` (frontend hook) rather than pure Rust — the Sonnet lane owns it, but closure needs the Vitest matrix green, not just `cargo`.
- **Filing note:** the 22 stories were re-parented from the existing backlog (not created fresh), so their created-dates predate this sprint and their grooming tags (`complexity:`, `route:`) are intact.

## Outcomes
<!-- Populated by /sprint-review after /blitz runs. -->

## Demo
<!-- Populated by /sprint-review. -->

## Retro
<!-- Populated by /sprint-review. -->
