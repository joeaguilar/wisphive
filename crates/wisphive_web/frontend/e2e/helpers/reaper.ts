/**
 * Shared orphan reaper for the e2e server boot helpers.
 *
 * ONE module-level registry and ONE set of process handlers, imported by both
 * helpers/server.ts and fixtures/daemon-server.ts. A single shared registry is
 * load-bearing: Playwright reuses workers across spec files, so if each boot
 * module installed its own signal handler with its own `LIVE` set, the first
 * handler to fire could tear down the process before the sibling module's
 * cleanup ran — leaking that module's detached children and temp dirs.
 *
 * Within a run, callers `untrack()` on clean stop and reap nothing here. On an
 * interrupted run (Ctrl-C, worker crash) we reap synchronously — group-kill the
 * tracked children and remove their temp dirs — and RE-RAISE the signal rather
 * than `process.exit()`, so a co-registered handler (e.g. Playwright's own
 * teardown) is not preempted mid-flight.
 */
import { rmSync } from 'node:fs'

export interface Tracked {
  pid: number | undefined
  home: string
}

const LIVE = new Set<Tracked>()
let installed = false

function reap(): void {
  for (const t of LIVE) {
    if (t.pid !== undefined) {
      try {
        process.kill(-t.pid, 'SIGKILL')
      } catch {
        /* group already gone */
      }
    }
    try {
      rmSync(t.home, { recursive: true, force: true })
    } catch {
      /* best effort */
    }
  }
  LIVE.clear()
}

function install(): void {
  if (installed) return
  installed = true
  // Normal / process.exit() path (incl. Playwright exiting after its teardown).
  process.once('exit', reap)
  // Bare-signal path: reap, then re-raise with default disposition so we don't
  // truncate other handlers via process.exit(). Remove ourselves first so the
  // re-raised signal isn't caught here again.
  const onSignal = (sig: NodeJS.Signals): void => {
    reap()
    process.removeListener(sig, onSignal)
    process.kill(process.pid, sig)
  }
  process.on('SIGINT', onSignal)
  process.on('SIGTERM', onSignal)
}

/** Begin tracking a spawned server. Returns a handle to pass to `untrack`. */
export function track(pid: number | undefined, home: string): Tracked {
  install()
  const t: Tracked = { pid, home }
  LIVE.add(t)
  return t
}

/** Stop tracking (clean stop, before the temp dir is removed). */
export function untrack(t: Tracked): void {
  LIVE.delete(t)
}

/** SIGTERM/SIGKILL the whole process group, falling back to the bare pid. */
export function killGroup(
  child: { pid: number | undefined; kill: (s: NodeJS.Signals) => boolean },
  signal: NodeJS.Signals,
): void {
  if (child.pid === undefined) return
  try {
    process.kill(-child.pid, signal)
  } catch {
    try {
      child.kill(signal)
    } catch {
      /* already dead */
    }
  }
}
