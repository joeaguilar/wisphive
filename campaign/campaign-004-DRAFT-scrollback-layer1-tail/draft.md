# campaign-004 DRAFT — scrollback + Layer-1 tail (scout-roadmap, campaign-003, 2026-07-15)

**Status: DRAFT ONLY — not approved, not executed.** Produced by campaign-003's scout-roadmap. PO decision needed on the werkit-tracker question below before this becomes a plan.

## Recommended G

> Land server-authoritative terminal scrollback (Phase 5, epic #284) — #285→#286→#287 in three waves, closing #284/#289 with real-device evidence — and ride along the Layer-1 inbox tail (#441 searchable audit surface, #449 stale deep-link if not tied off by #464).

## Candidate queue

| Title | Source | Why now | Observable AC | Likely files | Size | Risk | In-004? |
|---|---|---|---|---|---|---|---|
| State-of-play Stop-hook writer + session-start render | werkit#6 / Phase 4 | True next in program order; §6.1 hook-safety gate cleared by #396 | End real session → `docs/state-of-play.md` correct; new session renders it first | werkit Stop-hook + session scripts | L | med | yes* (werkit tracker) |
| TermAttach: optional `from_seq` protocol field | itr#285 / Phase 5 | Unblocked head of scrollback chain | `TermAttach { id, from_seq }` round-trips Some/None; old clients parse | wire.rs, protocol.ts | S | low | yes (W1) |
| Daemon: bounded-tail replay on TermAttach | itr#286 | The actual scrollback fix; blocked-by #285 (same campaign) | Fresh attach shows tail; re-attach zero dup frames; cap degrades gracefully | daemon server/terminal/state | M | med | yes (W2) |
| Web: last-seen seq + `from_seq` on re-attach | itr#287 | Closes user-visible scrollback bug; blocked-by #285/#286 | Terminal switch preserves xterm scrollback; refresh recovers | useWisphive.ts, TerminalView.tsx, Terminals.tsx | M | med | yes (W3) |
| Docs: scrollback privacy stance | itr#289 | Cheap tail; lets epic #284 close clean | plan-mobile-device-pairing.md gains session-privacy subsection | docs | S | low | yes (tail) |
| Auto-answer feed → searchable audit surface | itr#441 | Extends shipped Layer-1 inbox; backend path scoped | Search + filter (project/rule/time) + pagination over full decision_log | daemon audit query + frontend feed | M/L | med | yes (ride-along) |
| Deferred deep-link stale-session feedback | itr#449 | Small correctness gap in deferred inbox | Stale focus-terminal shows toast; onFocusHandled fires; vitest branch | Terminals.tsx, Inbox.tsx | S | low | maybe (check #464 overlap; also campaign-003 Q-5 candidate) |
| Auto-clear deferred inbox items | itr#440 | Prereq regression #473 fixed in campaign-003 | Deferred row vanishes after native answer (installed binary) | verify-only | S | low | no (folded into c-003 via #473) |
| Devices UI + TUI event surfacing | itr#220 | Blocker for pairing #283 + enterprise #313 | Device list loads; revoke logs device out; TUI login-failure entries | Devices.tsx, tui/app.rs | M/L | med | maybe (needs-sprint decompose) |
| Upstream(itr): `--db` / `ITR_DB` | itr#476 | werkit#7 digest needs multi-DB addressing | Flag/env precedence defined; verbs work; invalid path fails loud | itr repo (cross-repo) | M | low | no (different repo; before werkit#7) |
| Remote answering Ask/ExitPlanMode | itr#439 | Reverses ADR-0002; security design first | none pullable — needs ADR spike | ADR + hook | XL | high | no (decompose after ADR) |
| WS reconnect backoff + rehydration | itr#104 / Phase 7 | Reliability under Layer-1 console | Kill daemon 60s → reconnect + queue/terminals rehydrated | useWisphive.ts | M | med | no (off program order) |

\* werkit#6/#7/#8 live in `werkit/.itr.db`, not wisphive itr; werkit#7 blocked-by #6.

## PO decisions needed before approval

1. **Tracker boundary:** Phase 4 (werkit#6, program-order next) lives in the werkit tracker. Run a werkit-scoped campaign for it, or take the wisphive-scoped scrollback G above and let Phase 4 be its own campaign?
2. #439 needs an ADR spike before any implementation campaign.
3. #220 / #476 need decomposition (needs-sprint / cross-repo).
