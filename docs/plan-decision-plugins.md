# Plan: Decision Plugins (Extensions for a Control Plane)

## Problem

Wisphive's extensibility today is limited to:
- **Auto-approve tiers** with substring-based deny/allow patterns in `config.json`
- **Agent adapters** — a trait with stubs for Red and LocalLLM (only Claude Code works)
- **Two UIs** — TUI and Web (hardcoded notification via `osascript`/`notify-send`)

Users need:
1. Real agent adapters that bridge Wisphive to the good workspace agents (Haunt, Coven via `--rpc`)
2. Richer policy rules beyond substring matching (regex, path globs, compound conditions)
3. Webhook/shell hooks triggered on decision events (Slack notifications, audit export, custom automation)

These don't need WASM/Rhai/Lua sandboxes — Wisphive's extension surface is narrow (policy evaluation + event notification), not broad (arbitrary tool execution). The right model is **config-driven rules + pluggable webhook hooks + real adapter implementations**.

## Design

### Part A: Agent Adapters

#### A1. Red/Haunt/Coven RPC Adapter

The good workspace agents all support `--rpc` mode: headless JSON-line protocol on stdin/stdout. Wisphive can spawn them as child processes (like it already spawns Claude Code via `ProcessRegistry`) and intercept their tool calls.

**Architecture:**

```
wisphive daemon
  ├── ProcessRegistry::spawn_agent()     ← spawns `haunt --rpc` or `coven --rpc`
  ├── RpcBridge (new)                    ← reads agent stdout, translates tool calls
  │     ├── reads AgentEvent JSON lines from stdout
  │     ├── on ToolUse event → creates DecisionRequest → enqueues in DecisionQueue
  │     ├── blocks until human approves/denies
  │     ├── sends approval/denial back to agent via stdin
  │     └── on TextDelta/Complete → forwards to TUI as agent activity
  └── AgentRegistry                      ← tracks the agent like any other
```

**New file: `wisphive_daemon/src/rpc_bridge.rs`**

```rust
/// Bridge between an RPC-mode agent (haunt --rpc, coven --rpc) and the Wisphive daemon.
///
/// Reads AgentEvent JSON lines from the agent's stdout. When a tool call is detected,
/// creates a DecisionRequest and blocks until the human resolves it. Then sends the
/// tool result (or denial) back to the agent via stdin.
pub struct RpcBridge {
    agent_id: String,
    child_stdin: ChildStdin,
    queue: Arc<Mutex<DecisionQueue>>,
    conflict_map: Arc<Mutex<FileConflictMap>>,
    tui_tx: broadcast::Sender<ServerMessage>,
}

impl RpcBridge {
    /// Start reading from the agent's stdout in a loop.
    /// Each tool call blocks on human approval via the DecisionQueue.
    pub async fn run(
        self,
        mut stdout_lines: Lines<BufReader<ChildStdout>>,
    ) -> Result<()> {
        while let Some(line) = stdout_lines.next_line().await? {
            let event: AgentEvent = serde_json::from_str(&line)?;
            match event {
                AgentEvent::ToolUse { id, name, input } => {
                    // Create DecisionRequest
                    let req = DecisionRequest { /* ... */ };
                    let rx = {
                        let mut q = self.queue.lock().await;
                        q.enqueue(req)
                    };
                    // Block until human decides
                    let decision = rx.await?;
                    // Send result back to agent via stdin
                    match decision.decision {
                        Decision::Approve => {
                            // Execute tool, send result back
                            // (or: tell agent to proceed — depends on RPC protocol)
                        }
                        Decision::Deny => {
                            // Send error result back to agent
                            let error_msg = decision.message.unwrap_or("Denied by Wisphive".into());
                            let result_json = json!({
                                "type": "tool_result",
                                "tool_use_id": id,
                                "is_error": true,
                                "content": error_msg
                            });
                            self.child_stdin.write_all(
                                format!("{}\n", result_json).as_bytes()
                            ).await?;
                        }
                    }
                }
                AgentEvent::TextDelta { text } => {
                    // Forward to TUI as agent activity
                    let _ = self.tui_tx.send(ServerMessage::AgentActivity {
                        agent_id: self.agent_id.clone(),
                        text,
                    });
                }
                AgentEvent::Complete { .. } => break,
                _ => {} // forward other events as needed
            }
        }
        Ok(())
    }
}
```

**Key design decision:** The RPC bridge intercepts tool calls *before* the agent executes them. This is different from the Claude Code hook path (which runs as a subprocess that Claude Code calls). Here, Wisphive IS the tool executor — it receives the tool call from the agent, decides whether to approve, executes the tool itself (or tells the agent to), and returns the result.

This requires understanding the RPC protocol used by Red/Haunt/Coven. The protocol is JSON lines on stdin/stdout with these key events:
- `ToolUse { id, name, input }` — agent wants to call a tool
- `TextDelta { text }` — streaming text output
- `Complete { stop_reason }` — agent finished

**Changes to `process_registry.rs`:**

Add an `AgentKind` enum:
```rust
pub enum AgentKind {
    ClaudeCode,           // spawned with `claude -p`, hooks handle gating
    RpcAgent { binary: String },  // spawned with `<binary> --rpc`, bridge handles gating
}
```

`spawn_agent()` gets a match on kind:
- `ClaudeCode` → existing logic (spawn claude, hooks intercept)
- `RpcAgent` → spawn binary with `--rpc`, create `RpcBridge`, spawn bridge task

**Changes to `SpawnAgentRequest` (protocol):**

```rust
pub struct SpawnAgentRequest {
    // ... existing fields ...
    /// For RPC agents: the binary name (e.g., "haunt", "coven")
    pub binary: Option<String>,
    /// Agent kind: "claude_code" (default) or "rpc"
    pub kind: Option<String>,
}
```

**TUI spawn modal:** Add a dropdown/toggle for agent type. "Claude Code" (default) or "RPC Agent" (prompts for binary name).

**CLI:**
```bash
wisphive agent spawn --kind rpc --binary haunt --project /path/to/project --prompt "fix the auth bug"
wisphive agent spawn --kind rpc --binary coven --project /path/to/project --prompt "run the backlog"
```

#### A2. Adapter Trait Usage

The existing `AgentAdapter` trait in `wisphive_adapters/src/adapter.rs` is well-designed but unused. Rather than building a full adapter registration system now, the RPC bridge approach above is more pragmatic — it uses the existing `ProcessRegistry` + a new bridge module. The `AgentAdapter` trait can be wired in later as a proper abstraction layer over both Claude Code hooks and RPC bridges.

For now, mark the trait as the future target and implement the concrete `RpcBridge` directly in the daemon. This avoids premature abstraction.

---

### Part B: Richer Policy Rules

#### B1. Current State

The hook's `is_auto_approved()` function in `wisphive_hook/src/main.rs:597-660` evaluates:
1. Explicit `auto_approve_remove` list
2. Explicit `auto_approve_add` list
3. Tiered level (off/read/write/execute/all)
4. Content-aware `tool_rules` with `deny_patterns` / `allow_patterns` (case-insensitive substring)

Substring matching is too blunt. `deny_patterns: ["rm"]` blocks `rm -rf /` but also `cargo fmt --check` (contains "rm" in "fmt"). Users need regex, path globs, and compound conditions.

#### B2. Extended Rule Schema

Expand `tool_rules` in `config.json` with backward-compatible new fields:

```json
{
  "tool_rules": {
    "Bash": {
      "deny_patterns": ["rm -rf", "DROP TABLE"],
      "deny_regex": [
        "curl\\s+.*\\|\\s*(sh|bash)",
        "wget\\s+.*\\|\\s*(sh|bash)",
        "\\bsudo\\b"
      ],
      "allow_patterns": ["cargo test", "cargo build"],
      "allow_regex": ["^cargo\\s+(test|build|clippy|fmt)"]
    },
    "Write": {
      "deny_paths": [
        "**/.env",
        "**/*.pem",
        "**/*.key",
        "**/secrets/**"
      ],
      "review_paths": [
        "**/Cargo.toml",
        "**/package.json",
        "**/.github/**"
      ]
    },
    "Edit": {
      "deny_paths": ["**/.env", "**/*.key"],
      "review_paths": ["**/Cargo.toml"]
    }
  }
}
```

**New rule types:**

| Field | Applies To | Semantics |
|-------|-----------|-----------|
| `deny_patterns` | All tools | Existing: case-insensitive substring on tool input text |
| `allow_patterns` | All tools | Existing: case-insensitive substring on tool input text |
| `deny_regex` | All tools | New: regex match on tool input text (Rust `regex` crate) |
| `allow_regex` | All tools | New: regex match on tool input text |
| `deny_paths` | Write, Edit, NotebookEdit | New: glob match on the `file_path` field |
| `review_paths` | Write, Edit, NotebookEdit | New: force human review even if auto-approve level includes writes |

**Evaluation order (within a single tool's rules):**
1. `deny_paths` / `deny_regex` / `deny_patterns` — any match → not auto-approved (goes to daemon)
2. `review_paths` — any match → not auto-approved
3. `allow_regex` / `allow_patterns` — any match → auto-approved
4. Fall through to base level check

#### B3. Implementation in `wisphive_hook`

**New dependency:** Add `regex` to `wisphive_hook/Cargo.toml`. The `glob` crate (or a simple glob matcher) for path patterns. Both are lightweight — `regex` is ~1MB, well within budget for a hook binary.

**Changes to `is_auto_approved()` in `main.rs`:**

```rust
fn evaluate_tool_rules(
    tool_name: &str,
    tool_input: &Value,
    rules: &Value,
) -> Option<bool> {
    let input_text = tool_input_text(tool_name, tool_input);
    let input_lower = input_text.to_lowercase();
    let file_path = extract_file_path_from_input(tool_name, tool_input);

    // Phase 1: Deny checks (any match → return Some(false))
    if let Some(patterns) = rules.get("deny_regex").and_then(|v| v.as_array()) {
        for p in patterns.iter().filter_map(|v| v.as_str()) {
            if let Ok(re) = regex::Regex::new(p) {
                if re.is_match(&input_text) {
                    return Some(false);
                }
            }
        }
    }
    if let Some(ref path) = file_path {
        if let Some(globs) = rules.get("deny_paths").and_then(|v| v.as_array()) {
            for g in globs.iter().filter_map(|v| v.as_str()) {
                if glob_match(g, path) {
                    return Some(false);
                }
            }
        }
    }
    // ... existing deny_patterns check ...

    // Phase 2: Review paths (any match → return Some(false), force human review)
    if let Some(ref path) = file_path {
        if let Some(globs) = rules.get("review_paths").and_then(|v| v.as_array()) {
            for g in globs.iter().filter_map(|v| v.as_str()) {
                if glob_match(g, path) {
                    return Some(false);
                }
            }
        }
    }

    // Phase 3: Allow checks (any match → return Some(true))
    if let Some(patterns) = rules.get("allow_regex").and_then(|v| v.as_array()) {
        for p in patterns.iter().filter_map(|v| v.as_str()) {
            if let Ok(re) = regex::Regex::new(p) {
                if re.is_match(&input_text) {
                    return Some(true);
                }
            }
        }
    }
    // ... existing allow_patterns check ...

    None // no rule matched, fall through to base level
}
```

**Glob matching:** Use a minimal glob function (~30 LOC) that handles `*`, `**`, and `?`. No external crate needed — the patterns are simple enough.

```rust
fn glob_match(pattern: &str, path: &Path) -> bool {
    let pat = pattern.to_lowercase();
    let path_str = path.to_string_lossy().to_lowercase();
    // Simple glob: ** matches any path segments, * matches within a segment
    let regex_str = pat
        .replace(".", "\\.")
        .replace("**", "§§")     // placeholder
        .replace("*", "[^/]*")
        .replace("§§", ".*");
    regex::Regex::new(&format!("(^|/){}$", regex_str))
        .map(|re| re.is_match(&path_str))
        .unwrap_or(false)
}
```

#### B4. Regex Compilation Cache

Regexes are expensive to compile. The hook is a short-lived subprocess (one invocation per tool call), so caching within a single run doesn't help much. But the patterns come from a stable config file. Options:

- **Option A (simple):** Compile on each invocation. Regex compilation is ~1-5μs per pattern. With <20 patterns, this is <100μs total. Acceptable for the hook's latency budget.
- **Option B (if needed later):** Pre-compile regexes when config is loaded and cache in a `lazy_static` or `OnceLock`. Only worth doing if profiling shows regex compilation is a bottleneck.

Start with Option A. The hook already reads and parses `config.json` on every invocation — regex compilation is noise next to file I/O.

---

### Part C: Decision Webhooks

#### C1. Event Hooks

Configurable actions triggered when decisions are resolved. Not a scripting runtime — just HTTP POST or shell command execution.

**Config schema:**

```json
{
  "decision_hooks": {
    "on_deny": [
      {
        "type": "webhook",
        "url": "https://hooks.slack.com/services/T.../B.../xxx",
        "method": "POST",
        "headers": { "Content-Type": "application/json" },
        "body_template": "{\"text\": \"🚫 Denied `{{tool_name}}` from {{agent_id}} in {{project}}: {{message}}\"}"
      }
    ],
    "on_approve": [],
    "on_conflict": [
      {
        "type": "webhook",
        "url": "https://hooks.slack.com/services/T.../B.../xxx",
        "body_template": "{\"text\": \"⚠️ Conflict: {{requesting_agent}} wants {{file}} (held by {{holding_agent}})\"}"
      }
    ],
    "on_agent_connected": [
      {
        "type": "shell",
        "command": "echo '{{agent_id}} connected to {{project}}' >> ~/wisphive-events.log"
      }
    ],
    "on_agent_exited": [
      {
        "type": "shell",
        "command": "say 'Agent {{agent_id}} has finished'"
      }
    ]
  }
}
```

**Supported events:**

| Event | Template Variables |
|-------|--------------------|
| `on_approve` | `agent_id`, `tool_name`, `tool_input`, `project`, `message`, `tool_use_id` |
| `on_deny` | same as above |
| `on_conflict` | `requesting_agent`, `holding_agent`, `file`, `project`, `claimed_at` |
| `on_agent_connected` | `agent_id`, `agent_type`, `project` |
| `on_agent_exited` | `agent_id`, `exit_code` |
| `on_queue_empty` | (no variables — fires when last pending decision is resolved) |

**Hook types:**

| Type | Behavior |
|------|----------|
| `webhook` | Async HTTP POST to URL. Fire-and-forget (errors logged, never block). Template variables replaced in `body_template`. Optional `headers` and `method`. |
| `shell` | Async `tokio::process::Command`. Fire-and-forget. Template variables replaced in `command`. Runs with daemon's env. |

#### C2. Implementation

**New file: `wisphive_daemon/src/decision_hooks.rs`**

```rust
pub struct HookDispatcher {
    client: reqwest::Client,
    config: HooksConfig,
}

#[derive(Debug, Clone, Deserialize)]
pub struct HooksConfig {
    #[serde(default)]
    pub on_approve: Vec<HookAction>,
    #[serde(default)]
    pub on_deny: Vec<HookAction>,
    #[serde(default)]
    pub on_conflict: Vec<HookAction>,
    #[serde(default)]
    pub on_agent_connected: Vec<HookAction>,
    #[serde(default)]
    pub on_agent_exited: Vec<HookAction>,
    #[serde(default)]
    pub on_queue_empty: Vec<HookAction>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(tag = "type")]
pub enum HookAction {
    #[serde(rename = "webhook")]
    Webhook {
        url: String,
        #[serde(default = "default_post")]
        method: String,
        #[serde(default)]
        headers: HashMap<String, String>,
        body_template: String,
    },
    #[serde(rename = "shell")]
    Shell {
        command: String,
    },
}

impl HookDispatcher {
    pub fn new(config: HooksConfig) -> Self {
        Self {
            client: reqwest::Client::new(),
            config,
        }
    }

    /// Dispatch hooks for an event. All hooks are fire-and-forget.
    pub fn dispatch(&self, event: HookEvent) {
        let actions = match &event {
            HookEvent::Approve { .. } => &self.config.on_approve,
            HookEvent::Deny { .. } => &self.config.on_deny,
            HookEvent::Conflict { .. } => &self.config.on_conflict,
            HookEvent::AgentConnected { .. } => &self.config.on_agent_connected,
            HookEvent::AgentExited { .. } => &self.config.on_agent_exited,
            HookEvent::QueueEmpty => &self.config.on_queue_empty,
        };

        let vars = event.template_vars();

        for action in actions {
            let action = action.clone();
            let vars = vars.clone();
            let client = self.client.clone();
            tokio::spawn(async move {
                if let Err(e) = execute_action(&client, &action, &vars).await {
                    tracing::warn!("hook failed: {e}");
                }
            });
        }
    }
}

fn render_template(template: &str, vars: &HashMap<String, String>) -> String {
    let mut result = template.to_string();
    for (key, value) in vars {
        // Escape JSON special chars in value to prevent injection
        let escaped = value.replace('\\', "\\\\").replace('"', "\\\"");
        result = result.replace(&format!("{{{{{}}}}}", key), &escaped);
    }
    result
}

async fn execute_action(
    client: &reqwest::Client,
    action: &HookAction,
    vars: &HashMap<String, String>,
) -> Result<()> {
    match action {
        HookAction::Webhook { url, method, headers, body_template } => {
            let body = render_template(body_template, vars);
            let mut req = match method.to_uppercase().as_str() {
                "GET" => client.get(url),
                _ => client.post(url),
            };
            for (k, v) in headers {
                req = req.header(k, v);
            }
            req.body(body).send().await?;
        }
        HookAction::Shell { command } => {
            let rendered = render_template(command, vars);
            tokio::process::Command::new("sh")
                .args(["-c", &rendered])
                .spawn()?;
        }
    }
    Ok(())
}
```

#### C3. Wiring into the daemon

**In `handle_hook` (server.rs):** After resolving a decision, dispatch the appropriate hook:

```rust
// After: state_db.resolve_pending(id, rich.decision).await?
match rich.decision {
    Decision::Approve => hook_dispatcher.dispatch(HookEvent::Approve {
        agent_id: agent_id.clone(),
        tool_name: req_tool_name.clone(),
        project: req_project.clone(),
        // ...
    }),
    Decision::Deny => hook_dispatcher.dispatch(HookEvent::Deny {
        agent_id: agent_id.clone(),
        tool_name: req_tool_name.clone(),
        message: rich.message.clone(),
        // ...
    }),
    _ => {}
}
```

**In the queue empty check:** After resolving a decision, if `queue.is_empty()`:
```rust
hook_dispatcher.dispatch(HookEvent::QueueEmpty);
```

**New dependency:** `reqwest` in `wisphive_daemon/Cargo.toml`. This is already a transitive dependency (used by `wisphive_web` via axum), so it adds no new compilation cost.

#### C4. Config Hot-Reload

The daemon reads `config.json` at startup. For hooks to be useful, users need to update config without restarting the daemon. Add a file watcher (the `notify` crate is already a workspace dependency) on `~/.wisphive/config.json`:

```rust
// In Server::run(), alongside other interval tasks:
let config_watcher = spawn_config_watcher(
    config.home_dir.join("config.json"),
    hook_dispatcher.clone(),
);
```

When the file changes, re-parse `decision_hooks` from the new config and swap it into the dispatcher via an `Arc<RwLock<HooksConfig>>`.

---

### Part D: Security Considerations

#### Webhook Safety
- **No secrets in templates:** Template variables come from tool inputs, which may contain sensitive code. The `body_template` should be treated as potentially leaking agent context to external services. Document this clearly.
- **URL validation:** Only `https://` URLs should be allowed for webhooks (or `http://localhost` for local development). Reject file://, ftp://, etc.
- **Timeout:** Webhook requests get a 5-second timeout. Fire-and-forget — never block decision resolution on webhook delivery.
- **Rate limiting:** If `on_approve` hooks fire on every auto-approved tool call (via event ingest), this could generate thousands of requests. Solution: `on_approve` hooks only fire for human-reviewed approvals, not auto-approved ones. Add `include_auto_approved: bool` (default false) for users who want the firehose.

#### Shell Command Safety
- Shell hooks run with the daemon's permissions. Document that `decision_hooks.shell` commands execute arbitrary code.
- Template variable injection: a tool input containing `$(rm -rf /)` could be injected into a shell template. The `render_template` function must escape shell metacharacters when rendering into shell commands. Use single-quoting: `echo '{{agent_id}}'` — and escape any `'` in the value.

#### RPC Bridge Safety
- The RPC bridge spawns child processes and communicates via stdin/stdout. The bridge trusts the agent's JSON output. A malicious binary posing as `haunt --rpc` could emit crafted tool calls. This is the same trust model as Claude Code's hook — you trust the binary you're spawning.

---

## Implementation Order

| Phase | What | Crate | Depends On | Size |
|-------|------|-------|------------|------|
| **1** | Extended policy rules (regex + path globs) | `wisphive_hook` | nothing | ~150 LOC |
| **2** | Decision webhooks + shell hooks | `wisphive_daemon` (new `decision_hooks.rs`) | nothing | ~300 LOC |
| **3** | Config hot-reload for hooks | `wisphive_daemon` | Phase 2 | ~80 LOC |
| **4** | RPC bridge for good workspace agents | `wisphive_daemon` (new `rpc_bridge.rs`) | nothing | ~400 LOC |
| **5** | `SpawnAgentRequest` kind field + CLI/TUI spawn changes | `wisphive_protocol`, `wisphive_cli`, `wisphive_tui` | Phase 4 | ~100 LOC |
| **6** | Web UI: spawn modal agent type selector + webhook config panel | `wisphive_web` frontend | Phase 4, 5 | ~200 LOC |

**Total estimate:** ~1,230 LOC across all phases. One new dependency (`regex` in `wisphive_hook`).

**Recommended start:** Phase 1 (policy rules) is the highest-value, lowest-risk change — it's entirely within the hook subprocess, no daemon changes, and immediately useful. Phase 2 (webhooks) is next — Slack notifications for denials is a frequently requested workflow. Phase 4 (RPC bridge) is the most complex and should be done after the conflict gate plan is implemented, since the bridge benefits from conflict detection.

---

## Testing Strategy

### Policy Rules (Phase 1)
- `test_deny_regex_blocks_curl_pipe_sh`
- `test_deny_regex_does_not_block_safe_curl`
- `test_allow_regex_approves_cargo_commands`
- `test_deny_paths_blocks_env_file`
- `test_deny_paths_with_glob_star_star`
- `test_review_paths_forces_human_review`
- `test_backward_compat_existing_patterns_still_work`
- `test_invalid_regex_is_ignored_not_crash`

### Decision Webhooks (Phase 2-3)
- `test_render_template_replaces_vars`
- `test_render_template_escapes_json_in_values`
- `test_dispatch_fires_correct_event_hooks`
- `test_webhook_timeout_does_not_block`
- `test_shell_hook_executes_command`
- `test_config_hot_reload_picks_up_changes`

### RPC Bridge (Phase 4)
- `test_bridge_creates_decision_request_on_tool_use`
- `test_bridge_blocks_until_decision`
- `test_bridge_sends_denial_back_to_agent`
- `test_bridge_forwards_text_delta_to_tui`
- `test_bridge_handles_agent_exit`
