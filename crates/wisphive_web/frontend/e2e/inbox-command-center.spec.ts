/**
 * itr#438 — Command Center inbox runtime smoke (spec §10 gating evidence for #399).
 *
 * Exercises the "Waiting on you" inbox end-to-end against a REAL daemon + web
 * server (fixtures/daemon-server.ts, isolated HOME — the live ~/.wisphive is
 * never touched) across TWO distinct projects, proving all five #438 ACs on
 * live, non-fixture data:
 *
 *   AC1 — a daemon-queued decision (project A) appears as an inbox row with a
 *         project/session/agent label + a LIVE-incrementing age; approving it
 *         in-console unblocks the blocked hook (its socket resolution settles
 *         `approve`) AND clears the row.
 *   AC2 — an AskUserQuestion (project B) surfaces as a `deferred` waiting-on-you
 *         row showing the literal question text + options (wired in blitz Wave
 *         2.5) with a go-to-terminal pointer naming project B (hook-only session
 *         → no embedded terminal to focus).
 *   AC3 — an auto-approved action appears in the "decided without you" feed with
 *         its `decided_by` rule visible.
 *   AC4 — with the queue drained and no deferred rows yet, the header reads
 *         exactly `0 waiting · N auto-answered in last hour (view)`, N > 0.
 *   AC5 — items from the two projects are visually distinguishable (per-group
 *         colour rail + group chip).
 *
 * Faithfulness: the deferred (AC2) and auto-approved (AC3) records are authored
 * by the REAL `wisphive-hook` binary running its REAL always-defer / auto-approve
 * classification and writing `events.jsonl`, which the daemon tails → ingests →
 * broadcasts. The only thing not "real" is a full Claude LLM driving the hook.
 * AC1 uses the socket hook-client fixture (a blocking real-wire hook).
 *
 * Oracle (correctness on non-deterministic live data): every observed row is
 * cross-checked against `wisphive audit` — decided_by rules and the auto-answered
 * count must match the daemon's own audit trail.
 *
 * Screenshots for each state are written to
 * sprint/<sprint>/blitz/evidence/ and attached to the Playwright report.
 */
import { test, expect, request as pwRequest, type APIRequestContext, type Page } from '@playwright/test'
import { spawn } from 'node:child_process'
import { chmodSync, existsSync, mkdirSync, writeFileSync } from 'node:fs'
import path from 'node:path'
import { fileURLToPath } from 'node:url'
import { startWisphiveDaemonServer, type WisphiveDaemonServer } from './fixtures/daemon-server'
import { sendDecisionRequest } from './fixtures/hook-client'

const SPEC_DIR = path.dirname(fileURLToPath(import.meta.url))
// e2e → frontend → wisphive_web → crates → repo root
const REPO_ROOT = path.resolve(SPEC_DIR, '..', '..', '..', '..')
const EVIDENCE_DIR = path.join(
  REPO_ROOT,
  'sprint',
  'sprint-2-2026-07-03-command-center-inbox',
  'blitz',
  'evidence',
)

const PASSWORD = 'wisphive-e2e-password'
const PROJECT_A = '/tmp/wisphive-smoke-project-alpha'
const PROJECT_B = '/tmp/wisphive-smoke-project-bravo'

let server: WisphiveDaemonServer
let api: APIRequestContext

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

function wisphiveBinary(): string {
  const fromEnv = process.env.WISPHIVE_BIN
  if (fromEnv && existsSync(fromEnv)) return fromEnv
  for (const c of [
    path.join(REPO_ROOT, 'target', 'debug', 'wisphive'),
    path.join(REPO_ROOT, 'target', 'release', 'wisphive'),
  ]) {
    if (existsSync(c)) return c
  }
  throw new Error('no wisphive binary found — run `cargo build -p wisphive_cli --bin wisphive`')
}

/** Run the REAL wisphive-hook against the isolated HOME, feeding a PreToolUse
 * event on stdin. The hook classifies it (always-defer / auto-approve) and
 * appends the authored record to <home>/.wisphive/events.jsonl, which the
 * running daemon tails and broadcasts. Resolves with {code, stdout}. */
function runHook(event: Record<string, unknown>): Promise<{ code: number | null; stdout: string }> {
  return new Promise((resolve, reject) => {
    const child = spawn(hookBinary(), [], {
      // Force the isolated HOME so the hook reads this daemon's mode/config and
      // writes this daemon's events.jsonl — never the developer's ~/.wisphive.
      env: { ...process.env, HOME: server.home },
      stdio: ['pipe', 'pipe', 'pipe'],
    })
    let stdout = ''
    let stderr = ''
    child.stdout.on('data', (d: Buffer) => (stdout += d.toString()))
    child.stderr.on('data', (d: Buffer) => (stderr += d.toString()))
    child.on('error', reject)
    child.on('close', (code) => {
      if (code !== 0 && code !== 2) {
        // 0 = allow/defer/message-deny; 2 = bare deny. Anything else is a crash.
        reject(new Error(`wisphive-hook exited ${code}. stderr:\n${stderr}\nstdout:\n${stdout}`))
        return
      }
      resolve({ code, stdout })
    })
    child.stdin.write(JSON.stringify(event))
    child.stdin.end()
  })
}

/** `wisphive audit <args>` against the isolated daemon — the correctness oracle. */
function runAudit(args: string[]): Promise<string> {
  return new Promise((resolve, reject) => {
    const child = spawn(wisphiveBinary(), ['audit', ...args], {
      env: { ...process.env, HOME: server.home },
      stdio: ['ignore', 'pipe', 'pipe'],
    })
    let stdout = ''
    let stderr = ''
    child.stdout.on('data', (d: Buffer) => (stdout += d.toString()))
    child.stderr.on('data', (d: Buffer) => (stderr += d.toString()))
    child.on('error', reject)
    child.on('close', (code) => {
      if (code !== 0) {
        reject(new Error(`wisphive audit exited ${code}. stderr:\n${stderr}`))
        return
      }
      resolve(stdout)
    })
  })
}

async function mintToken(deviceName: string): Promise<string> {
  const res = await api.post('/api/auth/login', {
    data: { password: PASSWORD, device_name: deviceName },
  })
  expect(res.ok(), `login failed: ${res.status()} ${await res.text()}`).toBeTruthy()
  return ((await res.json()) as { token: string }).token
}

async function openInbox(page: Page, deviceName: string): Promise<void> {
  const token = await mintToken(deviceName)
  await page.addInitScript((t: string) => localStorage.setItem('wisphive-web-token', t), token)
  await page.goto(`${server.baseURL}/`)
  // Inbox is the default view (#435). Wait for it to render.
  await expect(page.locator('.inbox')).toBeVisible({ timeout: 15_000 })
}

async function shot(page: Page, name: string): Promise<void> {
  mkdirSync(EVIDENCE_DIR, { recursive: true })
  const p = path.join(EVIDENCE_DIR, `${name}.png`)
  await page.screenshot({ path: p, fullPage: true })
  await test.info().attach(name, { path: p, contentType: 'image/png' })
}

test.beforeAll(async () => {
  server = await startWisphiveDaemonServer()
  // Gate must be ON for the hook to classify + author events.jsonl (mode !=
  // active ⇒ passthrough with no record). Auto-approve level=all makes a Read
  // auto-approve (decided_by=level:all); AskUserQuestion still defers because
  // intrinsic always-defer beats every level (ADR-0002).
  const wisphiveDir = path.join(server.home, '.wisphive')
  const modePath = path.join(wisphiveDir, 'mode')
  writeFileSync(modePath, 'active')
  chmodSync(modePath, 0o600)
  writeFileSync(path.join(wisphiveDir, 'config.json'), JSON.stringify({ auto_approve_level: 'all' }))

  api = await pwRequest.newContext({ baseURL: server.baseURL, ignoreHTTPSErrors: true })
  const res = await api.post('/api/auth/set-password', {
    data: { password: PASSWORD, device_name: 'e2e-setup' },
  })
  if (!res.ok()) {
    throw new Error(`set-password bootstrap failed: ${res.status()} ${await res.text()}`)
  }
})

test.afterAll(async () => {
  if (api) await api.dispose()
  if (server) await server.stop()
})

test('inbox surfaces queued, deferred, and auto-answered decisions across two projects', async ({
  page,
}) => {
  // Three real wisphive-hook cold-spawns + events.jsonl ingestion waits + a
  // live-age observation window exceed Playwright's 60s default; give the full
  // multi-AC walkthrough room.
  test.setTimeout(180_000)
  await openInbox(page, 'e2e-inbox-smoke')
  const header = page.locator('.inbox-count')

  // ── AC3 + AC4: auto-answered feed + exact empty-state header ──────────────
  // Author a REAL auto_approved record via the hook (project A, a Read).
  await runHook({
    session_id: 'smoke-auto',
    tool_name: 'Read',
    tool_input: { file_path: '/tmp/wisphive-smoke-alpha/notes.txt' },
    cwd: PROJECT_A,
    hook_event_name: 'PreToolUse',
    permission_mode: 'default',
  })

  // Queue is empty and no deferred rows yet ⇒ the header is the exact #436
  // empty-state string, N = 1.
  await expect(header).toHaveText('0 waiting · 1 auto-answered in last hour (view)', {
    timeout: 15_000,
  })
  await shot(page, 'ac4-empty-state-count')

  // Reveal the feed; the auto-approved row shows its decided_by rule.
  await header.getByRole('button', { name: '(view)' }).click()
  const feed = page.locator('.auto-feed')
  await expect(feed).toBeVisible()
  const autoRow = feed.locator('.auto-feed-item.kind-auto_approved', { hasText: 'Read' })
  await expect(autoRow).toBeVisible({ timeout: 15_000 })
  const feedRule = await autoRow.locator('.auto-feed-rule').textContent()
  expect(feedRule && feedRule.trim().length > 0, 'feed row missing decided_by rule').toBeTruthy()
  await shot(page, 'ac3-auto-answer-feed')

  // ── AC2: deferred AskUserQuestion (project B) with real question text ─────
  const QUESTION = 'Which datastore should the session cache use?'
  await runHook({
    session_id: 'smoke-defer',
    tool_name: 'AskUserQuestion',
    tool_input: {
      questions: [
        {
          question: QUESTION,
          header: 'Cache backend',
          options: [
            { label: 'Redis', description: 'in-memory, needs a sidecar' },
            { label: 'SQLite', description: 'embedded, zero-ops' },
          ],
        },
      ],
    },
    cwd: PROJECT_B,
    hook_event_name: 'PreToolUse',
    permission_mode: 'default',
  })

  const deferredSection = page.locator('.inbox-deferred')
  await expect(deferredSection).toBeVisible({ timeout: 15_000 })
  const deferredRow = deferredSection.locator('.inbox-deferred-item', { hasText: 'AskUserQuestion' })
  await expect(deferredRow).toBeVisible({ timeout: 15_000 })
  await expect(deferredRow.locator('.inbox-deferred-badge')).toHaveText('deferred')
  // The literal question text is on the row (Wave 2.5 wire fix), and the
  // hook-only session gets a go-to-terminal pointer naming project B.
  await expect(deferredRow).toContainText(QUESTION)
  await expect(deferredRow.locator('.inbox-goto-pointer')).toContainText('bravo')
  await shot(page, 'ac2-deferred-row')

  // Expand to the read-only detail: full question + BOTH option labels, no
  // in-console answer control.
  await deferredRow.click()
  const detail = deferredRow.locator('.deferred-detail')
  await expect(detail).toBeVisible()
  await expect(detail.locator('.deferred-question-text')).toContainText(QUESTION)
  await expect(detail.locator('.deferred-option')).toHaveCount(2)
  await expect(detail.locator('.deferred-option').first()).toContainText('Redis')
  await expect(detail.getByRole('button', { name: 'Approve' })).toHaveCount(0)
  await expect(detail.getByRole('button', { name: 'Deny' })).toHaveCount(0)
  await shot(page, 'ac2-deferred-detail')
  await deferredRow.click() // collapse

  // ── AC1 + AC5: a daemon-queued decision (project A) coexists with the ─────
  // deferred (project B) row; approving unblocks the blocked hook + clears it.
  // A non-sudo gated tool (Grep): approving it resolves directly. Sudo-class
  // tools (Bash/Write/Edit/…) would pop a reauth modal instead of resolving —
  // that reauth path is exercised by core-flows, not this inbox smoke.
  const pending = await sendDecisionRequest(server.socketPath, {
    toolName: 'Grep',
    toolInput: { pattern: 'TODO', path: PROJECT_A },
    project: PROJECT_A,
    agentId: 'cc-smoke-alpha',
  })
  try {
    const queuedRow = page.locator('.inbox-item', { hasText: 'Grep' }).first()
    await expect(queuedRow).toBeVisible({ timeout: 15_000 })
    await expect(queuedRow).toContainText('cc-smoke-alpha')
    // Live-incrementing age: capture, wait, capture — it must advance.
    const age1 = await queuedRow.locator('.inbox-age').textContent()
    await page.waitForTimeout(2100)
    const age2 = await queuedRow.locator('.inbox-age').textContent()
    expect(age1, 'age indicator missing').toBeTruthy()
    expect(age2, 'age did not update (static age fails AC1)').not.toBe(age1)

    // AC5: two projects visually distinguishable — the queued row (alpha) and
    // the deferred row (bravo) carry different group colour rails.
    const alphaColor = await queuedRow.evaluate((el) => getComputedStyle(el).borderLeftColor)
    const bravoColor = await deferredRow.evaluate((el) => getComputedStyle(el).borderLeftColor)
    expect(alphaColor, 'project A row has no colour rail').toBeTruthy()
    expect(alphaColor, 'two projects share a colour rail (not distinguishable)').not.toBe(bravoColor)
    await shot(page, 'ac5-two-projects')

    // Full-detail reachability (commit 8f41f1a + the project no-truncation rule):
    // the collapsed row shows only a one-line summary (here just the pattern),
    // but selecting the row reveals the FULL untruncated command via DetailView —
    // every tool_input field + Copy All — so nothing needed to review the command
    // is hidden. The previous inbox truncated the command; this asserts it no
    // longer does.
    await expect(queuedRow.locator('.inbox-summary')).toHaveText('/TODO/')
    // Select via the topline (a button-free region) so the click expands the row
    // rather than landing on the inline Approve/Deny.
    await queuedRow.locator('.inbox-item-topline').click()
    const detail = queuedRow.locator('.inbox-detail-full')
    await expect(detail).toBeVisible()
    // The path is a tool_input field the collapsed summary omitted — it must be
    // fully visible in the expanded detail.
    await expect(detail).toContainText(PROJECT_A)
    // The full-message "Copy All" affordance confirms this is the complete detail.
    await expect(detail.locator('.copy-btn-header')).toBeVisible()
    await shot(page, 'ac1-full-detail')

    // AC1: approve in-console via the expanded DetailView → the blocked hook
    // resolves approve, and the row clears.
    await detail.locator('.detail-actions .btn-approve').first().click()
    const resolution = await pending.resolution
    expect(resolution.decision).toBe('approve')
    await expect(page.locator('.inbox-item', { hasText: 'Grep' })).toHaveCount(0, { timeout: 15_000 })
    await shot(page, 'ac1-after-approve')
  } finally {
    pending.close()
  }

  // ── Oracle: cross-check every observed decision against `wisphive audit` ──
  const audit = await runAudit(['--since', '10m'])
  // Deferred AskUserQuestion attributed to the intrinsic always-defer rule.
  expect(audit).toContain('always_ask:intrinsic')
  expect(audit).toContain('AskUserQuestion')
  // Auto-approved Read carries the feed's decided_by rule.
  expect(audit).toContain('Read')
  expect(audit).toContain((feedRule as string).trim())
  // The human in-console approval of the Grep decision.
  expect(audit).toContain('Grep')
  expect(audit).toContain('human')

  // Project scoping holds: the deferred prompt is attributed to project B.
  const auditB = await runAudit(['--project', PROJECT_B, '--since', '10m'])
  expect(auditB).toContain('AskUserQuestion')
  expect(auditB).toContain('always_ask:intrinsic')

  // The empty-state count N (1 auto-answered) matches the audit trail.
  const auditAuto = await runAudit(['--decided-by', (feedRule as string).trim(), '--since', '1h'])
  const readRows = auditAuto.split('\n').filter((l) => l.includes('Read')).length
  expect(readRows, 'auto-answered count disagrees with audit oracle').toBe(1)
})
