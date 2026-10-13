# venv helpers shipped with dcw — sourced from the rc file by install.sh.
#
#   venvhmx    activate the project venv and cd into the project
#   venvhere   activate the venv that belongs to the CURRENT directory, staying put
#
# Paths come from the dcw config (~/.config/dcw/config), so these follow
# DEV_CAPTAIN_REPO / DEV_CAPTAIN_VENV instead of hardcoding $HOME/repo. With no
# config they fall back to those same defaults.
#
# bash + zsh only (uses [[ ]]-free POSIX tests but `local` and `deactivate`);
# not meant for POSIX sh. Sourced from ~/.zshenv / ~/.bashrc, so it must stay
# cheap: nothing here runs at startup except the function definitions.

_dcw_conf() { # print one value from the dcw config (subshell: no env pollution)
    local cfg="${DCW_CONFIG:-$HOME/.config/dcw/config}"
    [ -r "$cfg" ] || return 1
    ( . "$cfg"; eval "printf '%s' \"\${$1:-}\"" )
}

_dcw_venv_default() { # the venv dcw itself uses (config value, else ~/.venv)
    local v
    v=$(_dcw_conf DEV_CAPTAIN_VENV)
    printf '%s' "${v:-$HOME/.venv}"
}

_dcw_repo_default() { # the project dcw itself uses (config value, else ~/repo)
    local p
    p=$(_dcw_conf DEV_CAPTAIN_REPO)
    printf '%s' "${p:-$HOME/repo}"
}

_dcw_venv_activate() { # $1 = venv dir, $2 = caller name (for the error message)
    local venv="$1" who="$2"
    if [ -z "$venv" ] || [ ! -f "$venv/bin/activate" ]; then
        echo "$who: venv tidak ditemukan (${venv:-tidak ada kandidat})" >&2
        return 1
    fi
    # Release an active venv first — even the same one. The venv `activate`
    # script prepends to PATH unconditionally and saves the PATH it saw, so a
    # second activation without deactivating leaves two identical entries and a
    # deactivate that restores the doubled PATH.
    if [ -n "${VIRTUAL_ENV:-}" ]; then
        deactivate 2>/dev/null
    fi
    . "$venv/bin/activate" || return 1
}

venvhmx() { # activate the project venv and cd into the project
    local venv repo
    venv=$(_dcw_venv_default)
    repo=$(_dcw_repo_default)
    _dcw_venv_activate "$venv" venvhmx || return 1
    if [ -d "$repo" ]; then
        cd "$repo" || return 1
    else
        echo "venvhmx: folder proyek tidak ditemukan di $repo" >&2
        return 1
    fi
    echo "venvhmx: aktif -> $(python --version 2>&1) @ $PWD"
}

venvhere() { # activate the venv for the CURRENT directory, without moving
    local venv="${1:-}" root cand
    if [ -z "$venv" ]; then
        # A .venv inside the worktree/project wins over the shared one, so a
        # branch can carry its own venv without every other branch seeing it.
        root=$(git rev-parse --show-toplevel 2>/dev/null)
        for cand in "$PWD/.venv" ${root:+"$root/.venv"} "$(_dcw_venv_default)"; do
            if [ -f "$cand/bin/activate" ]; then venv="$cand"; break; fi
        done
    fi
    _dcw_venv_activate "$venv" venvhere || return 1
    echo "venvhere: aktif -> $(python --version 2>&1) @ $PWD"
}
