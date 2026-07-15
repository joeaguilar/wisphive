#!/bin/sh
# wisphive-rescue.sh — diagnose and repair the strict ~/.wisphive state that
# the wisphive-hook validators enforce (incident 2026-07-15, itr#533/#541).
#
# Pure POSIX sh with ZERO dependency on any wisphive binary: this script is
# the way out when the hook binary is broken, mis-signed, or mid-upgrade and
# every hook event is being denied.
#
# It mirrors the hook's validators exactly (keep in sync — see
# crates/wisphive_hook/src/main.rs read_mode_file and
# crates/wisphive_protocol/src/fs_trust.rs read_trusted):
#   HARD (any failure = every hook event denied):
#     - state dir exists, is a real directory, not a symlink,
#       owned by the current user, permissions exactly 0700
#     - mode file exists, is a regular file, not a symlink,
#       owned by the current user, permissions exactly 0600
#     - mode file is <= 64 bytes and its trimmed content is "active" or "off"
#   SOFT (failure = policy file silently ignored, safe defaults used):
#     - config.json / auto-approve.json (if present): regular file, not a
#       symlink, owned by the current user, not group- or world-writable
#     - config.json.lock (if present): regular file, owned, 0600
#
# Usage:
#   wisphive-rescue.sh              diagnose: PASS/FAIL per check + exact fix
#   wisphive-rescue.sh --fix        apply safe owner-only tightenings (never
#                                   loosens perms, never chowns, refuses on
#                                   symlinks/foreign owners: tamper evidence)
#   wisphive-rescue.sh --off        emergency exit: set gating mode to "off"
#                                   (also tightens dir/mode perms so the hook
#                                   actually reads it and passes through)
#   wisphive-rescue.sh --home DIR   target DIR instead of $HOME (for tests)

set -u

ACTION=diagnose
TARGET_HOME="${HOME:-}"

usage() {
    sed -n '2,32p' "$0" | sed 's/^# \{0,1\}//'
}

while [ $# -gt 0 ]; do
    case "$1" in
        --fix) ACTION=fix ;;
        --off) ACTION=off ;;
        --home)
            shift
            if [ $# -eq 0 ]; then
                echo "error: --home requires a directory argument" >&2
                exit 64
            fi
            TARGET_HOME="$1"
            ;;
        -h|--help)
            usage
            exit 0
            ;;
        *)
            echo "error: unknown argument: $1" >&2
            usage >&2
            exit 64
            ;;
    esac
    shift
done

if [ -z "$TARGET_HOME" ]; then
    echo "error: HOME is not set; pass --home <dir>" >&2
    exit 64
fi

WH_DIR="$TARGET_HOME/.wisphive"
MODE_FILE="$WH_DIR/mode"
SELF="$0"
MY_UID=$(id -u)

# ── stat portability (BSD/macOS vs GNU/Linux) ────────────────────────────────
if stat -f %u / >/dev/null 2>&1; then
    STAT_STYLE=bsd
else
    STAT_STYLE=gnu
fi

# uid of the entry itself (never follows symlinks)
stat_uid() {
    if [ "$STAT_STYLE" = bsd ]; then
        stat -f %u "$1"
    else
        stat -c %u "$1"
    fi
}

# 4-digit octal permissions of the entry itself
stat_perms() {
    if [ "$STAT_STYLE" = bsd ]; then
        sp_raw=$(stat -f %p "$1") || return 1
        # %p prints file type + permissions in octal (e.g. 40755, 100644);
        # keep the last four digits.
        printf '%s\n' "${sp_raw#"${sp_raw%????}"}"
    else
        sp_raw=$(stat -c %a "$1") || return 1
        while [ "${#sp_raw}" -lt 4 ]; do sp_raw="0$sp_raw"; done
        printf '%s\n' "$sp_raw"
    fi
}

FAILS=0
FIXED=0
REFUSED=0

pass() {
    printf 'PASS  %s\n' "$1"
}

# fail <name> <detail> <fix-command>
fail() {
    printf 'FAIL  %s\n' "$1"
    printf '      %s\n' "$2"
    printf '      fix: %s\n' "$3"
    FAILS=$((FAILS + 1))
}

info() {
    printf 'INFO  %s\n' "$1"
}

# refuse <name> <why> — tamper-evidence class; --fix will not touch it
refuse() {
    printf 'FAIL  %s\n' "$1"
    printf '      %s\n' "$2"
    printf '      REFUSING to auto-fix: this is tamper evidence. Inspect it,\n'
    printf '      then remove/replace it yourself if you trust the explanation.\n'
    FAILS=$((FAILS + 1))
    REFUSED=$((REFUSED + 1))
}

# mutate <cmd...> — echo every mutation, then run it
mutate() {
    printf 'FIXED %s\n' "$*"
    "$@"
    FIXED=$((FIXED + 1))
}

trimmed_contents() {
    # Approximates Rust's str::trim() for the sane single-line case; a value
    # with interior garbage still (correctly) fails the active/off match.
    sed -e 's/^[[:space:]]*//' -e 's/[[:space:]]*$//' "$1" 2>/dev/null | head -c 128
}

# ── HARD checks: state directory ─────────────────────────────────────────────
# Returns non-zero if the directory is unusable (skip file checks then).
check_dir() {
    if [ ! -e "$WH_DIR" ] && [ ! -h "$WH_DIR" ]; then
        fail "state dir exists ($WH_DIR)" \
            "missing: the hook denies every event until it exists with mode 0700" \
            "mkdir -p '$WH_DIR' && chmod 0700 '$WH_DIR'  (or: wisphive on)"
        if [ "$ACTION" = fix ]; then
            mutate mkdir -p "$WH_DIR"
            mutate chmod 0700 "$WH_DIR"
            # fall through: the freshly created dir passes the checks below
        else
            return 1
        fi
    else
        pass "state dir exists ($WH_DIR)"
    fi

    if [ -h "$WH_DIR" ]; then
        refuse "state dir is not a symlink" \
            "$WH_DIR is a symlink; the hook refuses to follow it (O_NOFOLLOW)"
        return 1
    fi
    pass "state dir is not a symlink"

    if [ ! -d "$WH_DIR" ]; then
        refuse "state dir is a directory" \
            "$WH_DIR exists but is not a directory"
        return 1
    fi
    pass "state dir is a directory"

    dir_uid=$(stat_uid "$WH_DIR")
    if [ "$dir_uid" != "$MY_UID" ]; then
        refuse "state dir owned by you" \
            "$WH_DIR is owned by uid $dir_uid, expected your uid $MY_UID (never chowned automatically)"
        return 1
    fi
    pass "state dir owned by you (uid $MY_UID)"

    dir_perms=$(stat_perms "$WH_DIR")
    if [ "$dir_perms" != "0700" ]; then
        fail "state dir permissions are exactly 0700" \
            "$WH_DIR has $dir_perms; the hook requires exactly 0700" \
            "chmod 0700 '$WH_DIR'"
        if [ "$ACTION" = fix ]; then
            mutate chmod 0700 "$WH_DIR"
        fi
    else
        pass "state dir permissions are exactly 0700"
    fi
    return 0
}

# ── HARD checks: mode file ───────────────────────────────────────────────────
check_mode_file() {
    if [ ! -e "$MODE_FILE" ] && [ ! -h "$MODE_FILE" ]; then
        fail "mode file exists ($MODE_FILE)" \
            "missing: the hook denies every event until it exists ('active' or 'off')" \
            "wisphive on  (enable gating)  or:  $SELF --off  (disable gating)"
        info "--fix will NOT create a missing mode file: choosing 'active' vs 'off' is a gating posture decision. Use one of the two commands above."
        return 1
    fi
    pass "mode file exists ($MODE_FILE)"

    if [ -h "$MODE_FILE" ]; then
        refuse "mode file is not a symlink" \
            "$MODE_FILE is a symlink; the hook refuses to follow it (O_NOFOLLOW)"
        return 1
    fi
    pass "mode file is not a symlink"

    if [ ! -f "$MODE_FILE" ]; then
        refuse "mode file is a regular file" \
            "$MODE_FILE exists but is not a regular file"
        return 1
    fi
    pass "mode file is a regular file"

    mode_uid=$(stat_uid "$MODE_FILE")
    if [ "$mode_uid" != "$MY_UID" ]; then
        refuse "mode file owned by you" \
            "$MODE_FILE is owned by uid $mode_uid, expected your uid $MY_UID (never chowned automatically)"
        return 1
    fi
    pass "mode file owned by you (uid $MY_UID)"

    mode_perms=$(stat_perms "$MODE_FILE")
    if [ "$mode_perms" != "0600" ]; then
        fail "mode file permissions are exactly 0600" \
            "$MODE_FILE has $mode_perms; the hook requires exactly 0600" \
            "chmod 0600 '$MODE_FILE'"
        if [ "$ACTION" = fix ]; then
            mutate chmod 0600 "$MODE_FILE"
        fi
    else
        pass "mode file permissions are exactly 0600"
    fi

    mode_size=$(wc -c < "$MODE_FILE")
    mode_size=$((mode_size))
    if [ "$mode_size" -gt 64 ]; then
        fail "mode file is <= 64 bytes" \
            "$MODE_FILE is $mode_size bytes; the hook rejects anything larger" \
            "wisphive on  or:  $SELF --off"
        return 0
    fi
    pass "mode file is <= 64 bytes ($mode_size)"

    mode_value=$(trimmed_contents "$MODE_FILE")
    case "$mode_value" in
        active)
            pass "mode file content is valid (\"active\": gating enabled)"
            ;;
        off)
            pass "mode file content is valid (\"off\": gating disabled, hooks pass through)"
            ;;
        *)
            fail "mode file content is \"active\" or \"off\"" \
                "content is \"$mode_value\"; the hook denies every event on any other value" \
                "wisphive on  (enable)  or:  $SELF --off  (disable). --fix never rewrites contents."
            ;;
    esac
    return 0
}

# ── SOFT checks: policy files (untrusted => silently ignored, not denied) ────
# check_policy_file <path> <label> <required-perms|go-w>
check_policy_file() {
    pf_path="$1"
    pf_label="$2"
    pf_rule="$3"

    if [ ! -e "$pf_path" ] && [ ! -h "$pf_path" ]; then
        info "$pf_label absent (fine: safe defaults apply)"
        return 0
    fi

    if [ -h "$pf_path" ]; then
        refuse "$pf_label is not a symlink" \
            "$pf_path is a symlink; the hook ignores it and falls back to safe defaults"
        return 0
    fi

    if [ ! -f "$pf_path" ]; then
        refuse "$pf_label is a regular file" \
            "$pf_path exists but is not a regular file; the hook ignores it"
        return 0
    fi

    pf_uid=$(stat_uid "$pf_path")
    if [ "$pf_uid" != "$MY_UID" ]; then
        refuse "$pf_label owned by you" \
            "$pf_path is owned by uid $pf_uid, expected your uid $MY_UID; the hook ignores it (never chowned automatically)"
        return 0
    fi

    pf_perms=$(stat_perms "$pf_path")
    if [ "$pf_rule" = "0600" ]; then
        if [ "$pf_perms" != "0600" ]; then
            fail "$pf_label permissions are 0600" \
                "$pf_path has $pf_perms" \
                "chmod 0600 '$pf_path'"
            if [ "$ACTION" = fix ]; then
                mutate chmod 0600 "$pf_path"
            fi
        else
            pass "$pf_label permissions are 0600"
        fi
    else
        # rule: not group- or world-writable (mode & 0022 == 0)
        pf_group=$(printf '%s' "$pf_perms" | cut -c3)
        pf_other=$(printf '%s' "$pf_perms" | cut -c4)
        if [ $((pf_group & 2)) -ne 0 ] || [ $((pf_other & 2)) -ne 0 ]; then
            fail "$pf_label not group/world-writable" \
                "$pf_path has $pf_perms; the hook ignores it and falls back to safe defaults" \
                "chmod go-w '$pf_path'"
            if [ "$ACTION" = fix ]; then
                mutate chmod go-w "$pf_path"
            fi
        else
            pass "$pf_label not group/world-writable ($pf_perms)"
        fi
    fi
    return 0
}

check_fail_mode() {
    fm_path="$WH_DIR/fail-mode"
    if [ ! -e "$fm_path" ]; then
        info "fail-mode absent (defaults to closed: runtime hook errors deny)"
        return 0
    fi
    fm_value=$(trimmed_contents "$fm_path")
    case "$fm_value" in
        open|closed)
            info "fail-mode is \"$fm_value\""
            ;;
        *)
            info "fail-mode content \"$fm_value\" is not open/closed; treated as closed"
            ;;
    esac
}

# ── --off: emergency exit ────────────────────────────────────────────────────
do_off() {
    # Refuse on tamper-evidence entries; everything else is repaired so the
    # hook can actually read "off" and pass through.
    if [ -h "$WH_DIR" ]; then
        echo "REFUSED: $WH_DIR is a symlink (tamper evidence)." >&2
        echo "Inspect it, then: rm '$WH_DIR' and re-run: $SELF --off" >&2
        return 1
    fi
    if [ -e "$WH_DIR" ]; then
        if [ ! -d "$WH_DIR" ]; then
            echo "REFUSED: $WH_DIR exists but is not a directory (tamper evidence)." >&2
            return 1
        fi
        off_dir_uid=$(stat_uid "$WH_DIR")
        if [ "$off_dir_uid" != "$MY_UID" ]; then
            echo "REFUSED: $WH_DIR is owned by uid $off_dir_uid, not you (uid $MY_UID) (tamper evidence)." >&2
            return 1
        fi
    else
        mutate mkdir -p "$WH_DIR"
    fi
    off_dir_perms=$(stat_perms "$WH_DIR")
    if [ "$off_dir_perms" != "0700" ]; then
        mutate chmod 0700 "$WH_DIR"
    fi

    if [ -h "$MODE_FILE" ]; then
        echo "REFUSED: $MODE_FILE is a symlink (tamper evidence)." >&2
        echo "Inspect it, then: rm '$MODE_FILE' and re-run: $SELF --off" >&2
        return 1
    fi
    if [ -e "$MODE_FILE" ]; then
        if [ ! -f "$MODE_FILE" ]; then
            echo "REFUSED: $MODE_FILE exists but is not a regular file (tamper evidence)." >&2
            return 1
        fi
        off_mode_uid=$(stat_uid "$MODE_FILE")
        if [ "$off_mode_uid" != "$MY_UID" ]; then
            echo "REFUSED: $MODE_FILE is owned by uid $off_mode_uid, not you (uid $MY_UID) (tamper evidence)." >&2
            return 1
        fi
    fi

    off_tmp="$WH_DIR/.mode.rescue.$$"
    printf 'FIXED write "off" -> %s (atomic via %s)\n' "$MODE_FILE" "$off_tmp"
    (
        umask 077
        printf 'off' > "$off_tmp"
    ) || return 1
    chmod 0600 "$off_tmp" || { rm -f "$off_tmp"; return 1; }
    mv -f "$off_tmp" "$MODE_FILE" || { rm -f "$off_tmp"; return 1; }

    echo ""
    echo "=============================================================="
    echo "  WISPHIVE GATING IS NOW OFF."
    echo "  Every hook now passes tool calls through WITHOUT review."
    echo "  Re-enable with:  wisphive on   (or: wisphive hooks enable)"
    echo "=============================================================="
    return 0
}

# ── main ─────────────────────────────────────────────────────────────────────
echo "wisphive-rescue: strict-state ${ACTION} for $WH_DIR"
echo ""

if [ "$ACTION" = off ]; then
    do_off
    exit $?
fi

if check_dir; then
    check_mode_file || true
    check_policy_file "$WH_DIR/config.json" "config.json" "go-w"
    check_policy_file "$WH_DIR/auto-approve.json" "auto-approve.json (legacy)" "go-w"
    check_policy_file "$WH_DIR/config.json.lock" "config.json.lock" "0600"
    check_fail_mode
fi

echo ""
if [ "$ACTION" = fix ]; then
    if [ "$FIXED" -gt 0 ]; then
        echo "$FIXED repair(s) applied. Re-run '$SELF${TARGET_HOME:+ --home "$TARGET_HOME"}' to verify."
    else
        echo "No repairs applied."
    fi
    if [ "$REFUSED" -gt 0 ]; then
        echo "$REFUSED check(s) REFUSED (tamper evidence: symlink / foreign owner / wrong type). See above."
        exit 1
    fi
    exit 0
fi

if [ "$FAILS" -gt 0 ]; then
    echo "$FAILS check(s) failing. Run '$SELF --fix${TARGET_HOME:+ --home "$TARGET_HOME"}' to apply safe owner-only repairs,"
    echo "or '$SELF --off${TARGET_HOME:+ --home "$TARGET_HOME"}' to disable gating entirely (emergency exit)."
    exit 1
fi
echo "All checks passing. The strict hook validators will accept this state."
exit 0
