/**
 * Accessibility harden pass — runtime proof (real daemon + web server, isolated
 * HOME; the live ~/.wisphive is never touched).
 *
 * The unit suite asserts the ARIA *props* render; this drives the *behaviours*
 * that only exist at runtime in a real browser:
 *
 *   1. The help modal (opened with `?`) is exposed as a labelled dialog
 *      (`role="dialog"` + `aria-modal` + accessible name).
 *   2. Focus is trapped inside it — Tabbing repeatedly never escapes the dialog.
 *   3. Closing it (Escape) restores focus to the control that opened it.
 *   4. The connection status is a labelled live region (`role="status"` +
 *      aria-label), so its state is not carried by colour alone.
 */
import { test, expect, request as pwRequest, type APIRequestContext, type Page } from '@playwright/test'
import { startWisphiveDaemonServer, type WisphiveDaemonServer } from './fixtures/daemon-server'

const PASSWORD = 'wisphive-a11y-password'

let server: WisphiveDaemonServer
let api: APIRequestContext

async function mintToken(deviceName: string): Promise<string> {
  const res = await api.post('/api/auth/login', {
    data: { password: PASSWORD, device_name: deviceName },
  })
  expect(res.ok(), `login failed: ${res.status()} ${await res.text()}`).toBeTruthy()
  return ((await res.json()) as { token: string }).token
}

async function openApp(page: Page, deviceName: string): Promise<void> {
  const token = await mintToken(deviceName)
  await page.addInitScript((t: string) => localStorage.setItem('wisphive-web-token', t), token)
  await page.goto(`${server.baseURL}/`)
  await expect(page.locator('.inbox')).toBeVisible({ timeout: 15_000 })
}

test.beforeAll(async () => {
  server = await startWisphiveDaemonServer()
  api = await pwRequest.newContext({ baseURL: server.baseURL, ignoreHTTPSErrors: true })
  const res = await api.post('/api/auth/set-password', {
    data: { password: PASSWORD, device_name: 'a11y-setup' },
  })
  if (!res.ok()) {
    throw new Error(`set-password bootstrap failed: ${res.status()} ${await res.text()}`)
  }
})

test.afterAll(async () => {
  if (api) await api.dispose()
  if (server) await server.stop()
})

test('help modal is a labelled, focus-trapping dialog that restores focus; status is labelled', async ({
  page,
}) => {
  await openApp(page, 'a11y-smoke')

  // ── 4. Connection status is a labelled live region, not colour-only ──────
  const status = page.locator('.status-dot')
  await expect(status).toHaveAttribute('role', 'status')
  await expect(status).toHaveAttribute('aria-label', /connected/i)

  // Focus a real trigger so we can prove focus RESTORE later. The Config nav
  // button is always present.
  const configBtn = page.getByRole('button', { name: /^Config/ })
  await configBtn.focus()
  await expect(configBtn).toBeFocused()

  // ── 1. Open help with `?` → a labelled modal dialog ──────────────────────
  await page.keyboard.press('Shift+Slash') // '?'
  const dialog = page.getByRole('dialog', { name: 'Keyboard Shortcuts' })
  await expect(dialog).toBeVisible()
  await expect(dialog).toHaveAttribute('aria-modal', 'true')

  // ── 2. Focus is trapped: Tab many times, focus never leaves the dialog ───
  for (let i = 0; i < 8; i++) await page.keyboard.press('Tab')
  const trapped = await page.evaluate(() => {
    const d = document.querySelector('[role="dialog"]')
    return !!d && d.contains(document.activeElement)
  })
  expect(trapped, 'focus escaped the modal on Tab').toBe(true)

  // Shift+Tab backward also stays inside.
  for (let i = 0; i < 3; i++) await page.keyboard.press('Shift+Tab')
  const trappedBack = await page.evaluate(() => {
    const d = document.querySelector('[role="dialog"]')
    return !!d && d.contains(document.activeElement)
  })
  expect(trappedBack, 'focus escaped the modal on Shift+Tab').toBe(true)

  // ── 3. Escape closes and restores focus to the opener ────────────────────
  await page.keyboard.press('Escape')
  await expect(page.locator('[role="dialog"]')).toHaveCount(0)
  await expect(configBtn, 'focus was not restored to the opener').toBeFocused()
})
