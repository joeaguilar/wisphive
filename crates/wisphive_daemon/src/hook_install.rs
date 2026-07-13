//! Wisphive hook installation machinery.
//!
//! Pure library API for writing/removing the Wisphive hook entries in a
//! project's `.claude/settings.json` and `.codex/hooks.json`. Lives in the
//! daemon crate (not the CLI) so both the `wisphive hooks install` command and
//! the in-daemon web-driven install (itr#460) share one implementation.
//!
//! The library functions are **silent** — they emit `tracing` diagnostics
//! rather than printing to stdout/stderr, because the daemon must never write
//! to its own stdio. The CLI wrapper re-adds the human-facing status lines
//! around these calls (see `wisphive_cli::commands::hooks`).
//!
//! `hook_binary_path()` resolves the `wisphive-hook` binary next to the
//! currently running executable (install.sh installs `wisphive` and
//! `wisphive-hook` side by side), so it works whether the caller is the CLI or
//! the daemon.

use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use tracing::info;

use crate::project_audit::{CLAUDE_HOOK_EVENTS, CODEX_HOOK_EVENTS};

/// Permissions that Wisphive adds to .claude/settings.json so Claude Code
/// auto-allows tools that Wisphive will gate via its hook.
/// This eliminates the double-prompt — wisphive becomes the sole gatekeeper.
const WISPHIVE_PERMISSIONS: &[&str] = &["Bash(*)", "Edit(*)", "Write(*)", "NotebookEdit(*)"];

/// Safety margin (seconds) added on top of the effective daemon hook-approval
/// timeout when writing the synchronous `timeout` field of an installed
/// Wisphive hook entry.
///
/// INVARIANT (itr#510): every installed synchronous Wisphive hook entry must
/// carry a `timeout` that strictly exceeds the daemon's effective
/// `hook_timeout_secs` (default 3600, configurable up to 86400). Claude Code
/// cancels a command hook after its `timeout` — **600 s when the field is
/// omitted** — so an absent or short timeout lets Claude Code kill the
/// blocking `wisphive-hook` and abandon the pending approval before the
/// daemon's own timeout resolves it: the human approves in the TUI, but the
/// agent has already given up and moved on. The same invariant is re-checked
/// at managed-spawn time (`process_registry::inspect_hook_settings`), so a
/// daemon timeout raised *after* install refuses spawns until
/// `wisphive hooks install` rewrites the entries.
pub(crate) const HOOK_TIMEOUT_MARGIN_SECS: u64 = 100;

/// The hook `timeout` value the installer writes for a daemon approval
/// timeout of `daemon_hook_timeout_secs`. See [`HOOK_TIMEOUT_MARGIN_SECS`]
/// for the invariant this maintains.
pub(crate) fn installed_hook_timeout_secs(daemon_hook_timeout_secs: u64) -> u64 {
    daemon_hook_timeout_secs.saturating_add(HOOK_TIMEOUT_MARGIN_SECS)
}

/// The timeout to install right now: the effective daemon hook-approval
/// timeout from `~/.wisphive/config.json` (defaulted + clamped exactly as the
/// daemon does) plus the safety margin.
fn effective_installed_hook_timeout_secs() -> u64 {
    installed_hook_timeout_secs(crate::config::effective_hook_timeout_secs_default_home())
}

/// [`effective_installed_hook_timeout_secs`] for an explicit Wisphive home:
/// the daemon-aligned timeout derived from `<wisphive_home>/config.json`
/// (defaulted + clamped exactly as `DaemonConfig::new` does) plus the margin.
fn effective_installed_hook_timeout_secs_in(wisphive_home: &Path) -> u64 {
    installed_hook_timeout_secs(crate::config::effective_hook_timeout_secs(wisphive_home))
}

/// Reminder shown after installing Codex hooks: Codex requires the user to
/// trust the hook command in `/hooks`, not just the project.
pub const CODEX_HOOK_REVIEW_NOTE: &str = "Codex project hooks are non-managed hooks. \
After installing or changing them, open /hooks in Codex and trust the Wisphive hook command; \
project trust alone is not enough for Codex to run them.";

/// Install Wisphive hooks (Claude Code + Codex) for `project`, deriving the
/// installed timeout from the default `~/.wisphive` home.
///
/// Silent library entry point used by standalone CLI processes. Performs
/// surgical JSON editing — only adds Wisphive entries, preserves everything
/// else. Idempotent: re-installing does not duplicate entries.
pub fn install_hooks(project: &Path) -> Result<()> {
    // Prepare and validate both user-owned files before writing either one.
    // A malformed Codex config must not leave Claude hooks half-installed (or
    // vice versa) merely because it was discovered second.
    let hook_timeout_secs = effective_installed_hook_timeout_secs();
    let claude = prepare_claude_install(project, hook_timeout_secs)?;
    let codex = prepare_codex_install(project, hook_timeout_secs)?;
    write_prepared_install(claude)?;
    write_prepared_install(codex)?;
    Ok(())
}

/// [`install_hooks`], but with the daemon-aligned hook timeout derived from
/// the explicit Wisphive home at `wisphive_home` instead of the default
/// `~/.wisphive`. Use this from a live daemon, whose configured home may
/// differ from the standalone CLI default.
pub fn install_hooks_in_home(project: &Path, wisphive_home: &Path) -> Result<()> {
    // As in `install_hooks`, validate both files before atomically replacing
    // either one so a malformed second config cannot cause a partial install.
    let hook_timeout_secs = effective_installed_hook_timeout_secs_in(wisphive_home);
    let claude = prepare_claude_install(project, hook_timeout_secs)?;
    let codex = prepare_codex_install(project, hook_timeout_secs)?;
    write_prepared_install(claude)?;
    write_prepared_install(codex)?;
    Ok(())
}

/// Install Wisphive hooks into `<project>/.claude/settings.json`.
///
/// Every entry is written with a synchronous `timeout` that exceeds the
/// effective daemon hook-approval timeout (see [`HOOK_TIMEOUT_MARGIN_SECS`],
/// itr#510); reinstalling upgrades legacy timeout-less entries in place.
///
/// Returns the path written. Silent — logs via `tracing::info!`.
pub fn install_claude(project: &Path) -> Result<PathBuf> {
    write_prepared_install(prepare_claude_install(
        project,
        effective_installed_hook_timeout_secs(),
    )?)
}

/// [`install_claude`], but with the daemon-aligned hook timeout derived from
/// the Wisphive home at `wisphive_home` instead of the default `~/.wisphive`.
/// Same end-to-end path (config load → clamp → margin → prepare → atomic
/// write); use it when the target daemon runs with a non-default state dir,
/// and in hermetic tests.
pub fn install_claude_in_home(project: &Path, wisphive_home: &Path) -> Result<PathBuf> {
    write_prepared_install(prepare_claude_install(
        project,
        effective_installed_hook_timeout_secs_in(wisphive_home),
    )?)
}

struct PreparedInstall {
    path: PathBuf,
    contents: String,
    agent_name: &'static str,
}

fn prepare_claude_install(project: &Path, hook_timeout_secs: u64) -> Result<PreparedInstall> {
    let settings_path = project.join(".claude").join("settings.json");

    // Read existing settings or start fresh.
    let mut settings = read_hook_settings(&settings_path)?;
    ensure_hooks_object(&mut settings, &settings_path)?;

    // Hook commands run via `sh -c`, so a binary path with special characters
    // must be quoted — both for correct execution and so the itr#359 matcher
    // recognizes our own entry on reinstall/uninstall. Plain paths pass
    // through unquoted (no churn in existing settings files).
    let hook_command = shell_quote_command(&hook_binary_path());

    // Write an explicit synchronous timeout on every entry (itr#510): a
    // PreToolUse hook blocks for up to the daemon's approval timeout, and
    // Claude Code's implicit 600 s hook timeout would cancel it first. Other
    // events (Stop, UserPromptSubmit, ...) can also route to the daemon queue
    // when their auto-approve toggles are off, so all entries get the aligned
    // value. This also upgrades legacy entries written without a timeout.
    for event in CLAUDE_HOOK_EVENTS {
        add_hook_entry_with_timeout(&mut settings, event, &hook_command, Some(hook_timeout_secs))?;
    }

    // Add permissions so Claude Code auto-allows tools wisphive gates
    // (eliminates double-prompt — wisphive becomes the sole gatekeeper)
    add_wisphive_permissions(&mut settings);

    let formatted = serde_json::to_string_pretty(&settings)?;
    Ok(PreparedInstall {
        path: settings_path,
        contents: formatted,
        agent_name: "Claude",
    })
}

/// Install Wisphive hooks into `<project>/.codex/hooks.json`.
///
/// Returns the path written. Silent — logs via `tracing::info!`.
pub fn install_codex(project: &Path) -> Result<PathBuf> {
    write_prepared_install(prepare_codex_install(
        project,
        effective_installed_hook_timeout_secs(),
    )?)
}

fn prepare_codex_install(project: &Path, hook_timeout_secs: u64) -> Result<PreparedInstall> {
    let hooks_path = project.join(".codex").join("hooks.json");

    let mut settings = read_hook_settings(&hooks_path)?;
    ensure_hooks_object(&mut settings, &hooks_path)?;

    let hook_command = codex_hook_command(&hook_binary_path());

    // Same daemon-aligned timeout as the Claude install (itr#510) — this was
    // previously a fixed 3700, which only covered the default 3600 daemon
    // timeout, not a raised one.
    for event in CODEX_HOOK_EVENTS {
        add_hook_entry_with_timeout(&mut settings, event, &hook_command, Some(hook_timeout_secs))?;
    }

    let formatted = serde_json::to_string_pretty(&settings)?;
    Ok(PreparedInstall {
        path: hooks_path,
        contents: formatted,
        agent_name: "Codex",
    })
}

fn read_hook_settings(path: &Path) -> Result<serde_json::Value> {
    if !path.exists() {
        return Ok(serde_json::json!({}));
    }

    let content = std::fs::read_to_string(path)
        .with_context(|| format!("reading hook settings {}", path.display()))?;
    serde_json::from_str(&content)
        .with_context(|| format!("parsing hook settings {}", path.display()))
}

fn ensure_hooks_object(settings: &mut serde_json::Value, path: &Path) -> Result<()> {
    let root = settings.as_object_mut().with_context(|| {
        format!(
            "{} must contain a JSON object at the document root",
            path.display()
        )
    })?;
    let hooks = root.entry("hooks").or_insert_with(|| serde_json::json!({}));
    if !hooks.is_object() {
        anyhow::bail!(
            "{} has an invalid `hooks` value: `hooks` must be a JSON object; \
             replace it with `\"hooks\": {{}}` or remove the key, then retry \
             hook installation",
            path.display()
        );
    }

    Ok(())
}

fn write_prepared_install(prepared: PreparedInstall) -> Result<PathBuf> {
    let dir = prepared.path.parent().ok_or_else(|| {
        anyhow::anyhow!(
            "hook settings path has no parent directory: {}",
            prepared.path.display()
        )
    })?;
    std::fs::create_dir_all(dir)?;
    crate::config::write_config_atomic(&prepared.path, &prepared.contents).with_context(|| {
        format!(
            "atomically writing hook settings {}",
            prepared.path.display()
        )
    })?;

    info!(path = %prepared.path.display(), agent = prepared.agent_name, "Wisphive hooks installed");
    Ok(prepared.path)
}

/// Remove Wisphive hooks from `<project>/.claude/settings.json`.
///
/// Returns `Some(path)` if a settings file existed and was rewritten, `None`
/// if no settings file was present. Only removes Wisphive entries — preserves
/// everything else. Silent — logs via `tracing::info!`.
pub fn uninstall_claude(project: &Path) -> Result<Option<PathBuf>> {
    let settings_path = project.join(".claude").join("settings.json");

    if !settings_path.exists() {
        return Ok(None);
    }

    let content = std::fs::read_to_string(&settings_path)?;
    let mut settings: serde_json::Value = serde_json::from_str(&content)?;

    let hook_command = hook_binary_path();

    // Remove Wisphive entries from all hook types
    for event in CLAUDE_HOOK_EVENTS {
        remove_hook_entries(&mut settings, event, &hook_command);
    }

    // Remove wisphive-managed permissions (preserves user-added ones)
    remove_wisphive_permissions(&mut settings);

    let formatted = serde_json::to_string_pretty(&settings)?;
    std::fs::write(&settings_path, formatted)?;

    info!(path = %settings_path.display(), "Wisphive Claude hooks removed");
    Ok(Some(settings_path))
}

/// Remove Wisphive hooks from `<project>/.codex/hooks.json`.
///
/// Returns `Some(path)` if a hooks file existed and was rewritten, `None` if
/// no hooks file was present. Silent — logs via `tracing::info!`.
pub fn uninstall_codex(project: &Path) -> Result<Option<PathBuf>> {
    let hooks_path = project.join(".codex").join("hooks.json");

    if !hooks_path.exists() {
        return Ok(None);
    }

    let content = std::fs::read_to_string(&hooks_path)?;
    let mut settings: serde_json::Value = serde_json::from_str(&content)?;

    let hook_command = hook_binary_path();

    for event in CODEX_HOOK_EVENTS {
        remove_hook_entries(&mut settings, event, &hook_command);
    }

    let formatted = serde_json::to_string_pretty(&settings)?;
    std::fs::write(&hooks_path, formatted)?;

    info!(path = %hooks_path.display(), "Wisphive Codex hooks removed");
    Ok(Some(hooks_path))
}

/// Get the path to the wisphive-hook binary.
fn hook_binary_path() -> String {
    // Look for wisphive-hook next to the wisphive binary, or in PATH
    if let Ok(exe) = std::env::current_exe()
        && let Some(dir) = exe.parent()
    {
        let hook_path = dir.join("wisphive-hook");
        if hook_path.exists() {
            return hook_path.to_string_lossy().to_string();
        }
    }
    // Fallback: assume it's in PATH
    "wisphive-hook".into()
}

/// Add Wisphive-managed permissions to the settings JSON.
/// Ensures Claude Code auto-allows tools that Wisphive gates,
/// eliminating the double-prompt.
fn add_wisphive_permissions(settings: &mut serde_json::Value) {
    if settings.get("permissions").is_none() {
        settings["permissions"] = serde_json::json!({});
    }
    if settings["permissions"].get("allow").is_none() {
        settings["permissions"]["allow"] = serde_json::json!([]);
    }

    if let Some(allow_arr) = settings["permissions"]["allow"].as_array_mut() {
        for &perm in WISPHIVE_PERMISSIONS {
            let already_present = allow_arr
                .iter()
                .any(|v| v.as_str().is_some_and(|s| s == perm));
            if !already_present {
                allow_arr.push(serde_json::Value::String(perm.to_string()));
            }
        }
    }
}

/// Remove Wisphive-managed permissions from the settings JSON.
/// Only removes permissions from the known WISPHIVE_PERMISSIONS list.
fn remove_wisphive_permissions(settings: &mut serde_json::Value) {
    if let Some(permissions) = settings.get_mut("permissions")
        && let Some(allow_arr) = permissions.get_mut("allow").and_then(|v| v.as_array_mut())
    {
        allow_arr.retain(|v| {
            v.as_str()
                .is_some_and(|s| !WISPHIVE_PERMISSIONS.contains(&s))
        });
    }
}

/// Add a Wisphive hook entry to the settings JSON, avoiding duplicates.
///
/// Claude Code hook config format:
/// ```json
/// {
///   "hooks": {
///     "PreToolUse": [
///       {
///         "matcher": "",
///         "hooks": [
///           { "type": "command", "command": "/path/to/wisphive-hook" }
///         ]
///       }
///     ]
///   }
/// }
/// ```
#[cfg(test)]
fn add_hook_entry(settings: &mut serde_json::Value, hook_type: &str, command: &str) -> Result<()> {
    add_hook_entry_with_timeout(settings, hook_type, command, None)
}

fn add_hook_entry_with_timeout(
    settings: &mut serde_json::Value,
    hook_type: &str,
    command: &str,
    timeout: Option<u64>,
) -> Result<()> {
    let hooks = settings
        .get_mut("hooks")
        .context("hook settings are missing the required `hooks` object")?
        .as_object_mut()
        .context("hook settings `hooks` must be a JSON object")?;

    let entries = hooks
        .entry(hook_type)
        .or_insert_with(|| serde_json::json!([]));

    if let Some(arr) = entries.as_array_mut() {
        let already_present = update_existing_wisphive_hooks(arr, command, timeout);
        if already_present {
            return Ok(());
        }
    }

    if let Some(arr) = entries.as_array_mut() {
        let mut command_hook = serde_json::json!({
            "type": "command",
            "command": command
        });
        if let Some(timeout) = timeout
            && let Some(obj) = command_hook.as_object_mut()
        {
            obj.insert("timeout".into(), serde_json::json!(timeout));
        }

        arr.push(serde_json::json!({
            "matcher": "",
            "hooks": [
                command_hook
            ]
        }));
    }

    Ok(())
}

fn codex_hook_command(command: &str) -> String {
    format!(
        "env WISPHIVE_AGENT_TYPE=codex {}",
        shell_quote_command(command)
    )
}

fn shell_quote_command(command: &str) -> String {
    if command
        .bytes()
        .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'/' | b'.' | b'_' | b'-'))
    {
        return command.to_string();
    }

    format!("'{}'", command.replace('\'', "'\\''"))
}

/// Whether a hook command string invokes the wisphive-hook binary (itr#359).
///
/// Precise on purpose: the old `cmd.contains("wisphive")` matcher rewrote or
/// deleted any USER hook whose command merely mentioned the word — e.g. a
/// project-local script living under a directory named `wisphive`, or a
/// `my-wisphive-logger` wrapper — silently corrupting settings.json on
/// install/uninstall. We parse instead: strip an `env VAR=...` prefix, take
/// argv[0], and compare its basename to `wisphive-hook`.
fn is_wisphive_hook_command(cmd: &str) -> bool {
    // Strip an `env [flags] VAR=... [flags]` prefix: assignments and flags
    // precede the actual command word. `-u`/`--unset` consume a NAME operand.
    let mut rest = cmd.trim_start();
    let (first, after_env) = split_word(rest);
    if first == "env" {
        rest = after_env;
        loop {
            let (word, after) = split_word(rest);
            if word.is_empty() {
                return false;
            }
            if word.starts_with('\'') || word.starts_with('"') {
                // A quoted word is the command, never an assignment — a
                // hook path may itself contain '=' (e.g. .../build=debug/).
                break;
            } else if word == "-u" || word == "--unset" {
                let (_, after_name) = split_word(after);
                rest = after_name;
            } else if word.contains('=') || word.starts_with('-') {
                rest = after;
            } else {
                break;
            }
        }
    }
    let Some(argv0) = parse_argv0(rest) else {
        return false;
    };
    let base = std::path::Path::new(&argv0)
        .file_name()
        .and_then(|f| f.to_str())
        .unwrap_or(&argv0);
    base == "wisphive-hook"
}

/// Pop the next whitespace-delimited word; returns (word, rest-after-word).
fn split_word(s: &str) -> (&str, &str) {
    let end = s.find(char::is_whitespace).unwrap_or(s.len());
    (&s[..end], s[end..].trim_start())
}

/// Extract argv[0] with shell quoting semantics: concatenated unquoted /
/// single-quoted / double-quoted segments up to the first unquoted
/// whitespace, so the installer's own `'\''` apostrophe escape (see
/// `shell_quote_command`) round-trips. Unterminated quotes yield None.
fn parse_argv0(s: &str) -> Option<String> {
    let mut out = String::new();
    let mut it = s.chars().peekable();
    while let Some(&c) = it.peek() {
        match c {
            c if c.is_whitespace() => break,
            '\'' => {
                it.next();
                loop {
                    match it.next() {
                        Some('\'') => break,
                        Some(ch) => out.push(ch),
                        None => return None,
                    }
                }
            }
            '"' => {
                it.next();
                loop {
                    match it.next() {
                        Some('"') => break,
                        Some('\\') => out.push(it.next()?),
                        Some(ch) => out.push(ch),
                        None => return None,
                    }
                }
            }
            '\\' => {
                it.next();
                out.push(it.next()?);
            }
            _ => {
                out.push(c);
                it.next();
            }
        }
    }
    (!out.is_empty()).then_some(out)
}

fn update_existing_wisphive_hooks(
    rules: &mut [serde_json::Value],
    command: &str,
    timeout: Option<u64>,
) -> bool {
    let mut found = false;
    for rule in rules {
        if let Some(hooks_arr) = rule.get_mut("hooks").and_then(|v| v.as_array_mut()) {
            for hook in hooks_arr {
                let is_wisphive = hook
                    .get("command")
                    .and_then(|v| v.as_str())
                    .is_some_and(is_wisphive_hook_command);
                if is_wisphive {
                    found = true;
                    update_command_hook(hook, command, timeout);
                }
            }
        } else {
            let is_wisphive = rule
                .get("command")
                .and_then(|v| v.as_str())
                .is_some_and(is_wisphive_hook_command);
            if is_wisphive {
                found = true;
                update_command_hook(rule, command, timeout);
            }
        }
    }
    found
}

fn update_command_hook(hook: &mut serde_json::Value, command: &str, timeout: Option<u64>) {
    if let Some(obj) = hook.as_object_mut() {
        obj.insert(
            "command".into(),
            serde_json::Value::String(command.to_string()),
        );
        if let Some(timeout) = timeout {
            obj.insert("timeout".into(), serde_json::json!(timeout));
        }
    }
}

/// Remove Wisphive hook entries from the settings JSON.
///
/// Surgical on purpose (itr#359): only OUR command hooks are removed from a
/// rule's nested hooks array; the rule survives if any user hooks remain in
/// it. A rule is dropped only when nothing of the user's is left in it.
fn remove_hook_entries(settings: &mut serde_json::Value, hook_type: &str, _command: &str) {
    if let Some(hooks) = settings.get_mut("hooks")
        && let Some(entries) = hooks.get_mut(hook_type)
        && let Some(arr) = entries.as_array_mut()
    {
        arr.retain_mut(|rule| {
            if let Some(hooks_arr) = rule.get_mut("hooks").and_then(|v| v.as_array_mut()) {
                hooks_arr.retain(|hook| {
                    !hook
                        .get("command")
                        .and_then(|v| v.as_str())
                        .is_some_and(is_wisphive_hook_command)
                });
                !hooks_arr.is_empty()
            } else {
                !rule
                    .get("command")
                    .and_then(|v| v.as_str())
                    .is_some_and(is_wisphive_hook_command)
            }
        });
    }
}

/// Check if a hook rule entry contains a wisphive hook command.
/// Handles the nested format: {"matcher": "...", "hooks": [{"type": "command",
/// "command": "...wisphive-hook"}]} AND the flat legacy format — both branches
/// are checked (no early return) so a hybrid rule carrying a flat wisphive
/// command next to a user hooks array is still detected.
pub fn has_wisphive_hook(rule: &serde_json::Value) -> bool {
    let nested = rule
        .get("hooks")
        .and_then(|v| v.as_array())
        .is_some_and(|hooks_arr| {
            hooks_arr.iter().any(|hook| {
                hook.get("command")
                    .and_then(|v| v.as_str())
                    .is_some_and(is_wisphive_hook_command)
            })
        });
    nested
        || rule
            .get("command")
            .and_then(|v| v.as_str())
            .is_some_and(is_wisphive_hook_command)
}

/// True iff `project` has a Wisphive **PreToolUse** hook installed in
/// `.codex/hooks.json`, matched strictly on the `wisphive-hook` binary
/// (itr#359) — not a substring a user hook under a "wisphive" directory would
/// satisfy. PreToolUse specifically, because that is the event that actually
/// gates tool calls.
///
/// Fail-closed: a missing, unreadable, or malformed hooks file returns `false`.
/// This is the authoritative check a security gate should use before spawning a
/// Codex agent with hook-trust bypassed (itr#467) — the substring matcher in
/// `project_audit` is deliberately looser and must not be trusted for gating.
pub fn codex_pretooluse_hook_installed(project: &Path) -> bool {
    let hooks_path = project.join(".codex").join("hooks.json");
    let Ok(content) = std::fs::read_to_string(&hooks_path) else {
        return false;
    };
    let Ok(settings) = serde_json::from_str::<serde_json::Value>(&content) else {
        return false;
    };
    settings
        .get("hooks")
        .and_then(|hooks| hooks.get("PreToolUse"))
        .and_then(|rules| rules.as_array())
        .is_some_and(|arr| arr.iter().any(has_wisphive_hook))
}

/// Every hook command in `project`'s `.codex/hooks.json` — across all events
/// and rules, nested or flat — that is **not** the `wisphive-hook` binary.
///
/// A managed Codex spawn passes `--dangerously-bypass-hook-trust`, which
/// suppresses Codex's trust prompt for *every* hook in that file, not only
/// Wisphive's (itr#471). This lists the hooks that would run headlessly so the
/// caller can warn or refuse. Returns empty for a missing/malformed file (the
/// caller already fail-closes on the wisphive-hook-present check) and
/// de-duplicates repeated commands. Order is deterministic (by event name, then
/// position within the event) but not otherwise meaningful.
pub fn codex_foreign_hook_commands(project: &Path) -> Vec<String> {
    let hooks_path = project.join(".codex").join("hooks.json");
    let Ok(content) = std::fs::read_to_string(&hooks_path) else {
        return Vec::new();
    };
    let Ok(settings) = serde_json::from_str::<serde_json::Value>(&content) else {
        return Vec::new();
    };
    let Some(events) = settings.get("hooks").and_then(|h| h.as_object()) else {
        return Vec::new();
    };

    let mut foreign = Vec::new();
    for rules in events.values() {
        let Some(rules) = rules.as_array() else {
            continue;
        };
        for rule in rules {
            for cmd in rule_hook_commands(rule) {
                if !is_wisphive_hook_command(cmd) && !foreign.iter().any(|f| f == cmd) {
                    foreign.push(cmd.to_string());
                }
            }
        }
    }
    foreign
}

/// All hook command strings carried by one rule entry — both the nested
/// `{"hooks": [{"command": ...}]}` form and the flat legacy `{"command": ...}`
/// form (a hybrid rule may carry both).
fn rule_hook_commands(rule: &serde_json::Value) -> Vec<&str> {
    let mut cmds: Vec<&str> = rule
        .get("hooks")
        .and_then(|v| v.as_array())
        .map(|arr| {
            arr.iter()
                .filter_map(|hook| hook.get("command").and_then(|v| v.as_str()))
                .collect()
        })
        .unwrap_or_default();
    if let Some(flat) = rule.get("command").and_then(|v| v.as_str()) {
        cmds.push(flat);
    }
    cmds
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use std::fs;

    // Shim wrappers mirroring the old CLI signatures so the migrated
    // filesystem integration tests below compile unchanged. The library API
    // splits install/uninstall per-agent; these recombine them.
    /// Deterministic hook timeout for tests: the value a default-config
    /// install writes (3600 daemon timeout + margin), independent of the
    /// developer machine's real ~/.wisphive/config.json.
    const TEST_INSTALL_TIMEOUT_SECS: u64 = 3_700;

    fn install(project: Option<PathBuf>, _all: bool) -> Result<()> {
        let p = project.unwrap();
        write_prepared_install(prepare_claude_install(&p, TEST_INSTALL_TIMEOUT_SECS)?)?;
        write_prepared_install(prepare_codex_install(&p, TEST_INSTALL_TIMEOUT_SECS)?)?;
        Ok(())
    }

    fn uninstall(project: Option<PathBuf>, _all: bool) -> Result<()> {
        let p = project.unwrap();
        uninstall_claude(&p)?;
        uninstall_codex(&p)?;
        Ok(())
    }

    fn temp_project() -> tempfile::TempDir {
        tempfile::tempdir().unwrap()
    }

    fn write_settings(project: &std::path::Path, settings: &serde_json::Value) {
        let dir = project.join(".claude");
        fs::create_dir_all(&dir).unwrap();
        fs::write(
            dir.join("settings.json"),
            serde_json::to_string_pretty(settings).unwrap(),
        )
        .unwrap();
    }

    fn read_settings(project: &std::path::Path) -> serde_json::Value {
        let content = fs::read_to_string(project.join(".claude").join("settings.json")).unwrap();
        serde_json::from_str(&content).unwrap()
    }

    fn write_codex_hooks(project: &std::path::Path, settings: &serde_json::Value) {
        let dir = project.join(".codex");
        fs::create_dir_all(&dir).unwrap();
        fs::write(
            dir.join("hooks.json"),
            serde_json::to_string_pretty(settings).unwrap(),
        )
        .unwrap();
    }

    fn read_codex_hooks(project: &std::path::Path) -> serde_json::Value {
        let content = fs::read_to_string(project.join(".codex").join("hooks.json")).unwrap();
        serde_json::from_str(&content).unwrap()
    }

    /// Build a Claude Code-format hook rule.
    fn cc_rule(command: &str) -> serde_json::Value {
        json!({"matcher": "", "hooks": [{"type": "command", "command": command}]})
    }

    fn assert_install_hooks_rejects_hooks_value(hooks: serde_json::Value) {
        let tmp = temp_project();
        let original = json!({"hooks": hooks, "theme": "dark"});
        write_settings(tmp.path(), &original);

        let error = install_hooks(tmp.path())
            .expect_err("malformed shared hook settings must fail without panicking");
        let message = error.to_string();
        assert!(
            message.contains("`hooks` must be a JSON object"),
            "unexpected error: {message}"
        );
        assert!(
            message.contains("replace it with `\"hooks\": {}`"),
            "error should explain how to repair the config: {message}"
        );
        assert_eq!(
            read_settings(tmp.path()),
            original,
            "a rejected config must remain unchanged"
        );
        assert!(
            !tmp.path().join(".codex/hooks.json").exists(),
            "a rejected Claude config must not partially install Codex hooks"
        );
    }

    // ══ Shared install validation ══

    #[test]
    fn shared_install_rejects_array_hooks_value() {
        assert_install_hooks_rejects_hooks_value(json!([]));
    }

    #[test]
    fn shared_install_rejects_string_hooks_value() {
        assert_install_hooks_rejects_hooks_value(json!("not hooks"));
    }

    #[test]
    fn shared_install_rejects_number_hooks_value() {
        assert_install_hooks_rejects_hooks_value(json!(42));
    }

    #[test]
    fn shared_install_rejects_boolean_hooks_value() {
        assert_install_hooks_rejects_hooks_value(json!(true));
    }

    #[test]
    fn shared_install_rejects_non_object_document_root_without_writes() {
        let tmp = temp_project();
        write_settings(tmp.path(), &json!(["not", "an", "object"]));
        let settings_path = tmp.path().join(".claude/settings.json");
        let original = fs::read(&settings_path).unwrap();

        let error = install_hooks(tmp.path())
            .expect_err("a non-object settings document must fail without panicking");
        assert!(
            error
                .to_string()
                .contains("must contain a JSON object at the document root")
        );
        assert_eq!(
            fs::read(&settings_path).unwrap(),
            original,
            "a rejected document must remain byte-for-byte unchanged"
        );
        assert!(
            !tmp.path().join(".codex/hooks.json").exists(),
            "a rejected Claude document must not partially install Codex hooks"
        );
    }

    #[test]
    fn shared_install_validates_codex_before_mutating_claude() {
        let tmp = temp_project();
        let claude_original = json!({"theme": "dark"});
        let codex_original = json!({"hooks": false, "custom": "keep"});
        write_settings(tmp.path(), &claude_original);
        write_codex_hooks(tmp.path(), &codex_original);

        let error = install_hooks(tmp.path())
            .expect_err("malformed Codex hooks must reject the combined install");
        assert!(error.to_string().contains("`hooks` must be a JSON object"));
        assert_eq!(read_settings(tmp.path()), claude_original);
        assert_eq!(read_codex_hooks(tmp.path()), codex_original);
    }

    #[cfg(unix)]
    #[test]
    fn install_atomically_replaces_settings_symlink_without_following_target() {
        use std::os::unix::fs::{PermissionsExt, symlink};

        let tmp = temp_project();
        let settings_dir = tmp.path().join(".claude");
        fs::create_dir_all(&settings_dir).unwrap();
        let sentinel = tmp.path().join("sentinel.json");
        fs::write(&sentinel, "{}").unwrap();
        let settings_path = settings_dir.join("settings.json");
        symlink(&sentinel, &settings_path).unwrap();

        write_prepared_install(
            prepare_claude_install(tmp.path(), TEST_INSTALL_TIMEOUT_SECS).unwrap(),
        )
        .unwrap();

        assert_eq!(
            fs::read_to_string(&sentinel).unwrap(),
            "{}",
            "atomic rename must replace the settings symlink, not overwrite its target"
        );
        let metadata = fs::symlink_metadata(&settings_path).unwrap();
        assert!(metadata.file_type().is_file());
        assert!(!metadata.file_type().is_symlink());
        assert_eq!(metadata.permissions().mode() & 0o777, 0o600);
        assert_eq!(
            read_settings(tmp.path())["hooks"]["PreToolUse"][0]["hooks"][0]["timeout"],
            TEST_INSTALL_TIMEOUT_SECS
        );
    }

    // ══ codex_pretooluse_hook_installed — security gate (itr#467) ══

    #[test]
    fn codex_gate_true_after_real_install() {
        let tmp = temp_project();
        install_codex(tmp.path()).unwrap();
        assert!(codex_pretooluse_hook_installed(tmp.path()));
    }

    #[test]
    fn codex_gate_false_for_substring_only_hook() {
        // itr#467 P2-A: a PreToolUse hook whose command merely CONTAINS the
        // substring "wisphive" (e.g. a tool under a wisphive-named directory)
        // but is not the wisphive-hook binary must NOT satisfy the gate — else
        // Codex would spawn with hook-trust bypassed and no real gating hook.
        let tmp = temp_project();
        write_codex_hooks(
            tmp.path(),
            &json!({"hooks": {"PreToolUse": [cc_rule("/opt/wisphive-tools/lint.sh")]}}),
        );
        assert!(!codex_pretooluse_hook_installed(tmp.path()));
    }

    #[test]
    fn codex_gate_false_when_only_on_non_gating_event() {
        // A wisphive hook on a telemetry-only event does not gate tool calls.
        let tmp = temp_project();
        write_codex_hooks(
            tmp.path(),
            &json!({"hooks": {"PostToolUse": [cc_rule("wisphive-hook")]}}),
        );
        assert!(!codex_pretooluse_hook_installed(tmp.path()));
    }

    #[test]
    fn codex_gate_fails_closed_when_missing_or_malformed() {
        let tmp = temp_project();
        // No .codex/hooks.json at all.
        assert!(!codex_pretooluse_hook_installed(tmp.path()));
        // Malformed JSON must fail closed, not pass.
        fs::create_dir_all(tmp.path().join(".codex")).unwrap();
        fs::write(tmp.path().join(".codex").join("hooks.json"), "{not json").unwrap();
        assert!(!codex_pretooluse_hook_installed(tmp.path()));
    }

    // ══ codex_foreign_hook_commands — trust-bypass blast radius (itr#471) ══

    #[test]
    fn foreign_hooks_empty_for_wisphive_only_install() {
        let tmp = temp_project();
        install_codex(tmp.path()).unwrap();
        assert!(codex_foreign_hook_commands(tmp.path()).is_empty());
    }

    #[test]
    fn foreign_hooks_lists_non_wisphive_across_all_events() {
        let tmp = temp_project();
        write_codex_hooks(
            tmp.path(),
            &json!({"hooks": {
                "PreToolUse": [cc_rule("/x/wisphive-hook"), cc_rule("/usr/bin/pre.sh")],
                "PostToolUse": [cc_rule("/usr/bin/post.sh")],
            }}),
        );
        let mut foreign = codex_foreign_hook_commands(tmp.path());
        foreign.sort();
        assert_eq!(foreign, vec!["/usr/bin/post.sh", "/usr/bin/pre.sh"]);
    }

    #[test]
    fn foreign_hooks_dedupes_repeated_commands() {
        let tmp = temp_project();
        write_codex_hooks(
            tmp.path(),
            &json!({"hooks": {
                "PreToolUse": [cc_rule("/usr/bin/dup.sh")],
                "Stop": [cc_rule("/usr/bin/dup.sh")],
            }}),
        );
        assert_eq!(
            codex_foreign_hook_commands(tmp.path()),
            vec!["/usr/bin/dup.sh"]
        );
    }

    #[test]
    fn foreign_hooks_empty_when_missing_or_malformed() {
        let tmp = temp_project();
        assert!(codex_foreign_hook_commands(tmp.path()).is_empty());
        fs::create_dir_all(tmp.path().join(".codex")).unwrap();
        fs::write(tmp.path().join(".codex").join("hooks.json"), "{bad").unwrap();
        assert!(codex_foreign_hook_commands(tmp.path()).is_empty());
    }

    // ══ add_hook_entry (writes correct nested format) ══

    #[test]
    fn add_to_empty_creates_nested_format() {
        let mut s = json!({"hooks": {}});
        add_hook_entry(&mut s, "PreToolUse", "wisphive-hook").unwrap();
        let rule = &s["hooks"]["PreToolUse"][0];
        assert_eq!(rule["matcher"], "");
        assert_eq!(rule["hooks"][0]["type"], "command");
        assert_eq!(rule["hooks"][0]["command"], "wisphive-hook");
    }

    #[test]
    fn add_preserves_existing_rules() {
        let mut s = json!({"hooks": {"PreToolUse": [cc_rule("other-hook")]}});
        add_hook_entry(&mut s, "PreToolUse", "wisphive-hook").unwrap();
        let arr = s["hooks"]["PreToolUse"].as_array().unwrap();
        assert_eq!(arr.len(), 2);
        assert_eq!(arr[0]["hooks"][0]["command"], "other-hook");
        assert_eq!(arr[1]["hooks"][0]["command"], "wisphive-hook");
    }

    #[test]
    fn add_is_idempotent() {
        let mut s = json!({"hooks": {}});
        add_hook_entry(&mut s, "PreToolUse", "wisphive-hook").unwrap();
        add_hook_entry(&mut s, "PreToolUse", "wisphive-hook").unwrap();
        add_hook_entry(&mut s, "PreToolUse", "/usr/bin/wisphive-hook").unwrap();
        assert_eq!(s["hooks"]["PreToolUse"].as_array().unwrap().len(), 1);
    }

    #[test]
    fn add_different_hook_types_independent() {
        let mut s = json!({"hooks": {}});
        add_hook_entry(&mut s, "PreToolUse", "wisphive-hook").unwrap();
        add_hook_entry(&mut s, "PostToolUse", "wisphive-hook").unwrap();
        assert_eq!(s["hooks"]["PreToolUse"].as_array().unwrap().len(), 1);
        assert_eq!(s["hooks"]["PostToolUse"].as_array().unwrap().len(), 1);
    }

    #[test]
    fn add_with_full_path() {
        let mut s = json!({"hooks": {}});
        add_hook_entry(&mut s, "PreToolUse", "/home/user/.cargo/bin/wisphive-hook").unwrap();
        assert_eq!(
            s["hooks"]["PreToolUse"][0]["hooks"][0]["command"],
            "/home/user/.cargo/bin/wisphive-hook"
        );
    }

    #[test]
    fn add_with_timeout_sets_command_timeout() {
        let mut s = json!({"hooks": {}});
        add_hook_entry_with_timeout(&mut s, "PreToolUse", "wisphive-hook", Some(42)).unwrap();
        assert_eq!(s["hooks"]["PreToolUse"][0]["hooks"][0]["timeout"], 42);
    }

    #[test]
    fn codex_hook_command_sets_agent_type_env() {
        assert_eq!(
            codex_hook_command("/usr/local/bin/wisphive-hook"),
            "env WISPHIVE_AGENT_TYPE=codex /usr/local/bin/wisphive-hook"
        );
    }

    #[test]
    fn codex_hook_command_quotes_spaces() {
        assert_eq!(
            codex_hook_command("/Applications/Wisphive Tools/wisphive-hook"),
            "env WISPHIVE_AGENT_TYPE=codex '/Applications/Wisphive Tools/wisphive-hook'"
        );
    }

    #[test]
    fn add_updates_existing_wisphive_hook_with_timeout() {
        let mut s = json!({"hooks": {"PreToolUse": [cc_rule("wisphive-hook")]}});
        add_hook_entry_with_timeout(
            &mut s,
            "PreToolUse",
            "env WISPHIVE_AGENT_TYPE=codex wisphive-hook",
            Some(TEST_INSTALL_TIMEOUT_SECS),
        )
        .unwrap();
        let arr = s["hooks"]["PreToolUse"].as_array().unwrap();
        assert_eq!(arr.len(), 1);
        assert_eq!(
            arr[0]["hooks"][0]["command"],
            "env WISPHIVE_AGENT_TYPE=codex wisphive-hook"
        );
        assert_eq!(arr[0]["hooks"][0]["timeout"], TEST_INSTALL_TIMEOUT_SECS);
    }

    // ══ remove_hook_entries (handles nested + legacy) ══

    #[test]
    fn remove_nested_format() {
        let mut s = json!({"hooks": {"PreToolUse": [
            cc_rule("other"), cc_rule("wisphive-hook"), cc_rule("another")
        ]}});
        remove_hook_entries(&mut s, "PreToolUse", "");
        let arr = s["hooks"]["PreToolUse"].as_array().unwrap();
        assert_eq!(arr.len(), 2);
        assert_eq!(arr[0]["hooks"][0]["command"], "other");
        assert_eq!(arr[1]["hooks"][0]["command"], "another");
    }

    #[test]
    fn remove_legacy_flat_format() {
        let mut s = json!({"hooks": {"PreToolUse": [
            {"command": "other"}, {"command": "wisphive-hook"}, {"command": "another"}
        ]}});
        remove_hook_entries(&mut s, "PreToolUse", "");
        let arr = s["hooks"]["PreToolUse"].as_array().unwrap();
        assert_eq!(arr.len(), 2);
    }

    #[test]
    fn remove_all_path_variants() {
        let mut s = json!({"hooks": {"PreToolUse": [
            cc_rule("wisphive-hook"),
            cc_rule("/usr/local/bin/wisphive-hook"),
            cc_rule("/home/u/.cargo/bin/wisphive-hook"),
            cc_rule("other-tool")
        ]}});
        remove_hook_entries(&mut s, "PreToolUse", "");
        let arr = s["hooks"]["PreToolUse"].as_array().unwrap();
        assert_eq!(arr.len(), 1);
        assert_eq!(arr[0]["hooks"][0]["command"], "other-tool");
    }

    #[test]
    fn remove_missing_hooks_section() {
        let mut s = json!({"other": "data"});
        remove_hook_entries(&mut s, "PreToolUse", "");
        assert_eq!(s, json!({"other": "data"}));
    }

    #[test]
    fn remove_missing_hook_type() {
        let mut s = json!({"hooks": {}});
        remove_hook_entries(&mut s, "PreToolUse", "");
        assert_eq!(s, json!({"hooks": {}}));
    }

    #[test]
    fn remove_empty_array() {
        let mut s = json!({"hooks": {"PreToolUse": []}});
        remove_hook_entries(&mut s, "PreToolUse", "");
        assert_eq!(s["hooks"]["PreToolUse"].as_array().unwrap().len(), 0);
    }

    #[test]
    fn remove_noop_when_no_wisphive() {
        let mut s = json!({"hooks": {"PreToolUse": [cc_rule("a"), cc_rule("b")]}});
        let orig = s.clone();
        remove_hook_entries(&mut s, "PreToolUse", "");
        assert_eq!(s, orig);
    }

    #[test]
    fn remove_keeps_entries_without_command() {
        let mut s = json!({"hooks": {"PreToolUse": [
            cc_rule("wisphive-hook"), {"not_command": "x"}, cc_rule("other")
        ]}});
        remove_hook_entries(&mut s, "PreToolUse", "");
        assert_eq!(s["hooks"]["PreToolUse"].as_array().unwrap().len(), 2);
    }

    #[test]
    fn remove_non_array_graceful() {
        let mut s = json!({"hooks": {"PreToolUse": "not-an-array"}});
        remove_hook_entries(&mut s, "PreToolUse", "");
        assert_eq!(s["hooks"]["PreToolUse"], "not-an-array");
    }

    // ══ has_wisphive_hook detection ══

    #[test]
    fn detect_nested() {
        assert!(has_wisphive_hook(&cc_rule("wisphive-hook")));
    }
    #[test]
    fn detect_legacy() {
        assert!(has_wisphive_hook(&json!({"command": "wisphive-hook"})));
    }
    #[test]
    fn detect_false_other() {
        assert!(!has_wisphive_hook(&cc_rule("other-tool")));
    }
    #[test]
    fn detect_false_empty() {
        assert!(!has_wisphive_hook(&json!({})));
    }
    #[test]
    fn detect_path_variants() {
        assert!(has_wisphive_hook(&cc_rule("/usr/local/bin/wisphive-hook")));
        assert!(has_wisphive_hook(&cc_rule("wisphive-hook --verbose")));
    }

    // ══ install / uninstall filesystem integration ══

    #[test]
    fn matcher_recognizes_only_the_wisphive_hook_binary() {
        // itr#359: positives — every form the installer writes.
        for ours in [
            "wisphive-hook",
            "/Users/x/.cargo/bin/wisphive-hook",
            "env WISPHIVE_AGENT_TYPE=codex /Users/x/.cargo/bin/wisphive-hook",
            "env WISPHIVE_AGENT_TYPE=codex '/Users/x/my tools/wisphive-hook'",
            // installer's own '\'' apostrophe escape must round-trip
            r"env WISPHIVE_AGENT_TYPE=codex '/Users/x/Joe'\''s tools/wisphive-hook'",
            "\"/Users/x/my tools/wisphive-hook\"",
            // hand-edited but shell-valid variants
            "env\tWISPHIVE_AGENT_TYPE=codex /usr/local/bin/wisphive-hook",
            "env -u FOO WISPHIVE_AGENT_TYPE=codex /usr/local/bin/wisphive-hook",
            r"/Users/x/my\ tools/wisphive-hook",
        ] {
            assert!(is_wisphive_hook_command(ours), "should match: {ours}");
        }
        // Negatives — user hooks that merely mention the word.
        for theirs in [
            "/Users/x/AI_Projects/wisphive/scripts/precommit.sh",
            "my-wisphive-logger --verbose",
            "python3 /opt/wisphive-tools/check.py",
            "wisphive doctor",
            "'/unterminated/quote/wisphive-hook",
            "env FOO=bar",
            "",
        ] {
            assert!(
                !is_wisphive_hook_command(theirs),
                "must NOT match: {theirs}"
            );
        }
    }

    #[test]
    fn matcher_round_trips_installer_quoting() {
        // Whatever shell_quote_command emits for the hook path — plain,
        // spaced, or apostrophed — both install forms must be recognized.
        for path in [
            "/Users/x/.cargo/bin/wisphive-hook",
            "/Users/x/my tools/wisphive-hook",
            "/Users/x/Joe's tools/wisphive-hook",
            "/Users/x/build=debug/wisphive-hook",
        ] {
            let claude_form = shell_quote_command(path);
            let codex_form = codex_hook_command(path);
            assert!(
                is_wisphive_hook_command(&claude_form),
                "claude form should match: {claude_form}"
            );
            assert!(
                is_wisphive_hook_command(&codex_form),
                "codex form should match: {codex_form}"
            );
        }
    }

    #[test]
    fn has_wisphive_hook_checks_both_nested_and_flat() {
        // Hybrid rule: flat wisphive command next to a user-only hooks array
        // must still be detected (no early return on the nested branch).
        let hybrid = json!({
            "command": "/usr/local/bin/wisphive-hook",
            "hooks": [{"type": "command", "command": "user-lint.sh"}]
        });
        assert!(has_wisphive_hook(&hybrid));
        let user_only = json!({
            "matcher": "*",
            "hooks": [{"type": "command", "command": "user-lint.sh"}]
        });
        assert!(!has_wisphive_hook(&user_only));
    }

    #[test]
    fn uninstall_keeps_user_hook_sharing_a_rule_with_ours() {
        // itr#359 destructive class: a user hook grouped into the SAME rule's
        // hooks array as wisphive-hook must survive uninstall.
        let mut s = json!({
            "hooks": {
                "PreToolUse": [{
                    "matcher": "*",
                    "hooks": [
                        {"type": "command", "command": "/usr/local/bin/wisphive-hook"},
                        {"type": "command", "command": "/Users/x/bin/my-lint.sh"}
                    ]
                }]
            }
        });
        remove_hook_entries(&mut s, "PreToolUse", "unused");
        let rules = s["hooks"]["PreToolUse"].as_array().unwrap();
        assert_eq!(
            rules.len(),
            1,
            "rule with a surviving user hook was dropped"
        );
        let cmds: Vec<&str> = rules[0]["hooks"]
            .as_array()
            .unwrap()
            .iter()
            .filter_map(|h| h["command"].as_str())
            .collect();
        assert_eq!(cmds, vec!["/Users/x/bin/my-lint.sh"]);
    }

    #[test]
    fn install_and_uninstall_leave_user_wisphive_named_hooks_alone() {
        // itr#359 acceptance: a user hook whose command contains "wisphive"
        // but is not the wisphive-hook binary survives install AND uninstall.
        let tmp = temp_project();
        let p = tmp.path().to_path_buf();
        let user_cmd = "/Users/x/AI_Projects/wisphive/scripts/lint.sh";
        write_settings(
            &p,
            &json!({
                "hooks": {
                    "PreToolUse": [
                        {"matcher": "*", "hooks": [{"type": "command", "command": user_cmd}]}
                    ]
                }
            }),
        );

        install(Some(p.clone()), false).unwrap();
        let s = read_settings(&p);
        let cmds: Vec<String> = s["hooks"]["PreToolUse"]
            .as_array()
            .unwrap()
            .iter()
            .flat_map(|r| r["hooks"].as_array().cloned().unwrap_or_default())
            .filter_map(|h| h["command"].as_str().map(String::from))
            .collect();
        assert!(
            cmds.iter().any(|c| c == user_cmd),
            "user hook rewritten by install: {cmds:?}"
        );
        assert!(
            cmds.iter().any(|c| is_wisphive_hook_command(c)),
            "wisphive hook not installed: {cmds:?}"
        );

        uninstall(Some(p.clone()), false).unwrap();
        let s = read_settings(&p);
        let cmds: Vec<String> = s["hooks"]["PreToolUse"]
            .as_array()
            .unwrap()
            .iter()
            .flat_map(|r| r["hooks"].as_array().cloned().unwrap_or_default())
            .filter_map(|h| h["command"].as_str().map(String::from))
            .collect();
        assert!(
            cmds.iter().any(|c| c == user_cmd),
            "user hook deleted by uninstall: {cmds:?}"
        );
        assert!(
            !cmds.iter().any(|c| is_wisphive_hook_command(c)),
            "wisphive hook not removed: {cmds:?}"
        );
    }

    #[test]
    fn install_creates_from_scratch() {
        let tmp = temp_project();
        let p = tmp.path().to_path_buf();
        install(Some(p.clone()), false).unwrap();
        let s = read_settings(&p);
        let rule = &s["hooks"]["PreToolUse"][0];
        assert!(
            rule["hooks"][0]["command"]
                .as_str()
                .unwrap()
                .contains("wisphive")
        );
        assert_eq!(rule["hooks"][0]["type"], "command");
    }

    // ══ itr#510: installed hook timeout must exceed the daemon approval timeout ══

    #[test]
    fn installed_hook_timeout_exceeds_daemon_timeout_at_default_and_max() {
        // Default daemon timeout (3600) → 3700, the value Codex installs
        // already carried before itr#510.
        assert_eq!(installed_hook_timeout_secs(3600), 3_700);
        // Maximum configurable daemon timeout (86400) must still be exceeded.
        assert_eq!(installed_hook_timeout_secs(86_400), 86_500);
        assert!(installed_hook_timeout_secs(86_400) > crate::config::HOOK_TIMEOUT_MAX_SECS);
        // Saturating: absurd inputs can't wrap into a tiny timeout.
        assert_eq!(installed_hook_timeout_secs(u64::MAX), u64::MAX);
    }

    #[test]
    fn claude_install_writes_daemon_aligned_timeout_on_every_event() {
        let tmp = temp_project();
        let p = tmp.path().to_path_buf();
        install(Some(p.clone()), false).unwrap();
        let s = read_settings(&p);
        for event in CLAUDE_HOOK_EVENTS {
            assert_eq!(
                s["hooks"][event][0]["hooks"][0]["timeout"], TEST_INSTALL_TIMEOUT_SECS,
                "{event} entry must carry the daemon-aligned timeout"
            );
        }
    }

    /// A hook entry written by a pre-itr#510 wisphive version has no `timeout`
    /// field, so Claude Code's implicit 600 s cancels the blocking hook long
    /// before the daemon's 3600 s approval timeout. Reinstalling must upgrade
    /// the legacy entry in place — same rule, no duplicate — with the timeout.
    #[test]
    fn reinstall_upgrades_legacy_claude_entry_without_timeout() {
        let tmp = temp_project();
        let p = tmp.path().to_path_buf();
        // Legacy install: wisphive PreToolUse entry with no timeout field.
        write_settings(
            tmp.path(),
            &json!({"hooks": {"PreToolUse": [cc_rule("/usr/local/bin/wisphive-hook")]}}),
        );

        install(Some(p.clone()), false).unwrap();

        let s = read_settings(&p);
        let rules = s["hooks"]["PreToolUse"].as_array().unwrap();
        assert_eq!(rules.len(), 1, "reinstall must not duplicate the entry");
        let hook = &rules[0]["hooks"][0];
        assert_eq!(hook["timeout"], TEST_INSTALL_TIMEOUT_SECS);
        assert!(is_wisphive_hook_command(hook["command"].as_str().unwrap()));
    }

    /// The public entry points must derive the written timeout from the same
    /// effective daemon config the daemon itself loads (default + clamp).
    #[test]
    fn public_install_uses_effective_daemon_timeout() {
        let tmp = temp_project();
        install_claude(tmp.path()).unwrap();
        let s = read_settings(tmp.path());
        let expected =
            installed_hook_timeout_secs(crate::config::effective_hook_timeout_secs_default_home());
        assert_eq!(s["hooks"]["PreToolUse"][0]["hooks"][0]["timeout"], expected);
        assert!(
            expected > crate::config::effective_hook_timeout_secs_default_home(),
            "installed timeout must strictly exceed the daemon approval timeout"
        );
    }

    /// itr#510 end-to-end at the MAXIMUM configurable daemon timeout: a real
    /// `config.json` with `hook_timeout_secs: 86400` in a fixture Wisphive
    /// home drives the actual public install path (config load → clamp →
    /// margin → prepare → atomic write) — not the arithmetic helper in
    /// isolation — and every resulting Claude hook entry, including a legacy
    /// pre-itr#510 timeout-less entry being upgraded in place, ends up with
    /// the derived 86_500 (86400 + margin).
    #[test]
    fn install_with_max_configured_timeout_end_to_end() {
        let home = tempfile::tempdir().unwrap();
        crate::config::write_config_atomic(
            &home.path().join("config.json"),
            r#"{"hook_timeout_secs": 86400}"#,
        )
        .unwrap();

        let tmp = temp_project();
        // Legacy install (pre-itr#510): a Wisphive PreToolUse entry with no
        // timeout field, so Claude Code's implicit 600 s would apply.
        write_settings(
            tmp.path(),
            &json!({"hooks": {"PreToolUse": [cc_rule("/usr/local/bin/wisphive-hook")]}}),
        );

        install_claude_in_home(tmp.path(), home.path()).unwrap();

        let expected = installed_hook_timeout_secs(crate::config::HOOK_TIMEOUT_MAX_SECS);
        assert_eq!(expected, 86_500, "max daemon timeout + margin");
        let s = read_settings(tmp.path());
        for event in CLAUDE_HOOK_EVENTS {
            let rules = s["hooks"][event].as_array().unwrap_or_else(|| {
                panic!("{event} must have an installed rule array");
            });
            assert_eq!(
                rules.len(),
                1,
                "{event}: reinstall must not duplicate entries"
            );
            for hook in rules[0]["hooks"].as_array().unwrap() {
                assert!(
                    is_wisphive_hook_command(hook["command"].as_str().unwrap()),
                    "{event} entry must be the Wisphive hook"
                );
                assert_eq!(
                    hook["timeout"], 86_500,
                    "{event} entry must carry the config-derived max timeout"
                );
            }
        }
        // The upgraded legacy entry specifically: same single rule slot
        // (upgraded in place, not appended), still the Wisphive hook, and the
        // previously-absent timeout is now the config-derived value.
        let pretool = &s["hooks"]["PreToolUse"][0]["hooks"][0];
        assert!(is_wisphive_hook_command(
            pretool["command"].as_str().unwrap()
        ));
        assert_eq!(pretool["timeout"], 86_500);
    }

    #[test]
    fn install_creates_codex_hooks_with_timeout() {
        let tmp = temp_project();
        let p = tmp.path().to_path_buf();
        install(Some(p.clone()), false).unwrap();
        let s = read_codex_hooks(&p);
        let rule = &s["hooks"]["PreToolUse"][0];
        assert!(
            rule["hooks"][0]["command"]
                .as_str()
                .unwrap()
                .contains("wisphive")
        );
        assert_eq!(rule["hooks"][0]["type"], "command");
        assert_eq!(rule["hooks"][0]["timeout"], TEST_INSTALL_TIMEOUT_SECS);
        assert!(
            rule["hooks"][0]["command"]
                .as_str()
                .unwrap()
                .contains("WISPHIVE_AGENT_TYPE=codex")
        );
        assert!(s.get("permissions").is_none());
    }

    #[test]
    fn install_preserves_existing_settings() {
        let tmp = temp_project();
        let p = tmp.path().to_path_buf();
        write_settings(&p, &json!({"mcpServers": {"s": {"url": "http://x"}}}));
        install(Some(p.clone()), false).unwrap();
        let s = read_settings(&p);
        assert_eq!(s["mcpServers"]["s"]["url"], "http://x");
        assert!(s["hooks"]["PreToolUse"].is_array());
    }

    #[test]
    fn install_preserves_existing_hooks() {
        let tmp = temp_project();
        let p = tmp.path().to_path_buf();
        write_settings(
            &p,
            &json!({"hooks": {
                "PreToolUse": [cc_rule("linter")],
                "PostToolUse": [cc_rule("logger")]
            }}),
        );
        install(Some(p.clone()), false).unwrap();
        let s = read_settings(&p);
        assert_eq!(s["hooks"]["PreToolUse"].as_array().unwrap().len(), 2);
        assert_eq!(s["hooks"]["PostToolUse"].as_array().unwrap().len(), 2);
    }

    #[test]
    fn install_preserves_existing_codex_hooks() {
        let tmp = temp_project();
        let p = tmp.path().to_path_buf();
        write_codex_hooks(&p, &json!({"hooks": {"PreToolUse": [cc_rule("policy")]}}));
        install(Some(p.clone()), false).unwrap();
        let s = read_codex_hooks(&p);
        assert_eq!(s["hooks"]["PreToolUse"].as_array().unwrap().len(), 2);
        assert_eq!(s["hooks"]["PreToolUse"][0]["hooks"][0]["command"], "policy");
    }

    #[test]
    fn install_idempotent() {
        let tmp = temp_project();
        let p = tmp.path().to_path_buf();
        install(Some(p.clone()), false).unwrap();
        install(Some(p.clone()), false).unwrap();
        install(Some(p.clone()), false).unwrap();
        assert_eq!(
            read_settings(&p)["hooks"]["PreToolUse"]
                .as_array()
                .unwrap()
                .len(),
            1
        );
    }

    #[test]
    fn uninstall_removes_wisphive_only() {
        let tmp = temp_project();
        let p = tmp.path().to_path_buf();
        write_settings(
            &p,
            &json!({"hooks": {
                "PreToolUse": [cc_rule("linter"), cc_rule("wisphive-hook")],
                "PostToolUse": [cc_rule("wisphive-hook"), cc_rule("logger")]
            }, "mcpServers": {"keep": "this"}}),
        );
        uninstall(Some(p.clone()), false).unwrap();
        let s = read_settings(&p);
        assert_eq!(s["hooks"]["PreToolUse"].as_array().unwrap().len(), 1);
        assert_eq!(s["hooks"]["PostToolUse"].as_array().unwrap().len(), 1);
        assert_eq!(s["mcpServers"]["keep"], "this");
    }

    #[test]
    fn uninstall_removes_codex_wisphive_only() {
        let tmp = temp_project();
        let p = tmp.path().to_path_buf();
        write_codex_hooks(
            &p,
            &json!({"hooks": {
                "PreToolUse": [cc_rule("policy"), cc_rule("wisphive-hook")],
                "PostToolUse": [cc_rule("wisphive-hook"), cc_rule("logger")]
            }}),
        );
        uninstall(Some(p.clone()), false).unwrap();
        let s = read_codex_hooks(&p);
        assert_eq!(s["hooks"]["PreToolUse"].as_array().unwrap().len(), 1);
        assert_eq!(s["hooks"]["PostToolUse"].as_array().unwrap().len(), 1);
        assert_eq!(s["hooks"]["PreToolUse"][0]["hooks"][0]["command"], "policy");
        assert_eq!(
            s["hooks"]["PostToolUse"][0]["hooks"][0]["command"],
            "logger"
        );
    }

    #[test]
    fn uninstall_ok_no_settings() {
        let tmp = temp_project();
        assert!(uninstall(Some(tmp.path().to_path_buf()), false).is_ok());
    }

    #[test]
    fn install_then_uninstall_round_trip() {
        let tmp = temp_project();
        let p = tmp.path().to_path_buf();
        write_settings(
            &p,
            &json!({"hooks": {"PreToolUse": [cc_rule("existing")]}, "other": "data"}),
        );
        install(Some(p.clone()), false).unwrap();
        assert_eq!(
            read_settings(&p)["hooks"]["PreToolUse"]
                .as_array()
                .unwrap()
                .len(),
            2
        );
        uninstall(Some(p.clone()), false).unwrap();
        let s = read_settings(&p);
        assert_eq!(s["hooks"]["PreToolUse"].as_array().unwrap().len(), 1);
        assert_eq!(
            s["hooks"]["PreToolUse"][0]["hooks"][0]["command"],
            "existing"
        );
        assert_eq!(s["other"], "data");
    }

    #[test]
    fn install_no_hooks_key() {
        let tmp = temp_project();
        let p = tmp.path().to_path_buf();
        write_settings(&p, &json!({"someKey": "someValue"}));
        install(Some(p.clone()), false).unwrap();
        let s = read_settings(&p);
        assert_eq!(s["someKey"], "someValue");
        assert!(s["hooks"]["PreToolUse"].is_array());
    }

    #[test]
    fn uninstall_empty_hooks() {
        let tmp = temp_project();
        let p = tmp.path().to_path_buf();
        write_settings(&p, &json!({"hooks": {}}));
        assert!(uninstall(Some(p), false).is_ok());
    }

    // ══ Permissions management ══

    #[test]
    fn add_permissions_to_empty_settings() {
        let mut s = json!({});
        add_wisphive_permissions(&mut s);
        let allow = s["permissions"]["allow"].as_array().unwrap();
        assert!(allow.iter().any(|v| v == "Bash(*)"));
        assert!(allow.iter().any(|v| v == "Edit(*)"));
        assert!(allow.iter().any(|v| v == "Write(*)"));
        assert!(allow.iter().any(|v| v == "NotebookEdit(*)"));
        assert_eq!(allow.len(), WISPHIVE_PERMISSIONS.len());
    }

    #[test]
    fn add_permissions_preserves_existing() {
        let mut s = json!({"permissions": {"allow": ["mcp__foo(*)"]}});
        add_wisphive_permissions(&mut s);
        let allow = s["permissions"]["allow"].as_array().unwrap();
        assert!(allow.iter().any(|v| v == "mcp__foo(*)"));
        assert!(allow.iter().any(|v| v == "Bash(*)"));
        assert_eq!(allow.len(), WISPHIVE_PERMISSIONS.len() + 1);
    }

    #[test]
    fn add_permissions_idempotent() {
        let mut s = json!({});
        add_wisphive_permissions(&mut s);
        add_wisphive_permissions(&mut s);
        add_wisphive_permissions(&mut s);
        let allow = s["permissions"]["allow"].as_array().unwrap();
        assert_eq!(allow.len(), WISPHIVE_PERMISSIONS.len());
    }

    #[test]
    fn add_permissions_no_duplicates_when_user_has_same() {
        let mut s = json!({"permissions": {"allow": ["Bash(*)"]}});
        add_wisphive_permissions(&mut s);
        let allow = s["permissions"]["allow"].as_array().unwrap();
        let bash_count = allow
            .iter()
            .filter(|v| v.as_str() == Some("Bash(*)"))
            .count();
        assert_eq!(bash_count, 1);
        assert_eq!(allow.len(), WISPHIVE_PERMISSIONS.len());
    }

    #[test]
    fn remove_permissions_cleans_wisphive_only() {
        let mut s = json!({"permissions": {"allow": [
            "Bash(*)", "Edit(*)", "Write(*)", "NotebookEdit(*)", "mcp__custom(*)"
        ]}});
        remove_wisphive_permissions(&mut s);
        let allow = s["permissions"]["allow"].as_array().unwrap();
        assert_eq!(allow.len(), 1);
        assert_eq!(allow[0], "mcp__custom(*)");
    }

    #[test]
    fn remove_permissions_noop_when_empty() {
        let mut s = json!({});
        remove_wisphive_permissions(&mut s);
        assert!(s.get("permissions").is_none());
    }

    #[test]
    fn remove_permissions_noop_when_no_allow() {
        let mut s = json!({"permissions": {"deny": ["something"]}});
        remove_wisphive_permissions(&mut s);
        assert!(s["permissions"].get("allow").is_none());
        assert_eq!(s["permissions"]["deny"][0], "something");
    }

    #[test]
    fn install_adds_both_hooks_and_permissions() {
        let tmp = temp_project();
        let p = tmp.path().to_path_buf();
        install(Some(p.clone()), false).unwrap();
        let s = read_settings(&p);
        assert!(s["hooks"]["PreToolUse"].is_array());
        let allow = s["permissions"]["allow"].as_array().unwrap();
        assert!(allow.iter().any(|v| v == "Bash(*)"));
    }

    #[test]
    fn uninstall_removes_both_hooks_and_permissions() {
        let tmp = temp_project();
        let p = tmp.path().to_path_buf();
        install(Some(p.clone()), false).unwrap();
        uninstall(Some(p.clone()), false).unwrap();
        let s = read_settings(&p);
        let hooks = s["hooks"]["PreToolUse"].as_array().unwrap();
        assert!(hooks.is_empty());
        let allow = s["permissions"]["allow"].as_array().unwrap();
        assert!(allow.is_empty());
    }

    #[test]
    fn round_trip_preserves_user_permissions() {
        let tmp = temp_project();
        let p = tmp.path().to_path_buf();
        write_settings(
            &p,
            &json!({
                "permissions": {"allow": ["mcp__github(*)"]},
                "hooks": {"PreToolUse": [cc_rule("linter")]}
            }),
        );
        install(Some(p.clone()), false).unwrap();
        uninstall(Some(p.clone()), false).unwrap();
        let s = read_settings(&p);
        let allow = s["permissions"]["allow"].as_array().unwrap();
        assert_eq!(allow.len(), 1);
        assert_eq!(allow[0], "mcp__github(*)");
        let hooks = s["hooks"]["PreToolUse"].as_array().unwrap();
        assert_eq!(hooks.len(), 1);
        assert_eq!(hooks[0]["hooks"][0]["command"], "linter");
    }

    // ══ audit after install (itr#460) ══

    #[test]
    fn audit_reports_installed_after_install_hooks() {
        use crate::project_audit::ProjectAudit;
        let tmp = temp_project();
        let home = tempfile::tempdir().unwrap();
        let p = tmp.path().to_path_buf();

        install_hooks(&p).unwrap();

        let audit = ProjectAudit::scan_with_home(&p, home.path());
        assert!(
            audit.hooks.claude.installed,
            "claude hooks should be installed after install_hooks"
        );
        assert!(
            audit.hooks.codex.installed,
            "codex hooks should be installed after install_hooks"
        );
        assert!(audit.hooks.all_installed);
    }
}
