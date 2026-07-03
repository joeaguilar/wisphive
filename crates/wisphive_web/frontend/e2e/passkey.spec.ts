/**
 * Passkey e2e: enroll a passkey through a Chrome DevTools Protocol (CDP)
 * virtual authenticator, sign out, then sign back in with that passkey —
 * against a real `wisphive web serve` (isolated HOME, self-signed TLS)
 * and the real webauthn-rs ceremony endpoints. No mocks: the browser
 * runs `navigator.credentials.create()` / `.get()` for real; only the
 * authenticator hardware is virtual.
 *
 * ## What this spec proves — and what stays HUMAN-ONLY
 *
 * This gate catches protocol/integration regressions between manual
 * passes: route shapes, base64url plumbing, webauthn-rs verification,
 * challenge-session lifecycle, token minting, the enroll/login UI wiring.
 *
 * It does NOT retire the human smoke matrix. Verification that remains
 * human-only (see the manual-testing tagged itr issues):
 *   - Real Touch ID / platform-authenticator UX on macOS hardware
 *   - iPhone cross-device (hybrid/CaBLE) enrollment and login
 *   - Firefox passkey flows on real hardware
 *   - OS-level biometric prompt behavior, cancellation UX, and keychain
 *     sync semantics (iCloud Keychain, Google Password Manager)
 * A green run here must never be read as "passkeys fully verified".
 *
 * ## Platform-authenticator emulation (the register/start rewrite)
 *
 * webauthn-rs 0.5's `start_passkey_registration` hard-codes
 * `require_resident_key(false)`, which serializes as
 * `authenticatorSelection.residentKey: "discouraged"` — see the
 * "Resident keys" deviation note at the top of
 * crates/wisphive_web/src/passkey.rs. Real platform authenticators
 * (Touch ID / iCloud Keychain, Windows Hello, Google Password Manager)
 * ignore that hint and create discoverable (resident) credentials
 * anyway, which is what makes the server's discoverable-only login
 * route (`start_discoverable_authentication`, empty allowCredentials)
 * work in production. The CDP virtual authenticator, however, honors
 * the hint literally and would mint a NON-resident credential that
 * discoverable login can never find. To exercise the same path real
 * hardware takes, we rewrite `residentKey` to `"required"` in the
 * /register/start response before it reaches the browser. This is a
 * client-side preference field only — the challenge, origin and RP ID
 * checks webauthn-rs performs at finish are untouched, and the server
 * accepts resident credentials under "discouraged" exactly as it does
 * from real Macs.
 */
import { test, expect, type CDPSession, type Page } from '@playwright/test'
import { startWisphiveServer, type WisphiveServer } from './helpers/server'

const PASSWORD = 'wisphive-e2e-password'

let server: WisphiveServer

test.beforeAll(async () => {
  server = await startWisphiveServer()
})

test.afterAll(async () => {
  if (server) await server.stop()
})

/**
 * Attach a CTAP2 virtual authenticator that behaves like a platform
 * authenticator: `internal` transport, resident-key capable, user
 * verification available and always satisfied (the virtual analogue of
 * a successful Touch ID press), presence simulated automatically so the
 * ceremony completes without a human.
 */
async function attachVirtualAuthenticator(
  page: Page,
): Promise<{ cdp: CDPSession; authenticatorId: string }> {
  const cdp = await page.context().newCDPSession(page)
  await cdp.send('WebAuthn.enable')
  const { authenticatorId } = await cdp.send('WebAuthn.addVirtualAuthenticator', {
    options: {
      protocol: 'ctap2',
      transport: 'internal',
      hasResidentKey: true,
      hasUserVerification: true,
      isUserVerified: true,
      automaticPresenceSimulation: true,
    },
  })
  return { cdp, authenticatorId }
}

/** See "Platform-authenticator emulation" in the module docstring. */
async function emulatePlatformAuthenticatorResidentKey(page: Page): Promise<void> {
  await page.route('**/api/auth/passkey/register/start', async (route) => {
    const response = await route.fetch()
    const json = (await response.json()) as {
      publicKey: { authenticatorSelection?: Record<string, unknown> }
    }
    json.publicKey.authenticatorSelection = {
      ...(json.publicKey.authenticatorSelection ?? {}),
      residentKey: 'required',
      requireResidentKey: true,
    }
    await route.fulfill({ response, json })
  })
}

test('enroll passkey via virtual authenticator, sign out, sign back in with passkey', async ({
  page,
}) => {
  const { cdp, authenticatorId } = await attachVirtualAuthenticator(page)
  await emulatePlatformAuthenticatorResidentKey(page)

  // ── First-run onboarding: set the password ─────────────────────────
  await page.goto(`${server.baseURL}/`)
  await expect(page.getByText('Welcome. Set a password to finish setup.')).toBeVisible({
    timeout: 15_000,
  })
  await page.getByLabel('New password').fill(PASSWORD)
  await page.getByLabel('Confirm password').fill(PASSWORD)
  await page.getByRole('button', { name: 'Set password' }).click()

  // ── Enroll step: this is the spec that does NOT skip it ────────────
  // localhost origins get the post-set-password enroll card
  // (phase === "authed-pending-enroll").
  await expect(page.getByText('Set up a passkey on this device?')).toBeVisible({
    timeout: 15_000,
  })
  await page.getByRole('button', { name: 'Enroll passkey' }).click()

  // Ceremony success releases the gate straight into the dashboard.
  const queueNav = page.getByRole('button', { name: /^Queue/ })
  await expect(queueNav).toBeVisible({ timeout: 15_000 })
  await expect(page.getByText('No pending decisions')).toBeVisible()

  // Runtime evidence: the authenticator now holds exactly one resident
  // credential scoped to RP ID "localhost" (the collapsed loopback RP
  // ID from AuthPolicy::rp_id_for_origin).
  const afterEnroll = await cdp.send('WebAuthn.getCredentials', { authenticatorId })
  expect(afterEnroll.credentials).toHaveLength(1)
  expect(afterEnroll.credentials[0].isResidentCredential).toBe(true)
  expect(afterEnroll.credentials[0].rpId).toBe('localhost')
  const signCountAfterEnroll = afterEnroll.credentials[0].signCount

  // The set-password flow minted a bearer; remember it so we can prove
  // passkey login mints a FRESH one (token rotation, new device row).
  const tokenAfterSetup = await page.evaluate(() => localStorage.getItem('wisphive-web-token'))
  expect(tokenAfterSetup).toBeTruthy()

  // ── Sign out ───────────────────────────────────────────────────────
  await page.getByRole('button', { name: 'Sign out' }).click()
  await expect(page.getByText('Sign in to review pending decisions.')).toBeVisible({
    timeout: 15_000,
  })
  expect(await page.evaluate(() => localStorage.getItem('wisphive-web-token'))).toBeNull()

  // ── Sign back in with the passkey ──────────────────────────────────
  const passkeyLogin = page.getByRole('button', { name: 'Sign in with a passkey' })
  await expect(passkeyLogin).toBeVisible({ timeout: 15_000 })
  await passkeyLogin.click()

  // Discoverable-credential login: assertion made by the virtual
  // authenticator, verified by webauthn-rs, fresh device token stashed.
  await expect(queueNav).toBeVisible({ timeout: 15_000 })
  await expect(page.getByText('No pending decisions')).toBeVisible()

  const tokenAfterPasskeyLogin = await page.evaluate(() =>
    localStorage.getItem('wisphive-web-token'),
  )
  expect(tokenAfterPasskeyLogin).toBeTruthy()
  expect(tokenAfterPasskeyLogin).not.toBe(tokenAfterSetup)

  // The assertion incremented the authenticator's signature counter and
  // the server accepted it (no counter-regression 401 — we're logged in).
  const afterLogin = await cdp.send('WebAuthn.getCredentials', { authenticatorId })
  expect(afterLogin.credentials).toHaveLength(1)
  expect(afterLogin.credentials[0].signCount).toBeGreaterThan(signCountAfterEnroll)

  // Screenshot artifact: the logged-in-via-passkey dashboard state.
  const loggedInShot = test.info().outputPath('logged-in-via-passkey.png')
  await page.screenshot({ path: loggedInShot })
  await test
    .info()
    .attach('logged-in-via-passkey', { path: loggedInShot, contentType: 'image/png' })
})
