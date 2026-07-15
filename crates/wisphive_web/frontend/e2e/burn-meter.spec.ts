/**
 * itr#402 — Burn meter runtime smoke (spec §5.4 + §10 evidence).
 *
 * Drives the Burn view end-to-end against a REAL daemon + web server
 * (fixtures/daemon-server.ts, isolated short temp HOME — the live ~/.wisphive
 * is never touched), proving the #402 ACs on live data:
 *
 *   AC1 — a real session's tile shows spend as a clearly LABELLED proxy
 *         ("activity proxy" · gated tool calls · active wall-clock · "token
 *         spend not observable") alongside its artifact list (file paths +
 *         `git commit` subjects derived from real hook-approved calls).
 *   AC2 — a deliberately artifact-free run (read-shaped calls only, spend
 *         past both documented thresholds) trips the dead-run alert.
 *   AC3 — hard constraint (spec §5): the meter is a read-only state mirror —
 *         zero write affordances (only artifact-list expanders may exist).
 *
 * Faithfulness: the productive session is authored by the REAL
 * `wisphive-hook` binary classifying real PreToolUse events (Write/Edit +
 * a `git commit` Bash) into events.jsonl → daemon ingest → decision_log →
 * `query_burn`. The dead run is wire-format events.jsonl records with
 * backdated timestamps — the only way to compress a 10-minute burn into a
 * test — exercising the same ingest + derivation path.
 *
 * Evidence screenshots land in campaign-003 artifacts as q4-402-*.png.
 */
import { test, expect, request as pwRequest, type APIRequestContext, type Page } from '@playwright/test'
import { spawn } from 'node:child_process'
import { appendFileSync, chmodSync, existsSync, mkdirSync, writeFileSync } from 'node:fs'
import path from 'node:path'
import { fileURLToPath } from 'node:url'
import { startWisphiveDaemonServer, type WisphiveDaemonServer } from './fixtures/daemon-server'

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
const PROJECT_A = '/tmp/wisphive-burn-project-alpha'
const PROJECT_B = '/tmp/wisphive-burn-project-bravo'

// Mirror the documented constants in src/components/burnMeter.ts.
const DEAD_RUN_MIN_TOOL_CALLS = 10
const DEAD_RUN_MIN_ACTIVE_MS = 10 * 60 * 1000

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
 * timestamp — the seeding path for the dead run's 10-minute spend span, since
 * a real hook always stamps "now". Same shape the hook writes; same ingest. */
function appendEventRecord(record: {
  agentId: string
  project: string
  toolName: string
  toolInput: Record<string, unknown>
  ts: Date
  toolUseId: string
}): void {
  const line = JSON.stringify({
    event: 'auto_approved',
    agent_id: record.agentId,
    agent_type: 'claude_code',
    project: record.project,
    tool_name: record.toolName,
    tool_input: record.toolInput,
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

async function openBurn(page: Page, deviceName: string): Promise<void> {
  const token = await mintToken(deviceName)
  await page.addInitScript((t: string) => localStorage.setItem('wisphive-web-token', t), token)
  await page.goto(`${server.baseURL}/`)
  await expect(page.locator('.inbox')).toBeVisible({ timeout: 15_000 })
  await page.getByRole('button', { name: /^Burn/ }).click()
  await expect(page.locator('.burn')).toBeVisible()
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
  // so hook-authored Write/Edit/Bash auto-approve into events.jsonl →
  // decision_log, the burn meter's artifact feed.
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

test('burn meter shows labelled spend proxy + artifacts, and trips the dead-run alert', async ({
  page,
}) => {
  // Real hook cold-spawns + events.jsonl ingest hops + the 15s query_burn
  // poll need more than the 60s default.
  test.setTimeout(240_000)

  await openBurn(page, 'e2e-burn-smoke')

  // ── Seed the productive session (PROJECT_A): real hook, real classification.
  const COMMIT_SUBJECT = 'feat(meter): ship the burn tile'
  await runHook({
    session_id: 'burn-productive',
    tool_name: 'Write',
    tool_input: { file_path: `${PROJECT_A}/src/meter.rs`, content: 'pub fn burn() {}' },
    cwd: PROJECT_A,
    hook_event_name: 'PreToolUse',
    permission_mode: 'default',
  })
  await runHook({
    session_id: 'burn-productive',
    tool_name: 'Edit',
    tool_input: {
      file_path: `${PROJECT_A}/src/meter.rs`,
      old_string: 'pub fn burn() {}',
      new_string: 'pub fn burn() { /* wired */ }',
    },
    cwd: PROJECT_A,
    hook_event_name: 'PreToolUse',
    permission_mode: 'default',
  })
  await runHook({
    session_id: 'burn-productive',
    tool_name: 'Bash',
    tool_input: { command: `git commit -m '${COMMIT_SUBJECT}'` },
    cwd: PROJECT_A,
    hook_event_name: 'PreToolUse',
    permission_mode: 'default',
  })
  // A read-shaped call: spend, never an artifact (itr#549).
  await runHook({
    session_id: 'burn-productive',
    tool_name: 'Read',
    tool_input: { file_path: `${PROJECT_A}/src/meter.rs` },
    cwd: PROJECT_A,
    hook_event_name: 'PreToolUse',
    permission_mode: 'default',
  })

  // ── Seed the dead run (PROJECT_B): deliberately artifact-free — only
  // read-shaped calls — with spend past BOTH documented thresholds
  // (≥10 approved calls spanning ≥10 minutes), ending 2 minutes ago so the
  // whole span sits inside the 1-hour burn window.
  const calls = DEAD_RUN_MIN_TOOL_CALLS + 2
  const endMs = Date.now() - 2 * 60 * 1000
  const spanMs = DEAD_RUN_MIN_ACTIVE_MS + 4 * 60 * 1000
  for (let i = 0; i < calls; i++) {
    appendEventRecord({
      agentId: 'cc-burn-dead',
      project: PROJECT_B,
      toolName: i % 2 === 0 ? 'Read' : 'Grep',
      toolInput: { file_path: `${PROJECT_B}/notes-${i}.md` },
      ts: new Date(endMs - spanMs + Math.round((i * spanMs) / (calls - 1))),
      toolUseId: `toolu-burn-dead-${i}`,
    })
  }

  // ── AC1: labelled spend proxy + artifact list on the productive tile ──────
  const tile = (agentId: string) =>
    page.locator('.burn-tile').filter({ has: page.locator(`.burn-agent[title="${agentId}"]`) })

  const productive = tile('cc-burn-productive')
  await expect(productive).toBeVisible({ timeout: 45_000 })
  // The proxy is labelled AS a proxy, right where the number is.
  await expect(productive.locator('.burn-proxy-label')).toHaveText('activity proxy')
  await expect(productive.locator('.burn-proxy-note')).toHaveText('token spend not observable')
  // All four hook events (Write/Edit/Bash/Read) count as spend once ingested.
  await expect(productive.locator('.burn-spend-value')).toContainText('4 tool calls', {
    timeout: 30_000,
  })
  // Artifact list: the file (Write+Edit aggregate to ×2) and the commit
  // subject, each fully rendered. query_burn polls every 15s.
  const fileRow = productive.locator('.burn-artifact').filter({ hasText: `${PROJECT_A}/src/meter.rs` })
  await expect(fileRow).toBeVisible({ timeout: 45_000 })
  await expect(fileRow.locator('.artifact-count')).toHaveText('×2')
  const commitRow = productive.locator('.burn-artifact').filter({ hasText: COMMIT_SUBJECT })
  await expect(commitRow).toBeVisible()
  await expect(commitRow.locator('.artifact-kind')).toHaveText('commit')
  // The honesty contract is stated on the surface.
  await expect(page.locator('.burn-note')).toContainText('activity proxy')
  await expect(page.locator('.burn-note')).toContainText('cannot see model tokens')
  await shot(page, 'q4-402-tile-proxy-artifacts')

  // ── AC2: the artifact-free run trips the dead-run alert ───────────────────
  const dead = tile('cc-burn-dead')
  await expect(dead).toBeVisible({ timeout: 45_000 })
  await expect(dead).toHaveClass(/dead-run/)
  const alert = dead.locator('.burn-dead-alert')
  await expect(alert).toBeVisible()
  await expect(alert).toContainText('DEAD RUN')
  await expect(alert).toContainText('zero artifacts')
  await expect(alert).toHaveAttribute('role', 'alert')
  // The header totals call it out too.
  await expect(page.locator('.burn-counts')).toContainText('1 dead runs')
  await shot(page, 'q4-402-dead-run')

  // ── AC3 (spec §5 hard constraint): zero write affordances ─────────────────
  // With ≤6 artifacts per tile there is no expander, so the meter has NO
  // buttons at all — nothing can stop/throttle/retarget a session from here.
  await expect(page.locator('.burn button')).toHaveCount(0)
  await expect(page.locator('.burn-note')).toContainText('Read-only state mirror')

  // ── Mobile (itr rule: responsive from the start) ──────────────────────────
  await page.setViewportSize({ width: 390, height: 844 })
  await expect(productive.locator('.burn-proxy-label')).toBeVisible()
  await expect(dead.locator('.burn-dead-alert')).toBeVisible()
  await shot(page, 'q4-402-mobile')
})
