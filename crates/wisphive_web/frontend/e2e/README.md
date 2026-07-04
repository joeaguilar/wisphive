# Web e2e authoring notes

Playwright specs for the Wisphive web UI. Each spec boots its **own** real
`wisphive` process against an isolated `$HOME` temp dir (never `~/.wisphive`) and
an ephemeral port — see `helpers/server.ts` (standalone `web serve`),
`fixtures/daemon-server.ts` (real daemon + queue), and `fixtures/hook-client.ts`
(a blocking, real-wire hook that injects `DecisionRequest`s).

## Run it — and when you MUST

```bash
just e2e                      # build dist + wisphive bin, then run all specs
cd crates/wisphive_web/frontend && npx playwright test <name>   # one spec
```

**e2e is NOT in the per-story gate.** The close gate most work runs is
`cargo test` + `clippy` + `fmt` + `frontend-lint` + **vitest** — e2e lives only in
the full `just verify`. So a change to any **user-visible default, view/tab, or
row/detail rendering** can leave this suite red while every per-story gate is
green. Run `just e2e` after such changes. This has bitten us repeatedly:

- itr#435 flipped the default view Queue→Inbox → broke `core-flows`/`smoke`
  (`.queue-layout` on load) — filed+fixed as itr#446.
- commit `8f41f1a` made a *selected* inbox row render the full `DetailView`
  instead of inline actions → broke the inbox smoke's approve selector.

## Gotchas that will bite you

1. **Sudo-class tools pop the reauth modal.** `Bash`, `Write`, `Edit`,
   `MultiEdit`, `NotebookEdit` are sudo-class (`sudo_gate.rs`): a **web** approve
   of them fires `WebReauthRequired` → the `SudoModal`, not a resolution. For a
   plain approve/deny round-trip use a **non-sudo** tool (`Read`, `Grep`,
   `Glob`). To test the sudo path itself, drive the `SudoModal` (`.sudo-form`,
   `input[type=password]`, `.login-submit`).

2. **The default SPA view is the Inbox, not the Queue** (itr#435). After
   login/set-password you land on `.inbox`. Specs asserting `.queue-layout` /
   `.queue-item` must click the **Queue** nav first
   (`getByRole('button', { name: /^Queue/ }).click()`).

3. **Selecting an inbox row swaps inline actions for the full DetailView**
   (`8f41f1a`, honoring the no-truncation rule). A collapsed `.inbox-item` has
   inline `.inbox-actions .btn-approve`; once **selected** it renders
   `.inbox-detail-full > .detail-view` whose approve is
   `.detail-actions .btn-approve`. Approve either from the collapsed row (don't
   select) or from the detail (after selecting) — not both.

4. **Click a button-free region to *select* a row.** `row.click()` targets the
   element center, which can land on a `stopPropagation` action button (→
   approves instead of selecting). Click `.inbox-item-topline` (or another
   button-free child) to expand.

5. **Reauth freshness is per-device and server-side** (`ReauthRegistry`, 5-min
   TTL) — not client state. To keep a device fresh across tests (so a second
   sudo action doesn't re-prompt), **share one device token** across them;
   otherwise expect the `SudoModal` each time.

6. **Always-defer tools never reach the daemon queue.** `AskUserQuestion` /
   `ExitPlanMode` / `Elicitation` are deferred at the hook (ADR-0002) and surface
   only as `deferred` `events.jsonl` records. To make them appear in the inbox,
   author a real record by feeding the **real `wisphive-hook` binary** a
   `PreToolUse` event on stdin (see `inbox-command-center.spec.ts`) — a socket
   `DecisionRequest` will NOT produce one.

7. **Filesystem-mutating web actions are sudo-gated.** `install_hooks` (cockpit
   gating, itr#460) writes `.claude/settings.json` — a web device with stale
   reauth is bounced with `WebReauthRequired` and nothing is written. Verify the
   real on-disk write, not just the badge (see `hook-gating.spec.ts`).

## Evidence

Specs that back an itr close write screenshots to
`sprint/<sprint>/blitz/evidence/` and attach them to the Playwright report.
Prefer asserting the **real effect** (a socket resolution settling, a file on
disk) over a UI label alone.
