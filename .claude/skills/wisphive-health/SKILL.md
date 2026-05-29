---
name: wisphive-health
description: Check the operational/storage health of the local Wisphive install — daemon liveness vs stale socket/PID, SQLite bloat (terminal_events), un-checkpointed WAL, unrotated JSONL logs, and effective gating posture. Trigger when the user asks to "check wisphive health", "is the daemon healthy", "why is wisphive slow/dead", "check for db bloat", "diagnose wisphive", or after a daemon crash/restart. Project-only. Reports first; only mutates state on explicit confirmation.
---

# Wisphive Health Check

Diagnoses the operational failure modes that `wisphive doctor` does **not** cover.
`wisphive doctor` checks hooks/mode/daemon-process/socket/permissions; this skill
checks **storage and runtime rot** — the class of problem that once ballooned the
DB to ~2GB and silently killed the daemon on a startup `VACUUM`.

## Step 1 — Run the read-only report

Always start here. The script never mutates state.

```bash
scripts/wisphive-health.sh
```

It prints PASS/WARN/FAIL per check with a remediation hint, and exits non-zero on
any FAIL. Override the install dir with `WISPHIVE_HOME=/path scripts/wisphive-health.sh`.

What it inspects and why each matters:

| Check | Failure it catches |
|-------|--------------------|
| Daemon liveness vs **stale socket/PID** | A dead daemon that left `wisphive.sock` behind makes permission-style hooks block on `ECONNREFUSED` instead of failing open — it breaks live agent tool calls. |
| **`terminal_events` row count** | `prune_terminal_events` is defined but never wired into the retention loop, so PTY output grows unbounded. This is the primary historical root cause. |
| **DB size** | Direct consequence of the above; a multi-GB DB makes the startup `VACUUM` slow/OOM-prone. |
| **WAL size** | A WAL that never checkpoints down (e.g. 1GB) signals a pinned read txn or an interrupted VACUUM. |
| **Unrotated JSONL logs** | `events.jsonl`, `logs/decision_log.jsonl`, `hook-debug.jsonl` are append-only with no rotation. |
| **Gating posture** | `mode=active` + `auto_approve_level=all` means nothing is actually gated. |
| **Footprint + leftover backups** | Surfaces `wisphive.db.bak-*` recovery backups left to clean up. |

## Step 2 — Interpret

- **All PASS / only WARN** → report the summary and stop. Do not mutate anything.
- **Any FAIL** → summarize the failing checks and the root cause, then propose the
  matching remediation from Step 3. **Get explicit confirmation before any destructive
  step** (anything that deletes rows, truncates logs, or removes files).

## Step 3 — Remediation (only on explicit confirmation)

Confirm the daemon is **not running** before touching the DB or socket
(`pgrep -fl "wisphive daemon"`). Back up before destructive DB ops.

**Stale socket / PID (FAIL):**
```bash
# only if no live daemon
rm -f ~/.wisphive/wisphive.sock ~/.wisphive/wisphive.pid
wisphive daemon start
```

**DB bloat / unbounded terminal_events (FAIL):** stop the daemon first.
```bash
cp ~/.wisphive/wisphive.db ~/.wisphive/wisphive.db.bak-$(date +%Y%m%d-%H%M%S)
sqlite3 ~/.wisphive/wisphive.db \
  "DELETE FROM terminal_events WHERE session_id IN
     (SELECT id FROM terminal_sessions WHERE ended_at IS NOT NULL);"
sqlite3 ~/.wisphive/wisphive.db "PRAGMA wal_checkpoint(TRUNCATE); VACUUM;"
sqlite3 ~/.wisphive/wisphive.db "PRAGMA quick_check;"   # expect: ok
```

**Oversized JSONL log (FAIL):** archive then truncate in place (preserves inode/perms).
```bash
f=~/.wisphive/logs/decision_log.jsonl   # or events.jsonl / hook-debug.jsonl
gzip -c "$f" > "$f.$(date +%Y%m%d-%H%M%S).gz" && : > "$f"
```

**Gating posture WARN:** user's call — to re-enable gating:
`wisphive config auto-approve level <off|read|write|execute>`.

**Leftover backup WARN:** delete once the live DB is confirmed healthy:
`rm ~/.wisphive/wisphive.db.bak-*`.

## Step 4 — Verify

Re-run `scripts/wisphive-health.sh` and confirm the FAILs cleared and the daemon
stays up (`wisphive daemon status` twice, a few seconds apart).

## Note on permanent fixes

This skill treats symptoms. The durable fixes live in the code and should be tracked
separately (e.g. via `itr`): wire `prune_terminal_events` into the retention loop,
size-guard the startup `VACUUM`, and add rotation for the JSONL logs.
