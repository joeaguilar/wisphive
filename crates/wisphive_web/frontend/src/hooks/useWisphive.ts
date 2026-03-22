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
} from "../types/protocol";

export interface WisphiveState {
  connected: boolean;
  queue: DecisionRequest[];
  agents: AgentInfo[];
  history: HistoryEntry[];
  sessions: SessionSummary[];
  projects: ProjectSummary[];
}

const WS_URL =
  import.meta.env.VITE_WS_URL || `ws://${window.location.host}/ws`;

export function useWisphive() {
  const wsRef = useRef<WebSocket | null>(null);
  const reconnectTimer = useRef<ReturnType<typeof setTimeout>>(undefined);
  const [state, setState] = useState<WisphiveState>({
    connected: false,
    queue: [],
    agents: [],
    history: [],
    sessions: [],
    projects: [],
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
            // Tab title badge
            document.title = newQueue.length > 0 ? `(${newQueue.length}) Wisphive` : "Wisphive";
            // Browser notification if tab not focused
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

          case "history_response":
            return { ...prev, history: msg.entries };

          case "sessions_response":
            return { ...prev, sessions: msg.sessions };

          case "projects_response":
            return { ...prev, projects: msg.projects };

          case "reimport_complete":
            return prev; // No-op, the subsequent history query will update

          case "error":
            console.error("Daemon error:", msg.message);
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
      // Request notification permission on first connect
      if ("Notification" in window && Notification.permission === "default") {
        Notification.requestPermission();
      }
    };

    ws.onmessage = (event) => {
      handleMessage(event.data);
    };

    ws.onclose = () => {
      setState((prev) => ({ ...prev, connected: false }));
      // Reconnect after 2s
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
      send({ type: "reimport_events" });
      send({ type: "query_history", agent_id: agentId, limit: 200 });
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
    (query: string) => {
      send({ type: "search_history", query, limit: 200 });
    },
    [send],
  );

  const spawnAgent = useCallback(
    (req: SpawnAgentRequest) => {
      send({ type: "spawn_agent", ...req });
    },
    [send],
  );

  return {
    ...state,
    send,
    approve,
    deny,
    queryHistory,
    querySessions,
    queryProjects,
    searchHistory,
    spawnAgent,
  };
}
