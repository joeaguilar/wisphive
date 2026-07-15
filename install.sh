#!/usr/bin/env bash
# install.sh — atomic, preflighted install/upgrade of the wisphive binaries
# (itr#536/#534, ADR-0010, incident 2026-07-15).
#
# Order of operations, and why:
#   1. BUILD     cargo build --release
#   2. STAGE     copy binaries to hidden temp names INSIDE the install dir
#                (same filesystem, so the final step is an atomic rename)
#   3. CODESIGN  ad-hoc sign the STAGED paths (macOS). Signing after copying
#                to the final path leaves a live window where the installed
#                hook is unsigned (Gatekeeper SIGKILL for callers) — signing
#                the staged copy closes that window entirely.
#   4. PREFLIGHT run the NEW wisphive-hook's read-only validators against the
#                operator's REAL state dir (`wisphive-hook --statecheck`).
#                A new binary whose validators reject existing state would
#                deny EVERY hook event once live (fail-closed, ADR-0010) —
#                exactly how incident 2026-07-15 shipped a brick. On failure:
#                print the findings + repair commands and ABORT with the old
#                binaries untouched — unless repair is explicitly chosen
#                (--fix-perms flag, or interactive confirmation), which runs
#                scripts/wisphive-rescue.sh --fix and re-probes. Repair is a
#                DELIBERATE, announced act (itr#534) — never silent.
#   5. SWAP      mv -f staged -> final (atomic rename). Running processes
#                keep their already-open old inode; no half-written or
#                unsigned binary is ever visible at the final path.
#
# Flags:
#   --fix-perms   if the preflight finds repairable legacy perms, apply the
#                 safe owner-only tightenings (via scripts/wisphive-rescue.sh
#                 --fix) and continue. Without it, an interactive terminal is
#                 asked; a non-interactive run aborts.
#   -h | --help   this header.
#
# Env overrides (tests must never touch ~/.cargo/bin or ~/.wisphive):
#   WISPHIVE_INSTALL_DIR   install dir (default: $INSTALL_PREFIX/bin if
#                          INSTALL_PREFIX is set, else ~/.cargo/bin)
#   INSTALL_PREFIX         prefix whose bin/ receives the binaries
#   WISPHIVE_STATE_HOME    HOME whose .wisphive the preflight probes
#                          (default: $HOME)
#   WISPHIVE_SKIP_BUILD=1  reuse existing target/release binaries (tests)
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
INSTALL_DIR="${WISPHIVE_INSTALL_DIR:-${INSTALL_PREFIX:-$HOME/.cargo}/bin}"
STATE_HOME="${WISPHIVE_STATE_HOME:-$HOME}"

FIX_PERMS=0
for arg in "$@"; do
    case "$arg" in
        --fix-perms) FIX_PERMS=1 ;;
        -h|--help)
            sed -n '2,41p' "$0" | sed 's/^# \{0,1\}//'
            exit 0
            ;;
        *)
            echo "error: unknown argument: $arg (see --help)" >&2
            exit 64
            ;;
    esac
done

# ── 1. build ─────────────────────────────────────────────────────────────────
cd "$SCRIPT_DIR"
if [ "${WISPHIVE_SKIP_BUILD:-0}" = "1" ]; then
    echo "Skipping build (WISPHIVE_SKIP_BUILD=1); using existing target/release binaries."
else
    echo "Building wisphive (release)..."
    cargo build --release
fi
for bin in target/release/wisphive target/release/wisphive-hook; do
    [ -x "$bin" ] || { echo "error: missing $bin" >&2; exit 1; }
done

# ── 2. stage inside the install dir (same fs => atomic rename later) ────────
mkdir -p "$INSTALL_DIR"
STAGE_WISP="$INSTALL_DIR/.wisphive.staged.$$"
STAGE_HOOK="$INSTALL_DIR/.wisphive-hook.staged.$$"
cleanup_stage() { rm -f "$STAGE_WISP" "$STAGE_HOOK"; }
trap cleanup_stage EXIT

echo "Staging binaries in $INSTALL_DIR..."
cp target/release/wisphive "$STAGE_WISP"
cp target/release/wisphive-hook "$STAGE_HOOK"
chmod +x "$STAGE_WISP" "$STAGE_HOOK"

# ── 3. codesign the STAGED copies (macOS) ────────────────────────────────────
if command -v codesign &>/dev/null; then
    if ! codesign -s - -f "$STAGE_WISP" 2>/dev/null || \
       ! codesign -s - -f "$STAGE_HOOK" 2>/dev/null; then
        echo "error: codesign failed on the staged binaries; aborting." >&2
        echo "Old binaries in $INSTALL_DIR are untouched." >&2
        exit 1
    fi
    echo "Staged binaries signed (ad-hoc)."
fi

# ── 4. preflight: the NEW hook's validators vs the REAL state dir ───────────
probe() { "$STAGE_HOOK" --statecheck --home "$STATE_HOME"; }

echo ""
echo "Preflight: probing $STATE_HOME/.wisphive with the NEW wisphive-hook (read-only)..."
if ! probe; then
    echo ""
    echo "PREFLIGHT FAILED: installing now would deny every hook event on this"
    echo "machine (fail-closed, ADR-0010) until the state above is repaired."
    echo "Repair is a deliberate act (itr#534); it will NOT happen silently."
    echo ""
    DO_FIX=$FIX_PERMS
    if [ "$DO_FIX" -ne 1 ] && [ -t 0 ]; then
        printf "Apply safe owner-only tightenings now via scripts/wisphive-rescue.sh --fix? [y/N] "
        read -r reply
        case "$reply" in [yY]|[yY][eE][sS]) DO_FIX=1 ;; esac
    fi
    if [ "$DO_FIX" -eq 1 ]; then
        echo "Repairing (deliberate, per --fix-perms/confirmation):"
        sh "$SCRIPT_DIR/scripts/wisphive-rescue.sh" --fix --home "$STATE_HOME"
        echo ""
        echo "Re-probing after repair..."
        if ! probe; then
            echo ""
            echo "ABORT: state is still rejected after repair (likely tamper evidence:" >&2
            echo "symlink / foreign owner — inspect it by hand)." >&2
            echo "Old binaries in $INSTALL_DIR are untouched." >&2
            exit 1
        fi
    else
        echo "ABORT: old binaries in $INSTALL_DIR are untouched." >&2
        echo "Fix options:" >&2
        echo "  ./install.sh --fix-perms          repair during install (announced)" >&2
        echo "  wisphive doctor --fix-perms       repair via the installed CLI" >&2
        echo "  sh scripts/wisphive-rescue.sh     diagnose/repair, no binary needed" >&2
        exit 1
    fi
fi

# ── 5. atomic swap ───────────────────────────────────────────────────────────
echo ""
echo "Installing (atomic rename) to $INSTALL_DIR..."
mv -f "$STAGE_WISP" "$INSTALL_DIR/wisphive"
mv -f "$STAGE_HOOK" "$INSTALL_DIR/wisphive-hook"
trap - EXIT

echo ""
echo "Installed:"
echo "  wisphive      -> $INSTALL_DIR/wisphive"
echo "  wisphive-hook -> $INSTALL_DIR/wisphive-hook"
if ! command -v wisphive &>/dev/null; then
    echo ""
    echo "Note: $INSTALL_DIR is not in PATH. Add this to your shell profile:"
    echo "  export PATH=\"$INSTALL_DIR:\$PATH\""
fi
echo ""
echo "Quick start:"
echo "  wisphive on                        # enable gating with strict perms"
echo "  wisphive daemon start              # in a dedicated terminal"
echo "  wisphive hooks install --project .  # in your project"
echo "  wisphive hooks enable"
echo "  wisphive tui                       # in another terminal"
