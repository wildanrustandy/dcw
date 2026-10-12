#!/usr/bin/env bash
# test-celery.sh — end-to-end test for dcw's per-branch Celery support.
#
# Creates a scratch branch with `dcw start`, proves the branch's worker is
# isolated from the base branch's Redis namespace, then cleans up with
# `dcw abort`. Needs a working HMX repo + venv + postgres (see ~/.config/dcw/config)
# and writes a real DB clone, so run it when a scratch branch is acceptable.
#
#     ./test-celery.sh              # default branch name dcw-celery-test
#     BR=my-scratch ./test-celery.sh
#
# What it asserts, and why these probes:
#   - control.ping() exercises broker publish + the worker's pidbox queue + the
#     reply. It needs no task registration: 'celery.ping' is NOT registered in
#     this app (sending it just raises NotRegistered, which proves nothing).
#   - a django cache write proves the cache is namespaced too.
#   - the same ping from the BASE namespace is the negative control: with no base
#     worker running, the branch worker must stay silent.
set -uo pipefail

BR="${BR:-dcw-celery-test}"
REPO="${REPO:-$HOME/repo}"
VENV="${VENV:-$HOME/.venv}"
DCW_SRC="${DCW_SRC:-$HOME/dcw/dcw}"
DCW_BIN="${DCW_BIN:-$HOME/.local/bin/dcw}"
STATE="${STATE:-$HOME/.dev-captain}"

WT="$REPO/.worktrees/$BR"
BASE_ENV="$REPO/hmx/.env"
WT_ENV="$WT/hmx/.env"
DB_NAME=$(printf '%s' "$BR" | tr '/[:upper:].-' '_[:lower:]__' | tr -cd '[:alnum:]_' | cut -c1-63)

# This test drives the INSTALLED dcw, which install.sh copies from the source.
# Editing the source without re-installing silently tests the old build.
if ! cmp -s "$DCW_SRC" "$DCW_BIN"; then
  echo "!! $DCW_BIN differs from $DCW_SRC — run ~/dcw/install.sh first" >&2
  exit 1
fi
echo "installed dcw matches source ($(sha256sum "$DCW_BIN" | cut -c1-12))"

scope_of() { python3 -c "import hashlib,sys;print(hashlib.sha256(sys.argv[1].encode()).hexdigest())" "$1"; }
BASE_NS=$(sed -n 's/^HMX_INSTANCE_NAMESPACE=//p' "$BASE_ENV")
BASE_SCOPE=$(scope_of "$BASE_NS")
EXPECT_NS="$BASE_NS-$DB_NAME"
WT_SCOPE=$(scope_of "$EXPECT_NS")

# Derive the DB coordinates from the repo's own .env instead of hardcoding this
# host's private cluster: the test ships with dcw and must work on another box.
env_val() { sed -n "s/^$1=//p" "$BASE_ENV" | tail -1; }
DB_HOST=$(env_val DATABASE_HOST); DB_PORT=$(env_val DATABASE_PORT)
DB_USER=$(env_val DATABASE_USER)
DB_HOST=${DB_HOST:-127.0.0.1}; DB_PORT=${DB_PORT:-5432}; DB_USER=${DB_USER:-$(id -un)}
# redis DB index from a URL like redis://host:6379/6 (last path segment)
redis_db() { printf '%s' "$1" | sed -E 's#.*/([0-9]+)/?$#\1#'; }
BROKER_DB=$(redis_db "$(env_val CELERY_BROKER_URL)")
RESULT_DB=$(redis_db "$(env_val CELERY_RESULT_BACKEND)")
CACHE_DB=$(redis_db "$(env_val REDIS_CACHE)")
BROKER_DB=${BROKER_DB:-0}; RESULT_DB=${RESULT_DB:-0}; CACHE_DB=${CACHE_DB:-0}

echo "base   ns/scope: $BASE_NS / ${BASE_SCOPE:0:16}..."
echo "branch ns/scope: $EXPECT_NS / ${WT_SCOPE:0:16}..."
echo "db     $DB_USER@$DB_HOST:$DB_PORT   redis broker/result/cache = db$BROKER_DB/db$RESULT_DB/db$CACHE_DB"
echo

echo "############ 0. dcw help must be INERT (unquoted heredocs execute backticks) ############"
# The help text lives in an unquoted `cat <<HELP` heredoc so $VARS expand. Any
# backtick inside it runs as a command — which once made `dcw help` spawn a worker.
help_out=$(dcw help 2>&1)
case "$help_out" in
  *"command not found"*) echo "  FAIL  dcw help executed a stray command" ;;
  *CELERY*URL*)          echo "  FAIL  dcw help ran 'dcw status' and inlined its output" ;;
  *) echo "  ok    dcw help is inert" ;;
esac
case "$help_out" in
  *"dcw celery start"*) echo "  ok    help documents the celery subcommand" ;;
  *) echo "  FAIL  help is missing the celery subcommand" ;;
esac

# Static guard for the same class of bug: a backtick inside an UNQUOTED heredoc
# body is command-substituted by the shell, not printed.
python3 - "$DCW_SRC" <<'PY'
import re, sys
lines = open(sys.argv[1]).read().split("\n")
i, bad = 0, []
while i < len(lines):
    m = re.search(r"<<(\'?)([A-Z_]+)\1", lines[i])
    if m and m.group(1) == "":              # unquoted -> shell expands the body
        delim, j = m.group(2), i + 1
        while j < len(lines) and lines[j].strip() != delim:
            if "`" in lines[j]:
                bad.append((j + 1, lines[j].strip()))
            j += 1
        i = j
    i += 1
if bad:
    print("  FAIL  backtick inside an unquoted heredoc (runs as a command):")
    for n, l in bad:
        print(f"        line {n}: {l}")
else:
    print("  ok    no backticks in unquoted heredocs")
PY
echo

echo "############ 1. dcw start $BR ############"
dcw start "$BR" 2>&1 | grep -vE 'Updating files|^From |new branch|^   [0-9a-f]+\.\.|^ \* '
echo

echo "############ 1b. wait for the worker to be READY ############"
# `dcw start` deliberately does not block on the worker (HMX imports the whole
# module registry, ~15-30s), so wait for readiness before probing it.
n=0
while [ "$n" -lt 120 ]; do
  if dcw celery status 2>/dev/null | grep -q ' ready '; then break; fi
  sleep 1; n=$((n+1))
done
dcw celery status
echo "  (waited ${n}s)"
echo

echo "############ 2. worktree .env ############"
echo "  DATABASE_NAME          = $(sed -n 's/^DATABASE_NAME=//p' "$WT_ENV")"
echo "  HMX_INSTANCE_NAMESPACE = $(sed -n 's/^HMX_INSTANCE_NAMESPACE=//p' "$WT_ENV")"
echo "  base .env namespace    = $(sed -n 's/^HMX_INSTANCE_NAMESPACE=//p' "$BASE_ENV")  <- must be unchanged"
echo

echo "############ 3. worker process ############"
WPID=$(pgrep -u "$(id -un)" -f "dcw-$BR@" | head -1)
ps -o pid,etime,cmd -p "$WPID" 2>/dev/null | tail -1 | sed 's/^/  /'
echo "  cwd: $(readlink "/proc/$WPID/cwd" 2>/dev/null || echo '?')"
echo

echo "############ 4. redis broker bindings (settle 4s so the worker binds) ############"
sleep 4
echo "  branch scope keys: $(redis-cli -n "$BROKER_DB" --scan --pattern "hmx:$WT_SCOPE:*" 2>/dev/null | wc -l)"
redis-cli -n "$BROKER_DB" --scan --pattern "hmx:$WT_SCOPE:*" 2>/dev/null | sed "s/hmx:$WT_SCOPE://" | sed 's/^/    /' | sort
echo "  base scope keys:   $(redis-cli -n "$BROKER_DB" --scan --pattern "hmx:$BASE_SCOPE:*" 2>/dev/null | wc -l)"
echo

echo "############ 5. POSITIVE: broker round trip from the BRANCH namespace ############"
(
  cd "$WT/hmx" || exit 1
  "$VENV/bin/python" - <<'PY'
import os
os.environ.setdefault("DJANGO_SETTINGS_MODULE", "hmx.settings")
from hmx.celery import app
from django.core.cache import cache
print("  broker prefix:", app.conf.broker_transport_options.get("global_keyprefix"))
print("  cache prefix :", app.conf.get("CACHES", {}).get("default", {}).get("KEY_PREFIX"))
reply = app.control.ping(timeout=8)
print("  control.ping :", reply if reply else "NO REPLY")
cache.set("dcw-probe", "branch-value", 600)
print("  cache write  : ok (TTL 600s)")
PY
)
echo

echo "############ 6. NEGATIVE: same ping from the BASE namespace ############"
echo "  (no base worker runs, so the branch worker must NOT answer)"
(
  cd "$REPO/hmx" || exit 1
  "$VENV/bin/python" - <<'PY'
import os
os.environ.setdefault("DJANGO_SETTINGS_MODULE", "hmx.settings")
from hmx.celery import app
print("  broker prefix:", app.conf.broker_transport_options.get("global_keyprefix"))
reply = app.control.ping(timeout=6)
print("  control.ping :", reply if reply else "NO REPLY  <- isolation holds")
PY
)
echo

echo "############ 7. every Redis component is namespaced ############"
echo "  db$BROKER_DB (CELERY_BROKER_URL) branch bindings : $(redis-cli -n "$BROKER_DB" --scan --pattern "hmx:$WT_SCOPE:*" 2>/dev/null | wc -l)"
echo "  db$RESULT_DB (CELERY_RESULT_BACKEND) branch keys : $(redis-cli -n "$RESULT_DB" --scan --pattern "hmx:$WT_SCOPE:*" 2>/dev/null | wc -l)"
echo "  db$CACHE_DB (REDIS_CACHE) branch keys           : $(redis-cli -n "$CACHE_DB" --scan --pattern "hmx:$WT_SCOPE:*" 2>/dev/null | wc -l)"
echo "  the dcw-probe cache key, branch scope (db$CACHE_DB):"
redis-cli -n "$CACHE_DB" --scan --pattern "hmx:$WT_SCOPE:*dcw-probe*" 2>/dev/null | sed 's/^/    /'
echo "  same key in the BASE scope (must be empty):"
redis-cli -n "$CACHE_DB" --scan --pattern "hmx:$BASE_SCOPE:*dcw-probe*" 2>/dev/null | sed 's/^/    /'
echo

echo "############ 8. dcw status / dcw celery status ############"
dcw status
echo
dcw celery status
echo

echo "############ 9. idempotency ############"
dcw celery start "$BR"
dcw start "$BR" 2>&1 | grep -E 'already|celery:'
echo

echo "############ 10. dcw stop (server + worker) ############"
dcw stop "$BR"
sleep 2
echo "  celery procs : $(pgrep -u "$(id -un)" -x celery | wc -l)"
echo

echo "############ 11. dcw abort (full cleanup) ############"
echo y | dcw abort "$BR"
echo
echo "  worktrees      : $(ls -A "$REPO/.worktrees" 2>/dev/null | wc -l)"
echo "  branch exists  : $(git -C "$REPO" branch --list "$BR" | wc -l)"
echo "  DB exists      : $(psql -h "$DB_HOST" -p "$DB_PORT" -U "$DB_USER" -d postgres -Atc "select count(*) from pg_database where datname='$DB_NAME'" 2>/dev/null)"
echo "  pidfiles       : $(ls -A "$STATE/celery" 2>/dev/null | wc -l)"
echo "  celery procs   : $(pgrep -u "$(id -un)" -x celery | wc -l)"
echo "  base .env ns   : $(sed -n 's/^HMX_INSTANCE_NAMESPACE=//p' "$BASE_ENV")"
echo "  branch bindings left in redis (expire on their own): $(redis-cli -n "$BROKER_DB" --scan --pattern "hmx:$WT_SCOPE:*" 2>/dev/null | wc -l)"
