# ADR-0008: Same-UID config tampering — tamper-evidence, not tamper-proofing

- **Status:** Accepted
- **Date:** 2026-07-11
- **Deciders:** Josef Aguilar (PO) + Fable review session
- **itr:** #96 (descoped by this ADR), #159 (duplicate), #508 (sprint-4 epic)
- **Related:** ADR-0001 (tiered fail posture), ADR-0002 (always-defer classification)

## Context

itr#96 reports that a malicious local process can write `auto_approve_level: all`
into `~/.wisphive/config.json` and silently auto-approve every gated tool call
(CVSS ~5.5, source: backend-security-auditor). The prescribed fix was: 0600 on
write, verify mode+owner on read, atomic writes, passive notification on policy
widening, and **signing config.json with a daemon-managed key**, defaulting safe
on mismatch. The acceptance criterion required that a non-daemon config write be
"ignored until user-confirmed".

Sprint-4 blitz Wave 5 quarantined the issue: the signing scheme cannot deliver
what it promises. The attacker in this threat model runs **as the same UID as
the user**. Such a process can:

1. read any file-backed signing key (same UID reads everything the daemon reads);
2. `chmod`/`chown` files back into whatever shape a verifier expects;
3. edit `~/.claude/settings.json` / `.codex/hooks.json` to **remove the Wisphive
   hook entirely** — no gate, no config needed;
4. replace the `wisphive-hook` or `wisphive` binaries in `~/.cargo/bin`;
5. write `off` into `~/.wisphive/mode` (the documented kill switch);
6. kill the daemon (hook fails open on daemon-unreachable, per ADR-0001 — by
   design, so a crashed control plane can't brick every agent).

Config signing with a same-UID-readable key closes one of six equivalent doors
and adds a trusted-writer protocol spanning daemon, CLI, web, wire protocol, and
notification surfaces. A macOS-only real boundary exists in principle (Keychain
item ACL'd to the codesigned binary), but it still leaves doors 3–6 open, has no
Linux equivalent, and is far beyond the C2 sizing of #96.

## Decision

Wisphive's protection boundary **excludes malicious code running as the
operator's own UID**. Against same-UID tampering, Wisphive provides
**tamper-evidence (detection + safe-defaulting), not tamper-proofing**:

1. Config files are written 0600 and atomically (already shipped: itr#92/#308).
2. Readers verify ownership and permissions before trusting `config.json` /
   `auto-approve.json`; a loose file (group/world-writable or foreign-owned) is
   treated as absent — the hook falls back to safe defaults and the daemon
   raises an alert. Warn-and-fail-safe, not hard refusal.
3. The daemon watches `config.json` and fires a passive notification plus an
   audit event whenever the effective policy **widens** (level increase, new
   auto-approve/always-ask-remove entries, dangerous posture, self-modification
   opt-in).
4. Cryptographic config signing is **rejected**, not deferred: any key readable
   by the attacker authenticates nothing, and the unhook/binary-replace bypasses
   make the residual value ~zero. If a future milestone wants a real same-UID
   boundary it must be designed whole (OS credential store + codesign ACLs +
   hook-integrity attestation) as its own epic — not bolted onto config I/O.

## Rationale

- **Honesty over theater.** Shipping a signature check that same-UID malware
  trivially bypasses would let us claim a security property we do not have.
  The quarantine note's analysis is correct: same-UID provenance cannot be
  authenticated with owner/mode checks or a file-backed key.
- **Detection is real value.** Malware can rewrite config.json, but it cannot
  unsend a notification the daemon already fired or unwrite an audit event.
  The widening alert also catches the far more likely *accidental* case — a
  stray script, a confused agent, or the operator themselves fat-fingering
  `auto_approve_level`.
- **Fail-safe beats fail-refuse.** Treating a loose config as absent degrades
  to `auto_approve_level: off` (everything queues for human review) instead of
  bricking agents or — worse — trusting the file anyway. A hard refusal gated
  by a flag *inside the distrusted file* would be circular.
- **The existing audit trail already anchors this.** Every hook decision
  carries `config_hash` (itr#397), so a widened config is correlatable with
  every decision it produced after the fact.

## Consequences

- itr#96 is descoped to: read-side ownership/permission verification, the
  policy-widening watcher + notification + audit event, and an inter-process
  lock closing the `update_config_json` lost-update race. Its original
  acceptance criterion ("ignored until user-confirmed") is rewritten — see the
  issue.
- The threat-model line moves into AGENTS.md / docs: Wisphive gates
  *cooperative-but-fallible agents*; it is not an anti-malware control for
  code already executing as the operator.
- Future security reviews should not re-file config-signing bugs; they should
  cite this ADR. A genuine same-UID boundary requires a dedicated epic
  (Keychain/secret-service key custody, codesign-scoped ACLs, hook and binary
  integrity attestation) and an explicit decision that the residual bypasses
  (mode file, settings unhook, daemon kill) are also in scope.
- Operators on multi-user machines get a real (not theatrical) improvement:
  foreign-owned or group/world-writable config is never trusted.

## Alternatives considered

- **Daemon-managed signing key on disk** (the original #96 prescription) —
  rejected: same-UID attacker reads the key and re-signs; pure theater.
- **macOS Keychain key + codesigned-binary ACL** — a real boundary for door #1
  only, macOS-only, and useless while doors 3–6 (unhook, binary replace, mode
  file, daemon kill) remain open. Rejected as a #96-sized fix; noted as the
  starting point if a dedicated integrity epic is ever commissioned.
- **Daemon-mediated config writes only (hook refuses operator-edited file)** —
  breaks the documented workflow (`config.json` is user-editable per CLAUDE.md),
  requires a trusted-writer protocol across CLI/web/daemon, and still fails
  against doors 3–6. Rejected.
- **Hard refusal (hook exits deny) on loose permissions** — punishes the
  accidental case with a bricked agent instead of routing to human review;
  contradicts the ADR-0001 posture of degrading legibly. Rejected in favor of
  treat-as-absent + alert.

## Links

- Code: `crates/wisphive_hook/src/main.rs` (config readers),
  `crates/wisphive_daemon/src/config.rs` (`write_config_atomic`,
  `update_config_json`), `crates/wisphive_daemon/src/disk_alert.rs`
  (alert-latch pattern the watcher follows)
- itr: #96, #159, #508
- Quarantine analysis: itr#96 note dated 2026-07-11 (Blitz Wave 5 triage)
