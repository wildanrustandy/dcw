#!/usr/bin/env bash
# test-install.sh — assertion tests for install.sh's shell/PATH detection.
#
# There is no test framework in this repo; this is a self-contained assertion
# script. Run it after touching install.sh:
#
#     ./test-install.sh              # groups 0-8
#     SKIP_LIVE=1 ./test-install.sh  # skip group 8 (live install on this host)
#
# Groups 1-3 and 7 build a fake HOME shaped like a no-root DevSpace host:
# /etc/passwd says /bin/bash (chsh needs root) while ~/.bashrc re-execs a
# user-local zsh — the setup that made the old installer write PATH to a file
# that is never executed.
#
# Group 8 asserts the real install on this host; it needs ~/.local/bin/dcw and
# ~/.config/dcw/config to exist. Override REPO/VENV if yours differ.
set -uo pipefail

SRC="$(cd "$(dirname "$0")" && pwd)/install.sh"
ZSH_BIN="${ZSH_BIN:-$HOME/.local/zsh/bin/zsh}"
REPO="${REPO:-$HOME/repo}"
VENV="${VENV:-$HOME/.venv}"
INSTALL_ARGS=(--repo "$REPO" --venv "$VENV" --port-base 4200 --port-max 4215 --yes)

[ -f "$SRC" ] || { echo "error: install.sh not found next to this script" >&2; exit 2; }
[ -x "$ZSH_BIN" ] || { echo "error: no zsh at $ZSH_BIN (set ZSH_BIN=...)" >&2; exit 2; }

PASS=0; FAIL=0
ok()  { PASS=$((PASS+1)); printf '  PASS  %s\n' "$1"; }
bad() { FAIL=$((FAIL+1)); printf '  FAIL  %s\n' "$1"; }
is()  { [ "$2" = "$3" ] && ok "$1" || bad "$1 — got [$2] want [$3]"; }
has() { case "$2" in *"$3"*) ok "$1" ;; *) bad "$1 — [$2] lacks [$3]" ;; esac; }

D=$(mktemp -d /tmp/dcw-install-test.XXXXXX)
trap 'rm -rf "$D"' EXIT

# a sandbox HOME whose bash hands over to the user-local zsh
zsh_host() {
  local h="$1"
  mkdir -p "$h/.local/bin"
  printf 'if [[ $- == *i* ]] && [[ -z "$ZSH_VERSION" ]] && [[ -x "$HOME/.local/zsh/bin/zsh" ]]; then\n    exec "$HOME/.local/zsh/bin/zsh" -l\nfi\n' > "$h/.bashrc"
}

echo "== 0. syntax =="
bash -n "$SRC" && ok "bash -n install.sh" || bad "bash -n install.sh"

echo "== 1. zsh host: PATH lands in ~/.zshenv, verified against the shell =="
A="$D/zsh"; zsh_host "$A"
grep -v 'added by dcw installer' "$HOME/.zshenv" | grep -v '^export PATH="' > "$A/.zshenv" || true
out=$(HOME="$A" bash "$SRC" --bin "$A/bin" "${INSTALL_ARGS[@]}" 2>&1)
has "writes to ~/.zshenv" "$out" "added $A/bin to $A/.zshenv"
has "verifies via login shell" "$out" "✓ verified"
is  "rc block appended once" "$(grep -c 'added by dcw installer' "$A/.zshenv")" "1"
is  "fresh zsh resolves dcw" "$(HOME="$A" "$ZSH_BIN" -lic 'command -v dcw' 2>/dev/null)" "$A/bin/dcw"

echo "== 2. idempotent re-run =="
out=$(HOME="$A" bash "$SRC" --bin "$A/bin" "${INSTALL_ARGS[@]}" 2>&1)
has "reports already on PATH" "$out" "already on the login shell's PATH"
is  "no duplicate rc block" "$(grep -c 'added by dcw installer' "$A/.zshenv")" "1"

echo "== 3. --bin auto-pick (~/.local/bin already on the login PATH) =="
out=$(HOME="$A" bash "$SRC" "${INSTALL_ARGS[@]}" 2>&1)
has "picks ~/.local/bin" "$out" "bin:        $A/.local/bin"
is  "binary installed there" "$([ -x "$A/.local/bin/dcw" ] && echo yes)" "yes"

echo "== 4. bash host: escalate past ~/.bashrc (a login bash never reads it) =="
B="$D/bash"; mkdir -p "$B"
printf 'export PATH="%s/.local/bin:$PATH"\n' "$HOME" > "$B/.bashrc"
out=$(HOME="$B" SHELL=/bin/bash bash "$SRC" --bin "$B/bin" "${INSTALL_ARGS[@]}" 2>&1)
has "escalates to .bash_profile" "$out" "added $B/bin to $B/.bash_profile"
has "verifies via login shell" "$out" "✓ verified"
is  "fresh login bash resolves dcw"     "$(HOME="$B" SHELL=/bin/bash bash -lic 'command -v dcw' 2>/dev/null)" "$B/bin/dcw"
is  "fresh non-login bash resolves dcw" "$(HOME="$B" SHELL=/bin/bash bash -ic  'command -v dcw' 2>/dev/null)" "$B/bin/dcw"

echo "== 5. an existing ~/.profile must not be shadowed =="
C="$D/profile"; mkdir -p "$C"
printf 'export MARKER_FROM_PROFILE=1\n' > "$C/.profile"
HOME="$C" SHELL=/bin/bash bash "$SRC" --bin "$C/bin" "${INSTALL_ARGS[@]}" >/dev/null 2>&1
is "no .bash_profile created" "$([ -e "$C/.bash_profile" ] && echo yes || echo no)" "no"
is "appends to .profile once" "$(grep -c 'added by dcw' "$C/.profile")" "1"
is "profile still sourced + dcw found" \
   "$(HOME="$C" SHELL=/bin/bash bash -lic 'printf "%s:" "${MARKER_FROM_PROFILE:-MISSING}"; command -v dcw' 2>/dev/null)" \
   "1:$C/bin/dcw"

echo "== 6. a hanging rc file must not block the installer =="
H="$D/hang"; mkdir -p "$H"
printf '# hangs\nsleep 30\n' > "$H/.profile"
printf '# nothing\n' > "$H/.bashrc"
s=$(date +%s)
HOME="$H" SHELL=/bin/bash timeout 60 bash "$SRC" --bin "$H/bin" "${INSTALL_ARGS[@]}" >/dev/null 2>&1
rc=$?; el=$(( $(date +%s) - s ))
is "exits cleanly" "$rc" "0"
[ "$el" -lt 15 ] && ok "bounded to ${el}s (interactive shells ignore SIGTERM)" || bad "took ${el}s — watchdog ineffective"

echo "== 7. already on PATH => zero dotfile writes =="
G="$D/clean"; zsh_host "$G"
printf 'export PATH="%s/.local/bin:$PATH"\n' "$G" > "$G/.zshenv"
before=$(md5sum "$G/.zshenv" | cut -d' ' -f1)
out=$(HOME="$G" bash "$SRC" --bin "$G/.local/bin" "${INSTALL_ARGS[@]}" 2>&1)
after=$(md5sum "$G/.zshenv" | cut -d' ' -f1)
has "reports already present" "$out" "already on the login shell's PATH"
is  "~/.zshenv untouched" "$before" "$after"
is  "no dcw block added" "$(grep -c 'added by dcw installer' "$G/.zshenv")" "0"

if [ "${SKIP_LIVE:-0}" = 1 ]; then
  echo "== 8. live install on this host — SKIPPED (SKIP_LIVE=1) =="
else
  echo "== 8. live install on this host =="
  is  "dcw installed" "$([ -x "$HOME/.local/bin/dcw" ] && echo yes)" "yes"
  cfg=$(cat "$HOME/.config/dcw/config" 2>/dev/null || true)
  has "config repo"  "$cfg" "DEV_CAPTAIN_REPO:=$REPO"
  has "config venv"  "$cfg" "DEV_CAPTAIN_VENV:=$VENV"
  has "config ports" "$cfg" 'PORT_BASE:=4200'
  is  "login shell resolves dcw" "$("$ZSH_BIN" -lic 'command -v dcw' 2>/dev/null)" "$HOME/.local/bin/dcw"
  is  "dcw status runs" "$("$HOME/.local/bin/dcw" status 2>&1)" "(no active captains)"
fi

echo
echo "== summary: $PASS passed, $FAIL failed =="
[ "$FAIL" -eq 0 ]
