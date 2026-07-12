# Roadmap - Wisphive

_Last updated: 2026-07-04 (Sprint-2 closed: Command Center inbox #399 shipped; Phase 3 now partial)_
_Last reviewed: 2026-07-04_

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
- **Steering spec (active program): `docs/command-center-spec.md`** + evidence base `docs/command-center-notes.md`
- Planning docs: `docs/plan-mobile-device-pairing.md`, `docs/plan-cross-agent-conflict-gate.md`, `docs/plan-decision-plugins.md`, `docs/plan-policy-learning-engine.md`, `docs/plan-deterministic-agent-analytics.md`, `docs/plan-red-support.md`, `docs/open-source-path.md`
- Sprint evidence: `sprint/sprint-1-2026-05-16-auth-profile-passkey-localLAN/plan.md`
- Backlog evidence: `itr` snapshot on 2026-07-02 (`406` total, `196` done, `209` open, `1` wontfix) — rectified: every open issue now lives under a phase epic below, or is explicitly deferred/continuous

## Program order — sequenced epics (adopted 2026-07-02)

The Command Center steering spec (`docs/command-center-spec.md`) is the active program. The full open
backlog was rectified into the phase epics below; work the phases **in order**. Phases 1→3 are strictly
sequential (each builds on trust the previous one establishes). Phases 4–5 follow the spec's priorities.
Phases 6–9 are the pre-existing v1 backlog, re-sequenced beneath the program; they may interleave
opportunistically, but ship in this order when forced to choose. `itr get <epic-id>` lists members;
hard technical dependencies are encoded as `itr` blocker edges, program order lives here.

| Phase | Epic | Tracker | Pri | Scope (members) | Hard deps |
|---|---|---|---|---|---|
| **1** | ✅ Decision-plane trust — Command Center P0 | wisphive **#396** (closed 2026-07-03) | critical | Five silent-weakening config bugs #358 #360 #361 #366 #308 + auto-answer audit trail #397 — all closed with evidence; spec §4 red-team check passed (see itr#396 notes + docs/handoff/2026-07-03-decision-plane-trust-phase1.md). Follow-ups: #407 #408 #409. | — |
| **2** | Decision-plane integrity — P0.5 | wisphive **#403** | high | Audit correctness & durability (#363 ghost approvals, #370 dup-id corruption, ~~#301/#302 ingest loss~~ ✅ done 2026-07-03, #368 fsync, #88 resolver identity, #347), secret redaction #89, hook fail-safety (#344 #345 #346 #337 #359), pending-decision persistence (#297–#300). Can start alongside Phase 1; **blocks the inbox (#399)**. | — |
| **3** | 🟡 Command Center Layer 1 — live ops console | wisphive **#398** | high | **✅ Waiting-on-you inbox #399 (centerpiece) — shipped Sprint-2 (2026-07-04), real-daemon+hook e2e evidence.** Remaining: agent liveness board #400, working-tree strip #401, burn meter #402. Answer-path correctness #249 #250 #253 ✅ done. Sprint-2 follow-ups: #439 #440 #441 #449–#453. | #396; #399 also blocked by #403, #249, #250, #253, #397 |
| **4** | Command Center Layer 2 — durable state of play | **werkit#5** (stories werkit#6 #7 #8) | high | State-of-play Stop-hook + start render, cross-project re-entry digest, promise ledger. Daemon-independent renderer; do not ship the Stop hook until Phase 1 lands (spec §6.1). | wisphive #396 (hook safety) |
| **5** | Remote access — scrollback + mobile pairing | wisphive **#284** → **#283** | high | Scrollback replay chain #285–#287 (+privacy #288 #289), then pairing: #266 (#270 #271 #272), enterprise enroll #313, Devices UI #220, smoke #316. The inbox-on-the-phone is the payoff (85% of sessions are remote-triggered). | #283 blocked by #284, #227, #270–#272, #313 |
| **6** | Project discovery, audit & seeding | wisphive **#349** | high | #353 #354 #355 — onboarding more projects into gating; feeds the command center's active-project list. | — |
| **7** | Client reliability & UX debt | wisphive **#404** | medium | 43 web/TUI issues: reconnect+rehydration, TS type-safety, React correctness, a11y, TUI panics/scrolling, terminal views, logs tail. | — |
| **8** | Security & correctness hardening tail | wisphive **#405** | medium | 67 issues: daemon DoS caps, spawn/terminal gating, file perms, hash-chain audit #93, web auth/TLS follow-ups, logging hygiene, CLI bugs, #364 #365 #389. OSS release gate. | — |
| **9** | Open-source release readiness | wisphive **#55** | high | LICENSE, repo hygiene, README/docs #67 #72; ships only after Phase 8 is green or explicitly waived. | Phase 8 (policy, not encoded) |
| **∞** | Code health (opportunistic) | wisphive **#406** | low | Refactors/duplication/perf (#124 #126 #127 #132 …). Pull items in when touching the area; never a dedicated sprint. **Open PO decision: #125 (delete adapters crate) contradicts #4/#5 (implement adapters).** | — |
| — | Deferred (v2) | — | — | Deterministic analytics #390–#395 (near-term slices superseded by #397/#402/werkit#6 — re-plan #390 after Phase 3), conflict gate, policy learning, decision plugins, Red/LocalLLM adapters + post-MVP #1 #2 #4–#8, loop console (spec §7). | all of the above |

Continuous (not phased): sprint-process improvements #324–#329 — apply at each `/sprint`/`/blitz`.

## Release boundary

**v1 ships when:**
- Command Center Phase 0 trust (itr#396) + integrity (itr#403) close their red-team exit criteria, and Layer 1 (itr#398) is live against real sessions. <!-- auto -->
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
- §D.5 Deterministic agent analytics - planned; fact extraction, work journals, risk digest, dashboards, and overlap analytics land before generated summaries or policy automation. Decisions: ADR-0004. <!-- auto -->

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
| §C.1 Agent spawning and terminal sessions | 🟡 <!-- auto --> | M <!-- auto --> | itr#15, itr#22, itr#84, itr#303, itr#305 <!-- auto --> | Agent spawn and terminal basics exist; terminal close now exposes one honest platform-defined behavior, while output streaming and replay direction handling remain. <!-- auto --> |
| §C.2 Server-authoritative terminal scrollback | ❌ <!-- auto --> | L <!-- auto --> | itr#284, itr#285, itr#286, itr#287, itr#288, itr#289 <!-- auto --> | Epic is defined; protocol field, daemon bounded-tail replay, frontend seq tracking, and privacy copy remain. <!-- auto --> |
| §C.3 Project discovery, AI-config audit, seeding, and config sharing | ❌ <!-- auto --> | XL <!-- auto --> | itr#349, itr#350, itr#351, itr#352, itr#353, itr#354, itr#355 <!-- auto --> | New product direction is filed; discovery core is the first unblocked child. <!-- auto --> |

## Sections - v2 (tracked, deferred)

| Section | Status | Size | Linked itr | Notes |
|---------|--------|------|------------|-------|
| §C.4 Cross-agent conflict gate | ❌ <!-- auto --> | L <!-- auto --> | docs/plan-cross-agent-conflict-gate.md <!-- auto --> | Design exists; no active `itr` implementation chain selected for v1. <!-- auto --> |
| §D.2 Policy learning engine | ❌ <!-- auto --> | XL <!-- auto --> | docs/plan-policy-learning-engine.md <!-- auto --> | Detailed safety design exists; not part of the first release boundary. <!-- auto --> |
| §D.3 Decision plugins, webhooks, and richer adapters | ❌ <!-- auto --> | XL <!-- auto --> | docs/plan-decision-plugins.md <!-- auto --> | Extension architecture is planned; defer until base control plane and policy safety are mature. <!-- auto --> |
| §D.4 Red / Local LLM / post-MVP adapters | ❌ <!-- auto --> | L <!-- auto --> | itr#4, itr#5, itr#7, itr#76, docs/plan-red-support.md <!-- auto --> | Red/LocalLLM adapter work is tracked as post-MVP; current hook-based Codex/Claude path remains primary. <!-- auto --> |
| §D.5 Deterministic agent analytics and work journal | ❌ <!-- auto --> | XL <!-- auto --> | itr#390, itr#391, itr#392, itr#393, itr#394, itr#395, docs/plan-deterministic-agent-analytics.md <!-- auto --> | Near-term slices superseded by the Command Center program (audit trail itr#397 ⊂ #391's substrate; burn meter itr#402 overlaps #394; werkit#6 state-of-play overlaps #392). Re-plan itr#390 after Layer 1 ships so remaining scope builds on the audit-trail substrate. Decisions: ADR-0004. <!-- auto --> |

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
- §D.5 Deterministic agent analytics and work journal - shared fact substrate for summaries, risk review, dashboards, overlap reports, and future policy-learning evidence. <!-- auto -->

**Inter-section edges:**
- §B.2 depends on §B.1 for AuthProfile, TLS/auth primitives, and origin-aware profile lookup. <!-- auto -->
- §B.3 depends on §B.1 and §B.2 for device bearer/passkey state and profile-aware passkey behavior. <!-- auto -->
- §B.4 depends on §B.1, §B.2, and §B.3 for LAN cert/user cert support, profile gating, and Devices UI entry points. <!-- auto -->
- §C.2 depends on §A.1 and §A.2 for protocol changes and persisted terminal event replay. <!-- auto -->
- §C.3 depends on §A.5 for CLI integration and feeds §A.3/§A.4 once surfaced in TUI/web. <!-- auto -->
- §D.2 and §D.3 depend on §D.1 so automation expands from a safer policy base. <!-- auto -->
- §D.5 depends on §A.2 for durable audit/history data and feeds §C.4 historical overlap evidence plus §D.2 policy-learning evidence. Decisions: ADR-0004. <!-- auto -->
- §E.3 is a release gate for all v1 sections. <!-- auto -->

## Trajectory

The trajectory is the **Program order** table above: Phase 1 (itr#396) → Phase 2 (itr#403) → Phase 3
(itr#398) is the strict critical path; Phases 4–9 follow in listed order, interleaving only when a phase
is blocked. `/sprint` should draw its Sprint Goal from the lowest-numbered phase with open, unblocked
members (`itr ready` inside the phase epic).

## Stub filing

No roadmap stubs were filed in this baseline. The existing `itr` backlog already covers the approved roadmap rows; future `/roadmap --update` runs can file selective stubs if a row has no issue coverage.

## Update cadence

- Read at the start of every `/sprint` to inform Sprint Goal selection.
- Update at the end of every `/sprint-review`.
- Re-run `/roadmap` manually when scope changes, major `itr` epics are added, or the v1/v2 boundary changes.
