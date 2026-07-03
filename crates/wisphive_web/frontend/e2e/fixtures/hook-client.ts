/**
 * Decision-injection fixture: a Node-side hook client for the wisphive
 * daemon's Unix socket.
 *
 * Mirrors the wire behavior of the real `wisphive-hook` binary
 * (crates/wisphive_hook/src/main.rs) over the newline-delimited JSON
 * protocol (crates/wisphive_protocol/src/wire.rs):
 *
 *   1. connect to `<home>/.wisphive/wisphive.sock`
 *   2. send `{"type":"hello","client":"hook","version":1}`
 *   3. read one line, expect `{"type":"welcome",...}`
 *   4. send `{"type":"decision_request", ...DecisionRequest fields}`
 *   5. BLOCK with the socket open until `{"type":"decision_response",...}`
 *
 * Keeping the socket open while blocked matters: the daemon `select!`s
 * over the hook connection and abandons the decision as a deny the moment
 * the hook socket dies (itr#363). Specs must therefore hold the returned
 * handle until its `resolution` settles, then call `close()`.
 *
 * Field spellings are load-bearing and mirror the Rust serde derives:
 * `agent_type` is snake_case (`"claude_code"`), `hook_event_name` is a
 * PascalCase variant name (`"PreToolUse"`), `timestamp` is RFC 3339, and
 * `decision` in the response is snake_case (`"approve" | "deny" | "ask"`).
 */
import { createConnection, type Socket } from 'node:net'
import { randomUUID } from 'node:crypto'

export const PROTOCOL_VERSION = 1

export interface DecisionRequestOptions {
  toolName: string
  toolInput: unknown
  /** Defaults to a recognizable fixture agent id. */
  agentId?: string
  /** Defaults to `/tmp/wisphive-e2e-project`. */
  project?: string
  /** Defaults to `PreToolUse`. */
  hookEventName?: string
  toolUseId?: string
}

/** The daemon's `decision_response` message, as delivered to the hook. */
export interface DecisionResponse {
  type: 'decision_response'
  id: string
  decision: 'approve' | 'deny' | 'ask'
  message?: string
  updated_input?: unknown
  additional_context?: string
}

export interface PendingDecision {
  /** UUID of the injected DecisionRequest — matches the queue item id. */
  id: string
  /** Resolves when a human (the spec's browser) resolves the decision. */
  resolution: Promise<DecisionResponse>
  /** Destroy the socket. Safe to call repeatedly; call after asserting. */
  close(): void
}

function connectWithRetry(socketPath: string, timeoutMs: number): Promise<Socket> {
  const deadline = Date.now() + timeoutMs
  return new Promise((resolve, reject) => {
    const attempt = () => {
      const sock = createConnection(socketPath)
      sock.once('connect', () => {
        sock.removeAllListeners('error')
        resolve(sock)
      })
      sock.once('error', (err) => {
        sock.destroy()
        if (Date.now() > deadline) {
          reject(new Error(`could not connect to daemon socket ${socketPath}: ${err.message}`))
        } else {
          setTimeout(attempt, 150)
        }
      })
    }
    attempt()
  })
}

/**
 * Connect as a hook, perform the Hello/Welcome handshake, submit a
 * DecisionRequest, and return a handle whose `resolution` settles when
 * the daemon answers (i.e. when the UI approves/denies it).
 */
export async function sendDecisionRequest(
  socketPath: string,
  opts: DecisionRequestOptions,
): Promise<PendingDecision> {
  const sock = await connectWithRetry(socketPath, 15_000)
  sock.setEncoding('utf8')

  const id = randomUUID()

  // Line-buffered reader over the socket: resolves one queued waiter per
  // complete newline-terminated frame, exactly like the Rust side's
  // read_line loop.
  let buffer = ''
  const lineWaiters: Array<(line: string) => void> = []
  const pendingLines: string[] = []
  let failure: Error | null = null
  const failWaiters: Array<(err: Error) => void> = []

  const fail = (err: Error) => {
    if (failure) return
    failure = err
    for (const w of failWaiters.splice(0)) w(err)
  }

  sock.on('data', (chunk: string) => {
    buffer += chunk
    for (;;) {
      const nl = buffer.indexOf('\n')
      if (nl === -1) break
      const line = buffer.slice(0, nl)
      buffer = buffer.slice(nl + 1)
      if (!line.trim()) continue
      const waiter = lineWaiters.shift()
      if (waiter) waiter(line)
      else pendingLines.push(line)
    }
  })
  sock.on('error', (err) => fail(new Error(`hook socket error: ${err.message}`)))
  sock.on('close', () => fail(new Error('hook socket closed before a decision arrived')))

  const nextLine = (): Promise<string> => {
    if (pendingLines.length > 0) return Promise.resolve(pendingLines.shift() as string)
    if (failure) return Promise.reject(failure)
    return new Promise((resolve, reject) => {
      lineWaiters.push(resolve)
      failWaiters.push(reject)
    })
  }

  const send = (msg: unknown) => {
    sock.write(JSON.stringify(msg) + '\n')
  }

  // 1. Hello / Welcome handshake (ClientMessage::Hello → ServerMessage::Welcome).
  send({ type: 'hello', client: 'hook', version: PROTOCOL_VERSION })
  const welcomeLine = await nextLine()
  const welcome = JSON.parse(welcomeLine) as { type?: string }
  if (welcome.type !== 'welcome') {
    sock.destroy()
    throw new Error(`expected welcome from daemon, got: ${welcomeLine}`)
  }

  // 2. DecisionRequest — field-for-field what wisphive-hook sends.
  send({
    type: 'decision_request',
    id,
    agent_id: opts.agentId ?? 'e2e-hook-fixture',
    agent_type: 'claude_code',
    project: opts.project ?? '/tmp/wisphive-e2e-project',
    tool_name: opts.toolName,
    tool_input: opts.toolInput,
    timestamp: new Date().toISOString(),
    hook_event_name: opts.hookEventName ?? 'PreToolUse',
    ...(opts.toolUseId ? { tool_use_id: opts.toolUseId } : {}),
  })

  // 3. Block (socket held open) for the decision_response.
  const resolution: Promise<DecisionResponse> = (async () => {
    for (;;) {
      const line = await nextLine()
      const msg = JSON.parse(line) as DecisionResponse
      if (msg.type === 'decision_response' && msg.id === id) return msg
      // Anything else on a hook connection is unexpected — surface it.
      throw new Error(`unexpected daemon message on hook connection: ${line}`)
    }
  })()
  // The handle's consumer decides when to await; avoid unhandled-rejection
  // noise if the spec fails before awaiting.
  resolution.catch(() => {})

  return {
    id,
    resolution,
    close: () => {
      sock.removeAllListeners('close')
      sock.removeAllListeners('error')
      sock.destroy()
    },
  }
}
