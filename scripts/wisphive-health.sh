#!/usr/bin/env bash
#
# wisphive-health.sh — operational/storage health check for a Wisphive install.
#
# Complements `wisphive doctor` (which checks hooks/mode/daemon-process/socket/
# permissions). This script focuses on the failure modes `doctor` does NOT cover:
# unbounded SQLite growth (terminal_events), an un-checkpointed WAL, unrotated
# append-only JSONL logs, a stale socket/PID left by a dead daemon, and an
# effective gating posture of "off". These are the conditions that ballooned the
# DB to ~2GB and silently killed the daemon on a startup VACUUM.
#
# Read-only: it inspects and reports, never mutates state. Prints PASS/WARN/FAIL
# per check and a remediation hint for anything not green.
#
# Exit codes: 0 = all PASS (warnings allowed), 1 = at least one FAIL, 2 = setup
# problem (e.g. missing sqlite3).
#
# Usage:
#   scripts/wisphive-health.sh            # human-readable report
#   WISPHIVE_HOME=/path scripts/wisphive-health.sh   # override home dir

set -u

WISPHIVE_HOME="${WISPHIVE_HOME:-$HOME/.wisphive}"
DB="$WISPHIVE_HOME/wisphive.db"

# --- thresholds (bytes / rows) ----------------------------------------------
MB=$((1024 * 1024))
DB_WARN=$((200 * MB));        DB_FAIL=$((500 * MB))
WAL_WARN=$((128 * MB));       WAL_FAIL=$((512 * MB))
TEVENTS_WARN=200000;          TEVENTS_FAIL=1000000
LOG_WARN=$((25 * MB));        LOG_FAIL=$((100 * MB))
FOOTPRINT_WARN=$((500 * MB)); FOOTPRINT_FAIL=$((2000 * MB))

fails=0
warns=0

# --- helpers ----------------------------------------------------------------
# Portable file size in bytes (0 if missing).
fsize() {
  [ -e "$1" ] || { echo 0; return; }
  if stat -f%z "$1" >/dev/null 2>&1; then stat -f%z "$1"; else stat -c%s "$1"; fi
}

human() { # bytes -> human
  awk -v b="$1" 'BEGIN{
    split("B KB MB GB TB",u," "); i=1;
    while (b>=1024 && i<5){b/=1024;i++}
    printf (i==1?"%d%s":"%.1f%s"), b, u[i]
  }'
}

pass() { printf "  \033[32mPASS\033[0m  %s\n" "$1"; }
warn() { printf "  \033[33mWARN\033[0m  %s\n     ↳ %s\n" "$1" "$2"; warns=$((warns+1)); }
fail() { printf "  \033[31mFAIL\033[0m  %s\n     ↳ %s\n" "$1" "$2"; fails=$((fails+1)); }

# Evaluate a numeric value against warn/fail thresholds.
# args: label value warn_threshold fail_threshold human_flag warn_fix fail_fix
check_threshold() {
  local label="$1" val="$2" wt="$3" ft="$4" hf="$5" wfix="$6" ffix="$7" shown
  if [ "$hf" = "human" ]; then shown="$(human "$val")"; else shown="$val"; fi
  if [ "$val" -ge "$ft" ]; then
    fail "$label: $shown" "$ffix"
  elif [ "$val" -ge "$wt" ]; then
    warn "$label: $shown" "$wfix"
  else
    pass "$label: $shown"
  fi
}

proc_alive() { kill -0 "$1" >/dev/null 2>&1; }

echo "Wisphive health — $WISPHIVE_HOME"
echo

if [ ! -d "$WISPHIVE_HOME" ]; then
  fail "wisphive home missing" "$WISPHIVE_HOME does not exist — is Wisphive installed?"
  exit 1
fi

# --- 1. daemon liveness vs stale socket/PID ---------------------------------
echo "Daemon & socket"
pidfile="$WISPHIVE_HOME/wisphive.pid"
sock="$WISPHIVE_HOME/wisphive.sock"
daemon_pid=""
if [ -f "$pidfile" ]; then
  daemon_pid="$(tr -d '[:space:]' < "$pidfile")"
  if [ -n "$daemon_pid" ] && proc_alive "$daemon_pid"; then
    pass "daemon running (pid $daemon_pid)"
  else
    fail "stale PID file (pid ${daemon_pid:-?} not alive)" \
         "daemon died without cleanup — rm '$pidfile'; restart with 'wisphive daemon start'"
  fi
else
  warn "no PID file" "daemon not running — start with 'wisphive daemon start' if expected"
fi

if [ -S "$sock" ]; then
  if [ -n "$daemon_pid" ] && proc_alive "$daemon_pid"; then
    pass "socket present with live daemon"
  else
    fail "stale socket (no live daemon)" \
         "this BLOCKS permission-style hooks (ECONNREFUSED) — rm '$sock' to fail-open cleanly"
  fi
else
  [ -n "$daemon_pid" ] && proc_alive "$daemon_pid" 2>/dev/null \
    && fail "daemon alive but socket missing" "restart the daemon" \
    || pass "no socket (consistent with stopped daemon)"
fi
echo

# --- 2. gating posture ------------------------------------------------------
echo "Gating posture"
mode="$(tr -d '[:space:]' < "$WISPHIVE_HOME/mode" 2>/dev/null || echo "")"
level="$(sed -n 's/.*"auto_approve_level"[[:space:]]*:[[:space:]]*"\([a-z]*\)".*/\1/p' \
          "$WISPHIVE_HOME/config.json" 2>/dev/null)"
if [ "$mode" = "active" ]; then
  if [ "$level" = "all" ]; then
    warn "mode=active but auto_approve_level=all" \
         "every tool call is auto-approved — nothing is gated; lower with 'wisphive config auto-approve level <off|read|write|execute>'"
  else
    pass "mode=active, auto_approve_level=${level:-default}"
  fi
else
  warn "mode='${mode:-unset}' (gating disabled)" \
       "kill switch is off — 'echo active > $WISPHIVE_HOME/mode' to enable gating"
fi
echo

# --- 3. SQLite storage health -----------------------------------------------
echo "SQLite storage"
db_bytes="$(fsize "$DB")"
check_threshold "db size" "$db_bytes" "$DB_WARN" "$DB_FAIL" human \
  "review retention; consider pruning terminal_events" \
  "DB bloated — prune terminal_events for ended sessions, then VACUUM (see skill remediation)"

wal_bytes="$(fsize "$DB-wal")"
check_threshold "WAL size" "$wal_bytes" "$WAL_WARN" "$WAL_FAIL" human \
  "WAL growing — a long-lived read txn may be pinning it" \
  "WAL not checkpointing — 'sqlite3 $DB \"PRAGMA wal_checkpoint(TRUNCATE);\"' (daemon must be idle)"

if command -v sqlite3 >/dev/null 2>&1; then
  if [ -f "$DB" ]; then
    tev="$(sqlite3 "$DB" 'SELECT COUNT(*) FROM terminal_events;' 2>/dev/null || echo "")"
    if [ -n "$tev" ]; then
      check_threshold "terminal_events rows" "$tev" "$TEVENTS_WARN" "$TEVENTS_FAIL" plain \
        "terminal output accumulating — prune_terminal_events is not wired into retention" \
        "terminal_events unbounded — DELETE events for ended sessions + VACUUM (root-cause: prune_terminal_events never called)"
      ended_ev="$(sqlite3 "$DB" 'SELECT COUNT(*) FROM terminal_events WHERE session_id IN (SELECT id FROM terminal_sessions WHERE ended_at IS NOT NULL);' 2>/dev/null || echo 0)"
      [ "${ended_ev:-0}" -gt 0 ] && echo "          (${ended_ev} rows belong to ended sessions and are prunable now)"
    fi
  fi
else
  warn "sqlite3 not found" "install sqlite3 to inspect terminal_events row counts"
fi
echo

# --- 4. append-only JSONL logs ----------------------------------------------
echo "Append-only logs"
for rel in events.jsonl hook-debug.jsonl logs/decision_log.jsonl; do
  f="$WISPHIVE_HOME/$rel"
  [ -e "$f" ] || { pass "$rel: absent"; continue; }
  check_threshold "$rel" "$(fsize "$f")" "$LOG_WARN" "$LOG_FAIL" human \
    "growing unrotated — gzip-archive + truncate periodically" \
    "very large unrotated log — 'gzip -c \"$f\" > \"$f.\$(date +%Y%m%d).gz\" && : > \"$f\"'"
done
echo

# --- 5. total footprint -----------------------------------------------------
echo "Footprint"
fp_bytes="$(du -sk "$WISPHIVE_HOME" 2>/dev/null | awk '{print $1*1024}')"
fp_bytes="${fp_bytes:-0}"
check_threshold "total ~/.wisphive" "$fp_bytes" "$FOOTPRINT_WARN" "$FOOTPRINT_FAIL" human \
  "review largest files: du -ah '$WISPHIVE_HOME' | sort -rh | head" \
  "large footprint — check for db backups (*.bak-*), big WAL, or unrotated logs"
# Surface leftover recovery backups explicitly.
for bak in "$WISPHIVE_HOME"/wisphive.db.bak-*; do
  [ -e "$bak" ] || continue
  warn "leftover backup: $(basename "$bak") ($(human "$(fsize "$bak")"))" \
       "delete once the live DB is confirmed healthy: rm '$bak'"
done
echo

# --- summary ----------------------------------------------------------------
echo "─────────────────────────────────────────"
if [ "$fails" -gt 0 ]; then
  printf "Result: \033[31m%d FAIL\033[0m, %d WARN\n" "$fails" "$warns"
  exit 1
elif [ "$warns" -gt 0 ]; then
  printf "Result: \033[33m%d WARN\033[0m, 0 FAIL\n" "$warns"
  exit 0
else
  printf "Result: \033[32mall healthy\033[0m\n"
  exit 0
fi
