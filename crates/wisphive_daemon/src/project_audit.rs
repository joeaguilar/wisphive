use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

pub const CLAUDE_HOOK_EVENTS: &[&str] = &[
    "PreToolUse",
    "PostToolUse",
    "PermissionRequest",
    "Elicitation",
    "UserPromptSubmit",
    "Stop",
    "SubagentStop",
    "ConfigChange",
    "TeammateIdle",
    "TaskCompleted",
];

pub const CODEX_HOOK_EVENTS: &[&str] = &[
    "PreToolUse",
    "PostToolUse",
    "PermissionRequest",
    "UserPromptSubmit",
    "Stop",
];

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProjectAudit {
    pub project_dir: PathBuf,
    pub claude_dir_present: bool,
    pub codex_dir_present: bool,
    pub custom_claude_skills_count: usize,
    pub claude_md_present: bool,
    pub agents_md_present: bool,
    pub hooks: ProjectHooksAudit,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProjectHooksAudit {
    pub mode: HookMode,
    pub claude: AgentHookAudit,
    pub codex: AgentHookAudit,
    pub all_installed: bool,
    pub all_enabled: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum HookMode {
    Active,
    Off,
    Missing,
    Invalid(String),
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum HookAgent {
    ClaudeCode,
    Codex,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AgentHookAudit {
    pub agent: HookAgent,
    pub config_path: PathBuf,
    pub config_present: bool,
    pub config_valid: bool,
    pub read_error: Option<String>,
    pub parse_error: Option<String>,
    pub installed_events: Vec<String>,
    pub missing_events: Vec<String>,
    pub installed: bool,
    pub enabled: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProjectDirectoryEntry {
    pub name: String,
    pub path: PathBuf,
    pub kind: ProjectDirectoryEntryKind,
    pub hidden: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ProjectDirectoryEntryKind {
    Directory,
    File,
    Symlink,
    Other,
}

impl ProjectAudit {
    pub fn scan(project_dir: impl AsRef<Path>) -> Self {
        audit_project(project_dir)
    }

    pub fn scan_with_home(project_dir: impl AsRef<Path>, wisphive_home: impl AsRef<Path>) -> Self {
        audit_project_with_home(project_dir, wisphive_home)
    }
}

impl HookMode {
    pub fn is_enabled(&self) -> bool {
        matches!(self, Self::Active)
    }
}

pub fn audit_project(project_dir: impl AsRef<Path>) -> ProjectAudit {
    audit_project_with_home(project_dir, default_wisphive_home())
}

pub fn list_project_directory(
    path: impl AsRef<Path>,
) -> std::io::Result<Vec<ProjectDirectoryEntry>> {
    let mut entries = Vec::new();

    for entry in std::fs::read_dir(path)? {
        let entry = entry?;
        let file_type = entry.file_type()?;
        let name = entry.file_name().to_string_lossy().to_string();
        let kind = if file_type.is_dir() {
            ProjectDirectoryEntryKind::Directory
        } else if file_type.is_file() {
            ProjectDirectoryEntryKind::File
        } else if file_type.is_symlink() {
            ProjectDirectoryEntryKind::Symlink
        } else {
            ProjectDirectoryEntryKind::Other
        };

        entries.push(ProjectDirectoryEntry {
            hidden: name.starts_with('.'),
            name,
            path: entry.path(),
            kind,
        });
    }

    entries.sort_by(|left, right| {
        left.kind
            .cmp(&right.kind)
            .then_with(|| left.name.to_lowercase().cmp(&right.name.to_lowercase()))
            .then_with(|| left.name.cmp(&right.name))
    });

    Ok(entries)
}

pub fn audit_project_with_home(
    project_dir: impl AsRef<Path>,
    wisphive_home: impl AsRef<Path>,
) -> ProjectAudit {
    let project_dir = project_dir.as_ref().to_path_buf();
    let mode = read_hook_mode(wisphive_home.as_ref());

    let claude = audit_agent_hooks(
        HookAgent::ClaudeCode,
        project_dir.join(".claude").join("settings.json"),
        CLAUDE_HOOK_EVENTS,
        &mode,
    );
    let codex = audit_agent_hooks(
        HookAgent::Codex,
        project_dir.join(".codex").join("hooks.json"),
        CODEX_HOOK_EVENTS,
        &mode,
    );

    let all_installed = claude.installed && codex.installed;
    let all_enabled = claude.enabled && codex.enabled;

    ProjectAudit {
        claude_dir_present: project_dir.join(".claude").is_dir(),
        codex_dir_present: project_dir.join(".codex").is_dir(),
        custom_claude_skills_count: count_dir_entries(&project_dir.join(".claude").join("skills")),
        claude_md_present: project_dir.join("CLAUDE.md").is_file(),
        agents_md_present: project_dir.join("AGENTS.md").is_file(),
        hooks: ProjectHooksAudit {
            mode,
            claude,
            codex,
            all_installed,
            all_enabled,
        },
        project_dir,
    }
}

fn audit_agent_hooks(
    agent: HookAgent,
    config_path: PathBuf,
    expected_events: &[&str],
    mode: &HookMode,
) -> AgentHookAudit {
    let config_present = config_path.is_file();
    let mut audit = AgentHookAudit {
        agent,
        config_path,
        config_present,
        config_valid: false,
        read_error: None,
        parse_error: None,
        installed_events: Vec::new(),
        missing_events: expected_events
            .iter()
            .map(|event| event.to_string())
            .collect(),
        installed: false,
        enabled: false,
    };

    if !config_present {
        return audit;
    }

    let content = match std::fs::read_to_string(&audit.config_path) {
        Ok(content) => content,
        Err(err) => {
            audit.read_error = Some(err.to_string());
            return audit;
        }
    };

    let settings = match serde_json::from_str::<serde_json::Value>(&content) {
        Ok(settings) => settings,
        Err(err) => {
            audit.parse_error = Some(err.to_string());
            return audit;
        }
    };

    audit.config_valid = true;
    audit.installed_events = expected_events
        .iter()
        .filter(|event| has_wisphive_hook_for_event(&settings, event))
        .map(|event| event.to_string())
        .collect();
    audit.missing_events = expected_events
        .iter()
        .filter(|event| {
            !audit
                .installed_events
                .iter()
                .any(|installed| installed == **event)
        })
        .map(|event| event.to_string())
        .collect();
    audit.installed = audit.missing_events.is_empty();
    audit.enabled = audit.installed && mode.is_enabled();
    audit
}

fn has_wisphive_hook_for_event(settings: &serde_json::Value, event: &str) -> bool {
    settings
        .get("hooks")
        .and_then(|hooks| hooks.get(event))
        .and_then(|entries| entries.as_array())
        .is_some_and(|entries| entries.iter().any(has_wisphive_hook))
}

fn has_wisphive_hook(rule: &serde_json::Value) -> bool {
    if let Some(hooks_arr) = rule.get("hooks").and_then(|hooks| hooks.as_array()) {
        return hooks_arr.iter().any(hook_command_mentions_wisphive);
    }

    hook_command_mentions_wisphive(rule)
}

fn hook_command_mentions_wisphive(hook: &serde_json::Value) -> bool {
    hook.get("command")
        .and_then(|command| command.as_str())
        .is_some_and(|command| command.contains("wisphive"))
}

fn read_hook_mode(wisphive_home: &Path) -> HookMode {
    match std::fs::read_to_string(wisphive_home.join("mode")) {
        Ok(mode) => match mode.trim() {
            "active" => HookMode::Active,
            "off" => HookMode::Off,
            other => HookMode::Invalid(other.to_string()),
        },
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => HookMode::Missing,
        Err(err) => HookMode::Invalid(err.to_string()),
    }
}

fn count_dir_entries(path: &Path) -> usize {
    std::fs::read_dir(path)
        .ok()
        .into_iter()
        .flat_map(|entries| entries.filter_map(Result::ok))
        .count()
}

fn default_wisphive_home() -> PathBuf {
    std::env::var_os("HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("/tmp"))
        .join(".wisphive")
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use std::fs;

    fn write_json(path: &Path, value: &serde_json::Value) {
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, serde_json::to_string_pretty(value).unwrap()).unwrap();
    }

    fn hook_settings(events: &[&str], command: &str) -> serde_json::Value {
        let mut hooks = serde_json::Map::new();
        for event in events {
            hooks.insert(
                event.to_string(),
                json!([{
                    "matcher": "",
                    "hooks": [{
                        "type": "command",
                        "command": command
                    }]
                }]),
            );
        }
        json!({ "hooks": hooks })
    }

    #[test]
    fn empty_project_reports_missing_items_without_writing() {
        let project = tempfile::tempdir().unwrap();
        let home = tempfile::tempdir().unwrap();

        let audit = ProjectAudit::scan_with_home(project.path(), home.path());

        assert!(!audit.claude_dir_present);
        assert!(!audit.codex_dir_present);
        assert_eq!(audit.custom_claude_skills_count, 0);
        assert!(!audit.claude_md_present);
        assert!(!audit.agents_md_present);
        assert_eq!(audit.hooks.mode, HookMode::Missing);
        assert!(!audit.hooks.all_installed);
        assert!(!audit.hooks.all_enabled);
        assert!(!audit.hooks.claude.config_present);
        assert!(!audit.hooks.claude.installed);
        assert_eq!(
            audit.hooks.claude.missing_events.len(),
            CLAUDE_HOOK_EVENTS.len()
        );
        assert!(!project.path().join(".claude").exists());
        assert!(!project.path().join(".codex").exists());
    }

    #[test]
    fn partial_project_counts_skills_and_reports_missing_hook_events() {
        let project = tempfile::tempdir().unwrap();
        let home = tempfile::tempdir().unwrap();

        fs::write(home.path().join("mode"), "off").unwrap();
        fs::create_dir_all(project.path().join(".claude").join("skills")).unwrap();
        fs::write(
            project.path().join(".claude").join("skills").join("one.md"),
            "",
        )
        .unwrap();
        fs::create_dir(project.path().join(".claude").join("skills").join("two")).unwrap();
        fs::create_dir(project.path().join(".codex")).unwrap();
        fs::write(project.path().join("CLAUDE.md"), "# Claude\n").unwrap();
        write_json(
            &project.path().join(".claude").join("settings.json"),
            &hook_settings(&["PreToolUse"], "wisphive-hook"),
        );

        let audit = ProjectAudit::scan_with_home(project.path(), home.path());

        assert!(audit.claude_dir_present);
        assert!(audit.codex_dir_present);
        assert!(audit.claude_md_present);
        assert!(!audit.agents_md_present);
        assert_eq!(audit.custom_claude_skills_count, 2);
        assert_eq!(audit.hooks.mode, HookMode::Off);
        assert_eq!(audit.hooks.claude.installed_events, vec!["PreToolUse"]);
        assert_eq!(
            audit.hooks.claude.missing_events.len(),
            CLAUDE_HOOK_EVENTS.len() - 1
        );
        assert!(!audit.hooks.claude.installed);
        assert!(!audit.hooks.claude.enabled);
        assert!(!audit.hooks.codex.config_present);
        assert!(!audit.hooks.all_installed);
    }

    #[test]
    fn full_project_reports_all_hooks_installed_and_enabled() {
        let project = tempfile::tempdir().unwrap();
        let home = tempfile::tempdir().unwrap();

        fs::write(home.path().join("mode"), "active\n").unwrap();
        fs::create_dir_all(project.path().join(".claude")).unwrap();
        fs::create_dir_all(project.path().join(".codex")).unwrap();
        fs::write(project.path().join("CLAUDE.md"), "# Claude\n").unwrap();
        fs::write(project.path().join("AGENTS.md"), "# Agents\n").unwrap();
        write_json(
            &project.path().join(".claude").join("settings.json"),
            &hook_settings(CLAUDE_HOOK_EVENTS, "/usr/local/bin/wisphive-hook"),
        );
        write_json(
            &project.path().join(".codex").join("hooks.json"),
            &hook_settings(
                CODEX_HOOK_EVENTS,
                "env WISPHIVE_AGENT_TYPE=codex wisphive-hook",
            ),
        );

        let audit = ProjectAudit::scan_with_home(project.path(), home.path());

        assert!(audit.claude_dir_present);
        assert!(audit.codex_dir_present);
        assert!(audit.claude_md_present);
        assert!(audit.agents_md_present);
        assert_eq!(audit.hooks.mode, HookMode::Active);
        assert!(audit.hooks.claude.config_valid);
        assert!(audit.hooks.codex.config_valid);
        assert!(audit.hooks.claude.installed);
        assert!(audit.hooks.codex.installed);
        assert!(audit.hooks.claude.enabled);
        assert!(audit.hooks.codex.enabled);
        assert!(audit.hooks.all_installed);
        assert!(audit.hooks.all_enabled);
    }

    #[test]
    fn malformed_hook_config_is_reported_inside_audit() {
        let project = tempfile::tempdir().unwrap();
        let home = tempfile::tempdir().unwrap();

        fs::create_dir_all(project.path().join(".codex")).unwrap();
        fs::write(project.path().join(".codex").join("hooks.json"), "{").unwrap();

        let audit = ProjectAudit::scan_with_home(project.path(), home.path());

        assert!(audit.hooks.codex.config_present);
        assert!(!audit.hooks.codex.config_valid);
        assert!(audit.hooks.codex.parse_error.is_some());
        assert!(!audit.hooks.codex.installed);
        assert!(!audit.hooks.codex.enabled);
    }

    #[test]
    fn list_project_directory_sorts_dirs_before_files_and_marks_hidden() {
        let project = tempfile::tempdir().unwrap();

        fs::write(project.path().join("zeta.txt"), "").unwrap();
        fs::create_dir(project.path().join("Alpha")).unwrap();
        fs::write(project.path().join(".env"), "").unwrap();
        fs::create_dir(project.path().join("beta")).unwrap();

        let entries = list_project_directory(project.path()).unwrap();
        let names: Vec<&str> = entries.iter().map(|entry| entry.name.as_str()).collect();

        assert_eq!(names, vec!["Alpha", "beta", ".env", "zeta.txt"]);
        assert_eq!(entries[0].kind, ProjectDirectoryEntryKind::Directory);
        assert_eq!(entries[1].kind, ProjectDirectoryEntryKind::Directory);
        assert_eq!(entries[2].kind, ProjectDirectoryEntryKind::File);
        assert!(entries[2].hidden);
        assert!(!entries[3].hidden);
    }
}
