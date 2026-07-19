/**
 * itr#624 / itr#479 — terminal scrollback must be REACHABLE, and touch must
 * page it.
 *
 * DECISIVE TEST (the reason this suite exists): "content moved" is NOT the
 * oracle. The only valid proof of scrolling is that previously-unseen EARLIER
 * text becomes visible and `term.buffer.active.viewportY` actually changed.
 * Same rows sliding around = native pane scroll, not scrollback paging. The
 * old vitest coverage asserted a scroll call against a MOCK under jsdom and
 * shipped the itr#624 regression green — these specs drive the real app
 * (real daemon, real PTY, real xterm, CDP touch through the real input
 * pipeline) and assert real buffer movement via the `__wisphiveTerm` handle.
 *
 * itr#624 root cause covered by the refresh spec below: an attach catchup
 * used to carry only the current vt100 screen, so after ANY page refresh the
 * client buffer held zero scrollback — wheel and touch alike had literally
 * nothing to scroll into (a resize only appeared to fix it by reflow). The
 * daemon now seeds the catchup with the persisted output tail for requesters
 * that pass the replay ACL; unauthorized requesters keep the legacy
 * screen-only catchup (third spec).
 */
import { execFile } from 'node:child_process'
import path from 'node:path'
import { fileURLToPath } from 'node:url'
import { promisify } from 'node:util'
import { test, expect, request as pwRequest, type APIRequestContext, type Page } from '@playwright/test'
import { startWisphiveDaemonServer, type WisphiveDaemonServer } from './fixtures/daemon-server'

const execFileP = promisify(execFile)
const PASSWORD = 'wisphive-e2e-password'
// e2e → frontend → wisphive_web → crates → repo root (mirrors fixtures/).
const REPO_ROOT = path.resolve(path.dirname(fileURLToPath(import.meta.url)), '..', '..', '..', '..')

let server: WisphiveDaemonServer
let api: APIRequestContext

test.use({
  ignoreHTTPSErrors: true,
  hasTouch: true,
  isMobile: true,
  viewport: { width: 412, height: 915 },
})

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

async function openDashboard(page: Page, deviceName: string): Promise<void> {
  const res = await api.post('/api/auth/login', {
    data: { password: PASSWORD, device_name: deviceName },
  })
  expect(res.ok(), `login failed: ${res.status()} ${await res.text()}`).toBeTruthy()
  const { token } = (await res.json()) as { token: string }
  await page.addInitScript((t: string) => {
    localStorage.setItem('wisphive-web-token', t)
    // Named opt-in (absent by default in production): makes TerminalView
    // expose `window.__wisphiveTerm` so these specs can assert REAL buffer
    // movement (`buffer.active.viewportY`) instead of a mocked scroll call.
    localStorage.setItem('wisphive-term-test-handle', '1')
  }, token)
  await page.goto(`${server.baseURL}/`)
  await expect(page.getByRole('button', { name: '+ Spawn Agent' })).toBeVisible({ timeout: 15_000 })
}

/** Create a terminal via the sidebar toolbar (label via native prompt()). */
async function createTerminal(page: Page, label: string) {
  page.once('dialog', (d) => void d.accept(label))
  await page.getByRole('button', { name: '+ New terminal' }).click()
  const item = page.locator('.terminals-sidebar-item', { hasText: label })
  await expect(item).toBeVisible({ timeout: 10_000 })
  await expect(item.locator('.term-status-running')).toBeVisible({ timeout: 10_000 })
  return item
}

/** Attach to a sidebar session (click a button-free region — README gotcha #4). */
async function attachTerminal(page: Page, label: string) {
  await page.locator('.terminals-sidebar-item', { hasText: label }).locator('.cmd').click()
  const xtermArea = page.locator('.terminals-terminal-slot .xterm')
  await expect(xtermArea).toBeVisible({ timeout: 10_000 })
  return xtermArea
}

/** Rendered visible rows text (DOM renderer) — the user-visible ground truth. */
async function visibleRows(page: Page): Promise<string> {
  return await page.locator('.terminals-terminal-slot .xterm-rows').innerText()
}

async function termState(page: Page): Promise<{ viewportY: number; baseY: number; length: number; rows: number }> {
  return await page.evaluate(() => {
    // eslint-disable-next-line @typescript-eslint/no-explicit-any
    const t = (window as unknown as Record<string, any>).__wisphiveTerm
    if (!t) throw new Error('__wisphiveTerm handle missing — is a terminal mounted?')
    return {
      viewportY: t.buffer.active.viewportY as number,
      baseY: t.buffer.active.baseY as number,
      length: t.buffer.active.length as number,
      rows: t.rows as number,
    }
  })
}

/**
 * Drive a real finger drag through CDP. Small per-move increments matter:
 * the handler's opening frames (below one cell of travel) intentionally pass
 * through unprevented (itr#480 near-tap), and a coarse-step drag would never
 * exercise them.
 */
async function touchDrag(page: Page, direction: 'down' | 'up', travelPx = 240): Promise<void> {
  const box = await page.locator('.terminals-terminal-slot .xterm').boundingBox()
  if (!box) throw new Error('no bounding box for terminal')
  const x = box.x + box.width / 2
  const y0 = direction === 'down' ? box.y + box.height * 0.2 : box.y + box.height * 0.8
  const step = direction === 'down' ? 4 : -4
  const steps = Math.floor(travelPx / 4)
  const client = await page.context().newCDPSession(page)
  await client.send('Input.dispatchTouchEvent', { type: 'touchStart', touchPoints: [{ x, y: y0, id: 1 }] })
  for (let i = 1; i <= steps; i++) {
    await client.send('Input.dispatchTouchEvent', {
      type: 'touchMove',
      touchPoints: [{ x, y: y0 + i * step, id: 1 }],
    })
    await page.waitForTimeout(8)
  }
  await client.send('Input.dispatchTouchEvent', { type: 'touchEnd', touchPoints: [] })
  await client.detach()
  await page.waitForTimeout(300)
}

/** Extract the numeric suffixes of marker lines currently rendered. */
function markerNumbers(text: string, prefix: string): number[] {
  return (text.match(new RegExp(`${prefix}\\d+`, 'g')) ?? []).map((l) => Number(l.slice(prefix.length)))
}

test('touch-drag pages live scrollback: earlier text becomes visible, reverse returns to tail, near-tap still focuses', async ({ page }) => {
  test.setTimeout(120_000)
  await openDashboard(page, 'e2e-touch-scroll')
  await page.getByRole('button', { name: /^Terminals/ }).click()
  await createTerminal(page, 'touch-scroll')
  const xtermArea = await attachTerminal(page, 'touch-scroll')

  // Generate scrollback LIVE while attached (never via switch-away/back —
  // restore-on-reattach is the second spec).
  await page.locator('.terminals-terminal-slot .xterm-helper-textarea').focus()
  await page.keyboard.type('for i in $(seq -w 1 300); do echo LINE_$i; done', { delay: 5 })
  await page.keyboard.press('Enter')
  await expect(xtermArea).toContainText('LINE_300', { timeout: 15_000 })

  const before = await termState(page)
  const beforeText = await visibleRows(page)
  expect(before.baseY, 'live output must have produced scrollback').toBeGreaterThan(0)
  expect(before.viewportY, 'must start at the live tail').toBe(before.baseY)

  // ── Decisive test: drag DOWN pages BACK through real scrollback.
  await touchDrag(page, 'down')
  const after = await termState(page)
  const afterText = await visibleRows(page)
  expect(after.viewportY, 'buffer viewport must page back').toBeLessThan(before.viewportY)
  const beforeMin = Math.min(...markerNumbers(beforeText, 'LINE_'))
  const afterMin = Math.min(...markerNumbers(afterText, 'LINE_'))
  expect(afterMin, 'previously-unseen EARLIER lines must be rendered').toBeLessThan(beforeMin)

  // ── Reverse drag returns to the live tail.
  await touchDrag(page, 'up', 600)
  const back = await termState(page)
  expect(back.viewportY, 'reverse drag must return to the live tail').toBe(back.baseY)

  // ── Near-tap (itr#480): a sub-threshold touch neither scrolls nor steals
  // focus from the terminal (tap-to-focus + on-screen keyboard path).
  await page.locator('body').click({ position: { x: 5, y: 5 } }) // blur the terminal
  const box = await xtermArea.boundingBox()
  if (!box) throw new Error('no bounding box for terminal')
  const client = await page.context().newCDPSession(page)
  const tx = box.x + box.width / 2
  const ty = box.y + box.height / 2
  await client.send('Input.dispatchTouchEvent', { type: 'touchStart', touchPoints: [{ x: tx, y: ty, id: 1 }] })
  await client.send('Input.dispatchTouchEvent', { type: 'touchMove', touchPoints: [{ x: tx, y: ty + 3, id: 1 }] })
  await client.send('Input.dispatchTouchEvent', { type: 'touchEnd', touchPoints: [] })
  await client.detach()
  await page.waitForTimeout(300)
  const afterTap = await termState(page)
  expect(afterTap.viewportY, 'a near-tap must not scroll').toBe(afterTap.baseY)
  await expect(
    page.locator('.terminals-terminal-slot .xterm-helper-textarea'),
    'a near-tap must still focus the terminal',
  ).toBeFocused()
})

test('after a page refresh, scrollback is restored and pages with NO resize — wheel and touch (itr#624)', async ({ page }) => {
  test.setTimeout(120_000)
  await openDashboard(page, 'e2e-touch-refresh')
  await page.getByRole('button', { name: /^Terminals/ }).click()
  await createTerminal(page, 'refresh-scroll')
  let xtermArea = await attachTerminal(page, 'refresh-scroll')

  await page.locator('.terminals-terminal-slot .xterm-helper-textarea').focus()
  await page.keyboard.type('for i in $(seq -w 1 300); do echo LINE_$i; done', { delay: 5 })
  await page.keyboard.press('Enter')
  await expect(xtermArea).toContainText('LINE_300', { timeout: 15_000 })
  // Let the daemon's async event batcher persist the tail the seed reads.
  await page.waitForTimeout(1000)

  // ── Refresh and re-attach. NO resize happens anywhere below: the itr#624
  // regression was "nothing scrolls after refresh until the window is
  // resized" (wheel and touch alike — the buffer held zero scrollback).
  await page.reload()
  await expect(page.getByRole('button', { name: '+ Spawn Agent' })).toBeVisible({ timeout: 15_000 })
  await page.getByRole('button', { name: /^Terminals/ }).click()
  xtermArea = await attachTerminal(page, 'refresh-scroll')
  await expect(xtermArea).toContainText('LINE_300', { timeout: 15_000 })

  const restored = await termState(page)
  expect(
    restored.baseY,
    'attach catchup must seed real scrollback for the authorized creator device (not just the current screen)',
  ).toBeGreaterThan(0)

  // ── Wheel pages back immediately (no resize).
  const beforeWheelText = await visibleRows(page)
  const box = await xtermArea.boundingBox()
  if (!box) throw new Error('no bounding box for terminal')
  await page.mouse.move(box.x + box.width / 2, box.y + box.height / 2)
  await page.mouse.wheel(0, -300)
  await page.waitForTimeout(300)
  const afterWheel = await termState(page)
  expect(afterWheel.viewportY, 'wheel must page back right after refresh').toBeLessThan(restored.viewportY)

  // ── Touch pages back too (no resize), revealing previously-unseen text.
  await touchDrag(page, 'down')
  const afterTouch = await termState(page)
  const afterTouchText = await visibleRows(page)
  expect(afterTouch.viewportY, 'touch must page back right after refresh').toBeLessThan(afterWheel.viewportY)
  const seenBefore = Math.min(...markerNumbers(beforeWheelText, 'LINE_'))
  const seenAfter = Math.min(...markerNumbers(afterTouchText, 'LINE_'))
  expect(seenAfter, 'previously-unseen EARLIER lines must be rendered after refresh + scroll').toBeLessThan(seenBefore)
})

test('a foreign-created session attaches screen-only: the scrollback seed stays behind the replay ACL', async ({ page }) => {
  test.setTimeout(120_000)
  // Create the session as the CLI ("human:tui") — the web device is neither
  // the creator nor in the replay ACL, so the attach catchup must NOT leak
  // history as scrollback (itr#98/#623 boundary; grant path is itr#623).
  const bin = process.env.WISPHIVE_BIN ?? path.join(REPO_ROOT, 'target', 'debug', 'wisphive')
  // NB: `--arg=-c` (equals form) — clap would parse a bare `-c` as a flag.
  await execFileP(
    bin,
    ['term', 'new', '--label', 'cli-owned', '--cmd', '/bin/sh', '--arg=-c', '--arg=seq -w 1 300 | sed s/^/LINE_/; exec sleep 300'],
    { env: { ...process.env, HOME: server.home } },
  )

  await openDashboard(page, 'e2e-acl-attach')
  await page.getByRole('button', { name: /^Terminals/ }).click()
  const xtermArea = await attachTerminal(page, 'cli-owned')
  await expect(xtermArea).toContainText('LINE_300', { timeout: 15_000 })

  const state = await termState(page)
  expect(state.baseY, 'no scrollback may be seeded for a non-creator device').toBe(0)
  // And scrolling reveals nothing: the earliest rendered marker is unchanged.
  const beforeText = await visibleRows(page)
  await touchDrag(page, 'down')
  const afterText = await visibleRows(page)
  expect(Math.min(...markerNumbers(afterText, 'LINE_'))).toBe(Math.min(...markerNumbers(beforeText, 'LINE_')))
})
