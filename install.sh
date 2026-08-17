#!/usr/bin/env bash
# install.sh — install dcw (Dev-Captain Worktree) for the current user
# Usage: ./install.sh [--bin DIR]
set -euo pipefail

BIN_DIR="$HOME/bin"
if [ "${1:-}" = "--bin" ]; then
  BIN_DIR="${2:?usage: ./install.sh --bin <dir>}"
fi

SRC="$(cd "$(dirname "$0")" && pwd)/dcw"
[ -f "$SRC" ] || { echo "error: dcw script not found next to install.sh" >&2; exit 1; }

echo "== dcw installer =="

# 1. Install the script
mkdir -p "$BIN_DIR"
cp "$SRC" "$BIN_DIR/dcw"
chmod +x "$BIN_DIR/dcw"
echo "✓ installed: $BIN_DIR/dcw"

# 2. Make sure the bin dir is on PATH
case ":$PATH:" in
  *":$BIN_DIR:"*) echo "✓ PATH: $BIN_DIR already present" ;;
  *)
    SHELL_RC="$HOME/.zshrc"
    [ -n "${BASH_VERSION:-}" ] && SHELL_RC="$HOME/.bashrc"
    if [ -f "$HOME/.zshrc" ]; then SHELL_RC="$HOME/.zshrc"; fi
    printf '\n# added by dcw installer\nexport PATH="%s:$PATH"\n' "$BIN_DIR" >> "$SHELL_RC"
    echo "✓ PATH: added $BIN_DIR to $SHELL_RC"
    echo "  → run: source $SHELL_RC   (or open a new terminal)"
    ;;
esac

# 3. Dependency check (warn only)
echo "== dependency check =="
MISSING=0
for dep in git python3 lsof; do
  if command -v "$dep" >/dev/null 2>&1; then
    echo "✓ $dep"
  else
    echo "✗ $dep NOT FOUND — dcw needs it"; MISSING=1
  fi
done
# psql/createdb needed only for per-worktree DB cloning
for dep in psql createdb dropdb pg_dump; do
  if command -v "$dep" >/dev/null 2>&1; then
    echo "✓ $dep"
  else
    echo "⚠ $dep not found — per-worktree DB clone will be skipped (brew install postgresql@16 or libpq)"
  fi
done

echo ""
echo "Done. Usage:"
echo "  dcw start <branch>    # worktree + clone DB + private files + server"
echo "  dcw status | list | logs <branch> | stop <branch> | abort <branch>"
echo ""
echo "Defaults (override via env):"
echo "  repo: \$HOME/dev/hmx-002    venv: \$HOME/dev/.venv    ports: 5201-5220"
[ "$MISSING" = 1 ] && { echo "warning: some required dependencies are missing" >&2; exit 1; }
exit 0
