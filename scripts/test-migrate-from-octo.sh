#!/usr/bin/env bash
# Tests migrate-from-octo.sh in a throwaway folder with docker faked (CI runs it beside shellcheck).
set -uo pipefail
script="$(cd "$(dirname "$0")" && pwd)/migrate-from-octo.sh"
work="$(mktemp -d)"
trap 'rm -rf "$work"' EXIT
mkdir -p "$work/bin"
printf '#!/usr/bin/env bash\nprintf "%%s\\n" ${FAKE_RUNNING:-}\n' > "$work/bin/docker"
chmod +x "$work/bin/docker"
export PATH="$work/bin:$PATH"
failures=0
expect() { # description, command...
  local what="$1"; shift
  if "$@"; then echo "ok   $what"; else echo "FAIL $what"; failures=$((failures + 1)); fi
}
run() { bash "$script" --dir "$site" "$@"; }

site="$work/one"; mkdir -p "$site/octo-config"
cat > "$site/.env" <<'ENV'
OCTO_CONFIG_DIR=
OCTO_IMAGE_TAG=latest
RADIO_SONIC_URL=http://octo-sonic:8080
UPDATES_REPO=winters27/octo
LIBRARY_ACTIONS_TRASH_DIR=.octo-trash
LASTFM_API_KEY=hunter2secret
ENV
chmod 600 "$site/.env"
original="$(cat "$site/.env")"

out="$(run)"
expect "a dry run changes nothing" test "$(cat "$site/.env")" = "$original"
expect "a dry run makes no backup" test ! -e "$site/.env.octo-backup"
expect "a dry run names what changes" grep -q 'FLAKYFY_IMAGE_TAG' <<< "$out"
expect "no value is ever printed" bash -c '! grep -q hunter2secret <<< "$1"' _ "$out"

run --apply > /dev/null
expect "the default Octo config folder is kept" grep -qx 'FLAKYFY_CONFIG_DIR=./octo-config' "$site/.env"
expect "OCTO_ keys are renamed" grep -qx 'FLAKYFY_IMAGE_TAG=latest' "$site/.env"
expect "no OCTO_ key is left" bash -c '! grep -q "^OCTO_" "$1"' _ "$site/.env"
expect "the sonic address follows the service" grep -qx 'RADIO_SONIC_URL=http://flakyfy-sonic:8080' "$site/.env"
expect "the update check leaves winters27/octo" bash -c '! grep -q "^UPDATES_REPO" "$1"' _ "$site/.env"
expect "other settings stay" grep -qx 'LASTFM_API_KEY=hunter2secret' "$site/.env"
expect "the trash folder stays Octo's" grep -qx 'LIBRARY_ACTIONS_TRASH_DIR=.octo-trash' "$site/.env"
expect "the backup is the original" test "$(cat "$site/.env.octo-backup")" = "$original"
expect "the file stays private" test "$(stat -c %a "$site/.env")" = 600

once="$(cat "$site/.env")"
out="$(run --apply)"
expect "a second run changes nothing" test "$(cat "$site/.env")" = "$once"
expect "a second run says so" grep -q 'Nothing to change' <<< "$out"
expect "a second run keeps the original backup" test "$(cat "$site/.env.octo-backup")" = "$original"

site="$work/two"; mkdir -p "$site"
printf 'OCTO_CONFIG_DIR=/srv/octo\nFLAKYFY_IMAGE_TAG=latest\nOCTO_IMAGE_TAG=2026.10.22\n' > "$site/.env"
run --apply > /dev/null
expect "a custom config folder is kept" grep -qx 'FLAKYFY_CONFIG_DIR=/srv/octo' "$site/.env"
expect "a value already set for Flakyfy wins" test "$(grep -c '^FLAKYFY_IMAGE_TAG=' "$site/.env")" = 1
expect "the winning value is Flakyfy's" grep -qx 'FLAKYFY_IMAGE_TAG=latest' "$site/.env"

site="$work/three"; mkdir -p "$site"
printf 'OCTO_IMAGE_TAG=latest\n' > "$site/.env"
FAKE_RUNNING="navidrome octo" run --apply > /dev/null 2>&1; code=$?
expect "it refuses while octo runs" test "$code" -ne 0
expect "a refusal changes nothing" grep -qx 'OCTO_IMAGE_TAG=latest' "$site/.env"

site="$work/four"; mkdir -p "$site"
run > /dev/null 2>&1; code=$?
expect "no .env is an error" test "$code" -ne 0

[ "$failures" -eq 0 ] && echo "migrate-from-octo: all ok"
exit "$failures"
