import { defineConfig, devices } from '@playwright/test'

// Playwright e2e config for the Wisphive web UI.
//
// There is intentionally NO `webServer` block here: each spec boots its own
// `wisphive web serve` process via `e2e/helpers/server.ts` so every run gets
// a fresh isolated state dir (HOME=<tempdir>) and an ephemeral port. The
// real ~/.wisphive is never read or written.
//
// The production server uses a self-signed TLS cert minted into the isolated
// state dir, hence `ignoreHTTPSErrors`.
export default defineConfig({
  testDir: './e2e',
  outputDir: './test-results',
  // Specs each own a server process; keep them serialized so a laptop
  // doesn't juggle N cargo-built servers at once. Individual specs are
  // still isolated (fresh HOME + port per boot).
  fullyParallel: false,
  workers: 1,
  forbidOnly: !!process.env.CI,
  retries: 0,
  timeout: 60_000,
  reporter: [['list']],
  use: {
    ignoreHTTPSErrors: true,
    trace: 'retain-on-failure',
    screenshot: 'only-on-failure',
  },
  projects: [
    {
      name: 'chromium',
      use: { ...devices['Desktop Chrome'] },
    },
  ],
})
