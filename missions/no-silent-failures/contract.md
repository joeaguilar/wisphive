# Mission contract — no-silent-failures

- **Mode:** `--backlog` · lanes=2 · budget: unset (burn-based kill criteria skipped)
- **Baseline:** `41c14eecf0e09db123dab632eb5dfe0636234fc3` (clean; rust gate green — gatr tag `mission-baseline-rust`)
- **Launched:** 2026-07-18

## Outcomes

1. **O1 — The web UI never swallows a failure.** Daemon errors are surfaced to the operator (banner/toast + correlation), and the PO-reported "Spawn Agent form does nothing" regression is root-caused and fixed. (itr#567 → itr#565)
2. **O2 — The managed-spawn path has no silent fail-open.** Capacity rejection, unknown-provider detection, headless always-defer, false-disconnect reaping, and stop-agent blast radius all resolve loudly, audited, per ADR-0001/0002 postures. (itr#560, #562, #559, #568, #561)
3. **O3 (queued chain) — Sessions survive a daemon restart (plan A).** Pin flag + persisted respawn spec + reconcile-on-start. (itr#590, #589, #591 under epic #588) Admitted when a lane frees.

## Non-goals

- itr#601 (agentic discovery gates) — tagged **solo-agent-only**; explicitly excluded from multi-agent orchestration.
- Session-survival plan B/C (live FD-handoff #594–#597/#599, VIP PTY-host #598) — beyond the evidence horizon; spike #581 exists and stays queued in its epic.
- herdr UX (#574–#578) — territory overlaps chain C (#589 touches the same TUI/web dirs); not admitted this mission.
- Provider-transport/ADR-0009 spikes (#564/#566/#569/#572) — separate research track.
- Program phases per ROADMAP (#403 remainder, #398 remainder, #270/#283/#284) — noted as successor candidates, not admitted.

## Acceptance oracles (executable)

| Tag | Command | Scope |
|---|---|---|
| `oracle-rust` | `sh -c 'cargo fmt --all -- --check && cargo clippy --workspace -- -D warnings && cargo test --workspace'` | every link (integration gate) + closure |
| `oracle-frontend` | `sh -c 'cd crates/wisphive_web/frontend && npx eslint . && npx vitest run'` | links touching `crates/wisphive_web/frontend` + closure if chain A landed |
| `oracle-e2e` | `just e2e` | chain A terminal link + closure if chain A landed |

All run as `gatr run --tag <tag> -- <cmd>`.

## Budget arithmetic

No `--budget` set. total: null · per-chain alloc: n/a · spent: tracked qualitatively in digests · returned: n/a. Link/rework/contradicts kill criteria still fire.

## Authority envelope

Operator attended-with-defaults but this session runs autonomously: every intake question was resolved to its recommended default and tagged `assumed-by-default`. On operator silence, lanes **continue the uncontested subset** — never stall, never expand scope. Councils fire on mechanical triggers only. No push/PR. `./install.sh` is never run by agents (live gated sessions exist — CLAUDE.md standing rule). Commits per landed link by the orchestrator only, Conventional Commits, to `main`.

## Intake decisions (defaulted — PO may override any time with `contest`/`amend`)

| Q | Decision (default taken) | Tag |
|---|---|---|
| Q1 Priority: criticals-first vs ROADMAP program order (#403 next)? | **Criticals-first**: chains A+B are decision-plane integrity in substance (silent fail-open, PO-reported dead spawn); ROADMAP is 6 days stale, predates incident-driven findings. | assumed-by-default |
| Q2 Chain B includes #561/#568 (registry territory, weak semantic coupling)? | **Yes** — same files, serial lane avoids collisions; premise carries them as territory-order, not semantic deps. | assumed-by-default |
| Q3 e2e cadence for chain A? | **Terminal link + closure only** (per-link e2e too slow; vitest+lint per frontend link). | assumed-by-default |
| Q4 ADR amendments (ADR-0001 for #560, ADR-0002 for #559) in-link or separate links? | **In-link** — the ACs demand them; a behavior change and its ADR land in one commit. | assumed-by-default |

## Assumption register

Chain-level assumptions live in each `chains/<id>/premise.md`. Register tags after intake red-team (see premises for verdicts):

- `X-A1` (cross-chain): server.rs/process_registry.rs collisions between chains A and B are rare enough for serial patch-landing on fresh HEAD. — assumed-by-default
- `X-A2` (cross-chain): no concurrent foreign session mutates these files mid-mission; git status checked before each land; foreign changes = surface, never stage. — assumed-by-default
- `X-A3`: issue bodies/notes (esp. #565 note 218, #559 note 217) reflect current code reality. — spike-scheduled → resolved by intake red-team, see premises.
