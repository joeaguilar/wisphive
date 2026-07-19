# Chain 602 premise — web error surfacing → spawn regression

**IMMUTABLE.** Rescope = supersede via a new epic + `itr relate --type supersedes`.

## Ordering-claim

itr#565 ("Spawn Agent form does nothing", critical, PO-reported) cannot be verifiably
diagnosed or fixed before itr#567 lands, because the web UI discards every
`ServerMessage::Error` (`useWisphive.ts:421-423`, `term_error` at `:447-449`) and never sets
`correlation_id` — no failure, including a spawn refusal, can reach the operator's eyes.
Falsifier: a #565 root cause is found that is fully client-side and provable without any
error surfacing (e.g. the `send()` no-op path alone) — then the ordering was convenience,
not necessity.

## Value-claim

Landing both links resolves a PO-reported critical regression AND leaves behind a general
error-surfacing substrate (banner + correlation) that every future daemon error uses.
Falsifier: error surfacing lands but #565's root cause turns out to be daemon-side spawn
brokenness out of chain territory — value then requires a successor chain, not this one.

## Links (serial)

1. **L1 = itr#567** — route generic `error`/`term_error` into the existing banner pattern
   (`DiskAlertBanner`/`ConfigAlertBanner`, `useWisphive.ts:463-488`); browser sets
   `correlation_id` on outbound ws messages; spawn submit awaits correlated result.
2. **L2 = itr#565** — root-cause with file:line + repro (candidates: `send()` ws-not-OPEN
   no-op `:617-620`; pending-approval not rendering in Agents view; post-approval hook-gate
   refusal on unhooked project), then fix; spawn → running agent OR visible specific error.
   Terminal link: e2e (`just e2e`) + runtime-verified evidence. User-visible work goes live
   here (this chain's whole point is surfacing — banner from L1 may ship live immediately;
   it is itself the fix).

## Assumptions

| id | text | tag |
|---|---|---|
| A-A1 | #567 is the root cause of #565's *invisibility*; "fix #567 first" (note 218) is correct | verified-at-intake (note 218 + code) |
| A-A2 | `useWisphive.ts:421-423`/`447-449` log-and-drop all daemon errors | verified-at-intake |
| A-A3 | `correlation_id` exists on the wire (`wire.rs:318`) and daemon threads it (`server.rs:1288, 2343-2350`); browser is the only client omitting it | verified-at-intake |
| A-A4 | #565's root cause is among: send() no-op, pending-approval rendering, hook-gate refusal | assumed-by-default (until L2 diagnosis) |
| A-A5 | banner pattern is reusable for generic errors; no toast lib needed | verified-at-intake |
| A-A6 | e2e harness can prove error surfacing despite landmines (real hook binary for always-defer, SudoModal class) — `e2e/README.md` | assumed-by-default |

## Kill criteria

- Link rework ≥ 2 → council.
- Reviewer `contradicts` on any assumption → council.
- L2 diagnosis finds root cause outside chain territory (daemon spawn internals beyond the
  handler) → council (likely rescope to successor).
- Every 5 landed links → council (n/a here; chain is 2 links).

## Territory

`crates/wisphive_web/frontend/src/**` (esp. `hooks/useWisphive.ts`, `App.tsx`,
`components/SpawnModal.tsx`, `components/Agents.tsx`, banner components),
`crates/wisphive_web/src/ws_bridge.rs`, `crates/wisphive_web/frontend/e2e/**`,
`crates/wisphive_daemon/src/server.rs` **spawn-handler + correlation regions only**
(~`:1288`, `:2343-2377`) — shared file with chain 603, regions disjoint (X-A1).

## Budget

Mission budget unset; no per-chain allocation. Rework/contradicts criteria still fire.
