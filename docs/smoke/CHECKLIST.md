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
- [√] Verified — signed off: josef

### TUI feel: latency, information density, color/contrast (added 2026-07-03, source: `crates/wisphive_tui`)
- **Steps:** Run `wisphive tui` in your real daily terminal (not a screenshot harness) with a
  live queue: several pending decisions, agents panel populated, a terminal session attached.
  Exercise `a`/`d`, bulk `A`/`D`, `/` filter, Tab panel switching, detail views.
- **Expected:** Keypress-to-render feels instant; queue and detail views show full untruncated
  input without feeling cramped; colors/contrast are legible in your actual terminal theme;
  every keybinding available in a view is visible in that view's status bar.
- **Evidence:** _subjective note + screenshot of queue and detail views_
- [√] Verified — signed off: josef

### TLS-on-LAN browser trust UX (self-signed cert warnings) (added 2026-07-03, source: `crates/wisphive_web/src/tls.rs`)
- **Steps:** Start `wisphive daemon start --web` (LocalLAN profile, self-signed cert). From
  another device or browser profile on the LAN, open `https://<lan-ip>:3100`. Note the
  interstitial each browser shows (Chrome, Firefox, Brave) and how many clicks it takes to
  proceed; confirm the SPA works after accepting.
- **Expected:** The warning is survivable by a non-expert (documented click-path works in each
  browser); after acceptance the app functions normally, and the LAN-IP origin correctly hides
  the passkey enroll button (LocalLAN profile gates passkeys to localhost origins).
- **Evidence:** _per-browser notes / screenshots_
- [√] Verified — signed off: josef

### LocalLAN passkey matrix: Firefox + Brave on real hardware (added 2026-07-03, source: itr#323)
- **Steps:** Execute the LocalLAN smoke procedure in `docs/plan-mobile-device-pairing.md` on
  Firefox and Brave at `https://localhost:3100`: set password → enroll passkey (real
  Touch ID) → logout → login-with-passkey → dashboard. Run the §5 edge-case matrix
  (LAN-IP origin hides enroll, throttle countdown, skip-enroll path) on all three browsers.
- **Expected:** Full round-trip green in both browsers; edge-case matrix matches documented
  behavior. Results table appended as an itr note to #323 (the canonical record).
- **Evidence:** _itr#323 note link_
- [√] Verified — signed off: josef

### Command Center inbox: real daily two-session perception (added 2026-07-04, source: itr#438/#399, Sprint-2)
- **Steps:** Automated §10 runtime evidence already captured — `crates/wisphive_web/frontend/e2e/inbox-command-center.spec.ts`
  drives a REAL `wisphive daemon start --web` (isolated HOME) across two projects, proving all
  five #438 ACs + the `wisphive audit` oracle on live (non-fixture) daemon data. Provenance is
  mixed by design: the deferred (AC2) and auto-approved (AC3, which drives the AC4 header count)
  `events.jsonl` records are authored by the REAL `wisphive-hook` binary running its real
  always-defer / auto-approve classification. AC1's gated human-review queue item comes from a
  real-wire socket client fixture (`e2e/fixtures/hook-client.ts`), **not** the hook binary —
  under `auto_approve_level:all` the real hook auto-approves everything and so cannot produce a
  human-review row. Screenshots in
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
- **Evidence:** _automated: e2e spec + `blitz/evidence/*.png` (attached to itr#399)._
  _human (2026-07-19, josef): mechanics verified on the live installed build (daemon on current
  binary, web :3100). **Pass:** deferred AskUserQuestion surfaced as a read-only "waiting in your
  terminal" row — correct `wisphive · session cc-d2809` label, live-ticking age, full untruncated
  question + all 4 options; header read true (`0 waiting · 1 in your terminal · 17 auto-answered in
  last hour`); a gated Bash decision (forced to human review by `allow_self_modification:false`,
  real hook — no fixture) surfaced in the queue and in-console approve unblocked the session.
  Backend confirmed healthy end-to-end from `events.jsonl` line 15 (`decided_by:always_ask:intrinsic`)
  → daemon tailing (open FD) → web serving. **Fail:** the "Answer in your wisphive terminal" /
  Focus-terminal pointer is a dead affordance — clicking it just collapses the row instead of
  focusing the terminal → filed **itr#622**. **Untested:** the two-genuine-concurrent-sessions
  "single pane, not a lagging mirror" feel — both event types were exercised from one session
  sequentially, not two live agents at once._
- [ ] Verified — **partial, held open**: mechanics pass; blocked on itr#622 (dead terminal pointer)
  + residual two-session concurrency feel. Sign off after itr#622 lands and a real two-session
  working session confirms the single-pane feel. _(2026-07-19, josef)_

### Terminal touch-to-scroll on a real phone/tablet (added 2026-07-05, source: itr#445, commit <sha>)
- **Steps:** On a real touch device (or Chrome DevTools mobile emulation with touch enabled),
  open the Terminals view, attach to a running session and **generate scrollback while attached**
  (run e.g. `seq 400` inside it — see caveat). Vertical-drag up/down inside the terminal pane.
  Then tap the pane and type a command on the on-screen keyboard.
- **Expected:** Dragging down reveals earlier scrollback (content follows the finger); dragging
  **up** returns to the live tail. The page/outer pane does **not** scroll instead of the
  terminal. After scrolling, tap-to-focus and on-screen-keyboard input still work — no gesture
  gets stuck, no accidental text selection during the drag. A **tiny jitter** (a ~6-8px finger
  slip that is not enough to scroll a whole row) followed by a lift must **still focus** the
  terminal — the drag must not swallow that near-tap (itr#480). Automated coverage today is
  **unit-only**: `TerminalView.test.tsx` asserts the drag drives xterm's public `term.scrollLines()`
  with the right signed row delta, but against a mock (jsdom does no layout) — it does **not** prove
  the real viewport scrolls. Real-app touch scroll was verified **interactively during development**
  (isolated daemon+web over TLS, live PTY, Playwright CDP `Input.dispatchTouchEvent`), but that
  harness was scratch-only and is **not committed** — a committed regression is tracked in **itr#479**.
  Context: xterm 6 uses a custom scrollable and its own touch Gesture does NOT scroll this build, so
  the JS handler is required. This item covers only real-hardware touch feel that automation can't.
- **CAVEAT (itr#284 — not this item):** re-attaching a terminal (switching away and back) restores
  only the current screen — no scrollback — so **both wheel and touch have nothing to scroll after
  a switch** until you generate new output. That is the server-authoritative-scrollback-on-attach
  epic, not a touch bug. Test touch on a terminal whose scrollback you produced since attaching.
  **Confirmed by instrumentation 2026-07-19** (itr#624): after refresh+attach the client buffer
  measured `baseY=0, length=54 (== rows)` while xterm's scroll dimensions were healthy — there was
  literally nothing to page into. **Separately, itr#623:** a replay request from a web device for a
  TUI/CLI-created session (authored `human:tui`) is *actively denied* by the replay ACL with no grant
  path — a distinct second defect, not the cause of the empty-buffer symptom above.
- **Evidence:** _human (2026-07-19, josef): tested on a REAL Android/Chrome device (in scope per
  itr#283) plus Chrome DevTools emulation. **FAIL.** A vertical touch-drag makes content move, but it
  is the same rows sliding around — no previously-unseen earlier text is ever revealed, so the xterm
  buffer never pages. **ROOT CAUSE (proven by instrumentation) — this is NOT a touch bug and NOT a
  sizing race:** immediately after a page refresh with NO resize, **wheel/trackpad scroll fails
  too**. The attach catchup carried only the current vt100 screen, so the client buffer held **zero
  scrollback** — measured `baseY=0, length=54 (== rows)` while xterm's scroll dimensions were
  *healthy*. Nothing to page into, by any input method. Once new output existed on the same page,
  `baseY=303` and both wheel and touch paged **without any resize**. The touch handler is innocent
  and was never modified. "Resizing fixes it" was a red herring — resize reflow manufactured exactly
  **one line** (`baseY 0→1`); note this is **not fully reconciled** with the lived impression that
  resize restored real scrolling, and remains an open loose end. This is the **itr#284** epic.
  Filed **itr#624**. Ruled out with evidence: itr#623 no-scrollback-after-reattach
  (scrollback WAS generated on the same page without switching), zero-height viewport (live DOM
  `.xterm-viewport` clientHeight 930, and it is an empty vestigial div — real content is in sibling
  `.xterm-scrollable-element`), CSS/event plumbing (pinch-zoom works), iOS Safari scope exclusion
  (device is Android/Chrome). **Note:** DevTools emulation appeared to "work" — emulated touch is
  **not** a valid oracle for this feature; only real hardware is._
- [ ] Verified — **FAILED**, blocked on itr#624 (touch never pages scrollback) and itr#623 (replay
  denied on re-attach). Do not sign off until both land and a real device reveals earlier scrollback
  text. _(2026-07-19, josef)_

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

## Mission no-silent-failures — post-install verification (added 2026-07-19; park until the operator runs ./install.sh)

All mission code was verified against isolated temp HOMEs and `./target/release` binaries per the
standing rule; these items burn down the residue that only the operator's real install can prove.

### Pinned terminal session survives a real daemon restart (source: itr#589/#590/#591, commit 0378792)
- **Steps:** After installing the new binaries (`./install.sh`, operator-only): `wisphive term new`
  a session, run something with visible output, pin it (TUI `p` or web star), then
  `wisphive daemon stop && wisphive daemon start`. Attach to the session.
- **Expected:** The session is Running again (starred), shows pre-restart scrollback with the
  process-loss honesty banner, and accepts live typing. An unpinned control session shows as
  orphaned/killed, not respawned. `web_audit` (sqlite3 -readonly) carries `terminal_respawn`
  outcome rows.
- **Evidence:** _subjective note + `wisphive term list` before/after_
- [√] Verified — signed off: josef

### Stopping an agent kills its whole process tree (source: itr#561, commit 25432ff)
- **Steps:** After install: start a managed agent that launches a long-running tool subprocess
  (e.g. a build), `wisphive agent stop <id>`, then `pgrep -g <pgid>` / check for surviving
  grandchildren.
- **Expected:** The stop takes ≤ ~4 s, reports success, and the entire tree is gone — no orphaned
  build/npm processes. A stop that cannot confirm whole-tree death reports a loud error naming
  the pgid instead of claiming success.
- **Evidence:** _shell transcript_
- [√] Verified — signed off: josef

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
| 2026-07-19 | Sprint-2 residue | Command Center inbox two-session perception (itr#438/#399) — partial: mechanics pass, filed itr#622 (dead terminal pointer), two-session feel untested | josef | Live build; deferred + gated loops both proven with the real hook |
| 2026-07-19 | Sprint-2 residue | Terminal touch-to-scroll (itr#445) — FAILED, filed itr#624 + itr#623 | josef | Real Android/Chrome; DevTools emulation gave a false pass |
