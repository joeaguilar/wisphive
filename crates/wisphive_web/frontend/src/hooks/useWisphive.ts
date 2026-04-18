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
}

/// Callback fired when live PTY output arrives. Consumers wire this into
/// xterm.js to render the session.
export type TerminalOutputHandler = (
  id: string,
  direction: "chunk" | "catchup" | "replay_chunk",
  bytes: Uint8Array,
) => void;

const WS_URL =
  import.meta.env.VITE_WS_URL || `ws://${window.location.host}/ws`;

// Well-known request_id prefixes for routing responses
const CHANNEL_HISTORY = "history";
const CHANNEL_AGENT = "agent";
const CHANNEL_SESSION = "session";

export function useWisphive() {
  const wsRef = useRef<WebSocket | null>(null);
  const reconnectTimer = useRef<ReturnType<typeof setTimeout>>(undefined);
  const terminalHandlersRef = useRef<Map<string, TerminalOutputHandler>>(new Map());
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

          default:
            return prev;
        }
      });
    } catch (e) {
      console.warn("Failed to parse message:", e);
    }
  }, []);

  const connect = useCallback(() => {
    if (wsRef.current?.readyState === WebSocket.OPEN) return;

    const ws = new WebSocket(WS_URL);
    wsRef.current = ws;

    ws.onopen = () => {
      if ("Notification" in window && Notification.permission === "default") {
        Notification.requestPermission();
      }
    };

    ws.onmessage = (event) => {
      handleMessage(event.data);
    };

    ws.onclose = () => {
      setState((prev) => ({ ...prev, connected: false }));
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

  const send = useCallback((msg: ClientMessage) => {
    if (wsRef.current?.readyState === WebSocket.OPEN) {
      wsRef.current.send(JSON.stringify(msg));
    }
  }, []);

  const approve = useCallback(
    (id: string, opts?: { message?: string; updated_input?: unknown; always_allow?: boolean; additional_context?: string }) => {
      send({ type: "approve", id, ...opts });
    },
    [send],
  );

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
