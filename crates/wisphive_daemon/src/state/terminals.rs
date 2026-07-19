use std::collections::{BTreeMap, HashMap};

use anyhow::{Result, anyhow};
use serde::{Deserialize, Serialize};
use wisphive_protocol::redact::{REDACTED, redact_text, redact_value};
use wisphive_protocol::{TerminalDirection, TerminalSessionMeta, TerminalStatus};

use super::StateDb;

/// Environment portion of a terminal session's persisted respawn spec,
/// stored as JSON in `terminal_sessions.env_json` (itr#590, Plan A of
/// `docs/research/terminal-session-survival.md`).
///
/// # Chosen semantics — read this before touching the field
///
/// This captures **only the client-requested env overrides** from
/// `TermCreate` — never the daemon's inherited environment. The spawn path
/// (`CommandBuilder` in `terminal.rs`) inherits the daemon's own environment
/// implicitly, so a reconciling daemon (itr#591) re-inherits *current* values
/// by construction. Persisting the full environment would replay stale
/// secrets and stale machine state; persisting the overrides reproduces
/// exactly what the client asked for.
///
/// Secret classification follows the shared itr#89 scrubber
/// (`wisphive_protocol::redact`): a var is secret-bearing when ANY scrubber
/// probe would redact it — `redact_value` on the one-entry map
/// `{NAME: value}` (the key-name rule: secret-named keys are replaced
/// wholesale regardless of value shape), `redact_text` on `NAME=value`, or
/// `redact_text` on the value alone. See [`env_pair_is_secret`].
///
/// - **Non-secret vars** are stored **verbatim** in [`Self::set`] (raw value,
///   not the scrubber's log-escaped form — escaping would corrupt values
///   containing backslashes; JSON encoding is already lossless, and
///   `env_json` is a machine-consumed spec, not a log/notify surface).
/// - **Secret-bearing vars** are stored as **NAME-ONLY markers** in
///   [`Self::inherit_names`] — no value, not even a `***REDACTED***`
///   placeholder, because respawning with a placeholder would corrupt the
///   session's environment. At respawn, [`Self::materialize`] re-sources
///   those names from the daemon's **live** environment (inherit-current,
///   not replay-stale) and omits names the daemon no longer carries.
/// - Vars whose **name** the scrubber would alter (secret-shaped or
///   containing control bytes/backslashes) are dropped entirely — even the
///   name is unsafe to persist.
///
/// Honest limits: secret detection is exactly as strong as the shared
/// scrubber (a high-entropy value with no recognized marker/prefix persists
/// verbatim — same bar as every other itr#89 surface); env fidelity is
/// best-effort, while cwd/command fidelity is exact (see
/// [`TerminalRespawnSpec`]).
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct TerminalEnvSpec {
    /// Non-secret client-requested vars, stored verbatim. `BTreeMap` for
    /// deterministic serialization.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub set: BTreeMap<String, String>,
    /// Names of secret-bearing client-requested vars. Values are never
    /// persisted; respawn re-sources these from the daemon's current
    /// environment or omits them.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub inherit_names: Vec<String>,
}

/// True when the itr#89 scrubber would redact this pair. Union of three
/// probes, ORed conservatively:
///
/// 1. **Key-name rule** — [`redact_value`] on the one-entry object
///    `{name: value}`: its object arm replaces values under secret-named
///    keys wholesale, and its string arm scrubs the value. Detected via the
///    marker appearing anywhere in the scrubbed JSON. This is what catches
///    a secret-named key with a leading-whitespace or control-prefixed
///    value (`MY_API_KEY=" hunter2"`, `GITHUB_TOKEN="\thunter2"`) and odd
///    names (`"MY_API_KEY "`, `"A=TOKEN"`) that word-based text probes
///    cannot see as assignments.
/// 2. **Assignment probe** — `redact_text("NAME=value")`, the shell-style
///    context.
/// 3. **Value probe** — `redact_text(value)` alone (`Bearer <tok>` etc).
///
/// Escaping-only alterations (backslashes / control bytes in a benign
/// value) deliberately do NOT classify as secret: the scrubber's escaping
/// is an injective, reversible transport encoding (itr#530), not redaction,
/// so a raw verbatim copy carries no secret the escaped form would not.
/// Pairs already containing the literal marker classify as secret
/// (conservative).
fn env_pair_is_secret(name: &str, value: &str) -> bool {
    let mut probe = serde_json::Map::with_capacity(1);
    probe.insert(
        name.to_string(),
        serde_json::Value::String(value.to_string()),
    );
    if redact_value(&serde_json::Value::Object(probe))
        .to_string()
        .contains(REDACTED)
    {
        return true;
    }
    redact_text(&format!("{name}={value}")).contains(REDACTED)
        || redact_text(value).contains(REDACTED)
}

impl TerminalEnvSpec {
    /// Classify a `TermCreate` env-override map into a persistable spec.
    /// Returns `None` when there is nothing safe to persist (empty input, or
    /// every var dropped) — the caller then leaves `env_json` NULL, the same
    /// shape as a session created with no overrides.
    pub fn from_requested_env(env: &HashMap<String, String>) -> Option<Self> {
        let mut spec = Self::default();
        for (name, value) in env {
            if redact_text(name) != *name {
                // The name itself is unsafe to persist (secret-shaped or
                // control-byte-bearing) — drop the var entirely.
                continue;
            }
            if env_pair_is_secret(name, value) {
                spec.inherit_names.push(name.clone());
            } else {
                spec.set.insert(name.clone(), value.clone());
            }
        }
        spec.inherit_names.sort();
        if spec.set.is_empty() && spec.inherit_names.is_empty() {
            None
        } else {
            Some(spec)
        }
    }

    /// Build the env map to pass to a respawn: verbatim vars as stored, plus
    /// each `inherit_names` entry re-sourced from `daemon_env` (the respawning
    /// daemon's current environment — inherit-current, not replay-stale).
    /// Names absent from `daemon_env` are omitted; this is the documented
    /// best-effort limit of env fidelity.
    pub fn materialize(&self, daemon_env: &HashMap<String, String>) -> HashMap<String, String> {
        let mut out: HashMap<String, String> = self
            .set
            .iter()
            .map(|(k, v)| (k.clone(), v.clone()))
            .collect();
        for name in &self.inherit_names {
            if let Some(value) = daemon_env.get(name) {
                out.insert(name.clone(), value.clone());
            }
        }
        out
    }
}

/// The restorable spawn spec for one terminal session (itr#590): everything
/// itr#591's reconcile-on-start needs to faithfully respawn the command.
/// `command`/`args`/`cwd` are exact; `env` is best-effort per
/// [`TerminalEnvSpec`] (`None` = no client overrides were requested, or a
/// legacy row from before env persistence).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TerminalRespawnSpec {
    pub command: String,
    pub args: Vec<String>,
    pub cwd: std::path::PathBuf,
    pub env: Option<TerminalEnvSpec>,
    /// True when `env_json` was present but unparsable: `env` degraded to
    /// `None` (the respawn proceeds with daemon-inherited env only) and the
    /// caller should surface the degradation in its audit detail. Always
    /// false for a NULL `env_json` (legacy rows / no overrides requested).
    pub env_degraded: bool,
}

type TerminalSessionRow = (
    String,
    Option<String>,
    String,
    String,
    String,
    i64,
    i64,
    String,
    Option<String>,
    Option<i64>,
    String,
    Option<String>,
    i64,
    Option<String>,
    Option<String>,
    i64,
);

fn hydrate_terminal_session(row: TerminalSessionRow) -> Option<TerminalSessionMeta> {
    let (
        id,
        label,
        command,
        args_json,
        cwd,
        cols,
        rows_,
        started_at,
        ended_at,
        exit_code,
        status,
        group_name,
        sort_order,
        created_by,
        replay_acl_json,
        pinned,
    ) = row;

    let Ok(id) = uuid::Uuid::parse_str(&id) else {
        return None;
    };
    let args: Vec<String> = serde_json::from_str(&args_json).unwrap_or_default();
    let Ok(started_at) = chrono::DateTime::parse_from_rfc3339(&started_at) else {
        return None;
    };
    let ended_at = ended_at
        .and_then(|s| chrono::DateTime::parse_from_rfc3339(&s).ok())
        .map(|d| d.with_timezone(&chrono::Utc));
    let Ok(status) = status.parse::<TerminalStatus>() else {
        return None;
    };
    let replay_acl = replay_acl_json
        .as_deref()
        .and_then(|json| serde_json::from_str::<Vec<String>>(json).ok())
        .unwrap_or_default();

    Some(TerminalSessionMeta {
        id,
        label,
        command,
        args,
        cwd: std::path::PathBuf::from(cwd),
        cols: cols as u16,
        rows: rows_ as u16,
        started_at: started_at.with_timezone(&chrono::Utc),
        ended_at,
        exit_code: exit_code.map(|c| c as i32),
        status,
        group_name,
        sort_order,
        created_by,
        replay_acl,
        pinned: pinned != 0,
    })
}

impl StateDb {
    // ── Terminal session helpers ──────────────────────────────────

    /// Insert a new terminal session row.
    ///
    /// `env` is the redaction-classified respawn environment (see
    /// [`TerminalEnvSpec`] for the persistence semantics); `None` leaves
    /// `env_json` NULL, matching legacy rows and no-override sessions.
    pub async fn create_terminal_session(
        &self,
        meta: &TerminalSessionMeta,
        env: Option<&TerminalEnvSpec>,
    ) -> Result<()> {
        let args_json = serde_json::to_string(&meta.args)?;
        let replay_acl_json = serde_json::to_string(&meta.replay_acl)?;
        let env_json = env.map(serde_json::to_string).transpose()?;
        sqlx::query(
            "INSERT INTO terminal_sessions (id, label, command, args, cwd, env_json, cols, rows, started_at, ended_at, exit_code, status, group_name, sort_order, created_by, replay_acl, pinned)
             VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)",
        )
        .bind(meta.id.to_string())
        .bind(&meta.label)
        .bind(&meta.command)
        .bind(args_json)
        .bind(meta.cwd.to_string_lossy().to_string())
        .bind(env_json)
        .bind(i64::from(meta.cols))
        .bind(i64::from(meta.rows))
        .bind(meta.started_at.to_rfc3339())
        .bind(meta.ended_at.map(|t| t.to_rfc3339()))
        .bind(meta.exit_code)
        .bind(meta.status.to_string())
        .bind(meta.group_name.as_deref())
        .bind(meta.sort_order)
        .bind(meta.created_by.as_deref())
        .bind(replay_acl_json)
        .bind(i64::from(meta.pinned))
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    /// Assign (or clear, when `group` is None) the group label for a session.
    pub async fn set_terminal_group(&self, id: uuid::Uuid, group: Option<&str>) -> Result<()> {
        sqlx::query("UPDATE terminal_sessions SET group_name = ? WHERE id = ?")
            .bind(group)
            .bind(id.to_string())
            .execute(&self.pool)
            .await?;
        Ok(())
    }

    /// Update a session's manual sort order.
    pub async fn set_terminal_sort_order(&self, id: uuid::Uuid, sort_order: i64) -> Result<()> {
        sqlx::query("UPDATE terminal_sessions SET sort_order = ? WHERE id = ?")
            .bind(sort_order)
            .bind(id.to_string())
            .execute(&self.pool)
            .await?;
        Ok(())
    }

    /// Pin or unpin a session as an "important session" (itr#589). The flag
    /// has execution consequences since itr#591: graceful shutdown preserves
    /// a pinned session's row as `running`, and the startup sweep
    /// ([`Self::mark_running_terminals_orphaned`]) captures pinned `running`
    /// rows as respawn candidates for reconcile-on-start.
    pub async fn set_terminal_pinned(&self, id: uuid::Uuid, pinned: bool) -> Result<()> {
        sqlx::query("UPDATE terminal_sessions SET pinned = ? WHERE id = ?")
            .bind(i64::from(pinned))
            .bind(id.to_string())
            .execute(&self.pool)
            .await?;
        Ok(())
    }

    /// Mark a terminal session as finished and record its final status.
    pub async fn end_terminal_session(
        &self,
        id: uuid::Uuid,
        exit_code: Option<i32>,
        status: TerminalStatus,
    ) -> Result<()> {
        sqlx::query(
            "UPDATE terminal_sessions
             SET ended_at = ?, exit_code = ?, status = ?
             WHERE id = ?",
        )
        .bind(chrono::Utc::now().to_rfc3339())
        .bind(exit_code)
        .bind(status.to_string())
        .bind(id.to_string())
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    /// List all terminal sessions. Ordered by `sort_order` ASC (manual order,
    /// with a newest-first default baked in at creation), tiebroken by
    /// `started_at` DESC. The client is responsible for sectioning by status.
    pub async fn list_terminal_sessions(&self) -> Result<Vec<TerminalSessionMeta>> {
        let rows: Vec<TerminalSessionRow> = sqlx::query_as(
            "SELECT id, label, command, args, cwd, cols, rows, started_at, ended_at, exit_code, status, group_name, sort_order, created_by, replay_acl, pinned
             FROM terminal_sessions
             ORDER BY sort_order ASC, started_at DESC
             LIMIT 500",
        )
        .fetch_all(&self.pool)
        .await?;

        let mut out = Vec::with_capacity(rows.len());
        for row in rows {
            if let Some(meta) = hydrate_terminal_session(row) {
                out.push(meta);
            }
        }
        Ok(out)
    }

    /// Look up a single terminal session by ID.
    pub async fn get_terminal_session(
        &self,
        id: uuid::Uuid,
    ) -> Result<Option<TerminalSessionMeta>> {
        let row: Option<TerminalSessionRow> = sqlx::query_as(
            "SELECT id, label, command, args, cwd, cols, rows, started_at, ended_at, exit_code, status, group_name, sort_order, created_by, replay_acl, pinned
             FROM terminal_sessions
             WHERE id = ?
             LIMIT 1",
        )
        .bind(id.to_string())
        .fetch_optional(&self.pool)
        .await?;
        Ok(row.and_then(hydrate_terminal_session))
    }

    /// Read back the restorable spawn spec for a session (itr#590).
    ///
    /// `command`/`args`/`cwd` are returned exactly as stored (non-negotiable
    /// respawn fidelity) — a corrupt `args` blob is a hard `Err`, never a
    /// fabricated empty argv: defaulting would respawn e.g.
    /// `/bin/sh -c 'my-server'` as a bare interactive `/bin/sh` and audit it
    /// as a success (itr#591 rework MUST-FIX). A NULL or unparsable
    /// `env_json` still degrades to `env: None` (flagged via
    /// [`TerminalRespawnSpec::env_degraded`]) rather than failing the whole
    /// spec — env is best-effort, the respawn itself must never be blocked
    /// by it.
    pub async fn get_terminal_respawn_spec(
        &self,
        id: uuid::Uuid,
    ) -> Result<Option<TerminalRespawnSpec>> {
        let row: Option<(String, String, String, Option<String>)> = sqlx::query_as(
            "SELECT command, args, cwd, env_json
             FROM terminal_sessions
             WHERE id = ?
             LIMIT 1",
        )
        .bind(id.to_string())
        .fetch_optional(&self.pool)
        .await?;
        let Some((command, args_json, cwd, env_json)) = row else {
            return Ok(None);
        };
        let args: Vec<String> = serde_json::from_str(&args_json).map_err(|e| {
            anyhow!(
                "malformed args_json in the stored respawn spec for terminal session {id} \
                 (refusing to fabricate an argv): {e}"
            )
        })?;
        let env = env_json
            .as_deref()
            .and_then(|json| serde_json::from_str::<TerminalEnvSpec>(json).ok());
        let env_degraded = env_json.is_some() && env.is_none();
        Ok(Some(TerminalRespawnSpec {
            command,
            args,
            cwd: std::path::PathBuf::from(cwd),
            env,
            env_degraded,
        }))
    }

    /// Grant one resolver label explicit replay access to a session.
    pub async fn grant_terminal_replay_access(
        &self,
        id: uuid::Uuid,
        requester: &str,
    ) -> Result<()> {
        let row: Option<(Option<String>,)> =
            sqlx::query_as("SELECT replay_acl FROM terminal_sessions WHERE id = ? LIMIT 1")
                .bind(id.to_string())
                .fetch_optional(&self.pool)
                .await?;
        let Some((acl_json,)) = row else {
            return Ok(());
        };
        let mut acl = acl_json
            .as_deref()
            .and_then(|json| serde_json::from_str::<Vec<String>>(json).ok())
            .unwrap_or_default();
        if !acl.iter().any(|entry| entry == requester) {
            acl.push(requester.to_string());
            let acl_json = serde_json::to_string(&acl)?;
            sqlx::query("UPDATE terminal_sessions SET replay_acl = ? WHERE id = ?")
                .bind(acl_json)
                .bind(id.to_string())
                .execute(&self.pool)
                .await?;
        }
        Ok(())
    }

    /// Insert a batch of terminal events in a single transaction.
    ///
    /// `rows` is `(session_id, seq, ts_us, direction, payload)`.
    pub async fn insert_terminal_events_batch(
        &self,
        rows: &[(uuid::Uuid, u64, i64, TerminalDirection, Vec<u8>)],
    ) -> Result<()> {
        if rows.is_empty() {
            return Ok(());
        }
        let mut tx = self.pool.begin().await?;
        for (session_id, seq, ts_us, direction, payload) in rows {
            sqlx::query(
                "INSERT OR IGNORE INTO terminal_events (session_id, seq, ts_us, direction, payload)
                 VALUES (?, ?, ?, ?, ?)",
            )
            .bind(session_id.to_string())
            .bind(*seq as i64)
            .bind(*ts_us)
            .bind(direction.to_string())
            .bind(payload.as_slice())
            .execute(&mut *tx)
            .await?;
        }
        tx.commit().await?;
        Ok(())
    }

    /// Stream events for replay. Returns `(seq, ts_us, direction, payload)`.
    pub async fn replay_terminal_events(
        &self,
        id: uuid::Uuid,
        from_seq: Option<u64>,
    ) -> Result<Vec<(u64, i64, TerminalDirection, Vec<u8>)>> {
        let rows: Vec<(i64, i64, String, Vec<u8>)> = sqlx::query_as(
            "SELECT seq, ts_us, direction, payload
             FROM terminal_events
             WHERE session_id = ? AND seq >= ?
             ORDER BY seq ASC",
        )
        .bind(id.to_string())
        .bind(from_seq.unwrap_or(0) as i64)
        .fetch_all(&self.pool)
        .await?;

        let mut out = Vec::with_capacity(rows.len());
        for (seq, ts_us, dir, payload) in rows {
            let Ok(direction) = dir.parse::<TerminalDirection>() else {
                continue;
            };
            out.push((seq as u64, ts_us, direction, payload));
        }
        Ok(out)
    }

    /// Mark any sessions still flagged 'running' as orphaned, returning the
    /// ids of the PINNED rows swept — the respawn candidates for itr#591's
    /// reconcile-on-start. Called on daemon startup — a running session
    /// across a restart has no live PTY behind it.
    ///
    /// Candidacy (itr#591): "pinned AND running at shutdown". A row is still
    /// `running` at this point in exactly two cases — the previous daemon
    /// crashed, or its graceful shutdown deliberately preserved the row
    /// (`shutdown_all` skips the end-persist for pinned sessions). A pinned
    /// row that is *already* orphaned/exited/killed is a STALE candidate from
    /// an earlier lifecycle (or a session that died on its own): pinning it
    /// marks it important but never resurrects it, so it is not returned.
    /// The capture and the sweep are one atomic UPDATE .. RETURNING — there
    /// is no window in which a candidate can be missed or double-counted.
    ///
    /// Every swept row (pinned or not) is truthfully `orphaned` after this
    /// call; a successful respawn flips its row back to `running` via
    /// [`Self::resurrect_terminal_session`], and a failed respawn leaves it
    /// orphaned.
    pub async fn mark_running_terminals_orphaned(&self) -> Result<Vec<uuid::Uuid>> {
        let rows: Vec<(String, i64)> = sqlx::query_as(
            "UPDATE terminal_sessions
             SET status = 'orphaned', ended_at = COALESCE(ended_at, ?)
             WHERE status = 'running'
             RETURNING id, pinned",
        )
        .bind(chrono::Utc::now().to_rfc3339())
        .fetch_all(&self.pool)
        .await?;
        Ok(rows
            .into_iter()
            .filter(|(_, pinned)| *pinned != 0)
            .filter_map(|(id, _)| uuid::Uuid::parse_str(&id).ok())
            .collect())
    }

    /// Flip a swept (orphaned) session back to `running` after a successful
    /// respawn (itr#591). Clears the sweep's `ended_at` stamp and any stale
    /// exit code; the original `started_at` is preserved — the row is the
    /// same logical session, resurrected with a fresh child process.
    pub async fn resurrect_terminal_session(&self, id: uuid::Uuid) -> Result<()> {
        sqlx::query(
            "UPDATE terminal_sessions
             SET status = 'running', ended_at = NULL, exit_code = NULL
             WHERE id = ?",
        )
        .bind(id.to_string())
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    /// Highest event sequence number recorded for a session, or `None` when
    /// no events exist. A respawn (itr#591) continues the stream at max+1 so
    /// the `INSERT OR IGNORE` de-dup on `(session_id, seq)` can never
    /// silently drop the new epoch's audit bytes onto old seq numbers.
    pub async fn max_terminal_event_seq(&self, id: uuid::Uuid) -> Result<Option<u64>> {
        let row: (Option<i64>,) =
            sqlx::query_as("SELECT MAX(seq) FROM terminal_events WHERE session_id = ?")
                .bind(id.to_string())
                .fetch_one(&self.pool)
                .await?;
        Ok(row.0.map(|s| s as u64))
    }

    /// The trailing OUTPUT bytes of a session's recorded history, capped at
    /// `max_bytes` (trimmed from the front, oldest first). Used by itr#591's
    /// respawn to seed the new epoch's vt100 screen with the prior scrollback
    /// (herdr's screen-history-replay path).
    ///
    /// SECURITY INVARIANT: output direction ONLY — input-direction rows
    /// (typed sudo passwords, pasted keys) are never part of the seed.
    /// Honest limit: PTY *echo* of typed input IS output-direction bytes, so
    /// echoed input can still appear in the seed — the guarantee is
    /// "input-direction rows never seeded", not "typed secrets can never
    /// appear" (no-echo input like a sudo password stays excluded). The
    /// seed's final-screen exposure therefore equals what a live attacher
    /// already saw on screen; the seed surfaces through the same
    /// unauthenticated-to-attach catchup screen, and the full input+output
    /// history stays behind the ACL-gated, audited `term replay` path
    /// (itr#98).
    pub async fn tail_terminal_output(&self, id: uuid::Uuid, max_bytes: usize) -> Result<Vec<u8>> {
        // Newest rows first, bounded (output frames are chunked at ~4 KiB by
        // the PTY reader, so 512 rows comfortably covers any sane byte cap);
        // reassembled oldest-first below.
        let rows: Vec<(i64, Vec<u8>)> = sqlx::query_as(
            "SELECT seq, payload FROM terminal_events
             WHERE session_id = ? AND direction = 'output'
             ORDER BY seq DESC
             LIMIT 512",
        )
        .bind(id.to_string())
        .fetch_all(&self.pool)
        .await?;
        let mut total = 0usize;
        let mut kept: Vec<Vec<u8>> = Vec::new();
        for (_seq, payload) in rows {
            // Always admit the newest frame even if it alone exceeds the cap;
            // stop before admitting an older frame that would overflow it.
            if !kept.is_empty() && total + payload.len() > max_bytes {
                break;
            }
            total += payload.len();
            kept.push(payload);
            if total >= max_bytes {
                break;
            }
        }
        kept.reverse();
        Ok(kept.concat())
    }

    /// Delete terminal events older than the retention cutoff for sessions
    /// that have already ended. Metadata rows are preserved.
    pub async fn prune_terminal_events(
        &self,
        cutoff: chrono::DateTime<chrono::Utc>,
    ) -> Result<u64> {
        let res = sqlx::query(
            "DELETE FROM terminal_events
             WHERE session_id IN (
                 SELECT id FROM terminal_sessions
                 WHERE ended_at IS NOT NULL AND ended_at < ?
             )",
        )
        .bind(cutoff.to_rfc3339())
        .execute(&self.pool)
        .await?;
        Ok(res.rows_affected())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::state::test_support::test_db;

    fn make_term_meta(id: uuid::Uuid) -> TerminalSessionMeta {
        TerminalSessionMeta {
            id,
            label: Some("main".into()),
            command: "/bin/sh".into(),
            args: vec!["-c".into(), "echo hi".into()],
            cwd: std::path::PathBuf::from("/tmp"),
            cols: 80,
            rows: 24,
            started_at: chrono::Utc::now(),
            ended_at: None,
            exit_code: None,
            status: TerminalStatus::Running,
            group_name: None,
            sort_order: 0,
            created_by: None,
            replay_acl: Vec::new(),
            pinned: false,
        }
    }

    #[tokio::test]
    async fn terminal_session_create_and_list() {
        let db = test_db().await;
        let id = uuid::Uuid::new_v4();
        db.create_terminal_session(&make_term_meta(id), None)
            .await
            .unwrap();

        let list = db.list_terminal_sessions().await.unwrap();
        assert_eq!(list.len(), 1);
        assert_eq!(list[0].id, id);
        assert_eq!(list[0].command, "/bin/sh");
        assert_eq!(list[0].args, vec!["-c".to_string(), "echo hi".into()]);
        assert_eq!(list[0].status, TerminalStatus::Running);
    }

    /// itr#98: the creator identity written at create time must be readable
    /// back for the replay authorship check.
    #[tokio::test]
    async fn created_by_round_trips() {
        let db = test_db().await;
        let id = uuid::Uuid::new_v4();
        let mut meta = make_term_meta(id);
        meta.created_by = Some("human:web:dev-1".into());
        meta.replay_acl = vec!["human:web:dev-2".into()];
        db.create_terminal_session(&meta, None).await.unwrap();
        let got = db.get_terminal_session(id).await.unwrap().unwrap();
        assert_eq!(got.created_by.as_deref(), Some("human:web:dev-1"));
        assert_eq!(got.replay_acl, vec!["human:web:dev-2"]);
        // Legacy-shaped row (no creator) reads back as None.
        let legacy = uuid::Uuid::new_v4();
        db.create_terminal_session(&make_term_meta(legacy), None)
            .await
            .unwrap();
        let got = db.get_terminal_session(legacy).await.unwrap().unwrap();
        assert_eq!(got.created_by, None);
        assert!(got.replay_acl.is_empty());
    }

    #[tokio::test]
    async fn grant_terminal_replay_access_adds_acl_entry_once() {
        let db = test_db().await;
        let id = uuid::Uuid::new_v4();
        db.create_terminal_session(&make_term_meta(id), None)
            .await
            .unwrap();

        db.grant_terminal_replay_access(id, "human:web:dev-2")
            .await
            .unwrap();
        db.grant_terminal_replay_access(id, "human:web:dev-2")
            .await
            .unwrap();

        let got = db.get_terminal_session(id).await.unwrap().unwrap();
        assert_eq!(got.replay_acl, vec!["human:web:dev-2"]);
    }

    #[tokio::test]
    async fn terminal_session_end_sets_fields() {
        let db = test_db().await;
        let id = uuid::Uuid::new_v4();
        db.create_terminal_session(&make_term_meta(id), None)
            .await
            .unwrap();
        db.end_terminal_session(id, Some(0), TerminalStatus::Exited)
            .await
            .unwrap();

        let got = db.get_terminal_session(id).await.unwrap().unwrap();
        assert_eq!(got.status, TerminalStatus::Exited);
        assert_eq!(got.exit_code, Some(0));
        assert!(got.ended_at.is_some());
    }

    #[tokio::test]
    async fn terminal_events_batch_and_replay_preserve_order_and_bytes() {
        let db = test_db().await;
        let id = uuid::Uuid::new_v4();
        db.create_terminal_session(&make_term_meta(id), None)
            .await
            .unwrap();

        let rows = vec![
            (
                id,
                1u64,
                100i64,
                TerminalDirection::Output,
                b"hello\n".to_vec(),
            ),
            (id, 2, 200, TerminalDirection::Input, b"yes\r".to_vec()),
            (
                id,
                3,
                300,
                TerminalDirection::Output,
                vec![0x1b, b'[', b'3', b'1', b'm'],
            ),
        ];
        db.insert_terminal_events_batch(&rows).await.unwrap();

        let replayed = db.replay_terminal_events(id, None).await.unwrap();
        assert_eq!(replayed.len(), 3);
        assert_eq!(replayed[0].0, 1);
        assert_eq!(replayed[0].2, TerminalDirection::Output);
        assert_eq!(replayed[0].3, b"hello\n");
        assert_eq!(replayed[2].3, vec![0x1b, b'[', b'3', b'1', b'm']);

        let from_two = db.replay_terminal_events(id, Some(2)).await.unwrap();
        assert_eq!(from_two.len(), 2);
        assert_eq!(from_two[0].0, 2);
    }

    #[tokio::test]
    async fn terminal_group_and_sort_order_round_trip() {
        let db = test_db().await;
        // Three sessions with distinct ids. Leave group/sort_order at defaults.
        let ids: Vec<uuid::Uuid> = (0..3).map(|_| uuid::Uuid::new_v4()).collect();
        for id in &ids {
            db.create_terminal_session(&make_term_meta(*id), None)
                .await
                .unwrap();
        }

        // Assign the first two to a group, reorder them.
        db.set_terminal_group(ids[0], Some("frontend"))
            .await
            .unwrap();
        db.set_terminal_group(ids[1], Some("frontend"))
            .await
            .unwrap();
        db.set_terminal_sort_order(ids[0], 200).await.unwrap();
        db.set_terminal_sort_order(ids[1], 100).await.unwrap();
        db.set_terminal_sort_order(ids[2], 50).await.unwrap();

        let list = db.list_terminal_sessions().await.unwrap();
        // Ordered by sort_order ASC: ids[2] (50), ids[1] (100), ids[0] (200).
        assert_eq!(list[0].id, ids[2]);
        assert_eq!(list[0].group_name, None);
        assert_eq!(list[0].sort_order, 50);
        assert_eq!(list[1].id, ids[1]);
        assert_eq!(list[1].group_name.as_deref(), Some("frontend"));
        assert_eq!(list[2].id, ids[0]);
        assert_eq!(list[2].group_name.as_deref(), Some("frontend"));

        // Clearing the group (None) removes the label.
        db.set_terminal_group(ids[0], None).await.unwrap();
        let after = db.list_terminal_sessions().await.unwrap();
        let found = after.iter().find(|m| m.id == ids[0]).unwrap();
        assert_eq!(found.group_name, None);
    }

    /// itr#589 AC#1 PIN: the pin flag persists in SQLite and round-trips
    /// through both the single-row and list read paths; toggling off
    /// restores the unpinned default.
    #[tokio::test]
    async fn terminal_pinned_flag_persists_and_toggles() {
        let db = test_db().await;
        let id = uuid::Uuid::new_v4();
        db.create_terminal_session(&make_term_meta(id), None)
            .await
            .unwrap();

        // Default: unpinned.
        let got = db.get_terminal_session(id).await.unwrap().unwrap();
        assert!(!got.pinned);

        db.set_terminal_pinned(id, true).await.unwrap();
        let got = db.get_terminal_session(id).await.unwrap().unwrap();
        assert!(got.pinned);
        let listed = db.list_terminal_sessions().await.unwrap();
        assert!(listed.iter().find(|m| m.id == id).unwrap().pinned);

        db.set_terminal_pinned(id, false).await.unwrap();
        let got = db.get_terminal_session(id).await.unwrap().unwrap();
        assert!(!got.pinned);

        // A meta created already-pinned persists the flag at insert.
        let pinned_id = uuid::Uuid::new_v4();
        let mut meta = make_term_meta(pinned_id);
        meta.pinned = true;
        db.create_terminal_session(&meta, None).await.unwrap();
        let got = db.get_terminal_session(pinned_id).await.unwrap().unwrap();
        assert!(got.pinned);
    }

    /// itr#589 AC#2, superseded by itr#591: the startup sweep still orphans
    /// pinned rows at the DB level (truthful — no PTY exists at sweep time),
    /// but now RETURNS them as respawn candidates. The flag itself survives
    /// the sweep (sticky pin: the user unpins when done).
    #[tokio::test]
    async fn pinned_sessions_still_orphaned_on_startup_sweep() {
        let db = test_db().await;
        let id = uuid::Uuid::new_v4();
        db.create_terminal_session(&make_term_meta(id), None)
            .await
            .unwrap();
        db.set_terminal_pinned(id, true).await.unwrap();

        let candidates = db.mark_running_terminals_orphaned().await.unwrap();
        assert_eq!(
            candidates,
            vec![id],
            "a pinned running row is a respawn candidate (itr#591)"
        );

        let got = db.get_terminal_session(id).await.unwrap().unwrap();
        assert_eq!(
            got.status,
            TerminalStatus::Orphaned,
            "the sweep stays truthful: no PTY exists until the respawn lands"
        );
        assert!(
            got.pinned,
            "the flag survives the sweep for the reconciler to honor"
        );
    }

    /// itr#591 AC candidacy PIN: only pinned rows still `running` at sweep
    /// time qualify. Unpinned running rows sweep silently; pinned rows
    /// already orphaned (stale, from an earlier lifecycle) or exited (died
    /// on their own) are marked important but never resurrected.
    #[tokio::test]
    async fn sweep_candidacy_is_pinned_and_running_only() {
        let db = test_db().await;

        let fresh_pinned = uuid::Uuid::new_v4();
        db.create_terminal_session(&make_term_meta(fresh_pinned), None)
            .await
            .unwrap();
        db.set_terminal_pinned(fresh_pinned, true).await.unwrap();

        let running_unpinned = uuid::Uuid::new_v4();
        db.create_terminal_session(&make_term_meta(running_unpinned), None)
            .await
            .unwrap();

        // Stale: pinned but already orphaned before this startup.
        let stale_pinned = uuid::Uuid::new_v4();
        let mut stale = make_term_meta(stale_pinned);
        stale.status = TerminalStatus::Orphaned;
        stale.pinned = true;
        db.create_terminal_session(&stale, None).await.unwrap();

        // Exited on its own: pinning marks it important, not resurrectable.
        let exited_pinned = uuid::Uuid::new_v4();
        db.create_terminal_session(&make_term_meta(exited_pinned), None)
            .await
            .unwrap();
        db.set_terminal_pinned(exited_pinned, true).await.unwrap();
        db.end_terminal_session(exited_pinned, Some(0), TerminalStatus::Exited)
            .await
            .unwrap();

        let candidates = db.mark_running_terminals_orphaned().await.unwrap();
        assert_eq!(candidates, vec![fresh_pinned]);

        // A second sweep returns nothing — the fresh candidate is orphaned
        // now, i.e. stale for any later lifecycle unless resurrected.
        let again = db.mark_running_terminals_orphaned().await.unwrap();
        assert!(again.is_empty(), "swept candidates must not re-qualify");
    }

    /// itr#591 PIN: resurrect flips a swept row back to running, clearing the
    /// sweep's ended_at/exit_code stamps and preserving pin + started_at.
    #[tokio::test]
    async fn resurrect_restores_running_and_clears_end_fields() {
        let db = test_db().await;
        let id = uuid::Uuid::new_v4();
        let meta = make_term_meta(id);
        db.create_terminal_session(&meta, None).await.unwrap();
        db.set_terminal_pinned(id, true).await.unwrap();
        db.mark_running_terminals_orphaned().await.unwrap();

        db.resurrect_terminal_session(id).await.unwrap();

        let got = db.get_terminal_session(id).await.unwrap().unwrap();
        assert_eq!(got.status, TerminalStatus::Running);
        assert!(got.ended_at.is_none(), "sweep's ended_at stamp cleared");
        assert!(got.exit_code.is_none());
        assert!(got.pinned, "pin is sticky across resurrect");
        assert_eq!(
            got.started_at.timestamp(),
            meta.started_at.timestamp(),
            "same logical session: started_at preserved"
        );
    }

    /// itr#591 PIN: seq continuation + output-only scrollback seed. The tail
    /// never contains input-direction bytes (the no-echo typed secret below
    /// models a sudo password), trims oldest-first at the byte cap, and
    /// max_terminal_event_seq points past the recorded stream. Honest
    /// caveat: PTY echo of typed input arrives as OUTPUT-direction bytes and
    /// therefore CAN appear in the seed — the pinned guarantee is
    /// "input-direction rows never seeded", not "typed secrets can never
    /// appear" (see `tail_terminal_output`).
    #[tokio::test]
    async fn max_seq_and_output_tail_for_respawn_seed() {
        let db = test_db().await;
        let id = uuid::Uuid::new_v4();
        db.create_terminal_session(&make_term_meta(id), None)
            .await
            .unwrap();

        assert_eq!(db.max_terminal_event_seq(id).await.unwrap(), None);
        assert!(
            db.tail_terminal_output(id, 1024).await.unwrap().is_empty(),
            "no history yields an empty seed"
        );

        db.insert_terminal_events_batch(&[
            (id, 1, 100, TerminalDirection::Output, b"old-old-".to_vec()),
            (
                id,
                2,
                200,
                TerminalDirection::Input,
                b"hunter2-typed-secret".to_vec(),
            ),
            (id, 3, 300, TerminalDirection::Output, b"middle-".to_vec()),
            (id, 4, 400, TerminalDirection::Resize, b"80,24".to_vec()),
            (id, 5, 500, TerminalDirection::Output, b"newest".to_vec()),
        ])
        .await
        .unwrap();

        assert_eq!(db.max_terminal_event_seq(id).await.unwrap(), Some(5));

        let full = db.tail_terminal_output(id, 1024).await.unwrap();
        assert_eq!(full, b"old-old-middle-newest".to_vec());

        // Byte cap trims the OLDEST frames first; input/resize bytes are
        // never present regardless of the cap.
        let capped = db.tail_terminal_output(id, 13).await.unwrap();
        assert_eq!(capped, b"middle-newest".to_vec());
        let tiny = db.tail_terminal_output(id, 1).await.unwrap();
        assert_eq!(
            tiny,
            b"newest".to_vec(),
            "the newest frame is always admitted even past the cap"
        );
        for seed in [&full, &capped, &tiny] {
            assert!(
                !String::from_utf8_lossy(seed).contains("hunter2"),
                "input bytes leaked into the respawn seed"
            );
        }
    }

    #[tokio::test]
    async fn mark_running_orphaned_on_startup() {
        let db = test_db().await;
        let id = uuid::Uuid::new_v4();
        db.create_terminal_session(&make_term_meta(id), None)
            .await
            .unwrap();
        // Directly invoke the sweeper (also runs inside StateDb::open).
        let candidates = db.mark_running_terminals_orphaned().await.unwrap();
        assert!(
            candidates.is_empty(),
            "unpinned rows are swept but never respawn candidates"
        );
        let got = db.get_terminal_session(id).await.unwrap().unwrap();
        assert_eq!(got.status, TerminalStatus::Orphaned);
        assert!(got.ended_at.is_some());
    }

    /// itr#590 PIN: classification is exactly the shared itr#89 scrubber's
    /// verdict — secret-named keys and secret-prefixed/lead-word values go
    /// NAME-ONLY; benign vars persist verbatim; unsafe names drop entirely.
    #[test]
    fn env_spec_classification_follows_shared_scrubber() {
        let env: HashMap<String, String> = [
            // Benign: stored verbatim, including a value with a backslash
            // (must NOT be log-escaped — respawn needs the raw value).
            ("MY_MARKER".to_string(), "hello-verbatim".to_string()),
            ("WEIRD_PATH".to_string(), "C:\\path\\to\\file".to_string()),
            // Secret-named key: NAME-ONLY regardless of value.
            (
                "MY_API_KEY".to_string(),
                "not-even-secret-looking".to_string(),
            ),
            // Benign name, secret-prefixed value: NAME-ONLY.
            ("INNOCENT".to_string(), "ghp_16chartoken1234".to_string()),
            // Benign name, lead-word credential inside the value: NAME-ONLY.
            ("HEADER".to_string(), "Bearer abc.def.ghi".to_string()),
            // Value already carrying the marker: conservative NAME-ONLY.
            ("ODD".to_string(), format!("x {REDACTED} y")),
            // Name the scrubber would alter: dropped entirely.
            ("sk-abcdef123456".to_string(), "1".to_string()),
        ]
        .into();
        let spec = TerminalEnvSpec::from_requested_env(&env).expect("nonempty spec");
        assert_eq!(
            spec.set.get("MY_MARKER").map(String::as_str),
            Some("hello-verbatim")
        );
        assert_eq!(
            spec.set.get("WEIRD_PATH").map(String::as_str),
            Some("C:\\path\\to\\file"),
            "non-secret values must persist raw, not log-escaped"
        );
        assert_eq!(
            spec.inherit_names,
            vec!["HEADER", "INNOCENT", "MY_API_KEY", "ODD"]
        );
        assert_eq!(
            spec.set.len(),
            2,
            "no secret var may land in the verbatim set"
        );
        assert!(
            !spec.set.contains_key("sk-abcdef123456"),
            "secret-shaped names must be dropped, not persisted"
        );

        // Empty input → nothing to persist.
        assert_eq!(TerminalEnvSpec::from_requested_env(&HashMap::new()), None);
        // All-dropped input → nothing to persist either.
        let all_dropped: HashMap<String, String> =
            [("sk-abcdef123456".to_string(), "1".to_string())].into();
        assert_eq!(TerminalEnvSpec::from_requested_env(&all_dropped), None);
    }

    /// itr#590 review PIN (gap-CLASS closure): any (name, value) pair the
    /// shared scrubber's `redact_value` would REDACT (marker in the scrubbed
    /// one-entry JSON) must classify NAME-ONLY or DROPPED — never verbatim.
    /// Pairs `redact_value` merely log-escapes (reversible transport
    /// encoding, itr#530 — not redaction) may stay verbatim.
    #[test]
    fn env_spec_corpus_never_persists_what_redact_value_would_redact() {
        // The first seven entries are the review-confirmed leak cases of the
        // text-probe-only classifier plus the empty/whitespace-value pin
        // (NOTE#4: a secret-named key with an empty or whitespace-only value
        // classifies NAME-ONLY — redact_value's key rule is unconditional).
        let corpus: &[(&str, &str)] = &[
            ("MY_API_KEY", " hunter2-super-secret"),    // leading space
            ("GITHUB_TOKEN", "\thunter2-super-secret"), // leading tab
            ("DB_PASSWORD", "\nhunter2-super-secret"),  // leading newline
            ("MY_API_KEY ", "hunter2-super-secret"),    // trailing-space name
            ("A=TOKEN", "hunter2-super-secret"),        // '=' inside the name
            ("MY_API_KEY", ""),                         // empty value
            ("MY_API_KEY", "   "),                      // whitespace-only value
            // Benign controls — must persist verbatim:
            ("MY_MARKER", "hello-verbatim"),
            ("WEIRD_PATH", "C:\\path\\to\\file"), // escape-only alteration
            ("SEARCH_PATH", "/usr/bin:/bin"),
        ];

        for (name, value) in corpus {
            let mut probe = serde_json::Map::with_capacity(1);
            probe.insert(
                (*name).to_string(),
                serde_json::Value::String((*value).to_string()),
            );
            let would_redact = redact_value(&serde_json::Value::Object(probe))
                .to_string()
                .contains(REDACTED);

            let env: HashMap<String, String> = [((*name).to_string(), (*value).to_string())].into();
            let spec = TerminalEnvSpec::from_requested_env(&env);
            let verbatim = spec.as_ref().is_some_and(|s| s.set.contains_key(*name));

            if would_redact {
                assert!(
                    !verbatim,
                    "scrubber would redact {name:?}={value:?} but it persisted verbatim"
                );
                if let Some(s) = &spec {
                    assert!(
                        s.set.values().all(|v| !v.contains("hunter2")),
                        "secret value leaked into the verbatim set for {name:?}"
                    );
                }
            } else {
                assert!(
                    verbatim,
                    "benign pair {name:?}={value:?} must persist verbatim \
                     (escape-only alteration is not redaction)"
                );
            }
        }

        // The seven secret-side cases specifically classify NAME-ONLY (kept
        // as inherit-current markers, not dropped): respawn re-sources them
        // from the daemon's live environment.
        for (name, value) in &corpus[..7] {
            let env: HashMap<String, String> = [((*name).to_string(), (*value).to_string())].into();
            let spec = TerminalEnvSpec::from_requested_env(&env)
                .unwrap_or_else(|| panic!("spec expected for {name:?}"));
            assert!(spec.set.is_empty(), "no verbatim entry for {name:?}");
            assert_eq!(
                spec.inherit_names,
                vec![(*name).to_string()],
                "expected NAME-ONLY classification for {name:?}={value:?}"
            );
        }
    }

    /// itr#590 review PIN: garbage in `env_json` degrades to `env: None` —
    /// the respawn spec's cwd/command fidelity is never blocked by a bad
    /// env blob.
    #[tokio::test]
    async fn respawn_spec_tolerates_malformed_env_json() {
        let db = test_db().await;
        let id = uuid::Uuid::new_v4();
        db.create_terminal_session(&make_term_meta(id), None)
            .await
            .unwrap();
        sqlx::query("UPDATE terminal_sessions SET env_json = ? WHERE id = ?")
            .bind("{not json at all")
            .bind(id.to_string())
            .execute(db.pool())
            .await
            .unwrap();

        let spec = db.get_terminal_respawn_spec(id).await.unwrap().unwrap();
        assert_eq!(spec.command, "/bin/sh");
        assert_eq!(spec.args, vec!["-c".to_string(), "echo hi".into()]);
        assert_eq!(spec.cwd, std::path::PathBuf::from("/tmp"));
        assert_eq!(spec.env, None, "malformed env_json must degrade to None");
        assert!(
            spec.env_degraded,
            "the degradation must be flagged for the respawn audit detail"
        );
    }

    /// itr#591 rework MUST-FIX PIN: a corrupt `args` blob fails the spec
    /// read LOUDLY — never a fabricated empty argv, which would respawn
    /// `/bin/sh -c 'cmd'` as a bare interactive `/bin/sh` and audit it as a
    /// success. (Contrast with `env_json`, which degrades best-effort.)
    #[tokio::test]
    async fn respawn_spec_fails_loud_on_malformed_args_json() {
        let db = test_db().await;
        let id = uuid::Uuid::new_v4();
        db.create_terminal_session(&make_term_meta(id), None)
            .await
            .unwrap();
        sqlx::query("UPDATE terminal_sessions SET args = ? WHERE id = ?")
            .bind("{not an argv")
            .bind(id.to_string())
            .execute(db.pool())
            .await
            .unwrap();

        let err = db.get_terminal_respawn_spec(id).await.unwrap_err();
        let msg = err.to_string();
        assert!(msg.contains("malformed args_json"), "{msg}");
        assert!(
            msg.contains(&id.to_string()),
            "error must name the session: {msg}"
        );
    }

    /// itr#590 AC#2 PIN: a secret-bearing env value NEVER lands in the DB in
    /// cleartext — the raw `env_json` column carries the name only.
    #[tokio::test]
    async fn env_spec_secret_values_never_persist_cleartext() {
        let db = test_db().await;
        let id = uuid::Uuid::new_v4();
        let env: HashMap<String, String> = [
            ("MY_MARKER".to_string(), "hello-verbatim".to_string()),
            ("MY_API_KEY".to_string(), "sk-livesecret123456".to_string()),
        ]
        .into();
        let spec = TerminalEnvSpec::from_requested_env(&env).expect("nonempty spec");
        db.create_terminal_session(&make_term_meta(id), Some(&spec))
            .await
            .unwrap();

        let (env_json,): (Option<String>,) =
            sqlx::query_as("SELECT env_json FROM terminal_sessions WHERE id = ?")
                .bind(id.to_string())
                .fetch_one(db.pool())
                .await
                .unwrap();
        let env_json = env_json.expect("env_json populated");
        assert!(
            !env_json.contains("sk-livesecret123456"),
            "cleartext secret leaked into env_json: {env_json}"
        );
        assert!(!env_json.contains(REDACTED), "no placeholder values either");
        assert!(env_json.contains("MY_API_KEY"), "name-only marker present");
        assert!(env_json.contains("hello-verbatim"), "non-secret verbatim");
    }

    /// itr#590 PIN: materialize re-sources secret names from the CURRENT
    /// daemon environment (inherit-current, not replay-stale) and omits
    /// names the daemon no longer carries.
    #[test]
    fn env_spec_materialize_inherits_current_not_stale() {
        let spec = TerminalEnvSpec {
            set: [("MY_MARKER".to_string(), "hello-verbatim".to_string())].into(),
            inherit_names: vec!["GONE_KEY".into(), "MY_API_KEY".into()],
        };
        let daemon_env: HashMap<String, String> =
            [("MY_API_KEY".to_string(), "sk-freshdaemon123456".to_string())].into();
        let out = spec.materialize(&daemon_env);
        assert_eq!(
            out.get("MY_MARKER").map(String::as_str),
            Some("hello-verbatim")
        );
        assert_eq!(
            out.get("MY_API_KEY").map(String::as_str),
            Some("sk-freshdaemon123456"),
            "secret must come from the live daemon env"
        );
        assert!(!out.contains_key("GONE_KEY"), "missing names are omitted");
    }

    /// itr#590 AC#1/AC#3 PIN: the stored spec round-trips command/args/cwd
    /// exactly; legacy NULL-env rows read back with `env: None`.
    #[tokio::test]
    async fn respawn_spec_round_trips_and_legacy_null_env() {
        let db = test_db().await;

        let id = uuid::Uuid::new_v4();
        let spec = TerminalEnvSpec {
            set: [("MY_MARKER".to_string(), "hello-verbatim".to_string())].into(),
            inherit_names: vec!["MY_API_KEY".into()],
        };
        db.create_terminal_session(&make_term_meta(id), Some(&spec))
            .await
            .unwrap();
        let got = db.get_terminal_respawn_spec(id).await.unwrap().unwrap();
        assert_eq!(got.command, "/bin/sh");
        assert_eq!(got.args, vec!["-c".to_string(), "echo hi".into()]);
        assert_eq!(got.cwd, std::path::PathBuf::from("/tmp"));
        assert_eq!(got.env.as_ref(), Some(&spec));

        // Legacy-shaped row (env_json NULL): spec still restorable, env None.
        let legacy = uuid::Uuid::new_v4();
        db.create_terminal_session(&make_term_meta(legacy), None)
            .await
            .unwrap();
        let got = db.get_terminal_respawn_spec(legacy).await.unwrap().unwrap();
        assert_eq!(got.command, "/bin/sh");
        assert_eq!(got.env, None);
        assert!(
            !got.env_degraded,
            "NULL env_json is legacy-clean, not degraded"
        );

        // Unknown id → None.
        assert!(
            db.get_terminal_respawn_spec(uuid::Uuid::new_v4())
                .await
                .unwrap()
                .is_none()
        );
    }

    /// Regression (itr#215 sec#5): `open_client` MUST NOT run the
    /// orphan-sweeper, otherwise CLI admin commands running against a
    /// shared DB will corrupt a live daemon's terminal sessions by
    /// flipping `running` rows to `orphaned` on every invocation.
    ///
    /// The test uses a file-backed DB because `:memory:` opens create a
    /// fresh DB per connection — `open_client` and our first test handle
    /// would see different data and the test would false-pass.
    #[tokio::test]
    async fn open_client_does_not_orphan_running_sessions() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("state.db");
        let path_s = path.to_str().unwrap();

        // Simulate the daemon creating a running session.
        let daemon_db = StateDb::open(path_s).await.unwrap();
        let id = uuid::Uuid::new_v4();
        daemon_db
            .create_terminal_session(&make_term_meta(id), None)
            .await
            .unwrap();

        // CLI client opens the same DB — must NOT flip running → orphaned.
        let _client_db = StateDb::open_client(path_s).await.unwrap();

        let got = daemon_db.get_terminal_session(id).await.unwrap().unwrap();
        assert_eq!(
            got.status,
            TerminalStatus::Running,
            "open_client orphaned a running session — CLI would corrupt daemon state"
        );
        assert!(got.ended_at.is_none());
    }
}
