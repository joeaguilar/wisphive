# Story Style — Wisphive

_Last updated: 2026-07-05_

> How this project writes issues. Read by `/sprint` Phase 0 and any agent that creates issues for this repo.

## Title & Body

**Title shape:** mixed — imperative for features/tasks ("Add deterministic risk digest", "Sudo-gate web-origin ApprovePermission"); symptom-declarative for bugs, naming the failure ("TUI byte-slice truncation panics on multi-byte agent content").
**Title length:** long is fine, ~40–95 chars; no hard cap. Specificity beats brevity — name the component and the actual symptom/action.
**Title devices** (use when they add signal, don't force):
- **Component prefix** with a colon — `Web UI: …`, `Web security: …`, `logging: …`.
- **Em-dash scope/impact suffix** — `… — review-surface DoS`, `… — daemon, hook, web auth/TLS, CLI`.
- **Parenthetical scope** — `(queue + session panels)`, `(wisphive web)`.
- **Provenance suffix** — `(from itr#312 review)`.
- **`EPIC:` prefix** for epics that head a program of work.

**Body template:** flat technical prose — no fixed Why/What skeleton. The one near-universal rule: **open with provenance** — where this came from (a review, a deferral, a plan doc, an ADR, a spec section, a code path).

```
<Provenance line — "Deferred from itr#297.", "Source: docs/code_review/…", "This is Epic 2 in docs/plan-…", "Follow-up from the itr#312 review (commit b6662b2)">

<Dense technical body. Ground every claim: file:line (panels.rs:117-118), commit SHAs, ADR-000N, spec §. State the failure mode and constraints. Use ## Background / ## subheads only when long.>
```

**Required sections:** Provenance/context prose (the opening line counts).
**Optional sections:** `## Background`, `## Notes`, or other markdown subheads for longer issues.

## Acceptance Criteria

**Format:** bulleted observable outcomes (default). `- [ ]` checkbox form is acceptable for a checklist-shaped bug. Epics use a short prose closure condition ("All member issues closed, or waived with a reason recorded").
**Observability rule:** must be agent-checkable and concrete — name the exact command and expected output, the specific repro input, and the test that covers it. Cite the test matrix when regression scope matters ("without regressing the 42-test Vitest matrix"). Prefer a named ADR/spec § over "works correctly".
**Runtime-evidence rule:** for UI / visual / behavioral / notification work, acceptance must demand runtime proof — a driven flow, a real session, a Playwright screenshot — not just green tests or a build. "User sees X" ⇒ the *full* X is reachable from the UI. (Mirrors CLAUDE.md: "A written value ≠ a wired feature.")
**DoD reference:** sprint-specific Definition of Done is appended by `/sprint`; not defined here.

## Tags & Priority

**Tag taxonomy:** flat, lowercase, comma-separated — no prefixes. Combine freely from these axes:
- **Component:** `tui`, `web`, `frontend`, `daemon`, `hook`, `cli`, `protocol`, `adapter`.
- **Concern:** `security`, `hardening`, `bug`, `panic`, `dos`, `crash`, `ux`, `a11y`, `auth`, `permission`, `correctness`, `durability`, `error-handling`.
- **Program/workflow:** `command-center`, `deterministic-analytics`, `product-backlog`, `needs-sprint`, `epic`, `fast-follow`.
- **Provenance:** `review-followup`, `followup-<id>`, `codex-review`, `from-review-2`.
**Skills:** route work with `--skills` — `rust`, `typescript`, `react`, `css`, `sqlite`, `axum`, `tokio`. Leave empty for pure-daemon Rust issues where it adds nothing.
**Files:** include `--files` with implicated paths (`crates/…/src/foo.rs`, docs, specs) whenever known — high signal for the picking agent.
**Priority scheme:** `critical`, `high`, `medium`, `low`.
**Epic linking:** `--parent <id>` for real parents. Cross-reference related issues in the body as `itr#NNN` / `#NNN`.

## Language & Voice

**Terminology:** prefer "issue". Cross-refs are `itr#NNN` or `#NNN`.
**Voice:** terse-technical, evidence-first. Anchor claims to code (`file.rs:line`), commit SHAs, ADR-000N, or spec §. Assume a fluent reader — no hand-holding.
**Banned phrases / anti-patterns:**
- No vague acceptance — "works properly", "handles it better", "should be fine".
- No filler — "simply", "just".
- No unprovenanced bug reports — say where it was observed or which review/path surfaced it.
- No "improve X" titles that don't name the observable target.

**Domain glossary:**
- **daemon** — the Tokio server on `~/.wisphive/wisphive.sock` that gates tool calls.
- **hook** — `wisphive-hook`, the Claude Code / Codex subprocess that defers decisions to the daemon.
- **always-defer** — the intrinsic classification (questions/plan-mode/elicitations) that never reaches the daemon queue (ADR-0002).
- **fail-open / fail-closed** — the tiered posture for daemon-unreachable vs runtime errors (ADR-0001).
- **Command Center** — the live ops console program (waiting-on-you inbox, liveness, burn meter).
- **ADR** — Architecture Decision Record under `docs/decisions/`; cite by number.

**Other project-specific notes:**
- Provenance-first bodies: reviews, deferrals, plan docs, ADRs, and `docs/code_review/` findings are the usual origins — always name them.
- Security-relevant issues state the trust/fail posture and reference the governing ADR.

## Worked Examples

### Example 1 — bug

TUI byte-slice truncation panics on multi-byte agent content (queue + session panels) — review-surface DoS

`panels.rs:117-118` (`&summary[..47]`) and `ui.rs:1007-1010` (`&s[..37]`) byte-slice agent-controlled strings (Bash command, prompt, raw tool_input JSON). If the byte boundary falls inside a UTF-8 continuation (emoji/CJK/accented char in a commit message), the slice panics — an agent can crash the review surface with a crafted commit message.

**Acceptance criteria:**
- Char-aware truncation everywhere (e.g. `s.chars().take(47).collect::<String>() + "…"`), mirroring `web.rs::devices_list:147`.
- A pending Bash command of `git commit -m "ship 🚀"` renders in queue + session panels without panic.
- Ideally extracted to a shared `truncate` helper covered by a unit test over multi-byte input.

### Example 2 — epic

Add deterministic risk digest for agent activity

This is Epic 2 in `docs/plan-deterministic-agent-analytics.md`. Surface review-worthy agent actions without changing policy; reuse deterministic facts rather than learned-policy confidence.

**Acceptance criteria:**
- Risk taxonomy covers destructive filesystem ops, privileged/sudo-like commands, network access, secret-adjacent paths, dependency/CI changes, publish/deploy actions, denied actions, and unknown tools.
- CLI can query risk items by time range, project, and session.
- All member issues closed with runtime evidence; no policy behavior changed (deterministic-only).
