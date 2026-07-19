/**
 * itr#591 — reconcile-on-start: pinned terminal sessions survive a daemon
 * restart. First Terminals-view Playwright spec repo-wide (coverage hole
 * recorded on itr#591 by the itr#589 review).
 *
 * The full user-value claim, end to end against a REAL daemon:
 *
 * 1. Operator opens the Terminals view, spawns a session, types a marker
 *    into the live PTY (real xterm.js -> ws bridge -> daemon PTY), and PINS
 *    it via the web UI. A second session stays unpinned as the control.
 * 2. The daemon is gracefully restarted (SIGTERM -> fresh process, same
 *    HOME + port — fixtures/daemon-server.ts `restart()`).
 * 3. After reload, the pinned session is back in Active — running, still
 *    starred — and ATTACHING it shows the pre-restart marker (scrollback
 *    seeded from terminal_events) plus the honesty banner. The unpinned
 *    session is NOT running — it lands as `orphaned` or `killed` depending
 *    on which side of the SIGTERM race persisted first.
 *
 * Gate note: the respawned child re-enters the wisphive-hook gate by
 * construction (same spawn path + WISPHIVE_TERMINAL_SESSION_ID re-injection)
 * — asserted at the Rust level in
 * crates/wisphive_daemon/src/terminal.rs::respawn_session_revives_pinned_orphan_with_seeded_scrollback
 * and server.rs::restart_respawns_pinned_sessions_and_leaves_the_rest_orphaned.
 */
import { test, expect, request as pwRequest, type APIRequestContext, type Page } from '@playwright/test'
import { startWisphiveDaemonServer, type WisphiveDaemonServer } from './fixtures/daemon-server'

const PASSWORD = 'wisphive-e2e-password'
const MARKER = 'respawn-marker-xyz'

let server: WisphiveDaemonServer
let api: APIRequestContext

test.beforeEach(async () => {
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

test.afterEach(async () => {
  await api?.dispose()
  await server?.stop()
})

test.use({ ignoreHTTPSErrors: true })

async function openDashboard(page: Page, deviceName: string): Promise<void> {
  const res = await api.post('/api/auth/login', {
    data: { password: PASSWORD, device_name: deviceName },
  })
  expect(res.ok(), `login failed: ${res.status()} ${await res.text()}`).toBeTruthy()
  const { token } = (await res.json()) as { token: string }
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

/** Create a terminal via the sidebar toolbar; the label arrives through the
 * native prompt() dialog. Returns the sidebar item locator. */
async function createTerminal(page: Page, label: string) {
  page.once('dialog', (d) => void d.accept(label))
  await page.getByRole('button', { name: '+ New terminal' }).click()
  const item = page.locator('.terminals-sidebar-item', { hasText: label })
  await expect(item).toBeVisible({ timeout: 10_000 })
  await expect(item.locator('.term-status-running')).toBeVisible({ timeout: 10_000 })
  return item
}

test('pinned session respawns live with scrollback after a daemon restart; unpinned orphans', async ({ page }) => {
  test.setTimeout(120_000)
  await openDashboard(page, 'e2e-term-respawn')
  await page.getByRole('button', { name: /^Terminals/ }).click()

  // ── Before: one pinned session with a typed marker, one unpinned control.
  const pinnedItem = await createTerminal(page, 'respawn-pinned')

  // Attach (click a button-free region — e2e/README.md gotcha #4) and type
  // the marker into the real PTY.
  await pinnedItem.locator('.cmd').click()
  const xtermArea = page.locator('.terminals-terminal-slot .xterm')
  await expect(xtermArea).toBeVisible({ timeout: 10_000 })
  await page.locator('.terminals-terminal-slot .xterm-helper-textarea').focus()
  await page.keyboard.type(`echo ${MARKER}`, { delay: 20 })
  await page.keyboard.press('Enter')
  // The echoed output proves the input round-tripped through the daemon PTY
  // (and is therefore persisted in terminal_events for the respawn seed).
  await expect(xtermArea).toContainText(MARKER, { timeout: 15_000 })

  await pinnedItem.getByRole('button', { name: 'Pin session respawn-pinned' }).click()
  await expect(pinnedItem.locator('.term-pinned')).toBeVisible({ timeout: 10_000 })

  const unpinnedItem = await createTerminal(page, 'respawn-unpinned')
  await expect(unpinnedItem.locator('.term-status-running')).toBeVisible()
  await attachShot(page, 'before-restart-pinned-and-control')

  // ── Restart the daemon (graceful SIGTERM cycle, same HOME + port).
  await server.restart()
  await page.reload()
  await expect(page.getByRole('button', { name: '+ Spawn Agent' })).toBeVisible({
    timeout: 15_000,
  })
  await page.getByRole('button', { name: /^Terminals/ }).click()

  // ── After: pinned is BACK — running, still starred, in Active.
  const pinnedAfter = page.locator('.terminals-sidebar-item', { hasText: 'respawn-pinned' })
  await expect(pinnedAfter).toBeVisible({ timeout: 15_000 })
  await expect(pinnedAfter.locator('.term-status-running')).toBeVisible({ timeout: 15_000 })
  await expect(pinnedAfter.locator('.term-pinned')).toBeVisible()

  // Unpinned control must NOT come back live. Its landed status races at
  // SIGTERM: the child-kill waiter can persist `killed` before the daemon
  // exits, otherwise the next startup sweep records `orphaned` — the Rust
  // server test asserts only `!= running` for the same reason. A `killed`
  // row lives in the collapsed-by-default Archived section, so expand it
  // when present before locating the row.
  const archivedExpand = page.getByLabel('Archived terminals — expand')
  if ((await archivedExpand.count()) > 0) await archivedExpand.click()
  const unpinnedAfter = page.locator('.terminals-sidebar-item', { hasText: 'respawn-unpinned' })
  await expect(unpinnedAfter).toBeVisible({ timeout: 10_000 })
  await expect(
    unpinnedAfter.locator('.term-status-orphaned, .term-status-killed'),
  ).toBeVisible()
  await expect(unpinnedAfter.locator('.term-status-running')).toHaveCount(0)
  await expect(unpinnedAfter.locator('.term-pinned')).toHaveCount(0)
  await attachShot(page, 'after-restart-pinned-active-unpinned-orphaned')

  // ── Attachable, with the pre-restart scrollback seeded + honesty banner.
  await pinnedAfter.locator('.cmd').click()
  const xtermAfter = page.locator('.terminals-terminal-slot .xterm')
  await expect(xtermAfter).toBeVisible({ timeout: 10_000 })
  await expect(xtermAfter).toContainText(MARKER, { timeout: 15_000 })
  await expect(xtermAfter).toContainText('pinned session respawned', { timeout: 15_000 })

  // Still a LIVE shell, not a replay: typing into it round-trips.
  await page.locator('.terminals-terminal-slot .xterm-helper-textarea').focus()
  await page.keyboard.type('echo alive-after-respawn', { delay: 20 })
  await page.keyboard.press('Enter')
  await expect(xtermAfter).toContainText('alive-after-respawn', { timeout: 15_000 })
  await attachShot(page, 'after-restart-attached-scrollback-and-live-shell')
})
