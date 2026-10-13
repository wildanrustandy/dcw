#!/usr/bin/env bash
# uninstall.sh — remove dcw from this machine
#
#   ./uninstall.sh [BIN_DIR]     (default: ~/.local/bin, as install.sh picks)
#
# Removes: the dcw binary, ~/.config/dcw (config + the venv helper file), and the
# three blocks install.sh appends to the rc file(s). Worktrees, cloned DBs and
# ~/.dev-captain/ are left alone.
#
# The rc-file blocks are matched by the markers install.sh writes, and the shape
# that follows each one — never by guessing at line numbers. Every edited rc file
# is backed up next to itself first.
set -euo pipefail
BIN_DIR="${1:-$HOME/.local/bin}"
CONFIG_DIR="$HOME/.config/dcw"

rm -f "$BIN_DIR/dcw" && echo "✓ removed $BIN_DIR/dcw"
rm -rf "$CONFIG_DIR" && echo "✓ removed $CONFIG_DIR (config + shell/venv.sh)"

# ── strip the installer's rc blocks ───────────────────────────────
python3 - "$HOME" <<'PY'
import os, shutil, sys

home = sys.argv[1]
candidates = [
    ".zshenv", ".bashrc", ".bash_profile", ".bash_login", ".profile",
    ".config/fish/config.fish",
]

# (marker, end-of-block predicate). The block runs from the marker line through
# the first line matching `end` (inclusive).
def ends_with_brace(line):      # the dcw() function
    return line.rstrip("\n") == "}"

def is_source_line(line):       # the venv helpers: one guarded source line
    return line.lstrip().startswith('[ -f "$HOME/.config/dcw/shell/venv.sh" ]')

def is_path_line(line):         # the PATH export the installer appends
    return line.lstrip().startswith("export PATH=")

blocks = [
    ("# dcw shell integration (added by installer)", ends_with_brace),
    ("# dcw venv helpers (added by installer)", is_source_line),
    ("# added by dcw installer", is_path_line),
]

for rel in candidates:
    path = os.path.join(home, rel)
    if not os.path.isfile(path):
        continue
    with open(path) as fh:
        lines = fh.readlines()

    out, i, removed = [], 0, 0
    while i < len(lines):
        marker = next((m for m, _ in blocks if lines[i].rstrip("\n") == m), None)
        if marker is None:
            out.append(lines[i]); i += 1
            continue
        end = next(end for m, end in blocks if m == marker)
        j = i + 1
        while j < len(lines) and not end(lines[j]):
            j += 1
        j = min(j + 1, len(lines))      # include the closing line
        removed += 1
        i = j

    if not removed:
        continue
    shutil.copy2(path, path + ".dcw-uninstall.bak")
    # Drop the blank line the installer writes before each block.
    cleaned, k = [], 0
    while k < len(out):
        if out[k].strip() == "" and k + 1 < len(out) and out[k + 1].strip() == "":
            k += 1
            continue
        cleaned.append(out[k]); k += 1
    with open(path, "w") as fh:
        fh.writelines(cleaned)
    print(f"✓ cleaned {removed} dcw block(s) from {path} (backup: {path}.dcw-uninstall.bak)")
PY

echo "  (worktrees in <repo>/.worktrees/, cloned DBs, and ~/.dev-captain/ are left alone)"
echo "  open a new terminal to drop the shell function and the venv helpers"
