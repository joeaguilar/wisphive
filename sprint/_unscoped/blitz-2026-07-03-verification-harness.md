# Blitz — Sprint 0 verification harness (epic itr#413)

## Config
- Tracker: itr (`itr list --tag verification-harness` / `itr close <ID> "reason"`)
- Dep graph: kgr (clean: no cycles, no rule violations)
- Verify gate: `gatr run --tag blitz-gate -- bash -c 'cargo test --workspace && cargo clippy --workspace -- -D warnings && cargo fmt --all --check && cd crates/wisphive_web/frontend && npm run lint && npm test'`
- Concurrency: 5 (largest wave is 3)
- Repos: . (wisphive)
- Stop: backlog empty | 2 no-progress waves
- Reserved for orchestrator (spec lane, off-limits to wave agents): `docs/plan-*.md`, `docs/decisions/`

## Waves

### Wave 1
| Task | Title | Owns |
|---|---|---|
| #414 | Scaffold Playwright e2e infrastructure | `crates/wisphive_web/frontend/**` (incl. package.json, package-lock.json, playwright config, e2e/helpers), `justfile` (e2e recipe only) |
| #418 | Ratatui TestBackend snapshot harness | `crates/wisphive_tui/**` (incl. its Cargo.toml), `Cargo.lock` (dev-dep add) |
| #420 | Human smoke-checklist convention | `docs/smoke/**`, `CLAUDE.md` (smoke-convention reference only) |

### Wave 2 (after #414 closes)
| Task | Title | Owns |
|---|---|---|
| #415 | Core web flow specs | `e2e/fixtures/**` (socket decision-injection fixture), `e2e/specs/core-flows*` |
| #416 | WebAuthn virtual-authenticator spec | `e2e/specs/passkey*`, `crates/wisphive_web/src/passkey.rs` (only if needed) |
| #417 | TLS/wss regression spec | `e2e/specs/tls*`, own helper e.g. `e2e/helpers/tls-server.ts`, `crates/wisphive_web/src/tls.rs` (only if needed) |

### Wave 3 (after #414 + #418 close)
| Task | Title | Owns |
|---|---|---|
| #419 | `just verify` + gatr wiring | `justfile`, `CLAUDE.md` (build/test section) |

## File conflicts
- `crates/wisphive_web/frontend` declared by #414/415/416/417 → resolved by wave split (#414 first) plus intra-dir ownership in Wave 2 (disjoint spec/helper files).
- `justfile` declared by #414 and #419 → serialized (Wave 1 vs Wave 3) via existing blocker.
- `CLAUDE.md` touched by #420 (Wave 1) and #419 (Wave 3) → serialized; #420 limited to the smoke-convention reference, #419 to the build/test section.

## Semantic warnings
- Wave 2 agents must NOT edit `playwright.config.ts`, `package.json`, `package-lock.json`, or #414's shared boot helper. Per-spec needs (ignoreHTTPSErrors, launch args) go in their own spec/helper files via `test.use(...)`; needed shared changes are reported, not applied.
- No agent touches `docs/plan-*.md` or `docs/decisions/` (orchestrator spec lane in flight).
- No write-mode formatters (cargo fmt, prettier --write) per blitz house rules.
- e2e must isolate state via `HOME=<tempdir>` — never read/write the real `~/.wisphive` (a live daemon gates this very session).

## Interventions
- Mid-Wave-1 rust-analyzer diagnostics appeared in `server.rs`/`config.rs`/`hooks.rs` (files no agent owns). `git status` audit confirmed no out-of-ownership edits — phantom re-index noise from #418's Cargo.toml/Cargo.lock dev-dep change. No action taken; wave gate is authoritative.

## Outcomes
### Wave 1
- #420 closed — docs/smoke/CHECKLIST.md convention + seed items; gate green (gatr blitz-gate exit=0, 53 vitest passed).
- #418 closed — TestBackend snapshot harness, 20 tests / 7 snapshots; enforcing status-bar completeness fixed real keybinding gaps in ui.rs; gate green (gatr exit=0, 21.7s).
- #414 closed — Playwright infra (config, temp-HOME boot helper, first-run smoke spec w/ screenshots, `just e2e` recipe, frontend README docs); gate green ×3 + `just e2e` 1 passed. Notes: transient fmt drift from #418's in-flight file self-healed; pre-existing dev-only npm audit findings (vite dev server) predate the change — `npm audit --omit=dev` clean.

Wave 1 gate (orchestrator): gatr wave1-gate exit=0, 21.1s. No quarantines. Wave 2 launched (#415, #416, #417).

### Wave 2
- #417 closed — e2e/tls.spec.ts: https-only subresources + zero mixed-content/CSP console errors, wss:///ws (token redacted in failure output), h2 via nextHopProtocol on nav + /api, :authority guard on unauth + bearer requests. No h2 finding — axum_server rustls ALPN negotiates h2. Gate green (20.7s), spec 1 passed.
- #416 closed — e2e/passkey.spec.ts: enroll via CDP CTAP2 virtual authenticator → sign out → discoverable passkey login → rotated token + counter verified; screenshot artifact; human-only matrix documented in spec header. Gate green (20.9s), spec 1 passed. Finding: webauthn-rs residentKey:'discouraged' empirically enrolls unusable credentials on hint-honoring authenticators → filed itr#427 (related to #321); spec carries a page.route workaround until fixed.
- #415 closed — e2e/fixtures/{daemon-server,hook-client}.ts + core-flows.spec.ts (4 specs): login invalid/valid, fixture decision → UI approve resolves allow, deny+message round-trips reason, devices revoke → 401. Architecture finding: standalone `web serve` has no decision queue/socket — specs boot `daemon start --web`; notification binaries stubbed via PATH so fixture decisions don't pop real banners. Deferred coverage filed as itr#428 (reauth modal; devices UI after #220; AskUserQuestion with #250).

Wave 2 gate (orchestrator): gatr wave2-gate exit=0 + full combined e2e suite wave2-e2e: 7/7 passed, 7.6s. No quarantines. Wave 3 launched (#419).

### Wave 3
- #419 closed — `just verify`: five gatr-tagged sub-gates (verify-fmt 0.2s, verify-clippy 0.2s, verify-rust 17.4s incl. TUI snapshots, verify-frontend 3.1s, verify-e2e 10.3s), fail-fast verified with an exit-code probe; CLAUDE.md Build & Test documents the tags and evidence contract. ~31s warm.

### Final
- Final orchestrator gate: `just verify` exit=0, 7/7 e2e (8.0s). Backlog empty — blitz complete, 7/7 closed, 0 quarantined, 0 failed-skipped.
- Epic #413 closed. Issues filed during run: #425 (~/.wisphive default-deny, under #403), #426 (decided_by bulk granularity), #427 (webauthn residentKey bug), #428 (deferred e2e coverage).
- Nothing committed — working tree awaiting PO review.

### Orchestrator spec lane (parallel, itr#421–424)
- #422 closed — Security Invariants I1–I10 in plan-policy-learning-engine.md + ADR-0005; follow-ups #425 (~/.wisphive default-deny, parented under #403) and #426 (decided_by bulk granularity).
- #423 closed — Trust Model T1–T7 in plan-decision-plugins.md + ADR-0006.
- #424 closed — Semantics S1–S6 + TOCTOU analysis in plan-cross-agent-conflict-gate.md.
- #421 closed — docs/plan-loop-supervisor.md created + ADR-0007 (fail toward stop).
