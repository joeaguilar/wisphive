/**
 * itr#624 follow-up — DESKTOP (non-emulated) wheel coverage.
 *
 * The touch-scroll suite runs under mobile emulation by design, which left
 * the desktop wheel path untested — the exact class of gap that let itr#624
 * ship. These specs run in the default Desktop Chrome project context (no
 * touch, no mobile emulation) and assert the decisive test: previously
 * unseen EARLIER text becomes visible and `buffer.active.viewportY` moves.
 *
 * Covers the wheel in both buffer states:
 *   (a) scrollback generated live while attached (pre-existing behavior)
 *   (b) scrollback restored by the seeded attach catchup after a page
 *       refresh, with NO resize (the itr#624 fix)
 */
import { test, expect, request as pwRequest, type APIRequestContext, type Page } from '@playwright/test'
import { startWisphiveDaemonServer, type WisphiveDaemonServer } from './fixtures/daemon-server'

const PASSWORD = 'wisphive-e2e-password'

let server: WisphiveDaemonServer
let api: APIRequestContext

// Desktop context: NO isMobile, NO hasTouch — the project default. Only TLS
// trust is relaxed for the self-signed cert.
test.use({ ignoreHTTPSErrors: true })

test.beforeEach(async () => {
  server = await startWisphiveDaemonServer()
  api = await pwRequest.newContext({ baseURL: server.baseURL, ignoreHTTPSErrors: true })
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

async function openDashboard(page: Page, deviceName: string): Promise<void> {
  const res = await api.post('/api/auth/login', {
    data: { password: PASSWORD, device_name: deviceName },
  })
  expect(res.ok(), `login failed: ${res.status()} ${await res.text()}`).toBeTruthy()
  const { token } = (await res.json()) as { token: string }
  await page.addInitScript((t: string) => {
    localStorage.setItem('wisphive-web-token', t)
    // Named opt-in for the __wisphiveTerm observability handle (itr#624).
    localStorage.setItem('wisphive-term-test-handle', '1')
  }, token)
  await page.goto(`${server.baseURL}/`)
  await expect(page.getByRole('button', { name: '+ Spawn Agent' })).toBeVisible({ timeout: 15_000 })
}

async function createTerminal(page: Page, label: string) {
  page.once('dialog', (d) => void d.accept(label))
  await page.getByRole('button', { name: '+ New terminal' }).click()
  const item = page.locator('.terminals-sidebar-item', { hasText: label })
  await expect(item).toBeVisible({ timeout: 10_000 })
  await expect(item.locator('.term-status-running')).toBeVisible({ timeout: 10_000 })
  return item
}

async function attachTerminal(page: Page, label: string) {
  await page.locator('.terminals-sidebar-item', { hasText: label }).locator('.cmd').click()
  const xtermArea = page.locator('.terminals-terminal-slot .xterm')
  await expect(xtermArea).toBeVisible({ timeout: 10_000 })
  return xtermArea
}

async function visibleRows(page: Page): Promise<string> {
  return await page.locator('.terminals-terminal-slot .xterm-rows').innerText()
}

async function termState(page: Page): Promise<{ viewportY: number; baseY: number; length: number }> {
  return await page.evaluate(() => {
    // eslint-disable-next-line @typescript-eslint/no-explicit-any
    const t = (window as unknown as Record<string, any>).__wisphiveTerm
    if (!t) throw new Error('__wisphiveTerm handle missing — is a terminal mounted?')
    return {
      viewportY: t.buffer.active.viewportY as number,
      baseY: t.buffer.active.baseY as number,
      length: t.buffer.active.length as number,
    }
  })
}

function minMarker(text: string): number {
  return Math.min(...(text.match(/LINE_\d+/g) ?? []).map((l) => Number(l.slice(5))))
}

async function wheelOverTerminal(page: Page, deltaY: number): Promise<void> {
  const box = await page.locator('.terminals-terminal-slot .xterm').boundingBox()
  if (!box) throw new Error('no bounding box for terminal')
  await page.mouse.move(box.x + box.width / 2, box.y + box.height / 2)
  await page.mouse.wheel(0, deltaY)
  await page.waitForTimeout(300)
}

test('desktop wheel pages live scrollback: earlier text becomes visible, wheel-down returns to tail', async ({ page }) => {
  test.setTimeout(120_000)
  await openDashboard(page, 'e2e-desktop-wheel')
  await page.getByRole('button', { name: /^Terminals/ }).click()
  await createTerminal(page, 'wheel-live')
  const xtermArea = await attachTerminal(page, 'wheel-live')

  await page.locator('.terminals-terminal-slot .xterm-helper-textarea').focus()
  await page.keyboard.type('for i in $(seq -w 1 300); do echo LINE_$i; done', { delay: 5 })
  await page.keyboard.press('Enter')
  await expect(xtermArea).toContainText('LINE_300', { timeout: 15_000 })

  const before = await termState(page)
  const beforeText = await visibleRows(page)
  expect(before.baseY, 'live output must have produced scrollback').toBeGreaterThan(0)
  expect(before.viewportY).toBe(before.baseY)

  await wheelOverTerminal(page, -400)
  const after = await termState(page)
  const afterText = await visibleRows(page)
  expect(after.viewportY, 'wheel-up must page the buffer back').toBeLessThan(before.viewportY)
  expect(minMarker(afterText), 'previously-unseen EARLIER lines must be rendered').toBeLessThan(minMarker(beforeText))

  await wheelOverTerminal(page, 4000)
  const back = await termState(page)
  expect(back.viewportY, 'wheel-down must return to the live tail').toBe(back.baseY)
})

test('desktop wheel pages seeded scrollback after a page refresh with NO resize (itr#624)', async ({ page }) => {
  test.setTimeout(120_000)
  await openDashboard(page, 'e2e-desktop-wheel-refresh')
  await page.getByRole('button', { name: /^Terminals/ }).click()
  await createTerminal(page, 'wheel-refresh')
  let xtermArea = await attachTerminal(page, 'wheel-refresh')

  await page.locator('.terminals-terminal-slot .xterm-helper-textarea').focus()
  await page.keyboard.type('for i in $(seq -w 1 300); do echo LINE_$i; done', { delay: 5 })
  await page.keyboard.press('Enter')
  await expect(xtermArea).toContainText('LINE_300', { timeout: 15_000 })
  // Let the daemon's async event batcher persist the tail the seed reads.
  await page.waitForTimeout(1000)

  await page.reload()
  await expect(page.getByRole('button', { name: '+ Spawn Agent' })).toBeVisible({ timeout: 15_000 })
  await page.getByRole('button', { name: /^Terminals/ }).click()
  xtermArea = await attachTerminal(page, 'wheel-refresh')
  await expect(xtermArea).toContainText('LINE_300', { timeout: 15_000 })

  const restored = await termState(page)
  const beforeText = await visibleRows(page)
  expect(restored.baseY, 'seeded catchup must restore scrollback for the creator device').toBeGreaterThan(0)

  await wheelOverTerminal(page, -400)
  const after = await termState(page)
  const afterText = await visibleRows(page)
  expect(after.viewportY, 'wheel must page back right after refresh, no resize').toBeLessThan(restored.viewportY)
  expect(minMarker(afterText), 'previously-unseen EARLIER lines must be rendered').toBeLessThan(minMarker(beforeText))
})
