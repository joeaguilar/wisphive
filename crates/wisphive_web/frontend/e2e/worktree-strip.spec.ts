/**
 * itr#401 — Working-tree strip runtime smoke (spec §5.3 + §10 evidence).
 *
 * Drives the Worktrees view end-to-end against a REAL daemon + web server
 * (fixtures/daemon-server.ts, isolated short temp HOME — the live ~/.wisphive
 * is never touched) across TWO throwaway fixture git repos, proving the #401
 * ACs on live data:
 *
 *   AC1 — after a session dirties two repos, the strip shows both dirty trees
 *         with branch, dirty count, plausible Conventional-Commits-conforming
 *         pre-generated messages, and correct attribution of agent-made
 *         changes (seeded by the REAL wisphive-hook classifying Edit/Write
 *         events into events.jsonl → decision_log; the human-made change
 *         stays "human/unknown").
 *   AC2 — copying the message and committing manually produces a Conventional
 *         Commits-valid commit: the copied text is `git commit`-ed in the
 *         THROWAWAY fixture repo (never this repo) and `git log -1` is
 *         validated against the CC v1.0.0 header grammar. The strip then
 *         REGENERATES on the tree change (the committed repo goes clean).
 *   AC3 — zero write affordances: every interactive element inside the strip
 *         is a copy button (Board-style enumeration).
 *   AC4 — screenshot evidence, including a mobile-width render.
 *
 * Evidence lands in campaign-003 artifacts as q3-401-*.png + q3-401-e2e.txt.
 */
import { test, expect, request as pwRequest, type APIRequestContext, type Page } from '@playwright/test'
import { execFileSync, spawn } from 'node:child_process'
import { appendFileSync, existsSync, chmodSync, mkdirSync, mkdtempSync, realpathSync, rmSync, writeFileSync } from 'node:fs'
import path from 'node:path'
import { fileURLToPath } from 'node:url'
import { startWisphiveDaemonServer, type WisphiveDaemonServer } from './fixtures/daemon-server'

const SPEC_DIR = path.dirname(fileURLToPath(import.meta.url))
// e2e → frontend → wisphive_web → crates → repo root
const REPO_ROOT = path.resolve(SPEC_DIR, '..', '..', '..', '..')
const EVIDENCE_DIR = path.join(
  REPO_ROOT,
  'campaign',
  'campaign-003-2026-07-14-command-center-layer1',
  'artifacts',
)
const TRANSCRIPT = path.join(EVIDENCE_DIR, 'q3-401-e2e.txt')

const PASSWORD = 'wisphive-e2e-password'

// Conventional Commits v1.0.0 header grammar (mirrors commitMessage.ts).
const CC_HEADER_RE =
  /^(feat|fix|docs|style|refactor|perf|test|build|ci|chore)(\([^()\r\n]+\))?!?: \S.*$/

let server: WisphiveDaemonServer
let api: APIRequestContext
let repoAlpha: string
let repoBravo: string

test.use({ permissions: ['clipboard-read', 'clipboard-write'] })

function log(line: string): void {
  mkdirSync(EVIDENCE_DIR, { recursive: true })
  appendFileSync(TRANSCRIPT, `${new Date().toISOString()} ${line}\n`)
}

/** Run git in a THROWAWAY fixture repo (test-side only; the daemon's own
 * probe path is read-only by construction — worktree.rs allowlist). */
function git(dir: string, args: string[]): string {
  const out = execFileSync(
    'git',
    [
      '-C', dir,
      '-c', 'user.email=q3-fixture@test',
      '-c', 'user.name=Q3 Fixture',
      '-c', 'commit.gpgsign=false',
      // Neutralize developer-global hooks (commit-msg linters etc.).
      '-c', 'core.hooksPath=/dev/null',
      ...args,
    ],
    { encoding: 'utf8' },
  )
  return out.trim()
}

function hookBinary(): string {
  const fromEnv = process.env.WISPHIVE_HOOK_BIN
  if (fromEnv && existsSync(fromEnv)) return fromEnv
  for (const c of [
    path.join(REPO_ROOT, 'target', 'debug', 'wisphive-hook'),
    path.join(REPO_ROOT, 'target', 'release', 'wisphive-hook'),
  ]) {
    if (existsSync(c)) return c
  }
  throw new Error('no wisphive-hook binary found — run `cargo build -p wisphive_hook`')
}

/** Run the REAL wisphive-hook against the isolated HOME, feeding a PreToolUse
 * event on stdin; at level=all it auto-approves and appends the audit record
 * (with the file path in tool_input) to events.jsonl → decision_log. */
function runHook(event: Record<string, unknown>): Promise<void> {
  return new Promise((resolve, reject) => {
    const child = spawn(hookBinary(), [], {
      env: { ...process.env, HOME: server.home },
      stdio: ['pipe', 'pipe', 'pipe'],
    })
    let stderr = ''
    child.stderr.on('data', (d: Buffer) => (stderr += d.toString()))
    child.on('error', reject)
    child.on('close', (code) => {
      if (code !== 0 && code !== 2) {
        reject(new Error(`wisphive-hook exited ${code}. stderr:\n${stderr}`))
        return
      }
      resolve()
    })
    child.stdin.write(JSON.stringify(event))
    child.stdin.end()
  })
}

async function openWorktrees(page: Page, deviceName: string): Promise<void> {
  const res = await api.post('/api/auth/login', {
    data: { password: PASSWORD, device_name: deviceName },
  })
  expect(res.ok(), `login failed: ${res.status()} ${await res.text()}`).toBeTruthy()
  const token = ((await res.json()) as { token: string }).token
  await page.addInitScript((t: string) => localStorage.setItem('wisphive-web-token', t), token)
  await page.goto(`${server.baseURL}/`)
  await expect(page.locator('.inbox')).toBeVisible({ timeout: 15_000 })
  await page.getByRole('button', { name: /^Worktrees/ }).click()
  await expect(page.locator('.worktrees')).toBeVisible()
}

async function shot(page: Page, name: string): Promise<void> {
  mkdirSync(EVIDENCE_DIR, { recursive: true })
  const p = path.join(EVIDENCE_DIR, `${name}.png`)
  await page.screenshot({ path: p, fullPage: true })
  await test.info().attach(name, { path: p, contentType: 'image/png' })
  log(`screenshot: ${p}`)
}

test.beforeAll(async () => {
  rmSync(TRANSCRIPT, { force: true })
  server = await startWisphiveDaemonServer()

  // Gate ON + level=all so hook-authored Edit/Write events auto-approve into
  // the audit stream (mode file 0600 inside the fixture's 0700 state dir).
  const wisphiveDir = path.join(server.home, '.wisphive')
  const modePath = path.join(wisphiveDir, 'mode')
  writeFileSync(modePath, 'active')
  chmodSync(modePath, 0o600)
  const configPath = path.join(wisphiveDir, 'config.json')
  writeFileSync(configPath, JSON.stringify({ auto_approve_level: 'all' }))
  chmodSync(configPath, 0o600)

  api = await pwRequest.newContext({ baseURL: server.baseURL, ignoreHTTPSErrors: true })
  const res = await api.post('/api/auth/set-password', {
    data: { password: PASSWORD, device_name: 'e2e-setup' },
  })
  if (!res.ok()) {
    throw new Error(`set-password bootstrap failed: ${res.status()} ${await res.text()}`)
  }

  // ── TWO throwaway fixture repos (never this repo) ─────────────────────────
  // realpathSync so the path string the hook records (cwd/project) is
  // identical to the one the daemon probes and joins change paths against
  // (macOS /tmp is a symlink to /private/tmp).
  repoAlpha = realpathSync(mkdtempSync('/tmp/wh-q3-alpha-'))
  repoBravo = realpathSync(mkdtempSync('/tmp/wh-q3-bravo-'))
  for (const repo of [repoAlpha, repoBravo]) {
    git(repo, ['init', '--initial-branch=main'])
    mkdirSync(path.join(repo, 'src'), { recursive: true })
    writeFileSync(path.join(repo, 'src/lib.rs'), 'pub fn existing() {}\n')
    writeFileSync(path.join(repo, 'README.md'), '# fixture\n')
    git(repo, ['add', '-A'])
    git(repo, ['commit', '-m', 'chore: initial fixture commit'])
  }
  log(`fixture repos: ${repoAlpha} ${repoBravo}`)
})

test.afterAll(async () => {
  if (api) await api.dispose()
  if (server) await server.stop()
  for (const repo of [repoAlpha, repoBravo]) {
    if (repo) rmSync(repo, { recursive: true, force: true })
  }
})

test('working-tree strip: two dirty repos, generated messages, attribution, manual-commit validity, read-only', async ({
  page,
}) => {
  // events.jsonl ingest + strip poll cycles need headroom over the 60s default.
  test.setTimeout(300_000)

  // ── A "real session" dirties both repos ──────────────────────────────────
  // Alpha: an agent-made edit (the file change on disk + the REAL hook
  // classifying the matching Edit event) and a human-made untracked file.
  const alphaEdited = path.join(repoAlpha, 'src/lib.rs')
  writeFileSync(alphaEdited, 'pub fn existing() {}\npub fn added_by_agent() {}\n')
  await runHook({
    session_id: 'q3-alpha-agent',
    tool_name: 'Edit',
    tool_input: { file_path: alphaEdited, old_string: 'x', new_string: 'y' },
    cwd: repoAlpha,
    hook_event_name: 'PreToolUse',
    permission_mode: 'default',
  })
  writeFileSync(path.join(repoAlpha, 'notes.txt'), 'human scratchpad — no hook event\n')

  // Bravo: an agent-written NEW source file (feat shape) via the real hook.
  const bravoNew = path.join(repoBravo, 'src/widget.rs')
  writeFileSync(bravoNew, 'pub struct Widget;\n')
  await runHook({
    session_id: 'q3-bravo-agent',
    tool_name: 'Write',
    tool_input: { file_path: bravoNew, content: 'pub struct Widget;\n' },
    cwd: repoBravo,
    hook_event_name: 'PreToolUse',
    permission_mode: 'default',
  })
  log('seeded: alpha Edit (agent) + notes.txt (human); bravo Write (agent, new file)')

  await openWorktrees(page, 'e2e-worktree-smoke')

  const card = (repo: string) => page.locator(`.worktree-card[aria-label="Working tree ${repo}"]`)

  // ── AC1: both dirty trees render with branch + counts + attribution ──────
  await expect(card(repoAlpha)).toBeVisible({ timeout: 90_000 })
  await expect(card(repoBravo)).toBeVisible({ timeout: 90_000 })
  await expect(card(repoAlpha).locator('.worktree-branch')).toHaveText('main')
  await expect(card(repoBravo).locator('.worktree-branch')).toHaveText('main')

  // Full untruncated project paths are on the surface.
  await expect(card(repoAlpha).locator('.worktree-path')).toHaveText(repoAlpha)
  await expect(card(repoBravo).locator('.worktree-path')).toHaveText(repoBravo)

  // Alpha: the agent-edited file is attributed; the human file is not.
  // (Attribution rides events.jsonl → decision_log ingest; allow a poll lag.)
  const alphaAgentRow = card(repoAlpha)
    .locator('.worktree-change')
    .filter({ hasText: 'src/lib.rs' })
  await expect(alphaAgentRow.locator('.change-attribution')).toHaveText(
    'agent: cc-q3-alpha-agent',
    { timeout: 60_000 },
  )
  const alphaHumanRow = card(repoAlpha)
    .locator('.worktree-change')
    .filter({ hasText: 'notes.txt' })
  await expect(alphaHumanRow.locator('.change-attribution')).toHaveText('human/unknown')

  // Bravo: the agent-written new file is attributed.
  const bravoRow = card(repoBravo)
    .locator('.worktree-change')
    .filter({ hasText: 'src/widget.rs' })
  await expect(bravoRow.locator('.change-attribution')).toHaveText('agent: cc-q3-bravo-agent', {
    timeout: 60_000,
  })

  // Both cards carry a generated message with a CC-conforming header.
  const alphaMessage = (await card(repoAlpha).locator('.worktree-commit-msg').textContent()) ?? ''
  const bravoMessage = (await card(repoBravo).locator('.worktree-commit-msg').textContent()) ?? ''
  for (const [repo, message] of [
    [repoAlpha, alphaMessage],
    [repoBravo, bravoMessage],
  ] as const) {
    const header = message.split('\n')[0]
    expect(header, `header for ${repo}: ${header}`).toMatch(CC_HEADER_RE)
    expect(header.length).toBeLessThanOrEqual(72)
    log(`generated header (${repo}): ${header}`)
  }
  // Bravo's new source file must shape the type to feat.
  expect(bravoMessage.split('\n')[0].startsWith('feat')).toBeTruthy()
  // Attribution is embedded in the copyable body too.
  expect(alphaMessage).toContain('agent cc-q3-alpha-agent')
  expect(alphaMessage).toContain('human/unknown')

  await shot(page, 'q3-401-strip-two-repos')

  // ── AC3: zero write affordances inside the strip ──────────────────────────
  const stripButtons = page.locator('.worktrees button')
  const count = await stripButtons.count()
  expect(count).toBeGreaterThan(0)
  for (let i = 0; i < count; i++) {
    const label = (await stripButtons.nth(i).getAttribute('aria-label')) ?? ''
    expect(label, `button ${i} must be a copy affordance: ${label}`).toMatch(/^Copy/)
  }
  await expect(page.locator('.worktrees input, .worktrees textarea, .worktrees select, .worktrees a, .worktrees form')).toHaveCount(0)
  await expect(page.locator('.worktrees-note')).toContainText('Read-only mirror — you own git')
  log(`AC3: ${count} interactive element(s) in strip, all copy buttons`)

  // ── AC2: copy the message, commit it manually in the THROWAWAY repo ──────
  const copyBtn = card(repoAlpha).getByRole('button', { name: /^Copy/ })
  await copyBtn.click()
  await expect(copyBtn).toHaveText('Copied!')
  const clipboard = await page.evaluate(() => navigator.clipboard.readText())
  expect(clipboard).toBe(alphaMessage)
  await shot(page, 'q3-401-copy-affordance')

  // The human's manual commit, outside wisphive, in the fixture repo.
  git(repoAlpha, ['add', '-A'])
  const commitMsgFile = path.join(repoAlpha, '.git', 'Q3_COMMIT_MSG')
  writeFileSync(commitMsgFile, clipboard)
  git(repoAlpha, ['commit', '-F', commitMsgFile])
  const committedHeader = git(repoAlpha, ['log', '-1', '--format=%s'])
  const committedBody = git(repoAlpha, ['log', '-1', '--format=%b'])
  expect(committedHeader).toMatch(CC_HEADER_RE)
  expect(committedHeader).toBe(clipboard.split('\n')[0])
  expect(committedBody.trim().length).toBeGreaterThan(0)
  log(`AC2: manual commit in ${repoAlpha}`)
  log(`AC2: committed header: ${committedHeader}`)
  log(`AC2: committed body:\n${committedBody}`)
  log(`AC2: header matches Conventional Commits v1.0.0 grammar: ${CC_HEADER_RE.test(committedHeader)}`)

  // ── Regeneration on tree change: alpha goes clean on the next poll ───────
  await expect(card(repoAlpha).locator('.worktree-clean')).toHaveText(
    'clean — nothing to commit',
    { timeout: 60_000 },
  )
  await expect(card(repoAlpha).locator('.worktree-commit-msg')).toHaveCount(0)
  log('regeneration: alpha strip flipped to clean after the manual commit')
  await shot(page, 'q3-401-regenerated-clean')

  // ── AC4: mobile-width render ──────────────────────────────────────────────
  await page.setViewportSize({ width: 390, height: 844 })
  await expect(card(repoBravo)).toBeVisible()
  await shot(page, 'q3-401-mobile')
})
