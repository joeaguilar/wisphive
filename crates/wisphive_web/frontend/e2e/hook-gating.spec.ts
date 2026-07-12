/**
 * itr#460 — Cockpit hook-gating runtime evidence.
 *
 * Drives the real "gate a project from the web UI" flow end-to-end against a
 * REAL `wisphive daemon start --web` (isolated HOME — the live ~/.wisphive is
 * never touched), proving the security-sensitive path that unit tests can't:
 * a web device writing `.claude/settings.json` into a filesystem path, gated by
 * the sudo-reauth modal, with the write verified ON DISK.
 *
 *   Test A — path-to-gate + sudo-reauth: type an un-listed project path, confirm
 *     the write preview, satisfy the SudoModal (reauth), and assert wisphive
 *     hooks are actually written to <dir>/.claude/settings.json.
 *   Test B — card badge + Gate button: a project with real activity shows a
 *     "Not gated" badge; clicking Gate (reauth still fresh) flips it to "Gated"
 *     and writes the hooks on disk.
 *
 * The daemon's install path is the real `wisphive_daemon::hook_install`; the
 * only thing synthesised is the operator's clicks (Playwright).
 */
import { test, expect, request as pwRequest, type APIRequestContext, type Page } from '@playwright/test'
import { chmodSync, existsSync, mkdtempSync, readFileSync, rmSync, writeFileSync } from 'node:fs'
import path from 'node:path'
import os from 'node:os'
import { startWisphiveDaemonServer, type WisphiveDaemonServer } from './fixtures/daemon-server'
import { sendDecisionRequest } from './fixtures/hook-client'

const PASSWORD = 'wisphive-e2e-password'
let server: WisphiveDaemonServer
let api: APIRequestContext
// One shared device across both tests: sudo-reauth freshness is server-side and
// per-device (5-min TTL), so reauthing in Test A keeps Test B's Gate click from
// re-prompting — the realistic "stay-fresh-across-gates" behaviour.
let sharedToken: string
const tmpProjects: string[] = []

function freshProjectDir(tag: string): string {
  const dir = mkdtempSync(path.join(os.tmpdir(), `wisphive-gate-${tag}-`))
  tmpProjects.push(dir)
  return dir
}

function claudeHookInstalled(projectDir: string): boolean {
  const settings = path.join(projectDir, '.claude', 'settings.json')
  if (!existsSync(settings)) return false
  const json = JSON.parse(readFileSync(settings, 'utf8')) as {
    hooks?: { PreToolUse?: Array<{ hooks?: Array<{ command?: string }> }> }
  }
  const pre = json.hooks?.PreToolUse ?? []
  return pre.some((r) => (r.hooks ?? []).some((h) => (h.command ?? '').includes('wisphive-hook')))
}

async function mintToken(name: string): Promise<string> {
  const res = await api.post('/api/auth/login', { data: { password: PASSWORD, device_name: name } })
  expect(res.ok(), `login failed: ${res.status()}`).toBeTruthy()
  return ((await res.json()) as { token: string }).token
}

async function openProjects(page: Page): Promise<void> {
  await page.addInitScript((t: string) => localStorage.setItem('wisphive-web-token', t), sharedToken)
  await page.goto(`${server.baseURL}/`)
  await page.getByRole('button', { name: /^Projects/ }).click()
  await expect(page.locator('.projects-view')).toBeVisible({ timeout: 15_000 })
}

async function shot(page: Page, name: string): Promise<void> {
  const dir = path.join(
    path.dirname(new URL(import.meta.url).pathname),
    '..', '..', '..', '..',
    'sprint', 'sprint-2-2026-07-03-command-center-inbox', 'blitz', 'evidence',
  )
  const p = path.join(dir, `gating-${name}.png`)
  await page.screenshot({ path: p, fullPage: true })
  await test.info().attach(`gating-${name}`, { path: p, contentType: 'image/png' })
}

test.beforeAll(async () => {
  server = await startWisphiveDaemonServer()
  // Global mode active so a fully-installed project audits as "Gated". The
  // daemon's secure-mode read requires the file to be a 0600 regular file.
  const modePath = path.join(server.home, '.wisphive', 'mode')
  writeFileSync(modePath, 'active')
  chmodSync(modePath, 0o600)
  api = await pwRequest.newContext({ baseURL: server.baseURL, ignoreHTTPSErrors: true })
  const res = await api.post('/api/auth/set-password', { data: { password: PASSWORD, device_name: 'e2e-setup' } })
  if (!res.ok()) throw new Error(`set-password failed: ${res.status()} ${await res.text()}`)
  sharedToken = await mintToken('e2e-gate')
})

test.afterAll(async () => {
  if (api) await api.dispose()
  if (server) await server.stop()
  for (const d of tmpProjects) rmSync(d, { recursive: true, force: true })
})

test('path-to-gate: confirm + sudo-reauth writes wisphive hooks to disk', async ({ page }) => {
  test.setTimeout(120_000)
  const projectDir = freshProjectDir('pathgate')
  expect(claudeHookInstalled(projectDir), 'precondition: dir starts un-gated').toBe(false)

  await openProjects(page)

  // Type the un-listed path and submit.
  await page.locator('.gate-path-input').fill(projectDir)
  await page.locator('.gate-path-btn').click()

  // Confirm-before-write preview names the exact files.
  const confirm = page.locator('.modal-content', { hasText: 'Gate this project?' })
  await expect(confirm).toBeVisible()
  await expect(confirm).toContainText('.claude/settings.json')
  await shot(page, 'a1-confirm-preview')
  await confirm.getByRole('button', { name: 'Install hooks' }).click()

  // Sudo gate: a web device with no fresh reauth must satisfy the SudoModal,
  // scoped to the InstallHooks action.
  const sudo = page.locator('.sudo-form')
  await expect(sudo).toBeVisible({ timeout: 15_000 })
  await expect(page.locator('.sudo-tool')).toHaveText('InstallHooks')
  await shot(page, 'a2-sudo-reauth')
  await sudo.locator('input[type="password"]').fill(PASSWORD)
  await sudo.locator('.login-submit').click()

  // The install replays after reauth; assert the REAL on-disk write.
  await expect(async () => {
    expect(claudeHookInstalled(projectDir)).toBe(true)
  }).toPass({ timeout: 15_000 })
  await expect(page.locator('.sudo-form')).toHaveCount(0)
  await shot(page, 'a3-installed')

  // The daemon wrote a genuine wisphive PreToolUse hook + the codex config.
  expect(existsSync(path.join(projectDir, '.codex', 'hooks.json'))).toBe(true)
})

test('card badge: an active project shows Not gated, Gate flips it to Gated on disk', async ({ page }) => {
  test.setTimeout(120_000)
  const projectDir = freshProjectDir('cardgate')

  // Give the project real activity so it appears as a Projects card: inject a
  // decision over the socket and approve it via the API so it lands in the
  // decision log the projects query aggregates.
  const pending = await sendDecisionRequest(server.socketPath, {
    toolName: 'Read',
    toolInput: { file_path: `${projectDir}/x.txt` },
    project: projectDir,
    agentId: 'cc-gate-card',
  })
  try {
    await openProjects(page)
    const card = page.locator('.session-item', { hasText: projectDir.split('/').pop() as string })
    await expect(card).toBeVisible({ timeout: 15_000 })
    // Un-gated project → amber "Not gated" badge + a Gate button.
    await expect(card.locator('.gate-badge')).toHaveText('Not gated', { timeout: 15_000 })
    await shot(page, 'b1-not-gated-badge')

    await card.locator('.gate-repair-btn').click()
    const confirm = page.locator('.modal-content', { hasText: 'Gate this project?' })
    await expect(confirm).toBeVisible()
    await confirm.getByRole('button', { name: 'Install hooks' }).click()

    // Reauth is still fresh from Test A (5-min TTL) → no SudoModal; install runs.
    await expect(card.locator('.gate-badge')).toHaveText('Gated', { timeout: 15_000 })
    expect(claudeHookInstalled(projectDir)).toBe(true)
    await shot(page, 'b2-gated-badge')
  } finally {
    pending.close()
  }
})
