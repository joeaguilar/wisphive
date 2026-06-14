# ADR-0003: Enterprise auth profile is non-functional until user-cert TLS lands

- **Status:** Accepted
- **Date:** 2026-06-14
- **Deciders:** Josef (PO)
- **itr:** #310 (auth-profile selection), #270 (user-provided TLS — pending)
- **Related:** —

## Context

Web auth/security posture is selected at startup via `--auth-profile {local-lan|enterprise}`
(default `local-lan`) rather than a single locked posture (see `~/.claude` memory
`feedback_auth_profile_module.md`). The `enterprise` profile is meant to require a real,
user-provided TLS certificate plus an explicit WebAuthn RP ID (`--auth-rp-id <domain>`). The
user-provided-TLS wiring (itr#270) has not landed yet, so the cert side of `enterprise` cannot
actually be honored — but the CLI flag and profile plumbing (itr#310) already exist.

## Decision

Until itr#270 lands, config validation for `--auth-profile enterprise` checks the TLS flags
**first** and **always fails** with `MissingTlsFlags`, regardless of whether `--auth-rp-id` is
supplied. The `enterprise` profile is therefore non-functional by design in this window;
`local-lan` remains the only usable profile. `--auth-rp-id` under `local-lan` is ignored with a
warning.

## Rationale

Failing fast and loudly at startup is safer than half-wiring enterprise auth and letting an
operator believe they have certificate-backed enterprise security when they do not. Checking the
TLS flags before the RP-ID makes the failure deterministic and the error message accurate — the
genuine blocker is the missing user-cert support, not the RP ID. Shipping the profile-selection
plumbing (itr#310) ahead of the TLS work (itr#270) keeps the auth surface organized around the
`AuthProfile` abstraction without pretending the enterprise path is ready.

## Consequences

- Documentation must state plainly that `enterprise` is non-functional regardless of
  `--auth-rp-id` until itr#270; `CLAUDE.md`/`AGENTS.md` already do.
- When itr#270 lands, flip this ADR's status to *Superseded by ADR-XXXX* (the ADR that records
  the user-cert TLS design) and update `resolve_auth_profile` / `validate_enterprise_config` so
  the `MissingTlsFlags` short-circuit is replaced by real validation.

## Alternatives considered

- **Hide the `enterprise` flag entirely until TLS lands** — rejected: the profile-selection
  architecture is real and worth landing early; a loud, documented failure is clearer than a
  hidden flag.
- **Check RP ID before TLS flags** — rejected: would produce a misleading "missing RP ID" error
  when the true blocker is missing user-cert support.

## Links

- Code: `crates/wisphive_cli/src/main.rs` (`resolve_auth_profile`),
  `wisphive_web::validate_enterprise_config`
- itr: #310 (landed), #270 (pending — TLS user-cert wiring)
- Spec: `CLAUDE.md` → `wisphive_cli` crate entry (auth-profile note)
- Memory: `feedback_auth_profile_module.md`
