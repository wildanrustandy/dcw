#!/usr/bin/env bash
# install.sh — install dcw, interactively configuring project & venv paths
# Usage: ./install.sh [options]
#   Interactive when run from a terminal; flags can pre-seed/override answers.
set -euo pipefail

usage() {
  cat <<'EOF'
Usage: ./install.sh [options]

Options:
  --bin DIR        install dcw into DIR                (default: auto, see below)
  --repo PATH      HMX project root                    (default: ~/dev/hmx-002)
  --venv PATH      python venv path                    (default: ~/dev/.venv)
  --port-base N    base port for the base branch       (default: 5200)
  --port-max N     upper bound of the port range       (default: 5220)
  --yes            accept answers non-interactively (use flags/defaults)
  -h, --help       this help

Answers are saved to ~/.config/dcw/config (dcw reads it at runtime).
Environment variables still override the config per-call.

--bin default is the first of ~/.local/bin, ~/bin that the LOGIN shell already
searches, else ~/.local/bin. PATH is appended to the rc file that shell really
reads (~/.zshenv for zsh; ~/.bashrc plus the login file for bash), then the
installer re-asks the shell to confirm it took effect. $SHELL is never trusted:
on a no-root host /etc/passwd still says bash while ~/.bashrc re-execs a
user-local zsh.
EOF
}

# ── defaults ──────────────────────────────────────────────────────
BIN_DIR=""                  # empty = auto-pick, resolved once the login PATH is known
ANS_REPO=""
ANS_VENV=""
ANS_PBASE=""
ANS_PMAX=""
ASSUME_YES=0

# ── flag parsing ──────────────────────────────────────────────────
while [ $# -gt 0 ]; do
  case "$1" in
    --bin)       BIN_DIR="${2:?}"; shift 2 ;;
    --repo)      ANS_REPO="${2:?}"; shift 2 ;;
    --venv)      ANS_VENV="${2:?}"; shift 2 ;;
    --port-base) ANS_PBASE="${2:?}"; shift 2 ;;
    --port-max)  ANS_PMAX="${2:?}"; shift 2 ;;
    --yes|-y)    ASSUME_YES=1; shift ;;
    -h|--help)   usage; exit 0 ;;
    *)           echo "unknown option: $1" >&2; usage >&2; exit 2 ;;
  esac
done

SRC="$(cd "$(dirname "$0")" && pwd)/dcw"
[ -f "$SRC" ] || { echo "error: dcw script not found next to install.sh" >&2; exit 1; }
SHELL_SRC="$(cd "$(dirname "$0")" && pwd)/shell/venv.sh"   # venvhmx / venvhere

# ── shell / PATH detection ────────────────────────────────────────
# $SHELL lies on a no-root host: /etc/passwd still says /bin/bash (chsh needs
# root) while ~/.bashrc re-execs a user-local zsh. In that setup the PATH line
# written to ~/.bashrc is never executed, and zsh never reads ~/.bashrc at all.
# So: find the shell a NEW terminal actually starts, and read PATH from it.

_login_shell() { # shell a new terminal will run
  local sh="${SHELL:-/bin/sh}" hit
  if [ -f "$HOME/.bashrc" ]; then
    hit=$(grep -E '^[[:space:]]*exec[[:space:]]+.*(zsh|fish)' "$HOME/.bashrc" 2>/dev/null | head -1 || true)
    case "$hit" in
      *zsh*)  if [ -x "$HOME/.local/zsh/bin/zsh" ]; then sh="$HOME/.local/zsh/bin/zsh"
              else sh=$(command -v zsh 2>/dev/null || printf '%s' "$sh"); fi ;;
      *fish*) sh=$(command -v fish 2>/dev/null || printf '%s' "$sh") ;;
    esac
  fi
  printf '%s' "$sh"
}

_rc_candidates_for() { # rc files to try, in order, for a given shell
  case "$(basename "${1:-sh}")" in
    zsh)  printf '%s\n' "$HOME/.zshenv" ;;   # read by every zsh: login, interactive, script
    bash)
      printf '%s\n' "$HOME/.bashrc"          # non-login interactive (most terminals, ttyd)
      for f in "$HOME/.bash_profile" "$HOME/.bash_login" "$HOME/.profile"; do
        if [ -e "$f" ]; then printf '%s\n' "$f"; return 0; fi   # first one a LOGIN bash reads
      done
      printf '%s\n' "$HOME/.bash_profile"    # none exists -> creating it shadows nothing
      ;;
    fish) printf '%s\n' "$HOME/.config/fish/config.fish" ;;
    *)    printf '%s\n' "$HOME/.profile" ;;
  esac
}

_ask_login_shell() { # run CMD in the login shell; echo its stdout (empty if unqueryable)
  # The marker survives rc-file noise (banners, plugin output) on stdout: keep
  # everything after the LAST marker, up to the end of that line.
  local sh="$1" marker="$2" cmd="$3" tmp pid n=0 raw
  tmp=$(mktemp 2>/dev/null || printf '%s' "$HOME/.dcw.ask.$$")
  ( "$sh" -lic "printf '\\n$marker%s\\n' \"\$($cmd)\"" >"$tmp" 2>/dev/null ) &
  pid=$!
  # An rc file can hang (waiting on input, network, a sleeping hook). Poll, then
  # SIGKILL: an INTERACTIVE shell ignores SIGTERM, so TERM alone would let
  # `wait` block for as long as the rc file does.
  while kill -0 "$pid" 2>/dev/null && [ "$n" -lt 30 ]; do sleep 0.1; n=$((n+1)); done
  kill -TERM "$pid" 2>/dev/null || true
  sleep 0.2
  kill -KILL "$pid" 2>/dev/null || true
  wait "$pid" 2>/dev/null || true
  raw=$(cat "$tmp" 2>/dev/null || true)
  rm -f "$tmp"
  case "$raw" in
    *"$marker"*) raw=${raw##*"$marker"}   # last marker: rc files may print too
                 raw=${raw%%$'\n'*} ;;
    *)           raw="" ;;                # could not query the shell
  esac
  printf '%s' "$raw"
}

_login_path() { _ask_login_shell "$1" '__DCW_PATH__' 'printf "%s" "$PATH"'; }

_login_shell_has_function() { # is NAME a shell function in a fresh login shell?
  # There is no portable type flag: zsh has `whence -w`, bash has `type -t` (and
  # zsh's `type -t` silently fails). Plain `type NAME` works in both and says
  # "NAME is a shell function from <file>" (zsh) / "NAME is a function" (bash),
  # while a binary answers "NAME is /path/NAME". So match the word.
  case "$(_ask_login_shell "$1" '__DCW_FN__' "type $2 2>/dev/null")" in
    *function*) return 0 ;;
    *)           return 1 ;;
  esac
}

_login_shell_knows_dcw() { # a BINARY would answer "is /path/dcw" — not enough
  _login_shell_has_function "$1" dcw
}

INTERACTIVE=0
if [ "$ASSUME_YES" = 0 ] && [ -t 0 ] && [ -t 1 ]; then
  INTERACTIVE=1
fi

# expand ~ in flag values
expand() { local v="${1/#\~/$HOME}"; printf '%s' "$v"; }
ANS_REPO=$(expand "$ANS_REPO"); ANS_VENV=$(expand "$ANS_VENV")
BIN_DIR=$(expand "$BIN_DIR")

# ── resolve install dir against the login shell's real PATH ───────
LOGIN_SHELL=$(_login_shell)
LOGIN_PATH=$(_login_path "$LOGIN_SHELL")
ASKED=1
if [ -z "$LOGIN_PATH" ]; then      # shell could not be queried — fall back, and say so later
  ASKED=0
  LOGIN_PATH="$PATH"
fi
if [ -z "$BIN_DIR" ]; then
  for cand in "$HOME/.local/bin" "$HOME/bin"; do
    case ":$LOGIN_PATH:" in
      *":$cand:"*) BIN_DIR="$cand"; break ;;
    esac
  done
  [ -n "$BIN_DIR" ] || BIN_DIR="$HOME/.local/bin"
fi

echo "== dcw installer =="
if [ "$INTERACTIVE" = 0 ] && [ "$ASSUME_YES" = 0 ]; then
  echo "(non-interactive session: using defaults/flags; re-run from a terminal to be prompted)"
fi

# ── interactive questions (defaults shown in brackets) ────────────
REPO_PATH="${ANS_REPO:-$HOME/dev/hmx-002}"
VENV_PATH="${ANS_VENV:-$HOME/dev/.venv}"
PORT_BASE="${ANS_PBASE:-5200}"
PORT_MAX="${ANS_PMAX:-5220}"

if [ "$INTERACTIVE" = 1 ]; then
  ask() { # ask VAR "prompt" "default" [validator]
    local __var="$1" __prompt="$2" __def="$3" __val=""
    while :; do
      printf '%s [%s]: ' "$__prompt" "$__def"
      IFS= read -r __val </dev/tty || true
      __val=$(expand "${__val:-$__def}")
      case "$__var" in
        PORT_BASE|PORT_MAX)
          [[ "$__val" =~ ^[0-9]+$ ]] || { echo "  please enter a number"; continue; } ;;
      esac
      [ -n "$__val" ] && break
    done
    printf -v "$__var" '%s' "$__val"
  }

  ask REPO_PATH  "HMX project folder (repo root)"        "$REPO_PATH"
  ask VENV_PATH  "Python venv folder"                    "$VENV_PATH"
  ask PORT_BASE  "Base port (base branch server)"        "$PORT_BASE"
  ask PORT_MAX   "Max port (end of worktree range)"      "$PORT_MAX"
fi

# ── validation (warnings only, install can proceed) ───────────────
WARN=0
[ -d "$REPO_PATH/.git" ] || { echo "⚠ warning: $REPO_PATH is not a git repo (dcw start will refuse until it is)"; WARN=1; }
[ -f "$VENV_PATH/bin/activate" ] || { echo "⚠ warning: no venv found at $VENV_PATH (server will fall back to system python3)"; WARN=1; }
[ "$PORT_MAX" -gt "$PORT_BASE" ] || { echo "✗ error: PORT_MAX ($PORT_MAX) must be greater than PORT_BASE ($PORT_BASE)"; exit 2; }
command -v psql >/dev/null 2>&1 || { echo "⚠ warning: psql not found — per-worktree DB clone will be skipped (brew install postgresql@16 libpq)"; WARN=1; }

# ── install script (atomic: never overwrite a running dcw mid-read) ─
mkdir -p "$BIN_DIR"
cp "$SRC" "$BIN_DIR/.dcw.incoming.$$"
chmod +x "$BIN_DIR/.dcw.incoming.$$"
mv -f "$BIN_DIR/.dcw.incoming.$$" "$BIN_DIR/dcw"
echo "✓ installed: $BIN_DIR/dcw"

# ── write user config ─────────────────────────────────────────────
CONFIG_DIR="$HOME/.config/dcw"
mkdir -p "$CONFIG_DIR"
cat > "$CONFIG_DIR/config" <<EOF
# dcw config — generated by dcw installer $(date +%Y-%m-%d)
# Precedence: environment variables > this file > built-in defaults
: "\${DEV_CAPTAIN_REPO:=$REPO_PATH}"
: "\${DEV_CAPTAIN_VENV:=$VENV_PATH}"
: "\${PORT_BASE:=$PORT_BASE}"
: "\${PORT_MAX:=$PORT_MAX}"
EOF
echo "✓ config:   $CONFIG_DIR/config"

# ── venv helpers (venvhmx / venvhere) ─────────────────────────────
# Shipped as a file next to the config and sourced from the rc file (see the
# shell-integration block below). A file rather than an inline block: re-running
# the installer then refreshes the functions without touching the rc file again.
VENV_DEST="$CONFIG_DIR/shell/venv.sh"
if [ -f "$SHELL_SRC" ]; then
  mkdir -p "$(dirname "$VENV_DEST")"
  cp "$SHELL_SRC" "$VENV_DEST"
  echo "✓ venv:     $VENV_DEST (venvhmx, venvhere)"
else
  echo "⚠ venv:     shell/venv.sh not found next to install.sh — venv helpers NOT installed"
fi

# ── PATH ──────────────────────────────────────────────────────────
# Edit the rc files, then RE-ASK the login shell. A login bash does not read
# ~/.bashrc, so the first candidate can silently do nothing — escalate to the
# next one instead of trusting the edit.
case ":$LOGIN_PATH:" in
  *":$BIN_DIR:"*)
    echo "✓ PATH: $BIN_DIR already on the login shell's PATH ($LOGIN_SHELL)"
    ;;
  *)
    VERIFIED=0
    for SHELL_RC in $(_rc_candidates_for "$LOGIN_SHELL"); do
      mkdir -p "$(dirname "$SHELL_RC")"
      if [ -f "$SHELL_RC" ] && grep -Fq "export PATH=\"$BIN_DIR:" "$SHELL_RC"; then
        echo "✓ PATH: $BIN_DIR already declared in $SHELL_RC"
      else
        printf '\n# added by dcw installer\nexport PATH="%s:$PATH"\n' "$BIN_DIR" >> "$SHELL_RC"
        echo "✓ PATH: added $BIN_DIR to $SHELL_RC"
      fi
      [ "$ASKED" = 1 ] || break
      case ":$(_login_path "$LOGIN_SHELL"):" in
        *":$BIN_DIR:"*)
          echo "  ✓ verified: $LOGIN_SHELL resolves $BIN_DIR/dcw"
          VERIFIED=1; break ;;
      esac
    done
    if [ "$ASKED" = 0 ]; then
      echo "  ⚠ could not query $LOGIN_SHELL — open a new terminal to confirm"
    elif [ "$VERIFIED" = 0 ]; then
      echo "  ⚠ unverified: $BIN_DIR still missing from $LOGIN_SHELL's PATH"
      echo "    add it manually to one of: $(_rc_candidates_for "$LOGIN_SHELL" | tr '\n' ' ')"
    fi
    ;;
esac

# ── shell integration: `dcw start` also cds into the worktree ─────
# Only the CALLING shell can change its own working directory, so this can never
# live in the binary: it is a thin function that asks the binary for the target
# dir ('dcw path <branch>') and cds there after `start`. Written to the same rc
# file the PATH block targets, then verified the same way — by asking a fresh
# login shell whether dcw really IS a function (the binary alone would pass a
# `command -v` check, which is exactly the failure mode to catch).
DCW_BIN="$BIN_DIR/dcw"
INTEG_MARKER='# dcw shell integration (added by installer)'
INTEG_NOTE="none ($(basename "$LOGIN_SHELL"): cd \"\$(dcw path <branch>)\" after start)"
case "$(basename "$LOGIN_SHELL")" in
  zsh|bash)
    INTEG_NOTE="dcw function (cd after 'dcw start')"
    INTEG_BODY=$(sed "s|@BIN@|$DCW_BIN|g" <<'INTEG'
# A function, not the binary: a child process cannot change its parent shell's
# directory, so `dcw start` could not cd anywhere without this. Every subcommand
# other than `start` is passed straight through. Set DCW_BIN to point the
# function at another dcw; `command dcw ...` bypasses it.
dcw() {
  local _dcw_bin="${DCW_BIN:-@BIN@}" _dcw_dir _dcw_rc
  if [ "${1:-}" = start ] && [ -n "${2:-}" ]; then
    _dcw_dir=$("$_dcw_bin" path "$2" 2>/dev/null) || _dcw_dir=""
    "$_dcw_bin" "$@"; _dcw_rc=$?
    [ -n "$_dcw_dir" ] && [ -d "$_dcw_dir" ] && cd "$_dcw_dir"
    return "$_dcw_rc"
  fi
  "$_dcw_bin" "$@"
}
INTEG
)
    VERIFIED_FN=0
    for SHELL_RC in $(_rc_candidates_for "$LOGIN_SHELL"); do
      mkdir -p "$(dirname "$SHELL_RC")"
      if [ -f "$SHELL_RC" ] && grep -Fq "$INTEG_MARKER" "$SHELL_RC"; then
        if grep -Fq "DCW_BIN:-$DCW_BIN}" "$SHELL_RC"; then
          echo "✓ shell:    dcw function already declared in $SHELL_RC"
        else
          echo "⚠ shell:    $SHELL_RC has a dcw function pointing at another binary"
          echo "            remove the '$INTEG_MARKER' block and re-run to use $DCW_BIN"
          break
        fi
      else
        printf '\n%s\n%s\n' "$INTEG_MARKER" "$INTEG_BODY" >> "$SHELL_RC"
        echo "✓ shell:    dcw function added to $SHELL_RC"
      fi
      [ "$ASKED" = 1 ] || break
      if _login_shell_knows_dcw "$LOGIN_SHELL"; then
        echo "  ✓ verified: $LOGIN_SHELL loads dcw as a function"
        VERIFIED_FN=1; break
      fi
    done
    if [ "$ASKED" = 0 ]; then
      echo "  ⚠ could not query $LOGIN_SHELL — open a new terminal to confirm"
    elif [ "$VERIFIED_FN" = 0 ]; then
      echo "  ⚠ unverified: dcw is not a function in $LOGIN_SHELL"
    fi

    # ── venv helpers: source the file installed above ──
    # A second marker, checked independently: an older install already has the
    # dcw function block and must still pick this up on a re-run. Paths are left
    # as $HOME (not the resolved path) so the rc file stays portable.
    VENV_MARKER='# dcw venv helpers (added by installer)'
    VENV_BODY='# venvhmx: activate the project venv and cd into it.
# venvhere: activate the venv for the CURRENT directory, staying put (so it
# pairs with `dcw start <branch>`, which moves you into the worktree).
[ -f "$HOME/.config/dcw/shell/venv.sh" ] && . "$HOME/.config/dcw/shell/venv.sh"'
    if [ ! -f "$SHELL_SRC" ]; then
      echo "  ⚠ venv:     no shell/venv.sh to install — venvhmx/venvhere skipped"
    elif [ ! -f "$VENV_DEST" ]; then
      echo "  ⚠ venv:     $VENV_DEST missing — venvhmx/venvhere skipped"
    else
      VERIFIED_VENV=0
      for SHELL_RC in $(_rc_candidates_for "$LOGIN_SHELL"); do
        mkdir -p "$(dirname "$SHELL_RC")"
        if [ -f "$SHELL_RC" ] && grep -Fq "$VENV_MARKER" "$SHELL_RC"; then
          echo "✓ venv:     already sourced in $SHELL_RC"
        else
          printf '\n%s\n%s\n' "$VENV_MARKER" "$VENV_BODY" >> "$SHELL_RC"
          echo "✓ venv:     sourced in $SHELL_RC"
        fi
        [ "$ASKED" = 1 ] || break
        if _login_shell_has_function "$LOGIN_SHELL" venvhere; then
          echo "  ✓ verified: $LOGIN_SHELL loads venvhere + venvhmx"
          VERIFIED_VENV=1; break
        fi
      done
      if [ "$ASKED" = 0 ]; then
        echo "  ⚠ could not query $LOGIN_SHELL — open a new terminal to confirm"
      elif [ "$VERIFIED_VENV" = 0 ]; then
        echo "  ⚠ unverified: venvhere is not a function in $LOGIN_SHELL"
      fi
    fi
    ;;
  *)
    echo "⚠ shell:    no integration for $(basename "$LOGIN_SHELL") — cd \"\$(dcw path <branch>)\" after start"
    ;;
esac

# ── summary ───────────────────────────────────────────────────────
echo ""
echo "== summary =="
echo "  bin:        $BIN_DIR"
echo "  repo:       $REPO_PATH"
echo "  venv:       $VENV_PATH"
echo "  ports:      $PORT_BASE-$PORT_MAX"
echo "  shell:      $INTEG_NOTE"
if [ -f "$VENV_DEST" ]; then
  echo "  venv fns:   venvhmx + venvhere ($VENV_DEST)"
fi
echo ""
echo "Usage: dcw start <branch> | status | list | path <b> | logs <b> | stop <b> | abort <b>"
[ "$WARN" = 1 ] && echo "(review the warnings above)"
exit 0
