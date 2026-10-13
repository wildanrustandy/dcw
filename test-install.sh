#!/usr/bin/env bash
# test-install.sh — assertion tests for install.sh's shell/PATH detection.
#
# There is no test framework in this repo; this is a self-contained assertion
# script. Run it after touching install.sh:
#
#     ./test-install.sh              # groups 0-10
#     SKIP_LIVE=1 ./test-install.sh  # skip group 10 (live install on this host)
#
# Groups 1-3 and 7 build a fake HOME shaped like a no-root DevSpace host:
# /etc/passwd says /bin/bash (chsh needs root) while ~/.bashrc re-execs a
# user-local zsh — the setup that made the old installer write PATH to a file
# that is never executed.
#
# Group 8 needs the HOME built by group 7 (the shell function installed there).
# Group 10 asserts the real install on this host; it needs ~/.local/bin/dcw and
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
START_PWD=$(pwd)
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
# Seed with a dcw-free copy of the real ~/.zshenv. It must drop the WHOLE shell
# function block (marker + comments + body), not just the marker line: leaving a
# stray function would make these sandboxes pass for the wrong reason.
awk '
  /dcw shell integration \(added by installer\)/ { skip = 1; next }
  skip && /^}/                                  { skip = 0; next }
  skip                                          { next }
  /added by dcw installer/                      { next }
  /^export PATH="/                             { next }
  { print }
' "$HOME/.zshenv" > "$A/.zshenv"
out=$(HOME="$A" bash "$SRC" --bin "$A/bin" "${INSTALL_ARGS[@]}" 2>&1)
has "writes to ~/.zshenv" "$out" "added $A/bin to $A/.zshenv"
has "verifies via login shell" "$out" "✓ verified"
is  "rc block appended once" "$(grep -c 'added by dcw installer' "$A/.zshenv")" "1"
# dcw is a FUNCTION now, so `command -v dcw` prints "dcw" — assert the function
# loaded (whence -w) and, separately, that the PATH edit really took effect.
is  "fresh zsh loads dcw as a function" "$(HOME="$A" "$ZSH_BIN" -lic 'whence -w dcw' 2>/dev/null)" "dcw: function"
has "bin dir on the fresh login PATH" "$(HOME="$A" "$ZSH_BIN" -lic 'printf %s "$PATH"' 2>/dev/null)" "$A/bin"

echo "== 2. idempotent re-run =="
out=$(HOME="$A" bash "$SRC" --bin "$A/bin" "${INSTALL_ARGS[@]}" 2>&1)
has "reports already on PATH" "$out" "already on the login shell's PATH"
is  "no duplicate rc block" "$(grep -c 'added by dcw installer' "$A/.zshenv")" "1"
has "reports function already declared" "$out" "dcw function already declared"
is  "no duplicate function block" "$(grep -c 'dcw shell integration' "$A/.zshenv")" "1"

echo "== 3. --bin auto-pick (~/.local/bin already on the login PATH) =="
out=$(HOME="$A" bash "$SRC" "${INSTALL_ARGS[@]}" 2>&1)
has "picks ~/.local/bin" "$out" "bin:        $A/.local/bin"
is  "binary installed there" "$([ -x "$A/.local/bin/dcw" ] && echo yes)" "yes"
has "flags the stale function (installed for $A/bin)" "$out" "pointing at another binary"

echo "== 4. bash host: escalate past ~/.bashrc (a login bash never reads it) =="
B="$D/bash"; mkdir -p "$B"
printf 'export PATH="%s/.local/bin:$PATH"\n' "$HOME" > "$B/.bashrc"
out=$(HOME="$B" SHELL=/bin/bash bash "$SRC" --bin "$B/bin" "${INSTALL_ARGS[@]}" 2>&1)
has "escalates to .bash_profile" "$out" "added $B/bin to $B/.bash_profile"
has "verifies via login shell" "$out" "✓ verified"
is  "fresh login bash resolves dcw"     "$(HOME="$B" SHELL=/bin/bash bash -lic 'type -t dcw' 2>/dev/null)" "function"
is  "fresh non-login bash resolves dcw" "$(HOME="$B" SHELL=/bin/bash bash -ic  'type -t dcw' 2>/dev/null)" "function"
has "bin dir on the fresh login PATH"   "$(HOME="$B" SHELL=/bin/bash bash -lic 'printf %s "$PATH"' 2>/dev/null)" "$B/bin"

echo "== 5. an existing ~/.profile must not be shadowed =="
C="$D/profile"; mkdir -p "$C"
printf 'export MARKER_FROM_PROFILE=1\n' > "$C/.profile"
HOME="$C" SHELL=/bin/bash bash "$SRC" --bin "$C/bin" "${INSTALL_ARGS[@]}" >/dev/null 2>&1
is "no .bash_profile created" "$([ -e "$C/.bash_profile" ] && echo yes || echo no)" "no"
is "appends to .profile once" "$(grep -c 'added by dcw' "$C/.profile")" "1"
is "profile still sourced + dcw found" \
   "$(HOME="$C" SHELL=/bin/bash bash -lic 'printf "%s:" "${MARKER_FROM_PROFILE:-MISSING}"; type -t dcw' 2>/dev/null)" \
   "1:function"

echo "== 6. a hanging rc file must not block the installer =="
H="$D/hang"; mkdir -p "$H"
printf '# hangs\nsleep 30\n' > "$H/.profile"
printf '# nothing\n' > "$H/.bashrc"
s=$(date +%s)
HOME="$H" SHELL=/bin/bash timeout 60 bash "$SRC" --bin "$H/bin" "${INSTALL_ARGS[@]}" >/dev/null 2>&1
rc=$?; el=$(( $(date +%s) - s ))
is "exits cleanly" "$rc" "0"
# Two verify loops (PATH, then the shell function) each poll ~3s before giving
# up on a hanging rc file — 30s+ means the watchdog is gone, 20s means it works.
[ "$el" -lt 20 ] && ok "bounded to ${el}s (interactive shells ignore SIGTERM)" || bad "took ${el}s — watchdog ineffective"

echo "== 7. already on PATH => no PATH block, function still installed once =="
G="$D/clean"; zsh_host "$G"
printf 'export PATH="%s/.local/bin:$PATH"\n' "$G" > "$G/.zshenv"
out=$(HOME="$G" bash "$SRC" --bin "$G/.local/bin" "${INSTALL_ARGS[@]}" 2>&1)
has "reports already present" "$out" "already on the login shell's PATH"
is  "no PATH block added" "$(grep -c 'added by dcw installer' "$G/.zshenv")" "0"
is  "pre-existing PATH line kept" "$(grep -c "^export PATH=\"$G/.local/bin:" "$G/.zshenv")" "1"
is  "function block added" "$(grep -c 'dcw shell integration' "$G/.zshenv")" "1"
has "verifies the function" "$out" "✓ verified"
is  "fresh zsh loads dcw as a function" "$(HOME="$G" "$ZSH_BIN" -lic 'whence -w dcw' 2>/dev/null)" "dcw: function"
out=$(HOME="$G" bash "$SRC" --bin "$G/.local/bin" "${INSTALL_ARGS[@]}" 2>&1)
has "reports function already declared" "$out" "dcw function already declared"
is  "function block not duplicated" "$(grep -c 'dcw shell integration' "$G/.zshenv")" "1"

echo "== 8. the function cds into the worktree after 'dcw start' =="
# A stub binary: `path` answers with $FAKE_WT, `start` prints and exits $FAKE_RC.
F="$D/fakebin"; mkdir -p "$F" "$D/wt/foo"
cat > "$F/dcw" <<'FAKE'
#!/usr/bin/env bash
case "${1:-}" in
  path)  printf '%s\n' "$FAKE_WT" ;;
  start) printf 'fake start %s\n' "${2:-}"; exit "${FAKE_RC:-0}" ;;
  *)     printf 'fake %s\n' "$*" ;;
esac
FAKE
chmod +x "$F/dcw"
run_fn() { HOME="$G" DCW_BIN="$F/dcw" FAKE_WT="$1" FAKE_RC="$2" "$ZSH_BIN" -lic "$3" 2>/dev/null; }
is  "cds into the worktree"        "$(run_fn "$D/wt/foo" 0 'dcw start foo >/dev/null; pwd')" "$D/wt/foo"
is  "no cd when the dir is absent" "$(run_fn "$D/wt/gone" 0 'dcw start foo >/dev/null; pwd')" "$START_PWD"
is  "exit code passes through"     "$(run_fn "$D/wt/foo" 3 'dcw start foo >/dev/null; echo rc=$?')" "rc=3"
is  "still cds after a failed start" "$(run_fn "$D/wt/foo" 3 'dcw start foo >/dev/null; pwd')" "$D/wt/foo"
has "other subcommands pass through" "$(run_fn "$D/wt/foo" 0 'dcw status')" "fake status"
is  "other subcommands do not cd"  "$(run_fn "$D/wt/foo" 0 'dcw status >/dev/null; pwd')" "$START_PWD"
is  "bare 'start' is passed through" "$(run_fn "$D/wt/foo" 0 'dcw start >/dev/null; pwd')" "$START_PWD"

echo "== 9. unsupported login shell: no function, warning only =="
FS="$D/fish"; mkdir -p "$FS"
out=$(HOME="$FS" SHELL=/usr/bin/fish bash "$SRC" --bin "$FS/bin" "${INSTALL_ARGS[@]}" 2>&1)
has "warns there is no integration" "$out" "no integration for fish"
has "summary reports none" "$out" "shell:      none"
# grep -c prints 0 AND exits 1 on no match, so `|| echo 0` would double it.
fish_fn=$(grep -c 'dcw shell integration' "$FS/.config/fish/config.fish" 2>/dev/null || true)
is  "no function block written" "${fish_fn:-0}" "0"

if [ "${SKIP_LIVE:-0}" = 1 ]; then
  echo "== 10. live install on this host — SKIPPED (SKIP_LIVE=1) =="
else
  echo "== 10. live install on this host =="
  is  "dcw installed" "$([ -x "$HOME/.local/bin/dcw" ] && echo yes)" "yes"
  cfg=$(cat "$HOME/.config/dcw/config" 2>/dev/null || true)
  has "config repo"  "$cfg" "DEV_CAPTAIN_REPO:=$REPO"
  has "config venv"  "$cfg" "DEV_CAPTAIN_VENV:=$VENV"
  has "config ports" "$cfg" 'PORT_BASE:=4200'
  is  "login shell loads dcw as a function" "$("$ZSH_BIN" -lic 'whence -w dcw' 2>/dev/null)" "dcw: function"
  is  "dcw path <branch> points into .worktrees" \
      "$("$ZSH_BIN" -lic 'dcw path dcw-selftest' 2>/dev/null)" "$REPO/.worktrees/dcw-selftest"
  is  "dcw status runs" "$("$HOME/.local/bin/dcw" status 2>&1)" "(no active captains)"
fi

echo
echo "== summary: $PASS passed, $FAIL failed =="
[ "$FAIL" -eq 0 ]
