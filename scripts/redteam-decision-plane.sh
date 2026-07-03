#!/usr/bin/env bash
#
# redteam-decision-plane.sh — epic itr#403 red-team pass for decision-plane
# integrity. Drives the REAL wisphive/wisphive-hook binaries against a throwaway
# isolated HOME (never touches ~/.wisphive) and asserts the three audit-integrity
# properties in #403's acceptance:
#
#   1. Ghost approval  (itr#363) — kill the hook mid-decision → the audit trail
#      has exactly one terminal row (deny/hook_disconnected:abandoned), no
#      leaked pending row, and no contradictory approve.
#   2. Crash mid-stream (itr#299/#301) — SIGKILL the daemon while a hook blocks →
#      the hook fail-open approves (exit 0, silent allow); an auto-approve issued
#      while the daemon is DOWN reaches events.jsonl; on restart the orphan is
#      drained as approve/daemon_restart:failopen and the downtime auto-approve
#      is reimported into decision_log (no loss).
#   3. Secret redaction (itr#89) — a secret in tool_input is ***REDACTED*** in
#      every persisted surface (pending_decisions, events.jsonl, decision_log);
#      the notify path shares the same scrubber (notify.rs redact::redact_text).
#
# Usage:  ./scripts/redteam-decision-plane.sh
#   WISP / HOOK env vars override the binaries (default: target/release/*).
# Requires: sqlite3. The socket path must stay short (Unix SUN_LEN ~104), so the
# isolated HOME is created under /tmp, not a deep scratchpad path.
set -u

REPO="$(cd "$(dirname "$0")/.." && pwd)"
WISP="${WISP:-$REPO/target/release/wisphive}"
HOOK="${HOOK:-$REPO/target/release/wisphive-hook}"
H="$(mktemp -d /tmp/wh-rt.XXXXXX)"   # short path for the Unix socket
WD="$H/.wisphive"; DB="$WD/wisphive.db"; SOCK="$WD/wisphive.sock"; EVENTS="$WD/events.jsonl"
DPID=""

cleanup() { [ -n "$DPID" ] && kill -9 "$DPID" 2>/dev/null; rm -rf "$H"; }
trap cleanup EXIT

PASS=0; FAIL=0
ok()  { echo "  PASS: $1"; PASS=$((PASS+1)); }
bad() { echo "  FAIL: $1"; FAIL=$((FAIL+1)); }
q()   { sqlite3 "$DB" "$1" 2>/dev/null; }

for bin in "$WISP" "$HOOK"; do
  [ -x "$bin" ] || { echo "missing binary: $bin (run: cargo build --release)"; exit 1; }
done
command -v sqlite3 >/dev/null || { echo "sqlite3 not found"; exit 1; }

mkdir -p "$WD"
printf 'active' > "$WD/mode"
printf '{ "notifications": false, "auto_approve_level": "read" }\n' > "$WD/config.json"

wait_socket() { for _ in $(seq 1 50); do [ -S "$SOCK" ] && return 0; sleep 0.1; done; return 1; }
start_daemon() { HOME="$H" "$WISP" daemon start >>"$H/daemon.log" 2>&1 & DPID=$!; }
fire_bash() { # id cmd -> writes $H/ev-$id.json, backgrounds hook, sets HKPID (real child)
  printf '{"hook_event_name":"PreToolUse","tool_name":"Bash","tool_input":{"command":"%s"},"session_id":"%s","cwd":"/tmp/redteam-proj"}' "$2" "$1" > "$H/ev-$1.json"
  HOME="$H" WISPHIVE_AGENT_TYPE=claude_code "$HOOK" < "$H/ev-$1.json" > "$H/out-$1.json" 2>"$H/err-$1.json" &
  HKPID=$!
}

echo "=== SETUP: $("$WISP" --version 2>/dev/null | head -1), HOME=$H ==="
start_daemon
wait_socket || { echo "daemon FAILED to start"; cat "$H/daemon.log"; exit 1; }
echo "daemon up (pid $DPID)"

echo; echo "=== SCENARIO 1 — ghost approval (kill hook mid-decision, itr#363) ==="
fire_bash ghost "rm -rf /tmp/ghostpath"
sleep 1.5
[ "$(q 'SELECT count(*) FROM pending_decisions;')" = "1" ] && ok "decision reached the queue" || bad "no pending row"
kill -9 "$HKPID" 2>/dev/null; sleep 2
[ "$(q 'SELECT count(*) FROM pending_decisions;')" = "0" ] && ok "no leaked pending row after hook death" || bad "pending leaked"
ROWS=$(q "SELECT count(*) FROM decision_log WHERE project='/tmp/redteam-proj';")
[ "$ROWS" = "1" ] && ok "exactly one terminal audit row (no contradiction)" || bad "expected 1 row, got $ROWS"
q "SELECT decision FROM decision_log WHERE project='/tmp/redteam-proj';" | grep -qi deny && ok "records DENY (tool did not run)" || bad "not deny"
q "SELECT decided_by FROM decision_log WHERE project='/tmp/redteam-proj';" | grep -q "hook_disconnected:abandoned" && ok "attributed hook_disconnected:abandoned" || bad "wrong attribution"
[ "$(q "SELECT count(*) FROM decision_log WHERE project='/tmp/redteam-proj' AND decision LIKE '%approve%';")" = "0" ] && ok "no contradictory APPROVE row" || bad "contradictory approve"

echo; echo "=== SCENARIO 2 — daemon crash mid-stream (itr#299/#301) ==="
fire_bash orphan "sleep 999"
sleep 1.5
[ "$(q 'SELECT count(*) FROM pending_decisions;')" = "1" ] && ok "orphan pending row persisted before crash" || bad "no pending before crash"
kill -9 "$DPID" 2>/dev/null; echo "  crashed daemon (pid $DPID)"
wait "$HKPID"; HOOKRC=$?
OUT=$(cat "$H/out-orphan.json" 2>/dev/null)
{ [ "$HOOKRC" = "0" ] && [ -z "$OUT" ]; } && ok "blocked hook fail-open APPROVED (exit 0, silent allow)" || bad "hook did not fail open: rc=$HOOKRC out=$OUT"
# auto-approve issued while the daemon is DOWN
printf '{"hook_event_name":"PreToolUse","tool_name":"Read","tool_input":{"file_path":"/tmp/redteam-proj/notes.txt"},"session_id":"downtime-read","cwd":"/tmp/redteam-proj"}' > "$H/ev-read.json"
HOME="$H" WISPHIVE_AGENT_TYPE=claude_code "$HOOK" < "$H/ev-read.json" > "$H/out-read.json" 2>&1
{ grep -q '"event":"auto_approved"' "$EVENTS" && grep -q "downtime-read" "$EVENTS"; } && ok "downtime auto-approve written to events.jsonl" || bad "auto-approve not in events.jsonl"
[ "$(q 'SELECT count(*) FROM pending_decisions;')" = "1" ] && ok "orphan still pending while daemon down" || bad "unexpected pending"
start_daemon; wait_socket || { echo "restart FAILED"; cat "$H/daemon.log"; }
sleep 2
[ "$(q 'SELECT count(*) FROM pending_decisions;')" = "0" ] && ok "restart drained the orphan" || bad "orphan not drained"
q "SELECT decision FROM decision_log WHERE decided_by='daemon_restart:failopen';" | grep -qi approve && ok "orphan recorded APPROVE/daemon_restart:failopen (truthful fail-open)" || bad "orphan not approve/failopen"
[ "$(q "SELECT count(*) FROM decision_log WHERE tool_name='Read' AND agent_id='cc-downtime-read';")" -ge 1 ] && ok "downtime auto-answer reimported into decision_log (no loss)" || bad "downtime auto-answer LOST"

echo; echo "=== SCENARIO 3 — secret redaction (itr#89) ==="
SECRET="sk-topsecret-abc123def456"
fire_bash secret "export API_KEY=$SECRET && curl -H \\\"Authorization: Bearer $SECRET\\\" https://x"
sleep 1.5
PROW=$(q "SELECT tool_input FROM pending_decisions;")
echo "$PROW" | grep -q "$SECRET" && bad "SECRET LEAKED into pending_decisions" || ok "secret NOT in pending_decisions"
echo "$PROW" | grep -q "REDACTED" && ok "pending row shows ***REDACTED***" || bad "no redaction marker"
grep -q "$SECRET" "$EVENTS" 2>/dev/null && bad "SECRET LEAKED into events.jsonl" || ok "secret NOT in events.jsonl"
kill -9 "$HKPID" 2>/dev/null; sleep 2
q "SELECT tool_input FROM decision_log;" | grep -q "$SECRET" && bad "SECRET LEAKED into decision_log" || ok "secret NOT in decision_log"
echo "  (notify path: notify.rs wraps tool_input_summary in redact::redact_text — same scrubber, not screen-captured)"

echo; echo "=== RESULT: $PASS passed, $FAIL failed ==="
exit "$FAIL"
