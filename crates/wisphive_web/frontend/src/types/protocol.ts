// Wisphive protocol types — mirrors wisphive_protocol Rust types

export interface DecisionRequest {
  id: string;
  agent_id: string;
  agent_type: string;
  project: string;
  tool_name: string;
  tool_input: Record<string, unknown> | null;
  timestamp: string;
  hook_event_name: string;
  tool_use_id?: string;
  permission_suggestions?: PermissionSuggestion[];
  event_data?: Record<string, unknown>;
}

export interface PermissionSuggestion {
  behavior: string;
  suggestion_type: string;
  destination: string;
  mode?: string;
  rules: PermissionRule[];
}

export interface PermissionRule {
  tool_name: string;
  rule_content: string;
}

export interface HistoryEntry {
  id: string;
  agent_id: string;
  agent_type: string;
  project: string;
  tool_name: string;
  tool_input: Record<string, unknown>;
  decision: "approve" | "deny" | "ask";
  requested_at: string;
  resolved_at: string;
  tool_result?: Record<string, unknown>;
  tool_use_id?: string;
}

export interface AgentInfo {
  agent_id: string;
  agent_type: string;
  project: string;
  connected_at: string;
  last_seen: string;
}

export interface SessionSummary {
  agent_id: string;
  agent_type: string;
  project: string;
  first_seen: string;
  last_seen: string;
  total_calls: number;
  approved: number;
  denied: number;
  is_live: boolean;
  pending_count: number;
}

// Server → Client messages
export type ServerMessage =
  | { type: "welcome"; version: number }
  | { type: "queue_snapshot"; items: DecisionRequest[] }
  | { type: "new_decision" } & DecisionRequest
  | { type: "decision_resolved"; id: string; decision: string }
  | { type: "agent_connected" } & AgentInfo
  | { type: "agent_disconnected"; agent_id: string }
  | { type: "agents_snapshot"; agents: AgentInfo[] }
  | { type: "history_response"; entries: HistoryEntry[] }
  | { type: "sessions_response"; sessions: SessionSummary[] }
  | { type: "projects_response"; projects: ProjectSummary[] }
  | { type: "reimport_complete"; count: number }
  | { type: "error"; message: string };

export interface ProjectSummary {
  project: string;
  first_seen: string;
  last_seen: string;
  total_calls: number;
  approved: number;
  denied: number;
  agent_count: number;
}

export interface SpawnAgentRequest {
  project: string;
  prompt: string;
  model?: string;
  reasoning?: string;
  max_turns?: number;
}

// Client → Server messages
export type ClientMessage =
  | { type: "approve"; id: string; message?: string; updated_input?: unknown; always_allow?: boolean; additional_context?: string }
  | { type: "deny"; id: string; message?: string }
  | { type: "approve_all"; filter?: unknown }
  | { type: "deny_all"; filter?: unknown }
  | { type: "query_history"; agent_id?: string; limit?: number }
  | { type: "query_sessions" }
  | { type: "query_projects" }
  | { type: "reimport_events" }
  | { type: "spawn_agent" } & SpawnAgentRequest;
