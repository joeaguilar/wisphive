//! Strict `~/.wisphive` state validation for `wisphive doctor` and the
//! `wisphive on` / `wisphive off` mode transitions (itr#537/#541, incident
//! 2026-07-15 / itr#533).
//!
//! MIRROR NOTICE: these checks intentionally mirror the hook's validators —
//! `read_mode_file` in `crates/wisphive_hook/src/main.rs` (hard: any failure
//! denies every hook event) and `wisphive_protocol::fs_trust::read_trusted`
//! (soft: an untrusted policy file is silently ignored in favor of safe
//! defaults). `scripts/wisphive-rescue.sh` carries the same checklist in pure
//! POSIX sh for when no wisphive binary works. Keep all three in sync; a
//! follow-up issue tracks extracting one shared validator module.
//!
//! Repair policy (`fix_perms`): only safe owner-side tightenings — `chmod`
//! toward 0700/0600, removing group/other write bits. Never loosens
//! permissions, never chowns, and REFUSES to touch symlinks, foreign-owned
//! entries, or wrong file types: those are tamper evidence the operator must
//! inspect by hand.

use std::path::{Path, PathBuf};

/// Maximum mode-file size the hook accepts (mirrors `MODE_FILE_MAX_BYTES` in
/// `wisphive_hook`).
pub const MODE_FILE_MAX_BYTES: u64 = 64;

/// Outcome of one named validator check.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Status {
    Pass,
    Fail,
    Info,
}

/// One named check, mirroring a hook validator, with the exact fix command
/// for failures and whether the automatic repair refuses it (tamper class).
#[derive(Debug)]
pub struct Check {
    pub name: String,
    pub status: Status,
    pub detail: String,
    /// Exact fix command; `None` when passing or when only a manual
    /// inspect-and-remove is appropriate (tamper evidence).
    pub fix: Option<String>,
    /// True when `fix_perms` must refuse: symlink / foreign owner / wrong
    /// type. These are tamper evidence, never auto-repaired.
    pub tamper: bool,
}

impl Check {
    fn pass(name: impl Into<String>, detail: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            status: Status::Pass,
            detail: detail.into(),
            fix: None,
            tamper: false,
        }
    }

    fn fail(name: impl Into<String>, detail: impl Into<String>, fix: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            status: Status::Fail,
            detail: detail.into(),
            fix: Some(fix.into()),
            tamper: false,
        }
    }

    fn tamper(name: impl Into<String>, detail: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            status: Status::Fail,
            detail: format!(
                "{} — tamper evidence: inspect it, then remove/replace it yourself",
                detail.into()
            ),
            fix: None,
            tamper: true,
        }
    }

    fn info(name: impl Into<String>, detail: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            status: Status::Info,
            detail: detail.into(),
            fix: None,
            tamper: false,
        }
    }
}

/// Repairs applied / refused by [`fix_perms`]. Every mutation is echoed in
/// `applied` (the exact command equivalent) — nothing is fixed silently.
#[derive(Debug, Default)]
pub struct FixOutcome {
    pub applied: Vec<String>,
    pub refused: Vec<String>,
}

#[cfg(unix)]
fn effective_uid() -> u32 {
    // SAFETY: `geteuid` takes no arguments and has no preconditions.
    unsafe { libc::geteuid() }
}

#[cfg(unix)]
fn lstat(path: &Path) -> Option<std::fs::Metadata> {
    std::fs::symlink_metadata(path).ok()
}

#[cfg(unix)]
fn perms_of(metadata: &std::fs::Metadata) -> u32 {
    use std::os::unix::fs::MetadataExt;
    metadata.mode() & 0o7777
}

#[cfg(unix)]
fn uid_of(metadata: &std::fs::Metadata) -> u32 {
    use std::os::unix::fs::MetadataExt;
    metadata.uid()
}

/// Classify one filesystem entry against the hook's strict expectations.
#[cfg(unix)]
struct EntryFacts {
    exists: bool,
    is_symlink: bool,
    kind_ok: bool,
    owned: bool,
    perms: u32,
}

#[cfg(unix)]
fn entry_facts(path: &Path, want_dir: bool) -> EntryFacts {
    match lstat(path) {
        None => EntryFacts {
            exists: false,
            is_symlink: false,
            kind_ok: false,
            owned: false,
            perms: 0,
        },
        Some(metadata) => EntryFacts {
            exists: true,
            is_symlink: metadata.file_type().is_symlink(),
            kind_ok: if want_dir {
                metadata.file_type().is_dir()
            } else {
                metadata.file_type().is_file()
            },
            owned: uid_of(&metadata) == effective_uid(),
            perms: perms_of(&metadata),
        },
    }
}

/// Run every strict check the hook enforces against `wisphive_dir`
/// (`~/.wisphive`), in validator order. Mirrors `wisphive_hook::read_mode_file`
/// (hard) + `fs_trust::read_trusted` policy-file trust (soft).
#[cfg(unix)]
pub fn run_checks(wisphive_dir: &Path) -> Vec<Check> {
    let mut checks = Vec::new();
    let dir_display = wisphive_dir.display();
    let uid = effective_uid();

    // ── HARD: state directory ──
    let dir = entry_facts(wisphive_dir, true);
    if !dir.exists {
        checks.push(Check::fail(
            "state dir exists",
            format!("{dir_display} is missing: the hook denies every event until it exists with mode 0700"),
            format!("mkdir -p '{dir_display}' && chmod 0700 '{dir_display}'  (or: wisphive on)"),
        ));
        return checks;
    }
    checks.push(Check::pass("state dir exists", dir_display.to_string()));

    if dir.is_symlink {
        checks.push(Check::tamper(
            "state dir is not a symlink",
            format!("{dir_display} is a symlink; the hook refuses to follow it (O_NOFOLLOW)"),
        ));
        return checks;
    }
    checks.push(Check::pass("state dir is not a symlink", ""));

    if !dir.kind_ok {
        checks.push(Check::tamper(
            "state dir is a directory",
            format!("{dir_display} exists but is not a directory"),
        ));
        return checks;
    }
    checks.push(Check::pass("state dir is a directory", ""));

    if !dir.owned {
        checks.push(Check::tamper(
            "state dir owned by you",
            format!("{dir_display} is not owned by your uid {uid} (never chowned automatically)"),
        ));
        return checks;
    }
    checks.push(Check::pass("state dir owned by you", format!("uid {uid}")));

    if dir.perms != 0o700 {
        checks.push(Check::fail(
            "state dir permissions are exactly 0700",
            format!(
                "{dir_display} has {:04o}; the hook requires exactly 0700",
                dir.perms
            ),
            format!("chmod 0700 '{dir_display}'"),
        ));
    } else {
        checks.push(Check::pass("state dir permissions are exactly 0700", ""));
    }

    // ── HARD: mode file ──
    let mode_path = wisphive_dir.join("mode");
    let mode_display = mode_path.display().to_string();
    let mode = entry_facts(&mode_path, false);
    if !mode.exists {
        checks.push(Check::fail(
            "mode file exists",
            format!("{mode_display} is missing: the hook denies every event until it exists ('active' or 'off'). Automatic repair never picks a gating posture for you."),
            "wisphive on  (enable gating)  or:  wisphive off  (disable gating)".to_string(),
        ));
    } else if mode.is_symlink {
        checks.push(Check::tamper(
            "mode file is not a symlink",
            format!("{mode_display} is a symlink; the hook refuses to follow it (O_NOFOLLOW)"),
        ));
    } else if !mode.kind_ok {
        checks.push(Check::tamper(
            "mode file is a regular file",
            format!("{mode_display} exists but is not a regular file"),
        ));
    } else if !mode.owned {
        checks.push(Check::tamper(
            "mode file owned by you",
            format!("{mode_display} is not owned by your uid {uid} (never chowned automatically)"),
        ));
    } else {
        checks.push(Check::pass("mode file exists", mode_display.clone()));
        checks.push(Check::pass("mode file is not a symlink", ""));
        checks.push(Check::pass("mode file is a regular file", ""));
        checks.push(Check::pass("mode file owned by you", format!("uid {uid}")));

        if mode.perms != 0o600 {
            checks.push(Check::fail(
                "mode file permissions are exactly 0600",
                format!(
                    "{mode_display} has {:04o}; the hook requires exactly 0600",
                    mode.perms
                ),
                format!("chmod 0600 '{mode_display}'"),
            ));
        } else {
            checks.push(Check::pass("mode file permissions are exactly 0600", ""));
        }

        match std::fs::read(&mode_path) {
            Ok(bytes) if bytes.len() as u64 > MODE_FILE_MAX_BYTES => {
                checks.push(Check::fail(
                    "mode file is <= 64 bytes",
                    format!(
                        "{mode_display} is {} bytes; the hook rejects anything larger",
                        bytes.len()
                    ),
                    "wisphive on  or:  wisphive off".to_string(),
                ));
            }
            Ok(bytes) => {
                checks.push(Check::pass(
                    "mode file is <= 64 bytes",
                    format!("{} bytes", bytes.len()),
                ));
                match std::str::from_utf8(&bytes).map(str::trim) {
                    Ok("active") => checks.push(Check::pass(
                        "mode file content is \"active\" or \"off\"",
                        "\"active\": gating enabled",
                    )),
                    Ok("off") => checks.push(Check::pass(
                        "mode file content is \"active\" or \"off\"",
                        "\"off\": gating disabled, hooks pass through",
                    )),
                    Ok(other) => checks.push(Check::fail(
                        "mode file content is \"active\" or \"off\"",
                        format!("content is {other:?}; the hook denies every event on any other value. Automatic repair never rewrites contents."),
                        "wisphive on  (enable)  or:  wisphive off  (disable)".to_string(),
                    )),
                    Err(_) => checks.push(Check::fail(
                        "mode file content is \"active\" or \"off\"",
                        format!("{mode_display} is not UTF-8"),
                        "wisphive on  (enable)  or:  wisphive off  (disable)".to_string(),
                    )),
                }
            }
            Err(error) => {
                checks.push(Check::fail(
                    "mode file is readable",
                    format!("reading {mode_display} failed: {error}"),
                    "wisphive on  or:  wisphive off".to_string(),
                ));
            }
        }
    }

    // ── SOFT: policy files (untrusted => silently ignored, safe defaults) ──
    for (file_name, label, exact_0600) in [
        ("config.json", "config.json", false),
        ("auto-approve.json", "auto-approve.json (legacy)", false),
        ("config.json.lock", "config.json.lock", true),
    ] {
        checks.extend(policy_file_checks(
            &wisphive_dir.join(file_name),
            label,
            exact_0600,
        ));
    }

    // ── INFO: fail-mode posture ──
    match std::fs::read_to_string(wisphive_dir.join("fail-mode")) {
        Err(_) => checks.push(Check::info(
            "fail-mode",
            "absent (defaults to closed: runtime hook errors deny)",
        )),
        Ok(contents) => match contents.trim() {
            value @ ("open" | "closed") => {
                checks.push(Check::info("fail-mode", format!("\"{value}\"")));
            }
            other => checks.push(Check::info(
                "fail-mode",
                format!("content {other:?} is not open/closed; treated as closed"),
            )),
        },
    }

    checks
}

/// Trust checks for one optional policy file, mirroring
/// `fs_trust::read_trusted`: regular non-symlink file, owned by the effective
/// user, and (unless `exact_0600`) not group/other-writable.
#[cfg(unix)]
fn policy_file_checks(path: &Path, label: &str, exact_0600: bool) -> Vec<Check> {
    let display = path.display();
    let facts = entry_facts(path, false);
    if !facts.exists {
        return vec![Check::info(
            format!("{label} trust"),
            "absent (fine: safe defaults apply)",
        )];
    }
    if facts.is_symlink {
        return vec![Check::tamper(
            format!("{label} is not a symlink"),
            format!("{display} is a symlink; the hook ignores it and falls back to safe defaults"),
        )];
    }
    if !facts.kind_ok {
        return vec![Check::tamper(
            format!("{label} is a regular file"),
            format!("{display} exists but is not a regular file; the hook ignores it"),
        )];
    }
    if !facts.owned {
        return vec![Check::tamper(
            format!("{label} owned by you"),
            format!(
                "{display} is not owned by your uid {} ; the hook ignores it (never chowned automatically)",
                effective_uid()
            ),
        )];
    }
    if exact_0600 {
        if facts.perms != 0o600 {
            return vec![Check::fail(
                format!("{label} permissions are 0600"),
                format!("{display} has {:04o}", facts.perms),
                format!("chmod 0600 '{display}'"),
            )];
        }
        return vec![Check::pass(format!("{label} permissions are 0600"), "")];
    }
    if facts.perms & 0o022 != 0 {
        return vec![Check::fail(
            format!("{label} not group/world-writable"),
            format!(
                "{display} has {:04o}; the hook ignores it and falls back to safe defaults",
                facts.perms
            ),
            format!("chmod go-w '{display}'"),
        )];
    }
    vec![Check::pass(
        format!("{label} not group/world-writable"),
        format!("{:04o}", facts.perms),
    )]
}

/// Apply the safe owner-only repairs for every failing, non-tamper check:
/// `chmod 0700` on the state dir, `chmod 0600` on mode / config.json.lock,
/// `chmod go-w` on policy files. Never loosens, never chowns, never creates
/// or rewrites the mode file, and refuses symlinks / foreign owners / wrong
/// types (tamper evidence). Every mutation is echoed in the outcome.
#[cfg(unix)]
pub fn fix_perms(wisphive_dir: &Path) -> FixOutcome {
    use std::os::unix::fs::PermissionsExt;

    let mut outcome = FixOutcome::default();

    let chmod = |path: &Path, mode: u32, outcome: &mut FixOutcome| match std::fs::set_permissions(
        path,
        std::fs::Permissions::from_mode(mode),
    ) {
        Ok(()) => outcome
            .applied
            .push(format!("chmod {mode:04o} '{}'", path.display())),
        Err(error) => outcome.refused.push(format!(
            "chmod {mode:04o} '{}' failed: {error}",
            path.display()
        )),
    };

    // State dir: tighten to 0700 only when it is a real, owned directory.
    let dir = entry_facts(wisphive_dir, true);
    if dir.exists && !dir.is_symlink && dir.kind_ok && dir.owned && dir.perms != 0o700 {
        chmod(wisphive_dir, 0o700, &mut outcome);
    } else if dir.exists && (dir.is_symlink || !dir.kind_ok || !dir.owned) {
        outcome.refused.push(format!(
            "'{}': symlink / foreign owner / not a directory — tamper evidence, inspect manually",
            wisphive_dir.display()
        ));
        return outcome; // nothing inside an untrusted dir is safe to touch
    }

    // Mode file: tighten to exactly 0600.
    let mode_path = wisphive_dir.join("mode");
    let mode = entry_facts(&mode_path, false);
    if mode.exists && !mode.is_symlink && mode.kind_ok && mode.owned && mode.perms != 0o600 {
        chmod(&mode_path, 0o600, &mut outcome);
    } else if mode.exists && (mode.is_symlink || !mode.kind_ok || !mode.owned) {
        outcome.refused.push(format!(
            "'{}': symlink / foreign owner / not a regular file — tamper evidence, inspect manually",
            mode_path.display()
        ));
    }

    // Policy files: remove group/other write bits (config.json,
    // auto-approve.json) or tighten to 0600 (config.json.lock).
    for (file_name, exact_0600) in [
        ("config.json", false),
        ("auto-approve.json", false),
        ("config.json.lock", true),
    ] {
        let path = wisphive_dir.join(file_name);
        let facts = entry_facts(&path, false);
        if !facts.exists {
            continue;
        }
        if facts.is_symlink || !facts.kind_ok || !facts.owned {
            outcome.refused.push(format!(
                "'{}': symlink / foreign owner / not a regular file — tamper evidence, inspect manually",
                path.display()
            ));
            continue;
        }
        if exact_0600 {
            if facts.perms != 0o600 {
                chmod(&path, 0o600, &mut outcome);
            }
        } else if facts.perms & 0o022 != 0 {
            // go-w: clear only the group/other write bits, never touch the rest.
            chmod(&path, facts.perms & !0o022, &mut outcome);
        }
    }

    outcome
}

/// Refuse a mode transition when the existing state is tamper-evidence class
/// (symlinked or foreign-owned dir/mode file). Returns the exact reason and
/// the manual step; safe states (missing entries, loose but owned perms)
/// return `Ok(())` because [`write_mode_file_atomic`] repairs those.
///
/// [`write_mode_file_atomic`]: wisphive_daemon::config::write_mode_file_atomic
#[cfg(unix)]
pub fn refuse_if_tampered(wisphive_dir: &Path) -> Result<(), String> {
    let dir = entry_facts(wisphive_dir, true);
    if dir.exists {
        if dir.is_symlink {
            return Err(format!(
                "{} is a symlink — tamper evidence. Inspect it, then `rm '{}'` and retry.",
                wisphive_dir.display(),
                wisphive_dir.display()
            ));
        }
        if !dir.kind_ok {
            return Err(format!(
                "{} exists but is not a directory — tamper evidence. Inspect and remove it manually.",
                wisphive_dir.display()
            ));
        }
        if !dir.owned {
            return Err(format!(
                "{} is not owned by your uid {} — tamper evidence. Never chowned automatically; inspect it manually.",
                wisphive_dir.display(),
                effective_uid()
            ));
        }
    }

    let mode_path = wisphive_dir.join("mode");
    let mode = entry_facts(&mode_path, false);
    if mode.exists {
        if mode.is_symlink {
            return Err(format!(
                "{} is a symlink — tamper evidence. Inspect it, then `rm '{}'` and retry.",
                mode_path.display(),
                mode_path.display()
            ));
        }
        if !mode.kind_ok {
            return Err(format!(
                "{} exists but is not a regular file — tamper evidence. Inspect and remove it manually.",
                mode_path.display()
            ));
        }
        if !mode.owned {
            return Err(format!(
                "{} is not owned by your uid {} — tamper evidence. Never chowned automatically; inspect it manually.",
                mode_path.display(),
                effective_uid()
            ));
        }
    }
    Ok(())
}

/// Non-Unix: the strict validators are Unix-descriptor semantics; report that
/// honestly instead of faking green.
#[cfg(not(unix))]
pub fn run_checks(_wisphive_dir: &Path) -> Vec<Check> {
    vec![Check::info(
        "strict state checks",
        "unsupported on non-Unix platforms (the hook fails closed there)",
    )]
}

#[cfg(not(unix))]
pub fn fix_perms(_wisphive_dir: &Path) -> FixOutcome {
    FixOutcome::default()
}

#[cfg(not(unix))]
pub fn refuse_if_tampered(_wisphive_dir: &Path) -> Result<(), String> {
    Ok(())
}

/// The wisphive home (`$HOME/.wisphive`), shared by doctor and on/off.
pub fn wisphive_home() -> PathBuf {
    let home = std::env::var("HOME").unwrap_or_else(|_| "/tmp".into());
    PathBuf::from(home).join(".wisphive")
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;
    use std::os::unix::fs::PermissionsExt;

    fn broken_fixture() -> tempfile::TempDir {
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path().join(".wisphive");
        std::fs::create_dir(&dir).unwrap();
        std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o755)).unwrap();
        std::fs::write(dir.join("mode"), "active").unwrap();
        std::fs::set_permissions(dir.join("mode"), std::fs::Permissions::from_mode(0o644)).unwrap();
        tmp
    }

    fn failing(checks: &[Check]) -> Vec<&Check> {
        checks
            .iter()
            .filter(|check| check.status == Status::Fail)
            .collect()
    }

    #[test]
    fn broken_perms_name_each_failing_check_with_exact_fix() {
        let tmp = broken_fixture();
        let dir = tmp.path().join(".wisphive");
        let checks = run_checks(&dir);
        let fails = failing(&checks);
        assert_eq!(fails.len(), 2, "{checks:?}");
        assert_eq!(fails[0].name, "state dir permissions are exactly 0700");
        assert_eq!(
            fails[0].fix.as_deref(),
            Some(format!("chmod 0700 '{}'", dir.display()).as_str())
        );
        assert_eq!(fails[1].name, "mode file permissions are exactly 0600");
        assert_eq!(
            fails[1].fix.as_deref(),
            Some(format!("chmod 0600 '{}'", dir.join("mode").display()).as_str())
        );
    }

    #[test]
    fn fix_perms_repairs_to_a_state_the_strict_mode_reader_accepts() {
        let tmp = broken_fixture();
        let dir = tmp.path().join(".wisphive");
        let mode_path = dir.join("mode");

        // The strict reader (same contract as the hook) rejects the broken state.
        assert!(wisphive_daemon::config::read_mode_file(&mode_path).is_err());

        let outcome = fix_perms(&dir);
        assert_eq!(outcome.applied.len(), 2, "{outcome:?}");
        assert!(outcome.refused.is_empty(), "{outcome:?}");
        assert!(outcome.applied[0].contains("chmod 0700"));
        assert!(outcome.applied[1].contains("chmod 0600"));

        assert!(failing(&run_checks(&dir)).is_empty());
        assert_eq!(
            wisphive_daemon::config::read_mode_file(&mode_path).unwrap(),
            wisphive_daemon::config::ModeFileState::Active
        );
    }

    #[test]
    fn symlinked_mode_file_is_refused_not_repaired() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path().join(".wisphive");
        std::fs::create_dir(&dir).unwrap();
        std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o700)).unwrap();
        let target = tmp.path().join("target");
        std::fs::write(&target, "active").unwrap();
        std::os::unix::fs::symlink(&target, dir.join("mode")).unwrap();

        let checks = run_checks(&dir);
        let fails = failing(&checks);
        assert_eq!(fails.len(), 1, "{checks:?}");
        assert!(fails[0].tamper);
        assert!(fails[0].detail.contains("symlink"));

        let outcome = fix_perms(&dir);
        assert!(outcome.applied.is_empty(), "{outcome:?}");
        assert_eq!(outcome.refused.len(), 1, "{outcome:?}");
        assert!(
            std::fs::symlink_metadata(dir.join("mode"))
                .unwrap()
                .file_type()
                .is_symlink()
        );

        assert!(refuse_if_tampered(&dir).is_err());
    }

    #[test]
    fn missing_mode_file_is_reported_but_never_created_by_fix() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path().join(".wisphive");
        std::fs::create_dir(&dir).unwrap();
        std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o700)).unwrap();

        let checks = run_checks(&dir);
        let fails = failing(&checks);
        assert_eq!(fails.len(), 1, "{checks:?}");
        assert_eq!(fails[0].name, "mode file exists");
        assert!(fails[0].fix.as_deref().unwrap().contains("wisphive on"));

        let outcome = fix_perms(&dir);
        assert!(outcome.applied.is_empty(), "{outcome:?}");
        assert!(!dir.join("mode").exists());
    }

    #[test]
    fn group_writable_config_fails_soft_and_fix_only_clears_write_bits() {
        let tmp = broken_fixture();
        let dir = tmp.path().join(".wisphive");
        let config = dir.join("config.json");
        std::fs::write(&config, "{}").unwrap();
        std::fs::set_permissions(&config, std::fs::Permissions::from_mode(0o664)).unwrap();

        let checks = run_checks(&dir);
        assert!(checks.iter().any(|check| {
            check.status == Status::Fail && check.name == "config.json not group/world-writable"
        }));

        let outcome = fix_perms(&dir);
        assert!(
            outcome
                .applied
                .iter()
                .any(|entry| entry.contains("config.json"))
        );
        let perms = std::fs::metadata(&config).unwrap().permissions().mode() & 0o7777;
        assert_eq!(perms, 0o644, "only write bits cleared, read kept");
    }

    #[test]
    fn valid_off_state_passes_all_checks() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path().join(".wisphive");
        std::fs::create_dir(&dir).unwrap();
        std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o700)).unwrap();
        std::fs::write(dir.join("mode"), "off").unwrap();
        std::fs::set_permissions(dir.join("mode"), std::fs::Permissions::from_mode(0o600)).unwrap();

        assert!(failing(&run_checks(&dir)).is_empty());
    }
}
