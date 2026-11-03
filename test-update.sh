#!/usr/bin/env bash
# Hermetic update tests: no network, no writes to the real HOME or installed dcw.
set -uo pipefail
D=$(mktemp -d /tmp/dcw-update-test.XXXXXX)
trap 'rm -rf "$D"' EXIT
SRC=$(cd "$(dirname "$0")" && pwd)
PASS=0; FAIL=0
ok() { PASS=$((PASS+1)); printf '  PASS %s\n' "$1"; }
bad() { FAIL=$((FAIL+1)); printf '  FAIL %s\n' "$1"; }
is() { if [ "$2" = "$3" ]; then ok "$1"; else bad "$1: got [$2], want [$3]"; fi; }
run() { HOME="$D/home" DEV_CAPTAIN_REPO="$D/project" "$D/bin/dcw" update "$@" 2>&1; }
mkdir -p "$D/home/.config/dcw/shell" "$D/bin" "$D/project" "$D/remote.git" "$D/source"
git init -q --bare "$D/remote.git"
git init -q "$D/source"
git -C "$D/source" config user.name Test
git -C "$D/source" config user.email test@example.invalid
git -C "$D/source" remote add origin "$D/remote.git"
git -C "$D/source" checkout -q -b main
cp "$SRC/dcw" "$D/source/dcw"
cp "$SRC/install.sh" "$D/source/install.sh"
mkdir -p "$D/source/shell"
cp "$SRC/shell/venv.sh" "$D/source/shell/venv.sh"
git -C "$D/source" add .
git -C "$D/source" commit -qm initial
git -C "$D/source" push -q -u origin main
cp "$SRC/dcw" "$D/bin/dcw"
chmod +x "$D/bin/dcw"
printf '%s\n' "$D/source" > "$D/bin/.dcw-source"
printf '%s\n' '# local config preserved' > "$D/home/.config/dcw/config"
printf '%s\n' old-helper > "$D/home/.config/dcw/shell/venv.sh"

# Simulate a teammate's new push from another checkout.
git clone -q -b main "$D/remote.git" "$D/teammate"
git -C "$D/teammate" config user.name Test
git -C "$D/teammate" config user.email test@example.invalid
printf '\n# teammate update\n' >> "$D/teammate/dcw"
printf '\n# new helper\n' >> "$D/teammate/shell/venv.sh"
git -C "$D/teammate" add .
git -C "$D/teammate" commit -qm 'add teammate feature'
printf '\n# teammate fix\n' >> "$D/teammate/dcw"
git -C "$D/teammate" add dcw
git -C "$D/teammate" commit -qm 'fix teammate feature'
git -C "$D/teammate" push -q origin main
out=$(run); rc=$?
is 'pull succeeds' "$rc" 0
case "$out" in
  *'New commits (subjects):'*'add teammate feature'*'fix teammate feature'*) ok 'commit titles shown oldest first' ;;
  *) bad "commit titles missing or out of order: $out" ;;
esac
is 'source fast-forwarded' "$(git -C "$D/source" rev-parse HEAD)" "$(git -C "$D/teammate" rev-parse HEAD)"
is 'installed binary refreshed' "$(cmp -s "$D/source/dcw" "$D/bin/dcw" && echo yes)" yes
is 'helper refreshed' "$(cmp -s "$D/source/shell/venv.sh" "$D/home/.config/dcw/shell/venv.sh" && echo yes)" yes
is 'config unchanged' "$(< "$D/home/.config/dcw/config")" '# local config preserved'
# An up-to-date run must not reinstall either file, even if the local copies
# differ from the source (the tracked source HEAD is what decides freshness).
printf '\n# installed marker\n' >> "$D/bin/dcw"
printf '\n# helper marker\n' >> "$D/home/.config/dcw/shell/venv.sh"
out=$(run); rc=$?
is 'repeat update succeeds' "$rc" 0
case "$out" in
  *'dcw is already up to date. No changes needed.'*) ok 'no-change message shown in English' ;;
  *) bad "no-change message missing: $out" ;;
esac
is 'up-to-date run leaves binary untouched' "$(cmp -s "$D/source/dcw" "$D/bin/dcw" && echo replaced || echo untouched)" untouched
is 'up-to-date run leaves helper untouched' "$(cmp -s "$D/source/shell/venv.sh" "$D/home/.config/dcw/shell/venv.sh" && echo replaced || echo untouched)" untouched

printf '\n# local edit\n' >> "$D/source/dcw"
before=$(git -C "$D/source" rev-parse HEAD)
out=$(run); rc=$?
is 'dirty checkout rejected' "$rc" 1
is 'dirty checkout not overwritten' "$(git -C "$D/source" rev-parse HEAD)" "$before"
git -C "$D/source" restore dcw
mv "$D/bin/.dcw-source" "$D/bin/.dcw-source.hidden"
out=$(run); rc=$?
is 'missing source metadata rejected' "$rc" 1
mv "$D/bin/.dcw-source.hidden" "$D/bin/.dcw-source"
git -C "$D/source" branch --unset-upstream
out=$(run); rc=$?
is 'missing upstream rejected' "$rc" 1
out=$(run unexpected); rc=$?
is 'extra args rejected' "$rc" 2

printf 'update tests: %s passed, %s failed\n' "$PASS" "$FAIL"
[ "$FAIL" -eq 0 ]
