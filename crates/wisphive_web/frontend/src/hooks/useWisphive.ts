import { useCallback, useEffect, useRef, useState } from "react";
import type {
  AgentInfo,
  AuditDecision,
  ClientMessage,
  ConfigAlertKind,
  DecisionRequest,
  DiskAlertKind,
  HistoryEntry,
  ProjectHookStatus,
  ProjectSummary,
  SessionSummary,
  SpawnAgentRequest,
  TerminalSessionMeta,
} from "../types/protocol";
import { parseServerMessage } from "../types/protocol";
import { apiFetch, clearWebToken, getWebToken, subscribeAuthChange } from "../api";
import { environment } from "../env";

export interface WisphiveState {
  connected: boolean;
  queue: DecisionRequest[];
  agents: AgentInfo[];
  history: HistoryEntry[];
  agentTimeline: HistoryEntry[];
  sessionTimeline: HistoryEntry[];
  sessions: SessionSummary[];
  projects: ProjectSummary[];
  /** Per-project Wisphive hook install state (itr#460), keyed by absolute
   * project path. Lazily populated by `query_project_hook_status` /
   * `install_hooks_result`; drives the Projects view gating badges. */
  hookStatus: Record<string, ProjectHookStatus>;
  /** Per-project install error string from a failed `install_hooks` (itr#460),
   * keyed by project path. Cleared on the next successful install for that
   * project. Server/agent-derived — render as inert text. */
  hookErrors: Record<string, string>;
  auditDecisions: AuditDecision[];
  /** Agent ids the daemon has told us DISCONNECTED (registry reap →
   * `agent_disconnected`) and have not since reconnected. Positive "session is
   * gone" evidence — a deferred prompt from such a session can never be answered,
   * so the inbox drops it (itr#464). Deliberately built only from observed
   * disconnects (not "absent from the live snapshot"), so a session that merely
   * hasn't registered yet is never falsely treated as gone. */
  endedAgentIds: string[];
  terminals: TerminalSessionMeta[];
  /** The newest sudo-class request refused with `web_reauth_required`. The App
   * renders one SudoModal while this is non-null; on successful reauth the
   * hook's `retryPendingApprove` replays every gated request accumulated while
   * that modal was open. Null when no sudo prompt is pending. */
  pendingReauth: PendingReauth | null;
  /** Currently-active resource alerts (audit archive size, low disk), one per
   * `kind`. The daemon raises these instead of deleting audit data (itr#340);
   * a `disk_alert` with `active:false` clears its kind. */
  diskAlerts: DiskAlert[];
  /** Currently-active config trust / policy-widening alerts, one per kind.
   * A `config_alert` with `active:false` clears its kind. */
  configAlerts: ConfigAlert[];
}

export interface DiskAlert {
  kind: DiskAlertKind;
  message: string;
  at: string;
}

export interface ConfigAlert {
  kind: ConfigAlertKind;
  message: string;
  at: string;
}

export interface PendingReauth {
  request_id: string;
  tool_name: string;
}

type ApproveOpts = {
  message?: string;
  updated_input?: unknown;
  always_allow?: boolean;
  additional_context?: string;
};

/// Callback fired when live PTY output arrives. Consumers wire this into
/// xterm.js to render the session.
export type TerminalOutputHandler = (
  id: string,
  direction: "chunk" | "catchup" | "replay_chunk",
  bytes: Uint8Array,
) => void;

export interface TerminalHandlerOptions {
  replayMode?: boolean;
}

interface TerminalHandlerRegistration {
  handler: TerminalOutputHandler;
  replayMode: boolean;
}

// Match the page's protocol so an HTTPS-served page uses wss://. itr#214
// flipped the backend to TLS in production (via axum_server::bind_rustls),
// and browsers refuse mixed-content: a page loaded over https:// cannot
// open a plain ws:// socket without being blocked. Dev mode still serves
// plain HTTP, so http:// pages get ws://. VITE_WS_URL remains the escape
// hatch for split-host dev setups where the Vite page and the WS backend
// are on different origins entirely.
const WS_BASE =
  environment.wsUrl ||
  `${window.location.protocol === "https:" ? "wss:" : "ws:"}//${window.location.host}/ws`;

// Well-known request_id prefixes for routing responses
const CHANNEL_HISTORY = "history";
const CHANNEL_AGENT = "agent";
const CHANNEL_SESSION = "session";

export function useWisphive() {
  const wsRef = useRef<WebSocket | null>(null);
  const authProbeAbortRef = useRef<AbortController | null>(null);
  const wsEverOpenedRef = useRef<boolean>(false);
  const reconnectTimer = useRef<ReturnType<typeof setTimeout>>(undefined);
  const terminalHandlersRef = useRef<Map<string, TerminalHandlerRegistration>>(new Map());
  // Mirrors state.queue for synchronous reads inside plain (non-setState)
  // callbacks — approve() needs the tool_name of the request it's approving
  // at call time, and useState's setter doesn't hand back the current value
  // outside a functional updater. Kept in sync by the effect below.
  const queueRef = useRef<DecisionRequest[]>([]);
  // Per-request approve stash. Keyed by request_id so a successful reauth
  // replays the exact approve the daemon gated — not whatever the user
  // clicked last. The daemon carries `request_id` in every
  // WebReauthRequired (wire.rs) so we can correlate unambiguously even
  // when two sudo-class approves are in flight back-to-back.
  //
  // Each entry also carries the tool_name we believed we were approving
  // (itr#275). Today the daemon only ever emits WebReauthRequired
  // synchronously from a sudo-class gated arm on the same connection, so
  // this can't currently be exploited — but the invariant is fragile: any
  // future code path that emits WebReauthRequired out-of-band (an
  // admin-triggered reauth, session-expiry, a background freshness check)
  // could reference a stale or reused request_id. Without cross-checking
  // the tool, a non-sudo approve (e.g. Read) sitting in the stash could get
  // replayed under cover of an unrelated reauth. retryPendingApprove
  // refuses to replay unless the incoming WebReauthRequired.tool_name
  // matches what we actually stashed for that request_id.
  const approveStashRef = useRef<Map<string, { toolName: string; opts: ApproveOpts | undefined }>>(
    new Map(),
  );
  // Every WebReauthRequired received while the single modal is open. State
  // exposes the newest event for display, while this map retains the full
  // deduplicated batch so one successful reauth can replay all gated requests.
  // Keying by request_id also makes duplicate server frames idempotent.
  const pendingReauthsRef = useRef<Map<string, PendingReauth>>(new Map());
  // Pending install-hooks stash (itr#460). `install_hooks` is sudo-gated, so
  // the daemon may bounce it with `web_reauth_required` (request_id = the
  // project path, tool_name = "InstallHooks"). We stash the project the
  // instant we send the install so a successful reauth replays exactly that
  // install — mirrors approveStashRef for the approve path.
  const installStashRef = useRef<Set<string>>(new Set());
  const [state, setState] = useState<WisphiveState>({
    connected: false,
    queue: [],
    agents: [],
    history: [],
    agentTimeline: [],
    sessionTimeline: [],
    sessions: [],
    projects: [],
    hookStatus: {},
    hookErrors: {},
    auditDecisions: [],
    endedAgentIds: [],
    terminals: [],
    pendingReauth: null,
    diskAlerts: [],
    configAlerts: [],
  });

  // Keep document side effects out of state updaters. React may invoke a
  // functional updater more than once in StrictMode or concurrent renders;
  // this title assignment is idempotent and follows the committed queue.
  useEffect(() => {
    document.title = state.queue.length > 0 ? `(${state.queue.length}) Wisphive` : "Wisphive";
  }, [state.queue.length]);

  // Browsers require Notification permission prompts to originate from a
  // user gesture. Defer the one-time prompt until the first click rather than
  // asking when the WebSocket opens in the background.
  useEffect(() => {
    if (!("Notification" in window) || window.Notification.permission !== "default") {
      return;
    }

    const requestNotificationPermission = () => {
      // The API can disappear with a WebView lifecycle change after this
      // effect registered its click listener. Re-check before every access.
      if (
        "Notification" in window &&
        window.Notification.permission === "default"
      ) {
        void window.Notification.requestPermission();
      }
    };

    window.addEventListener("click", requestNotificationPermission, { once: true });
    return () => window.removeEventListener("click", requestNotificationPermission);
  }, []);

  // Keep queueRef in sync so approve() can read the current queue
  // synchronously (see queueRef declaration above, itr#275).
  useEffect(() => {
    queueRef.current = state.queue;
  }, [state.queue]);

  const handleMessage = useCallback((data: string) => {
    try {
      const msg = parseServerMessage(data);

      if (msg.type === "web_reauth_required") {
        pendingReauthsRef.current.set(msg.request_id, {
          request_id: msg.request_id,
          tool_name: msg.tool_name,
        });
      }

      // Terminal output is an imperative xterm side effect, not a state
      // transition. Keep it outside the functional updater: React may invoke
      // updater functions more than once in StrictMode or concurrent renders.
      // The message has already crossed parseServerMessage's trust boundary,
      // so decoding and dispatching it here preserves the existing validation.
      switch (msg.type) {
        case "term_chunk": {
          const registration = terminalHandlersRef.current.get(msg.id);
          if (registration && !registration.replayMode && msg.direction === "output") {
            registration.handler(msg.id, "chunk", decodeBase64(msg.data));
          }
          return;
        }

        case "term_catchup": {
          const registration = terminalHandlersRef.current.get(msg.id);
          if (registration && !registration.replayMode) {
            registration.handler(msg.id, "catchup", decodeBase64(msg.screen));
          }
          return;
        }

        case "term_replay_chunk": {
          const registration = terminalHandlersRef.current.get(msg.id);
          if (registration?.replayMode && msg.direction === "output") {
            registration.handler(msg.id, "replay_chunk", decodeBase64(msg.data));
          }
          return;
        }
      }

      // Notification delivery belongs to the validated message boundary, not
      // a functional state updater. One server frame therefore constructs one
      // notification even when React probes updater purity in StrictMode.
      if (
        msg.type === "new_decision" &&
        document.hidden &&
        "Notification" in window &&
        window.Notification.permission === "granted"
      ) {
        const req = msg.request;
        new window.Notification(`Wisphive: ${req.tool_name}`, {
          body: `${req.agent_id.slice(0, 20)} needs a decision`,
          tag: "wisphive-decision",
        });
      }

      // Likewise, clear imperative retry bookkeeping before computing the
      // next immutable state snapshot.
      if (msg.type === "decision_resolved") {
        approveStashRef.current.delete(msg.id);
      }

      setState((prev) => {
        switch (msg.type) {
          case "welcome":
            return { ...prev, connected: true };

          case "queue_snapshot":
            return { ...prev, queue: msg.items };

          case "new_decision": {
            const req = msg.request;
            const newQueue = [...prev.queue, req];
            return { ...prev, queue: newQueue };
          }

          case "decision_resolved": {
            // The event boundary above already cleared any matching approve
            // stash; this transition only computes the next queue value.
            const filtered = prev.queue.filter((r) => r.id !== msg.id);
            return { ...prev, queue: filtered };
          }

          case "audit_snapshot":
            return { ...prev, auditDecisions: mergeAuditDecisions([], msg.items) };

          case "audit_decision": {
            return {
              ...prev,
              auditDecisions: mergeAuditDecisions([msg.audit], prev.auditDecisions),
            };
          }

          case "deferred_resolved": {
            // A deferred native prompt was answered in the terminal (itr#461).
            // Mirror the Queue's splice-on-resolve (see "decision_resolved"
            // above): drop the matching deferred row so it leaves "waiting in
            // your terminal" immediately. The answered outcome stays discoverable
            // in History (the decision_log row now carries tool_result). Keyed on
            // tool_use_id, the stable id the daemon correlated the answer against.
            const resolvedDeferrals = prev.auditDecisions.filter(
              (a) => !(a.kind === "deferred" && a.tool_use_id === msg.tool_use_id),
            );
            return { ...prev, auditDecisions: resolvedDeferrals };
          }

          case "agents_snapshot": {
            // Any agent that is live in the snapshot is, by definition, not gone —
            // reconcile it out of the ended set (itr#464).
            const liveIds = new Set(msg.agents.map((a) => a.agent_id));
            return {
              ...prev,
              agents: msg.agents,
              endedAgentIds: prev.endedAgentIds.filter((id) => !liveIds.has(id)),
            };
          }

          case "agent_connected": {
            const info = msg.agent;
            return {
              ...prev,
              agents: [...prev.agents, info],
              // Reconnected → no longer gone.
              endedAgentIds: prev.endedAgentIds.filter((id) => id !== info.agent_id),
            };
          }

          case "agent_disconnected":
            return {
              ...prev,
              agents: prev.agents.filter((a) => a.agent_id !== msg.agent_id),
              // Record positive "session gone" evidence for the inbox (itr#464).
              endedAgentIds: prev.endedAgentIds.includes(msg.agent_id)
                ? prev.endedAgentIds
                : [...prev.endedAgentIds, msg.agent_id],
            };

          case "history_response": {
            const channel = msg.request_id ?? CHANNEL_HISTORY;
            if (channel.startsWith(CHANNEL_AGENT)) {
              return { ...prev, agentTimeline: msg.entries };
            } else if (channel.startsWith(CHANNEL_SESSION)) {
              return { ...prev, sessionTimeline: msg.entries };
            }
            return { ...prev, history: msg.entries };
          }

          case "sessions_response":
            return { ...prev, sessions: msg.sessions };

          case "projects_response":
            return { ...prev, projects: msg.projects };

          case "project_hook_status": {
            const status = msg.status;
            return {
              ...prev,
              hookStatus: { ...prev.hookStatus, [status.project]: status },
            };
          }

          case "install_hooks_result": {
            // Success carries the freshly-audited status; failure carries an
            // error string. Update the badge map on success, surface the
            // error on failure. Either way the install for this project is no
            // longer pending reauth, so drop its stash.
            installStashRef.current.delete(msg.project);
            if (msg.status) {
              const nextErrors = { ...prev.hookErrors };
              delete nextErrors[msg.project];
              return {
                ...prev,
                hookStatus: { ...prev.hookStatus, [msg.project]: msg.status },
                hookErrors: nextErrors,
              };
            }
            return {
              ...prev,
              hookErrors: {
                ...prev.hookErrors,
                [msg.project]: msg.error ?? "Install failed.",
              },
            };
          }

          case "reimport_complete":
            return prev;

          case "error":
            console.error("Daemon error:", msg.message);
            return prev;

          case "term_created": {
            const meta = msg.session;
            return {
              ...prev,
              terminals: [meta, ...prev.terminals.filter((t) => t.id !== meta.id)],
            };
          }

          case "term_list_response":
            return { ...prev, terminals: msg.sessions };

          case "term_ended":
            return {
              ...prev,
              terminals: prev.terminals.map((t) =>
                t.id === msg.id ? { ...t, status: msg.status, exit_code: msg.exit_code } : t,
              ),
            };

          case "term_replay_done":
            return prev;

          case "term_error":
            console.warn("Terminal error:", msg.message);
            return prev;

          case "web_reauth_required":
            // Display the newest gate in the single modal. The complete
            // deduplicated batch lives in pendingReauthsRef and is drained
            // after one successful reauth.
            return {
              ...prev,
              pendingReauth: {
                request_id: msg.request_id,
                tool_name: msg.tool_name,
              },
            };

          case "disk_alert": {
            // Keep at most one active alert per kind. A raise upserts; a clear
            // (active:false) removes it. Never deletes audit data — purely a
            // banner signal (itr#340).
            const others = prev.diskAlerts.filter((a) => a.kind !== msg.kind);
            if (!msg.active) {
              return { ...prev, diskAlerts: others };
            }
            return {
              ...prev,
              diskAlerts: [...others, { kind: msg.kind, message: msg.message, at: msg.at }],
            };
          }

          case "config_alert": {
            // Mirror resource alerts: trust loss and widening state are keyed
            // by kind, with active:false retiring the matching banner.
            const others = prev.configAlerts.filter((a) => a.kind !== msg.kind);
            if (!msg.active) {
              return { ...prev, configAlerts: others };
            }
            return {
              ...prev,
              configAlerts: [...others, { kind: msg.kind, message: msg.message, at: msg.at }],
            };
          }

          default:
            return prev;
        }
      });
    } catch (error) {
      console.warn("Rejected invalid server message:", error);
    }
  }, []);

  const connect = useCallback(async () => {
    if (wsRef.current?.readyState === WebSocket.OPEN) return;

    // Daemon requires a bearer token on /ws. Browsers can't set custom
    // headers on WebSocket constructors, so we pass it as ?token=.
    // If no token is present (logged out, Login not yet submitted), don't
    // dial at all — we'd just trigger a thrashing reconnect loop against
    // an auth-gated endpoint. The auth-change subscription below picks
    // the connection back up once a token appears.
    const token = getWebToken();
    if (!token) {
      setState((prev) => ({ ...prev, connected: false }));
      return;
    }
    const url = `${WS_BASE}${WS_BASE.includes("?") ? "&" : "?"}token=${encodeURIComponent(token)}`;

    // Reset per-connection state before the dial: the 1006 stale-token
    // probe branch below keys off "never opened on *this* socket", so
    // carrying the flag across a logout/login within the same hook
    // instance would skip the probe and leave us in a reconnect loop
    // against a revoked token.
    wsEverOpenedRef.current = false;

    const ws = new WebSocket(url);
    wsRef.current = ws;

    ws.onopen = () => {
      wsEverOpenedRef.current = true;
    };

    ws.onmessage = (event) => {
      handleMessage(event.data);
    };

    ws.onclose = (ev) => {
      setState((prev) => ({ ...prev, connected: false }));
      // 1008 (policy violation) and the 4xxx user-range codes are what
      // the bridge sends when auth fails; 4401 is our "unauthorized"
      // convention. Drop the local token so App re-gates to Login rather
      // than hammering /ws in a reconnect loop.
      if (ev.code === 1008 || ev.code === 4401 || ev.code === 4403) {
        clearWebToken();
        return;
      }
      // A 401 on the upgrade request manifests as code 1006 (abnormal
      // closure) because the upgrade never completes. Can't tell that
      // apart from a transient network blip just from the close event,
      // so when we close before ever receiving a message, probe /api/me
      // — apiFetch's 401 side effect will clear the token on its own if
      // the server rejects us. If /api/me is fine, fall through to the
      // normal reconnect path.
      if (ev.code === 1006 && !wsEverOpenedRef.current && getWebToken()) {
        authProbeAbortRef.current?.abort();
        const controller = new AbortController();
        authProbeAbortRef.current = controller;
        void (async () => {
          try {
            // apiFetch clears the token on 401/403 via its side effect,
            // which fires the auth-change listener and tears us down.
            await apiFetch("/api/me", { signal: controller.signal });
          } catch {
            // Network error; leave it to the normal reconnect timer.
          } finally {
            if (authProbeAbortRef.current === controller) {
              authProbeAbortRef.current = null;
            }
          }
        })();
      }
      reconnectTimer.current = setTimeout(connect, 2000);
    };

  }, [handleMessage]);

  useEffect(() => {
    connect();
    return () => {
      clearTimeout(reconnectTimer.current);
      authProbeAbortRef.current?.abort();
      authProbeAbortRef.current = null;
      if (wsRef.current) {
        wsRef.current.onclose = null;
        wsRef.current.close();
      }
    };
  }, [connect]);

  // Re-dial when a token first appears (post-login) or tear down when it
  // disappears (logout / 401). Cheap to subscribe; fires rarely.
  //
  // Load-bearing: the outer useEffect above re-runs whenever `connect`'s
  // identity changes, which would close+reopen the socket. `connect` is
  // a useCallback of [handleMessage], and handleMessage is a useCallback
  // of []. If a future edit ever makes handleMessage dep on state, this
  // hook will churn the socket on every state update. Keep handleMessage
  // dep-free.
  useEffect(() => {
    return subscribeAuthChange(() => {
      const hasToken = !!getWebToken();
      if (hasToken && wsRef.current?.readyState !== WebSocket.OPEN) {
        clearTimeout(reconnectTimer.current);
        void connect();
      } else if (!hasToken) {
        clearTimeout(reconnectTimer.current);
        authProbeAbortRef.current?.abort();
        authProbeAbortRef.current = null;
        // Null the close handler before tearing down: otherwise the
        // existing onclose will fire async after logout, see a stale
        // wsRef, and schedule a reconnect we don't want.
        if (wsRef.current) {
          wsRef.current.onclose = null;
          wsRef.current.close();
        }
        wsRef.current = null;
      }
    });
  }, [connect]);

  const send = useCallback((msg: ClientMessage) => {
    if (wsRef.current?.readyState === WebSocket.OPEN) {
      wsRef.current.send(JSON.stringify(msg));
    }
  }, []);

  const approve = useCallback(
    (id: string, opts?: ApproveOpts) => {
      // Stash keyed by id so a subsequent web_reauth_required can correlate
      // back to the exact approve by request_id (see wire.rs). Clobbering is
      // impossible — each approve gets its own map entry. Capture the
      // tool_name of the request we're approving right now (itr#275) so
      // retryPendingApprove can refuse to replay if a later reauth event
      // claims to be gating a different tool.
      const toolName = queueRef.current.find((r) => r.id === id)?.tool_name ?? "";
      approveStashRef.current.set(id, { toolName, opts });
      send({ type: "approve", id, ...opts });
    },
    [send],
  );

  /** User cancels the sudo modal (Esc / Cancel). Drop every request accumulated
   * under that modal so a later reauth success cannot replay any approval the
   * user chose to abandon. */
  const dismissReauth = useCallback(() => {
    setState((prev) => {
      const gated = [...pendingReauthsRef.current.values()];
      pendingReauthsRef.current.clear();
      if (gated.length === 0 && prev.pendingReauth) {
        gated.push(prev.pendingReauth);
      }
      for (const reauth of gated) {
        // Only one stash kind applies per request_id, but deleting both is
        // harmless and keeps mixed approve/install batches simple.
        approveStashRef.current.delete(reauth.request_id);
        installStashRef.current.delete(reauth.request_id);
      }
      return { ...prev, pendingReauth: null };
    });
  }, []);

  /** Called by SudoModal on 200 from /api/auth/reauth. Replays each accumulated
   * gated approve (keyed by request_id from the daemon) iff it's still in the
   * queue — requests resolved out-of-band are skipped. Uses functional setState
   * so every queue check observes the current snapshot. */
  const retryPendingApprove = useCallback(() => {
    setState((prev) => {
      const gated = [...pendingReauthsRef.current.values()];
      pendingReauthsRef.current.clear();
      if (gated.length === 0 && prev.pendingReauth) {
        gated.push(prev.pendingReauth);
      }

      for (const reauth of gated) {
        // `install_hooks` (itr#460) is sudo-gated too; its reauth carries
        // request_id = the project path and tool_name = "InstallHooks".
        if (reauth.tool_name === "InstallHooks") {
          const project = reauth.request_id;
          const wasStashed = installStashRef.current.delete(project);
          if (wasStashed) {
            send({ type: "install_hooks", project });
          }
          continue;
        }

        const stashed = approveStashRef.current.get(reauth.request_id);
        approveStashRef.current.delete(reauth.request_id);
        // itr#275: only replay if the stashed tool matches the gate. A
        // mismatch (or no stash) is stale/out-of-band and must not approve.
        if (
          stashed &&
          stashed.toolName === reauth.tool_name &&
          prev.queue.some((r) => r.id === reauth.request_id)
        ) {
          send({ type: "approve", id: reauth.request_id, ...stashed.opts });
        }
      }
      return { ...prev, pendingReauth: null };
    });
  }, [send]);

  const deny = useCallback(
    (id: string, message?: string) => {
      send({ type: "deny", id, message });
    },
    [send],
  );

  const queryHistory = useCallback(
    (agentId?: string) => {
      send({ type: "query_history", agent_id: agentId, limit: 200, request_id: CHANNEL_HISTORY });
    },
    [send],
  );

  const queryAgentTimeline = useCallback(
    (agentId: string) => {
      send({ type: "query_history", agent_id: agentId, limit: 200, request_id: CHANNEL_AGENT });
    },
    [send],
  );

  const querySessionTimeline = useCallback(
    (agentId: string) => {
      send({ type: "query_history", agent_id: agentId, limit: 200, request_id: CHANNEL_SESSION });
    },
    [send],
  );

  const querySessions = useCallback(() => {
    send({ type: "query_sessions" });
  }, [send]);

  const queryProjects = useCallback(() => {
    send({ type: "query_projects" });
  }, [send]);

  /** Install (or repair) Wisphive hooks into a project (itr#460). Sudo-gated
   * server-side: the daemon may bounce with `web_reauth_required`, so we stash
   * the project before sending — a successful reauth replays exactly this
   * install via retryPendingApprove. */
  const installHooks = useCallback(
    (project: string) => {
      installStashRef.current.add(project);
      send({ type: "install_hooks", project });
    },
    [send],
  );

  /** Fetch a project's current hook install state (itr#460); the response
   * lands as `project_hook_status` and merges into `hookStatus`. */
  const queryProjectHookStatus = useCallback(
    (project: string) => {
      send({ type: "query_project_hook_status", project });
    },
    [send],
  );

  const searchHistory = useCallback(
    (query: string, requestId?: string) => {
      send({ type: "search_history", query, limit: 200, request_id: requestId ?? CHANNEL_HISTORY });
    },
    [send],
  );

  const spawnAgent = useCallback(
    (req: SpawnAgentRequest) => {
      send({ type: "spawn_agent", ...req });
    },
    [send],
  );

  // ── Terminal session actions ───────────────────────────────────
  const termList = useCallback(() => {
    send({ type: "term_list" });
  }, [send]);

  const termCreate = useCallback(
    (opts: { label?: string; command?: string; args?: string[]; cwd?: string; cols: number; rows: number }) => {
      send({ type: "term_create", ...opts });
    },
    [send],
  );

  const termAttach = useCallback(
    (id: string) => {
      send({ type: "term_attach", id });
    },
    [send],
  );

  const termDetach = useCallback(
    (id: string) => {
      send({ type: "term_detach", id });
    },
    [send],
  );

  const termInput = useCallback(
    (id: string, data: string) => {
      // Convert JS string to base64, UTF-8 preserving.
      const bytes = new TextEncoder().encode(data);
      send({ type: "term_input", id, data: encodeBase64(bytes) });
    },
    [send],
  );

  const termResize = useCallback(
    (id: string, cols: number, rows: number) => {
      send({ type: "term_resize", id, cols, rows });
    },
    [send],
  );

  const termClose = useCallback(
    (id: string) => {
      send({ type: "term_close", id });
    },
    [send],
  );

  const termReplay = useCallback(
    (id: string, fromSeq?: number) => {
      send({ type: "term_replay", id, from_seq: fromSeq });
    },
    [send],
  );

  const termSetGroup = useCallback(
    (id: string, group?: string) => {
      send({ type: "term_set_group", id, group });
    },
    [send],
  );

  const termReorder = useCallback(
    (id: string, sortOrder: number) => {
      send({ type: "term_reorder", id, sort_order: sortOrder });
    },
    [send],
  );

  const registerTerminalHandler = useCallback(
    (id: string, handler: TerminalOutputHandler, options?: TerminalHandlerOptions) => {
      const registration: TerminalHandlerRegistration = {
        handler,
        replayMode: options?.replayMode ?? false,
      };
      terminalHandlersRef.current.set(id, registration);
      return () => {
        // A same-id live -> replay remount replaces the registration. Ignore a
        // stale live cleanup rather than deleting the newer replay handler.
        if (terminalHandlersRef.current.get(id) === registration) {
          terminalHandlersRef.current.delete(id);
        }
      };
    },
    [],
  );

  return {
    ...state,
    send,
    approve,
    deny,
    dismissReauth,
    retryPendingApprove,
    queryHistory,
    queryAgentTimeline,
    querySessionTimeline,
    querySessions,
    queryProjects,
    installHooks,
    queryProjectHookStatus,
    searchHistory,
    spawnAgent,
    termList,
    termCreate,
    termAttach,
    termDetach,
    termInput,
    termResize,
    termClose,
    termReplay,
    termSetGroup,
    termReorder,
    registerTerminalHandler,
  };
}

// ── base64 helpers ─────────────────────────────────────────────────

function decodeBase64(s: string): Uint8Array {
  const binary = atob(s);
  const bytes = new Uint8Array(binary.length);
  for (let i = 0; i < binary.length; i++) bytes[i] = binary.charCodeAt(i);
  return bytes;
}

function encodeBase64(bytes: Uint8Array): string {
  let binary = "";
  for (const byte of bytes) binary += String.fromCharCode(byte);
  return btoa(binary);
}

function auditKey(audit: AuditDecision): string {
  return [
    audit.kind,
    audit.ts,
    audit.project,
    audit.agent_id,
    audit.terminal_session_id ?? "",
    audit.tool_name,
    audit.decided_by ?? "",
    // tool_use_id distinguishes two genuinely distinct calls that otherwise share
    // every field (e.g. two AskUserQuestions in the same session at the same
    // second). It is also the key deferred_resolved correlates on, so collapsing
    // them here would let one resolution clear the wrong/both rows (itr#461).
    audit.tool_use_id ?? "",
  ].join("\u0000");
}

function mergeAuditDecisions(
  primary: AuditDecision[],
  secondary: AuditDecision[],
): AuditDecision[] {
  const seen = new Set<string>();
  return [...primary, ...secondary]
    .filter((audit) => {
      const key = auditKey(audit);
      if (seen.has(key)) return false;
      seen.add(key);
      return true;
    })
    .sort((a, b) => new Date(b.ts).getTime() - new Date(a.ts).getTime());
}
