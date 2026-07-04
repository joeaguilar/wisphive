/**
 * First-run smoke: boot a fresh isolated server, walk the set-password
 * onboarding in a real Chromium, and land on the SPA queue view.
 *
 * The server is standalone `wisphive web serve` with NO daemon behind it —
 * the queue view renders (empty, status dot disconnected) either way, which
 * is exactly the smoke surface we want: TLS listener up, embedded SPA
 * served, auth bootstrap wired, dashboard shell reachable.
 */
import { test, expect } from '@playwright/test'
import os from 'node:os'
import path from 'node:path'
import { startWisphiveServer, type WisphiveServer } from './helpers/server'

const PASSWORD = 'wisphive-e2e-password'

let server: WisphiveServer

test.beforeAll(async () => {
  server = await startWisphiveServer()
})

test.afterAll(async () => {
  if (server) await server.stop()
})

test('first-run: set-password page loads, password sets, queue view renders', async ({
  page,
}) => {
  // State-isolation sanity: the server's HOME is a temp dir, never the
  // real home (whose ~/.wisphive belongs to the live daemon).
  expect(path.resolve(server.home)).not.toBe(path.resolve(os.homedir()))
  expect(path.resolve(server.home).startsWith(path.resolve(os.homedir()) + path.sep)).toBe(false)

  await page.goto(`${server.baseURL}/`)

  // Fresh state dir → first-run onboarding card.
  await expect(page.getByText('Welcome. Set a password to finish setup.')).toBeVisible({
    timeout: 15_000,
  })
  const setPasswordShot = test.info().outputPath('set-password-page.png')
  await page.screenshot({ path: setPasswordShot })
  await test.info().attach('set-password-page', { path: setPasswordShot, contentType: 'image/png' })

  await page.getByLabel('New password').fill(PASSWORD)
  await page.getByLabel('Confirm password').fill(PASSWORD)
  await page.getByRole('button', { name: 'Set password' }).click()

  // On localhost origins the SPA offers an optional passkey-enroll step
  // after set-password; skip it if shown. (Origins that can't host the
  // ceremony go straight to the dashboard.)
  const skipEnroll = page.getByRole('button', { name: 'Skip for now' })
  const queueNav = page.getByRole('button', { name: /^Queue/ })
  await expect(skipEnroll.or(queueNav).first()).toBeVisible({ timeout: 15_000 })
  if (await skipEnroll.isVisible()) {
    await skipEnroll.click()
  }

  // Dashboard shell: sidebar nav + queue layout with the empty state. The
  // default view is the Inbox (itr#435), so click through to the Queue view.
  await expect(queueNav).toBeVisible()
  await queueNav.click()
  await expect(page.locator('.queue-layout')).toBeVisible()
  await expect(page.getByText('No pending decisions')).toBeVisible()
  const queueShot = test.info().outputPath('queue-view.png')
  await page.screenshot({ path: queueShot })
  await test.info().attach('queue-view', { path: queueShot, contentType: 'image/png' })
})
