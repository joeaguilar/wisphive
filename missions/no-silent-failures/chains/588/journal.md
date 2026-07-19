# Chain 588 journal (queued)
facts-cursor: 2026-07-19T00:23:29Z

## admission — 2026-07-19T01:59:22Z
Chain 588 ADMITTED to lane A (602 sealed). Admission checks: C-A2 resolved (#591 body read — reconcile scope matches premise: pinned respawn + scrollback seed + re-enters gate; blocked_by 589,590 confirmed); C-A3 moot (602 sealed before #589 starts); FACTS diff: no entries touch C-A1..C-A4; operational constraints inherited (stale-worktree base check, dist copy). facts-cursor advanced to 2026-07-19T01:59:22Z.

## L1/decision — 2026-07-19T01:59:22Z
Claimed itr#590 (lane A) on HEAD 20b0ad7. Daemon-only persistence link; itr#89 redaction constraint applies to env storage.

## L1/discovery — 2026-07-19T02:11:56Z
Link agent report in, gate green (945 passed/0 failed). TerminalEnvSpec: non-secret verbatim, secret = NAME-ONLY inherit_names re-sourced from live daemon env at materialize (never a stored placeholder), scrubber-driven via protocol public API. Dark landing (nothing reads spec in prod; wire untouched). Runtime PIN: real-PTY respawn from stored spec (same cwd/command, rotated-secret re-sourced, stale never replayed). CORRECTION (not contradiction): env_json column existed since table creation — zero-migration link; 'riskiest schema early' rationale overstated, ordering still holds (#591 needs the spec). Notes for #589 (env preview = names only, needs deliberate protocol addition) and #591 (materialize(std::env::vars) at reconcile; replayed scrollback may contain old secrets). Reviewer spawned.

## L1/review — 2026-07-19T02:20:21Z
Reviewer: conforms=FALSE (1 MUST-FIX, empirically confirmed), oracle_adequate=FALSE. C-A1 supports, C-A2 supports — no council. MUST-FIX: classification probes redact_text only, never the key-name rule — secret-named keys with whitespace-prefixed values (+ trailing-space/embedded-= names) persist CLEARTEXT in env_json (5 confirmed cases); baseline stores no env, so this is a NEW leak class = DoD#2 violation. One-line fix (OR in redact_value key-rule probe) + differential corpus test. SHOULD-FIX: malformed env_json degradation untested; differential corpus. Dormancy PROVEN (zero prod callers, no allow(dead_code), env never on wire); PTY test genuine (drives from DB-read spec, 0.05s). Rework #1 dispatched.

## L1/decision — 2026-07-19T02:27:56Z (LANDED)
Rework #1 verified: three-probe union (key-rule first via redact_value one-entry-object probe), corpus differential test (5 confirmed leak cases + 2 empty-value pins all NAME-ONLY, benign controls verbatim), malformed-env_json degradation test. Patch applied CLEAN on d484e59. Gate land-588-L1 EXIT 0.
