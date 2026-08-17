#!/usr/bin/env bash
# uninstall.sh — remove dcw from this machine
set -euo pipefail
BIN_DIR="${1:-$HOME/bin}"
rm -f "$BIN_DIR/dcw" && echo "✓ removed $BIN_DIR/dcw"
echo "  (worktrees in <repo>/.worktrees/, DB clones, and ~/.dev-captain/ are left alone)"
