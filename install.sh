#!/usr/bin/env bash
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
INSTALL_DIR="${WISPHIVE_INSTALL_DIR:-$HOME/.cargo/bin}"

echo "Building wisphive (release)..."
cd "$SCRIPT_DIR"
cargo build --release

echo "Installing binaries to $INSTALL_DIR..."
cp target/release/wisphive "$INSTALL_DIR/wisphive"
cp target/release/wisphive-hook "$INSTALL_DIR/wisphive-hook"
chmod +x "$INSTALL_DIR/wisphive" "$INSTALL_DIR/wisphive-hook"

# Ad-hoc sign to prevent macOS Gatekeeper from killing unsigned binaries
if command -v codesign &>/dev/null; then
    codesign -s - -f "$INSTALL_DIR/wisphive" 2>/dev/null
    codesign -s - -f "$INSTALL_DIR/wisphive-hook" 2>/dev/null
    echo "Binaries signed (ad-hoc)"
fi

# Create ~/.wisphive if it doesn't exist
mkdir -p "$HOME/.wisphive"

# Verify
if command -v wisphive &>/dev/null; then
    echo ""
    echo "Installed:"
    echo "  wisphive      -> $INSTALL_DIR/wisphive"
    echo "  wisphive-hook -> $INSTALL_DIR/wisphive-hook"
    echo ""
    echo "Quick start:"
    echo "  wisphive daemon start              # in a dedicated terminal"
    echo "  wisphive hooks install --project .  # in your project"
    echo "  wisphive hooks enable"
    echo "  wisphive tui                       # in another terminal"
else
    echo ""
    echo "Binaries installed to $INSTALL_DIR but it's not in PATH."
    echo "Add this to your shell profile:"
    echo "  export PATH=\"$INSTALL_DIR:\$PATH\""
fi
