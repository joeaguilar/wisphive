# Campaign 001 — Codex spawn gating (2026-07-04)

**Goal (G):** file + fix the two gating bugs proved in the Codex-spawn probe.
**Scope:** inline (probe findings from this session). No roadmap slice.
**Verify gate (V):** `cargo fmt --all --check` + `cargo clippy --workspace -D warnings` + `cargo test --workspace` (all green). Frontend/e2e not run — no UI surface touched.

## Items

### itr#467 — Codex managed spawn is ungated AND fails to launch [security, P?] — VERIFIED
Root cause: `process_registry.rs` built `codex exec --ask-for-approval never …` (invalid flag on codex-cli 0.142.5 → arg-parse abort), and even corrected, Codex silently skips untrusted hooks → spawned agent ran **completely ungated**.

Fix (`crates/wisphive_daemon/src/process_registry.rs`):
- Fail-closed guard: refuse Codex spawn unless `audit_project(project).hooks.codex.installed`.
- Corrected flags: drop `--ask-for-approval never`; add `--skip-git-repo-check` + `--dangerously-bypass-hook-trust` (daemon-vetted hook runs headlessly).
- `#[tokio::test] codex_spawn_fails_closed_without_wisphive_hook`.

**Runtime evidence (isolated daemon on temp HOME, real code path):**
- (a) launches: `agent start --agent-type codex` → `Agent started: Type: codex, PID: 17624` (was: arg-parse abort, no process).
- (b) fails closed: spawn into non-hooked project → `Error: … fix: wisphive hooks install --project …` (no process spawned).
- (c) gated: hooks present → codex-authored **PreToolUse** event landed in daemon events.jsonl (`agent_type=codex, decided_by=level:all, project=/private/tmp/whproj`). Before the fix this produced **0 records** (ungated).

### itr#468 — CLI misreads daemon response (drain 2, daemon sends 3) [bug] — VERIFIED
Root cause: `agent.rs send_and_recv` drained a fixed 2 snapshots; daemon now sends 3 (AgentsSnapshot/QueueSnapshot/AuditSnapshot, itr#399/#434) → AuditSnapshot misread as the reply.

Fix (`crates/wisphive_cli/src/commands/agent.rs`): skip any Agents/Queue/Audit snapshot, return first real response (robust to count).

**Runtime evidence:** old CLI on live daemon → `Unexpected response: AuditSnapshot {…}` (148 KB dump); new CLI on same daemon → `No managed agents running.` Also confirmed by the itr#467 spawn printing a clean `Agent started:` block.

## Review (adversarial, pre-commit)
Verdict: no P0/P1. One security should-fix applied, rest filed.
- **P2-A (FIXED):** guard originally used the deprecated substring `"wisphive"` matcher (`project_audit`) — a hook merely *containing* "wisphive" would pass, re-opening the ungated hole on the web/loop/TUI paths (which skip the CLI preflight). Replaced with strict `hook_install::codex_pretooluse_hook_installed` (`is_wisphive_hook_command` basename match, itr#359; PreToolUse-specific). Regression test `codex_gate_false_for_substring_only_hook` proves it.
- **P2-C (comment fixed, proper fix filed #470):** `send_and_recv` skips connect snapshots but not interleaved broadcast events (same-variant misread). Pre-existing; not introduced here. Comment softened; correlation fix → #470.
- **P2-B → #471:** `--dangerously-bypass-hook-trust` runs *all* project hooks headlessly; guard only checks wisphive presence. + TOCTOU. 
- **P3 → #472:** daemon guard doesn't check kill-switch `mode` like the CLI preflight does.

## Incidental
- Pre-existing fmt drift in `crates/wisphive_daemon/src/server.rs:1989` (not my change) — reformatted to unblock the fmt gate; committed separately from the feature.

## Doc
- `docs/plan-loop-supervisor.md` open-Q #3: added the empirically-proven Codex hook-trust gating constraint (a Codex-backed loop only gates when the hook is installed + trust bypassed).
