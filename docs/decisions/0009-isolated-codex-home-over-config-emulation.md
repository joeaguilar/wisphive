# ADR-0009: Isolated daemon-controlled CODEX_HOME to minimize Codex config emulation

- **Status:** Proposed
- **Date:** 2026-07-14
- **Deciders:** PO review pending (drafted from itr#528, sprint-5 crossfire-blitz round-3 review)
- **itr:** #528, #511, #471
- **Related:** ADR-0008 (same-UID tamper-evidence, not tamper-proofing)

## Context

itr#511's Codex managed-spawn hook audit (`audit_codex_effective_hooks` in
`crates/wisphive_daemon/src/process_registry.rs`) must prove, before spawning a managed Codex
child, that (1) the Wisphive PreToolUse gate is present **and enabled** in the child's effective
hook inventory, and (2) no un-vetted foreign hook will run headlessly — because the spawn passes
`--dangerously-bypass-hook-trust`, which suppresses Codex's trust prompt for *every* enabled hook
(itr#471; released only by the `codex_allow_foreign_hooks` opt-in).

To prove that, the audit re-derives Codex's *effective* configuration from the outside. At the
time of this decision, the containing file exceeded 5,500 lines, split roughly evenly between
implementation and tests. It reimplements, from observed behavior of an evolving third-party
binary: semver parsing and precedence for plugin active-version selection, plugin manifest shape
resolution, TOML profile layering
(`$CODEX_HOME/<name>.config.toml` chased from `profile = ...`), kill-switch precedence
(`features.hooks` vs the deprecated `features.codex_hooks` alias, `allow_managed_hooks_only`),
and the persisted `/hooks`-disablement `hooks.state."<key>"` key format.

Each of these is an emulation of undocumented or partially documented internals. A future Codex
release that changes any of them desyncs the audit, and **desync in the under-detection direction
is a control-plane bypass**: an "audited clean" spawn whose child actually loads a hook source the
audit didn't model, or skips the gate the audit thought was enabled. Round-3 review found three
concrete instances (disableAllHooks scope ambiguity, `features.hooks`/`codex_hooks` precedence,
persisted-disable state-key format) — all since resolved toward fail-closed conservatism, but the
structural fragility remains: the audit's correctness is coupled to a moving target we don't
control, and the failure mode of that coupling is silent.

Why the audit is shaped this way today: the daemon already pins the child to a captured
`CODEX_HOME` (`cmd.env("CODEX_HOME", &self.codex_home)`) and enumerates the session-flag source
against its own argv (`audit_codex_session_argv`) — but that `CODEX_HOME` is the *user's real
one* (`~/.codex` or inherited env), so everything under it (user `hooks.json`, `config.toml`,
profiles, `requirements.toml`, the plugin cache) is agent-writable state the audit must model.

Facts about the installed Codex CLI (0.144.1, from `--help` surfaces only):

- **No `codex config` subcommand exists** — there is no `codex config effective-hooks` or other
  authoritative, non-interactive, machine-readable effective-inventory surface that the daemon
  can use as a pre-spawn gate. The subcommand list is: exec, review, login, logout, mcp, plugin,
  mcp-server, app-server, remote-control, app, completion, update, doctor, sandbox, debug, apply,
  resume, archive, delete, unarchive, fork, cloud, exec-server, features.
- **Interactive `/hooks` inspection does exist.** It lets an operator inspect configured hook
  sources, review trust, and disable hooks in the CLI. It is not a stable non-interactive output
  or machine-readable effective-inventory API, so it cannot replace the daemon's pre-spawn proof.
- **`codex doctor --json`** emits a "redacted machine-readable report" of "installation, config,
  auth, and runtime health" — its help makes no mention of hooks; it is not an effective-hook
  inventory surface today.
- **`codex features list`** reports "known features with their stage and effective state" — this
  *is* authoritative introspection for the `features.hooks` kill switch specifically, but covers
  nothing else (no hook inventory, no plugins, no `hooks.state`).
- **`codex exec --ignore-user-config`** skips `$CODEX_HOME/config.toml` entirely ("auth still
  uses `CODEX_HOME`") — a partial isolation lever that already exists.
- **`-c key=value` / `--enable <FEATURE>`** are session-level config overrides, documented as
  overriding "a configuration value that would otherwise be loaded from `~/.codex/config.toml`".

## Decision

Spawn managed Codex children into a **daemon-provisioned, isolated `CODEX_HOME`** that the daemon
writes and therefore fully owns — eliminating the user-level configuration surface (user
`hooks.json`, `config.toml`, profiles, `requirements.toml`, plugin cache, persisted
`hooks.state`) from the audit entirely, because there is nothing there the daemon didn't write.
Isolation does not preserve user capability by itself: plugins, personal config, and MCP servers
disappear from managed children by default. Selected capability is re-grantable via an
operator-vetted import list behind a named opt-in flag. In parallel, file an upstream Codex
feature request for an effective-hook-inventory introspection surface and adopt it as a
cross-check if it lands; do not block on it.

## Rationale

The three candidate directions, and why (b) wins:

**(a) Query Codex for its own effective inventory.** This is the ideal end state — the same code
that will load the hooks reports what it will load, so emulation drift is impossible by
construction. But **an authoritative non-interactive surface does not exist** (interactive
`/hooks` is operator-facing; there is no `codex config` subcommand; `doctor --json` doesn't cover
hooks), so this option reduces to filing an upstream feature request whose timeline we don't
control. Even when granted, it has residual weaknesses as a *primary* gate: a TOCTOU window
between the introspection invocation and the spawn (same class the current
`AuditSnapshot` re-verify narrows today), and dependence on the stability of an output schema.
It is an excellent *cross-check* (defense-in-depth that would have caught all three round-3
desyncs), not a foundation we can build on this quarter. The one fragment that exists today —
`codex features list` for the hooks kill switch — is worth adopting opportunistically, and the
`--enable hooks` / `-c` session-override mechanism converts kill-switch *detection* (emulating
precedence) into kill-switch *assertion* (the daemon's own argv wins), which is strictly less
emulation.

**(b) Isolated daemon-controlled `CODEX_HOME`.** The mechanism already half-exists in the spawn
path: the daemon already sets `CODEX_HOME` on the child; pointing it at a directory the daemon
provisions (minimal `config.toml` with hooks enabled and no profile, no user `hooks.json`, no
plugins, no `requirements.toml`) deletes the *reason* for most of the emulation. What the audit
must still model shrinks to the **project-level** sources, which are irreducible: the Wisphive
gate itself lives in project `.codex/hooks.json` (written by `wisphive hooks install`), and
project `.codex/config.toml` can still carry kill switches and foreign inline hooks. But
project-file auditing is the *original*, well-understood core of the audit (shared with the
Claude Code path via `inspect_hook_content`) — it is the user-level layering, profile chase, and
plugin semver/manifest machinery that ballooned it and that isolation deletes.

The honest cost of (b) is capability loss: a managed Codex child no longer sees the operator's
plugins, personal config, or MCP servers. Per the repo convention ("preserve capability behind a
named opt-in flag"), this is re-granted explicitly rather than silently: a
`codex_managed_home_import` allowlist in `~/.wisphive/config.json` naming the plugins/config
fragments the operator has vetted, which the daemon copies into the controlled home at
provision time — vetted-by-construction, the same trust shape as `codex_allow_foreign_hooks`.
Two wrinkles:

- **Auth.** Codex auth lives under `CODEX_HOME` (the `--ignore-user-config` help confirms "auth
  still uses `CODEX_HOME`"). The daemon must seed auth material (e.g. `auth.json`, mode 0600)
  into the controlled home at spawn, and token refresh may write back there, drifting from the
  real home. Mitigation: re-seed per spawn from the real home (real home stays canonical), or
  copy refreshed credentials back; either is bounded, known work — not an open-ended emulation.
- **ADR-0008 still applies.** The agent runs as the operator's UID and can rewrite the controlled
  home mid-session just as it can rewrite `~/.wisphive`. Isolation does **not** upgrade the
  same-UID trust model to tamper-proofing; it removes the *pre-spawn* dependence on emulating
  the resolution of state the daemon never wrote. The `AuditSnapshot` verify→spawn→verify TOCTOU
  narrowing stays, and verifying the controlled home is trivial: assert the daemon-written bytes
  are unchanged, rather than re-deriving what a foreign layering engine would make of them.

**(b-lite) `--ignore-user-config` on the existing real home.** Cheaper (S): one argv flag, no
auth seeding. But it removes only `$CODEX_HOME/config.toml` — user `hooks.json`,
`requirements.toml`, and (help is silent on this) possibly plugin state remain live, so most of
the emulation surface survives. Worth taking *immediately* as a stopgap hardening only if the
full isolated home is deferred; it is not the destination.

**(c) Status quo** — keep hardening the emulation. Rejected: three under-detection desyncs in one
review round is the empirical failure rate of this approach at Codex 0.144.x; each future Codex
release re-rolls those dice, and the audit only fails safe when we correctly *anticipate* the
ambiguity. The several-thousand-line audit surface is a symptom, not the disease.

## Consequences

- The Codex spawn path gains a provisioning step (create/refresh controlled home, seed auth,
  apply the vetted import list) and a new config key (`codex_managed_home_import`, default
  empty). CLAUDE.md's runtime-files and config sections must document both.
- The audit's scope contract changes from "everything Codex might read" to "project-level
  `.codex/` sources + integrity of the daemon-written home". `audit_codex_effective_hooks`'s
  user-config layering, profile chase, `requirements.toml` scan, and the entire plugin
  semver/manifest/active-version machinery become deletable — an estimated majority of the
  implementation and security tests retarget or go.
- **Transition discipline: the existing audit is kept intact as defense-in-depth until the
  isolated home has shipped and soaked** (it still audits the controlled home — everything
  should come back clean, and any finding is a bug in provisioning). Only then is it shrunk to
  project-scope + home-integrity. It is never fully deleted: project files and the TOCTOU
  snapshot re-verify remain load-bearing.
- Managed Codex children lose implicit access to user plugins/config; operators who need them
  must vet and list them. This is a deliberate posture improvement (headless spawns run only
  reviewed code) but will surface as "my plugin doesn't run under Wisphive" support friction.
- Residual limits are unchanged and inherited from today's audit: enterprise system/MDM/cloud
  config layers outside `CODEX_HOME` and server-injected remote `extra_plugins` remain
  non-enumerable trust roots; `--dangerously-bypass-hook-trust` semantics and the
  `codex_allow_foreign_hooks` gate (itr#471) are unaffected.
- We take a soft dependency on Codex honoring `CODEX_HOME` for all user-level reads. That is a
  documented, stable, coarse contract (one env var) — categorically easier to keep verified than
  the fine-grained resolution semantics we emulate today. A smoke test that spawns Codex with a
  canary `CODEX_HOME` and asserts no reads of the real home should pin it per Codex upgrade.

## Recommendation & sizing

**Phased hybrid, anchored on (b):**

1. **Isolated `CODEX_HOME` provisioning + auth seeding + `codex_managed_home_import` opt-in — M.**
   The env-pinning and audit plumbing already exist; the new work is directory provisioning,
   auth-material lifecycle, one config key, and tests. M because auth seeding/refresh-drift and
   the import copier need care, but every piece is daemon-owned code with no third-party
   semantics to reverse-engineer.
2. **Audit shrink to project-scope + home-integrity (after soak) — M.** Mostly deletion plus test
   retargeting; M rather than S because the large security-test surface must be consciously
   dispositioned, not bulk-deleted.
3. **Upstream ask + opportunistic adoption — S.** File the Codex feature request for
   `codex hooks list --effective --json` (or a hooks section in `codex doctor --json`); adopt
   `codex features list` as a kill-switch cross-check where cheap. Cross-check only, never the
   primary gate.

Net effect on the itr#528 concern: the only Codex internals still modeled are the project-file
formats Wisphive itself writes and shares with the Claude Code path — the entire user-level
resolution emulation (the source of all three round-3 desyncs) ceases to exist.

## Alternatives considered

- **(a) as primary: block on an authoritative Codex introspection surface** — rejected: no
  authoritative non-interactive, machine-readable surface exists in 0.144.1, and its delivery
  timeline is external; retained as a defense-in-depth cross-check to adopt when available.
- **(b-lite) `--ignore-user-config` only** — rejected as the destination (leaves user
  `hooks.json`/`requirements.toml`/plugin surfaces live); acceptable as an S-sized stopgap if
  phase 1 is deferred.
- **(c) keep hardening the emulation** — rejected: empirically produced three under-detection
  desyncs in one review round; each Codex release re-exposes the same silent-bypass failure mode.
- **Force-enable hooks via session overrides (`--enable hooks`) instead of detecting kill
  switches** — not adopted alone (it would also force-run foreign hooks an operator deliberately
  disabled), but noted as an assert-not-emulate lever that composes with (b), where the
  controlled home contains no foreign hooks to force.

## Links

- Code: `crates/wisphive_daemon/src/process_registry.rs` (`audit_codex_effective_hooks`,
  `build_agent_command`, `audit_codex_session_argv`, `AuditSnapshot::verify_unchanged`)
- itr: #528 (this ADR), #511 (the audit), #471 (`codex_allow_foreign_hooks`), #467 (silent
  hook-trust skip)
- ADR-0008 — same-UID tamper-evidence, not tamper-proofing
- Official Codex hooks documentation: <https://learn.chatgpt.com/docs/hooks>
- Codex CLI introspection ground truth: `codex --help`, `codex exec --help`,
  `codex doctor --help`, `codex features --help`, `codex plugin --help` at codex-cli 0.144.1
  (2026-07-14)
