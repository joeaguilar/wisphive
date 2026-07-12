// Wisphive protocol types — mirrors wisphive_protocol Rust types

export type JsonValue =
  | null
  | boolean
  | number
  | string
  | JsonValue[]
  | { [key: string]: JsonValue };

export type AgentType = "codex" | "claude_code" | "red" | "local_llm";

export type HookEventType =
  | "PreToolUse"
  | "PostToolUse"
  | "PostToolUseFailure"
  | "PermissionRequest"
  | "Elicitation"
  | "ElicitationResult"
  | "InstructionsLoaded"
  | "UserPromptSubmit"
  | "Stop"
  | "SubagentStop"
  | "SubagentStart"
  | "StopFailure"
  | "ConfigChange"
  | "TeammateIdle"
  | "TaskCompleted"
  | "WorktreeCreate"
  | "WorktreeRemove"
  | "PreCompact"
  | "PostCompact"
  | "SessionStart"
  | "SessionEnd"
  | "Notification"
  | "Unknown";

export interface DecisionRequest {
  id: string;
  agent_id: string;
  agent_type: AgentType;
  project: string;
  tool_name: string;
  tool_input: JsonValue;
  timestamp: string;
  hook_event_name: HookEventType;
  tool_use_id?: string;
  permission_suggestions?: PermissionSuggestion[];
  event_data?: JsonValue;
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
  agent_type: AgentType;
  project: string;
  tool_name: string;
  tool_input: JsonValue;
  decision: "approve" | "deny" | "ask";
  requested_at: string;
  resolved_at: string;
  tool_result?: JsonValue;
  tool_use_id?: string;
  hook_event_name?: string;
  terminal_session_id?: string;
  decided_by?: string;
  config_hash?: string;
}

export type AuditDecisionKind = "auto_approved" | "deferred" | "denied";

export interface AuditDecision {
  kind: AuditDecisionKind;
  decided_by?: string;
  project: string;
  agent_id: string;
  terminal_session_id?: string;
  tool_name: string;
  ts: string;
  /** Claude Code tool_use_id of the gated call, when present. For a DEFERRED
   * prompt this is the stable key a later `deferred_resolved` correlates against
   * so the inbox can clear the exact "waiting in your terminal" row (itr#462). */
  tool_use_id?: string;
  /** Set on a DEFERRED row that has since been ANSWERED in the native prompt
   * (daemon stamped its tool_result). True → resolved; a reconnect snapshot uses
   * this so an already-answered deferral is not re-shown as waiting (itr#461).
   * Absent = still waiting or not a deferral. */
  resolved?: boolean;
  /** Redacted tool input for DEFERRED native prompts (AskUserQuestion /
   * ExitPlanMode / Elicitation) so the inbox can render the literal question +
   * options. Present only for kind === "deferred"; absent otherwise. Already
   * secret-redacted upstream (hook redact + itr#89). */
  tool_input?: JsonValue;
}

export interface AgentInfo {
  agent_id: string;
  agent_type: AgentType;
  project: string;
  connected_at: string;
  last_seen: string;
}

export interface ManagedAgent {
  agent_id: string;
  agent_type: AgentType;
  pid: number;
  project: string;
  model?: string;
  name?: string;
  started_at: string;
  reasoning?: string;
  max_turns?: number;
  permission_mode?: string;
}

export interface SessionSummary {
  agent_id: string;
  agent_type: AgentType;
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
  /** Audit-trail identity of the creating client (itr#98); absent on legacy sessions. */
  created_by?: string;
  /** Resolver labels explicitly allowed to replay this session. */
  replay_acl?: string[];
}

// Server → Client messages
export type ServerMessage =
  | { type: "welcome"; version: number }
  | {
      type: "decision_response";
      id: string;
      decision: "approve" | "deny" | "ask";
      message?: string;
      updated_input?: JsonValue;
      additional_context?: string;
      selected_permission?: PermissionSuggestion;
    }
  | { type: "queue_snapshot"; items: DecisionRequest[] }
  | { type: "new_decision"; request: DecisionRequest }
  | { type: "decision_resolved"; id: string; decision: "approve" | "deny" | "ask" }
  | { type: "audit_snapshot"; items: AuditDecision[] }
  | { type: "audit_decision"; audit: AuditDecision }
  | {
      type: "deferred_resolved";
      tool_use_id: string;
      agent_id: string;
      tool_name: string;
      ts: string;
      answer_summary?: string;
    }
  | { type: "agent_connected"; agent: AgentInfo }
  | { type: "agent_disconnected"; agent_id: string }
  | { type: "agent_spawned"; agent: ManagedAgent }
  | { type: "agent_exited"; agent_id: string; exit_code?: number }
  | { type: "agent_list"; agents: ManagedAgent[] }
  | { type: "agents_snapshot"; agents: AgentInfo[] }
  | { type: "history_response"; entries: HistoryEntry[]; request_id?: string }
  | { type: "sessions_response"; sessions: SessionSummary[] }
  | { type: "projects_response"; projects: ProjectSummary[] }
  | { type: "project_hook_status"; status: ProjectHookStatus }
  | { type: "install_hooks_result"; project: string; status?: ProjectHookStatus; error?: string }
  | { type: "reimport_complete"; count: number }
  | { type: "error"; message: string }
  | { type: "web_login_failure"; ip: string; attempt_count: number; at: string }
  | { type: "web_login_success"; device_id: string; device_name: string; ip: string; at: string }
  | { type: "web_device_registered"; device_id: string; device_name: string; at: string }
  | { type: "web_device_revoked"; device_id: string; at: string }
  | { type: "term_created"; session: TerminalSessionMeta }
  | { type: "term_list_response"; sessions: TerminalSessionMeta[] }
  | { type: "term_chunk"; id: string; seq: number; ts_us: number; direction: TerminalDirection; data: string }
  | { type: "term_catchup"; id: string; cols: number; rows: number; next_seq: number; screen: string }
  | { type: "term_ended"; id: string; exit_code?: number; status: TerminalStatus }
  | { type: "term_replay_chunk"; id: string; seq: number; ts_us: number; direction: TerminalDirection; data: string }
  | { type: "term_replay_done"; id: string; total_events: number }
  | { type: "term_error"; id?: string; message: string }
  | { type: "web_reauth_required"; device_id: string; request_id: string; tool_name: string; at: string }
  | { type: "mark_device_fresh_ack"; device_id: string }
  | { type: "disk_alert"; kind: DiskAlertKind; active: boolean; message: string; at: string }
  | { type: "config_alert"; kind: ConfigAlertKind; active: boolean; message: string; at: string };

/** Which resource condition a `disk_alert` describes. Wisphive never deletes
 * audit data; these are non-destructive warnings (itr#340). */
export type DiskAlertKind = "archive_size" | "low_disk_space";

/** A configuration trust or effective-policy alert from the daemon. */
export type ConfigAlertKind = "policy_widened" | "untrusted_config";

export interface ProjectSummary {
  project: string;
  first_seen: string;
  last_seen: string;
  total_calls: number;
  approved: number;
  denied: number;
  agent_count: number;
  /** Present on current daemons; optional here for persisted/test fixtures. */
  pending_count?: number;
  /** Present on current daemons; optional here for persisted/test fixtures. */
  has_live_agents?: boolean;
}

/** Wire mirror of `wisphive_protocol::ProjectHookStatus` (itr#460) — a
 * project's Wisphive hook install state. `mode` is a label string:
 * "active" | "off" | "missing" | "invalid: <x>". */
export interface ProjectHookStatus {
  project: string;
  mode: string;
  claude_installed: boolean;
  codex_installed: boolean;
  missing_events: string[];
  all_installed: boolean;
  all_enabled: boolean;
}

type UnknownRecord = Record<string, unknown>;
type ValueParser<T> = (value: unknown, path: string) => T;

/**
 * Parse and validate one daemon WebSocket frame. The Rust protocol serializes
 * tuple variants by flattening their payload into the tagged object; this
 * boundary adapter validates that wire shape and returns wrapped payloads so
 * `ServerMessage` remains a true discriminated union in application code.
 */
export function parseServerMessage(data: string): ServerMessage {
  let decoded: unknown;
  try {
    decoded = JSON.parse(data);
  } catch (error) {
    throw new Error("server message is not valid JSON", { cause: error });
  }

  const message = readObject(decoded, "message");
  const type = readField(message, "type", readString, "message");

  switch (type) {
    case "welcome":
      return {
        type,
        version: readField(message, "version", readU32, "message"),
      };
    case "decision_response":
      return {
        type,
        id: readField(message, "id", readUuid, "message"),
        decision: readField(message, "decision", readDecision, "message"),
        message: readOptionalField(message, "message", readString, "message"),
        updated_input: readOptionalJsonField(message, "updated_input", "message"),
        additional_context: readOptionalField(
          message,
          "additional_context",
          readString,
          "message",
        ),
        selected_permission: readOptionalField(
          message,
          "selected_permission",
          parsePermissionSuggestion,
          "message",
        ),
      };
    case "queue_snapshot":
      return {
        type,
        items: readField(message, "items", arrayOf(parseDecisionRequest), "message"),
      };
    case "new_decision":
      return { type, request: parseDecisionRequest(message, "message") };
    case "decision_resolved":
      return {
        type,
        id: readField(message, "id", readUuid, "message"),
        decision: readField(message, "decision", readDecision, "message"),
      };
    case "audit_snapshot":
      return {
        type,
        items: readField(message, "items", arrayOf(parseAuditDecision), "message"),
      };
    case "audit_decision":
      return { type, audit: parseAuditDecision(message, "message") };
    case "deferred_resolved":
      return {
        type,
        tool_use_id: readField(message, "tool_use_id", readString, "message"),
        agent_id: readField(message, "agent_id", readString, "message"),
        tool_name: readField(message, "tool_name", readString, "message"),
        ts: readField(message, "ts", readRfc3339, "message"),
        answer_summary: readOptionalField(message, "answer_summary", readString, "message"),
      };
    case "agent_connected":
      return { type, agent: parseAgentInfo(message, "message") };
    case "agent_disconnected":
      return {
        type,
        agent_id: readField(message, "agent_id", readString, "message"),
      };
    case "agent_spawned":
      return { type, agent: parseManagedAgent(message, "message") };
    case "agent_exited":
      return {
        type,
        agent_id: readField(message, "agent_id", readString, "message"),
        exit_code: readOptionalField(message, "exit_code", readI32, "message"),
      };
    case "agent_list":
      return {
        type,
        agents: readField(message, "agents", arrayOf(parseManagedAgent), "message"),
      };
    case "agents_snapshot":
      return {
        type,
        agents: readField(message, "agents", arrayOf(parseAgentInfo), "message"),
      };
    case "history_response":
      return {
        type,
        entries: readField(message, "entries", arrayOf(parseHistoryEntry), "message"),
        request_id: readOptionalField(message, "request_id", readString, "message"),
      };
    case "sessions_response":
      return {
        type,
        sessions: readField(message, "sessions", arrayOf(parseSessionSummary), "message"),
      };
    case "projects_response":
      return {
        type,
        projects: readField(message, "projects", arrayOf(parseProjectSummary), "message"),
      };
    case "project_hook_status":
      return { type, status: parseProjectHookStatus(message, "message") };
    case "install_hooks_result":
      return {
        type,
        project: readField(message, "project", readString, "message"),
        status: readOptionalField(
          message,
          "status",
          parseProjectHookStatus,
          "message",
        ),
        error: readOptionalField(message, "error", readString, "message"),
      };
    case "reimport_complete":
      return {
        type,
        count: readField(message, "count", readU64Safe, "message"),
      };
    case "error":
      return {
        type,
        message: readField(message, "message", readString, "message"),
      };
    case "web_login_failure":
      return {
        type,
        ip: readField(message, "ip", readString, "message"),
        attempt_count: readField(
          message,
          "attempt_count",
          readU32,
          "message",
        ),
        at: readField(message, "at", readRfc3339, "message"),
      };
    case "web_login_success":
      return {
        type,
        device_id: readField(message, "device_id", readString, "message"),
        device_name: readField(message, "device_name", readString, "message"),
        ip: readField(message, "ip", readString, "message"),
        at: readField(message, "at", readRfc3339, "message"),
      };
    case "web_device_registered":
      return {
        type,
        device_id: readField(message, "device_id", readString, "message"),
        device_name: readField(message, "device_name", readString, "message"),
        at: readField(message, "at", readRfc3339, "message"),
      };
    case "web_device_revoked":
      return {
        type,
        device_id: readField(message, "device_id", readString, "message"),
        at: readField(message, "at", readRfc3339, "message"),
      };
    case "term_created":
      return { type, session: parseTerminalSessionMeta(message, "message") };
    case "term_list_response":
      return {
        type,
        sessions: readField(
          message,
          "sessions",
          arrayOf(parseTerminalSessionMeta),
          "message",
        ),
      };
    case "term_chunk":
      return {
        type,
        id: readField(message, "id", readUuid, "message"),
        seq: readField(message, "seq", readU64Safe, "message"),
        ts_us: readField(message, "ts_us", readI64Safe, "message"),
        direction: readField(message, "direction", readTerminalDirection, "message"),
        data: readField(message, "data", readString, "message"),
      };
    case "term_catchup":
      return {
        type,
        id: readField(message, "id", readUuid, "message"),
        cols: readField(message, "cols", readU16, "message"),
        rows: readField(message, "rows", readU16, "message"),
        next_seq: readField(message, "next_seq", readU64Safe, "message"),
        screen: readField(message, "screen", readString, "message"),
      };
    case "term_ended":
      return {
        type,
        id: readField(message, "id", readUuid, "message"),
        exit_code: readOptionalField(message, "exit_code", readI32, "message"),
        status: readField(message, "status", readTerminalStatus, "message"),
      };
    case "term_replay_chunk":
      return {
        type,
        id: readField(message, "id", readUuid, "message"),
        seq: readField(message, "seq", readU64Safe, "message"),
        ts_us: readField(message, "ts_us", readI64Safe, "message"),
        direction: readField(message, "direction", readTerminalDirection, "message"),
        data: readField(message, "data", readString, "message"),
      };
    case "term_replay_done":
      return {
        type,
        id: readField(message, "id", readUuid, "message"),
        total_events: readField(message, "total_events", readU64Safe, "message"),
      };
    case "term_error":
      return {
        type,
        id: readOptionalField(message, "id", readUuid, "message"),
        message: readField(message, "message", readString, "message"),
      };
    case "web_reauth_required":
      return {
        type,
        device_id: readField(message, "device_id", readString, "message"),
        request_id: readField(message, "request_id", readString, "message"),
        tool_name: readField(message, "tool_name", readString, "message"),
        at: readField(message, "at", readRfc3339, "message"),
      };
    case "mark_device_fresh_ack":
      return {
        type,
        device_id: readField(message, "device_id", readString, "message"),
      };
    case "disk_alert":
      return {
        type,
        kind: readField(message, "kind", readDiskAlertKind, "message"),
        active: readField(message, "active", readBoolean, "message"),
        message: readField(message, "message", readString, "message"),
        at: readField(message, "at", readRfc3339, "message"),
      };
    case "config_alert":
      return {
        type,
        kind: readField(message, "kind", readConfigAlertKind, "message"),
        active: readField(message, "active", readBoolean, "message"),
        message: readField(message, "message", readString, "message"),
        at: readField(message, "at", readRfc3339, "message"),
      };
    default:
      return invalid("message.type", `known server message type, received ${String(type)}`);
  }
}

function parseDecisionRequest(value: unknown, path: string): DecisionRequest {
  const request = readObject(value, path);
  return {
    id: readField(request, "id", readUuid, path),
    agent_id: readField(request, "agent_id", readString, path),
    agent_type: readField(request, "agent_type", readAgentType, path),
    project: readField(request, "project", readString, path),
    tool_name: readField(request, "tool_name", readString, path),
    tool_input: readField(request, "tool_input", readJsonValue, path),
    timestamp: readField(request, "timestamp", readRfc3339, path),
    hook_event_name: readField(request, "hook_event_name", readHookEventType, path),
    tool_use_id: readOptionalField(request, "tool_use_id", readString, path),
    permission_suggestions: readOptionalField(
      request,
      "permission_suggestions",
      arrayOf(parsePermissionSuggestion),
      path,
    ),
    event_data: readOptionalJsonField(request, "event_data", path),
    terminal_session_id: readOptionalField(
      request,
      "terminal_session_id",
      readUuid,
      path,
    ),
  };
}

function parsePermissionSuggestion(value: unknown, path: string): PermissionSuggestion {
  const suggestion = readObject(value, path);
  return {
    behavior: readField(suggestion, "behavior", readString, path),
    suggestion_type: readField(suggestion, "type", readString, path),
    destination: readField(suggestion, "destination", readString, path),
    mode: readOptionalField(suggestion, "mode", readString, path),
    rules:
      readOptionalField(suggestion, "rules", arrayOf(parsePermissionRule), path) ?? [],
  };
}

function parsePermissionRule(value: unknown, path: string): PermissionRule {
  const rule = readObject(value, path);
  return {
    tool_name: readField(rule, "toolName", readString, path),
    rule_content: readField(rule, "ruleContent", readString, path),
  };
}

function parseHistoryEntry(value: unknown, path: string): HistoryEntry {
  const entry = readObject(value, path);
  return {
    id: readField(entry, "id", readUuid, path),
    agent_id: readField(entry, "agent_id", readString, path),
    agent_type: readField(entry, "agent_type", readAgentType, path),
    project: readField(entry, "project", readString, path),
    tool_name: readField(entry, "tool_name", readString, path),
    tool_input: readField(entry, "tool_input", readJsonValue, path),
    decision: readField(entry, "decision", readDecision, path),
    requested_at: readField(entry, "requested_at", readRfc3339, path),
    resolved_at: readField(entry, "resolved_at", readRfc3339, path),
    tool_result: readOptionalJsonField(entry, "tool_result", path),
    tool_use_id: readOptionalField(entry, "tool_use_id", readString, path),
    hook_event_name: readOptionalField(entry, "hook_event_name", readString, path),
    terminal_session_id: readOptionalField(
      entry,
      "terminal_session_id",
      readUuid,
      path,
    ),
    decided_by: readOptionalField(entry, "decided_by", readString, path),
    config_hash: readOptionalField(entry, "config_hash", readString, path),
  };
}

function parseAuditDecision(value: unknown, path: string): AuditDecision {
  const audit = readObject(value, path);
  return {
    kind: readField(audit, "kind", readAuditDecisionKind, path),
    decided_by: readOptionalField(audit, "decided_by", readString, path),
    project: readField(audit, "project", readString, path),
    agent_id: readField(audit, "agent_id", readString, path),
    terminal_session_id: readOptionalField(
      audit,
      "terminal_session_id",
      readUuid,
      path,
    ),
    tool_name: readField(audit, "tool_name", readString, path),
    ts: readField(audit, "ts", readRfc3339, path),
    tool_use_id: readOptionalField(audit, "tool_use_id", readString, path),
    resolved: readOptionalField(audit, "resolved", readBoolean, path),
    tool_input: readOptionalJsonField(audit, "tool_input", path),
  };
}

function parseAgentInfo(value: unknown, path: string): AgentInfo {
  const agent = readObject(value, path);
  return {
    agent_id: readField(agent, "agent_id", readString, path),
    agent_type: readField(agent, "agent_type", readAgentType, path),
    project: readField(agent, "project", readString, path),
    connected_at: readField(agent, "connected_at", readRfc3339, path),
    last_seen: readField(agent, "last_seen", readRfc3339, path),
  };
}

function parseManagedAgent(value: unknown, path: string): ManagedAgent {
  const agent = readObject(value, path);
  return {
    agent_id: readField(agent, "agent_id", readString, path),
    agent_type: readField(agent, "agent_type", readAgentType, path),
    pid: readField(agent, "pid", readU32, path),
    project: readField(agent, "project", readString, path),
    model: readOptionalField(agent, "model", readString, path),
    name: readOptionalField(agent, "name", readString, path),
    started_at: readField(agent, "started_at", readRfc3339, path),
    reasoning: readOptionalField(agent, "reasoning", readString, path),
    max_turns: readOptionalField(agent, "max_turns", readU32, path),
    permission_mode: readOptionalField(agent, "permission_mode", readString, path),
  };
}

function parseSessionSummary(value: unknown, path: string): SessionSummary {
  const session = readObject(value, path);
  return {
    agent_id: readField(session, "agent_id", readString, path),
    agent_type: readField(session, "agent_type", readAgentType, path),
    project: readField(session, "project", readString, path),
    first_seen: readField(session, "first_seen", readRfc3339, path),
    last_seen: readField(session, "last_seen", readRfc3339, path),
    total_calls: readField(session, "total_calls", readU32, path),
    approved: readField(session, "approved", readU32, path),
    denied: readField(session, "denied", readU32, path),
    is_live: readField(session, "is_live", readBoolean, path),
    pending_count: readField(session, "pending_count", readU32, path),
  };
}

function parseProjectSummary(value: unknown, path: string): ProjectSummary {
  const project = readObject(value, path);
  return {
    project: readField(project, "project", readString, path),
    first_seen: readField(project, "first_seen", readRfc3339, path),
    last_seen: readField(project, "last_seen", readRfc3339, path),
    total_calls: readField(project, "total_calls", readU32, path),
    approved: readField(project, "approved", readU32, path),
    denied: readField(project, "denied", readU32, path),
    agent_count: readField(project, "agent_count", readU32, path),
    pending_count: readField(project, "pending_count", readU32, path),
    has_live_agents: readField(project, "has_live_agents", readBoolean, path),
  };
}

function parseProjectHookStatus(value: unknown, path: string): ProjectHookStatus {
  const status = readObject(value, path);
  return {
    project: readField(status, "project", readString, path),
    mode: readField(status, "mode", readString, path),
    claude_installed: readField(status, "claude_installed", readBoolean, path),
    codex_installed: readField(status, "codex_installed", readBoolean, path),
    missing_events: readField(status, "missing_events", arrayOf(readString), path),
    all_installed: readField(status, "all_installed", readBoolean, path),
    all_enabled: readField(status, "all_enabled", readBoolean, path),
  };
}

function parseTerminalSessionMeta(value: unknown, path: string): TerminalSessionMeta {
  const session = readObject(value, path);
  return {
    id: readField(session, "id", readUuid, path),
    label: readOptionalField(session, "label", readString, path),
    command: readField(session, "command", readString, path),
    args: readOptionalField(session, "args", arrayOf(readString), path) ?? [],
    cwd: readField(session, "cwd", readString, path),
    cols: readField(session, "cols", readU16, path),
    rows: readField(session, "rows", readU16, path),
    started_at: readField(session, "started_at", readRfc3339, path),
    ended_at: readOptionalField(session, "ended_at", readRfc3339, path),
    exit_code: readOptionalField(session, "exit_code", readI32, path),
    status: readField(session, "status", readTerminalStatus, path),
    group_name: readOptionalField(session, "group_name", readString, path),
    sort_order: readField(session, "sort_order", readI64Safe, path),
    created_by: readOptionalField(session, "created_by", readString, path),
    replay_acl: readOptionalField(session, "replay_acl", arrayOf(readString), path),
  };
}

function readObject(value: unknown, path: string): UnknownRecord {
  if (!isUnknownRecord(value)) return invalid(path, "object");
  return value;
}

function isUnknownRecord(value: unknown): value is UnknownRecord {
  return typeof value === "object" && value !== null && !Array.isArray(value);
}

function readString(value: unknown, path: string): string {
  if (typeof value !== "string") return invalid(path, "string");
  return value;
}

function readBoolean(value: unknown, path: string): boolean {
  if (typeof value !== "boolean") return invalid(path, "boolean");
  return value;
}

function readJsonValue(value: unknown, path: string): JsonValue {
  if (
    value === null ||
    typeof value === "string" ||
    typeof value === "boolean"
  ) {
    return value;
  }
  if (typeof value === "number") {
    if (!Number.isFinite(value)) return invalid(path, "finite JSON number");
    return value;
  }
  if (Array.isArray(value)) {
    return value.map((item, index) => readJsonValue(item, `${path}[${index}]`));
  }
  if (isUnknownRecord(value)) {
    // Object.fromEntries defines own data properties, including `__proto__`.
    // Indexed assignment into `{}` would invoke the legacy prototype setter,
    // lose that valid JSON key, and expose attacker-controlled inherited fields.
    return Object.fromEntries(
      Object.entries(value).map(([key, item]) => [
        key,
        readJsonValue(item, `${path}.${key}`),
      ]),
    );
  }
  return invalid(path, "JSON value");
}

function readAgentType(value: unknown, path: string): AgentType {
  switch (value) {
    case "codex":
    case "claude_code":
    case "red":
    case "local_llm":
      return value;
    default:
      return invalid(path, "known AgentType value");
  }
}

function readHookEventType(value: unknown, path: string): HookEventType {
  switch (value) {
    case "PreToolUse":
    case "PostToolUse":
    case "PostToolUseFailure":
    case "PermissionRequest":
    case "Elicitation":
    case "ElicitationResult":
    case "InstructionsLoaded":
    case "UserPromptSubmit":
    case "Stop":
    case "SubagentStop":
    case "SubagentStart":
    case "StopFailure":
    case "ConfigChange":
    case "TeammateIdle":
    case "TaskCompleted":
    case "WorktreeCreate":
    case "WorktreeRemove":
    case "PreCompact":
    case "PostCompact":
    case "SessionStart":
    case "SessionEnd":
    case "Notification":
    case "Unknown":
      return value;
    default:
      return invalid(path, "known HookEventType value");
  }
}

function readUuid(value: unknown, path: string): string {
  const uuid = readString(value, path);
  if (!/^[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$/i.test(uuid)) {
    return invalid(path, "hyphenated UUID");
  }
  return uuid;
}

function readRfc3339(value: unknown, path: string): string {
  const timestamp = readString(value, path);
  if (!/^\d{4}-\d{2}-\d{2}T\d{2}:\d{2}:\d{2}(?:\.\d+)?(?:Z|[+-]\d{2}:\d{2})$/.test(timestamp)) {
    return invalid(path, "RFC 3339 timestamp");
  }

  const year = Number(timestamp.slice(0, 4));
  const month = Number(timestamp.slice(5, 7));
  const day = Number(timestamp.slice(8, 10));
  const hour = Number(timestamp.slice(11, 13));
  const minute = Number(timestamp.slice(14, 16));
  const second = Number(timestamp.slice(17, 19));
  const offset = timestamp.endsWith("Z") ? null : timestamp.slice(-6);
  const offsetHour = offset ? Number(offset.slice(1, 3)) : 0;
  const offsetMinute = offset ? Number(offset.slice(4, 6)) : 0;

  if (
    month < 1 ||
    month > 12 ||
    day < 1 ||
    day > daysInMonth(year, month) ||
    hour > 23 ||
    minute > 59 ||
    second > 60 ||
    offsetHour > 23 ||
    offsetMinute > 59
  ) {
    return invalid(path, "RFC 3339 timestamp");
  }
  return timestamp;
}

function daysInMonth(year: number, month: number): number {
  switch (month) {
    case 2:
      return year % 4 === 0 && (year % 100 !== 0 || year % 400 === 0) ? 29 : 28;
    case 4:
    case 6:
    case 9:
    case 11:
      return 30;
    default:
      return 31;
  }
}

function readBoundedInteger(
  value: unknown,
  path: string,
  minimum: number,
  maximum: number,
  rustType: string,
): number {
  if (
    typeof value !== "number" ||
    !Number.isSafeInteger(value) ||
    value < minimum ||
    value > maximum
  ) {
    return invalid(path, `${rustType} integer`);
  }
  return value;
}

function readU16(value: unknown, path: string): number {
  return readBoundedInteger(value, path, 0, 65_535, "u16");
}

function readU32(value: unknown, path: string): number {
  return readBoundedInteger(value, path, 0, 4_294_967_295, "u32");
}

function readI32(value: unknown, path: string): number {
  return readBoundedInteger(value, path, -2_147_483_648, 2_147_483_647, "i32");
}

/**
 * Rust u64 values above Number.MAX_SAFE_INTEGER cannot survive JSON.parse
 * exactly. Reject them at the trust boundary instead of silently rounding.
 */
function readU64Safe(value: unknown, path: string): number {
  return readBoundedInteger(value, path, 0, Number.MAX_SAFE_INTEGER, "u64 (JavaScript-safe)");
}

/**
 * Rust i64 values outside JavaScript's safe-integer range cannot survive
 * JSON.parse exactly. The representable subset is validated explicitly.
 */
function readI64Safe(value: unknown, path: string): number {
  return readBoundedInteger(
    value,
    path,
    Number.MIN_SAFE_INTEGER,
    Number.MAX_SAFE_INTEGER,
    "i64 (JavaScript-safe)",
  );
}

function readDecision(value: unknown, path: string): "approve" | "deny" | "ask" {
  if (value !== "approve" && value !== "deny" && value !== "ask") {
    return invalid(path, '"approve", "deny", or "ask"');
  }
  return value;
}

function readAuditDecisionKind(value: unknown, path: string): AuditDecisionKind {
  if (value !== "auto_approved" && value !== "deferred" && value !== "denied") {
    return invalid(path, '"auto_approved", "deferred", or "denied"');
  }
  return value;
}

function readTerminalStatus(value: unknown, path: string): TerminalStatus {
  if (value !== "running" && value !== "exited" && value !== "killed" && value !== "orphaned") {
    return invalid(path, '"running", "exited", "killed", or "orphaned"');
  }
  return value;
}

function readTerminalDirection(value: unknown, path: string): TerminalDirection {
  if (value !== "input" && value !== "output" && value !== "resize") {
    return invalid(path, '"input", "output", or "resize"');
  }
  return value;
}

function readDiskAlertKind(value: unknown, path: string): DiskAlertKind {
  if (value !== "archive_size" && value !== "low_disk_space") {
    return invalid(path, '"archive_size" or "low_disk_space"');
  }
  return value;
}

function readConfigAlertKind(value: unknown, path: string): ConfigAlertKind {
  if (value !== "policy_widened" && value !== "untrusted_config") {
    return invalid(path, '"policy_widened" or "untrusted_config"');
  }
  return value;
}

function arrayOf<T>(parseItem: ValueParser<T>): ValueParser<T[]> {
  return (value, path) => {
    if (!Array.isArray(value)) return invalid(path, "array");
    return value.map((item, index) => parseItem(item, `${path}[${index}]`));
  };
}

function readField<T>(
  object: UnknownRecord,
  key: string,
  parseValue: ValueParser<T>,
  path: string,
): T {
  return parseValue(object[key], `${path}.${key}`);
}

function readOptionalField<T>(
  object: UnknownRecord,
  key: string,
  parseValue: ValueParser<T>,
  path: string,
): T | undefined {
  const value = object[key];
  if (value === undefined || value === null) return undefined;
  return parseValue(value, `${path}.${key}`);
}

function readOptionalJsonField(
  object: UnknownRecord,
  key: string,
  path: string,
): JsonValue | undefined {
  if (!Object.hasOwn(object, key)) return undefined;
  return readJsonValue(object[key], `${path}.${key}`);
}

function invalid(path: string, expected: string): never {
  throw new Error(`${path}: expected ${expected}`);
}

export interface SpawnAgentRequest {
  agent_type?: "claude_code" | "codex";
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
  | { type: "approve_all"; filter?: unknown; confirm?: boolean }
  | { type: "deny_all"; filter?: unknown }
  | { type: "query_history"; agent_id?: string; limit?: number; request_id?: string }
  | { type: "query_sessions" }
  | { type: "query_projects" }
  | { type: "install_hooks"; project: string }
  | { type: "query_project_hook_status"; project: string }
  | { type: "reimport_events" }
  | { type: "spawn_agent" } & SpawnAgentRequest
  | { type: "search_history"; query?: string; tool_name?: string; agent_id?: string; limit?: number; request_id?: string }
  | { type: "term_create"; label?: string; command?: string; args?: string[]; cwd?: string; cols: number; rows: number; env?: Record<string, string> }
  | { type: "term_attach"; id: string }
  | { type: "term_detach"; id: string }
  | { type: "term_input"; id: string; data: string }
  | { type: "term_resize"; id: string; cols: number; rows: number }
  | { type: "term_close"; id: string }
  | { type: "term_list" }
  | { type: "term_replay"; id: string; from_seq?: number; speed?: number }
  | { type: "term_set_group"; id: string; group?: string }
  | { type: "term_reorder"; id: string; sort_order: number };
