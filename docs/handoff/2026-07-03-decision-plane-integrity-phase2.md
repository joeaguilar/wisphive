# Decision-plane integrity (Command Center Phase 2) — handoff & next steps

**Date:** 2026-07-03
**Branch:** `main` @ `6265a4d`
**Epic / itr:** itr#403 (decision-plane integrity, Command Center P0.5) — all 16 members now closed
**Closed this session:** itr#425, #298, #299, #300, #337
**Filed this session:** none
**Predecessor handoff:** `docs/handoff/2026-07-03-verification-harness-and-spec-hardening.md` (which pointed here as "start next"); the epic's Phase 1 sibling is `docs/handoff/2026-07-03-decision-plane-trust-phase1.md`

If you only have 60 seconds: every member bug of the Phase 2 integrity epic (itr#403)
is closed with tests + an adversarial review pass on each commit. The tree is green
(`cargo test --workspace` = 543 passing, clippy + fmt clean). The **one thing left to
call the epic itself done** is the epic-level red-team verification pass in #403's
acceptance (ghost-approval / restart-mid-stream / secret-redaction against installed
binaries) — see § Where to start next. That is a verification capstone, not more code.

## What just shipped

Two hardening themes closed the last open members of the decision-plane integrity epic:

**1. Control-plane self-protection (itr#425).** A gated agent holding a generic
Write/Edit/Bash approval could previously rewrite its own gate by editing
`~/.wisphive/**` (config.json, mode, events.jsonl, the DB). The hook now forces any
`Write`/`Edit`/`MultiEdit`/`NotebookEdit`/`Bash` call targeting the state dir past
**both** auto-approve layers to daemon human review, at any `auto_approve_level`
including `all`, unless `allow_self_modification: true`. It routes to the human queue
rather than `Decision::Ask`, because `hooks install` allow-lists Write/Edit/Bash in
Claude's native prompt — an Ask would auto-allow and defeat the guard. Path matching
canonicalizes the deepest existing ancestor (resolving symlinks + case-insensitive-FS
spellings), closing two bypasses an initial lexical matcher missed.

**2. pending_decisions persistence semantics (itr#298/#299/#300).** `pending_decisions`
is now defined as **transient in-flight bookkeeping, not a recovery queue**: a blocked
hook's oneshot dies with the daemon and can't be recreated, and that hook already
fail-open-approved the instant the socket closed (DaemonUnreachable, ADR-0001). So on
restart `drain_orphaned_pending` records every orphan as `Approve` /
`daemon_restart:failopen` (the truthful outcome — the tool ran) and clears the table;
an Ask/defer now deletes its row without logging (#298); and `permission_suggestions`
is deliberately **not** persisted (#300, resolved by the #299 decision — no recovery
read model to feed, and the raw write leaked secrets past the itr#89 redactor).

**3. Hook failure-classification harness (itr#337).** The daemon transport was extracted
from `run_active` into a pure `request_decision(...)` seam so the
connect-vs-handshake-vs-live-daemon fail-open/closed boundary is testable against a real
Unix socket. Four scenarios lock it: pre-Welcome EOF → open, mid-wait EOF → open,
well-formed Error → closed, garbage → closed.

```
60667e6  feat(hook): default-deny agent edits of ~/.wisphive (control-plane self-protection)
700c605  fix(hook): resolve symlinks and case in control-plane path check
63d1273  fix(daemon): pending_decisions recovery semantics, Ask cleanup, suggestion persistence
190541d  style: cargo fmt on hook self-protection + pending_decisions changes
5c1d813  fix(daemon): do not persist permission_suggestions to pending_decisions
6265a4d  test(hook): socket-harness integration tests for decision failure classification
```

| Surface | Anchor |
|---|---|
| self-protection gate + matcher | `crates/wisphive_hook/src/main.rs` (`targets_control_plane`, `path_in_dir`, `resolve_existing_ancestor`, `allow_self_modification`) |
| pending drain / Ask cleanup | `crates/wisphive_daemon/src/state/decisions.rs` (`drain_orphaned_pending`, `delete_pending`) |
| drain wiring | `crates/wisphive_daemon/src/server.rs` (`Server::new`) |
| transport seam + harness | `crates/wisphive_hook/src/main.rs` (`request_decision`, `mod tests` socket scenarios) |
| self-protection spec | `docs/decisions/0005-policy-learning-security-invariants.md` (I9), `docs/plan-policy-learning-engine.md` §I9 |

## How this was verified

Each commit got an adversarial review agent before its issue closed (the house
commit→review→act→close loop):

- **#425** — review found two real bypasses on the first (lexical) matcher: macOS
  case-insensitive FS (`~/.Wisphive/config.json` hits the real inode) and a symlinked
  ancestor. Both fixed by canonicalizing the deepest existing ancestor (`700c605`);
  regression tests for both.
- **#298/#299/#300** — review confirmed the fail-open drain semantics are truthful for
  every orphan, but caught that the #300 suggestion-persistence was dead code (nothing
  reads it under #299) **and** leaked secrets unredacted. Resolved by dropping the write
  (`5c1d813`).
- **#337** — review confirmed the extraction is behavior-preserving byte-for-byte (every
  `map_err`/classification site unchanged; Codex-Ask audit log moved to the caller with
  an identical condition) and the tests are race-free.

## Trade-offs made

- **#300 resolved against its literal acceptance.** #300 asked to persist
  `permission_suggestions` "even if pending recovery lands." #299 (same cluster) decided
  recovery does **not** land — pending rows are drained, never re-served — which moots the
  premise. Persisting suggestions became dead code that also introduced an itr#89 secret
  leak. Chosen resolution: drop the write, document that the in-memory queue is
  authoritative and the column stays NULL. If #399's inbox later wants "options on the
  table" in the audit trail, that's a *separate* redacted `decision_log` enhancement, not
  a pending-row write.
- **Self-protection fails open when the daemon is down.** Consistent with ADR-0001; when
  the daemon is unreachable, gating is off for every tool anyway, so the guard adds nothing
  there. The case it closes is daemon-up + high auto-approve level.
- **Bash self-protection is a best-effort substring backstop** (documented, not sound).
  The sound layer is the file-tool path canonicalization; the malicious end of a
  symlink-setup-then-write bypass is caught at the *write* step even when the `ln` slips
  past the Bash check.

## What's NOT shipped — explicit scope gaps

1. **Epic-level red-team pass for #403** — not yet run. The member fixes are individually
   tested, but #403's acceptance calls for an end-to-end red-team against installed
   binaries (see § Where to start next). The epic is member-complete but not
   acceptance-closed.
2. **MCP filesystem servers bypass self-protection** — the guard covers built-in Claude
   Code tool names only; an operator-installed MCP server that writes files by another
   tool name is out of scope (noted in the I9 spec). Residual decision-time TOCTOU on the
   path check also remains by nature — human review is the backstop.
3. **`drained` counter can overcount** on a partial-failure re-drain (LOW, cosmetic — the
   audit rows are correct via INSERT OR IGNORE; only the `info!` count inflates). Left as-is.

## Hard rules established this session

1. **Agent writes to `~/.wisphive/**` never auto-approve.** The self-protection gate
   routes them to human review at every level unless `allow_self_modification: true`. It
   must route to the daemon queue, **never** `Decision::Ask` (Ask is allow-listed by
   `hooks install`). Path checks canonicalize before comparing — do not revert to a lexical
   `starts_with`.
2. **`pending_decisions` is not a recovery queue.** Never re-serve a pending row into the
   live queue on restart; drain it as `daemon_restart:failopen`. Recording a Deny there is
   an audit lie (the hook fail-open ran the tool) — unlike the hook-*disconnect* path where
   Deny is correct.
3. **Never persist `permission_suggestions` (or any raw agent field) to disk without the
   itr#89 redactor.** The pending row leaves the column NULL.
4. **The hook fails open only pre-Welcome.** Any failure up to and including a bad Welcome
   is `DaemonUnreachable` (open); a live daemon that then refuses/garbles is fail-closed per
   fail-mode. The `request_decision` seam and its socket harness lock this — keep them in
   sync if you touch the transport.

## Where to start next

The Phase 2 member work is done and green. Next, in priority order:

1. **Run the epic-level red-team pass and close #403 (recommended, ~1 session).** #403's
   acceptance: kill the hook mid-decision → assert no contradictory audit rows
   (exercises #363); restart the daemon mid-stream → assert no auto-answer events lost and
   orphans land as `daemon_restart:failopen` (exercises #299); put a secret in `tool_input`
   → assert redaction in persisted rows AND notifications (exercises #89, and the #300 NULL
   column). Mirror the Phase 1 method on #396 (red-team against **installed** binaries —
   `./install.sh` first, since a stale hook can auto-resolve differently). Record evidence
   on #403 and close it; it currently BLOCKS the inbox story #399.
2. **Then the inbox (#399)** — now unblocked once #403 closes. It renders the
   `decided_by`/`config_hash` audit stream this epic made durable.
3. **Policy-learning engine** — its default-deny blocker (ADR-0005 I9) is now cleared by
   #425, but keep the I2 "no substring `allow_patterns`" invariant front-of-mind (the
   plan-doc still had violations that the last session's review caught).

## Memory / docs to read for context

- `docs/decisions/0005-policy-learning-security-invariants.md` (I9 self-protection) and
  ADR-0001 (tiered fail posture — why daemon-unreachable always fails open).
- `CLAUDE.md` → "Runtime Files" (`config.json` `allow_self_modification` key; the
  `pending_decisions` / `SQLite WAL crash recovery` bullet, updated this session).
- `~/.claude` memory: `reference_wisphive_self_gating` (this dev session is gated by the
  installed hook — reinstall via `./install.sh` after hook changes before any red-team),
  `feedback_review_workflow` (commit→review→act→close).
