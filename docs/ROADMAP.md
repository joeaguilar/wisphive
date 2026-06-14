# Roadmap - Wisphive

_Last updated: 2026-05-30 (manual $roadmap baseline)_
_Last reviewed: 2026-06-14_

> Cross-sprint product map. Bridges the repo's product docs, sprint history, and live `itr` backlog.
> Read at the start of every `/sprint`. Update at the end of every `/sprint-review`.

## Status legend

- ✅ - section is feature-complete for the current release boundary
- 🟡 - partial; some shipped scope exists, but open work remains
- ❌ - not started or only planned in docs/issues

Cells with a trailing `<!-- auto -->` marker are agent-owned and refreshed by `/roadmap --update`.
Cells without the marker are PO-edited and should be preserved verbatim.

## Source baseline

- Product overview: `README.md`, `AGENTS.md`
- Planning docs: `docs/plan-mobile-device-pairing.md`, `docs/plan-cross-agent-conflict-gate.md`, `docs/plan-decision-plugins.md`, `docs/plan-policy-learning-engine.md`, `docs/plan-red-support.md`, `docs/open-source-path.md`
- Sprint evidence: `sprint/sprint-1-2026-05-16-auth-profile-passkey-localLAN/plan.md`
- Backlog evidence: `itr` snapshot on 2026-05-30 (`355` total, `170` done, `184` open, `1` wontfix)

## Release boundary

**v1 ships when:**
- Core hook, daemon, TUI, web UI, CLI, and agent-launch flows are stable enough for daily agent supervision. <!-- auto -->
- Web auth, TLS, first-run onboarding, desktop passkeys, Devices UI, and mobile pairing are complete. <!-- auto -->
- Project discovery/audit/seeding has at least CLI support and one UI surface. <!-- auto -->
- Critical security, CI, dependency-audit, and retention risks are resolved or explicitly waived. <!-- auto -->
- Chrome/Firefox/Brave LocalLAN smoke and Enterprise smoke have documented pass/fail outcomes. <!-- auto -->

**v2 scope (tracked, deferred):**
- §C.4 Cross-agent conflict gate - planned, not required for the first release boundary. <!-- auto -->
- §D.2 Policy learning engine - powerful but safety-sensitive; defer until core policy rules are mature. <!-- auto -->
- §D.3 Decision plugins, webhooks, and richer adapters - extension surface after the core product stabilizes. <!-- auto -->
- §D.4 Red / Local LLM / post-MVP adapters - adapter expansion after the primary Codex/Claude path is solid. <!-- auto -->

**Excluded (never):**
- None currently. <!-- auto -->

## Sections - v1

### §A - Core Control Plane

| Section | Status | Size | Linked itr | Notes |
|---------|--------|------|------------|-------|
| §A.1 Hook IPC and decision protocol | 🟡 <!-- auto --> | M <!-- auto --> | itr#25, itr#30, itr#251, itr#252, itr#253, itr#254 <!-- auto --> | Rich decisions, PermissionRequest schema verification, and hook-event dispatch hardening shipped; ExitPlanMode failure fallback remains. Decisions: ADR-0001 (tiered fail posture), ADR-0002 (always-defer classification). <!-- auto --> |
| §A.2 Daemon state, queue, persistence, and audit log | 🟡 <!-- auto --> | L <!-- auto --> | itr#31, itr#88, itr#89, itr#297, itr#298, itr#299, itr#300, itr#301, itr#302, itr#332, itr#333, itr#334, itr#335, itr#336 <!-- auto --> | Core queue/persistence exists; audit binding, retention, ingest, stale-socket, and storage durability gaps remain. <!-- auto --> |
| §A.3 TUI review dashboard | 🟡 <!-- auto --> | M <!-- auto --> | itr#12, itr#32, itr#36, itr#56, itr#127, itr#220 <!-- auto --> | Main review UX exists; navigation polish and web-auth event surfacing remain. <!-- auto --> |
| §A.4 Web review UI and WebSocket bridge | 🟡 <!-- auto --> | L <!-- auto --> | itr#40, itr#41, itr#44, itr#45, itr#52, itr#104, itr#105, itr#106, itr#108, itr#109, itr#110, itr#111, itr#112, itr#113, itr#114, itr#115, itr#116, itr#204, itr#205, itr#206, itr#207, itr#208, itr#240, itr#241, itr#295, itr#296 <!-- auto --> | Functional queue/history/spawn UX exists; reconnect, type-safety, logs live-tail, terminals, and accessibility are still partial. <!-- auto --> |
| §A.5 CLI, hook install/status, doctor, and agent launch | 🟡 <!-- auto --> | M <!-- auto --> | itr#15, itr#65, itr#294, itr#306, itr#307, itr#308, itr#348 <!-- auto --> | CLI surface is broad; daemon handshake, invalid host/config handling, hook install edge cases, and sentinel port behavior remain. <!-- auto --> |

### §B - Auth, Web Security, and Onboarding

| Section | Status | Size | Linked itr | Notes |
|---------|--------|------|------------|-------|
| §B.1 TLS, web auth, device bearer tokens, and origin/host gates | 🟡 <!-- auto --> | L <!-- auto --> | itr#78, itr#79, itr#80, itr#209, itr#210, itr#211, itr#212, itr#213, itr#214, itr#215, itr#224, itr#225, itr#226, itr#227, itr#228, itr#229, itr#270, itr#274, itr#278, itr#279, itr#280, itr#317 <!-- auto --> | Password/device-token auth and self-signed TLS shipped; LAN SAN filtering, user cert flags, and several auth hardening tails remain. Decisions: ADR-0003 (enterprise profile non-functional until user-cert TLS / itr#270). <!-- auto --> |
| §B.2 First-run onboarding and desktop passkeys | 🟡 <!-- auto --> | M <!-- auto --> | itr#267, itr#268, itr#269, itr#310, itr#311, itr#312, itr#315, itr#316, itr#319, itr#321, itr#323 <!-- auto --> | Sprint-1 shipped Chrome LocalLAN happy path; Firefox/Brave LocalLAN matrix, Enterprise smoke, and passkey follow-ups remain. <!-- auto --> |
| §B.3 Device management and enterprise enrollment | 🟡 <!-- auto --> | L <!-- auto --> | itr#220, itr#257, itr#313 <!-- auto --> | Revoke hardening exists; Devices UI, passkey list/remove, and Enterprise enroll flow still need delivery. <!-- auto --> |
| §B.4 Mobile phone pairing workflow | ❌ <!-- auto --> | XL <!-- auto --> | itr#266, itr#270, itr#271, itr#272, itr#283, itr#288, itr#289 <!-- auto --> | Planned in detail, but the LAN cert, pairing listener, QR/mobile route, and privacy notice chain is not shipped. <!-- auto --> |

### §C - Agent and Project Operations

| Section | Status | Size | Linked itr | Notes |
|---------|--------|------|------------|-------|
| §C.1 Agent spawning and terminal sessions | 🟡 <!-- auto --> | M <!-- auto --> | itr#15, itr#22, itr#84, itr#303, itr#305 <!-- auto --> | Agent spawn and terminal basics exist; output streaming, terminal close semantics, and replay direction handling remain. <!-- auto --> |
| §C.2 Server-authoritative terminal scrollback | ❌ <!-- auto --> | L <!-- auto --> | itr#284, itr#285, itr#286, itr#287, itr#288, itr#289 <!-- auto --> | Epic is defined; protocol field, daemon bounded-tail replay, frontend seq tracking, and privacy copy remain. <!-- auto --> |
| §C.3 Project discovery, AI-config audit, seeding, and config sharing | ❌ <!-- auto --> | XL <!-- auto --> | itr#349, itr#350, itr#351, itr#352, itr#353, itr#354, itr#355 <!-- auto --> | New product direction is filed; discovery core is the first unblocked child. <!-- auto --> |

## Sections - v2 (tracked, deferred)

| Section | Status | Size | Linked itr | Notes |
|---------|--------|------|------------|-------|
| §C.4 Cross-agent conflict gate | ❌ <!-- auto --> | L <!-- auto --> | docs/plan-cross-agent-conflict-gate.md <!-- auto --> | Design exists; no active `itr` implementation chain selected for v1. <!-- auto --> |
| §D.2 Policy learning engine | ❌ <!-- auto --> | XL <!-- auto --> | docs/plan-policy-learning-engine.md <!-- auto --> | Detailed safety design exists; not part of the first release boundary. <!-- auto --> |
| §D.3 Decision plugins, webhooks, and richer adapters | ❌ <!-- auto --> | XL <!-- auto --> | docs/plan-decision-plugins.md <!-- auto --> | Extension architecture is planned; defer until base control plane and policy safety are mature. <!-- auto --> |
| §D.4 Red / Local LLM / post-MVP adapters | ❌ <!-- auto --> | L <!-- auto --> | itr#4, itr#5, itr#7, itr#76, docs/plan-red-support.md <!-- auto --> | Red/LocalLLM adapter work is tracked as post-MVP; current hook-based Codex/Claude path remains primary. <!-- auto --> |

## Sections - Hardening and Release

### §D - Policy and Extensibility

| Section | Status | Size | Linked itr | Notes |
|---------|--------|------|------------|-------|
| §D.1 Auto-approve policy engine and tool-rule safety | 🟡 <!-- auto --> | M <!-- auto --> | itr#121, itr#129, itr#308 <!-- auto --> | Content-aware rules exist; duplicated tool lists, config parsing posture, and hook config performance remain. Decisions: ADR-0002 (always-defer classification / itr#380). <!-- auto --> |

### §E - Hardening, Quality, and Release

| Section | Status | Size | Linked itr | Notes |
|---------|--------|------|------------|-------|
| §E.1 Security hardening backlog | 🟡 <!-- auto --> | XL <!-- auto --> | itr#81, itr#82, itr#83, itr#84, itr#85, itr#86, itr#88, itr#89, itr#90, itr#91, itr#92, itr#93, itr#94, itr#95, itr#96, itr#97, itr#98, itr#99, itr#101, itr#102, itr#262, itr#330, itr#344, itr#345, itr#346 <!-- auto --> | Many critical web/hook fixes are done, including the cargo-deny advisory gate; daemon IPC, audit, config, DoS, and hook edge hardening remain broad. <!-- auto --> |
| §E.2 Frontend reliability, accessibility, and type safety | 🟡 <!-- auto --> | L <!-- auto --> | itr#104, itr#105, itr#106, itr#107, itr#108, itr#109, itr#110, itr#111, itr#112, itr#113, itr#114, itr#115, itr#116, itr#117, itr#118, itr#120, itr#273, itr#274, itr#275, itr#276, itr#295, itr#296, itr#309 <!-- auto --> | Some refactors landed, but the web UI still has reliability, a11y, reducer side-effect, terminal, and bundle-size work. <!-- auto --> |
| §E.3 Test, CI, dependency audit, and cargo-deny health | 🟡 <!-- auto --> | L <!-- auto --> | itr#61, itr#77, itr#115, itr#248, itr#330 <!-- auto --> | Local CI reproduction and cargo-deny advisory gate are green; frontend test coverage and future audit CI integration remain. <!-- auto --> |
| §E.4 Open-source release readiness | 🟡 <!-- auto --> | M <!-- auto --> | itr#55, docs/open-source-path.md <!-- auto --> | Some OSS blockers have landed, but the release-readiness checklist is still open. <!-- auto --> |
| §E.5 Sprint process and quality gates | 🟡 <!-- auto --> | S <!-- auto --> | itr#324, itr#325, itr#326, itr#327, itr#328, itr#329 <!-- auto --> | Sprint-1 retro produced concrete process follow-ups; none are closed yet. <!-- auto --> |

## Cross-cutting

**Wide dependencies (early-land candidates):**
- §A.1 Hook IPC and decision protocol - consumed by hooks, daemon, TUI, web, terminals, logs, and future plugins. <!-- auto -->
- §A.2 Daemon state, queue, persistence, and audit log - the shared runtime base for almost every feature. <!-- auto -->
- §B.1 TLS, web auth, device bearer tokens, and origin/host gates - gates onboarding, Devices UI, Enterprise, and mobile pairing. <!-- auto -->
- §C.3 Project discovery, AI-config audit, seeding, and config sharing - discovery core gates CLI, TUI, and web project onboarding. <!-- auto -->
- §D.1 Auto-approve policy engine and tool-rule safety - safety base for policy learning, plugins, and wider automation. <!-- auto -->

**Inter-section edges:**
- §B.2 depends on §B.1 for AuthProfile, TLS/auth primitives, and origin-aware profile lookup. <!-- auto -->
- §B.3 depends on §B.1 and §B.2 for device bearer/passkey state and profile-aware passkey behavior. <!-- auto -->
- §B.4 depends on §B.1, §B.2, and §B.3 for LAN cert/user cert support, profile gating, and Devices UI entry points. <!-- auto -->
- §C.2 depends on §A.1 and §A.2 for protocol changes and persisted terminal event replay. <!-- auto -->
- §C.3 depends on §A.5 for CLI integration and feeds §A.3/§A.4 once surfaced in TUI/web. <!-- auto -->
- §D.2 and §D.3 depend on §D.1 so automation expands from a safer policy base. <!-- auto -->
- §E.3 is a release gate for all v1 sections. <!-- auto -->

## Trajectory

No trajectory drafted in this baseline. `/sprint` should derive the next sprint from the current `itr` state, with wide dependencies and release blockers surfaced first.

## Stub filing

No roadmap stubs were filed in this baseline. The existing `itr` backlog already covers the approved roadmap rows; future `/roadmap --update` runs can file selective stubs if a row has no issue coverage.

## Update cadence

- Read at the start of every `/sprint` to inform Sprint Goal selection.
- Update at the end of every `/sprint-review`.
- Re-run `/roadmap` manually when scope changes, major `itr` epics are added, or the v1/v2 boundary changes.
