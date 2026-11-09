#!/usr/bin/env bash
# install.sh — one-shot installer for dcw (the Rust rewrite).
#
#   curl -fsSL https://raw.githubusercontent.com/wildanrustandy/dcw/main/install.sh | bash
#
# Downloads the prebuilt binary for this OS/arch and runs `dcw init`. No git clone
# and no Rust toolchain are required on the target machine.
#
# Overrides:
#   DCW_UPDATE_URL   full URL of the binary (skips OS/arch detection)
#   DCW_UPDATE_BASE  base URL the asset name is appended to (default: GitHub latest release)
#   DCW_BIN_DIR      install directory (default: ~/.local/bin)
set -euo pipefail

# --- asset name (must match Rust's std::env::consts OS/ARCH in src/main.rs) ---
os=$(uname -s)
arch=$(uname -m)
case "$os" in
  Linux)  os=linux ;;
  Darwin) os=macos ;;
  *)      os=$(printf '%s' "$os" | tr '[:upper:]' '[:lower:]') ;;
esac
case "$arch" in
  x86_64|amd64)    arch=x86_64 ;;
  aarch64|arm64)   arch=aarch64 ;;
esac
asset="dcw-${os}-${arch}"

# --- resolve download URL ---
default_base="https://github.com/wildanrustandy/dcw/releases/latest/download"
if [ -n "${DCW_UPDATE_URL:-}" ]; then
  url="$DCW_UPDATE_URL"
else
  base="${DCW_UPDATE_BASE:-$default_base}"
  base="${base%/}"
  url="$base/$asset"
fi

bin_dir="${DCW_BIN_DIR:-$HOME/.local/bin}"
mkdir -p "$bin_dir"

tmp="$bin_dir/.dcw.incoming.$$"
trap 'rm -f "$tmp"' EXIT

echo "→ downloading dcw ($asset)"
if command -v curl >/dev/null 2>&1; then
  curl -fsSL --retry 2 -o "$tmp" "$url"
elif command -v wget >/dev/null 2>&1; then
  wget -q -O "$tmp" "$url"
else
  echo "✗ neither curl nor wget found — install one and retry" >&2
  exit 1
fi
chmod +x "$tmp"

echo "→ installing to $bin_dir/dcw (via dcw init --bin)"
"$tmp" init --yes --bin "$bin_dir"

echo
echo "✓ dcw installed."
echo "  Open a new terminal (or: source ~/.zshenv) so the \`dcw\` shell function loads."
case ":$PATH:" in
  *":$bin_dir:"*) ;;
  *) echo "  (note: $bin_dir isn't on your PATH; add it or rely on the dcw shell function)" ;;
esac
echo "  To reconfigure repo/venv/ports:  dcw init"
echo "  To remove:                      dcw uninstall"