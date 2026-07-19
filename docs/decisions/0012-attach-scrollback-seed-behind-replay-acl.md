# ADR-0012: Attach catchup seeds scrollback, gated by the existing replay ACL

- **Status:** Accepted
- **Date:** 2026-07-19
- **Deciders:** PO (Josef) + implementing session, out of the itr#624 investigation
- **itr:** #624, #284, #479, #623
- **Related:** ADR-0008 (config trust), itr#98 (replay audit/ACL), itr#591 (respawn seed)

## Context

A terminal attach (`TermAttach`) used to deliver only the current vt100 screen
(`contents_formatted()`). Consequence, confirmed by instrumentation during the
itr#624 investigation: after **any** page refresh or re-attach, the web
client's xterm buffer held zero scrollback — wheel and touch alike had
literally nothing to scroll into until new live output accumulated. On mobile
the dead gesture surfaced as "content slides but no earlier text ever
appears"; a window resize only *appeared* to repair it (reflow manufactures at
most ~1 line; catchup-painted rows truncate rather than reflow). The client
was exonerated: render dimensions, the touch handler, and xterm's scroll
machinery were all healthy — the data was simply absent.

The daemon already persists every output frame (`terminal_events`, used by the
audited `TermReplay` path and the itr#591 respawn seed), so the history exists
server-side. The forces in tension:

- **Utility:** an operator refreshing the page expects their terminal's
  scrollback to still be there (itr#284).
- **Security:** session history is more than the live screen — replay is
  ACL-gated and audited precisely because history can contain echoed input
  (itr#98). Serving history on attach to *any* authenticated device would
  silently widen that boundary.
- **Wire safety:** a seed must not corrupt the seam with the live stream or
  the screen repaint, and must be bounded — histories can be huge.

## Decision

`TermAttach` prepends a **scrollback seed** — the tail of persisted
output-direction rows — to the catchup screen, **iff the requester passes the
same authorization check as `TermReplay`** (session creator identity or
explicit `replay_acl` entry). Unauthorized requesters keep the legacy
screen-only catchup; the attach itself still succeeds.

Mechanics:

- **Bound:** `ATTACH_SCROLLBACK_SEED_MAX_BYTES = 256 KiB`, trimmed
  oldest-first — the same budget as the itr#591 respawn seed. Full,
  unbounded history remains exclusively behind the audited `TermReplay`.
- **Seam (live):** the seed query takes an **exclusive `before_seq` upper
  bound** at the `next_seq` captured for the catchup, so a frame can arrive
  either in the seed or from the live forwarder, never both (no duplicated
  output at the seam; a not-yet-persisted frame below `next_seq` is at worst
  absent from *scrollback* — the screen repaint stays authoritative).
- **Seam (repaint):** the vt100 screen snapshot that follows the seed
  self-prefixes `ESC[H ESC[J` (home + erase-below), which cannot scroll
  content into or wipe the client's freshly seeded scrollback.
- **Direction:** output-direction rows only — the itr#591 security invariant
  (input-direction bytes, e.g. no-echo passwords, are never seeded).

## Rationale

- History disclosure is **replay-class**, so it must sit behind the
  replay authorization — reusing the existing check adds no second policy
  surface to keep consistent, and the itr#98 threat model transfers as-is.
- The daemon is the only honest source of scrollback across
  refresh/re-attach; any client-side "fix" was impossible (the data is not
  there), and pretending otherwise is how itr#624 got three wrong diagnoses.
- The `before_seq` bound makes the seed/live seam deterministic instead of
  racy; the 256 KiB cap reuses an already-accepted budget rather than
  inventing a new knob.

## Consequences

- Refresh/re-attach now restores real, scrollable history for the session's
  creator device — verified end-to-end (wheel and touch, desktop and mobile
  emulation) by `e2e/touch-scroll.spec.ts` and `e2e/desktop-wheel.spec.ts`.
- **Creator-vs-non-creator asymmetry becomes visible:** a non-creator device
  (including the operator's own desktop browser looking at a phone- or
  TUI-created session) still gets screen-only, and the denial is silent at
  attach. This is the pre-existing cross-surface grant-path gap — tracked as
  **itr#623**, not resolved here; the asymmetry was PO-confirmed by a 4-cell
  device matrix during review (no ACL leak).
- Catchup messages grow (up to ~340 KiB base64 JSON) for seeded attaches.
- Future work touching attach, replay ACLs, or catchup format must preserve:
  the ACL gate on the seed, the output-only invariant, the `before_seq`
  exclusivity, and the erase-below-only repaint.

## Alternatives considered

- **Unbounded seed (full history on attach)** — rejected: unbounded message
  sizes on an interactive path, and it would collapse the distinction between
  attach and the audited, rate-limited `TermReplay`.
- **No ACL gate (seed for every authenticated device)** — rejected: silently
  converts attach into history disclosure, sidestepping the itr#98 posture;
  the itr#623 defect is fixed by building a *grant path*, not by removing the
  boundary.
- **Client-side reconstruction** — impossible: the buffer data does not exist
  in the browser after a refresh; only the daemon has it.
- **Seed without a `before_seq` bound** — rejected: frames persisted between
  `next_seq` capture and the seed read would render twice (seed + live
  forwarder), corrupting the seam.

## Links

- Code: `crates/wisphive_daemon/src/server.rs` (`TermAttach` arm),
  `crates/wisphive_daemon/src/terminal.rs` (`catchup_message`,
  `ATTACH_SCROLLBACK_SEED_MAX_BYTES`),
  `crates/wisphive_daemon/src/state/terminals.rs` (`tail_terminal_output`)
- Tests: `crates/wisphive_web/frontend/e2e/touch-scroll.spec.ts`,
  `crates/wisphive_web/frontend/e2e/desktop-wheel.spec.ts`
- itr: #624 (root cause + fix), #284 (mechanism delivered), #479 (coverage),
  #623 (remaining grant-path work)
- Commit: e3d29b9
