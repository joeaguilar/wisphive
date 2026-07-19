/**
 * itr#567 — daemon errors must reach the operator's eyes.
 *
 * Drives the REAL daemon (fixtures/daemon-server.ts) from the real SPA and
 * proves the two new surfaces end-to-end:
 *
 * 1. The spawn modal's correlated status: a submit whose request the daemon
 *    refuses (nonexistent project → `validate_spawn_request`) renders the
 *    daemon's refusal text inside the modal via the correlated
 *    `command_error` reply, instead of the modal closing as if it worked.
 * 2. The positive ack: a valid submit shows "queued for approval" from the
 *    correlated `agent_spawn_queued` reply.
 * 3. The dismissible error banner: after the modal is closed, a human DENY
 *    of the queued spawn makes the async worker's correlated `command_error`
 *    fall through to the banner — the operator sees WHY nothing spawned.
 *
 * Before this change every one of those frames hit
 * `case "error": console.error(...)` and vanished (useWisphive.ts).
 */
import { test, expect, request as pwRequest, type APIRequestContext, type Page } from '@playwright/test'
import { mkdir } from 'node:fs/promises'
import path from 'node:path'
import { startWisphiveDaemonServer, type WisphiveDaemonServer } from './fixtures/daemon-server'

const PASSWORD = 'wisphive-e2e-password'

let server: WisphiveDaemonServer
let api: APIRequestContext

async function mintToken(deviceName: string): Promise<string> {
  const res = await api.post('/api/auth/login', {
    data: { password: PASSWORD, device_name: deviceName },
  })
  expect(res.ok(), `login failed: ${res.status()} ${await res.text()}`).toBeTruthy()
  const body = (await res.json()) as { token: string }
  return body.token
}

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

/** Open the spawn modal and submit project+prompt. The fixture daemon has no
 * seeded projects, so the modal renders the free-text project input. */
async function submitSpawn(page: Page, project: string, prompt: string): Promise<void> {
  await page.getByRole('button', { name: '+ Spawn Agent' }).click()
  await page.getByPlaceholder('/path/to/project').fill(project)
  await page.getByPlaceholder('What should the agent do?').fill(prompt)
  await page.getByRole('button', { name: 'Spawn', exact: true }).click()
}

test.beforeAll(async () => {
  server = await startWisphiveDaemonServer()
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

test.afterAll(async () => {
  await api?.dispose()
  await server?.stop()
})

test.use({ ignoreHTTPSErrors: true })

test('a refused spawn surfaces the daemon message in the modal instead of closing', async ({ page }) => {
  await openDashboard(page, 'e2e-error-modal')

  const bogusProject = path.join(server.home, 'does-not-exist')
  await submitSpawn(page, bogusProject, 'exercise the refusal path')

  // The daemon's synchronous validate_spawn_request refusal comes back as a
  // correlated command_error and lands INSIDE the still-open modal.
  const refusal = page.locator('.spawn-status-refused')
  await expect(refusal).toBeVisible({ timeout: 10_000 })
  await expect(refusal).toContainText('invalid agent spawn request')
  await expect(refusal).toContainText('project path does not resolve')
  // The modal did not close pretending success, and the failure did not
  // double-report into the banner (the modal owns it while open).
  await expect(page.locator('.spawn-form')).toBeVisible()
  await expect(page.locator('.error-banner')).toHaveCount(0)
  await attachShot(page, 'spawn-refused-in-modal')

  await page.getByRole('button', { name: 'Cancel' }).click()
})

test('a queued spawn confirms positively, and a human deny lands in the error banner', async ({ page }) => {
  await openDashboard(page, 'e2e-error-banner')

  const project = path.join(server.home, 'real-project')
  await mkdir(project, { recursive: true })
  await submitSpawn(page, project, 'wait for approval then get denied')

  // Positive confirmation via the correlated agent_spawn_queued ack.
  const queuedStatus = page.locator('.spawn-status-queued')
  await expect(queuedStatus).toBeVisible({ timeout: 10_000 })
  await expect(queuedStatus).toContainText('queued for approval')
  await attachShot(page, 'spawn-queued-in-modal')
  await page.getByRole('button', { name: 'Close', exact: true }).click()

  // The queued SpawnAgent decision is now in the Inbox — deny it from the
  // collapsed row (deny is not sudo-gated; see e2e/README.md gotcha 1).
  const inboxRow = page.locator('.inbox-item', { hasText: 'SpawnAgent' }).first()
  await expect(inboxRow).toBeVisible({ timeout: 10_000 })
  await inboxRow.locator('.btn-deny').first().click()

  // The spawn worker's correlated command_error now has no open modal to
  // claim it (clearSpawnStatus ran on close), so it falls through to the
  // dismissible banner — the operator sees exactly why nothing spawned.
  const banner = page.locator('.error-banner .error-alert')
  await expect(banner).toBeVisible({ timeout: 10_000 })
  await expect(banner).toContainText('failed to spawn agent')
  await expect(banner).toContainText('denied by human reviewer')
  await attachShot(page, 'deny-surfaced-in-banner')

  // Dismissible: the operator can clear it.
  await banner.getByRole('button', { name: /dismiss/i }).click()
  await expect(page.locator('.error-banner')).toHaveCount(0)
})
