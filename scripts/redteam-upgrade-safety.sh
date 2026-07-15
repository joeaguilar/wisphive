#!/usr/bin/env bash
#
# redteam-upgrade-safety.sh — epic itr#533/#539 red-team pass for upgrade
# safety. Drives the REAL release binaries and the REAL install.sh against a
# throwaway isolated HOME (never touches ~/.wisphive or ~/.cargo/bin) and
# asserts the PO-ruled invariant chain (ADR-0010: the total stop is DESIRED;
# denial must be deliberate, the message must name the exact fix, repair
# tooling must recover, and install preflight must catch broken state):
#
#   (a) legacy perms (0755 dir / 0644 mode — incident 2026-07-15) => the hook
#       DENIES tool events (exit 2) naming the failing file + fix commands
#       (itr#535).
#   (b) the brick detector fires exactly ONE notification + BRICKED marker
#       for the repeated same cause, no notification storm (itr#538).
#   (c) `wisphive doctor --fix-perms` repairs the state => the hook accepts,
#       and the healthy invocation clears the BRICKED marker (itr#537/#538).
#   (d) install.sh preflight catches the same broken state: aborts with
#       guidance leaving old binaries untouched, or repairs+swaps atomically
#       under --fix-perms (itr#536/#534).
#   (e) UserPromptSubmit remains DENIED while broken — fail-closed held for
#       human-origin events too (ADR-0010).
#
# If someone adds a hook validator without migration coverage, (c)/(d) fail
# loudly: the repaired state (or the preflight) will no longer satisfy the
# new binary.
#
# Usage:  ./scripts/redteam-upgrade-safety.sh
#   WISP / HOOK env vars override the binaries (default: target/release/*).
# The isolated HOME lives under /tmp (short paths; consistent with sibling
# redteam-decision-plane.sh).
set -u

REPO="$(cd "$(dirname "$0")/.." && pwd)"
WISP="${WISP:-$REPO/target/release/wisphive}"
HOOK="${HOOK:-$REPO/target/release/wisphive-hook}"
H="$(mktemp -d /tmp/wh-up.XXXXXX)"
WD="$H/.wisphive"
CAP="$H/notify-capture.txt"   # brick notifications land here, not the OS

cleanup() { rm -rf "$H"; }
trap cleanup EXIT

PASS=0; FAIL=0
ok()  { echo "  PASS: $1"; PASS=$((PASS+1)); }
bad() { echo "  FAIL: $1"; FAIL=$((FAIL+1)); }

for bin in "$WISP" "$HOOK"; do
  [ -x "$bin" ] || { echo "missing binary: $bin (run: cargo build --release)"; exit 1; }
done

# Every hook invocation in this script captures notifications to a file —
# never fire real desktop notifications from a test.
hook_event() { # event_name session -> runs hook, echoes exit code, output in $H/out.txt/$H/err.txt
  printf '{"hook_event_name":"%s","tool_name":"Bash","tool_input":{"command":"echo hi"},"session_id":"%s","cwd":"/tmp/rt-up-proj"}' "$1" "$2" > "$H/ev.json"
  HOME="$H" WISPHIVE_BRICK_NOTIFY_CAPTURE="$CAP" WISPHIVE_AGENT_TYPE=claude_code \
    "$HOOK" < "$H/ev.json" > "$H/out.txt" 2> "$H/err.txt"
  echo $?
}

echo "=== SETUP: legacy-perm state dir (incident 2026-07-15 shape), HOME=$H ==="
mkdir -p "$WD"
printf 'active' > "$WD/mode"
printf '{ "notifications": false, "auto_approve_level": "read" }\n' > "$WD/config.json"
chmod 755 "$WD"        # legacy dir perms (hook requires 0700)
chmod 644 "$WD/mode"   # legacy mode perms (hook requires 0600)
chmod 644 "$WD/config.json"

echo
echo "=== (a) legacy perms => deliberate DENY naming file + fix commands (itr#535) ==="
RC="$(hook_event PreToolUse rt-a)"
[ "$RC" = "2" ] && ok "PreToolUse denied (exit 2) — fail-closed held" || bad "expected exit 2, got $RC"
ERR="$(cat "$H/err.txt")"
echo "$ERR" | grep -qF "$WD" && ok "denial names the failing path ($WD)" || bad "denial does not name the path: $ERR"
echo "$ERR" | grep -q "chmod 700" && echo "$ERR" | grep -q "chmod 600" && ok "denial carries the exact chmod repair" || bad "no chmod repair line"
echo "$ERR" | grep -q "wisphive doctor --fix-perms" && ok "denial names doctor --fix-perms" || bad "no doctor --fix-perms"
echo "$ERR" | grep -q "scripts/wisphive-rescue.sh" && ok "denial names wisphive-rescue.sh" || bad "no rescue script pointer"

echo
echo "=== (e) UserPromptSubmit remains DENIED while broken (ADR-0010) ==="
RC="$(hook_event UserPromptSubmit rt-e)"
[ "$RC" = "2" ] && ok "UserPromptSubmit denied (exit 2) — no human-origin fail-open hole" || bad "UserPromptSubmit rc=$RC (must stay fail-closed)"

echo
echo "=== (b) brick detector: repeated same cause => exactly ONE notification + marker (itr#538) ==="
for i in $(seq 1 10); do hook_event PreToolUse "rt-b-$i" >/dev/null; done
[ -f "$WD/BRICKED" ] && ok "BRICKED marker dropped in state dir" || bad "no BRICKED marker"
grep -q "wisphive doctor --fix-perms" "$WD/BRICKED" 2>/dev/null && ok "marker carries repair guidance" || bad "marker lacks repair guidance"
LINES=$(wc -l < "$CAP" 2>/dev/null | tr -d ' ')
[ "${LINES:-0}" = "1" ] && ok "exactly 1 notification after 12 same-cause denials" || bad "expected 1 notification, got ${LINES:-0}"
for i in $(seq 1 20); do hook_event PreToolUse "rt-b2-$i" >/dev/null; done
LINES=$(wc -l < "$CAP" | tr -d ' ')
[ "$LINES" = "1" ] && ok "still 1 notification after 20 more (no storm, rate-limited per cause)" || bad "notification storm: $LINES lines"

echo
echo "=== (d) install.sh preflight catches the broken state (itr#536/#534) ==="
BIN="$H/bin"; mkdir -p "$BIN"
printf 'OLD-WISPHIVE-SENTINEL' > "$BIN/wisphive"
printf 'OLD-HOOK-SENTINEL' > "$BIN/wisphive-hook"
( cd "$REPO" && WISPHIVE_SKIP_BUILD=1 WISPHIVE_INSTALL_DIR="$BIN" WISPHIVE_STATE_HOME="$H" \
    ./install.sh </dev/null > "$H/install-abort.txt" 2>&1 )
RC=$?
[ "$RC" != "0" ] && ok "install.sh ABORTED on broken state (rc=$RC)" || bad "install.sh proceeded over a brick-to-be"
grep -q "PREFLIGHT FAILED" "$H/install-abort.txt" && ok "abort explains the preflight failure" || bad "no preflight explanation"
grep -q "fix-perms" "$H/install-abort.txt" && ok "abort names the repair options" || bad "abort lacks repair options"
[ "$(cat "$BIN/wisphive")" = "OLD-WISPHIVE-SENTINEL" ] && [ "$(cat "$BIN/wisphive-hook")" = "OLD-HOOK-SENTINEL" ] \
  && ok "old binaries untouched after abort" || bad "old binaries were touched on abort"
STAGED_LEFT=$(find "$BIN" -name '.wisphive*staged*' | wc -l | tr -d ' ')
[ "$STAGED_LEFT" = "0" ] && ok "no staged leftovers after abort" || bad "$STAGED_LEFT staged files leaked"

( cd "$REPO" && WISPHIVE_SKIP_BUILD=1 WISPHIVE_INSTALL_DIR="$BIN" WISPHIVE_STATE_HOME="$H" \
    ./install.sh --fix-perms </dev/null > "$H/install-fix.txt" 2>&1 )
RC=$?
[ "$RC" = "0" ] && ok "install.sh --fix-perms repaired and installed (rc=0)" || { bad "install.sh --fix-perms failed rc=$RC"; sed -n '1,40p' "$H/install-fix.txt"; }
grep -q "Repairing (deliberate" "$H/install-fix.txt" && ok "repair was announced, not silent (itr#534)" || bad "repair not announced"
DPERM=$(stat -f %Lp "$WD" 2>/dev/null || stat -c %a "$WD")
MPERM=$(stat -f %Lp "$WD/mode" 2>/dev/null || stat -c %a "$WD/mode")
[ "$DPERM" = "700" ] && [ "$MPERM" = "600" ] && ok "state repaired to 0700/0600" || bad "state not repaired: dir=$DPERM mode=$MPERM"
[ "$(cat "$BIN/wisphive-hook" 2>/dev/null)" != "OLD-HOOK-SENTINEL" ] && ok "new hook binary swapped into place" || bad "hook not swapped"
"$BIN/wisphive-hook" --statecheck --home "$H" > "$H/postcheck.txt" 2>&1
[ $? = 0 ] && ok "installed hook's own statecheck passes post-swap (working binary, healthy state)" || bad "post-swap statecheck failed"

echo
echo "=== (c) repaired state => hook accepts; healthy invocation clears marker (itr#537/#538) ==="
# Re-break, then repair via the CLI path the denial message advertises.
chmod 755 "$WD"; chmod 644 "$WD/mode"
RC="$(hook_event PreToolUse rt-c-broken)"
[ "$RC" = "2" ] && ok "re-broken state denies again" || bad "re-broken state rc=$RC"
HOME="$H" "$WISP" doctor --fix-perms > "$H/doctor.txt" 2>&1 || true
DPERM=$(stat -f %Lp "$WD" 2>/dev/null || stat -c %a "$WD")
MPERM=$(stat -f %Lp "$WD/mode" 2>/dev/null || stat -c %a "$WD/mode")
[ "$DPERM" = "700" ] && [ "$MPERM" = "600" ] && ok "doctor --fix-perms tightened to 0700/0600" || bad "doctor did not repair: dir=$DPERM mode=$MPERM"
RC="$(hook_event PreToolUse rt-c-healthy)"
[ "$RC" = "0" ] && ok "hook accepts after repair (exit 0)" || { bad "hook still failing rc=$RC"; cat "$H/err.txt"; }
[ ! -f "$WD/BRICKED" ] && ok "healthy invocation cleared the BRICKED marker" || bad "BRICKED marker not cleared"

echo
echo "=== RESULT: $PASS passed, $FAIL failed ==="
exit "$FAIL"
