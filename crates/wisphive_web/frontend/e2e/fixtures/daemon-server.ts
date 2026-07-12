/**
 * Boot variant for specs that need a REAL decision queue.
 *
 * `wisphive web serve` (what e2e/helpers/server.ts boots) is standalone:
 * it only *connects* to the daemon's Unix socket per WebSocket upgrade
 * (see crates/wisphive_web/src/ws_bridge.rs) and never creates the socket
 * or the queue itself. To drive the approve/deny round-trip end-to-end we
 * boot `wisphive daemon start --web ...` instead — one process that owns
 * the Unix socket, the decision queue, AND the embedded TLS web UI.
 *
 * Isolation mirrors e2e/helpers/server.ts exactly (which stays frozen —
 * this is a sibling, not an edit): HOME=<tempdir> so the real ~/.wisphive
 * (owned by a LIVE daemon gating this dev session) is never read or
 * written. Additionally, a stub `bin/` dir is prepended to PATH so the
 * daemon's passive-notification spawns (`terminal-notifier`, `osascript`,
 * `notify-send` — see crates/wisphive_daemon/src/notify.rs) become no-ops
 * instead of popping real banners on the developer's desktop every time a
 * fixture decision is queued.
 */
import { spawn, type ChildProcessWithoutNullStreams } from 'node:child_process'
import { existsSync } from 'node:fs'
import { chmod, mkdir, mkdtemp, rm, writeFile } from 'node:fs/promises'
import { request as httpsRequest } from 'node:https'
import { createServer } from 'node:net'
import os from 'node:os'
import path from 'node:path'
import { fileURLToPath } from 'node:url'
import { killGroup, track, untrack } from '../helpers/reaper'

const FIXTURES_DIR = path.dirname(fileURLToPath(import.meta.url))
// e2e/fixtures → e2e → frontend → wisphive_web → crates → repo root
const REPO_ROOT = path.resolve(FIXTURES_DIR, '..', '..', '..', '..', '..')

export interface WisphiveDaemonServer {
  /** Browser-facing origin (`localhost` — covered by the self-signed cert). */
  baseURL: string
  port: number
  /** The isolated `$HOME` temp dir. State lives under `<home>/.wisphive`. */
  home: string
  /** The daemon's Unix socket — connect hook fixtures here. */
  socketPath: string
  process: ChildProcessWithoutNullStreams
  /** Combined stdout+stderr captured so far — attach on failure. */
  output(): string
  /** SIGTERM (then SIGKILL) the daemon and delete the temp state dir. */
  stop(): Promise<void>
}

/** Resolve the `wisphive` binary: $WISPHIVE_BIN, then debug, then release. */
function resolveBinary(): string {
  const fromEnv = process.env.WISPHIVE_BIN
  if (fromEnv) {
    if (!existsSync(fromEnv)) {
      throw new Error(`WISPHIVE_BIN points at a missing file: ${fromEnv}`)
    }
    return fromEnv
  }
  const candidates = [
    path.join(REPO_ROOT, 'target', 'debug', 'wisphive'),
    path.join(REPO_ROOT, 'target', 'release', 'wisphive'),
  ]
  for (const c of candidates) {
    if (existsSync(c)) return c
  }
  throw new Error(
    `no wisphive binary found (looked for ${candidates.join(', ')}). ` +
      'Run `just e2e` from the repo root, or `cargo build -p wisphive_cli --bin wisphive`, ' +
      'or set WISPHIVE_BIN.',
  )
}

function allocateEphemeralPort(): Promise<number> {
  return new Promise((resolve, reject) => {
    const srv = createServer()
    srv.once('error', reject)
    srv.listen(0, '127.0.0.1', () => {
      const addr = srv.address()
      if (addr && typeof addr === 'object') {
        const port = addr.port
        srv.close(() => resolve(port))
      } else {
        srv.close(() => reject(new Error('failed to allocate an ephemeral port')))
      }
    })
  })
}

function sleep(ms: number): Promise<void> {
  return new Promise((resolve) => setTimeout(resolve, ms))
}

/** One readiness probe: GET /api/auth/status over TLS, cert unverified. */
function probeReady(port: number): Promise<boolean> {
  return new Promise((resolve) => {
    const req = httpsRequest(
      {
        host: '127.0.0.1',
        port,
        path: '/api/auth/status',
        method: 'GET',
        rejectUnauthorized: false,
        timeout: 2_000,
      },
      (res) => {
        res.resume()
        resolve(res.statusCode === 200)
      },
    )
    req.on('error', () => resolve(false))
    req.on('timeout', () => {
      req.destroy()
      resolve(false)
    })
    req.end()
  })
}

/** Write a no-op executable stub so a notification spawn succeeds silently. */
async function writeStub(binDir: string, name: string): Promise<void> {
  const p = path.join(binDir, name)
  await writeFile(p, '#!/bin/sh\nexit 0\n')
  await chmod(p, 0o755)
}

export async function startWisphiveDaemonServer(): Promise<WisphiveDaemonServer> {
  const bin = resolveBinary()

  const home = await mkdtemp(path.join(os.tmpdir(), 'wisphive-e2e-daemon-'))
  // Belt-and-braces: never proceed if the isolated dir somehow resolves to
  // (or inside) the real home — a live wisphive daemon owns ~/.wisphive.
  const realHome = path.resolve(os.homedir())
  const isolated = path.resolve(home)
  if (isolated === realHome || isolated.startsWith(realHome + path.sep)) {
    throw new Error(`refusing to run e2e against the real HOME: ${home}`)
  }
  await mkdir(path.join(home, '.wisphive'), { recursive: true, mode: 0o700 })

  // Secure-mode file: the daemon re-validates mode per hook DecisionRequest
  // (crates/wisphive_daemon/src/server.rs `hook_decision_mode_denial`, main
  // commit b6a1551) and denies before enqueue unless `<home>/.wisphive/mode`
  // is a 0600 regular file reading `active` inside the 0700 state dir. A
  // real hook only ever sends a DecisionRequest when mode is active, so the
  // fixture daemon must start in that state or every injected decision is
  // denied without ever reaching the queue.
  const modePath = path.join(home, '.wisphive', 'mode')
  await writeFile(modePath, 'active')
  await chmod(modePath, 0o600)

  // Notification no-op stubs, first on PATH.
  const binDir = path.join(home, 'stub-bin')
  await mkdir(binDir, { recursive: true })
  await writeStub(binDir, 'terminal-notifier')
  await writeStub(binDir, 'osascript')
  await writeStub(binDir, 'notify-send')

  const port = await allocateEphemeralPort()
  const args = [
    'daemon',
    'start',
    '--web',
    '--port',
    String(port),
    '--host',
    '127.0.0.1',
    '--no-open',
  ]

  const child = spawn(bin, args, {
    env: {
      ...process.env,
      HOME: home,
      PATH: `${binDir}${path.delimiter}${process.env.PATH ?? ''}`,
    },
    stdio: ['ignore', 'pipe', 'pipe'],
    // Own process group so an interrupted runner can group-kill the tree.
    detached: true,
  })

  const tracked = track(child.pid, home)

  let output = ''
  child.stdout.on('data', (d: Buffer) => (output += d.toString()))
  child.stderr.on('data', (d: Buffer) => (output += d.toString()))

  let exited = false
  const exitPromise = new Promise<void>((resolve) => {
    child.once('exit', () => {
      exited = true
      resolve()
    })
  })

  const waitExit = async (ms: number): Promise<boolean> =>
    Promise.race([exitPromise.then(() => true), sleep(ms).then(() => false)])

  const cleanup = async () => {
    untrack(tracked)
    await rm(home, { recursive: true, force: true })
  }

  const socketPath = path.join(home, '.wisphive', 'wisphive.sock')

  // Ready = web listener answering AND the daemon's Unix socket bound (the
  // embedded web server is spawned before Server::new binds the socket, so
  // the two are racing — hook fixtures need the socket, not just the web UI).
  const readyTimeoutMs = 30_000
  const deadline = Date.now() + readyTimeoutMs
  try {
    for (;;) {
      if (exited) {
        throw new Error(
          `wisphive daemon start exited before becoming ready (args: ${args.join(' ')}).\n--- output ---\n${output}`,
        )
      }
      if ((await probeReady(port)) && existsSync(socketPath)) break
      if (Date.now() > deadline) {
        throw new Error(
          `wisphive daemon start not ready after ${readyTimeoutMs}ms on port ${port}.\n--- output ---\n${output}`,
        )
      }
      await sleep(150)
    }
  } catch (err) {
    if (!exited) killGroup(child, 'SIGKILL')
    await waitExit(5_000)
    await cleanup()
    throw err
  }

  return {
    baseURL: `https://localhost:${port}`,
    port,
    home,
    socketPath,
    process: child,
    output: () => output,
    stop: async () => {
      if (!exited) {
        killGroup(child, 'SIGTERM')
        if (!(await waitExit(5_000))) {
          killGroup(child, 'SIGKILL')
          await waitExit(5_000)
        }
      }
      await cleanup()
    },
  }
}
