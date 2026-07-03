# ADR-0006: Decision-plugin trust model — observer-only, redacted, env-var shell hooks

- **Status:** Proposed
- **Date:** 2026-07-03
- **Deciders:** Josef Aguilar (PO), Claude (trust-model pass, itr#423)
- **itr:** #423, #425
- **Related:** ADR-0001 (tiered fail posture), ADR-0005 (policy-learning invariants)

## Context

The decision-plugins plan (`docs/plan-decision-plugins.md`) adds webhooks, shell hooks, and
an RPC agent bridge to the control plane. Extension points in a security gate ossify once
shipped, so the trust boundaries had to be decided before the API. The adversarial pass
found: webhook templates would render from the in-memory, **unredacted** `DecisionRequest`
(bypassing the itr#89 scrubber that protects persist/notify); shell-hook template
interpolation is an injection channel authored by the gated agent no escaping fully closes;
config hot-reload turns one approved `config.json` write into a silent live exfiltration
feed (SSRF included); the plan had no posture classification for plugins that could
influence decisions; and the RPC bridge sketch drifts toward the daemon executing tools
itself.

## Decision

Plugins are governed by T1–T7 in the plan's Trust Model section:

1. **T1** — only **observer** plugins exist (watch, never influence); dispatched after the
   decision is resolved and persisted; fire-and-forget. Decision-influencing plugins
   require a successor ADR and would be fail-closed with a hard time budget.
2. **T2** — plugins see **post-redaction** data only; `tool_input` excluded from templates
   by default behind per-hook `include_tool_input: true`.
3. **T3** — shell hooks get variables as **environment variables**; the config command
   string executes verbatim with zero interpolation.
4. **T4** — https/localhost-only webhook URLs with link-local blocking; itr#425 ships
   before decision hooks; every hot-reloaded `decision_hooks` change is audited and raises
   a banner + OS notification naming the destinations.
5. **T5** — fail postures: observer failures are fail-open (telemetry tier of ADR-0001);
   malformed hook config disables all hooks with a banner; RPC bridge protocol errors are
   fail-closed per tool call.
6. **T6** — every dispatch is audit-stamped (event, destination host/command name, active
   `config_hash`); spawned plugin binaries carry spawn-trust, no signing regime.
7. **T7** — the RPC bridge signals approve/deny; the agent process executes its own tools.
   Daemon-side tool execution is out of scope and needs its own ADR.

## Rationale

The gated agent authors most data flowing through this system (tool inputs, agent ids) and
can plausibly obtain a single approved config write; the trust model therefore removes
whole attack classes structurally rather than sanitizing at the edges — env-vars instead of
escaping (T3), redaction-boundary reuse instead of new scrubbing paths (T2),
observer-only instead of a plugin veto/approve API (T1), and signaling instead of moving
code execution into the control plane (T7). Fail postures reuse ADR-0001's existing tiers
rather than inventing new ones.

## Consequences

- Easier: webhook/shell-hook implementation has crisp acceptance criteria; Slack-notify
  use cases ship without a sandbox runtime; ADR-0001's posture table extends cleanly.
- Harder / costs: decision hooks are blocked on itr#425; shell hooks lose inline
  templating ergonomics (scripts must read env vars); webhook bodies carrying tool input
  require explicit opt-in; any future policy-plugin ambition must clear a new ADR.
- The hot-reload watcher gains audit + notification responsibilities (small but real).

## Alternatives considered

- **Escape shell template variables (single-quoting)** — rejected: escaping adversary-
  authored strings into shell is a perennial CVE factory; env-vars eliminate the class.
- **Send unredacted tool input to webhooks with a doc warning** — rejected: recreates the
  leak itr#89 closed, on a network egress path.
- **Decision-influencing plugin API now (veto/approve callbacks)** — rejected: highest
  blast radius for no current user need; observer hooks cover the requested workflows.
- **Restart-only hook config (no hot reload)** — considered as the simpler T4; rejected in
  favor of audited + notified hot reload because restart-gating only narrows, not closes,
  the window while costing the main UX benefit.
- **Daemon-side tool execution in the RPC bridge** — rejected: concentrates arbitrary code
  execution inside the control plane; signaling preserves today's trust geometry.

## Links

- Plan: `docs/plan-decision-plugins.md` (Trust Model section, T1–T7)
- Code (governed, future): `wisphive_daemon/src/decision_hooks.rs`,
  `wisphive_daemon/src/rpc_bridge.rs`
- itr: #423, #425
