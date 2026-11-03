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
# Seed ~/.zshenv with static content shaped like a user-local zsh setup — the
# same shape this host has (ZSH_LOCAL, module_path, a guarded ~/.local/bin PATH
# block), so the tests are hermetic: they must not depend on (or be fooled by)
# whatever the installer previously wrote to THIS host's ~/.zshenv.
printf '%s\n' \
  '# --- user-local zsh (installed under ~/.local/zsh, no root needed) ---' \
  'export ZSH_LOCAL="$HOME/.local/zsh"' \
  'typeset -a module_path=( "$ZSH_LOCAL/lib/zsh/5.9" $module_path )' \
  'case ":$PATH:" in' \
  '  *":$HOME/.local/bin:"*) ;;' \
  '  *) export PATH="$HOME/.local/bin:$PATH" ;;' \
  'esac' > "$A/.zshenv"
out=$(HOME="$A" bash "$SRC" --bin "$A/bin" "${INSTALL_ARGS[@]}" 2>&1)
has "writes to ~/.zshenv" "$out" "added $A/bin to $A/.zshenv"
has "verifies via login shell" "$out" "✓ verified"
is  "rc block appended once" "$(grep -c 'added by dcw installer' "$A/.zshenv")" "1"
has "pre-existing .zshenv content kept" "$(cat "$A/.zshenv")" "module_path"
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
is  "update source recorded" "$(< "$A/.local/bin/.dcw-source")" "$(cd "$(dirname "$SRC")" && pwd)"
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
# foo has an `hmx` subdir (the Django project) so the wrapper lands in hmx;
# bar is root-only so the fallback-to-root path is still exercised.
F="$D/fakebin"; mkdir -p "$F" "$D/wt/foo/hmx" "$D/wt/bar"
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
is  "cds into hmx when present"    "$(run_fn "$D/wt/foo" 0 'dcw start foo >/dev/null; pwd')" "$D/wt/foo/hmx"
is  "cds into worktree root w/o hmx" "$(run_fn "$D/wt/bar" 0 'dcw start bar >/dev/null; pwd')" "$D/wt/bar"
is  "no cd when the dir is absent" "$(run_fn "$D/wt/gone" 0 'dcw start foo >/dev/null; pwd')" "$START_PWD"
is  "exit code passes through"     "$(run_fn "$D/wt/foo" 3 'dcw start foo >/dev/null; echo rc=$?')" "rc=3"
is  "still cds after a failed start" "$(run_fn "$D/wt/foo" 3 'dcw start foo >/dev/null; pwd')" "$D/wt/foo/hmx"
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

echo "== 10. venv helpers (venvhmx / venvhere) installed and working =="
# A sandbox with a fake project + fake venvs, so the assertions do not depend on
# this host's ~/repo or ~/.venv. venv.sh is sourced from the rc file, so the
# functions must exist in a fresh login shell without any manual .zshrc edit.
V="$D/venvs"; zsh_host "$V"
FAKE_REPO="$V/proj"; FAKE_VENV="$V/venv-cfg"
mkfake_venv() {
  mkdir -p "$1/bin"
  cat > "$1/bin/activate" <<EOF
export VIRTUAL_ENV="$1"
export PATH="\$VIRTUAL_ENV/bin:\$PATH"
deactivate() { PATH="\${PATH#\$VIRTUAL_ENV/bin:}"; unset VIRTUAL_ENV; unset -f deactivate; }
EOF
}
mkfake_venv "$FAKE_VENV"
mkfake_venv "$FAKE_REPO/.venv"
mkdir -p "$FAKE_REPO/hmx"
( cd "$FAKE_REPO" && git init -q . )
out=$(HOME="$V" bash "$SRC" --bin "$V/.local/bin" --repo "$FAKE_REPO" --venv "$FAKE_VENV" \
      --port-base 4200 --port-max 4215 --yes 2>&1)
has "installs the venv helper file" "$out" "venv.sh (venvhmx, venvhere)"
is  "helper file present"  "$([ -f "$V/.config/dcw/shell/venv.sh" ] && echo yes)" "yes"
is  "source line written"  "$(grep -c 'dcw venv helpers' "$V/.zshenv")" "1"
has "verifies the helpers" "$out" "loads venvhere + venvhmx"
# zsh has no `type -t` (that is bash); `whence -w` is the zsh way to ask what a
# name is. Assert the word, not just "something resolved".
is  "fresh zsh has venvhere" "$(HOME="$V" "$ZSH_BIN" -lic 'whence -w venvhere' 2>/dev/null)" "venvhere: function"
is  "fresh zsh has venvhmx"  "$(HOME="$V" "$ZSH_BIN" -lic 'whence -w venvhmx' 2>/dev/null)" "venvhmx: function"

vh() { HOME="$V" "$ZSH_BIN" -lic "$1" 2>/dev/null; }
is "venvhere keeps the cwd"      "$(vh 'cd /tmp && venvhere >/dev/null; pwd')" "/tmp"
is "venvhere uses the config venv" "$(vh 'cd /tmp && venvhere >/dev/null; printf %s "$VIRTUAL_ENV"')" "$FAKE_VENV"
is "venvhmx cds to the repo"     "$(vh "cd /tmp && venvhmx >/dev/null; pwd")" "$FAKE_REPO"
is "venvhmx activates the venv"  "$(vh "cd /tmp && venvhmx >/dev/null; printf %s \"\$VIRTUAL_ENV\"")" "$FAKE_VENV"
# A .venv inside the project wins over the shared/config one, and venvhere must
# still not move the caller (that is the whole point of the helper).
is "project .venv wins"          "$(vh 'cd '"$FAKE_REPO"' && venvhere >/dev/null; printf %s "$VIRTUAL_ENV"')" "$FAKE_REPO/.venv"
is "no cd from a nested dir"     "$(vh 'cd '"$FAKE_REPO/hmx"' && venvhere >/dev/null; pwd')" "$FAKE_REPO/hmx"
is "explicit venv argument wins" "$(vh "venvhere $FAKE_VENV >/dev/null; printf %s \"\$VIRTUAL_ENV\"")" "$FAKE_VENV"
is "bad path exits 1"            "$(vh 'venvhere /nonexistent >/dev/null 2>&1; echo rc=$?')" "rc=1"
has "bad path is reported"       "$(vh 'venvhere /nonexistent 2>&1 >/dev/null')" "venvhere: venv tidak ditemukan"
is "no venv left active on failure" "$(vh 'venvhere /nonexistent >/dev/null 2>&1; printf %s "${VIRTUAL_ENV:-none}"')" "none"
is "single PATH entry after repeat" "$(vh "venvhere >/dev/null; venvhere >/dev/null; printf %s \"\$PATH\" | tr ':' '\n' | grep -cx '$FAKE_VENV/bin'")" "1"
out=$(HOME="$V" bash "$SRC" --bin "$V/.local/bin" --repo "$FAKE_REPO" --venv "$FAKE_VENV" \
      --port-base 4200 --port-max 4215 --yes 2>&1)
has "re-run: already sourced" "$out" "already sourced in $V/.zshenv"
is  "no duplicate source line" "$(grep -c 'dcw venv helpers' "$V/.zshenv")" "1"

echo "== 11. uninstall strips the rc blocks and leaves the rest alone =="
printf '# sentinel: this line is not ours\n' >> "$V/.zshenv"
out=$(HOME="$V" bash "$(cd "$(dirname "$SRC")" && pwd)/uninstall.sh" "$V/.local/bin" 2>&1)
has "removes the binary" "$out" "removed $V/.local/bin/dcw"
is  "binary gone"       "$([ -e "$V/.local/bin/dcw" ] && echo yes || echo no)" "no"
is  "config dir gone"   "$([ -d "$V/.config/dcw" ] && echo yes || echo no)" "no"
is  "no dcw markers left" "$(grep -c 'added by dcw installer\|dcw shell integration\|dcw venv helpers' "$V/.zshenv")" "0"
is  "no function left"  "$(HOME="$V" "$ZSH_BIN" -lic 'type -t venvhere' 2>/dev/null)" ""
is  "rc backup written" "$([ -f "$V/.zshenv.dcw-uninstall.bak" ] && echo yes)" "yes"
has "backup still has the block" "$(cat "$V/.zshenv.dcw-uninstall.bak")" "dcw venv helpers"
has "unrelated rc content kept" "$(cat "$V/.zshenv")" "sentinel"

if [ "${SKIP_LIVE:-0}" = 1 ]; then
  echo "== 12. live install on this host — SKIPPED (SKIP_LIVE=1) =="
else
  echo "== 12. live install on this host =="
  is  "dcw installed" "$([ -x "$HOME/.local/bin/dcw" ] && echo yes)" "yes"
  cfg=$(cat "$HOME/.config/dcw/config" 2>/dev/null || true)
  has "config repo"  "$cfg" "DEV_CAPTAIN_REPO:=$REPO"
  has "config venv"  "$cfg" "DEV_CAPTAIN_VENV:=$VENV"
  has "config ports" "$cfg" 'PORT_BASE:=4200'
  is  "login shell loads dcw as a function" "$("$ZSH_BIN" -lic 'whence -w dcw' 2>/dev/null)" "dcw: function"
  is  "dcw path <branch> points into .worktrees" \
      "$("$ZSH_BIN" -lic 'dcw path dcw-selftest' 2>/dev/null)" "$REPO/.worktrees/dcw-selftest"
  is  "venv helpers installed" "$([ -f "$HOME/.config/dcw/shell/venv.sh" ] && echo yes)" "yes"
  is  "login shell has venvhere" "$("$ZSH_BIN" -lic 'whence -w venvhere' 2>/dev/null)" "venvhere: function"
  is  "login shell has venvhmx"  "$("$ZSH_BIN" -lic 'whence -w venvhmx' 2>/dev/null)" "venvhmx: function"
  # Not "no active captains": this host may legitimately have worktrees running.
  is  "dcw status exits 0" "$("$HOME/.local/bin/dcw" status >/dev/null 2>&1; echo rc=$?)" "rc=0"
fi

echo
echo "== summary: $PASS passed, $FAIL failed =="
[ "$FAIL" -eq 0 ]
