//! Read-only working-tree probe for the Command Center working-tree strip
//! (itr#401, spec §5.3).
//!
//! # Hard product constraint: state mirror, zero write affordances
//!
//! The strip mirrors git state — it never mutates it ("you own git": the
//! pre-generated commit message is text the human copies; committing happens
//! outside wisphive). This module therefore runs ONLY non-mutating git
//! subcommands, enforced structurally by [`READ_ONLY_SUBCOMMANDS`]: any
//! attempt to spawn a subcommand outside the allowlist is refused at runtime
//! (and covered by a test), so a future edit can't quietly grow a write path.
//!
//! Additionally every git invocation sets `GIT_OPTIONAL_LOCKS=0`: without it,
//! `git status` may opportunistically refresh and rewrite `.git/index`, which
//! would make the "read-only" claim a lie at the filesystem level.
//!
//! # Attribution
//!
//! Changed paths are attributed to agents from the decision audit stream
//! (`decision_log`, fed by `events.jsonl` ingest — itr#397): the most recent
//! approved `Edit`/`Write`/`MultiEdit`/`NotebookEdit` call whose `file_path`
//! (or `notebook_path`) resolves to the changed file wins; failing that, the
//! most recent approved `Bash` call whose command string mentions the path
//! (a documented substring heuristic — command text is free-form). No match
//! means "human/unknown".

use std::path::{Path, PathBuf};
use std::time::Duration;

use tracing::warn;
use wisphive_protocol::{WorktreeChange, WorktreeStatus};

/// Only these git subcommands may be spawned by this module. All are
/// non-mutating. `status` additionally runs under `GIT_OPTIONAL_LOCKS=0` so it
/// cannot even do its optimistic index refresh.
const READ_ONLY_SUBCOMMANDS: &[&str] = &["status", "diff", "rev-parse", "log"];

/// Per-git-command wall-clock budget. A wedged repo (dead network FS, huge
/// index) degrades to an error entry instead of stalling the query.
const GIT_TIMEOUT: Duration = Duration::from_secs(10);

/// Cap on parsed change entries per repo; the flag `changes_truncated` tells
/// the UI more exist. Generated commit messages are useless for thousand-file
/// trees anyway, and the wire frame must stay bounded.
pub const MAX_CHANGES: usize = 400;

/// Cap on bytes of git stdout we will parse (a runaway status can't balloon
/// daemon memory).
const MAX_OUTPUT_BYTES: usize = 4 * 1024 * 1024;

/// One decision-log row that may attribute a file change to an agent.
/// Ordered most-recent-first by the caller.
#[derive(Debug, Clone)]
pub struct FileTouch {
    pub agent_id: String,
    pub tool_name: String,
    pub tool_input: serde_json::Value,
}

/// Run one allowlisted read-only git subcommand in `project`.
async fn run_git(project: &Path, args: &[&str]) -> Result<std::process::Output, String> {
    let sub = args.first().copied().unwrap_or("");
    if !READ_ONLY_SUBCOMMANDS.contains(&sub) {
        // Structural enforcement of the read-only constraint (see module docs).
        return Err(format!(
            "refusing non-allowlisted git subcommand {sub:?} — worktree probe is read-only"
        ));
    }
    let fut = tokio::process::Command::new("git")
        .arg("-C")
        .arg(project)
        .args(args)
        .env("GIT_OPTIONAL_LOCKS", "0")
        .env("GIT_TERMINAL_PROMPT", "0")
        .stdin(std::process::Stdio::null())
        .kill_on_drop(true)
        .output();
    match tokio::time::timeout(GIT_TIMEOUT, fut).await {
        Err(_) => Err(format!("git {sub} timed out after {GIT_TIMEOUT:?}")),
        Ok(Err(e)) => Err(format!("failed to spawn git: {e}")),
        Ok(Ok(out)) => Ok(out),
    }
}

/// Probe one project directory. Never fails: non-git dirs, missing dirs, a
/// missing git binary, and timeouts all come back as a `WorktreeStatus` with
/// `is_git_repo: false` and/or `error` set.
pub async fn probe_worktree(project: &Path) -> WorktreeStatus {
    let mut status = WorktreeStatus {
        project: project.to_path_buf(),
        is_git_repo: false,
        branch: None,
        detached: false,
        head: None,
        upstream: None,
        ahead: None,
        behind: None,
        changes: Vec::new(),
        changes_truncated: false,
        diffstat: None,
        probed_at: chrono::Utc::now(),
        error: None,
    };

    if !project.is_dir() {
        status.error = Some("project directory does not exist".to_string());
        return status;
    }

    match run_git(
        project,
        &[
            "status",
            "--porcelain=v2",
            "--branch",
            "-z",
            "--untracked-files=all",
        ],
    )
    .await
    {
        Err(e) => {
            warn!(project = %project.display(), error = %e, "worktree probe failed");
            status.error = Some(e);
        }
        Ok(out) if !out.status.success() => {
            // The common case is "not a git repository" — that's a state, not
            // an error. Anything else is surfaced (trimmed) for the operator.
            let stderr = String::from_utf8_lossy(&out.stderr);
            if !stderr.contains("not a git repository") {
                status.error = Some(truncate_line(stderr.trim(), 300));
            }
        }
        Ok(out) => {
            status.is_git_repo = true;
            let stdout = &out.stdout[..out.stdout.len().min(MAX_OUTPUT_BYTES)];
            parse_porcelain_v2(stdout, &mut status);
            if out.stdout.len() > MAX_OUTPUT_BYTES {
                status.changes_truncated = true;
            }
        }
    }

    // Tracked-change shortstat one-liner (spec: "one-line diff summary").
    // Fails harmlessly on an unborn HEAD (no commits yet) — leave None.
    if status.is_git_repo
        && !status.changes.is_empty()
        && let Ok(out) = run_git(project, &["diff", "HEAD", "--shortstat"]).await
        && out.status.success()
    {
        let line = String::from_utf8_lossy(&out.stdout).trim().to_string();
        if !line.is_empty() {
            status.diffstat = Some(truncate_line(&line, 200));
        }
    }

    status
}

fn truncate_line(s: &str, max: usize) -> String {
    if s.len() <= max {
        return s.to_string();
    }
    let mut end = max;
    while !s.is_char_boundary(end) {
        end -= 1;
    }
    format!("{}…", &s[..end])
}

/// Parse `git status --porcelain=v2 --branch -z` output.
///
/// With `-z`, records are NUL-terminated; a rename/copy record (`2 …`) is
/// followed by ONE extra NUL-terminated token: the original path.
fn parse_porcelain_v2(stdout: &[u8], status: &mut WorktreeStatus) {
    let text = String::from_utf8_lossy(stdout);
    let mut tokens = text.split('\0').peekable();

    while let Some(tok) = tokens.next() {
        if tok.is_empty() {
            continue;
        }
        if let Some(header) = tok.strip_prefix("# ") {
            parse_branch_header(header, status);
            continue;
        }
        if status.changes.len() >= MAX_CHANGES {
            status.changes_truncated = true;
            // Keep draining so rename orig-path tokens stay paired, but a
            // simple break is fine: we no longer read any tokens after this.
            break;
        }
        let mut fields = tok.splitn(2, ' ');
        let kind = fields.next().unwrap_or("");
        let rest = fields.next().unwrap_or("");
        match kind {
            // `1 XY sub mH mI mW hH hI path` — ordinary change
            "1" => {
                if let Some((xy, path)) = split_entry(rest, 6) {
                    status.changes.push(WorktreeChange {
                        path: path.to_string(),
                        status: xy,
                        orig_path: None,
                        attributed_to: None,
                        attributed_tool: None,
                    });
                }
            }
            // `2 XY sub mH mI mW hH hI Xscore path` NUL origpath
            "2" => {
                let orig = tokens.next().map(str::to_string);
                if let Some((xy, path)) = split_entry(rest, 7) {
                    status.changes.push(WorktreeChange {
                        path: path.to_string(),
                        status: xy,
                        orig_path: orig,
                        attributed_to: None,
                        attributed_tool: None,
                    });
                }
            }
            // `u XY sub m1 m2 m3 mW h1 h2 h3 path` — unmerged
            "u" => {
                if let Some((xy, path)) = split_entry(rest, 8) {
                    status.changes.push(WorktreeChange {
                        path: path.to_string(),
                        status: xy,
                        orig_path: None,
                        attributed_to: None,
                        attributed_tool: None,
                    });
                }
            }
            // `? path` — untracked
            "?" => {
                status.changes.push(WorktreeChange {
                    path: rest.to_string(),
                    status: "??".to_string(),
                    orig_path: None,
                    attributed_to: None,
                    attributed_tool: None,
                });
            }
            // `! path` — ignored entries (not requested, but be tolerant)
            _ => {}
        }
    }
}

/// Split a porcelain v2 entry remainder into (XY, path): the first field is
/// the two-character XY code, then `skip` further space-separated fields
/// precede the path (which may itself contain spaces — `splitn` leaves the
/// remainder intact).
fn split_entry(rest: &str, skip: usize) -> Option<(String, String)> {
    let mut parts = rest.splitn(skip + 2, ' ');
    let xy = parts.next()?.to_string();
    for _ in 0..skip {
        parts.next()?;
    }
    let path = parts.next()?;
    if xy.len() != 2 || path.is_empty() {
        return None;
    }
    Some((xy, path.to_string()))
}

fn parse_branch_header(header: &str, status: &mut WorktreeStatus) {
    if let Some(oid) = header.strip_prefix("branch.oid ") {
        if oid != "(initial)" {
            status.head = Some(oid.to_string());
        }
    } else if let Some(head) = header.strip_prefix("branch.head ") {
        if head == "(detached)" {
            status.detached = true;
        } else {
            status.branch = Some(head.to_string());
        }
    } else if let Some(up) = header.strip_prefix("branch.upstream ") {
        status.upstream = Some(up.to_string());
    } else if let Some(ab) = header.strip_prefix("branch.ab ") {
        for part in ab.split(' ') {
            if let Some(a) = part.strip_prefix('+') {
                status.ahead = a.parse().ok();
            } else if let Some(b) = part.strip_prefix('-') {
                status.behind = b.parse().ok();
            }
        }
    }
}

/// Attribute changed paths to agents from decision-log file touches
/// (most-recent-first). See module docs for the matching rules.
pub fn attribute_changes(project: &Path, changes: &mut [WorktreeChange], touches: &[FileTouch]) {
    for change in changes.iter_mut() {
        let abs: PathBuf = project.join(&change.path);
        let abs_str = abs.to_string_lossy();
        for touch in touches {
            let matched = match touch.tool_name.as_str() {
                "Edit" | "Write" | "MultiEdit" | "NotebookEdit" => {
                    let candidate = touch
                        .tool_input
                        .get("file_path")
                        .or_else(|| touch.tool_input.get("notebook_path"))
                        .and_then(|v| v.as_str());
                    match candidate {
                        Some(p) => p == abs_str || p == change.path,
                        None => false,
                    }
                }
                "Bash" => {
                    // Heuristic: the command string mentions the file. Guard
                    // against trivially-short relative paths over-matching.
                    match touch.tool_input.get("command").and_then(|v| v.as_str()) {
                        Some(cmd) => {
                            cmd.contains(abs_str.as_ref())
                                || (change.path.len() >= 4 && cmd.contains(&change.path))
                        }
                        None => false,
                    }
                }
                _ => false,
            };
            if matched {
                change.attributed_to = Some(touch.agent_id.clone());
                change.attributed_tool = Some(touch.tool_name.clone());
                break; // most recent touch wins
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::process::Command;

    /// Run git in a fixture repo (tests may mutate their own tempdir fixtures;
    /// the production probe path stays read-only).
    fn git(dir: &Path, args: &[&str]) {
        let out = Command::new("git")
            .arg("-C")
            .arg(dir)
            .args([
                "-c",
                "user.email=fixture@test",
                "-c",
                "user.name=Fixture",
                "-c",
                "commit.gpgsign=false",
                // Neutralize any developer-global hooks (commit-msg linters
                // etc.) so fixture commits behave identically everywhere.
                "-c",
                "core.hooksPath=/dev/null",
            ])
            .args(args)
            .output()
            .expect("spawn git");
        assert!(
            out.status.success(),
            "git {:?} failed: {}",
            args,
            String::from_utf8_lossy(&out.stderr)
        );
    }

    fn init_repo(dir: &Path) {
        git(dir, &["init", "--initial-branch=main"]);
        std::fs::write(dir.join("README.md"), "hello\n").unwrap();
        std::fs::create_dir_all(dir.join("src")).unwrap();
        std::fs::write(dir.join("src/lib.rs"), "pub fn a() {}\n").unwrap();
        git(dir, &["add", "-A"]);
        git(dir, &["commit", "-m", "chore: initial fixture commit"]);
    }

    #[tokio::test]
    async fn non_git_dir_reports_not_a_repo() {
        let tmp = tempfile::tempdir().unwrap();
        let wt = probe_worktree(tmp.path()).await;
        assert!(!wt.is_git_repo);
        assert!(wt.changes.is_empty());
        assert!(wt.error.is_none(), "not-a-repo is a state, not an error");
    }

    #[tokio::test]
    async fn missing_dir_reports_error() {
        let wt = probe_worktree(Path::new("/nonexistent/wisphive-worktree-test")).await;
        assert!(!wt.is_git_repo);
        assert_eq!(
            wt.error.as_deref(),
            Some("project directory does not exist")
        );
    }

    #[tokio::test]
    async fn clean_repo_has_branch_and_no_changes() {
        let tmp = tempfile::tempdir().unwrap();
        init_repo(tmp.path());
        let wt = probe_worktree(tmp.path()).await;
        assert!(wt.is_git_repo);
        assert_eq!(wt.branch.as_deref(), Some("main"));
        assert!(!wt.detached);
        assert!(wt.head.is_some());
        assert!(wt.changes.is_empty());
        assert!(!wt.changes_truncated);
        assert!(wt.diffstat.is_none());
    }

    #[tokio::test]
    async fn dirty_repo_lists_modified_untracked_staged_and_renamed() {
        let tmp = tempfile::tempdir().unwrap();
        init_repo(tmp.path());
        // Unstaged modification.
        std::fs::write(
            tmp.path().join("src/lib.rs"),
            "pub fn a() {}\npub fn b() {}\n",
        )
        .unwrap();
        // Untracked file.
        std::fs::write(tmp.path().join("notes.txt"), "scratch\n").unwrap();
        // Staged new file.
        std::fs::write(tmp.path().join("src/new_mod.rs"), "pub struct S;\n").unwrap();
        git(tmp.path(), &["add", "src/new_mod.rs"]);
        // Staged rename.
        git(tmp.path(), &["mv", "README.md", "README2.md"]);

        let wt = probe_worktree(tmp.path()).await;
        assert!(wt.is_git_repo);

        let by_path = |p: &str| {
            wt.changes
                .iter()
                .find(|c| c.path == p)
                .unwrap_or_else(|| panic!("missing change {p}: {:?}", wt.changes))
        };
        assert_eq!(by_path("src/lib.rs").status, ".M");
        assert_eq!(by_path("notes.txt").status, "??");
        assert_eq!(by_path("src/new_mod.rs").status, "A.");
        let renamed = by_path("README2.md");
        assert!(renamed.status.contains('R'), "status: {}", renamed.status);
        assert_eq!(renamed.orig_path.as_deref(), Some("README.md"));

        // Tracked-change diffstat is present for a dirty tree.
        let diffstat = wt.diffstat.expect("diffstat");
        assert!(diffstat.contains("changed"), "diffstat: {diffstat}");
    }

    #[tokio::test]
    async fn detached_head_is_flagged() {
        let tmp = tempfile::tempdir().unwrap();
        init_repo(tmp.path());
        git(tmp.path(), &["checkout", "--detach"]);
        let wt = probe_worktree(tmp.path()).await;
        assert!(wt.is_git_repo);
        assert!(wt.detached);
        assert!(wt.branch.is_none());
        assert!(wt.head.is_some());
    }

    #[tokio::test]
    async fn unborn_branch_has_branch_name_but_no_head() {
        let tmp = tempfile::tempdir().unwrap();
        git(tmp.path(), &["init", "--initial-branch=main"]);
        std::fs::write(tmp.path().join("first.txt"), "x\n").unwrap();
        let wt = probe_worktree(tmp.path()).await;
        assert!(wt.is_git_repo);
        assert_eq!(wt.branch.as_deref(), Some("main"));
        assert!(wt.head.is_none());
        assert_eq!(wt.changes.len(), 1);
        assert_eq!(wt.changes[0].status, "??");
    }

    #[tokio::test]
    async fn run_git_refuses_mutating_subcommands() {
        let tmp = tempfile::tempdir().unwrap();
        init_repo(tmp.path());
        for sub in ["add", "commit", "checkout", "stash", "reset", "clean"] {
            let err = run_git(tmp.path(), &[sub, "--help"]).await.unwrap_err();
            assert!(err.contains("read-only"), "{sub}: {err}");
        }
    }

    #[test]
    fn parse_branch_ab_header() {
        let mut wt = WorktreeStatus {
            project: PathBuf::from("/p"),
            is_git_repo: true,
            branch: None,
            detached: false,
            head: None,
            upstream: None,
            ahead: None,
            behind: None,
            changes: Vec::new(),
            changes_truncated: false,
            diffstat: None,
            probed_at: chrono::Utc::now(),
            error: None,
        };
        let raw = b"# branch.oid deadbeef\0# branch.head main\0# branch.upstream origin/main\0# branch.ab +3 -1\0";
        parse_porcelain_v2(raw, &mut wt);
        assert_eq!(wt.branch.as_deref(), Some("main"));
        assert_eq!(wt.upstream.as_deref(), Some("origin/main"));
        assert_eq!(wt.ahead, Some(3));
        assert_eq!(wt.behind, Some(1));
    }

    #[test]
    fn parse_handles_paths_with_spaces() {
        let mut wt = WorktreeStatus {
            project: PathBuf::from("/p"),
            is_git_repo: true,
            branch: None,
            detached: false,
            head: None,
            upstream: None,
            ahead: None,
            behind: None,
            changes: Vec::new(),
            changes_truncated: false,
            diffstat: None,
            probed_at: chrono::Utc::now(),
            error: None,
        };
        let raw = b"1 .M N... 100644 100644 100644 abc def my file.txt\0? another odd name.md\0";
        parse_porcelain_v2(raw, &mut wt);
        assert_eq!(wt.changes.len(), 2);
        assert_eq!(wt.changes[0].path, "my file.txt");
        assert_eq!(wt.changes[0].status, ".M");
        assert_eq!(wt.changes[1].path, "another odd name.md");
    }

    #[test]
    fn attribution_matches_edit_write_and_bash() {
        let project = Path::new("/proj/alpha");
        let mut changes = vec![
            WorktreeChange {
                path: "src/lib.rs".into(),
                status: ".M".into(),
                orig_path: None,
                attributed_to: None,
                attributed_tool: None,
            },
            WorktreeChange {
                path: "scripts/gen.sh".into(),
                status: ".M".into(),
                orig_path: None,
                attributed_to: None,
                attributed_tool: None,
            },
            WorktreeChange {
                path: "notes.txt".into(),
                status: "??".into(),
                orig_path: None,
                attributed_to: None,
                attributed_tool: None,
            },
        ];
        let touches = vec![
            // Most recent first: this Edit wins over the older Write below.
            FileTouch {
                agent_id: "cc-new".into(),
                tool_name: "Edit".into(),
                tool_input: serde_json::json!({"file_path": "/proj/alpha/src/lib.rs"}),
            },
            FileTouch {
                agent_id: "cc-old".into(),
                tool_name: "Write".into(),
                tool_input: serde_json::json!({"file_path": "/proj/alpha/src/lib.rs"}),
            },
            FileTouch {
                agent_id: "codex-1".into(),
                tool_name: "Bash".into(),
                tool_input: serde_json::json!({"command": "chmod +x scripts/gen.sh && ./scripts/gen.sh"}),
            },
        ];
        attribute_changes(project, &mut changes, &touches);
        assert_eq!(changes[0].attributed_to.as_deref(), Some("cc-new"));
        assert_eq!(changes[0].attributed_tool.as_deref(), Some("Edit"));
        assert_eq!(changes[1].attributed_to.as_deref(), Some("codex-1"));
        assert_eq!(changes[1].attributed_tool.as_deref(), Some("Bash"));
        assert!(
            changes[2].attributed_to.is_none(),
            "human/unknown stays None"
        );
    }

    #[test]
    fn attribution_short_bash_paths_do_not_overmatch() {
        let project = Path::new("/p");
        let mut changes = vec![WorktreeChange {
            path: "a.c".into(),
            status: ".M".into(),
            orig_path: None,
            attributed_to: None,
            attributed_tool: None,
        }];
        let touches = vec![FileTouch {
            agent_id: "cc-x".into(),
            tool_name: "Bash".into(),
            // Contains "a.c" only as a substring of an unrelated word — but the
            // rel-path guard (< 4 chars) refuses substring matching entirely.
            tool_input: serde_json::json!({"command": "cargo build --package alpha.core"}),
        }];
        attribute_changes(project, &mut changes, &touches);
        assert!(changes[0].attributed_to.is_none());
    }
}
