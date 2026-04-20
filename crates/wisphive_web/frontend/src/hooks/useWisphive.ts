import { useCallback, useEffect, useRef, useState } from "react";
import type {
  AgentInfo,
  ClientMessage,
  DecisionRequest,
  HistoryEntry,
  ProjectSummary,
  ServerMessage,
  SessionSummary,
  SpawnAgentRequest,
  TerminalSessionMeta,
} from "../types/protocol";
import { apiFetch, clearWebToken, getWebToken, subscribeAuthChange } from "../api";

export interface WisphiveState {
  connected: boolean;
  queue: DecisionRequest[];
  agents: AgentInfo[];
  history: HistoryEntry[];
  agentTimeline: HistoryEntry[];
  sessionTimeline: HistoryEntry[];
  sessions: SessionSummary[];
  projects: ProjectSummary[];
  terminals: TerminalSessionMeta[];
  /** Set when the daemon refuses a sudo-class approve with
   * `web_reauth_required`. The App renders SudoModal while this is non-null;
   * on successful reauth the hook's `retryPendingApprove` replays the stashed
   * approve. Null when no sudo prompt is pending. */
  pendingReauth: PendingReauth | null;
}

export interface PendingReauth {
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

// Match the page's protocol so an HTTPS-served page uses wss://. itr#214
// flipped the backend to TLS in production (via axum_server::bind_rustls),
// and browsers refuse mixed-content: a page loaded over https:// cannot
// open a plain ws:// socket without being blocked. Dev mode still serves
// plain HTTP, so http:// pages get ws://. VITE_WS_URL remains the escape
// hatch for split-host dev setups where the Vite page and the WS backend
// are on different origins entirely.
const WS_BASE =
  import.meta.env.VITE_WS_URL ||
  `${window.location.protocol === "https:" ? "wss:" : "ws:"}//${window.location.host}/ws`;

// Well-known request_id prefixes for routing responses
const CHANNEL_HISTORY = "history";
const CHANNEL_AGENT = "agent";
const CHANNEL_SESSION = "session";

export function useWisphive() {
  const wsRef = useRef<WebSocket | null>(null);
  const wsEverOpenedRef = useRef<boolean>(false);
  const reconnectTimer = useRef<ReturnType<typeof setTimeout>>(undefined);
  const terminalHandlersRef = useRef<Map<string, TerminalOutputHandler>>(new Map());
  // Last approve attempt, stashed so a successful reauth can replay it with
  // the exact same (id, opts). Single-slot because the daemon's sudo gate is
  // per-click: the browser only ever has one approve in flight at a time
  // from the user's POV, and a fresh click clobbers the previous stash.
  const lastApproveRef = useRef<{ id: string; opts?: ApproveOpts } | null>(null);
  const [state, setState] = useState<WisphiveState>({
    connected: false,
    queue: [],
    agents: [],
    history: [],
    agentTimeline: [],
    sessionTimeline: [],
    sessions: [],
    projects: [],
    terminals: [],
    pendingReauth: null,
  });

  const handleMessage = useCallback((data: string) => {
    try {
      const msg: ServerMessage = JSON.parse(data);

      setState((prev) => {
        switch (msg.type) {
          case "welcome":
            return { ...prev, connected: true };

          case "queue_snapshot":
            return { ...prev, queue: msg.items };

          case "new_decision": {
            const { type: _, ...req } = msg;
            const newQueue = [...prev.queue, req as DecisionRequest];
            document.title = newQueue.length > 0 ? `(${newQueue.length}) Wisphive` : "Wisphive";
            if (document.hidden && Notification.permission === "granted") {
              new Notification(`Wisphive: ${(req as DecisionRequest).tool_name}`, {
                body: `${(req as DecisionRequest).agent_id.slice(0, 20)} needs a decision`,
                tag: "wisphive-decision",
              });
            }
            return { ...prev, queue: newQueue };
          }

          case "decision_resolved": {
            const filtered = prev.queue.filter((r) => r.id !== msg.id);
            document.title = filtered.length > 0 ? `(${filtered.length}) Wisphive` : "Wisphive";
            return { ...prev, queue: filtered };
          }

          case "agents_snapshot":
            return { ...prev, agents: msg.agents };

          case "agent_connected": {
            const { type: _, ...info } = msg;
            return { ...prev, agents: [...prev.agents, info as AgentInfo] };
          }

          case "agent_disconnected":
            return {
              ...prev,
              agents: prev.agents.filter((a) => a.agent_id !== msg.agent_id),
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

          case "reimport_complete":
            return prev;

          case "error":
            console.error("Daemon error:", msg.message);
            return prev;

          case "term_created": {
            const { type: _, ...meta } = msg;
            return {
              ...prev,
              terminals: [meta as TerminalSessionMeta, ...prev.terminals.filter((t) => t.id !== (meta as TerminalSessionMeta).id)],
            };
          }

          case "term_list_response":
            return { ...prev, terminals: msg.sessions };

          case "term_chunk": {
            const handler = terminalHandlersRef.current.get(msg.id);
            if (handler && msg.direction === "output") {
              handler(msg.id, "chunk", decodeBase64(msg.data));
            }
            return prev;
          }

          case "term_catchup": {
            const handler = terminalHandlersRef.current.get(msg.id);
            if (handler) {
              handler(msg.id, "catchup", decodeBase64(msg.screen));
            }
            return prev;
          }

          case "term_ended":
            return {
              ...prev,
              terminals: prev.terminals.map((t) =>
                t.id === msg.id ? { ...t, status: msg.status, exit_code: msg.exit_code } : t,
              ),
            };

          case "term_replay_chunk": {
            const handler = terminalHandlersRef.current.get(msg.id);
            if (handler && msg.direction === "output") {
              handler(msg.id, "replay_chunk", decodeBase64(msg.data));
            }
            return prev;
          }

          case "term_replay_done":
            return prev;

          case "term_error":
            console.warn("Terminal error:", msg.message);
            return prev;

          case "web_reauth_required":
            // Daemon rejected a sudo-class approve from this device. App.tsx
            // reads `pendingReauth` and mounts SudoModal; on success the
            // modal calls retryPendingApprove which replays lastApproveRef.
            // If a reauth is already pending, prefer the newer tool_name —
            // it reflects whatever the user most recently clicked.
            return { ...prev, pendingReauth: { tool_name: msg.tool_name } };

          default:
            return prev;
        }
      });
    } catch (e) {
      console.warn("Failed to parse message:", e);
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
      if ("Notification" in window && Notification.permission === "default") {
        Notification.requestPermission();
      }
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
        void (async () => {
          try {
            // apiFetch clears the token on 401/403 via its side effect,
            // which fires the auth-change listener and tears us down.
            await apiFetch("/api/me");
          } catch {
            // Network error; leave it to the normal reconnect timer.
          }
        })();
      }
      // eslint-disable-next-line react-hooks/immutability
      reconnectTimer.current = setTimeout(connect, 2000);
    };

    ws.onerror = () => {
      ws.close();
    };
  }, [handleMessage]);

  useEffect(() => {
    connect();
    return () => {
      clearTimeout(reconnectTimer.current);
      wsRef.current?.close();
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
      // Stash before sending: if the daemon bounces this with
      // web_reauth_required, retryPendingApprove needs the exact same
      // (id, opts) tuple to replay.
      lastApproveRef.current = { id, opts };
      send({ type: "approve", id, ...opts });
    },
    [send],
  );

  /** User cancels the sudo modal (Esc / Cancel). Clear the pending state
   * and drop the stashed approve so a later reauth success doesn't replay
   * a request the user chose to abandon. */
  const dismissReauth = useCallback(() => {
    lastApproveRef.current = null;
    setState((prev) => ({ ...prev, pendingReauth: null }));
  }, []);

  // Mirror of queue into a ref so retryPendingApprove can check "is the
  // request still pending?" without pulling queue into its useCallback deps
  // (which would churn every connected component that uses the hook on
  // every decision delta).
  const queueRef = useRef<DecisionRequest[]>([]);
  useEffect(() => {
    queueRef.current = state.queue;
  }, [state.queue]);

  /** Called by SudoModal on 200 from /api/auth/reauth. Replays the stashed
   * approve iff its request_id is still in the queue — if the request was
   * resolved out-of-band (e.g. another client approved it) there's nothing
   * to replay and we just close the modal. */
  const retryPendingApprove = useCallback(() => {
    const pending = lastApproveRef.current;
    lastApproveRef.current = null;
    if (pending && queueRef.current.some((r) => r.id === pending.id)) {
      send({ type: "approve", id: pending.id, ...pending.opts });
    }
    setState((prev) => ({ ...prev, pendingReauth: null }));
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
    (id: string, kill = true) => {
      send({ type: "term_close", id, kill });
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

  const registerTerminalHandler = useCallback((id: string, handler: TerminalOutputHandler) => {
    terminalHandlersRef.current.set(id, handler);
    return () => {
      terminalHandlersRef.current.delete(id);
    };
  }, []);

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
  for (let i = 0; i < bytes.length; i++) binary += String.fromCharCode(bytes[i]);
  return btoa(binary);
}
