//! Tamper-evidence for the user-editable approval configuration.
//!
//! This does not claim to stop code already running as the operator's UID.
//! Instead it safely defaults an untrusted config and makes an effective policy
//! widening visible through a notification and TUI/web banner (ADR-0008).

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use notify::{EventKind, RecursiveMode, Watcher};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use tokio::sync::{broadcast, mpsc};
use tracing::{error, info, warn};
use wisphive_protocol::{AutoApproveLevel, ConfigAlertKind, ServerMessage};

use crate::state::StateDb;

/// Parsed, policy-relevant portion of `config.json`. This deliberately reads
/// raw JSON rather than [`crate::config::UserConfig`]: the hook also consumes
/// future fields and raw rule patterns that the daemon config struct need not
/// model yet.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PolicySnapshot {
    /// `off=0`, `read=1`, `write=2`, `execute=3`, `all=4`.
    #[serde(default = "default_read_tier")]
    pub level_rank: u8,
    #[serde(default)]
    pub auto_approve_add: BTreeSet<String>,
    #[serde(default)]
    pub auto_approve_remove: BTreeSet<String>,
    #[serde(default)]
    pub always_ask: BTreeSet<String>,
    #[serde(default)]
    pub always_ask_remove: BTreeSet<String>,
    #[serde(default)]
    pub dangerous: bool,
    #[serde(default)]
    pub allow_self_modification: bool,
    #[serde(default)]
    pub codex_allow_foreign_hooks: bool,
    #[serde(default)]
    pub auto_approve_stop: bool,
    #[serde(default = "default_true")]
    pub auto_approve_user_prompt: bool,
    #[serde(default = "default_true")]
    pub auto_approve_config_change: bool,
    #[serde(default = "default_true")]
    pub auto_approve_lifecycle: bool,
    #[serde(default)]
    pub allow_patterns: BTreeMap<String, BTreeSet<String>>,
    /// Missing config is a trusted safe-default input. A present file that
    /// fails [`wisphive_protocol::fs_trust::read_trusted`] is not trusted.
    #[serde(default = "default_true")]
    pub trusted: bool,
    /// Same 16-hex SHA-256 prefix the hook puts in its audit events.
    #[serde(default)]
    pub hash: Option<String>,
    /// Diagnostic retained with the snapshot so a transition can tell the
    /// operator why the config was rejected. It is not policy input.
    #[serde(default)]
    pub untrusted_reason: Option<String>,
}

impl Default for PolicySnapshot {
    fn default() -> Self {
        Self {
            level_rank: default_read_tier(),
            auto_approve_add: BTreeSet::new(),
            auto_approve_remove: BTreeSet::new(),
            always_ask: BTreeSet::new(),
            always_ask_remove: BTreeSet::new(),
            dangerous: false,
            allow_self_modification: false,
            codex_allow_foreign_hooks: false,
            auto_approve_stop: false,
            auto_approve_user_prompt: true,
            auto_approve_config_change: true,
            auto_approve_lifecycle: true,
            allow_patterns: BTreeMap::new(),
            trusted: true,
            hash: None,
            untrusted_reason: None,
        }
    }
}

fn default_read_tier() -> u8 {
    1
}

fn default_true() -> bool {
    true
}

impl PolicySnapshot {
    /// Read the current policy through the shared descriptor-based trust gate.
    /// A missing config is the documented default-read policy; all other read
    /// trust failures become a safe-default, untrusted snapshot.
    pub fn read(path: &Path) -> Self {
        let hash = config_snapshot_hash(path);
        match wisphive_protocol::fs_trust::read_trusted(path) {
            Ok(contents) => {
                let value = serde_json::from_str(&contents).unwrap_or(serde_json::Value::Null);
                Self::from_value(&value, true, hash, None)
            }
            Err(wisphive_protocol::fs_trust::TrustError::Io(error))
                if error.kind() == std::io::ErrorKind::NotFound =>
            {
                Self {
                    hash,
                    ..Self::default()
                }
            }
            Err(error) => Self {
                trusted: false,
                hash,
                untrusted_reason: Some(error.to_string()),
                ..Self::default()
            },
        }
    }

    fn from_value(
        value: &serde_json::Value,
        trusted: bool,
        hash: Option<String>,
        untrusted_reason: Option<String>,
    ) -> Self {
        let level_rank = value
            .get("auto_approve_level")
            .and_then(serde_json::Value::as_str)
            .and_then(|value| value.parse::<AutoApproveLevel>().ok())
            .map(level_rank)
            .unwrap_or_else(default_read_tier);

        Self {
            level_rank,
            auto_approve_add: string_set(value, "auto_approve_add"),
            auto_approve_remove: string_set(value, "auto_approve_remove"),
            always_ask: string_set(value, "always_ask"),
            always_ask_remove: string_set(value, "always_ask_remove"),
            dangerous: bool_at(value, "auto_approve_dangerous", false),
            allow_self_modification: bool_at(value, "allow_self_modification", false),
            codex_allow_foreign_hooks: bool_at(value, "codex_allow_foreign_hooks", false),
            auto_approve_stop: bool_at(value, "auto_approve_stop", false),
            auto_approve_user_prompt: bool_at(value, "auto_approve_user_prompt", true),
            auto_approve_config_change: bool_at(value, "auto_approve_config_change", true),
            auto_approve_lifecycle: bool_at(value, "auto_approve_lifecycle", true),
            allow_patterns: allow_patterns(value),
            trusted,
            hash,
            untrusted_reason,
        }
    }
}

fn level_rank(level: AutoApproveLevel) -> u8 {
    match level {
        AutoApproveLevel::Off => 0,
        AutoApproveLevel::Read => 1,
        AutoApproveLevel::Write => 2,
        AutoApproveLevel::Execute => 3,
        AutoApproveLevel::All => 4,
    }
}

fn level_name(level: u8) -> &'static str {
    match level {
        0 => "off",
        1 => "read",
        2 => "write",
        3 => "execute",
        4 => "all",
        _ => "unknown",
    }
}

fn string_set(value: &serde_json::Value, key: &str) -> BTreeSet<String> {
    value
        .get(key)
        .and_then(serde_json::Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(serde_json::Value::as_str)
        .map(str::to_owned)
        .collect()
}

fn bool_at(value: &serde_json::Value, key: &str, default: bool) -> bool {
    value
        .get(key)
        .and_then(serde_json::Value::as_bool)
        .unwrap_or(default)
}

fn allow_patterns(value: &serde_json::Value) -> BTreeMap<String, BTreeSet<String>> {
    let Some(rules) = value
        .get("tool_rules")
        .and_then(serde_json::Value::as_object)
    else {
        return BTreeMap::new();
    };

    rules
        .iter()
        .filter_map(|(tool, rule)| {
            let patterns = rule
                .get("allow_patterns")
                .and_then(serde_json::Value::as_array)
                .into_iter()
                .flatten()
                .filter_map(serde_json::Value::as_str)
                .map(str::to_owned)
                .collect::<BTreeSet<_>>();
            (!patterns.is_empty()).then(|| (tool.clone(), patterns))
        })
        .collect()
}

fn config_snapshot_hash(path: &Path) -> Option<String> {
    let bytes = std::fs::read(path).ok()?;
    let digest = Sha256::digest(&bytes);
    Some(
        digest
            .iter()
            .take(8)
            .map(|byte| format!("{byte:02x}"))
            .collect(),
    )
}

/// One latched config-alert condition. Policy widenings update an already
/// visible banner, while untrusted input raises on a trust-loss crossing and
/// is reasserted from its current level at startup.
#[derive(Debug, Default, Clone, Copy)]
pub struct AlertState {
    untrusted_active: bool,
    policy_widened_active: bool,
}

/// A config alert transition produced by [`evaluate`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConfigAlertEvent {
    pub kind: ConfigAlertKind,
    pub active: bool,
    pub message: String,
    /// Present for a policy widening so notification/logging can name every
    /// individual widening instead of only the joined banner message.
    pub deltas: Vec<String>,
}

fn untrusted_config_alert(snapshot: &PolicySnapshot) -> ConfigAlertEvent {
    ConfigAlertEvent {
        kind: ConfigAlertKind::UntrustedConfig,
        active: true,
        message: format!(
            "config.json is untrusted ({}); Wisphive is using the safe default read-tier approval policy.",
            snapshot
                .untrusted_reason
                .as_deref()
                .unwrap_or("ownership or permissions could not be verified")
        ),
        deltas: Vec::new(),
    }
}

/// Compare two snapshots and return human-readable policy widenings. A
/// narrowing or no-op returns no entries.
pub fn widenings(old: &PolicySnapshot, new: &PolicySnapshot) -> Vec<String> {
    let mut deltas = Vec::new();

    if new.level_rank > old.level_rank {
        deltas.push(format!(
            "auto_approve_level increased from {} to {}",
            level_name(old.level_rank),
            level_name(new.level_rank)
        ));
    }

    for tool in new.auto_approve_add.difference(&old.auto_approve_add) {
        deltas.push(format!("auto_approve_add added {tool}"));
    }
    for tool in old.auto_approve_remove.difference(&new.auto_approve_remove) {
        deltas.push(format!("auto_approve_remove removed {tool}"));
    }
    for tool in new.always_ask_remove.difference(&old.always_ask_remove) {
        deltas.push(format!("always_ask_remove added {tool}"));
    }
    for tool in old.always_ask.difference(&new.always_ask) {
        deltas.push(format!("always_ask removed {tool}"));
    }

    for (name, old_value, new_value) in [
        ("auto_approve_dangerous", old.dangerous, new.dangerous),
        (
            "allow_self_modification",
            old.allow_self_modification,
            new.allow_self_modification,
        ),
        (
            "codex_allow_foreign_hooks",
            old.codex_allow_foreign_hooks,
            new.codex_allow_foreign_hooks,
        ),
        (
            "auto_approve_stop",
            old.auto_approve_stop,
            new.auto_approve_stop,
        ),
        (
            "auto_approve_user_prompt",
            old.auto_approve_user_prompt,
            new.auto_approve_user_prompt,
        ),
        (
            "auto_approve_config_change",
            old.auto_approve_config_change,
            new.auto_approve_config_change,
        ),
        (
            "auto_approve_lifecycle",
            old.auto_approve_lifecycle,
            new.auto_approve_lifecycle,
        ),
    ] {
        if !old_value && new_value {
            deltas.push(format!("{name} enabled"));
        }
    }

    for (tool, patterns) in &new.allow_patterns {
        let previous = old.allow_patterns.get(tool);
        for pattern in patterns {
            if previous.is_none_or(|previous| !previous.contains(pattern)) {
                deltas.push(format!("tool_rules.{tool}.allow_patterns added {pattern}"));
            }
        }
    }

    deltas
}

/// Compare snapshots, update alert latches, and return the banner transitions
/// for this observation. Pure state logic keeps every crossing testable.
pub fn evaluate(
    old: &PolicySnapshot,
    new: &PolicySnapshot,
    state: &mut AlertState,
) -> Vec<ConfigAlertEvent> {
    let mut events = Vec::new();

    if old.trusted && !new.trusted && !state.untrusted_active {
        state.untrusted_active = true;
        events.push(untrusted_config_alert(new));
    } else if !old.trusted && new.trusted && state.untrusted_active {
        state.untrusted_active = false;
        events.push(ConfigAlertEvent {
            kind: ConfigAlertKind::UntrustedConfig,
            active: false,
            message: "config.json ownership and permissions are trusted again.".into(),
            deltas: Vec::new(),
        });
    }

    let deltas = widenings(old, new);
    if !deltas.is_empty() {
        state.policy_widened_active = true;
        events.push(ConfigAlertEvent {
            kind: ConfigAlertKind::PolicyWidened,
            active: true,
            message: deltas.join("; "),
            deltas,
        });
    } else if state.policy_widened_active {
        state.policy_widened_active = false;
        events.push(ConfigAlertEvent {
            kind: ConfigAlertKind::PolicyWidened,
            active: false,
            message: "approval policy widening banner cleared after a narrowing or no-op change."
                .into(),
            deltas: Vec::new(),
        });
    }

    events
}

/// Spawn the config watcher. It tracks `config.json` changes, coalesces bursts
/// from atomic rename writes, and persists each observed snapshot for restart
/// comparisons.
pub fn spawn_config_watcher(
    config_path: PathBuf,
    state_db: Arc<StateDb>,
    tui_tx: broadcast::Sender<ServerMessage>,
    notifications_enabled: bool,
) -> tokio::task::JoinHandle<()> {
    tokio::spawn(async move {
        if let Err(error) =
            run_config_watcher(config_path, state_db, tui_tx, notifications_enabled).await
        {
            error!("config watcher task failed: {error}");
        }
    })
}

async fn run_config_watcher(
    config_path: PathBuf,
    state_db: Arc<StateDb>,
    tui_tx: broadcast::Sender<ServerMessage>,
    notifications_enabled: bool,
) -> anyhow::Result<()> {
    let (tx, mut rx) = mpsc::channel::<()>(64);
    let watched_path = config_path.clone();
    let mut watcher =
        notify::recommended_watcher(move |result: Result<notify::Event, notify::Error>| {
            if let Ok(event) = result
                && matches!(event.kind, EventKind::Modify(_) | EventKind::Create(_))
                && event.paths.iter().any(|path| path == &watched_path)
            {
                let _ = tx.try_send(());
            }
        })?;
    let watch_dir = config_path.parent().unwrap_or_else(|| Path::new("."));
    watcher.watch(watch_dir, RecursiveMode::NonRecursive)?;
    info!(path = %config_path.display(), "config watcher active");

    let (mut previous, mut alert_state, startup_events) =
        startup_check(&config_path, &state_db).await?;
    dispatch_events(startup_events, &tui_tx, notifications_enabled);

    while rx.recv().await.is_some() {
        while rx.try_recv().is_ok() {}
        // `write_config_atomic` renames a finished temp file, but this still
        // coalesces editor saves and duplicate backend notifications.
        tokio::time::sleep(Duration::from_millis(250)).await;
        while rx.try_recv().is_ok() {}

        let path = config_path.clone();
        let current = tokio::task::spawn_blocking(move || PolicySnapshot::read(&path))
            .await
            .map_err(|error| anyhow::anyhow!("config watcher snapshot task failed: {error}"))?;
        let events = evaluate(&previous, &current, &mut alert_state);
        state_db.save_config_watch_snapshot(&current).await?;
        dispatch_events(events, &tui_tx, notifications_enabled);
        previous = current;
    }

    Ok(())
}

/// Compare the persisted baseline with the configuration visible at daemon
/// startup. No baseline means this is a first boot, so persist silently rather
/// than warning about a policy the daemon has never observed before.
pub(crate) async fn startup_check(
    config_path: &Path,
    state_db: &StateDb,
) -> anyhow::Result<(PolicySnapshot, AlertState, Vec<ConfigAlertEvent>)> {
    let current = PolicySnapshot::read(config_path);
    let mut alert_state = AlertState::default();
    let mut events = match state_db.load_config_watch_snapshot().await? {
        Some(previous) => evaluate(&previous, &current, &mut alert_state),
        None => Vec::new(),
    };
    // Trust loss is a level condition on startup: the persisted policy
    // baseline remains the real comparison input for widenings, but a config
    // still being safe-defaulted must restore its banner after a restart.
    if !current.trusted && !alert_state.untrusted_active {
        alert_state.untrusted_active = true;
        events.push(untrusted_config_alert(&current));
    }
    state_db.save_config_watch_snapshot(&current).await?;
    Ok((current, alert_state, events))
}

fn dispatch_events(
    events: Vec<ConfigAlertEvent>,
    tui_tx: &broadcast::Sender<ServerMessage>,
    notifications_enabled: bool,
) {
    for event in events {
        if event.kind == ConfigAlertKind::PolicyWidened && event.active {
            for delta in &event.deltas {
                warn!(delta = %crate::notify::sanitize_for_log(delta), "approval policy widened");
            }
            if notifications_enabled {
                crate::notify::notify_config_widened(&event.deltas);
            }
        }

        let message = crate::notify::sanitize_for_log(&event.message);
        if event.kind == ConfigAlertKind::UntrustedConfig && event.active {
            warn!(message = %message, "config policy input is untrusted");
            if notifications_enabled {
                crate::notify::notify_config_untrusted(&message);
            }
        }
        let _ = tui_tx.send(ServerMessage::ConfigAlert {
            kind: event.kind,
            active: event.active,
            message,
            at: chrono::Utc::now(),
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn snapshot() -> PolicySnapshot {
        PolicySnapshot::default()
    }

    #[test]
    fn level_increase_is_widening() {
        let old = snapshot();
        let mut new = snapshot();
        new.level_rank = 4;

        assert_eq!(
            widenings(&old, &new),
            vec!["auto_approve_level increased from read to all"]
        );
    }

    #[test]
    fn added_auto_approve_add_is_widening() {
        let old = snapshot();
        let mut new = snapshot();
        new.auto_approve_add.insert("Bash".into());

        assert!(
            widenings(&old, &new)
                .iter()
                .any(|delta| delta == "auto_approve_add added Bash")
        );
    }

    #[test]
    fn removed_always_ask_entry_is_widening() {
        let mut old = snapshot();
        old.always_ask.insert("Bash".into());
        let new = snapshot();

        assert!(
            widenings(&old, &new)
                .iter()
                .any(|delta| delta == "always_ask removed Bash")
        );
    }

    #[test]
    fn removed_auto_approve_remove_entry_is_widening() {
        let mut old = snapshot();
        old.auto_approve_remove.insert("Bash".into());
        let new = snapshot();

        assert!(
            widenings(&old, &new)
                .iter()
                .any(|delta| delta == "auto_approve_remove removed Bash")
        );
    }

    #[test]
    fn new_allow_pattern_is_widening() {
        let old = snapshot();
        let mut new = snapshot();
        new.allow_patterns
            .entry("Edit".into())
            .or_default()
            .insert("/tmp/safe".into());

        assert!(
            widenings(&old, &new)
                .iter()
                .any(|delta| delta == "tool_rules.Edit.allow_patterns added /tmp/safe")
        );
    }

    #[test]
    fn narrowing_produces_no_events() {
        let mut old = snapshot();
        old.level_rank = 4;
        old.auto_approve_add.insert("Bash".into());
        let new = snapshot();

        assert!(widenings(&old, &new).is_empty());
    }

    #[test]
    fn dangerous_flip_is_widening() {
        let old = snapshot();
        let mut new = snapshot();
        new.dangerous = true;

        assert!(
            widenings(&old, &new)
                .iter()
                .any(|delta| delta == "auto_approve_dangerous enabled")
        );
    }

    #[test]
    fn untrusted_latches_once_and_clears() {
        let trusted = snapshot();
        let mut untrusted = snapshot();
        untrusted.trusted = false;
        untrusted.untrusted_reason = Some("file permissions are 0o0666".into());
        let mut state = AlertState::default();

        let raised = evaluate(&trusted, &untrusted, &mut state);
        assert!(
            raised
                .iter()
                .any(|event| { event.kind == ConfigAlertKind::UntrustedConfig && event.active })
        );
        assert!(evaluate(&untrusted, &untrusted, &mut state).is_empty());

        let cleared = evaluate(&untrusted, &trusted, &mut state);
        assert!(
            cleared
                .iter()
                .any(|event| { event.kind == ConfigAlertKind::UntrustedConfig && !event.active })
        );
    }

    #[test]
    fn absent_config_baseline_is_read_tier() {
        let directory = tempfile::tempdir().unwrap();
        let snapshot = PolicySnapshot::read(&directory.path().join("config.json"));

        assert_eq!(snapshot.level_rank, 1);
        assert!(
            snapshot.trusted,
            "absent config is the safe default, not an alert"
        );
        assert!(snapshot.hash.is_none());
    }

    #[tokio::test]
    async fn startup_alerts_on_widening_applied_while_stopped() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("config.json");
        std::fs::write(&path, r#"{"auto_approve_level":"read"}"#).unwrap();
        let state_db = crate::state::test_support::test_db().await;
        state_db
            .save_config_watch_snapshot(&PolicySnapshot::read(&path))
            .await
            .unwrap();

        std::fs::write(&path, r#"{"auto_approve_level":"all"}"#).unwrap();
        let (_, _, events) = startup_check(&path, &state_db).await.unwrap();

        assert!(events.iter().any(|event| {
            event.kind == ConfigAlertKind::PolicyWidened
                && event.active
                && event.message.contains("increased from read to all")
        }));
    }

    #[tokio::test]
    #[cfg(unix)]
    async fn startup_reasserts_untrusted_banner_when_config_still_untrusted() {
        use std::os::unix::fs::PermissionsExt;

        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("config.json");
        std::fs::write(&path, r#"{"auto_approve_level":"read"}"#).unwrap();
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o666)).unwrap();

        let state_db = crate::state::test_support::test_db().await;
        let mut baseline = snapshot();
        baseline.trusted = false;
        baseline.untrusted_reason = Some("file permissions are 0o0666".into());
        state_db
            .save_config_watch_snapshot(&baseline)
            .await
            .unwrap();

        let (_, _, events) = startup_check(&path, &state_db).await.unwrap();

        assert!(events.iter().any(|event| {
            event.kind == ConfigAlertKind::UntrustedConfig
                && event.active
                && event.message.contains("safe default read-tier")
        }));
    }
}
