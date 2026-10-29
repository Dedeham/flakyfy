#!/usr/bin/env bash
# Collects each bench song's YouTube preview once, the way a check asks for it, with a throwaway
# shim. That is the bench's only network step; `verify_bench replay` then runs offline.
# See README.md next to this script.
#
#   ./collect-previews.sh [bench folder, default <checkout>/bench-data]
#
# By default the shim runs on this machine and listens on 127.0.0.1 only. With VPN_CONTAINER set
# (a Gluetun container), the shim and the bench run in its network namespace instead, and the
# egress, DNS and IPv6 are checked before and after.
#   SHIM_IMAGE   default the shim of this checkout's release (VERSION)
#   SHIM_PORT    default 18080
#   BENCH_BIN    default <bench folder>/verify_bench
set -euo pipefail
HERE="$(cd "$(dirname "$0")" && pwd)"
BENCH="$(cd "${1:-$HERE/../../bench-data}" && pwd)"
RELEASE="$(cat "$HERE/../../VERSION")"
SHIM_IMAGE="${SHIM_IMAGE:-ghcr.io/winters27/octo-yt-dlp-shim:$RELEASE}"
SHIM_PORT="${SHIM_PORT:-18080}"
BIN="${BENCH_BIN:-$BENCH/verify_bench}"
CURL="curlimages/curl:8.10.1"
[ -x "$BIN" ] || { echo "FAIL $BIN is missing or not executable"; exit 1; }
[ -f "$BENCH/requests.json" ] || { echo "FAIL $BENCH/requests.json is missing (run make_cases.py)"; exit 1; }

cleanup() { docker rm -f bench-shim >/dev/null 2>&1 || true; }
trap cleanup EXIT
cleanup

if [ -z "${VPN_CONTAINER:-}" ]; then
  docker run -d --name bench-shim -p "127.0.0.1:$SHIM_PORT:8080" \
    --cap-drop ALL --security-opt no-new-privileges "$SHIM_IMAGE" >/dev/null
  echo "Waiting for the throwaway shim on 127.0.0.1:$SHIM_PORT"
  for _ in $(seq 1 60); do
    curl -fsS --max-time 5 "http://127.0.0.1:$SHIM_PORT/health" >/dev/null 2>&1 && break
    sleep 2
  done
  curl -fsS --max-time 5 "http://127.0.0.1:$SHIM_PORT/health" >/dev/null \
    || { echo "FAIL the shim did not come up"; docker logs --tail 30 bench-shim; exit 1; }
  "$BIN" collect --bench "$BENCH" --shim "http://127.0.0.1:$SHIM_PORT"
  exit $?
fi

in_vpn() { docker run --rm --network "container:$VPN_CONTAINER" "$CURL" "$@"; }
guard() {
  local wan vpn ns
  wan="$(curl -4 -fsS --max-time 15 https://ifconfig.me/ip || true)"
  vpn="$(in_vpn -4 -fsS --max-time 15 https://ifconfig.me/ip 2>/dev/null || true)"
  if [ -z "$vpn" ]; then echo "FAIL no egress through $VPN_CONTAINER"; return 1; fi
  if [ -z "$wan" ] || [ "$vpn" = "$wan" ]; then echo "FAIL $VPN_CONTAINER does not leave through the VPN"; return 1; fi
  ns="$(docker exec "$VPN_CONTAINER" sh -c 'grep -m1 ^nameserver /etc/resolv.conf' | awk '{print $2}')"
  case "$ns" in 127.0.0.1|::1) ;; *) echo "FAIL DNS in $VPN_CONTAINER goes to $ns"; return 1;; esac
  if in_vpn -6 -fsS --max-time 10 https://api6.ipify.org >/dev/null 2>&1; then
    echo "FAIL IPv6 leaves $VPN_CONTAINER"; return 1
  fi
  echo "OK   egress through the VPN, DNS through Gluetun, no IPv6"
}

guard
docker run -d --name bench-shim --network "container:$VPN_CONTAINER" \
  --cap-drop ALL --security-opt no-new-privileges -e PORT="$SHIM_PORT" "$SHIM_IMAGE" >/dev/null
for _ in $(seq 1 60); do
  in_vpn -fsS --max-time 5 "http://127.0.0.1:$SHIM_PORT/health" >/dev/null 2>&1 && break
  sleep 2
done
status=0
docker run --rm --network "container:$VPN_CONTAINER" --user "$(id -u):$(id -g)" \
  --cap-drop ALL --security-opt no-new-privileges -v "$BENCH:/bench" \
  --entrypoint /bench/verify_bench "${OCTO_IMAGE:-ghcr.io/winters27/octo:$RELEASE}" \
  collect --bench /bench --shim "http://127.0.0.1:$SHIM_PORT" || status=$?
guard || status=1
exit "$status"
