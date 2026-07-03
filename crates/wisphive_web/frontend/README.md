# Wisphive Web Frontend

React 19 + TypeScript + Vite SPA for the Wisphive control plane. In
production the Vite build output (`dist/`) is embedded into the Rust
binary via `rust-embed` and served by `wisphive web serve` (or
`wisphive daemon start --web`) alongside the `/ws` daemon bridge.

## Development

```bash
just frontend-install    # npm install
just frontend-dev        # Vite dev server on :5173 — pair with `just web-dev`
just frontend-build      # Production build → dist/ (embedded via rust-embed)
just frontend-lint       # ESLint
just frontend-test       # Vitest unit tests (jsdom + @testing-library/react)
```

In dev mode (`wisphive web serve --dev`) the Rust process serves only
`/ws` over plain HTTP and Vite serves the UI. In production mode the
Rust process serves everything over TLS with a self-signed cert.

Rendered agent/tool output is untrusted — see the security notes in the
repo-root `CLAUDE.md` (no `dangerouslySetInnerHTML` for agent-controlled
content).

## End-to-end tests (Playwright)

Headless-Chromium e2e tests live in `e2e/`, configured by
`playwright.config.ts`. Run them from the repo root:

```bash
just e2e                       # full pipeline: build + install browser + run
just e2e smoke.spec.ts         # extra args are passed to `playwright test`
```

`just e2e` does, in order:

1. `npm run build` — produce a fresh `dist/`.
2. `cargo build -p wisphive_cli --bin wisphive` — debug binary. In debug
   builds `rust-embed` reads `frontend/dist/` from disk at request time,
   so the freshly built SPA is what gets tested without a release build.
3. `npx playwright install chromium` — idempotent browser install.
4. `npx playwright test` — the suite itself (`npm run test:e2e`).

### How the harness works

There is no `webServer` block in the Playwright config. Each spec boots
its own real server through `e2e/helpers/server.ts`:

- **State isolation:** the Wisphive state dir resolves purely via
  `$HOME`, so the helper spawns `wisphive web serve` with
  `HOME=<fresh mkdtemp dir>`. The real `~/.wisphive` (socket, DB, mode
  file, certs) is never read or written; teardown deletes the temp dir.
  The helper refuses to run if the temp dir resolves inside the real
  home.
- **No hardcoded ports:** an ephemeral port is allocated per boot.
- **Server mode:** production embedded-assets mode (TLS, self-signed
  cert minted into the isolated state dir) — hence `ignoreHTTPSErrors`
  in the Playwright config. `--no-open` suppresses the first-run
  browser auto-open.
- **Readiness:** polls `GET /api/auth/status` (unauthenticated) until
  it answers 200.
- **Base URL:** `https://localhost:<port>` — the server redirects UI
  paths on IP-literal hosts to `localhost`, and passkey affordances are
  origin-sensitive, so specs should always drive `localhost`.

Binary resolution order: `$WISPHIVE_BIN`, then `target/debug/wisphive`,
then `target/release/wisphive`.

Writing new specs: import `startWisphiveServer` from
`./helpers/server` — it takes `extraArgs` (additional CLI flags, e.g.
future TLS options), `env`, and an optional fixed `port`, so new specs
should not need to modify the helper. Vitest only picks up
`src/**/*.{test,spec}.{ts,tsx}` and Playwright only picks up `e2e/`, so
the two suites never collide.

Artifacts (screenshots, traces on failure) land in `test-results/`
(gitignored).
