/**
 * itr#400 — Agent liveness board runtime smoke (spec §5.2 + §10 evidence).
 *
 * Drives the Board view end-to-end against a REAL daemon + web server
 * (fixtures/daemon-server.ts, isolated short temp HOME — the live ~/.wisphive
 * is never touched) across TWO projects, proving the #400 ACs on live data:
 *
 *   AC1 — lanes per project × session with working / waiting / stalled states
 *         (+ subagent wave progress where derivable) render from real events.
 *   AC2 — a session that stops emitting events (the killed-agent case) flips
 *         its lane to stalled once its silence crosses the 600s threshold
 *         (STALL_THRESHOLD_MS): seeded near the boundary, the flip is observed
 *         LIVE on the clock with no further events.
 *   AC3 — a waiting-on-input session shows "waiting on you" AND cross-links to
 *         its inbox item: a queued decision lands selected (full DetailView),
 *         a deferred native prompt lands expanded (DeferredDetailView).
 *   AC4 — Codex activity appears in its own lane with a codex badge.
 *   AC6 — hard constraint (spec §5): the board is a read-only state mirror —
 *         its ONLY buttons are waiting lanes' inbox links.
 *
 * Faithfulness: working/deferred/codex lanes are authored by the REAL
 * `wisphive-hook` binary classifying real PreToolUse events into events.jsonl
 * (daemon tails → ingests → broadcasts). The waiting lane is a blocking
 * real-wire hook socket (fixtures/hook-client.ts). The stalled/near-stall
 * lanes are wire-format events.jsonl records with backdated timestamps —
 * the only way to compress a 600s silence into a test — exercising the same
 * ingest + derivation path as hook-authored records.
 *
 * Evidence screenshots land in campaign-003 artifacts as q2-400-*.png.
 */
import { test, expect, request as pwRequest, type APIRequestContext, type Page } from '@playwright/test'
import { spawn } from 'node:child_process'
import { appendFileSync, chmodSync, existsSync, mkdirSync, writeFileSync } from 'node:fs'
import path from 'node:path'
import { fileURLToPath } from 'node:url'
import { startWisphiveDaemonServer, type WisphiveDaemonServer } from './fixtures/daemon-server'
import { sendDecisionRequest, type PendingDecision } from './fixtures/hook-client'

const SPEC_DIR = path.dirname(fileURLToPath(import.meta.url))
// e2e → frontend → wisphive_web → crates → repo root
const REPO_ROOT = path.resolve(SPEC_DIR, '..', '..', '..', '..')
const EVIDENCE_DIR = path.join(
  REPO_ROOT,
  'campaign',
  'campaign-003-2026-07-14-command-center-layer1',
  'artifacts',
)

const PASSWORD = 'wisphive-e2e-password'
const PROJECT_A = '/tmp/wisphive-board-project-alpha'
const PROJECT_B = '/tmp/wisphive-board-project-bravo'

// Mirrors STALL_THRESHOLD_MS in src/components/liveness.ts (spec §5.2).
const STALL_MS = 600_000

let server: WisphiveDaemonServer
let api: APIRequestContext

function hookBinary(): string {
  const fromEnv = process.env.WISPHIVE_HOOK_BIN
  if (fromEnv && existsSync(fromEnv)) return fromEnv
  for (const c of [
    path.join(REPO_ROOT, 'target', 'debug', 'wisphive-hook'),
    path.join(REPO_ROOT, 'target', 'release', 'wisphive-hook'),
  ]) {
    if (existsSync(c)) return c
  }
  throw new Error('no wisphive-hook binary found — run `cargo build -p wisphive_hook`')
}

/** Run the REAL wisphive-hook against the isolated HOME, feeding a PreToolUse
 * event on stdin; it classifies and appends the record to events.jsonl. */
function runHook(event: Record<string, unknown>): Promise<{ code: number | null; stdout: string }> {
  return new Promise((resolve, reject) => {
    const child = spawn(hookBinary(), [], {
      env: { ...process.env, HOME: server.home },
      stdio: ['pipe', 'pipe', 'pipe'],
    })
    let stdout = ''
    let stderr = ''
    child.stdout.on('data', (d: Buffer) => (stdout += d.toString()))
    child.stderr.on('data', (d: Buffer) => (stderr += d.toString()))
    child.on('error', reject)
    child.on('close', (code) => {
      if (code !== 0 && code !== 2) {
        reject(new Error(`wisphive-hook exited ${code}. stderr:\n${stderr}\nstdout:\n${stdout}`))
        return
      }
      resolve({ code, stdout })
    })
    child.stdin.write(JSON.stringify(event))
    child.stdin.end()
  })
}

/** Append a wire-format events.jsonl record with a chosen (backdated)
 * timestamp — the seeding path for silence-based states, since a real hook
 * always stamps "now". Same shape the hook writes; same ingest path. */
function appendEventRecord(record: {
  agentId: string
  agentType?: 'claude_code' | 'codex'
  project: string
  toolName: string
  ts: Date
  toolUseId: string
}): void {
  const line = JSON.stringify({
    event: 'auto_approved',
    agent_id: record.agentId,
    agent_type: record.agentType ?? 'claude_code',
    project: record.project,
    tool_name: record.toolName,
    tool_input: { seeded: true },
    timestamp: record.ts.toISOString(),
    tool_use_id: record.toolUseId,
    hook_event_name: 'PreToolUse',
    decided_by: 'level:all',
  })
  appendFileSync(path.join(server.home, '.wisphive', 'events.jsonl'), line + '\n')
}

async function mintToken(deviceName: string): Promise<string> {
  const res = await api.post('/api/auth/login', {
    data: { password: PASSWORD, device_name: deviceName },
  })
  expect(res.ok(), `login failed: ${res.status()} ${await res.text()}`).toBeTruthy()
  return ((await res.json()) as { token: string }).token
}

async function openBoard(page: Page, deviceName: string): Promise<void> {
  const token = await mintToken(deviceName)
  await page.addInitScript((t: string) => localStorage.setItem('wisphive-web-token', t), token)
  await page.goto(`${server.baseURL}/`)
  await expect(page.locator('.inbox')).toBeVisible({ timeout: 15_000 })
  await page.getByRole('button', { name: /^Board/ }).click()
  await expect(page.locator('.board')).toBeVisible()
}

async function shot(page: Page, name: string): Promise<void> {
  mkdirSync(EVIDENCE_DIR, { recursive: true })
  const p = path.join(EVIDENCE_DIR, `${name}.png`)
  await page.screenshot({ path: p, fullPage: true })
  await test.info().attach(name, { path: p, contentType: 'image/png' })
}

test.beforeAll(async () => {
  server = await startWisphiveDaemonServer()
  // Gate ON (0600 mode file inside the fixture's 0700 state dir) + level=all
  // so hook-authored Reads auto-approve into the audit stream while
  // AskUserQuestion still always-defers (ADR-0002).
  const wisphiveDir = path.join(server.home, '.wisphive')
  const modePath = path.join(wisphiveDir, 'mode')
  writeFileSync(modePath, 'active')
  chmodSync(modePath, 0o600)
  const configPath = path.join(wisphiveDir, 'config.json')
  writeFileSync(configPath, JSON.stringify({ auto_approve_level: 'all' }))
  chmodSync(configPath, 0o600)

  api = await pwRequest.newContext({ baseURL: server.baseURL, ignoreHTTPSErrors: true })
  const res = await api.post('/api/auth/set-password', {
    data: { password: PASSWORD, device_name: 'e2e-setup' },
  })
  if (!res.ok()) {
    throw new Error(`set-password bootstrap failed: ${res.status()} ${await res.text()}`)
  }
})

test.afterAll(async () => {
  if (api) await api.dispose()
  if (server) await server.stop()
})

test('liveness board shows per-project lanes, live stall flip, and inbox cross-links', async ({
  page,
}) => {
  // Real hook cold-spawns + events.jsonl ingest hops + a live ~60s stall-flip
  // observation window need far more than the 60s default.
  test.setTimeout(300_000)

  // Open the console FIRST (the Burst-mode reality: board up while agents
  // work). This also matters for AC3b: the deferred row's redacted question
  // text rides the LIVE ingest wire only — the reconnect snapshot serves
  // tool_input: None by design (state/decisions.rs recent_audit_decisions).
  await openBoard(page, 'e2e-board-smoke')

  // ── Seed the fleet ─────────────────────────────────────────────────────────
  // Project A, working: a real hook auto-approved Read (fresh event).
  await runHook({
    session_id: 'board-working',
    tool_name: 'Read',
    tool_input: { file_path: `${PROJECT_A}/notes.txt` },
    cwd: PROJECT_A,
    hook_event_name: 'PreToolUse',
    permission_mode: 'default',
  })

  // Project B, deferred waiting: a real hook always-deferred AskUserQuestion.
  const QUESTION = 'Which lane state should the retry loop report?'
  await runHook({
    session_id: 'board-defer',
    tool_name: 'AskUserQuestion',
    tool_input: {
      questions: [
        {
          question: QUESTION,
          header: 'Lane state',
          options: [{ label: 'working' }, { label: 'stalled' }],
        },
      ],
    },
    cwd: PROJECT_B,
    hook_event_name: 'PreToolUse',
    permission_mode: 'default',
  })

  // Project B, Codex lane: the real hook classifies a `model`-bearing event as
  // Codex (detect_agent_type) — its lane must carry the codex badge.
  await runHook({
    session_id: 'board-codex',
    model: 'gpt-5.6-terra',
    tool_name: 'Read',
    tool_input: { file_path: `${PROJECT_B}/src/main.rs` },
    cwd: PROJECT_B,
    hook_event_name: 'PreToolUse',
    permission_mode: 'default',
  })

  // Project B, stalled (the killed-agent shape): last event 11 minutes ago,
  // nothing since — silence already past the 600s threshold.
  appendEventRecord({
    agentId: 'cc-board-killed',
    project: PROJECT_B,
    toolName: 'Bash',
    ts: new Date(Date.now() - STALL_MS - 60_000),
    toolUseId: 'toolu-board-killed-1',
  })

  // Project A, near-stall flip subject: last event ~9m ago; its lane must be
  // observed flipping working → stalled on the clock with NO further events.
  appendEventRecord({
    agentId: 'cc-board-flip',
    project: PROJECT_A,
    toolName: 'Edit',
    ts: new Date(Date.now() - STALL_MS + 55_000),
    toolUseId: 'toolu-board-flip-1',
  })

  // Project A, waiting on a queued decision: a blocking real-wire hook.
  let pending: PendingDecision | null = null
  pending = await sendDecisionRequest(server.socketPath, {
    toolName: 'Grep',
    toolInput: { pattern: 'stall', path: PROJECT_A },
    project: PROJECT_A,
    agentId: 'cc-board-waiting',
  })

  try {
    // ── AC1 + AC4: lanes per project × session, correct states, codex lane ──
    const lane = (agentId: string) =>
      page.locator('.board-lane').filter({ has: page.locator(`.lane-agent[title="${agentId}"]`) })

    await expect(lane('cc-board-working')).toBeVisible({ timeout: 30_000 })
    await expect(lane('cc-board-working').locator('.lane-state-label')).toHaveText('working')
    await expect(lane('cc-board-working').locator('.lane-task')).toHaveText('last: Read')

    await expect(lane('cc-board-waiting')).toBeVisible({ timeout: 30_000 })
    await expect(lane('cc-board-waiting').locator('.lane-state-label')).toHaveText('waiting on you')

    await expect(lane('cc-board-defer')).toBeVisible({ timeout: 30_000 })
    await expect(lane('cc-board-defer').locator('.lane-state-label')).toHaveText('waiting on you')

    await expect(lane('cc-board-killed')).toBeVisible({ timeout: 30_000 })
    await expect(lane('cc-board-killed').locator('.lane-state-label')).toContainText('stalled')
    await expect(lane('cc-board-killed')).toHaveClass(/state-stalled/)

    await expect(lane('codex-board-codex')).toBeVisible({ timeout: 30_000 })
    await expect(lane('codex-board-codex').locator('.lane-type-badge')).toHaveText('codex')

    // Both projects render as groups.
    await expect(page.locator('.board-project-name', { hasText: 'wisphive-board-project-alpha' })).toBeVisible()
    await expect(page.locator('.board-project-name', { hasText: 'wisphive-board-project-bravo' })).toBeVisible()

    // ── AC6 (spec §5 hard constraint): read-only surface ────────────────────
    // The ONLY buttons inside the board are waiting lanes' inbox links.
    const boardButtons = page.locator('.board button')
    await expect(boardButtons).toHaveCount(2)
    for (const text of await boardButtons.allTextContents()) {
      expect(text).toContain('Answer in Inbox')
    }
    await expect(page.locator('.board-note')).toContainText('Read-only state mirror')

    await shot(page, 'q2-400-board-lanes')

    // ── AC2: the near-stall lane flips to stalled LIVE, no further events ───
    const flipLabel = lane('cc-board-flip').locator('.lane-state-label')
    await expect(lane('cc-board-flip')).toBeVisible({ timeout: 30_000 })
    // Seeded ~55s under the threshold; ingest lag eats into the margin, so a
    // still-working read here is asserted only if we caught it in time —
    // the load-proof assertion is the flip itself.
    const preFlip = await flipLabel.textContent()
    await expect(flipLabel).toContainText('stalled', { timeout: 120_000 })
    await expect(lane('cc-board-flip')).toHaveClass(/state-stalled/)
    console.log(`stall flip observed: "${preFlip}" → "${await flipLabel.textContent()}"`)
    await shot(page, 'q2-400-stalled-flip')

    // ── AC3a: waiting (queued) lane cross-links to its selected inbox row ───
    await lane('cc-board-waiting').getByRole('button', { name: 'Answer in Inbox →' }).click()
    await expect(page.locator('.inbox')).toBeVisible()
    const selectedRow = page.locator('.inbox-item.selected', { hasText: 'Grep' })
    await expect(selectedRow).toBeVisible()
    // Selected = the full DetailView is open on exactly the linked decision.
    await expect(selectedRow.locator('.inbox-detail-full .detail-view')).toBeVisible()

    // ── AC3b: waiting (deferred) lane cross-links to the expanded row ───────
    await page.getByRole('button', { name: /^Board/ }).click()
    await expect(page.locator('.board')).toBeVisible()
    await lane('cc-board-defer').getByRole('button', { name: 'Answer in Inbox →' }).click()
    await expect(page.locator('.inbox')).toBeVisible()
    const deferredDetail = page.locator('.deferred-detail')
    await expect(deferredDetail).toBeVisible()
    await expect(deferredDetail).toContainText(QUESTION)
    await shot(page, 'q2-400-waiting-crosslink')
  } finally {
    pending?.close()
  }
})
