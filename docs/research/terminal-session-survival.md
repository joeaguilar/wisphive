# Terminal / agent session survival across a daemon restart

> **Status:** Research / design — no code changes.
> **Date:** 2026-07-17
> **Author:** Investigation via two read-only agents (wisphive terminal/daemon lifecycle map + herdr persistence/handoff map), synthesized with PO direction.
> **Companion:** [`herdr-ux-mining.md`](herdr-ux-mining.md) (broader herdr UX survey). herdr cloned read-only at `inspiration/herdr` (AGPL-3.0).
> **Tracking:** Filed as its own itr epic (Plan A / B / C phased children); cross-links epic #573's #580 (restart matrix), #581 (hot-swap spike), #582 (resume registry).

## The ask

Adopt herdr's model for terminals so the user can (1) **keep important sessions open** — already works today for client detach/reattach — and (2) **survive a wisphive restart**. This doc establishes exactly how far each system goes today and what it would take to close the gap, with the PO's decisions recorded.

## Reframe: "survive a restart" is two questions on two axes

Detach/reattach (a client closing while the daemon stays up) **already works** in wisphive — the daemon owns the PTY and clients subscribe to a broadcast channel, so a client leaving never touches the process. The remaining ask is surviving the **daemon process itself** going away. That splits on two axes:

|  | **Planned restart / upgrade** | **Crash (SIGKILL)** |
|---|---|---|
| **Live process must survive** (a running build / stateful REPL keeps going) | FD-handoff — **Plan B** (herdr's actual mechanism) | Only via a PTY host outside the daemon — **Plan C** (beyond herdr) |
| **Session just comes back** (same cwd, scrollback, agent conversation resumed; process was restarted) | Snapshot + respawn/resume — **Plan A** | Snapshot + respawn/resume — **Plan A** |

**Honest ceiling:** no system — *including herdr* — keeps a live process alive across an ungraceful crash. herdr's live-process survival is exclusively a graceful, opt-in, experimental in-process FD-handoff for planned upgrades. A `SIGKILL` of herdr's server loses the processes exactly like wisphive; it doesn't even persist `child_pid`.

## Current wisphive model (ground truth)

Terminals and managed agents are both **daemon-lifetime-scoped, in-memory-owned child processes**. Only audit byte-history survives a restart, as replay data.

- **PTY children are direct children of the daemon**, spawned via `openpty` + `spawn_command` (`crates/wisphive_daemon/src/terminal.rs:174-206`), sharing its process group, with **no `setsid` / `process_group` / `pre_exec`** separation. The daemon holds the sole master fd, so on daemon exit that fd closes and the child gets SIGHUP/EOF.
- **Graceful stop actively kills children:** `daemon stop` → SIGTERM → teardown runs `process_registry.shutdown_all()` then `terminal_manager.shutdown_all()` (`server.rs:524-533`), which kills every live PTY child via a clone-killer (`terminal.rs:459-469`).
- **Startup repudiates survivors:** `mark_running_terminals_orphaned()` flips every `running` row to `orphaned` on `open` (`terminals.rs:278-288`) — restart is written on the assumption that no PTY survived.
- **What persists (SQLite only):** a `terminal_sessions` metadata row + the full `terminal_events` byte-history that powers `term replay`. Note `env_json` is bound **NULL** at create (`terminals.rs:88-89`) — environment is not persisted. The live catch-up buffer is a vt100 **current-screen snapshot with scrollback=0** plus a 256-frame broadcast channel — both in-memory, both die with the daemon. (This is exactly why detach/reattach works only within one daemon lifetime.)
- **Managed agents are worse off:** spawned via `tokio::process::Command` with `Stdio::null()`, stored in an in-memory `HashMap` with **no SQLite table at all** (`process_registry.rs`) — zero persistence, no reconciliation, no re-adoption.

Five obstacles, one root cause — *the daemon is the PTY owner*: (1) children parented to the daemon, (2) teardown kills them, (3) startup marks them orphaned, (4) live handles are non-serializable in-memory objects (and env isn't even stored), (5) managed agents have no persistence.

## Current herdr model (ground truth)

- **Detach:** herdr runs a detached background server (`setsid`, `autodetect.rs:179-186`) that owns the PTYs; the TUI is a client over a socket. Client detach drops the socket; server + PTYs + children keep running. (wisphive already matches this half.)
- **Planned upgrade (`herdr update --handoff`, opt-in/experimental):** an **in-process live handoff between two concurrently-alive servers**. Old server spawns the new one, passes each live PTY master fd via `sendmsg`/`SCM_RIGHTS` (`handoff.rs:377-408`), then drops its runtimes **without killing** children (`preserve_processes_on_drop`, `pane.rs:1456-1469`); the new server calls `assume_handoff_ownership` after a multi-phase commit handshake (`validated → restored → ready → committed → owned`). If any step fails before commit, the old server rolls back and keeps its children.
- **Crash / plain restart:** live processes **die** — no FD recipient, master fds close, children get SIGHUP. Falls back to restoring session *shape* (`session.json`) + optional dead-scrollback replay + optional agent `--resume`. The docs' "what survives" matrix is honest: server-restart row = "Processes keep running: **No**"; only the two `--handoff` rows claim live survival, each qualified "best effort / if handoff succeeds."
- **Five state paths:** (1) live persistence = original process never stops [LIVE]; (2) snapshot restore = session shape, new shells [replay]; (3) pane screen-history replay = 8 KiB dead scrollback seeded into a new shell [replay]; (4) native agent resume = re-spawn the agent CLI with `--resume`, *not* a live process, with a dedupe key and "native resume owns history" precedence [replay+resume]; (5) live handoff = the FD-passing path [LIVE]. herdr layers **#5 for planned upgrades and #2+#3+#4 for everything else**.

**Injection check:** the herdr investigator found no prompt-injection aimed at an inspecting agent. herdr's `CLAUDE.md` contains contributor-workflow directives (refuse to skip its contribution process, don't open issues on a contributor's behalf, run `just check` before committing) and the standard `curl … | sh` install line — all ordinary project content for people working *on herdr*, none applicable here and none followed. Consistent with the earlier survey's findings.

## The three plans, costed against the obstacles

### Plan A — "Important sessions come back" (respawn / reconcile) — **build now**
Covers *both* crash and planned restart for the session-comes-back guarantee. Additive; no new trust boundary; the respawned child re-enters the hook gate automatically (it's a fresh subprocess through `wisphive-hook`), so gating stays clean.

- Per-session **pin / keep-alive flag** — the "*important sessions*" selector (herdr keeps everything; wisphive lets the user choose what's worth reconciling).
- Persist the full respawn spec (**stop binding `env_json` NULL**) so respawn is faithful; command/args/cwd are already stored.
- **Startup reconciliation** replaces the blanket orphan-sweep: for pinned sessions, respawn the command in saved cwd and seed the new shell's scrollback from the `terminal_events` history you already store (herdr path #3, and you already have the bytes).
- **Managed-agent persistence** — add the missing table so agents can be reconciled/resumed at all; agent panes resume via provider-native `--resume` (**itr#582**).
- **Honesty affordance** — reconciled sessions are marked as *restarted* (a "process was restarted, scrollback replayed" banner), never misrepresented as the same live process.
- **Cost: Medium.** Caveat: a mid-flight process (running build, REPL state) restarts — not preserved. That's the whole reason Plan B exists.

### Plan B — Live process survives a *planned* restart (FD-handoff) — **explore (this is what the PO wants)**
herdr's `--update --handoff` mechanism, adapted. Precursor spike already filed as **itr#581**.

- Stop teardown from killing children during handoff (a `preserve_processes_on_drop` equivalent; today `shutdown_all` kills them).
- `SCM_RIGHTS` PTY-fd passing over a private two-daemon handoff socket + multi-phase commit + rollback-on-failure.
- Wire it into the upgrade path (`install.sh` / daemon restart), tied to the 2026-07-15 incident (itr#533).
- **Gating during the handoff window — PO decision:** the gate being briefly "off"/transferring during a *planned* upgrade is of **minor consequence**, because when wisphive is unavailable most agent prompts still surface through the **terminal window itself** (the agent's native prompt), which covers the gap. **Requirement:** surface an explicit user-facing message that the gate is momentarily transferring during an upgrade, with that rationale — making the (brief, deliberate) gap legible rather than silent. Still security-reviewed for enforcement-continuity, but the posture is "acceptable, surfaced," not "must be zero-gap."
- **Cost: Large.** Does **not** cover a crash — like herdr, it's the planned-upgrade path only.

### Plan C — VIP (Very Important Processes) — **last, unless it makes B easier**
Move PTY ownership *out* of the daemon into a separate long-lived PTY host (or double-fork+`setsid` per session) the daemon attaches to as a client — the relationship the TUI has to the daemon today. Then daemon restart/crash never touches those PTYs. **Stronger than herdr** (whose PTYs die with its server).

- **PO framing — "VIP":** e.g. a spawned terminal that must survive a **planned upgrade of wisphive itself** so it can inspect/observe wisphive remotely *while the upgrade runs*. That terminal cannot live inside the process being upgraded.
- **Open evaluation (do this before committing to B):** does a PTY-host abstraction make Plan B's fd-handoff simpler or unnecessary? If the host owns the fds, a daemon upgrade needs no handoff at all — the new daemon just reconnects. The C-spike should decide whether C is a cleaner foundation than B, in which case build order flips.
- **Cost: XL**, plus a new trust boundary (the host runs children and must be owned/gated correctly). Only justified where crash-survival of a live process is non-negotiable — i.e. the VIP case.

## Recommendation / decisions recorded

1. **Plan A now.** Most pieces exist; make it a first-class "pin this session, survive a restart" feature.
2. **Plan B explored** as herdr-style FD-handoff for planned upgrades; the brief gate-transfer gap is accepted and **must be surfaced to the user**, leaning on the terminal-native prompt as the backstop.
3. **Plan C (VIP) last**, but the C-spike runs *before* committing B's design, because a PTY host could subsume B.
