//! Secret redaction for tool inputs/results before they are persisted,
//! archived, or shown in notifications (itr#89).
//!
//! Tool inputs routinely carry live credentials — `export API_KEY=...` in
//! Bash, `.env` contents in Read/Write results, bearer tokens in WebFetch —
//! and those fields land in SQLite, events.jsonl, and the notification UI
//! (visible on a locked macOS screen). This module is the single shared
//! scrubber: the hook uses it before writing events.jsonl, the daemon before
//! persisting rows and before notifying.
//!
//! Deliberately dependency-free (no regex): a token scanner over three rules,
//! tuned to avoid false positives on git SHAs, UUIDs, and file paths.

/// Replacement marker. Kept greppable and distinct from real data.
pub const REDACTED: &str = "***REDACTED***";

/// Key names whose assigned values are always redacted, wherever they appear
/// as `KEY=value`, `key: value`, or JSON object keys. Matched
/// case-insensitively as substrings of the key token.
const SECRET_KEY_MARKERS: &[&str] = &[
    "api_key",
    "apikey",
    "api-key",
    "secret",
    "token",
    "password",
    "passwd",
    "credential",
    "authorization",
    "private_key",
    "access_key",
    "client_id",
    "client-secret",
];

/// Well-known secret prefixes: any token starting with one of these is
/// redacted outright (OpenAI/Anthropic keys, GitHub PATs, Slack tokens, AWS
/// access keys, Google API keys, GitLab PATs, JWTs).
const SECRET_PREFIXES: &[&str] = &[
    "sk-",
    "ghp_",
    "gho_",
    "ghu_",
    "ghs_",
    "ghr_",
    "github_pat_",
    "xoxb-",
    "xoxp-",
    "xoxa-",
    "xoxr-",
    "xoxs-",
    "AKIA",
    "ASIA",
    "AIza",
    "glpat-",
    "eyJ",
];

/// Words that mark the FOLLOWING token as a credential (`Bearer <tok>`).
const SECRET_LEAD_WORDS: &[&str] = &["bearer", "basic"];

fn key_is_secret(key: &str) -> bool {
    let lower = key.to_lowercase();
    SECRET_KEY_MARKERS.iter().any(|m| lower.contains(m))
}

/// Whether a bare token looks like a credential by prefix.
fn token_has_secret_prefix(token: &str) -> bool {
    // Require some payload beyond the prefix so `sk-` alone or short
    // non-secrets ("eyJa") don't trip it.
    SECRET_PREFIXES
        .iter()
        .any(|p| token.starts_with(p) && token.len() >= p.len() + 6)
}

/// Trim quotes/punctuation that commonly wrap a token in shell or JSON text.
fn trim_wrapping(token: &str) -> &str {
    token.trim_matches(|c: char| {
        matches!(
            c,
            '"' | '\'' | '`' | ',' | ';' | '(' | ')' | '{' | '}' | '[' | ']'
        )
    })
}

/// Redact one whitespace-delimited word in place, given the previous word.
/// Returns the (possibly replaced) word.
fn redact_word(word: &str, prev_word: &str) -> String {
    let core = trim_wrapping(word);

    // Rule 1: `KEY=value` / `KEY:value` where KEY names a secret. The whole
    // word (including any wrapping quotes around the value) is replaced.
    for sep in ['=', ':'] {
        if let Some((lhs, rhs)) = core.split_once(sep)
            && key_is_secret(lhs)
            && !rhs.is_empty()
        {
            return format!("{lhs}{sep}{REDACTED}");
        }
    }

    // Rule 2: value token following a secret key or a lead word
    // ("password: hunter2", `"token": "abc"`, "Bearer abc123"). A lead word
    // itself ("Bearer" after "Authorization:") is a scheme name, not a
    // secret — keep it and redact what follows instead.
    let is_lead = |w: &str| SECRET_LEAD_WORDS.iter().any(|l| w.eq_ignore_ascii_case(l));
    let prev_core = trim_wrapping(prev_word).trim_end_matches(':');
    let prev_is_key = key_is_secret(prev_core) && prev_word.trim_end().ends_with(':');
    if (prev_is_key || is_lead(prev_core)) && !core.is_empty() && !is_lead(core) {
        return REDACTED.to_string();
    }

    // Rule 3: well-known secret prefixes anywhere in the word (handles
    // `FOO=sk-abc...` even when FOO isn't a secret-named key).
    if token_has_secret_prefix(core) {
        return REDACTED.to_string();
    }
    if let Some(eq) = core.find('=')
        && token_has_secret_prefix(&core[eq + 1..])
    {
        let lhs = &core[..eq];
        return format!("{lhs}={REDACTED}");
    }

    word.to_string()
}

/// Redact secrets in free text (shell commands, notification bodies, file
/// contents). Whitespace layout is preserved.
pub fn redact_text(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut prev_word = "";
    let mut rest = text;
    while !rest.is_empty() {
        let ws_len = rest.len() - rest.trim_start().len();
        out.push_str(&rest[..ws_len]);
        rest = &rest[ws_len..];
        if rest.is_empty() {
            break;
        }
        let word_len = rest.find(|c: char| c.is_whitespace()).unwrap_or(rest.len());
        let word = &rest[..word_len];
        out.push_str(&redact_word(word, prev_word));
        prev_word = word;
        rest = &rest[word_len..];
    }
    out
}

/// Redact secrets in a JSON value: object values under secret-named keys are
/// replaced wholesale; every string is additionally run through
/// [`redact_text`]; arrays/objects recurse.
pub fn redact_value(value: &serde_json::Value) -> serde_json::Value {
    match value {
        serde_json::Value::Object(map) => {
            let mut out = serde_json::Map::with_capacity(map.len());
            for (k, v) in map {
                if key_is_secret(k) {
                    out.insert(k.clone(), serde_json::Value::String(REDACTED.into()));
                } else {
                    out.insert(k.clone(), redact_value(v));
                }
            }
            serde_json::Value::Object(out)
        }
        serde_json::Value::Array(items) => {
            serde_json::Value::Array(items.iter().map(redact_value).collect())
        }
        serde_json::Value::String(s) => serde_json::Value::String(redact_text(s)),
        other => other.clone(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn redacts_env_style_assignment() {
        // The itr#89 acceptance example.
        assert_eq!(
            redact_text("export API_KEY=sk-abc123def456"),
            format!("export API_KEY={REDACTED}")
        );
        assert_eq!(
            redact_text("export MY_PASSWORD='hunter2!'"),
            format!("export MY_PASSWORD={REDACTED}")
        );
    }

    #[test]
    fn redacts_known_prefixes_even_without_secret_key_name() {
        assert_eq!(
            redact_text("curl -H x https://x -d ghp_16chartoken1234"),
            format!("curl -H x https://x -d {REDACTED}")
        );
        // Prefix behind an innocent-looking assignment.
        assert_eq!(
            redact_text("FOO=sk-proj-abcdef123456"),
            format!("FOO={REDACTED}")
        );
    }

    #[test]
    fn redacts_bearer_and_key_colon_value() {
        assert_eq!(
            redact_text("Authorization: Bearer abc.def.ghi"),
            format!("Authorization: Bearer {REDACTED}")
        );
        assert_eq!(
            redact_text("password: hunter2"),
            format!("password: {REDACTED}")
        );
    }

    #[test]
    fn leaves_benign_text_alone() {
        for benign in [
            "git checkout 8bca254e0530ce2c66de9e030b0d770712345678",
            "ls -la /tmp/tokens-of-appreciation.txt",
            "echo hello world",
            "cargo test -p wisphive_daemon",
            "id=550e8400-e29b-41d4-a716-446655440000",
        ] {
            assert_eq!(redact_text(benign), benign, "false positive on: {benign}");
        }
    }

    #[test]
    fn redact_value_scrubs_secret_keys_and_nested_strings() {
        let input = serde_json::json!({
            "command": "export API_KEY=sk-abc123def456 && ./run",
            "env": {"GITHUB_TOKEN": "ghp_zzzzzzzzzzzz", "PATH": "/usr/bin"},
            "args": ["--password=hunter2secret"],
            "count": 3,
        });
        let redacted = redact_value(&input);
        assert_eq!(
            redacted["command"],
            format!("export API_KEY={REDACTED} && ./run")
        );
        // Key named *_TOKEN → value replaced wholesale.
        assert_eq!(redacted["env"]["GITHUB_TOKEN"], REDACTED);
        assert_eq!(redacted["env"]["PATH"], "/usr/bin");
        assert_eq!(redacted["args"][0], format!("--password={REDACTED}"));
        assert_eq!(redacted["count"], 3);
        // Nothing secret survives serialization.
        let dump = redacted.to_string();
        assert!(!dump.contains("sk-abc123"));
        assert!(!dump.contains("ghp_"));
        assert!(!dump.contains("hunter2"));
    }
}
