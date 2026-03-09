#!/usr/bin/env bash
set -euo pipefail

# Build binaries, install to ~/bin.
# Set UR_BUILD_PROFILE=debug for debug builds (default: release).

PROFILE="${UR_BUILD_PROFILE:-release}"

if [ "$PROFILE" = "debug" ]; then
    cargo build -p ur -p urd
    TARGET_DIR="target/debug"
else
    cargo build --release -p ur -p urd
    TARGET_DIR="target/release"
fi

# Kill existing daemon before replacing binary
if [ -x "$HOME/bin/ur" ]; then
    "$HOME/bin/ur" kill server 2>/dev/null || true
fi

mkdir -p "$HOME/bin" "$HOME/.ur/logs"
cp "$TARGET_DIR/ur" "$HOME/bin/ur"
cp "$TARGET_DIR/urd" "$HOME/bin/urd"
echo "Installed ur and urd to $HOME/bin/"

# --- pf anchor setup (macOS only, one-time) ---
# Ensures the com.ur.proxy anchor exists in /etc/pf.conf so urd can load
# per-session firewall rules at runtime via pfctl.
PF_ANCHOR='anchor "com.ur.proxy"'
PF_CONF="/etc/pf.conf"

if [ "$(uname)" = "Darwin" ]; then
    if ! grep -qF "$PF_ANCHOR" "$PF_CONF" 2>/dev/null; then
        echo ""
        echo "Setting up pf anchor for container network restriction."
        echo "This requires sudo to modify $PF_CONF and enable pf."
        echo ""
        sudo sh -c "echo '$PF_ANCHOR' >> $PF_CONF"
        sudo pfctl -f "$PF_CONF" 2>/dev/null || true
        sudo pfctl -e 2>/dev/null || true
        echo "pf anchor 'com.ur.proxy' added to $PF_CONF"
    else
        echo "pf anchor 'com.ur.proxy' already configured"
    fi
fi
