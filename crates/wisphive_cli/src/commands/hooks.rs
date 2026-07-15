use std::path::PathBuf;

use anyhow::{Context, Result};
use wisphive_daemon::config::{ModeFileState, read_mode_file, write_mode_file_atomic};

use super::statecheck as commands_statecheck;
use wisphive_daemon::hook_install;
use wisphive_daemon::project_audit::{AgentHookAudit, HookMode, audit_project};

/// Get the wisphive home directory.
fn wisphive_home() -> PathBuf {
    let home = std::env::var("HOME").unwrap_or_else(|_| "/tmp".into());
    PathBuf::from(home).join(".wisphive")
}

/// Get the mode file path.
fn mode_path() -> PathBuf {
    wisphive_home().join("mode")
}

/// Emergency kill switch — writes "off" to the mode file. Same strict
/// transition as `wisphive off` (itr#541).
pub fn emergency_off() -> Result<()> {
    mode_off()
}

/// Set the mode file to the given value.
pub fn set_mode(mode: &str) -> Result<()> {
    let path = mode_path();
    write_mode_file_atomic(&path, mode)
        .with_context(|| format!("securely writing {}", path.display()))?;
    eprintln!("Wisphive hooks mode: {mode}");
    Ok(())
}

/// `wisphive on` — strict mode transition (itr#541): create/repair the state
/// dir (0700) and mode file (0600, atomic) so an enabled system always
/// satisfies the hook's strict validators, refusing on tamper-evidence-class
/// state (symlinks / foreign owners).
pub fn mode_on() -> Result<()> {
    transition_at(&wisphive_home(), "active")?;
    eprintln!("Wisphive gating is ON: hooks route tool calls through the daemon for review.");
    Ok(())
}

/// `wisphive off` — same strict transition writing "off", with a loud notice.
pub fn mode_off() -> Result<()> {
    transition_at(&wisphive_home(), "off")?;
    eprintln!("Wisphive gating is OFF: every hook passes tool calls through WITHOUT review.");
    eprintln!("Re-enable with: wisphive on");
    Ok(())
}

/// Strict, repair-then-enable mode transition against a specific state dir.
///
/// Tamper-evidence-class state (symlinked or foreign-owned dir/mode file)
/// refuses with the exact reason — never silently replaced, because that
/// state is evidence the operator must inspect. Everything else is repaired
/// by `write_mode_file_atomic` itself: it creates the dir, tightens it to
/// 0700 (owner-only, never chowns), and atomically writes a fresh 0600 mode
/// file, then re-reads it through the same strict validators the hook uses.
fn transition_at(wisphive_dir: &std::path::Path, mode: &str) -> Result<()> {
    if let Err(reason) = commands_statecheck::refuse_if_tampered(wisphive_dir) {
        anyhow::bail!(
            "refusing to set mode {mode:?}: {reason}\n\
             (binary-independent alternative: scripts/wisphive-rescue.sh)"
        );
    }
    let path = wisphive_dir.join("mode");
    write_mode_file_atomic(&path, mode)
        .with_context(|| format!("securely writing {}", path.display()))?;
    eprintln!(
        "Wrote {} = {mode:?} (state dir 0700, mode file 0600, atomic replace)",
        path.display()
    );
    Ok(())
}

/// Install Wisphive hooks for Claude Code and Codex.
/// Performs surgical JSON editing — only adds Wisphive entries, preserves everything else.
///
/// The actual JSON-editing lives in `wisphive_daemon::hook_install` (shared with
/// the daemon's web-driven install, itr#460); this wrapper adds the CLI's
/// human-facing status lines around the silent library calls.
pub fn install(project: Option<PathBuf>, _all: bool) -> Result<()> {
    let project = project
        .or_else(|| std::env::current_dir().ok())
        .context("could not determine project directory")?;

    hook_install::install_hooks(&project)?;

    let claude_path = project.join(".claude").join("settings.json");
    eprintln!("Wisphive hooks installed in {}", claude_path.display());

    let codex_path = project.join(".codex").join("hooks.json");
    eprintln!("Wisphive hooks installed in {}", codex_path.display());
    eprintln!("{}", hook_install::CODEX_HOOK_REVIEW_NOTE);

    Ok(())
}

/// Remove Wisphive hooks for Claude Code and Codex.
/// Only removes entries with the Wisphive hook command — preserves everything else.
pub fn uninstall(project: Option<PathBuf>, _all: bool) -> Result<()> {
    let project = project
        .or_else(|| std::env::current_dir().ok())
        .context("could not determine project directory")?;

    match hook_install::uninstall_claude(&project)? {
        Some(path) => eprintln!("Wisphive hooks removed from {}", path.display()),
        None => eprintln!("No .claude/settings.json found in {}", project.display()),
    }

    match hook_install::uninstall_codex(&project)? {
        Some(path) => eprintln!("Wisphive hooks removed from {}", path.display()),
        None => eprintln!("No .codex/hooks.json found in {}", project.display()),
    }

    Ok(())
}

/// Show current hook status.
pub fn status() -> Result<()> {
    // Mode
    let path = mode_path();
    match read_mode_file(&path) {
        Ok(ModeFileState::Active) => eprintln!("Mode: active"),
        Ok(ModeFileState::Off) => eprintln!("Mode: off"),
        Err(error) => eprintln!("Mode: unsafe or unavailable ({error})"),
    }

    // Daemon
    let pid_path = wisphive_home().join("wisphive.pid");
    if pid_path.exists() {
        let pid = std::fs::read_to_string(&pid_path)?;
        let pid = pid.trim();
        if let Ok(pid_num) = pid.parse::<i32>() {
            #[cfg(unix)]
            {
                if process_exists(pid_num) {
                    eprintln!("Daemon: running (pid: {pid})");
                } else {
                    eprintln!("Daemon: not running (stale PID file: {pid})");
                }
            }
            #[cfg(not(unix))]
            {
                eprintln!("Daemon PID file: {pid}");
            }
        } else {
            eprintln!("Daemon: invalid PID file ({pid})");
        }
    } else {
        eprintln!("Daemon: not running");
    }

    // Socket
    let socket_path = wisphive_home().join("wisphive.sock");
    if socket_path.exists() {
        eprintln!("Socket: {}", socket_path.display());
    } else {
        eprintln!("Socket: not found");
    }

    if let Ok(project) = std::env::current_dir() {
        let audit = audit_project(&project);
        eprintln!("Project: {}", audit.project_dir.display());
        print_agent_hook_status("Claude Code", &audit.hooks.claude, &audit.hooks.mode);
        print_agent_hook_status("Codex", &audit.hooks.codex, &audit.hooks.mode);

        if audit.hooks.codex.installed {
            eprintln!("Codex: {}", hook_install::CODEX_HOOK_REVIEW_NOTE);
        }
    }

    Ok(())
}

fn print_agent_hook_status(agent_name: &str, audit: &AgentHookAudit, mode: &HookMode) {
    let installed = audit.installed_events.len();
    let total = installed + audit.missing_events.len();

    if audit.enabled {
        eprintln!("{agent_name}: hooks enabled ({installed}/{total} events)");
        return;
    }

    if audit.installed {
        eprintln!(
            "{agent_name}: hooks installed but disabled (mode: {})",
            hook_mode_label(mode)
        );
        return;
    }

    if !audit.config_present {
        eprintln!(
            "{agent_name}: hooks not installed (missing {})",
            audit.config_path.display()
        );
        return;
    }

    if !audit.config_valid {
        let detail = audit
            .read_error
            .as_deref()
            .or(audit.parse_error.as_deref())
            .unwrap_or("invalid hook config");
        eprintln!("{agent_name}: hook config invalid ({detail})");
        return;
    }

    eprintln!("{agent_name}: hooks incomplete ({installed}/{total} events)");
}

fn hook_mode_label(mode: &HookMode) -> String {
    match mode {
        HookMode::Active => "active".into(),
        HookMode::Off => "off".into(),
        HookMode::Missing => "missing".into(),
        HookMode::Invalid(value) => format!("invalid: {value}"),
    }
}

#[cfg(unix)]
fn process_exists(pid: i32) -> bool {
    if unsafe { libc::kill(pid, 0) } == 0 {
        return true;
    }

    std::io::Error::last_os_error().raw_os_error() == Some(libc::EPERM)
}

#[cfg(test)]
mod tests {
    use super::install;
    #[cfg(unix)]
    use super::{ModeFileState, read_mode_file, transition_at};

    fn write_claude_settings(project: &std::path::Path, settings: serde_json::Value) {
        let claude_dir = project.join(".claude");
        std::fs::create_dir_all(&claude_dir).unwrap();
        std::fs::write(
            claude_dir.join("settings.json"),
            serde_json::to_vec(&settings).unwrap(),
        )
        .unwrap();
    }

    fn assert_install_rejects_hooks_value(hooks: serde_json::Value) {
        let tmp = tempfile::tempdir().unwrap();
        write_claude_settings(tmp.path(), serde_json::json!({ "hooks": hooks }));

        let error = install(Some(tmp.path().to_path_buf()), false)
            .expect_err("malformed Claude hooks must fail without panicking");
        let message = error.to_string();
        assert!(
            message.contains("`hooks` must be a JSON object"),
            "unexpected error: {message}"
        );
        assert!(
            message.contains("replace it with `\"hooks\": {}`"),
            "error should tell the user how to repair the config: {message}"
        );
        assert!(
            !tmp.path().join(".codex/hooks.json").exists(),
            "a rejected Claude config must not partially install Codex hooks"
        );
    }

    #[test]
    fn install_rejects_array_hooks_value() {
        assert_install_rejects_hooks_value(serde_json::json!([]));
    }

    #[test]
    fn install_rejects_string_hooks_value() {
        assert_install_rejects_hooks_value(serde_json::json!("not hooks"));
    }

    #[test]
    fn install_rejects_number_hooks_value() {
        assert_install_rejects_hooks_value(serde_json::json!(42));
    }

    #[test]
    fn install_rejects_boolean_hooks_value() {
        assert_install_rejects_hooks_value(serde_json::json!(true));
    }

    #[test]
    fn install_accepts_object_hooks_and_preserves_other_settings() {
        let valid = tempfile::tempdir().unwrap();
        write_claude_settings(
            valid.path(),
            serde_json::json!({ "hooks": {}, "theme": "dark" }),
        );
        install(Some(valid.path().to_path_buf()), false).unwrap();

        let installed: serde_json::Value = serde_json::from_slice(
            &std::fs::read(valid.path().join(".claude/settings.json")).unwrap(),
        )
        .unwrap();
        assert_eq!(installed["theme"], "dark");
        assert!(installed["hooks"].is_object());
    }

    #[test]
    fn install_accepts_missing_hooks_key_and_missing_settings_file() {
        let missing_key = tempfile::tempdir().unwrap();
        write_claude_settings(missing_key.path(), serde_json::json!({ "theme": "dark" }));
        install(Some(missing_key.path().to_path_buf()), false).unwrap();

        let missing = tempfile::tempdir().unwrap();
        install(Some(missing.path().to_path_buf()), false).unwrap();
        assert!(missing.path().join(".claude/settings.json").is_file());
        assert!(missing.path().join(".codex/hooks.json").is_file());
    }

    // ══ Mode file ══

    #[test]
    fn mode_creates_and_writes() {
        let tmp = tempfile::tempdir().unwrap();
        let f = tmp.path().join("mode");
        std::fs::write(&f, "active").unwrap();
        assert_eq!(std::fs::read_to_string(&f).unwrap(), "active");
        std::fs::write(&f, "off").unwrap();
        assert_eq!(std::fs::read_to_string(&f).unwrap(), "off");
    }

    #[test]
    fn mode_missing_defaults_off() {
        let tmp = tempfile::tempdir().unwrap();
        let f = tmp.path().join("nope").join("mode");
        assert_eq!(
            std::fs::read_to_string(&f).unwrap_or_else(|_| "off".into()),
            "off"
        );
    }

    #[test]
    fn mode_whitespace_trimmed() {
        let tmp = tempfile::tempdir().unwrap();
        let f = tmp.path().join("mode");
        std::fs::write(&f, "  active  \n").unwrap();
        assert_eq!(std::fs::read_to_string(&f).unwrap().trim(), "active");
    }

    // ══ Strict mode transitions (itr#541) ══

    #[test]
    #[cfg(unix)]
    fn transition_from_empty_home_yields_strict_validator_compliant_state() {
        use std::os::unix::fs::PermissionsExt;

        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path().join(".wisphive");
        transition_at(&dir, "active").unwrap();

        let dir_perms = std::fs::metadata(&dir).unwrap().permissions().mode() & 0o7777;
        assert_eq!(dir_perms, 0o700);
        let mode_perms = std::fs::metadata(dir.join("mode"))
            .unwrap()
            .permissions()
            .mode()
            & 0o7777;
        assert_eq!(mode_perms, 0o600);
        // Same strict reader contract the hook enforces.
        assert_eq!(
            read_mode_file(&dir.join("mode")).unwrap(),
            ModeFileState::Active
        );
    }

    #[test]
    #[cfg(unix)]
    fn transition_repairs_legacy_loose_perms_then_enables() {
        use std::os::unix::fs::PermissionsExt;

        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path().join(".wisphive");
        std::fs::create_dir(&dir).unwrap();
        std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o755)).unwrap();
        std::fs::write(dir.join("mode"), "off").unwrap();
        std::fs::set_permissions(dir.join("mode"), std::fs::Permissions::from_mode(0o644)).unwrap();

        transition_at(&dir, "active").unwrap();
        assert_eq!(
            read_mode_file(&dir.join("mode")).unwrap(),
            ModeFileState::Active
        );
    }

    #[test]
    #[cfg(unix)]
    fn transition_refuses_symlinked_mode_file_with_reason() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path().join(".wisphive");
        std::fs::create_dir(&dir).unwrap();
        std::fs::set_permissions(
            &dir,
            <std::fs::Permissions as std::os::unix::fs::PermissionsExt>::from_mode(0o700),
        )
        .unwrap();
        let target = tmp.path().join("target");
        std::fs::write(&target, "active").unwrap();
        std::os::unix::fs::symlink(&target, dir.join("mode")).unwrap();

        let error = transition_at(&dir, "off").unwrap_err().to_string();
        assert!(error.contains("symlink"), "{error}");
        assert!(error.contains("tamper evidence"), "{error}");
        // The symlink is untouched — it is evidence.
        assert!(
            std::fs::symlink_metadata(dir.join("mode"))
                .unwrap()
                .file_type()
                .is_symlink()
        );
    }
}
