/**
 * Core web flows against a REAL daemon: password login (valid + invalid),
 * decision queue, approve/deny round-trips driven end-to-end by a
 * socket-level hook fixture, and device revocation.
 *
 * Server mode: `wisphive daemon start --web` (see fixtures/daemon-server.ts)
 * because standalone `wisphive web serve` has no decision queue — it only
 * bridges WebSockets to a daemon socket it expects to already exist.
 *
 * The hook fixture (fixtures/hook-client.ts) speaks the real wire protocol
 * (Hello + DecisionRequest over newline-delimited JSON) and BLOCKS on the
 * open Unix socket until the browser resolves the decision, so approve/deny
 * here proves the full hook → daemon → WS → UI → daemon → hook loop.
 *
 * NOT covered here (intentionally):
 * - AskUserQuestion answer-passing: blocked on the itr#250 fix; that spec
 *   must land alongside the fix as its regression test.
 * - A devices UI: the SPA has no devices view today (only the backend
 *   /api/devices routes and the `wisphive web devices` CLI exist), so the
 *   devices flow is exercised at the HTTP API layer below.
 */
import { test, expect, request as pwRequest, type APIRequestContext, type Page } from '@playwright/test'
import { startWisphiveDaemonServer, type WisphiveDaemonServer } from './fixtures/daemon-server'
import { sendDecisionRequest } from './fixtures/hook-client'

const PASSWORD = 'wisphive-e2e-password'
const WRONG_PASSWORD = 'not-the-right-password'

let server: WisphiveDaemonServer
let api: APIRequestContext

async function mintToken(deviceName: string): Promise<{ token: string; deviceId: string }> {
  const res = await api.post('/api/auth/login', {
    data: { password: PASSWORD, device_name: deviceName },
  })
  expect(res.ok(), `login for ${deviceName} failed: ${res.status()} ${await res.text()}`).toBeTruthy()
  const body = (await res.json()) as { device_id: string; token: string }
  return { token: body.token, deviceId: body.device_id }
}

/** Land `page` on the authed dashboard by minting a device token via the
 * API and seeding it into localStorage before the SPA boots (same slot
 * Login.tsx writes — see src/api.ts TOKEN_STORAGE_KEY). */
async function openDashboard(page: Page, deviceName: string): Promise<void> {
  const { token } = await mintToken(deviceName)
  await page.addInitScript(
    (t: string) => localStorage.setItem('wisphive-web-token', t),
    token,
  )
  await page.goto(`${server.baseURL}/`)
  // Inbox is the default view (itr#435); this spec drives the Queue view, so
  // navigate to it before asserting the queue layout.
  const queueNav = page.getByRole('button', { name: /^Queue/ })
  await expect(queueNav).toBeVisible({ timeout: 15_000 })
  await queueNav.click()
  await expect(page.locator('.queue-layout')).toBeVisible()
}

async function attachShot(page: Page, name: string): Promise<void> {
  const p = test.info().outputPath(`${name}.png`)
  await page.screenshot({ path: p })
  await test.info().attach(name, { path: p, contentType: 'image/png' })
}

test.beforeAll(async () => {
  server = await startWisphiveDaemonServer()
  api = await pwRequest.newContext({
    baseURL: server.baseURL,
    ignoreHTTPSErrors: true,
  })
  // First-run bootstrap: set the admin password once via the API so the
  // login specs exercise the sign-in page, not onboarding (which
  // smoke.spec.ts already covers).
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
  if (api) await api.dispose()
  if (server) await server.stop()
})

test('login: invalid credentials are rejected, valid credentials reach the queue', async ({
  page,
}) => {
  await page.goto(`${server.baseURL}/`)
  await expect(page.getByText('Sign in to review pending decisions.')).toBeVisible({
    timeout: 15_000,
  })

  // Wrong password → inline error, still on the login card.
  await page.getByLabel('Password', { exact: true }).fill(WRONG_PASSWORD)
  await page.getByRole('button', { name: 'Sign in', exact: true }).click()
  await expect(page.getByRole('alert')).toHaveText('Invalid password.', { timeout: 15_000 })
  await attachShot(page, 'login-invalid')

  // The per-IP throttle briefly locks after a failed attempt. Rather than
  // encode the backoff schedule as a fixed sleep (which silently starts
  // flaking if the lockout is ever lengthened), retry the valid login until
  // it is no longer throttled and the dashboard renders.
  await expect(async () => {
    await page.getByLabel('Password', { exact: true }).fill(PASSWORD)
    await page.getByRole('button', { name: 'Sign in', exact: true }).click()
    await expect(page.getByRole('button', { name: /^Queue/ })).toBeVisible({ timeout: 3_000 })
  }).toPass({ timeout: 20_000 })
  // Default view is the Inbox (itr#435); switch to the Queue view this test asserts.
  await page.getByRole('button', { name: /^Queue/ }).click()
  await expect(page.locator('.queue-layout')).toBeVisible()
  await expect(page.getByText('No pending decisions')).toBeVisible()
  await attachShot(page, 'login-valid-dashboard')
})

test('queue: a fixture hook decision appears and approving resolves it with allow', async ({
  page,
}) => {
  await openDashboard(page, 'e2e-approver')
  await expect(page.getByText('No pending decisions')).toBeVisible({ timeout: 15_000 })

  // Inject a real DecisionRequest over the daemon's Unix socket. `Read` is
  // deliberately not sudo-class (crates/wisphive_daemon/src/sudo_gate.rs),
  // so a web approve resolves without the reauth modal.
  const pending = await sendDecisionRequest(server.socketPath, {
    toolName: 'Read',
    toolInput: { file_path: '/tmp/wisphive-e2e-read-target.txt' },
  })
  try {
    const item = page.locator('.queue-item', { hasText: 'Read' })
    await expect(item).toBeVisible({ timeout: 15_000 })
    await expect(item).toContainText('e2e-hook-fixture')
    await attachShot(page, 'queue-pending-decision')

    await item.click()
    await item.locator('.queue-item-actions .btn-approve').click()

    // The blocked fixture hook unblocks with an allow.
    const resolution = await pending.resolution
    expect(resolution.decision).toBe('approve')

    await expect(page.getByText('No pending decisions')).toBeVisible({ timeout: 15_000 })
    await attachShot(page, 'queue-after-approve')
  } finally {
    pending.close()
  }
})

test('queue: denying with a message returns deny + reason to the fixture hook', async ({
  page,
}) => {
  const DENY_REASON = 'denied by the e2e spec - do not run this'

  await openDashboard(page, 'e2e-denier')
  await expect(page.getByText('No pending decisions')).toBeVisible({ timeout: 15_000 })

  const pending = await sendDecisionRequest(server.socketPath, {
    toolName: 'Bash',
    toolInput: { command: 'rm -rf /tmp/wisphive-e2e-pretend-target' },
  })
  try {
    const item = page.locator('.queue-item', { hasText: 'Bash' })
    await expect(item).toBeVisible({ timeout: 15_000 })
    await item.click()

    // Selecting the item opens the detail view; deny with feedback so the
    // reason round-trips to the blocked hook.
    const detail = page.locator('.detail-view')
    await expect(detail).toBeVisible()
    await detail.getByRole('button', { name: 'Deny + Message' }).click()
    const modal = page.locator('.modal-content')
    await expect(modal).toBeVisible()
    await attachShot(page, 'deny-message-modal')
    await modal.locator('.modal-textarea').fill(DENY_REASON)
    await modal.locator('.modal-actions .btn-deny').click()

    const resolution = await pending.resolution
    expect(resolution.decision).toBe('deny')
    expect(resolution.message).toBe(DENY_REASON)

    await expect(page.getByText('No pending decisions')).toBeVisible({ timeout: 15_000 })
  } finally {
    pending.close()
  }
})

test('devices: list shows enrolled devices; revocation 401s the revoked token', async () => {
  // Two fresh devices — A revokes B. (The SPA has no devices view yet, so
  // this exercises the backend contract the CLI and future UI sit on.)
  const deviceA = await mintToken('e2e-device-a')
  const deviceB = await mintToken('e2e-device-b')

  const asA = await pwRequest.newContext({
    baseURL: server.baseURL,
    ignoreHTTPSErrors: true,
    extraHTTPHeaders: { Authorization: `Bearer ${deviceA.token}` },
  })
  const asB = await pwRequest.newContext({
    baseURL: server.baseURL,
    ignoreHTTPSErrors: true,
    extraHTTPHeaders: { Authorization: `Bearer ${deviceB.token}` },
  })
  try {
    // Both tokens work before revocation.
    expect((await asA.get('/api/me')).status()).toBe(200)
    expect((await asB.get('/api/me')).status()).toBe(200)

    // The devices list renders every enrolled device, unrevoked.
    type DeviceRow = { id: string; name: string; revoked_at: string | null }
    const listRes = await asA.get('/api/devices')
    expect(listRes.status()).toBe(200)
    const devices = (await listRes.json()) as DeviceRow[]
    const rowB = devices.find((d) => d.id === deviceB.deviceId)
    expect(rowB, 'device B missing from /api/devices').toBeTruthy()
    expect(rowB?.name).toBe('e2e-device-b')
    expect(rowB?.revoked_at).toBeNull()
    expect(devices.some((d) => d.id === deviceA.deviceId)).toBe(true)

    // Revoke B from A (password re-entry required by the endpoint).
    const revokeRes = await asA.post(`/api/devices/${deviceB.deviceId}/revoke`, {
      data: { password: PASSWORD },
    })
    expect(revokeRes.status(), await revokeRes.text()).toBe(200)

    // B's next request 401s; A is untouched.
    expect((await asB.get('/api/me')).status()).toBe(401)
    expect((await asA.get('/api/me')).status()).toBe(200)

    // And the list now records the revocation.
    const after = (await (await asA.get('/api/devices')).json()) as DeviceRow[]
    expect(after.find((d) => d.id === deviceB.deviceId)?.revoked_at).not.toBeNull()
  } finally {
    await asA.dispose()
    await asB.dispose()
  }
})
