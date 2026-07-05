# Human Smoke Checklist

Human verification is the scarce resource in the autonomous loop. It is **batched at phase
boundaries, never blocking per-issue**: agents close work on the automated verify gate and
append any human-only residue here; the human burns the pending items down in one sitting.

## The convention

**Who appends.** Any agent (wave agent, orchestrator, reviewer) that ships work whose
verification is intrinsically human-only — perception ("does the banner actually feel
non-intrusive?"), real hardware (Touch ID, phones, USB keys), OS-mediated dialogs that
automation can't drive, or subjective feel (TUI latency/contrast). Do **not** block or hold
open an itr issue for these: close the issue on the automated gate, append an item here in
the right phase section, and reference the item from the issue's close-reason.

**When the human burns it down.** At phase boundaries per the program order in
[`docs/ROADMAP.md`](../ROADMAP.md) — when a phase epic closes (or at a natural milestone
like a sprint review), the human runs **one session** covering every pending item in the
"burn down now" section. Items for unshipped functionality stay parked in their future-phase
section until that phase lands.

**How sign-off is recorded.** Per item: tick the checkbox, fill the Evidence slot (screenshot
path, itr note link, result table), and date the sign-off line. If an item **fails**, leave the
box unchecked, file an itr issue with the repro, and link it in the Evidence slot. After each
session, add a row to the Burn-down log at the bottom. Items are never deleted — signed-off
items stay in place as the record; superseded items get struck through with a pointer.

**Related references.** The detailed browser-smoke procedure (commands, expected screens,
failure modes) lives in [`docs/plan-mobile-device-pairing.md`](../plan-mobile-device-pairing.md)
("LocalLAN browser smoke procedure"); checklist items link to it rather than duplicating steps.
Known classes of bug that manual smoke catches and unit tests can't are tracked in itr#327
(Origin/Host, HTTP/2 `:authority`, probe races, IP-literal RP IDs).

## Item template

Copy this block into the appropriate phase section:

```markdown
### <Short title> (added YYYY-MM-DD, source: itr#NNN / commit <sha>)
- **Steps:** how to reproduce, or a link to the documented procedure
- **Expected:** the observable result that counts as a pass
- **Evidence:** _screenshot / itr note / result-table link goes here_
- [ ] Verified — signed off: _YYYY-MM-DD, name_
```

---

## Burn down now (shipped functionality, pending human verification)

### macOS notification perception (added 2026-07-03, source: `crates/wisphive_daemon/src/notify.rs`)
- **Steps:** With the daemon running in active mode, trigger a gated tool call from a Claude
  Code session (e.g. a `Bash` decision that needs review). Observe the macOS banner
  (`osascript display notification` path). Repeat a few times during normal work.
- **Expected:** Banner actually appears, arrives promptly (within ~1–2 s of the hook
  blocking), is readable (tool name + input context visible, secrets redacted), and is
  non-intrusive — informational only, does not steal focus, does not become annoying at
  realistic decision volume.
- **Evidence:** _screenshot of banner + subjective note_
- [ ] Verified — signed off: _______

### TUI feel: latency, information density, color/contrast (added 2026-07-03, source: `crates/wisphive_tui`)
- **Steps:** Run `wisphive tui` in your real daily terminal (not a screenshot harness) with a
  live queue: several pending decisions, agents panel populated, a terminal session attached.
  Exercise `a`/`d`, bulk `A`/`D`, `/` filter, Tab panel switching, detail views.
- **Expected:** Keypress-to-render feels instant; queue and detail views show full untruncated
  input without feeling cramped; colors/contrast are legible in your actual terminal theme;
  every keybinding available in a view is visible in that view's status bar.
- **Evidence:** _subjective note + screenshot of queue and detail views_
- [ ] Verified — signed off: _______

### TLS-on-LAN browser trust UX (self-signed cert warnings) (added 2026-07-03, source: `crates/wisphive_web/src/tls.rs`)
- **Steps:** Start `wisphive daemon start --web` (LocalLAN profile, self-signed cert). From
  another device or browser profile on the LAN, open `https://<lan-ip>:3100`. Note the
  interstitial each browser shows (Chrome, Firefox, Brave) and how many clicks it takes to
  proceed; confirm the SPA works after accepting.
- **Expected:** The warning is survivable by a non-expert (documented click-path works in each
  browser); after acceptance the app functions normally, and the LAN-IP origin correctly hides
  the passkey enroll button (LocalLAN profile gates passkeys to localhost origins).
- **Evidence:** _per-browser notes / screenshots_
- [ ] Verified — signed off: _______

### LocalLAN passkey matrix: Firefox + Brave on real hardware (added 2026-07-03, source: itr#323)
- **Steps:** Execute the LocalLAN smoke procedure in `docs/plan-mobile-device-pairing.md` on
  Firefox and Brave at `https://localhost:3100`: set password → enroll passkey (real
  Touch ID) → logout → login-with-passkey → dashboard. Run the §5 edge-case matrix
  (LAN-IP origin hides enroll, throttle countdown, skip-enroll path) on all three browsers.
- **Expected:** Full round-trip green in both browsers; edge-case matrix matches documented
  behavior. Results table appended as an itr note to #323 (the canonical record).
- **Evidence:** _itr#323 note link_
- [ ] Verified — signed off: _______

### Command Center inbox: real daily two-session perception (added 2026-07-04, source: itr#438/#399, Sprint-2)
- **Steps:** Automated §10 runtime evidence already captured — `crates/wisphive_web/frontend/e2e/inbox-command-center.spec.ts`
  drives a REAL `wisphive daemon start --web` (isolated HOME) with the REAL `wisphive-hook`
  binary authoring deferred + auto-approved `events.jsonl` records across two projects,
  proving all five #438 ACs + the `wisphive audit` oracle; screenshots in
  `sprint/sprint-2-2026-07-03-command-center-inbox/blitz/evidence/`. This human item is the
  residual perception pass: run `wisphive daemon start --web`, open the Inbox in a real
  browser, and work with **two genuine concurrent Claude/Codex sessions** in different
  projects during normal use. Trigger a real gated decision in one and a real AskUserQuestion
  in the other.
- **Expected:** The gated decision surfaces within ~5 s with a live-ticking age and correct
  project/session label; in-console approve unblocks the agent and clears the row. The real
  AskUserQuestion surfaces as a deferred "waiting in your terminal" row showing the actual
  question text/options; the go-to-terminal pointer (or Focus terminal for wisphive sessions)
  lands you where you can answer. The auto-answer feed and `0 waiting · N auto-answered…`
  header read true at a glance. Overall: the inbox _feels_ like a trustworthy single pane, not
  a lagging mirror.
- **Evidence:** _automated: e2e spec + `blitz/evidence/*.png` (attached to itr#399); human: subjective note + screenshot from a real two-session session_
- [ ] Verified — signed off: _______

### Terminal touch-to-scroll on a real phone/tablet (added 2026-07-05, source: itr#445, commit <sha>)
- **Steps:** On a real touch device (or Chrome DevTools mobile emulation with touch enabled),
  open the Terminals view, attach to a running session and **generate scrollback while attached**
  (run e.g. `seq 400` inside it — see caveat). Vertical-drag up/down inside the terminal pane.
  Then tap the pane and type a command on the on-screen keyboard.
- **Expected:** Dragging down reveals earlier scrollback (content follows the finger); dragging
  back down returns to the live tail. The page/outer pane does **not** scroll instead of the
  terminal. After scrolling, tap-to-focus and on-screen-keyboard input still work — no gesture
  gets stuck, no accidental text selection during the drag. Automated proof exists
  (`TerminalView.test.tsx` asserts the drag drives xterm's public `term.scrollLines()`; a real-app
  Playwright CDP-touch test confirmed a finger-drag scrolls a live terminal's scrollback — xterm 6
  uses a custom scrollable and its own touch Gesture does NOT scroll this build, so the handler is
  required); this item covers only real-hardware touch feel that automation can't.
- **CAVEAT (itr#284, not this item):** re-attaching a terminal (switching away and back) restores
  only the current screen — no scrollback — so **both wheel and touch have nothing to scroll after
  a switch** until you generate new output. That is the server-authoritative-scrollback-on-attach
  epic, not a touch bug. Test touch on a terminal whose scrollback you produced since attaching.
- **Evidence:** _phone/tablet screenshots (before/after scroll) + subjective note_
- [ ] Verified — signed off: _______

---

## Phase 5 — Remote access: scrollback + mobile pairing (upcoming; park until the phase lands)

### Phone pairing over LAN with TLS cert trust (source: itr#283/#284, #271/#272)
- **Steps:** Once the pairing chain ships: `arm` pairing from desktop (sudo-gated), scan the
  QR with a real Android phone (Chrome Android), complete `/pair` on the phone over the LAN,
  accept/trust the cert path in use, confirm the phone receives live queue events and full
  session scrollback (itr#284 mechanism); then revoke from desktop and confirm the phone's
  WS disconnects within one broadcast cycle.
- **Expected:** Full arm → scan → pair → live-inbox loop works on a physical phone; cert trust
  UX is survivable; revoke disconnects promptly. (iOS Safari is out of v1 per itr#283.)
- **Evidence:** _phone screenshots / itr note_
- [ ] Verified — signed off: _______

### Enterprise passkey matrix: Chrome/Firefox + mkcert + wisphive.test (source: itr#316, blocked by itr#270)
- **Steps:** Once `--tls-cert`/`--tls-key` wiring lands (itr#270): mkcert a local CA + cert
  for `wisphive.test`, start with `--auth-profile enterprise --auth-rp-id wisphive.test`, and
  run set password → enroll passkey → logout → login-with-passkey in Chrome and Firefox on
  real hardware (real Touch ID / OS authenticator, not a virtual authenticator).
- **Expected:** Full flow passes in both browsers under the trusted-cert enterprise profile.
- **Evidence:** _itr#316 close-reason / result table_
- [ ] Verified — signed off: _______

### iPhone as cross-device passkey authenticator (source: itr#283 area)
- **Steps:** From desktop Chrome on the enroll screen, choose the cross-device (hybrid/QR)
  path instead of local Touch ID; scan with a real iPhone and complete enrollment, then
  login-with-passkey via the same cross-device path.
- **Expected:** Enrollment and login complete using the iPhone as the authenticator. Note:
  this is iPhone-as-authenticator only — browsing from iOS Safari remains out of v1.
- **Evidence:** _screenshots / notes_
- [ ] Verified — signed off: _______

---

## Signed off

### Chrome desktop LocalLAN passkey happy path — real Touch ID (source: itr#315)
- **Steps:** LocalLAN smoke procedure, Chrome desktop, macOS host (documented in
  `docs/plan-mobile-device-pairing.md`).
- **Expected:** set password → enroll Touch ID → logout → login-with-passkey → dashboard.
- **Evidence:** itr#315 close-reason (7-step result table; 4 in-sprint fixes: 76a4536,
  c3913cb, 081b9d8, caf896d).
- [x] Verified — signed off: 2026-05-17, Product Owner (pre-dates this checklist; recorded
  retroactively from itr#315).

---

## Burn-down log

| Date | Phase boundary | Items covered | Who | Notes |
|------|----------------|---------------|-----|-------|
| 2026-05-17 | Sprint-1 review | Chrome LocalLAN passkey happy path (itr#315) | Product Owner | Pre-convention session, recorded retroactively |
