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
  terminal_session_id?: string;
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
  hook_event_name?: string;
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

// Terminal sessions
export type TerminalStatus = "running" | "exited" | "killed" | "orphaned";
export type TerminalDirection = "input" | "output" | "resize";

export interface TerminalSessionMeta {
  id: string;
  label?: string;
  command: string;
  args: string[];
  cwd: string;
  cols: number;
  rows: number;
  started_at: string;
  ended_at?: string;
  exit_code?: number;
  status: TerminalStatus;
  group_name?: string;
  sort_order: number;
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
  | { type: "history_response"; entries: HistoryEntry[]; request_id?: string }
  | { type: "sessions_response"; sessions: SessionSummary[] }
  | { type: "projects_response"; projects: ProjectSummary[] }
  | { type: "reimport_complete"; count: number }
  | { type: "error"; message: string }
  | { type: "term_created" } & TerminalSessionMeta
  | { type: "term_list_response"; sessions: TerminalSessionMeta[] }
  | { type: "term_chunk"; id: string; seq: number; ts_us: number; direction: TerminalDirection; data: string }
  | { type: "term_catchup"; id: string; cols: number; rows: number; next_seq: number; screen: string }
  | { type: "term_ended"; id: string; exit_code?: number; status: TerminalStatus }
  | { type: "term_replay_chunk"; id: string; seq: number; ts_us: number; direction: TerminalDirection; data: string }
  | { type: "term_replay_done"; id: string; total_events: number }
  | { type: "term_error"; id?: string; message: string };

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
  | { type: "query_history"; agent_id?: string; limit?: number; request_id?: string }
  | { type: "query_sessions" }
  | { type: "query_projects" }
  | { type: "reimport_events" }
  | { type: "spawn_agent" } & SpawnAgentRequest
  | { type: "search_history"; query?: string; tool_name?: string; agent_id?: string; limit?: number; request_id?: string }
  | { type: "term_create"; label?: string; command?: string; args?: string[]; cwd?: string; cols: number; rows: number; env?: Record<string, string> }
  | { type: "term_attach"; id: string }
  | { type: "term_detach"; id: string }
  | { type: "term_input"; id: string; data: string }
  | { type: "term_resize"; id: string; cols: number; rows: number }
  | { type: "term_close"; id: string; kill?: boolean }
  | { type: "term_list" }
  | { type: "term_replay"; id: string; from_seq?: number; speed?: number }
  | { type: "term_set_group"; id: string; group?: string }
  | { type: "term_reorder"; id: string; sort_order: number };
