# Provider source contract — Codex app-server & Claude hooks (source-verified)

**Date:** 2026-07-16 · **Method:** four authoritative-source agents.
**Sources:** `openai/codex` @`315195492c` (real Rust, `inspiration/codex/codex-rs/`) and
`anthropics/claude-code` (public repo — **contract docs/examples/plugins only, engine is closed**,
`inspiration/claude-code/`). This doc is the citation anchor for the ADR-0009 amendment (itr#569) and
the gate-anchoring ADR (itr#572). Line numbers are from files the agents read.

Companion to [`t3code-recon.md`](t3code-recon.md), which studied a third-party *consumer* of these
interfaces. This studies the interfaces themselves, in the vendors' own source.

## 1. `codex app-server hooks/list` — replaces the emulation (itr#569)

- **Not a re-derivation.** `hooks/list` and the live hook engine call the *same* `discover_handlers`
  (`hooks/src/registry.rs:213` ← `catalog_processor.rs:649`; engine at `hooks/src/engine/mod.rs:126`).
  One loop (`discovery.rs:510-546`) builds both the reported inventory and the executed subset from
  one config walk. The inventory provably equals what the session does.
- **Fields all confirmed** (`app-server-protocol/.../v2` + `plugin.rs:512-526`): `enabled`, `source`,
  `sourcePath`, `isManaged`, `pluginId`, `trustStatus`, `eventName`, `matcher`, `command`,
  `timeoutSec`, `currentHash`, plus `key`/`handlerType`/`statusMessage`/`displayOrder`.
- **`HookSource` enumerates the "non-enumerable" layers.** `protocol.rs:1530-1545`: `System User
  Project Mdm SessionFlags Plugin CloudRequirements CloudManagedConfig LegacyManagedConfigFile
  LegacyManagedConfigMdm Unknown(#[default])`. ADR-0009:155-157's "non-enumerable trust roots" are
  enumerated; `Unknown` degrades loudly (anti-desync). **ADR-0009 option (a) was rejected on a false
  premise** — it never opened the JSON-RPC protocol.
- **Not experimental at the method level** (`common.rs:673`, no `#[experimental]`) — two agents
  concur. The `[experimental]` label is on the app-server transport broadly.
- **Corrections to bake into the ADR/ACs:**
  - `enabled:true` ≠ executes. Real predicate: `enabled && (bypass_hook_trust || trust ∈
    {Managed,Trusted})` (`discovery.rs:527-533`). Managed hooks are always `enabled`+`Managed`.
  - **Wisphive's own fresh gate is `Untrusted`** (`hooks_list.rs:159-189`) — a blanket
    "untrusted ⇒ refuse" self-blocks. See §2.
  - `currentHash` hashes the hook **definition** (event+matcher+command string+timeout,
    `discovery.rs:570-587`), **not the referenced script's bytes** — script-content TOCTOU uncovered.
  - `errors[]` (config load failed) ⇒ hard fail-closed; `warnings[]` softer ⇒ surface, not auto-stop.

## 2. Codex hook trust — closes itr#471's blast radius structurally (itr#570)

- `--dangerously-bypass-hook-trust` runs **every enabled hook** regardless of trust
  (`discovery.rs:527-545`, threaded at `:147`) — #471 confirmed; blast radius is "all *enabled*
  hooks", not just the managed one.
- Trust logic (`discovery.rs:589-603`): `Managed` if managed; else `trusted_hash==current_hash ⇒
  Trusted`, present-but-differs ⇒ `Modified`, absent ⇒ `Untrusted`. `trusted_hash` lives in
  `[hooks.state]` (`config/src/hook_config.rs:32`), writable via `config/batchWrite`
  (`hooks_list.rs:537-558`).
- **The better design:** write Wisphive's own hook's `currentHash` into `hooks.state.trusted_hash`,
  then spawn **without** bypass. Codex runs the Wisphive hook (now `Trusted`) and refuses any foreign
  `Untrusted`/`Modified` hook via its *own* trust gate. Removes the blast radius instead of fencing
  it; `codex_allow_foreign_hooks` becomes unnecessary.

## 3. Codex app-server as a session transport — unsafe without a three-part audit (itr#566)

- **`thread/start` carries the steering surface as stable fields** (`v2/thread.rs:56-148`):
  `approval_policy`, `approvals_reviewer` (incl. `AutoReview` = an LLM that auto-approves),
  `sandbox` (incl. `DangerFullAccess`), and `config: HashMap`. `resume`/`fork` carry the same;
  `turn/start` re-asserts per turn (`v2/turn.rs:109-116`).
- **`config` reaches the gate.** `config_manager.rs:224-241` extracts `bypass_hook_trust`, then
  `json_to_toml`s **every** remaining key with no allow/deny-list (`json-to-toml/src/lib.rs:5-27`),
  writing any dotted path (`overrides.rs:7-13`). So a start param can set `hooks.*`/`features.*` and
  `bypass_hook_trust:true` — the exact keys the argv audit refuses, which passes vacuously because
  argv is just `["app-server"]` (`app-server/src/main.rs:19-59`).
- **Sibling methods bypass the turn/hook plane entirely** (all stable, `common.rs`): `command/exec`
  (:1069), `thread/shellCommand` (:588, "runs unsandboxed with full access"), `fs/writeFile` (:755),
  `fs/remove` (:775), `config/value/write` (:1142), `config/batchWrite` (:1148, persists to
  `config.toml`, disabling the gate for **future** sessions). The Wisphive hook fires only on
  model-initiated PreToolUse *within a turn*.
- **Approval plane: `approvalPolicy=Never` is not a sufficient kill switch.** It silences exec+patch
  (`safety.rs:57`, `exec_policy.rs:180/734`), but three channels fire on separate switches:
  `item/tool/requestUserInput` (`experimental_request_user_input_enabled`),
  `item/permissions/requestApproval` (`Feature::RequestPermissionsTool`),
  `mcpServer/elicitation/request` (any eliciting MCP server). Four switches, not one — so #566's
  "treat any inbound approval as a security event" is **necessary**.
- **Server-side floor exists but is not a free pass:** `requirements.toml`/MDM clamps
  approval/sandbox/reviewer and can force `allow_managed_hooks_only` (`config_requirements.rs:1302-1398`,
  applied last and wins, client cannot disable — `state.rs:40-46`). But **off by default**
  (`allow_any`), and covers neither the arbitrary `config`-key vector nor the sibling methods.
- **Contract class: DOCUMENTED-BUT-UNVERSIONED.** Schema is binary-specific (`README.md:57`);
  `initialize` negotiates no numeric version (`v1.rs:29-74`). Breaking-change detection must be
  schema/capability probing that fails loud.

## 4. Claude hook contract vs Wisphive's reverse-engineered doc

- **Confirmed by Anthropic's shipped `security-guidance` hook:** PostToolUse field is `tool_response`
  (`security_reminder_hook.py:913,925-933`) — Anthropic's own *teaching* docs mislabel it
  `tool_result` (`SKILL.md:316`), so "verify against the docs" hits a false contradiction; the
  shipped hook is authority. `hookSpecificOutput`, `permissionDecision` allow/deny/ask,
  `updatedInput`, `additionalContext`, exit-2+stderr, `hooks.json` shape: all confirmed.
- **Unsupported (absent, not contradicted — reverse-engineered):** `permissionDecisionReason`,
  and the `PermissionRequest` response field names `decision.behavior`/`updatedPermissions`. Re-probe
  before relying on exact names (itr#571).
- **Managed/MDM tier above same-UID reach** (`examples/mdm/README.md:15-24`): root-owned
  `managed-settings.json`, top of precedence, `allowManagedHooksOnly` /
  `allowManagedPermissionRulesOnly` / `disableBypassPermissionsMode:"disable"`. Lets a "Chat inherits
  the gate above the agent's reach" design be expressed — but **hard-vs-advisory enforcement of a
  managed hook is closed engine behavior**; take on documented word, verify by probe.

## The cross-provider principle (itr#572)

Both providers offer a way to **anchor the gate above the agent's same-UID write reach** rather than
fence it: Codex `requirements.toml`/`allow_managed_hooks_only` + per-hook trust-registration; Claude
root-owned `managed-settings.json` + `allowManagedHooksOnly`. Both are **operator-provisioned, off by
default, and incomplete alone** — defense-in-depth, not a replacement for the hook. And app-server has
**two uses**: a Wisphive-authored short-lived control-plane process (hooks/list, trust-write — safe,
no agent on the pipe) versus an agent session transport (unsafe without pinning every steering field,
allowlisting the `config` map, and allowlisting the client→server method set).
