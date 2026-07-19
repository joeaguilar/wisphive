/**
 * itr#565 — "Spawn Agent form does nothing": the APPROVE half of the flow.
 *
 * itr#567 (error-surfacing.spec.ts) proved the refusal/deny paths surface.
 * This spec drives the missing half end-to-end against the REAL daemon: the
 * operator fills the form, sees the queued approval WAITING in the Inbox,
 * satisfies the SpawnAgent sudo gate, and the daemon actually EXECS the
 * agent process (a stub `claude` on the fixture PATH records its argv and
 * stays alive as the managed child).
 *
 * 1. Full happy path: form -> queued ack (pointing at the Inbox) -> Inbox
 *    row visible with Approve/Deny -> SudoModal (SpawnAgent is sudo-class)
 *    -> reauth -> replayed approve -> process_registry exec observed (argv
 *    marker on disk + `wisphive agent list` registry row) with NO error
 *    banner and the Inbox row resolved.
 * 2. The PO's "picking a project breaks it" case: a real, existing project
 *    WITHOUT Wisphive hooks queues fine, but the post-approval gate refuses
 *    the launch — that refusal must land in the error banner NAMING the fix
 *    (`wisphive hooks install`), never silently.
 */
import { test, expect, request as pwRequest, type APIRequestContext, type Page } from '@playwright/test'
import { execFile } from 'node:child_process'
import { existsSync, readFileSync } from 'node:fs'
import { chmod, mkdir, writeFile } from 'node:fs/promises'
import path from 'node:path'
import { fileURLToPath } from 'node:url'
import { promisify } from 'node:util'
import { startWisphiveDaemonServer, type WisphiveDaemonServer } from './fixtures/daemon-server'

const execFileAsync = promisify(execFile)

const SPEC_DIR = path.dirname(fileURLToPath(import.meta.url))
// e2e → frontend → wisphive_web → crates → repo root
const REPO_ROOT = path.resolve(SPEC_DIR, '..', '..', '..', '..')

const PASSWORD = 'wisphive-e2e-password'

/** Each test boots its OWN daemon: the spawn modal renders the free-text
 * project input only while the daemon has NO project history, and a resolved
 * SpawnAgent decision from an earlier test would flip the second test's modal
 * to the dropdown (which lists only historical projects — see the finding in
 * itr#565). Fresh state per test keeps the form deterministic. */
let server: WisphiveDaemonServer
let api: APIRequestContext
/** Marker the stub `claude` writes its argv to — proof the daemon exec'd it. */
let launchMarker: string

/** Same resolution order as fixtures/daemon-server.ts, so the CLI invoked for
 * `hooks install` / `agent list` is the exact binary the daemon runs as (the
 * hook-gate matcher trusts the current_exe-adjacent `wisphive-hook` path). */
function wisphiveBin(): string {
  const fromEnv = process.env.WISPHIVE_BIN
  if (fromEnv) return fromEnv
  for (const profile of ['debug', 'release']) {
    const candidate = path.join(REPO_ROOT, 'target', profile, 'wisphive')
    if (existsSync(candidate)) return candidate
  }
  throw new Error('no wisphive binary found — run `just e2e` or cargo build first')
}

/** Run the wisphive CLI against the fixture's isolated HOME. */
function runCli(args: string[]): Promise<{ stdout: string; stderr: string }> {
  return execFileAsync(wisphiveBin(), args, {
    env: { ...process.env, HOME: server.home },
  })
}

async function mintToken(deviceName: string): Promise<string> {
  const res = await api.post('/api/auth/login', {
    data: { password: PASSWORD, device_name: deviceName },
  })
  expect(res.ok(), `login failed: ${res.status()} ${await res.text()}`).toBeTruthy()
  const body = (await res.json()) as { token: string }
  return body.token
}

/** Each test logs in as a FRESH device so the SpawnAgent sudo gate always
 * prompts (reauth freshness is per-device server-side; see e2e/README.md). */
async function openDashboard(page: Page, deviceName: string): Promise<void> {
  const token = await mintToken(deviceName)
  await page.addInitScript(
    (t: string) => localStorage.setItem('wisphive-web-token', t),
    token,
  )
  await page.goto(`${server.baseURL}/`)
  await expect(page.getByRole('button', { name: '+ Spawn Agent' })).toBeVisible({
    timeout: 15_000,
  })
}

async function attachShot(page: Page, name: string): Promise<void> {
  const p = test.info().outputPath(`${name}.png`)
  await page.screenshot({ path: p })
  await test.info().attach(name, { path: p, contentType: 'image/png' })
}

/** Fill and submit the spawn form. The fixture daemon has no seeded projects,
 * so the modal renders the free-text project input. */
async function submitSpawn(page: Page, project: string, prompt: string): Promise<void> {
  await page.getByRole('button', { name: '+ Spawn Agent' }).click()
  await page.getByPlaceholder('/path/to/project').fill(project)
  await page.getByPlaceholder('What should the agent do?').fill(prompt)
  await page.getByRole('button', { name: 'Spawn', exact: true }).click()
}

/** Approve the queued SpawnAgent row from the Inbox and satisfy the sudo
 * reauth gate (SpawnAgent is sudo-class — sudo_gate.rs). */
async function approveSpawnFromInbox(page: Page): Promise<void> {
  const inboxRow = page.locator('.inbox-item', { hasText: 'SpawnAgent' }).first()
  await expect(inboxRow).toBeVisible({ timeout: 10_000 })
  await attachShot(page, 'spawnagent-waiting-in-inbox')
  await inboxRow.locator('.btn-approve').first().click()

  const sudo = page.locator('.sudo-form')
  await expect(sudo).toBeVisible({ timeout: 10_000 })
  await expect(page.locator('.sudo-tool')).toHaveText('SpawnAgent')
  await sudo.locator('input[type="password"]').fill(PASSWORD)
  await sudo.locator('.login-submit').click()
  await expect(page.locator('.sudo-form')).toHaveCount(0, { timeout: 10_000 })
}

test.beforeEach(async () => {
  server = await startWisphiveDaemonServer()

  // Stub `claude` in the fixture's stub-bin (already FIRST on the daemon's
  // PATH): record the exec's argv, then stay alive as the managed child so
  // the process-registry row remains observable. The child inherits the
  // daemon's isolated HOME, so the marker lands inside the temp dir.
  launchMarker = path.join(server.home, 'claude-launch-argv.txt')
  const stub = path.join(server.home, 'stub-bin', 'claude')
  await writeFile(stub, '#!/bin/sh\nprintf \'%s\\n\' "$@" > "$HOME/claude-launch-argv.txt"\nexec sleep 30\n')
  await chmod(stub, 0o755)

  api = await pwRequest.newContext({
    baseURL: server.baseURL,
    ignoreHTTPSErrors: true,
  })
  const res = await api.post('/api/auth/set-password', {
    data: { password: PASSWORD, device_name: 'e2e-setup' },
  })
  if (!res.ok()) {
    throw new Error(
      `set-password bootstrap failed: ${res.status()} ${await res.text()}\n--- server ---\n${server.output()}`,
    )
  }
})

test.afterEach(async () => {
  await api?.dispose()
  await server?.stop()
})

test.use({ ignoreHTTPSErrors: true })

test('form -> queued -> visible Inbox approval -> sudo reauth -> agent process actually starts', async ({ page }) => {
  // A project the spawn gate accepts: real dir with REAL installer-written
  // Wisphive hooks (the daemon refuses Claude spawns into hookless projects).
  const project = path.join(server.home, 'gated-project')
  await mkdir(project, { recursive: true })
  await runCli(['hooks', 'install', '--project', project])

  await openDashboard(page, 'e2e-spawn-approve')
  await submitSpawn(page, project, 'say hello from the managed spawn e2e')

  // Positive ack tells the operator WHERE to act (itr#565 AC: the pending
  // state is visible and actionable, never a dead end).
  const queued = page.locator('.spawn-status-queued')
  await expect(queued).toBeVisible({ timeout: 10_000 })
  await expect(queued).toContainText('review and approve it from the Inbox')
  await attachShot(page, 'spawn-queued-points-at-inbox')
  await page.getByRole('button', { name: 'Close', exact: true }).click()

  await approveSpawnFromInbox(page)

  // The daemon really exec'd the agent binary: the stub recorded the argv.
  await expect
    .poll(() => existsSync(launchMarker), { timeout: 15_000, message: 'stub claude was never exec\'d' })
    .toBe(true)
  const argv = readFileSync(launchMarker, 'utf8')
  expect(argv).toContain('-p')
  expect(argv).toContain('say hello from the managed spawn e2e')

  // The registry holds the live managed child (CLI output goes to stderr).
  const { stderr } = await runCli(['agent', 'list'])
  expect(stderr).toContain('claude_code')
  expect(stderr).toContain(project)

  // The approval resolved out of the Inbox, and nothing errored.
  await expect(page.locator('.inbox-item', { hasText: 'SpawnAgent' })).toHaveCount(0, {
    timeout: 10_000,
  })
  await expect(page.locator('.error-banner')).toHaveCount(0)

  // The OPERATOR sees the launch (itr#565): before the agent_spawned
  // broadcast + Spawned Processes section, this view said "Agents (0) / No
  // agents connected" right after a genuinely successful spawn — the literal
  // PO symptom surviving on the success path.
  await page.getByRole('button', { name: /^Agents/ }).click()
  const spawnedCard = page.locator('.agent-card', { hasText: 'claude_code' }).first()
  await expect(spawnedCard).toBeVisible({ timeout: 10_000 })
  await expect(spawnedCard).toContainText(project)
  await expect(page.locator('.agent-section', { hasText: 'Spawned Processes' })).toBeVisible()
  await attachShot(page, 'spawn-approved-agent-started')
})

test('a hook-less project queues, then the post-approval refusal surfaces the fix — never silence', async ({ page }) => {
  // The PO's real-world path: pick an existing project that simply does not
  // carry Wisphive hooks. validate_spawn_request accepts it (the dir is
  // fine), so it queues — the refusal only fires at the post-approval
  // process gate. Pre-itr#567 that refusal died in the console.
  const project = path.join(server.home, 'ungated-project')
  await mkdir(project, { recursive: true })

  await openDashboard(page, 'e2e-spawn-ungated')
  await submitSpawn(page, project, 'this launch must be refused visibly')

  await expect(page.locator('.spawn-status-queued')).toBeVisible({ timeout: 10_000 })
  await page.getByRole('button', { name: 'Close', exact: true }).click()

  await approveSpawnFromInbox(page)

  // The worker's gate refusal falls through to the dismissible banner (the
  // modal is closed) and NAMES the blocker and the exact fix.
  const banner = page.locator('.error-banner .error-alert')
  await expect(banner).toBeVisible({ timeout: 15_000 })
  await expect(banner).toContainText('failed to spawn agent')
  await expect(banner).toContainText('wisphive hooks install')
  await attachShot(page, 'ungated-refusal-names-the-fix')
})
