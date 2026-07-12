//! Snapshot test harness for the Wisphive TUI (itr#418).
//!
//! Renders the real `ui::draw` code path into a ratatui [`TestBackend`]
//! buffer — no daemon, no PTY, no `~/.wisphive` access — and asserts the
//! rendered frame against a human-readable `insta` snapshot committed under
//! `tests/snapshots/`. Each `.snap` file is a plain-text rendering of the
//! TUI frame that reviewers can read directly.
//!
//! # How to add a snapshot for a new TUI change
//!
//! 1. Build an [`App`] with fixture state (see the `fixtures` module —
//!    construct `DecisionRequest`s etc. directly; never talk to the daemon).
//!    Use [`fixture_time`] for timestamps so ages render deterministically.
//! 2. Set `app.view_mode` (or call the `enter_*_view` helpers) and render
//!    with [`render_app`].
//! 3. Assert with [`assert_frame_snapshot`] — it strips absolute datetimes
//!    (`YYYY-MM-DD HH:MM:SS` -> `[TIMESTAMP]`) so frames stay deterministic.
//! 4. Generate/refresh snapshots with either:
//!    `INSTA_UPDATE=always cargo test -p wisphive_tui` or
//!    `cargo insta review` (if cargo-insta is installed).
//!    Commit the `.snap` files — they are the acceptance evidence.
//!
//! # House rules enforced here
//!
//! - **Status-bar completeness**: every keybinding a view handles in
//!   `input.rs` must be shown in that view's status bar. The
//!   `status_bar_*` tests hardcode the expected key-hint tokens per view;
//!   adding a keybinding in `input.rs` requires updating both the view's
//!   status bar in `ui.rs` and the token list here. Alias keys
//!   (arrow keys for `j/k`, `PgDn` for `Spc`, `Esc` where `q` is shown as
//!   `q/Esc`) do not need separate hints.
//! - **Modal overlays render on every view**: `draw` must overlay
//!   `draw_modal` on the active view — covered by the
//!   `modal_overlay_over_*` snapshots (dashboard + detail).
//! - **Detail views must never render empty** (the ExitPlanMode /
//!   AskUserQuestion regression, `claude/investigation-empty-detail-views.md`):
//!   covered by explicit content assertions plus snapshots.

use ratatui::Terminal;
use ratatui::backend::TestBackend;
use ratatui::buffer::Buffer;
use ratatui::layout::Position;

use wisphive_tui::app::{App, ViewMode};
use wisphive_tui::modal::Modal;
use wisphive_tui::panels;
use wisphive_tui::ui;

// ── Rendering helpers ────────────────────────────────────────────────

/// Render `app` at the given size and return the frame as plain text
/// (styles dropped, trailing whitespace trimmed per line).
fn render_app(app: &App, width: u16, height: u16) -> String {
    let backend = TestBackend::new(width, height);
    let mut terminal = Terminal::new(backend).expect("test terminal");
    terminal
        .draw(|frame| ui::draw(frame, app))
        .expect("draw frame");
    buffer_to_string(terminal.backend().buffer())
}

/// Convert a rendered buffer into readable text, one line per row.
fn buffer_to_string(buffer: &Buffer) -> String {
    let area = buffer.area;
    let mut out = String::with_capacity((area.width as usize + 1) * area.height as usize);
    for y in area.top()..area.bottom() {
        let mut line = String::new();
        for x in area.left()..area.right() {
            let symbol = buffer
                .cell(Position::new(x, y))
                .map(|cell| cell.symbol())
                .unwrap_or(" ");
            line.push_str(symbol);
        }
        out.push_str(line.trim_end());
        out.push('\n');
    }
    out
}

/// Snapshot-assert a rendered frame with datetime filtering so absolute
/// timestamps (which derive from `Utc::now()` in fixtures) stay stable.
fn assert_frame_snapshot(name: &str, frame: String) {
    insta::with_settings!({
        prepend_module_to_snapshot => false,
        filters => vec![(r"\d{4}-\d{2}-\d{2} \d{2}:\d{2}:\d{2}", "[TIMESTAMP]")],
    }, {
        insta::assert_snapshot!(name, frame);
    });
}

/// The bottom row of a rendered frame — the status bar of every view.
fn bottom_row(frame: &str) -> String {
    frame
        .lines()
        .next_back()
        .map(str::to_string)
        .unwrap_or_default()
}

/// Assert every expected key-hint token appears in a view's status bar.
fn expect_bar_tokens(view: &str, bar: &str, tokens: &[&str]) {
    for token in tokens {
        assert!(
            bar.contains(token),
            "{view} status bar is missing key hint {token:?}.\n\
             House rule: every keybinding handled in input.rs must be shown \
             in the view's status bar.\nbar: {bar:?}"
        );
    }
}

/// Render a view wide enough that its status bar is never truncated, and
/// return just that bar. Width 240 comfortably fits the longest bar.
fn status_bar_of(app: &App) -> String {
    bottom_row(&render_app(app, 240, 30))
}

// ── Fixtures ─────────────────────────────────────────────────────────

mod fixtures {
    use std::path::PathBuf;

    use chrono::{DateTime, Duration, Utc};
    use uuid::Uuid;
    use wisphive_protocol::{
        AgentInfo, AgentType, Decision, DecisionRequest, HistoryEntry, HookEventType,
    };
    use wisphive_tui::app::App;

    /// A fixed-age timestamp: 2 hours ago. Relative ages render as a stable
    /// "2h" and absolute datetimes are filtered by `assert_frame_snapshot`.
    pub fn fixture_time() -> DateTime<Utc> {
        Utc::now() - Duration::hours(2)
    }

    pub fn request(id: u128, tool_name: &str, tool_input: serde_json::Value) -> DecisionRequest {
        DecisionRequest {
            id: Uuid::from_u128(id),
            agent_id: "claude-a1b2c3".into(),
            agent_type: AgentType::ClaudeCode,
            project: PathBuf::from("/tmp/wisphive-demo"),
            tool_name: tool_name.into(),
            tool_input,
            timestamp: fixture_time(),
            hook_event_name: HookEventType::PreToolUse,
            tool_use_id: Some(format!("toolu_{id:04}")),
            permission_suggestions: None,
            event_data: None,
            terminal_session_id: None,
        }
    }

    pub fn bash_request() -> DecisionRequest {
        request(
            1,
            "Bash",
            serde_json::json!({
                "command": "cargo test --workspace",
                "description": "Run all workspace tests",
            }),
        )
    }

    /// The second rocket begins at byte 46, so the old `&summary[..47]`
    /// queue truncation panicked in the middle of its UTF-8 encoding.
    pub fn unicode_bash_request() -> DecisionRequest {
        let prefix = "git commit -m \"ship 🚀\" && ";
        let padding = "x".repeat(46 - prefix.len());
        request(
            5,
            "Bash",
            serde_json::json!({
                "command": format!("{prefix}{padding}🚀 deploy now"),
            }),
        )
    }

    /// The rocket begins at byte 36, so the old `&s[..37]` timeline
    /// truncation panicked in the middle of its UTF-8 encoding.
    pub fn unicode_session_timeline_app() -> App {
        let mut app = App::new();
        app.connected = true;
        app.view_mode = wisphive_tui::app::ViewMode::SessionTimeline;
        app.session_timeline_agent_id = Some("codex-unicode".into());
        app.session_timeline = vec![HistoryEntry {
            id: Uuid::from_u128(6),
            agent_id: "codex-unicode".into(),
            agent_type: AgentType::Codex,
            project: PathBuf::from("/tmp/wisphive-demo"),
            tool_name: "Bash".into(),
            tool_input: serde_json::json!({
                "command": format!("{}🚀 timeline tail", "x".repeat(36)),
            }),
            decision: Decision::Approve,
            requested_at: fixture_time(),
            resolved_at: fixture_time(),
            tool_result: None,
            tool_use_id: Some("toolu_unicode".into()),
            hook_event_name: Some("PreToolUse".into()),
            terminal_session_id: None,
            decided_by: Some("human".into()),
            config_hash: None,
        }];
        app
    }

    pub fn edit_request() -> DecisionRequest {
        request(
            2,
            "Edit",
            serde_json::json!({
                "file_path": "/tmp/wisphive-demo/src/main.rs",
                "old_string": "fn main() {\n    println!(\"hello\");\n}",
                "new_string": "fn main() {\n    println!(\"hello, wisphive\");\n}",
            }),
        )
    }

    /// AskUserQuestion arrives as a PermissionRequest without permission
    /// suggestions; the questions live in `tool_input`.
    pub fn ask_user_question_request() -> DecisionRequest {
        let mut req = request(
            3,
            "AskUserQuestion",
            serde_json::json!({
                "questions": [{
                    "header": "Storage",
                    "question": "Which storage backend should the daemon use?",
                    "options": [
                        {"label": "SQLite", "description": "Embedded, zero-config"},
                        {"label": "Postgres", "description": "Networked, multi-user"},
                    ],
                    "multiSelect": false,
                }],
            }),
        );
        req.hook_event_name = HookEventType::PermissionRequest;
        req
    }

    /// ExitPlanMode arrives as a PermissionRequest whose plan text lives in
    /// `event_data.plan_content`.
    pub fn exit_plan_mode_request() -> DecisionRequest {
        let mut req = request(4, "ExitPlanMode", serde_json::json!({}));
        req.hook_event_name = HookEventType::PermissionRequest;
        req.event_data = Some(serde_json::json!({
            "plan_content": "# Plan\n\n1. Add a TestBackend snapshot harness\n2. Cover the detail views\n3. Enforce the status-bar house rule",
        }));
        req
    }

    pub fn agent(id: &str) -> AgentInfo {
        AgentInfo {
            agent_id: id.into(),
            agent_type: AgentType::ClaudeCode,
            project: PathBuf::from("/tmp/wisphive-demo"),
            connected_at: fixture_time(),
            last_seen: fixture_time(),
        }
    }

    /// A dashboard populated with a pending queue, agents, and projects.
    pub fn dashboard_app() -> App {
        let mut app = App::new();
        app.connected = true;
        app.queue = vec![
            bash_request(),
            edit_request(),
            ask_user_question_request(),
            exit_plan_mode_request(),
        ];
        app.agents = vec![agent("claude-a1b2c3"), agent("codex-d4e5f6")];
        app.rebuild_projects();
        app
    }

    /// An app showing the full-screen detail view for `req`.
    pub fn detail_app(req: DecisionRequest) -> App {
        let mut app = App::new();
        app.connected = true;
        app.queue = vec![req];
        app.rebuild_projects();
        app.enter_detail_view();
        assert_eq!(
            app.view_mode,
            wisphive_tui::app::ViewMode::Detail,
            "fixture should land in the detail view"
        );
        app
    }
}

use fixtures::{
    ask_user_question_request, bash_request, dashboard_app, detail_app, edit_request,
    exit_plan_mode_request, request, unicode_bash_request, unicode_session_timeline_app,
};

// ── Snapshot tests: queue panel + detail views ───────────────────────

#[test]
fn dashboard_queue_with_pending_decisions() {
    let app = dashboard_app();
    assert_frame_snapshot("dashboard_queue_pending", render_app(&app, 100, 24));
}

#[test]
fn dashboard_config_alert_banner() {
    let mut app = dashboard_app();
    app.apply_config_alert(
        wisphive_protocol::ConfigAlertKind::UntrustedConfig,
        true,
        "config.json is group writable; using the safe read-tier policy.".into(),
    );
    app.apply_config_alert(
        wisphive_protocol::ConfigAlertKind::PolicyWidened,
        true,
        "auto_approve_level increased from read to all".into(),
    );

    let frame = render_app(&app, 100, 24);
    assert!(frame.contains("CONFIG UNTRUSTED"));
    assert!(frame.contains("POLICY WIDENED"));
    assert_frame_snapshot("dashboard_config_alert_banner", frame);
}

/// Approval-safety regression: a selected request below the fold must be
/// rendered, otherwise `y` would approve an invisible queue entry.
#[test]
fn dashboard_queue_scrolls_deep_selection_into_view() {
    let mut app = App::new();
    app.connected = true;
    app.queue = (0_u128..24)
        .map(|index| {
            request(
                100 + index,
                "Bash",
                serde_json::json!({"command": format!("queue-item-{index:02}")}),
            )
        })
        .collect();
    app.queue_index = 23;
    app.rebuild_projects();

    // Eighteen rows leave only seven visible entries inside the queue panel.
    let frame = render_app(&app, 100, 18);
    assert!(
        frame.contains("queue-item-23"),
        "the deeply selected queue request must scroll into view\n{frame}"
    );
    assert!(
        !frame.contains("queue-item-00"),
        "the viewport should move away from the first queue request\n{frame}"
    );
}

#[test]
fn dashboard_queue_truncates_unicode_without_panicking() {
    let mut app = App::new();
    app.connected = true;
    app.queue = vec![unicode_bash_request()];
    app.rebuild_projects();

    let queue_item = panels::format_queue_item(&app.queue[0]);
    assert!(queue_item.contains("git commit -m \"ship 🚀\""));
    assert!(queue_item.contains("🚀 de..."));

    let frame = render_app(&app, 140, 24);
    assert!(
        frame.contains("git commit -m \"ship") && frame.matches('🚀').count() == 2,
        "the pending Unicode command should render through the real queue surface\n{frame}"
    );
    assert!(
        frame.contains("..."),
        "long command should be truncated\n{frame}"
    );
}

#[test]
fn session_timeline_truncates_unicode_without_panicking() {
    let app = unicode_session_timeline_app();
    let frame = render_app(&app, 120, 12);

    assert!(
        frame.contains(&format!("{}🚀 ...", "x".repeat(36))),
        "timeline command should be character-truncated after the rocket\n{frame}"
    );
}

#[test]
fn detail_view_bash() {
    let app = detail_app(bash_request());
    let frame = render_app(&app, 100, 28);
    assert!(frame.contains("cargo test --workspace"));
    assert_frame_snapshot("detail_bash", frame);
}

#[test]
fn detail_view_edit_shows_diff() {
    let app = detail_app(edit_request());
    let frame = render_app(&app, 100, 28);
    assert!(frame.contains("- ") && frame.contains("+ "), "diff markers");
    assert!(frame.contains("hello, wisphive"));
    assert_frame_snapshot("detail_edit", frame);
}

/// Regression guard: ExitPlanMode previously rendered an empty detail view
/// (see claude/investigation-empty-detail-views.md).
#[test]
fn detail_view_exit_plan_mode_is_not_empty() {
    let app = detail_app(exit_plan_mode_request());
    let frame = render_app(&app, 100, 28);
    assert!(frame.contains("── Plan ──"), "plan section header");
    assert!(
        frame.contains("Add a TestBackend snapshot harness"),
        "plan body must be rendered, not an empty view"
    );
    assert!(frame.contains("[A/Enter]accept"), "plan action hints");
    assert_frame_snapshot("detail_exit_plan_mode", frame);
}

/// Regression guard: AskUserQuestion previously rendered an empty detail
/// view (see claude/investigation-empty-detail-views.md).
#[test]
fn detail_view_ask_user_question_is_not_empty() {
    let app = detail_app(ask_user_question_request());
    let frame = render_app(&app, 100, 28);
    assert!(
        frame.contains("Which storage backend should the daemon use?"),
        "question text must be rendered, not an empty view"
    );
    assert!(
        frame.contains("SQLite") && frame.contains("Postgres"),
        "options"
    );
    assert!(frame.contains("[1-2]select"), "numbered-option hint");
    assert_frame_snapshot("detail_ask_user_question", frame);
}

// ── Snapshot tests: modal overlay on top of two different views ──────

#[test]
fn modal_overlay_over_dashboard() {
    let mut app = dashboard_app();
    app.modal = Some(Modal::confirm_approve_all(app.queue.len()));
    let frame = render_app(&app, 100, 24);
    assert!(frame.contains("Confirm Approve All"));
    assert_frame_snapshot("modal_over_dashboard", frame);
}

#[test]
fn modal_overlay_over_detail_view() {
    let mut app = detail_app(bash_request());
    let id = app.queue[0].id;
    app.modal = Some(Modal::deny_with_message(id));
    let frame = render_app(&app, 100, 28);
    assert!(
        frame.contains("Deny with Message"),
        "draw_detail_view must overlay draw_modal (house rule)"
    );
    assert_frame_snapshot("modal_over_detail", frame);
}

// ── Status-bar keybinding completeness per view ──────────────────────
//
// Expected tokens mirror the handlers in input.rs for each view. If a test
// here fails after you add a keybinding, add the hint to the view's status
// bar in ui.rs first, then extend the token list.

#[test]
fn status_bar_dashboard() {
    let bar = status_bar_of(&dashboard_app());
    expect_bar_tokens(
        "dashboard",
        &bar,
        &[
            "[j/k]",
            "[y]",
            "[Enter/a/d]",
            "[A]",
            "[D]",
            "[n]",
            "[P]",
            "[t]",
            "[T]",
            "[h]",
            "[s]",
            "[p]",
            "[c]",
            "[/]",
            "[Tab]",
            "[e]",
            "[q]",
            "[Q]",
        ],
    );
}

#[test]
fn status_bar_detail_pre_tool_use() {
    let bar = status_bar_of(&detail_app(bash_request()));
    expect_bar_tokens(
        "detail (PreToolUse)",
        &bar,
        &[
            "[Y]", "[N]", "[M]", "[!]", "[E]", "[C]", "[?]", "[j/k]", "[Spc]", "[g/G]", "[q/Esc]",
            "[Q]", "[P]",
        ],
    );
}

#[test]
fn status_bar_detail_exit_plan_mode() {
    let bar = status_bar_of(&detail_app(exit_plan_mode_request()));
    expect_bar_tokens(
        "detail (ExitPlanMode)",
        &bar,
        &[
            "[A/Enter]",
            "[D]",
            "[M]",
            "[j/k]",
            "[Spc]",
            "[g/G]",
            "[q/Esc]",
            "[Q]",
            "[P]",
        ],
    );
}

#[test]
fn status_bar_detail_ask_user_question() {
    let bar = status_bar_of(&detail_app(ask_user_question_request()));
    expect_bar_tokens(
        "detail (AskUserQuestion)",
        &bar,
        &[
            "[1-2]", "[O]", "[D]", "[M]", "[j/k]", "[Spc]", "[g/G]", "[q/Esc]", "[Q]", "[P]",
        ],
    );
}

#[test]
fn status_bar_history() {
    let mut app = App::new();
    app.view_mode = ViewMode::History;
    let bar = status_bar_of(&app);
    expect_bar_tokens(
        "history",
        &bar,
        &[
            "[j/k]", "[Enter]", "[/]", "[H/", "[L/", "[C]", "[f]", "[F]", "[q]", "[Q]",
        ],
    );
}

#[test]
fn status_bar_history_detail() {
    let mut app = App::new();
    app.view_mode = ViewMode::HistoryDetail;
    let bar = status_bar_of(&app);
    expect_bar_tokens(
        "history detail",
        &bar,
        &["[j/k]", "[Spc]", "[q/Esc]", "[Q]"],
    );
}

#[test]
fn status_bar_config() {
    // Set the config view directly — enter_config_view() reads
    // ~/.wisphive/config.json, which tests must never touch.
    let mut app = App::new();
    app.view_mode = ViewMode::Config;
    let bar = status_bar_of(&app);
    expect_bar_tokens(
        "config",
        &bar,
        &["[j/k]", "[←/→]", "[Space]", "[+]", "[-]", "[q/Esc]", "[Q]"],
    );
}

#[test]
fn status_bar_sessions() {
    let mut app = App::new();
    app.view_mode = ViewMode::Sessions;
    let bar = status_bar_of(&app);
    expect_bar_tokens(
        "sessions",
        &bar,
        &["[j/k]", "[Enter]", "[r]", "[q/Esc]", "[Q]"],
    );
}

#[test]
fn status_bar_session_timeline() {
    let mut app = App::new();
    app.view_mode = ViewMode::SessionTimeline;
    let bar = status_bar_of(&app);
    expect_bar_tokens(
        "session timeline",
        &bar,
        &["[j/k]", "[Enter]", "[H/", "[L/", "[q/Esc]", "[Q]"],
    );
}

#[test]
fn status_bar_projects_explorer() {
    let mut app = App::new();
    app.view_mode = ViewMode::ProjectsExplorer;
    let bar = status_bar_of(&app);
    expect_bar_tokens(
        "projects explorer",
        &bar,
        &["[j/k]", "[Enter]", "[n]", "[r]", "[q/Esc]", "[Q]"],
    );
}

#[test]
fn status_bar_terminal_list() {
    let mut app = App::new();
    app.view_mode = ViewMode::TerminalList;
    let bar = status_bar_of(&app);
    expect_bar_tokens(
        "terminal list",
        &bar,
        &["[n]", "[P]", "[Enter]", "[r]", "[d]", "[j/k]", "[q/Esc]"],
    );
}

#[test]
fn status_bar_terminal_view() {
    let mut app = App::new();
    app.view_mode = ViewMode::TerminalView;
    let bar = status_bar_of(&app);
    expect_bar_tokens("terminal view", &bar, &["[F10]", "[Esc Esc]", "[Ctrl-C]"]);
}

#[test]
fn status_bar_terminal_replay() {
    let mut app = App::new();
    app.view_mode = ViewMode::TerminalReplay;
    let bar = status_bar_of(&app);
    expect_bar_tokens("terminal replay", &bar, &["[q/Esc]"]);
}
