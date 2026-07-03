/**
 * Boot helper for Wisphive web e2e tests.
 *
 * Launches a real `wisphive web serve` process against a FRESH isolated
 * state dir and an ephemeral port, waits for readiness, and hands back a
 * `stop()` for teardown. The daemon/web state dir resolves purely via
 * `$HOME` (see `dirs_home` in crates/wisphive_daemon/src/config.rs), so
 * setting `HOME=<tempdir>` on the child process guarantees the real
 * `~/.wisphive` is never read or written — including its Unix socket, DB,
 * mode file, and TLS certs, which are all minted fresh under the temp dir.
 *
 * Server mode: production embedded-assets mode (`wisphive web serve`, TLS
 * with a self-signed cert). In debug cargo builds, rust-embed serves
 * `frontend/dist/` from disk at request time, so `npm run build` + a debug
 * `cargo build` is enough to e2e the current frontend (no release build
 * needed). `just e2e` does exactly that.
 *
 * Reusable by design: later specs (login, queue, TLS) should import this
 * unchanged — use `extraArgs` for additional CLI flags, `env` for extra
 * environment, and `port` only if a fixed port is genuinely required.
 */
import { spawn, type ChildProcessWithoutNullStreams } from 'node:child_process'
import { existsSync, rmSync } from 'node:fs'
import { mkdir, mkdtemp, rm } from 'node:fs/promises'
import { request as httpsRequest } from 'node:https'
import { createServer } from 'node:net'
import os from 'node:os'
import path from 'node:path'
import { fileURLToPath } from 'node:url'

const HELPERS_DIR = path.dirname(fileURLToPath(import.meta.url))
// e2e/helpers → e2e → frontend → wisphive_web → crates → repo root
const REPO_ROOT = path.resolve(HELPERS_DIR, '..', '..', '..', '..', '..')

export interface StartOptions {
  /** Extra CLI flags appended to `wisphive web serve --port <p> --host 127.0.0.1 --no-open`. */
  extraArgs?: string[]
  /**
   * Extra environment variables for the server process. `HOME` is always
   * forced to the isolated temp dir and cannot be overridden here.
   */
  env?: Record<string, string>
  /** Fixed port. Defaults to a freshly allocated ephemeral port. */
  port?: number
  /** How long to wait for `/api/auth/status` to answer 200. Default 30s. */
  readyTimeoutMs?: number
}

export interface WisphiveServer {
  /**
   * Browser-facing origin. Uses `localhost` (not `127.0.0.1`): the server
   * 302-redirects UI paths from IP-literal hosts to `localhost` so passkey
   * RP ID rules hold, and the self-signed cert covers both.
   */
  baseURL: string
  port: number
  /** The isolated `$HOME` temp dir. State lives under `<home>/.wisphive`. */
  home: string
  process: ChildProcessWithoutNullStreams
  /** Combined stdout+stderr captured so far — attach on failure. */
  output(): string
  /** SIGTERM (then SIGKILL) the server and delete the temp state dir. */
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
  // Debug first: in debug builds rust-embed serves frontend/dist from disk,
  // so the freshly built SPA is what gets tested. A release binary carries
  // whatever dist was embedded at compile time.
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

// --- Orphan reaper ---------------------------------------------------------
// Within a run, stop() tears servers down cleanly. But if the runner is
// interrupted (Ctrl-C, a worker crash mid-spec), afterAll may never fire and
// a live `wisphive` process plus a `wisphive-e2e-*` temp dir would leak. Each
// server is spawned `detached` (own process group) and tracked here; a
// process-exit reaper group-kills survivors and removes their temp dirs.
interface Tracked {
  pid: number | undefined
  home: string
}
const LIVE = new Set<Tracked>()
let reaperInstalled = false

function installReaper(): void {
  if (reaperInstalled) return
  reaperInstalled = true
  const reap = (): void => {
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
  process.once('exit', reap)
  // 'exit' does not fire on a bare signal — convert, reap, then exit.
  process.once('SIGINT', () => {
    reap()
    process.exit(130)
  })
  process.once('SIGTERM', () => {
    reap()
    process.exit(143)
  })
}

/** SIGTERM/SIGKILL the whole process group, falling back to the bare pid. */
function killGroup(child: ChildProcessWithoutNullStreams, signal: NodeJS.Signals): void {
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

/** One readiness probe: GET /api/auth/status over TLS, cert unverified. */
function probeReady(port: number): Promise<boolean> {
  return new Promise((resolve) => {
    const req = httpsRequest(
      {
        host: '127.0.0.1',
        port,
        path: '/api/auth/status',
        method: 'GET',
        // Self-signed cert minted into the isolated state dir.
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

export async function startWisphiveServer(opts: StartOptions = {}): Promise<WisphiveServer> {
  const bin = resolveBinary()

  const home = await mkdtemp(path.join(os.tmpdir(), 'wisphive-e2e-'))
  // Belt-and-braces: never proceed if the isolated dir somehow resolves to
  // (or inside) the real home — a live wisphive daemon owns ~/.wisphive.
  const realHome = path.resolve(os.homedir())
  const isolated = path.resolve(home)
  if (isolated === realHome || isolated.startsWith(realHome + path.sep)) {
    throw new Error(`refusing to run e2e against the real HOME: ${home}`)
  }
  await mkdir(path.join(home, '.wisphive'), { recursive: true, mode: 0o700 })

  const port = opts.port ?? (await allocateEphemeralPort())
  const args = [
    'web',
    'serve',
    '--port',
    String(port),
    '--host',
    '127.0.0.1',
    '--no-open',
    ...(opts.extraArgs ?? []),
  ]

  const child = spawn(bin, args, {
    env: { ...process.env, ...opts.env, HOME: home },
    stdio: ['ignore', 'pipe', 'pipe'],
    // Own process group so an interrupted runner can group-kill the whole
    // tree (see the orphan reaper) rather than orphaning the server.
    detached: true,
  })

  const tracked: Tracked = { pid: child.pid, home }
  LIVE.add(tracked)
  installReaper()

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
    LIVE.delete(tracked)
    await rm(home, { recursive: true, force: true })
  }

  const readyTimeoutMs = opts.readyTimeoutMs ?? 30_000
  const deadline = Date.now() + readyTimeoutMs
  try {
    for (;;) {
      if (exited) {
        throw new Error(
          `wisphive web serve exited before becoming ready (args: ${args.join(' ')}).\n--- output ---\n${output}`,
        )
      }
      if (await probeReady(port)) break
      if (Date.now() > deadline) {
        throw new Error(
          `wisphive web serve not ready after ${readyTimeoutMs}ms on port ${port}.\n--- output ---\n${output}`,
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
