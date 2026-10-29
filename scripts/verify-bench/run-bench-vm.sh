#!/usr/bin/env bash
# Runs the verify bench on the Docker host, with every request it makes (the shim's YouTube
# lookups and downloads) inside a VPN container's network namespace. See README.md.
#
#   VPN_CONTAINER=<gluetun container> ./run-bench-vm.sh [verify_bench flags, e.g. --repeat 1]
#   ./run-bench-vm.sh --offline [--only same_recording]     # no network at all
#
# Run from the bench folder (cases.json, corpus/, verify_bench). Optional:
#   OCTO_IMAGE   image whose ffmpeg and fpcalc run the bench (default: the running octo's)
#   SHIM_IMAGE   shim image to ask (default: the running yt-dlp-shim's)
#   SHIM_PORT    port the throwaway shim listens on inside the namespace (default 18080)
set -euo pipefail
BENCH="$(cd "$(dirname "$0")" && pwd)"
CURL="curlimages/curl:8.10.1"
SHIM_PORT="${SHIM_PORT:-18080}"
image_of() { docker inspect -f '{{.Config.Image}}' "$1" 2>/dev/null; }
OCTO_IMAGE="${OCTO_IMAGE:-$(image_of octo || true)}"
SHIM_IMAGE="${SHIM_IMAGE:-$(image_of yt-dlp-shim || true)}"
[ -n "$OCTO_IMAGE" ] || { echo "FAIL set OCTO_IMAGE (no running octo container)"; exit 1; }
[ -x "$BENCH/verify_bench" ] || { echo "FAIL $BENCH/verify_bench is missing or not executable"; exit 1; }
[ -f "$BENCH/cases.json" ] || { echo "FAIL $BENCH/cases.json is missing"; exit 1; }

bench() { # network, flags...
  local network="$1"; shift
  docker run --rm --network "$network" --user "$(id -u):$(id -g)" \
    --cap-drop ALL --security-opt no-new-privileges \
    -v "$BENCH:/bench" --entrypoint /bench/verify_bench "$OCTO_IMAGE" \
    --cases /bench/cases.json --out /bench/report.json "$@"
}

if [ "${1:-}" = "--offline" ]; then
  shift
  echo "Offline run: no network inside the container"
  bench none "$@"
  exit $?
fi

VPN="${VPN_CONTAINER:?set VPN_CONTAINER to the gluetun container the bench runs behind}"
[ -n "$SHIM_IMAGE" ] || { echo "FAIL set SHIM_IMAGE (no running yt-dlp-shim container)"; exit 1; }
in_vpn() { docker run --rm --network "container:$VPN" "$CURL" "$@"; }

# Egress guard: the namespace must leave through the VPN, and resolve through Gluetun.
guard() {
  local wan vpn ns
  wan="$(curl -4 -fsS --max-time 15 https://ifconfig.me/ip || true)"
  vpn="$(in_vpn -4 -fsS --max-time 15 https://ifconfig.me/ip 2>/dev/null || true)"
  if [ -z "$vpn" ]; then echo "FAIL no egress through $VPN"; return 1; fi
  if [ -z "$wan" ] || [ "$vpn" = "$wan" ]; then echo "FAIL $VPN does not leave through the VPN"; return 1; fi
  ns="$(docker exec "$VPN" sh -c 'grep -m1 ^nameserver /etc/resolv.conf' | awk '{print $2}')"
  case "$ns" in 127.0.0.1|::1) ;; *) echo "FAIL DNS in $VPN goes to $ns"; return 1;; esac
  if in_vpn -6 -fsS --max-time 10 https://api6.ipify.org >/dev/null 2>&1; then
    echo "FAIL IPv6 leaves $VPN"; return 1
  fi
  echo "OK   egress through the VPN, DNS through Gluetun, no IPv6"
}

cleanup() { docker rm -f bench-shim >/dev/null 2>&1 || true; }
trap cleanup EXIT

guard
cleanup
docker run -d --name bench-shim --network "container:$VPN" \
  --cap-drop ALL --security-opt no-new-privileges \
  -e PORT="$SHIM_PORT" "$SHIM_IMAGE" >/dev/null
echo "Waiting for the throwaway shim on :$SHIM_PORT inside $VPN"
for _ in $(seq 1 60); do
  in_vpn -fsS --max-time 5 "http://127.0.0.1:$SHIM_PORT/health" >/dev/null 2>&1 && break
  sleep 2
done
in_vpn -fsS --max-time 5 "http://127.0.0.1:$SHIM_PORT/health" >/dev/null \
  || { echo "FAIL the shim did not come up"; docker logs --tail 30 bench-shim; exit 1; }

status=0
bench "container:$VPN" --shim "http://127.0.0.1:$SHIM_PORT" "$@" || status=$?
guard || status=1
echo "Now run the stack's leak test and its journal: ./leak-test.sh && journalctl -u leak -n 50"
exit "$status"
