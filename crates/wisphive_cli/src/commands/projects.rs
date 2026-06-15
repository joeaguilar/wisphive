use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use wisphive_daemon::project_audit::{
    AgentHookAudit, HookMode, ProjectAudit, ProjectDirectoryEntryKind, audit_project,
    list_project_directory,
};

const SEED_SCRIPT: &str = "scripts/wisphive-project-seed.sh";

pub fn audit(path: PathBuf, json: bool) -> Result<()> {
    let path = require_directory(path)?;
    let audit = audit_project(&path);

    if json {
        println!("{}", serde_json::to_string_pretty(&audit)?);
    } else {
        print_audit(&audit);
    }

    Ok(())
}

pub fn list(path: PathBuf, json: bool) -> Result<()> {
    let path = require_directory(path)?;
    let entries = list_project_directory(&path)
        .with_context(|| format!("failed to list directory {}", path.display()))?;

    if json {
        println!("{}", serde_json::to_string_pretty(&entries)?);
    } else {
        println!("Directory: {}", path.display());
        for entry in entries {
            println!("  {:<7} {}", directory_kind_label(entry.kind), entry.name);
        }
    }

    Ok(())
}

pub fn seed(path: PathBuf) -> Result<()> {
    let path = require_directory(path)?;
    let script = find_seed_script(&path).with_context(|| {
        format!(
            "could not find {SEED_SCRIPT}; run from a Wisphive checkout or set WISPHIVE_PROJECT_SEED_SCRIPT"
        )
    })?;
    let wisphive_bin =
        std::env::current_exe().context("failed to resolve current wisphive binary path")?;

    let status = std::process::Command::new(&script)
        .arg(&path)
        .env("WISPHIVE_BIN", wisphive_bin)
        .status()
        .with_context(|| format!("failed to run seed script {}", script.display()))?;

    if !status.success() {
        anyhow::bail!(
            "seed script {} exited with status {}",
            script.display(),
            status
        );
    }

    Ok(())
}

fn print_audit(audit: &ProjectAudit) {
    println!("Project: {}", audit.project_dir.display());
    println!("AI config:");
    println!("  {:<8} .claude/", present_label(audit.claude_dir_present));
    println!("  {:<8} .codex/", present_label(audit.codex_dir_present));
    println!("  {:<8} CLAUDE.md", present_label(audit.claude_md_present));
    println!("  {:<8} AGENTS.md", present_label(audit.agents_md_present));
    println!(
        "  count    .claude/skills/ entries: {}",
        audit.custom_claude_skills_count
    );
    println!("Hooks:");
    println!("  mode     {}", hook_mode_label(&audit.hooks.mode));
    print_agent_hooks("Claude Code", &audit.hooks.claude);
    print_agent_hooks("Codex", &audit.hooks.codex);
    println!(
        "  summary  installed={} enabled={}",
        audit.hooks.all_installed, audit.hooks.all_enabled
    );
}

fn print_agent_hooks(name: &str, audit: &AgentHookAudit) {
    let installed = audit.installed_events.len();
    let total = installed + audit.missing_events.len();
    let status = if audit.enabled {
        "enabled"
    } else if audit.installed {
        "installed"
    } else if !audit.config_present {
        "missing"
    } else if !audit.config_valid {
        "invalid"
    } else {
        "partial"
    };

    println!("  {:<8} {name} hooks ({installed}/{total})", status);
    if !audit.config_valid
        && let Some(error) = audit.read_error.as_ref().or(audit.parse_error.as_ref())
    {
        println!("           error: {error}");
    }
    if !audit.missing_events.is_empty() && audit.config_valid {
        println!("           missing: {}", audit.missing_events.join(", "));
    }
}

fn present_label(present: bool) -> &'static str {
    if present { "present" } else { "missing" }
}

fn hook_mode_label(mode: &HookMode) -> String {
    match mode {
        HookMode::Active => "active".into(),
        HookMode::Off => "off".into(),
        HookMode::Missing => "missing".into(),
        HookMode::Invalid(value) => format!("invalid: {value}"),
    }
}

fn directory_kind_label(kind: ProjectDirectoryEntryKind) -> &'static str {
    match kind {
        ProjectDirectoryEntryKind::Directory => "dir",
        ProjectDirectoryEntryKind::File => "file",
        ProjectDirectoryEntryKind::Symlink => "link",
        ProjectDirectoryEntryKind::Other => "other",
    }
}

fn require_directory(path: PathBuf) -> Result<PathBuf> {
    if path.is_dir() {
        Ok(path)
    } else {
        anyhow::bail!("not a directory: {}", path.display());
    }
}

fn find_seed_script(project: &Path) -> Option<PathBuf> {
    if let Some(path) = std::env::var_os("WISPHIVE_PROJECT_SEED_SCRIPT") {
        let path = PathBuf::from(path);
        if path.is_file() {
            return Some(path);
        }
    }

    if let Some(path) = find_seed_script_from_ancestors(project) {
        return Some(path);
    }

    if let Ok(current_dir) = std::env::current_dir()
        && let Some(path) = find_seed_script_from_ancestors(&current_dir)
    {
        return Some(path);
    }

    let manifest_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    manifest_dir
        .parent()
        .and_then(Path::parent)
        .map(|repo_root| repo_root.join(SEED_SCRIPT))
        .filter(|path| path.is_file())
}

fn find_seed_script_from_ancestors(start: &Path) -> Option<PathBuf> {
    start
        .ancestors()
        .map(|ancestor| ancestor.join(SEED_SCRIPT))
        .find(|path| path.is_file())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn directory_kind_labels_are_stable() {
        assert_eq!(
            directory_kind_label(ProjectDirectoryEntryKind::Directory),
            "dir"
        );
        assert_eq!(
            directory_kind_label(ProjectDirectoryEntryKind::File),
            "file"
        );
        assert_eq!(
            directory_kind_label(ProjectDirectoryEntryKind::Symlink),
            "link"
        );
        assert_eq!(
            directory_kind_label(ProjectDirectoryEntryKind::Other),
            "other"
        );
    }

    #[test]
    fn require_directory_rejects_missing_path() {
        let path = tempfile::tempdir().unwrap().path().join("missing");
        assert!(require_directory(path).is_err());
    }
}
