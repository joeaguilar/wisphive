# Wisphive Roadmap

Prepared by Codex on 2026-05-15.

This roadmap is a stakeholder-facing synthesis of the current repository state, `itr`
tracker state, and the active planning documents under `docs/`. It was written by
Codex to make the project's direction legible without requiring readers to inspect
the issue tracker or implementation plans directly.

## Product Direction

Wisphive is a multiplexed AI agent control plane. Its core purpose is to put a
human-reviewed decision layer between AI agents and powerful local tools, while
preserving enough automation for day-to-day agent work to stay fast.

The project is currently moving from a working local control plane toward a
multi-device, security-hardened operator experience:

1. Make first-run web onboarding smooth.
2. Let an operator pair a phone and approve or deny agent actions from the same
   live queue as the desktop TUI.
3. Harden the web, hook, daemon, and IPC surfaces before exposing LAN-facing
   pairing flows.
4. Prepare the repository for a credible public/open-source release.
5. Expand into richer policy automation, cross-agent coordination, and adapter
   support after the mobile/security foundation is stable.

## Current State

The core architecture is in place:

- `wisphive-hook` gates Codex tool calls through the daemon.
- `wisphive_daemon` owns decisions, persistence, process registry, terminal
  sessions, notifications, and web/TUI fan-out.
- `wisphive_tui` provides the local approval dashboard.
- `wisphive_web` provides a browser UI, WebSocket bridge, TLS/auth primitives,
  device-token storage, and passkey database scaffolding.
- Runtime state lives under `~/.wisphive/`.

Recent foundations already merged include:

- Self-signed TLS cert management with rotation and fingerprint support.
- Per-device bearer tokens, password hashing, login throttling, and device
  persistence.
- `POST /api/auth/set-password` for first-run web setup (`itr#268`).
- Browser auto-open on first web start.
- WebAuthn schema support in daemon state.

Tracker status at the time this document was written:

- Total issues: 309
- Done: 144
- Open: 162
- In progress: 2
- Ready: 150
- Blocked: 14

Current in-progress items:

- `itr#238`: daemon logging with rolling file appender and in-memory ring buffer.
  Notes indicate implementation and review follow-ups have landed and the issue
  is ready to close.
- `itr#269`: frontend `/onboarding` route for first-run password setup and
  optional desktop passkey registration.

## Near-Term Milestone: Web Onboarding and Mobile Pairing

The active product milestone is mobile device pairing, tracked by `itr#283` and
`docs/plan-mobile-device-pairing.md`.

Goal: let a logged-in operator pair a phone with Wisphive in under a minute. The
phone runs the same web UI, authenticates as a first-class device, and approves
or denies decisions from the same queue the desktop TUI sees.

### Stage 1: First-Run Web Onboarding

Status: partially complete.

- Done: `itr#268` web-facing set-password endpoint.
- In progress: `itr#269` frontend `/onboarding` route.
- Expected outcome: a fresh user can launch web mode, set the initial password
  from the browser, optionally enroll a desktop passkey, and land on the
  dashboard.

### Stage 2: Mobile Pairing Critical Path

Status: not started beyond foundational backend/auth/TLS work.

Strict order:

1. `itr#227`: filter TLS SAN/LAN URL enumeration to avoid Docker, VPN, utun,
   Tailscale, and other unstable interfaces.
2. `itr#270`: include LAN IP in self-signed cert SANs and add user-provided
   `--tls-cert` / `--tls-key` support.
3. `itr#271`: pairing token module, ephemeral LAN listener, and `DevicePaired`
   WebSocket event.
4. `itr#272`: desktop QR flow and mobile `/pair` route.

Required parallel work:

- `itr#219`: WebAuthn passkey register/login handlers and frontend support.

Locked design decisions for v1:

- Each device enrolls its own per-origin passkey. Desktop and phone credentials
  are separate rows.
- Chrome, Firefox, and Brave are in scope. Safari and iOS Safari are out of
  scope for v1.
- The primary web listener and ephemeral pairing listener share the same TLS
  certificate.
- Only one pairing token can be armed at a time.
- `/api/pair/arm` should require fresh sudo-style reauthentication.
- The ephemeral pairing listener binds to the primary LAN IP only, never
  `0.0.0.0`.

### Security Prerequisites Before LAN Exposure

These should close before shipping production LAN pairing:

- `itr#79`: restrict dev-mode CORS; do not allow `Any` on config endpoints.
- `itr#80`: address exploitable hook fail-open behavior.
- `itr#257`: sudo-gate device revoke and add rate limiting.

Other high-priority security and hardening issues remain, including IPC socket
permissions, bounded socket reads, unbounded channel replacement, secret
redaction, config auth/body validation, XSS removal, and terminal command
approval.

## Open-Source Readiness

Open-source readiness is tracked by `itr#55` and `docs/open-source-path.md`.

The readiness document is partly stale. The repository now has:

- `LICENSE`
- `CONTRIBUTING.md`
- `.github/workflows/ci.yml`
- Real GitHub repository metadata in `Cargo.toml` and `README.md`
- `.itr.db` ignored in `.gitignore`

Remaining public-release work:

- Remove `.claude/settings.json` from git and add `.claude/` to `.gitignore`.
- Add `CODE_OF_CONDUCT.md`.
- Add `SECURITY.md`.
- Review git history for sensitive content.
- Decide whether issue and pull request templates are needed for the first
  public push.
- Decide whether to add `cargo deny` / `cargo audit` to CI before release.

## Mid-Term Roadmap

After web onboarding and mobile pairing, the next meaningful workstreams are:

### Cross-Agent Conflict Gate

Tracked by `docs/plan-cross-agent-conflict-gate.md`.

Purpose: detect when multiple agents attempt to edit the same file and surface
conflicts before humans approve conflicting writes.

Planned phases:

- `FileConflictMap` with tests.
- Protocol types for conflict info.
- Server-side enqueue/claim/release wiring.
- Config flags for detection mode and TTL.
- TUI conflict display and claims panel.
- Web conflict indicators and claims dashboard.

### Decision Plugins and Agent Bridge

Tracked by `docs/plan-decision-plugins.md`.

Purpose: make policy decisions more expressive and prepare for richer agent
integrations.

Planned phases:

- Extended hook policy rules with regex and path globs.
- Decision webhooks and shell hooks.
- Config hot reload.
- RPC bridge for workspace agents.
- Agent kind support in CLI/TUI spawn flows.
- Web configuration for spawn type and webhooks.

### Policy Learning Engine

Tracked by `docs/plan-policy-learning-engine.md`.

Purpose: learn from historical approvals and denials, suggest safe automation
rules, and eventually support opt-in auto-apply for high-confidence clean
patterns.

Recommended rollout:

- Ship suggestion mode first.
- Let users review and accept learned rules.
- Defer automatic application until the suggestion model has real-world feedback.

## Longer-Term Themes

These are present in the tracker but should follow the mobile/security/open-source
foundation:

- Server-authoritative terminal scrollback for multi-device use (`itr#284`).
- Red and local LLM adapter support.
- Agent dispatch and wave planning.
- NixOS sandbox bridge.
- Knowledge ledger.
- Public distribution improvements such as crates.io, Homebrew, and release
  binaries.

## Suggested Next Actions

1. Close `itr#238` if the logged implementation is already merged and verified.
2. Finish `itr#269` so first-run web onboarding is complete.
3. Fix open-source hygiene drift: remove tracked `.claude/settings.json`, ignore
   `.claude/`, and add `SECURITY.md`.
4. Start the mobile pairing chain with `itr#227`.
5. Schedule `itr#79`, `itr#80`, and `itr#257` alongside the pairing work so LAN
   exposure is not blocked at the finish line.

## Sources

- `itr` tracker state as of 2026-05-15.
- `docs/plan-mobile-device-pairing.md`
- `docs/open-source-path.md`
- `docs/plan-cross-agent-conflict-gate.md`
- `docs/plan-decision-plugins.md`
- `docs/plan-policy-learning-engine.md`
- `AGENTS.md`
