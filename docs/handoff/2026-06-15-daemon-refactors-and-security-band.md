# Daemon refactors + security-band hardening — handoff & next steps

**Date:** 2026-06-15
**Branch:** `main` @ `b6e5af1`
**Closed this session:** itr#388, #121, #85, #335, #332, #333, #334, #122, #123, #81, #82, #83
**Filed this session:** itr#389 (carved out of #122)
**Still open / next:** itr#84, #94 (queue-gating pair), #389 (DB migrator), #337 (hook integration tests)
**Predecessor handoff:** `docs/handoff/2026-06-14-always-defer-posture-modes.md`

If you only have 60 seconds: this session cleared a large refactor + security-hardening band (8 commits) and is paused **before the two queue-gating items, #84 and #94** — start there (§ Where to start next). Two things will bite you if you don't read them first: (1) **§ Hard rules #1** — the daemon DB migration must stay idempotent; do NOT naively switch to `sqlx::migrate!` (that's #389, and done wrong it bricks every existing user DB); (2) **§ The worktree-base gotcha** — fan-out agents in this repo branched off HEAD's *parent*, not HEAD, so verify merge-base before trusting a cherry-pick.

## What just shipped

The daemon's two biggest files were decomposed and a band of memory/credential DoS holes were closed. `state.rs` (3157 lines) became per-domain modules; `handle_tui` (720 lines, ~25 match arms) became a thin select-loop + four dispatchers; the Unix socket is now owner-only with a peer-cred gate; the per-connection channel is bounded; every socket reader is length-capped. Separately, a latent hook bug (`AskUserQuestion` auto-resolving) was fixed, and the auto-approve tool list was de-duplicated to a single source of truth.

```
b6e5af1  fix(daemon): cap socket line length to prevent unbounded-read memory DoS (itr#83)
b99eb9f  fix(daemon): bound the per-connection TUI channel to cap memory (itr#82)
0f8e674  fix(daemon): set 0600 perms + peer-cred check on the unix socket (itr#81)
789c227  refactor(daemon): split handle_tui into per-domain dispatchers + dedupe write/persist helpers (itr#123)
b8db1e0  refactor(daemon): split state.rs into per-domain modules (itr#122)
af5675b  fix(daemon): prevent AppleScript injection in osascript notification fallback (itr#85)
5f9c504  refactor: centralize auto-approve tool list in wisphive_protocol (itr#121)
10e78f5  fix(hook): defer always-ask tools on PermissionRequest, not just PreToolUse (itr#388)
```
(Interleaved with the user's own concurrent commits `60e0878` discovery CLI / `00d5bba` analytics roadmap — see § Heads-up.)

| Surface | Anchor |
|---|---|
| `state.rs` → per-domain modules (mod/migrate/decisions/retention/summaries/web_auth/web_passkeys/terminals), each ≤500 lines | `crates/wisphive_daemon/src/state/` |
| `handle_tui` (125 lines) → `dispatch_command` router + `handle_{decision,agent,query,terminal}_command` | `crates/wisphive_daemon/src/server.rs` |
| `write_msg()` / `eager_persist()` dedup helpers | `crates/wisphive_daemon/src/server.rs` |
| `set_socket_permissions()` (0600) + `peer_uid_allowed()` gate; `ensure_dirs` chmods ~/.wisphive 0700 | `server.rs`, `crates/wisphive_daemon/src/config.rs` |
| `CONN_CHANNEL_CAPACITY = 1024` bounded channel; `MAX_LINE_BYTES = 8 MiB` + `read_capped_line` | `crates/wisphive_daemon/src/server.rs` (+ web `ws_bridge.rs`, hook `main.rs`) |
| Always-defer now fires on `PermissionRequest` (was PreToolUse-only) | `crates/wisphive_hook/src/main.rs` (`is_always_deferred` guard, ~L749) |
| Single-source auto-approve tiers + `GET /api/tool-tiers` | `wisphive_protocol/src/types.rs` (`tier_tools`/`ToolTiers`/`all_known_tools`), `wisphive_web/src/lib.rs`, frontend `Config.tsx` |

## Trade-offs made

- **#122 split ≠ migration rewrite.** The issue bundled "split state.rs" with "replace the ALTER chain with `sqlx::migrate!`". I shipped only the split and **carved the migrator out to #389**, because adopting `sqlx::migrate!` on a DB that has no `_sqlx_migrations` table is a data-safety hazard (see Hard rules #1). The split is pure relocation — verified by daemon lib test count holding at 125→125 with only `state.rs` touched.
- **#123 `dispatch_command` always returns `ControlFlow::Continue`.** No command arm in the original `match` ever broke the select loop (the `break`s live in the conn_rx/tui_rx/EOF framing, which stayed in `handle_tui`). The `ControlFlow` return type is there for future break-signalling; today it's always Continue, and that's correct.
- **#82 channel policy is split by producer.** Terminal-attach forwarders `try_send` and **drop** a frame on `Full` (a stalled TUI must not grow memory or block the forwarder). The `TermReplay` task `send().await` **back-pressures** instead — it holds no lock and must not drop frames (gaps corrupt the rendered replay).
- **#83 `line_buf` is caller-owned, not allocated inside `read_capped_line`.** It persists across `tokio::select!` iterations so a cancelled read (a sibling branch fired) resumes its partial line instead of desyncing the framing. Moving it inside the helper reintroduces a cancel-safety bug.
- **#81 does not special-case root.** A foreign-uid peer (incl. root) is dropped; only the daemon's own euid is trusted. Single-user model.
- **#335/#332/#333/#334 were closed, not re-implemented** — they were already done in prior work (prune wired into `run_retention`, size-guarded VACUUM, `events.jsonl` rotation). Verified in code before closing. **Lesson: in this repo, "open" ≠ "undone" — grep the code before sending an agent to implement.**

## What's NOT shipped — explicit scope gaps

1. **#84 — route `TermCreate` through the human-decision queue + scrub env** (high, open). Architectural: changes a TUI/web-initiated terminal spawn from execute-immediately to approve-then-execute.
2. **#94 — validate `SpawnAgent` flags + queue for human approval** (high, open). Reject `bypassPermissions`, cap system-prompt, validate project path; then queue for approval. Was blocked behind #123; **now unblocked**. Shares the queue-routing mechanism with #84 — build it once.
3. **#389 — adopt `sqlx::migrate!` for the daemon DB** (high, open, **data-safety**). Needs a *baselined* migrator (single `0001` = full schema as `CREATE TABLE IF NOT EXISTS`, a no-op on existing DBs) + a backward-compat test that opens an old-schema DB and proves the migrator no-ops. Anchor: `crates/wisphive_daemon/src/state/migrate.rs`.
4. **#337 — integration tests for hook connect/handshake/decision failure classification** (open). The unit-level fail-open behavior is covered; the connect/handshake-boundary integration tests were deferred here.
5. **CLAUDE.md not updated for #81/#121.** Skipped intentionally to avoid colliding with the concurrent session also editing CLAUDE.md. A future session should add: socket is 0600 + peer-cred (Runtime Files § `wisphive.sock`), and the `GET /api/tool-tiers` endpoint.

## Hard rules established this session

1. **The daemon DB migration must stay idempotent until #389 lands a baselined migrator.** Current `migrate()` is `CREATE TABLE IF NOT EXISTS` + tolerant `ALTER … .ok()`, run every boot. Existing user DBs have **no `_sqlx_migrations` table**. A naive `sqlx::migrate!` with per-ALTER migrations will re-add existing columns and **brick the DB**. Only switch via a single baseline `0001` (full schema, IF NOT EXISTS) + a backward-compat test. There is **no startup VACUUM** and VACUUM is size-guarded in `run_retention` — keep it that way (#333).
2. **`state.rs` is gone — keep it gone.** Public paths (`StateDb`, `WebAuthError`, row structs, `RetentionOutcome`, `AutoApprovedEntry`) are preserved via `pub use` in `state/mod.rs`; no `state/` module may exceed ~500 lines. Don't recreate the monolith.
3. **`handle_tui` stays a thin loop (≤150 lines).** New TUI commands go in the appropriate `handle_*_command` dispatcher, not back inline. Socket writes go through `write_msg`; the eager-persist sequence goes through `eager_persist` (one definition). Don't reintroduce the inline `encode()? + write_all` triplet (it was 39×).
4. **Socket readers must go through `read_capped_line` / `MAX_LINE_BYTES`; the per-loop `line_buf` accumulator stays caller-owned** (select cancel-safety). The per-connection channel stays bounded — **no `mpsc::unbounded_channel` in the daemon** (`rg unbounded_channel crates/wisphive_daemon` must return 0).
5. **The socket is 0600 + peer-cred gated.** Don't loosen perms or accept foreign uids.
6. **The auto-approve tool list lives ONLY in `wisphive_protocol::AutoApproveLevel::tier_tools`** (#121). Hook/TUI/CLI derive from it; the web SPA fetches `GET /api/tool-tiers`. Adding a tool = edit `tier_tools` and nowhere else.
7. **Always-defer fires on BOTH `PreToolUse` and `PermissionRequest`** (#388). For an always-deferred tool, the PermissionRequest path returns `Decision::Ask` → emits no decision object → native dialog renders. Do not re-add the `!is_permission_request` guard.

## The worktree-base gotcha (read before using fan-out agents here)

Fan-out agents launched with `isolation: "worktree"` this session branched off **HEAD's parent commit**, not HEAD — so #81/#82/#83 were written against the *pre-#123* `server.rs` and would not cherry-pick onto the refactored tree. #81's conflict was small (test-module `use` line); #82/#83 had to be **manually re-applied** onto the new dispatcher structure using the agents' diffs (`git show <sha> -- <file>`). Mitigations for next time:
- After a just-made commit, **verify the agent's base**: `git merge-base main <worktree-branch>` should equal HEAD. If it's HEAD's parent, expect cherry-pick conflicts.
- For files an agent's base shares with current HEAD (untouched since the fork), you can `git checkout <sha> -- <file>` to grab its version directly instead of cherry-picking the whole commit (that's how #83's `ws_bridge.rs`/`hook/main.rs` were taken).
- For **sequential** edits to one shared file (`server.rs`), prefer working in the main tree (no worktree) — worktree isolation only pays off for genuinely disjoint files run in parallel.

## Where to start next

1. **#84 + #94 — the queue-gating pair (recommended, M–L).** Build the "validate → enqueue a human decision → execute on approval" path once and apply to both `SpawnAgent` (`handle_agent_command`, `process_registry.rs`) and `TermCreate` (`handle_terminal_command`, `terminal.rs`). Study `handle_hook`'s existing DecisionRequest→oneshot→resolve flow for the queue mechanism. **Do a brief design check first** — this changes how spawns/terminals *feel* (they now wait for approval), which is a UX decision worth confirming with the PO.
2. **#389 — baselined `sqlx::migrate!` (M, data-safety).** Self-contained but must not regress Hard rule #1. Write the backward-compat test first. A DB backup exists at `~/.wisphive/backups/wisphive-20260614-232947.db` (verified, integrity ok).
3. **#337 — hook connect/handshake integration tests (S).** Closes the coverage gap left when #335 was verified.

## Heads-up: concurrent work on the discovery epic

During this session the user (`joeaguilar`) committed `60e0878 feat: add project discovery CLI` (itr#351 — `wisphive projects {scan,audit,seed}`, `project_audit.rs`, `scripts/wisphive-project-seed.sh`) and `00d5bba` (deterministic-agent-analytics roadmap + ADR-0004). The discovery epic (itr#349/#351/#352) has progress outside this session's commits — `itr ready` and `git log` are the source of truth, not this handoff.

## Memory / docs to read for context

- `~/.claude` memory: `feedback_commit_to_main` (commit straight to main here), `reference_wisphive_self_gating` (this dev session is itself gated by the installed hook; reinstall via `./install.sh` after hook changes), `feedback_commit_dont_rewrite`, `reference_itr`.
- ADRs: [`0001-tiered-fail-posture`](../decisions/0001-tiered-fail-posture.md), [`0002-always-defer-classification`](../decisions/0002-always-defer-classification.md).
- `CLAUDE.md` → **Architecture** (the `state/` split and the daemon command flow described there now map to the new module layout), **Runtime Files** (`wisphive.sock`, `config.json`), **Key Design Decisions** (tiered fail posture).
- Predecessor: `docs/handoff/2026-06-14-always-defer-posture-modes.md` (the #388 fix this session completes the other half of — PermissionRequest path).
