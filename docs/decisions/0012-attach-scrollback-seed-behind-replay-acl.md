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
- **Seam (live):** exactly-once at the seam is established **atomically**
  (itr#626 fix; see Correction 1 for the original, falsified mechanism). The
  session's parser mutex doubles as the *sequence-assignment lock*: every
  `next_seq()` for a live session is taken under it, and the reader's vt100
  update + seq assignment form one critical section. `TerminalSession::
  attach_with` captures the boundary `N`, the screen snapshot, the broadcast
  subscription, and the output high-water mark in ONE critical section under
  that same lock (no awaits inside). Hence: a frame with `seq < N` already
  has its bytes in the snapshot and is dropped by the forwarder's `seq < N`
  filter if its broadcast is still seen; a frame with `seq >= N` is assigned
  — and therefore broadcast — strictly after the subscription exists. The
  seed query keeps the **exclusive `before_seq = N` bound**, and the attach
  first waits (bounded, 500 ms) for the db batcher's persistence watermark to
  reach the boundary's high-water mark, so the seed covers every output frame
  below `N` (within the seed budget); on timeout it degrades loudly and the
  screen repaint stays authoritative.
- **Seam (repaint):** the vt100 screen snapshot that follows the seed
  self-prefixes `ESC[H ESC[J` (home + erase-below), which cannot scroll
  content into or wipe the client's freshly seeded scrollback.
  > **⚠ CORRECTED — holds only from a neutral parser state, which the seed
  > does not guarantee. See [Correction 1](#correction-1-2026-07-19-two-invariants-above-are-not-achieved-as-implemented)
  > and itr#627.**
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
  exclusivity, the erase-below-only repaint, and (itr#626) the atomicity of
  boundary + snapshot + subscription under the sequence-assignment lock —
  in particular, never assign a seq or mutate the parser for a live session
  outside that lock, and never re-snapshot the screen after the boundary
  critical section.

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
  `crates/wisphive_daemon/src/terminal.rs` (`attach_with`, `ingest_output`,
  `catchup_message`, `ATTACH_SCROLLBACK_SEED_MAX_BYTES`),
  `crates/wisphive_daemon/src/state/terminals.rs` (`tail_terminal_output`)
- Tests: `crates/wisphive_web/frontend/e2e/touch-scroll.spec.ts`,
  `crates/wisphive_web/frontend/e2e/desktop-wheel.spec.ts`; itr#626 seam
  tests in `crates/wisphive_daemon/src/terminal.rs`
- itr: #624 (root cause + fix), #284 (mechanism delivered), #479 (coverage),
  #623 (remaining grant-path work), #626 (seam atomicity fix)
- Commit: e3d29b9

## Correction 1 (2026-07-19): two invariants above are NOT achieved as implemented

An independent review (Codex `gpt-5.6-sol`) of the implementing commit `e3d29b9`, run hours after
this ADR was accepted, falsified two of the guarantees stated above. The **decision** stands — seed
the attach catchup, gate it behind the replay ACL, bound it, output-direction only. What was wrong
was the claim that the chosen mechanism *delivers* certain properties. Recorded here rather than
quietly edited, because an ADR that asserts a safety property the code does not have is worse than
no ADR at all.

**1. "Either in the seed or from the live forwarder, never both" is false (itr#626).**
The exclusive `before_seq` bound separates *persisted seed rows* from *live frames*, but it does not
make the parser snapshot, the sequence boundary, and the broadcast subscription **atomic**. The
`TermAttach` arm captures `next_seq`, then performs several awaited DB operations and writes the
catchup, and only subscribes afterwards. In that window a frame can be broadcast with no receiver
attached and excluded from the seed by `seq < N` — **dropped from both paths**. Conversely, because
the producer updates the parser *before* assigning a sequence and broadcasting, a preemption there
lets the snapshot already contain a frame that is then also delivered live — **duplicated**. A
correct fix must establish the subscription and the snapshot atomically with respect to the sequence
boundary (e.g. subscribe first, then snapshot, then de-duplicate by `seq`).

> **RESOLVED (2026-07-19, itr#626).** The seam is now atomic; the Decision's "Seam (live)" bullet
> above restates the invariant as implemented. Mechanism: the parser mutex became the
> sequence-assignment lock (reader ingest fuses vt100 update + seq assignment in one critical
> section; input/resize assignments take the same lock), and `TerminalSession::attach_with` captures
> boundary + snapshot + subscription + output high-water mark in one critical section under it —
> neither Codex shape alone sufficed: subscribe-first (+ `seq` de-dup) fixes the drop but cannot see
> that a snapshot already contains a frame whose seq is not yet assigned, so the duplicate needed
> the fused producer-side critical section as well. A bounded (500 ms) wait for the batcher's
> persistence watermark before the seed read closes the residual scrollback gap ("not-yet-persisted
> frame scrolled off screen vanishes from the attachment"). Proven by deterministic red→green
> reproductions of both interleavings plus a mid-stream exactly-once acceptance test in
> `crates/wisphive_daemon/src/terminal.rs` (`frame_arriving_during_catchup_delivery_is_delivered_
> exactly_once`, `attach_overlapping_reader_ingest_delivers_exactly_once`,
> `attach_mid_stream_reconstructs_exactly_the_produced_stream`). Frames below the boundary that the
> seed's byte/row budget trims remain out of scope here (bounded seed by design; full history stays
> behind `TermReplay`), as do itr#627 (ANSI split at the seed boundary) and itr#630 (budget
> semantics).

**2. The `ESC[H ESC[J` repaint is only authoritative from a neutral parser state (itr#627).**
The sequence itself is sound, but the seed is trimmed at **frame** boundaries, and frames are
arbitrary ~4096-byte PTY reads — not ANSI boundaries. A seed ending inside an OSC/DCS string (whose
terminator lives in an excluded frame) leaves the client parser mid-string, so the repaint bytes are
consumed as *payload* rather than executed. The missing invariant is that the seed guarantees a
neutral parser state before the repaint.

**Also corrected:** the `ATTACH_SCROLLBACK_SEED_MAX_BYTES = 256 KiB` bound is enforced by a
**512-row prefetch** trimmed afterwards, so it is neither a reliable 256 KiB tail (many small frames
yield far less) nor a strict cap (the newest frame is admitted even when it alone exceeds the
budget) — itr#630.

**What the review confirmed as sound:** seed bytes are read only *after* the ACL decision; the SQL
bound is correctly exclusive and output-direction-only; two simultaneous attaches do not share
attachment handles; and no terminal payload bytes leak into logs or replay-audit detail.

**Silent denial.** This ADR documents that an unauthorized attach silently degrades to a screen-only
catchup. ADR-0013 later asserted that denials "remain audited," which contradicts this. That
contradiction is real and is resolved in favour of auditing — see ADR-0013 Correction 1 and itr#629.

## Correction 2 (2026-07-19): the restated invariant is STILL conditional

`dc63cad` (itr#626) fixed the original drop/duplicate interleavings via producer-side atomicity, and
Correction 1's RESOLVED block claims the invariant now holds. A second independent review found that
claim is **still overbroad**. Recorded rather than edited, for the same reason as before.

**1. `seq >= N` frames can still be dropped — receiver overflow (itr#626, reopened).** `attach_with`
subscribes, then leaves the receiver **undrained** across the 500 ms persistence wait, the seed
query, the catchup build, and its awaited delivery. The forwarder only begins receiving after
`attach_with` returns. The broadcast ring holds 256 frames of up to 4096 bytes, so ~>1 MiB of output
during that window yields `Lagged`, and the server responds by telling the client to re-attach and
terminating the forwarder. **A subscription existing is not the same as delivery being exactly
once.** The invariant holds only in the absence of receiver lag.

**2. The seed's completeness rests on a watermark that is not contiguous (itr#633).** A failed insert
batch is discarded, and a later successful batch advances the watermark via `fetch_max` — so it marks
*latest persisted*, not *contiguously persisted*, while the seed logic consumes it as the latter.

**3. The timeout path degrades silently (itr#632)**, and the wait runs *before* the ACL decision, so
a stalled batcher taxes even attaches that will be denied.

**Wording fix:** the invariant's "every `seq < N` frame has bytes in the snapshot" is literally
false for **input** frames, which are never parser content. It should read "every *output* frame".

**Also noted:** the preservation rules ("never assign a seq or mutate the parser outside the lock")
are documented but **not type-enforced** — `next_seq()` does not require a parser guard — so
correctness rests on module discipline rather than the compiler. Worth making structural.

**Standing lesson for this ADR:** three rounds of stated invariants have now each been narrower in
practice than in prose. Future edits should state the *conditions* under which a guarantee holds
rather than asserting it unconditionally.
