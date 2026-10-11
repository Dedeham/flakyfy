#!/usr/bin/env bash
# One command: generate the library, start Navidrome + Octo, seed, run the tests.
#   ./run.sh            run everything, then tear the stack down (volumes included)
#   ./run.sh --keep     leave the stack running afterwards
#   ./run.sh up         only start and seed
#   ./run.sh down       tear down
set -euo pipefail
cd "$(dirname "$0")"

export HARNESS_PROJECT="${HARNESS_PROJECT:-fla12-it}"
export HARNESS_NAVIDROME_URL="http://127.0.0.1:${HARNESS_NAVIDROME_PORT:-14533}"
export HARNESS_OCTO_URL="http://127.0.0.1:${HARNESS_OCTO_PORT:-15274}"
export HARNESS_OCTO_YT_URL="http://127.0.0.1:${HARNESS_OCTO_YT_PORT:-15275}"
export HARNESS_STUB_URL="http://127.0.0.1:${HARNESS_STUB_PORT:-18080}"
compose=(docker compose -f docker-compose.yml)

down() { "${compose[@]}" down -v --remove-orphans; rm -rf .data; }

up() {
  rm -rf .data && python3 seed/generate_library.py .data/music
  "${compose[@]}" up -d --build
  python3 seed/seed.py
  python3 - <<'PY'
import harness as h
h.wait_for(lambda: h.http_ok(h.OCTO + "/rest/ping?u=x&p=x&v=1.16.1&c=probe&f=json"), "octo")
h.wait_for(lambda: h.http_ok(h.OCTO_YT + "/rest/ping?u=x&p=x&v=1.16.1&c=probe&f=json"), "octo-yt")
h.wait_for(lambda: h.http_ok(h.STUB + "/__log"), "stub")
PY
}

case "${1:-}" in
  down) down ;;
  up) up ;;
  *)
    keep=0; [ "${1:-}" = "--keep" ] && keep=1
    # On failure, print the container logs before teardown removes them.
    finish() {
      rc=$?
      [ "$rc" = 0 ] || "${compose[@]}" logs --no-color --tail 200 || true
      down
      exit "$rc"
    }
    [ "$keep" = 1 ] || trap finish EXIT
    up
    python3 -m unittest discover -v -s . -p 'test_*.py'
    ;;
esac
