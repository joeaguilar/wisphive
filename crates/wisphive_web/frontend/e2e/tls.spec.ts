/**
 * TLS / wss / h2 regression gate.
 *
 * A prior TLS swap (itr#214) exposed two latent bug classes that curl-based
 * checks missed and only a real browser catches:
 *
 *  1. h2 `:authority` handling — axum_server negotiates HTTP/2 via ALPN by
 *     default once TLS is on; h2 drops the `Host:` header entirely and puts
 *     the authority on the `:authority` pseudo-header. A Host-allowlist that
 *     only reads the header 403s every modern browser (see
 *     `SecurityConfig::check_host` in crates/wisphive_web/src/security.rs).
 *  2. ws:// mixed content — an HTTPS-served page constructing `ws://` (not
 *     `wss://`) URLs is blocked by the browser as mixed content (see
 *     `useWisphive.ts`, which derives the scheme from `window.location`).
 *
 * This spec boots the production TLS serve (self-signed rcgen cert,
 * `ignoreHTTPSErrors` in playwright.config.ts) and asserts browser-level
 * behavior:
 *   - SPA document + assets load over https with zero mixed-content/CSP
 *     console errors and zero mixed-content-blocked requests.
 *   - The /ws WebSocket uses wss:// (never ws://).
 *   - /api requests succeed and negotiate h2 (nextHopProtocol === 'h2') —
 *     both unauthenticated and bearer-authenticated, with the Host/authority
 *     the browser naturally sends (the :authority regression guard).
 */
import { test, expect, type Page } from '@playwright/test'
import { startWisphiveServer, type WisphiveServer } from './helpers/server'

const PASSWORD = 'wisphive-e2e-tls-password'

/** Console errors of the mixed-content / CSP class (Chromium phrasings). */
const MIXED_CONTENT_OR_CSP = [
  /mixed content/i,
  /content security policy/i,
  /was loaded over https, but requested an insecure/i,
  /refused to (load|connect|execute|apply|frame)/i,
]

let server: WisphiveServer

test.beforeAll(async () => {
  server = await startWisphiveServer()
})

test.afterAll(async () => {
  if (server) await server.stop()
})

/** Walk first-run onboarding so the SPA holds a token and opens /ws. */
async function completeFirstRunSetup(page: Page): Promise<void> {
  await expect(page.getByText('Welcome. Set a password to finish setup.')).toBeVisible({
    timeout: 15_000,
  })
  await page.getByLabel('New password').fill(PASSWORD)
  await page.getByLabel('Confirm password').fill(PASSWORD)
  await page.getByRole('button', { name: 'Set password' }).click()

  const skipEnroll = page.getByRole('button', { name: 'Skip for now' })
  const queueNav = page.getByRole('button', { name: /^Queue/ })
  await expect(skipEnroll.or(queueNav).first()).toBeVisible({ timeout: 15_000 })
  if (await skipEnroll.isVisible()) {
    await skipEnroll.click()
  }
  await expect(queueNav).toBeVisible()
}

test('TLS serve: https SPA, wss /ws, no mixed-content/CSP errors, h2 /api', async ({ page }) => {
  // Listeners must be attached before navigation so nothing is missed.
  const consoleErrors: string[] = []
  page.on('console', (msg) => {
    if (msg.type() === 'error') consoleErrors.push(msg.text())
  })

  const wsUrls: string[] = []
  page.on('websocket', (ws) => wsUrls.push(ws.url()))

  // Requests the browser refused to send at all (mixed-content blocking
  // surfaces here as well as on the console).
  const blockedRequests: string[] = []
  page.on('requestfailed', (req) => {
    const reason = req.failure()?.errorText ?? ''
    if (/mixed.content|ERR_BLOCKED_BY_CLIENT|ERR_BLOCKED_BY_CSP/i.test(reason)) {
      blockedRequests.push(`${req.url()} (${reason})`)
    }
  })

  await page.goto(`${server.baseURL}/`)
  await completeFirstRunSetup(page)

  await test.step('document and all subresources are https', async () => {
    expect(page.url()).toMatch(/^https:\/\/localhost:\d+\//)
    const resourceUrls = await page.evaluate(() =>
      performance.getEntriesByType('resource').map((e) => e.name),
    )
    const insecure = resourceUrls.filter((u) => u.startsWith('http://') || u.startsWith('ws://'))
    expect(insecure, `insecure subresources fetched: ${insecure.join(', ')}`).toEqual([])
    expect(blockedRequests, `mixed-content/CSP-blocked requests: ${blockedRequests.join(', ')}`)
      .toEqual([])
  })

  await test.step('/ws connects via wss:// (never ws://)', async () => {
    // The SPA opens /ws once it holds a token (post set-password). With no
    // daemon behind the standalone web serve the bridge may drop, but the
    // browser-side URL scheme is what this regression gate is about.
    await expect
      .poll(() => wsUrls.length, {
        message: 'SPA never attempted a WebSocket connection',
        timeout: 15_000,
      })
      .toBeGreaterThan(0)
    for (const url of wsUrls) {
      // Redact the ?token= query when reporting a failure.
      const redacted = url.split('?')[0]
      expect(redacted, `WebSocket used a non-wss scheme: ${redacted}`).toMatch(
        /^wss:\/\/localhost:\d+\/ws$/,
      )
    }
  })

  await test.step('zero mixed-content or CSP console errors', async () => {
    const offenders = consoleErrors.filter((text) =>
      MIXED_CONTENT_OR_CSP.some((re) => re.test(text)),
    )
    expect(offenders, `mixed-content/CSP console errors: ${offenders.join(' | ')}`).toEqual([])
  })

  await test.step('document negotiated h2 over TLS', async () => {
    const navProto = await page.evaluate(() => {
      const nav = performance.getEntriesByType(
        'navigation',
      )[0] as PerformanceNavigationTiming | undefined
      return nav?.nextHopProtocol ?? null
    })
    // axum_server's RustlsConfig defaults ALPN to ["h2", "http/1.1"]; a
    // modern Chromium therefore negotiates h2. If this regresses to
    // http/1.1 the :authority handling in security.rs loses its browser
    // coverage — fail loudly rather than loosen.
    expect(navProto).toBe('h2')
  })

  await test.step('/api succeeds over h2 with the natural browser authority', async () => {
    // Regression guard for the :authority class: fetch() from the page
    // sends exactly the authority the browser naturally uses (h2
    // :authority pseudo-header, no Host header). A Host-allowlist reading
    // only the header would 403 this.
    const unauthenticated = await page.evaluate(async () => {
      const marker = `e2e-tls-${Date.now()}`
      const url = `/api/auth/status?${marker}`
      const res = await fetch(url)
      const abs = new URL(url, window.location.href).href
      let proto: string | null = null
      for (let i = 0; i < 40 && proto === null; i++) {
        const entry = performance
          .getEntriesByType('resource')
          .find((e) => e.name === abs) as PerformanceResourceTiming | undefined
        if (entry) proto = entry.nextHopProtocol
        else await new Promise((r) => setTimeout(r, 50))
      }
      return { status: res.status, proto }
    })
    expect(unauthenticated.status).toBe(200)
    expect(unauthenticated.proto).toBe('h2')

    // Bearer-authenticated path too: the token gate and the host gate are
    // separate layers; both must pass with the natural authority.
    const authenticated = await page.evaluate(async () => {
      const token = localStorage.getItem('wisphive-web-token')
      const res = await fetch('/api/devices', {
        headers: token ? { Authorization: `Bearer ${token}` } : {},
      })
      return { status: res.status, hadToken: token !== null }
    })
    expect(authenticated.hadToken).toBe(true)
    expect(authenticated.status).toBe(200)
  })
})
