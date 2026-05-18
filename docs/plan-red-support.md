# Wisphive Red Support Spec

## Goal

Add first-class Wisphive support for spawning and supervising Red agents, with Wisphive acting as the blocking pre-execution permission gate for Red tool calls.

Red support should follow the same product model as Codex and Claude support: users can spawn a Red agent from CLI/TUI/web, see its pending tool requests, approve/deny/edit them, and have tool results attached to Wisphive history.

## Non-Goals

- Do not rely on observing Red stdout after execution has already started.
- Do not build a generic adapter framework unless Codex work has already established one.
- Do not change Wisphive's approval UI model specifically for Red; normalize Red into existing `DecisionRequest` / `RichDecision` flows.
- Do not bypass Red's own tool result wiring; denied tool calls should still become Red tool-result messages with `is_error: true`.

## Red-Side Prerequisite Contract

Wisphive Red support depends on Red exposing a blocking RPC permission point before tool execution.

Red must support JSON-lines RPC over stdin/stdout.

### Tool Request Event

Red emits this before executing a tool and waits for a matching decision:

```json
{
  "type": "tool_request",
  "toolCallId": "toolu_123",
  "toolName": "bash",
  "args": { "command": "cargo test" }
}
```

### Tool Decision Command

Wisphive sends one of:

```json
{
  "type": "tool_decision",
  "toolCallId": "toolu_123",
  "decision": "approve"
}
```

```json
{
  "type": "tool_decision",
  "toolCallId": "toolu_123",
  "decision": "approve",
  "updatedArgs": { "command": "cargo test -p red_agent" }
}
```

```json
{
  "type": "tool_decision",
  "toolCallId": "toolu_123",
  "decision": "deny",
  "message": "Do not run full workspace tests yet."
}
```

Red behavior:

- `approve` executes with original args.
- `approve + updatedArgs` executes with rewritten args.
- `deny` does not execute the tool and returns a Red tool-result message with `is_error: true`.
- `ToolExecutionStart` occurs only after approval or denial is resolved.
- `ToolExecutionEnd` is emitted for both executed and denied tools so Wisphive can attach final history.

## Wisphive Protocol Changes

Extend `SpawnAgentRequest`:

```rust
pub struct SpawnAgentRequest {
    pub project: PathBuf,
    pub prompt: String,

    #[serde(default)]
    pub agent_type: AgentType,

    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub binary: Option<String>,

    // existing fields...
}
```

Extend `ManagedAgent`:

```rust
pub struct ManagedAgent {
    pub agent_id: String,
    pub pid: u32,
    pub agent_type: AgentType,
    pub project: PathBuf,

    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub binary: Option<String>,

    // existing fields...
}
```

`AgentType::Red` should be used as the selector for Red spawning.

## Process Registry Changes

Update `ProcessRegistry::spawn_agent`:

```rust
match req.agent_type {
    AgentType::ClaudeCode => spawn_claude(req).await,
    AgentType::Red => spawn_red_rpc(req).await,
    _ => unsupported,
}
```

Red spawn behavior:

- Binary: `req.binary.unwrap_or("red")`
- Args: `--rpc`
- Optional args:
  - `--cwd <project>`
  - `--model <model>`
  - `--provider <provider>` if Wisphive later exposes provider selection
- Prompt is sent via RPC stdin after process startup:

```json
{ "type": "prompt", "id": "initial", "message": "<prompt>" }
```

Red process must keep stdin/stdout piped. Stderr should be logged or streamed to terminal history, not parsed as protocol.

Recommended new file:

```text
crates/wisphive_daemon/src/red_bridge.rs
```

## Red Bridge Responsibilities

The Red bridge owns:

- Red child stdin writer
- Red child stdout reader
- agent id
- project path
- queue/state/db/broadcast handles

On startup:

1. Register agent in `AgentRegistry` as `AgentType::Red`.
2. Broadcast `ServerMessage::AgentConnected`.
3. Send initial prompt RPC command.
4. Read Red stdout JSON lines until exit.

On `tool_request`:

1. Normalize Red tool name for Wisphive UI/policy.
2. Create `DecisionRequest`.
3. Persist pending decision.
4. Enqueue into `DecisionQueue`.
5. Wait for `RichDecision`.
6. Write Red `tool_decision` command.

On `tool_execution_end`:

1. Attach a `ToolResult` using Red `toolCallId` as `tool_use_id`.
2. Store result payload with at least:

```json
{
  "result": "...",
  "is_error": false,
  "native_tool_name": "bash"
}
```

On process exit:

1. Deregister from `AgentRegistry`.
2. Broadcast `ServerMessage::AgentExited`.
3. Resolve any in-flight tool approvals as denied or cancelled.

## Tool Name Normalization

Red emits lowercase tool names. Wisphive policy/UI mostly expects Claude-style names.

Use canonical display/policy names:

| Red | Wisphive |
| --- | --- |
| `bash` | `Bash` |
| `read` | `Read` |
| `write` | `Write` |
| `edit` | `Edit` |
| `grep` | `Grep` |
| `find` | `Find` |
| `ls` | `LS` or `List` |

Store native name in `event_data`:

```json
{
  "native_tool_name": "bash",
  "source": "red_rpc"
}
```

Policy should run against the canonical name. The bridge should send decisions back to Red by `toolCallId`, so canonical naming does not affect Red execution.

## Permission Semantics

For Red, Wisphive is the real blocking gate.

Recommended default for Red bridge:

- If Wisphive queue resolves approve: send approve.
- If queue resolves approve with `updated_input`: send approve with `updatedArgs`.
- If queue resolves deny: send deny.
- If bridge loses daemon state, stdin, or decision channel: deny.
- If human decision times out: prefer deny by default for Red, configurable later.

This is stricter than hook fail-open behavior and matches the Red requirement that execution must not proceed without a pre-execution decision.

## UI / CLI Changes

CLI:

```bash
wisphive agent start --agent-type red --binary red --project /repo --prompt "fix tests"
```

If Codex work already adds agent selection, Red should reuse that same selector.

TUI/web spawn modal:

- Add agent type selector: Claude Code, Codex, Red.
- Show `binary` field only for RPC-backed agents.
- Default Red binary: `red`.

Frontend protocol TypeScript should mirror `SpawnAgentRequest`:

```ts
export interface SpawnAgentRequest {
  project: string;
  prompt: string;
  agent_type?: "claude_code" | "codex" | "red" | "local_llm";
  binary?: string;
  model?: string;
  reasoning?: string;
  max_turns?: number;
}
```

## Tests

Required tests:

- Protocol serde for Red spawn request with `agent_type: "red"` and `binary`.
- Red bridge converts `tool_request` into `DecisionRequest`.
- Bridge does not send `tool_decision` until queue resolves.
- Approve writes Red `tool_decision` with no `updatedArgs`.
- Approve with edited input writes `updatedArgs`.
- Deny writes Red deny with message.
- Red lowercase tools normalize to Wisphive policy names.
- `tool_execution_end` attaches result by `tool_use_id`.
- Red child exit deregisters agent and broadcasts exit.
- Malformed Red JSON does not crash daemon.

## Acceptance Criteria

- User can spawn Red from Wisphive CLI/TUI/web.
- Red tool calls appear in Wisphive approval queue before execution.
- Approve, deny, and edit-input decisions affect Red execution correctly.
- Red tool results are attached to history by `toolCallId`.
- Bash/write/edit from Red follow Wisphive sudo/policy classes.
- Red process lifecycle appears in agent list and exit events.
- Existing Claude/Codex flows are unchanged.
