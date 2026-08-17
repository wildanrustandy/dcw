#!/usr/bin/env bash
# uninstall.sh — remove dcw from this machine
set -euo pipefail
BIN_DIR="${1:-$HOME/bin}"
rm -f "$BIN_DIR/dcw" && echo "✓ removed $BIN_DIR/dcw"
CONFIG="$HOME/.config/dcw/config"
rm -f "$CONFIG" && echo "✓ removed $CONFIG"
echo "  (worktrees in <repo>/.worktrees/, cloned DBs, and ~/.dev-captain/ are left alone)"
