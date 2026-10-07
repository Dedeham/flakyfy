#!/usr/bin/env bash
# The box side of parity/lxc131.sh: build the Rust image from this checkout, record the corpus
# against it and against the C# release, and print the diffs per backlog group. Runs on the
# test box (LXC 131) from /opt/octo-parity/runs/<name>/src; lxc131.sh sends the code and calls
# it. Everything it makes is its own: the folder /opt/octo-parity, compose projects named
# octo-parity-*, the image octo-rust-parity:<name> and the buildx builder octo-parity. It never
# touches the starter rig (/opt/octo, project octo) or other images.
#
#   box-run.sh NAME [--no-build] [--csharp-only] [--only GLOB] [--bytes] [--csharp-twice] [--rust-twice]
#
# One run at a time (a lock), since a stack of two Octos, Navidrome and the stubs takes about
# 2 GB and the box has 6. The C# recording is kept per corpus version (parity.py digest), so it
# is made again only when the corpus, stubs, fixtures or compose file change.
set -euo pipefail

name="$1"
shift
build=1 only="" bytes=0 cs_twice=0 rs_twice=0 rust=1
while [ $# -gt 0 ]; do
  case "$1" in
    --no-build) build=0 ;;
    --csharp-only) build=0; rust=0 ;;
    --only) only="$2"; shift ;;
    --bytes) bytes=1 ;;
    --csharp-twice) cs_twice=1 ;;
    --rust-twice) rs_twice=1 ;;
    *) echo "unknown option $1" >&2; exit 64 ;;
  esac
  shift
done

ROOT=/opt/octo-parity
RUN="$ROOT/runs/$name"
SRC="$RUN/src"
OUT="$RUN/out"
CS_IMAGE=ghcr.io/winters27/octo:2026.10.05
RS_IMAGE="octo-rust-parity:$name"
BUILDER=octo-parity
MIN_FREE_GB=8

cd "$SRC"
exec 9> "$ROOT/run.lock"
if ! flock -n 9; then
  echo "another parity run is going on the box; waiting for it"
  flock 9
fi

free_gb() { df -BG --output=avail / | tail -n 1 | tr -dc '0-9'; }
say() { printf '\n== %s\n' "$*"; }

# ---- Disk: at least 8 GB free before anything is built or pulled.
if [ "$(free_gb)" -lt "$MIN_FREE_GB" ]; then
  say "only $(free_gb) GB free: trimming the parity builder's cache"
  docker buildx prune --builder "$BUILDER" -f --keep-storage 3gb > /dev/null 2>&1 || true
fi
if [ "$(free_gb)" -lt "$MIN_FREE_GB" ]; then
  echo "only $(free_gb) GB free on the box (need $MIN_FREE_GB); remove old parity images first:" >&2
  docker images --format '{{.Repository}}:{{.Tag}} {{.Size}}' | grep '^octo-rust-parity:' >&2 || true
  exit 1
fi

# ---- The Rust image, built from this checkout.
if [ "$build" = 1 ]; then
  if ! docker buildx inspect "$BUILDER" > /dev/null 2>&1; then
    # Its own builder, capped below the box's memory (the LTO link takes about 3 GB), with its
    # own cache, so a rebuild after a change in the Octo crates skips the dependencies.
    docker buildx create --name "$BUILDER" --driver docker-container \
      --driver-opt memory=3500m --driver-opt memory-swap=3500m > /dev/null
  fi
  old_id="$(docker image inspect -f '{{.Id}}' "$RS_IMAGE" 2> /dev/null || true)"
  say "building $RS_IMAGE from $(git rev-parse --short HEAD)"
  start=$(date +%s)
  docker buildx build --builder "$BUILDER" --progress plain -f Dockerfile.rust \
    --build-arg CARGO_BUILD_JOBS=3 -t "$RS_IMAGE" --load . > "$RUN/build.log" 2>&1 || {
    tail -n 40 "$RUN/build.log"
    echo "the Rust image did not build; the whole log is $RUN/build.log on the box" >&2
    exit 1
  }
  echo "built in $(($(date +%s) - start)) s"
  new_id="$(docker image inspect -f '{{.Id}}' "$RS_IMAGE")"
  if [ -n "$old_id" ] && [ "$old_id" != "$new_id" ]; then
    docker rmi "$old_id" > /dev/null 2>&1 || true
  fi
  # Keep the builder's cache from growing without end (dependencies stay; old Octo layers go),
  # and stop the builder: its daemon holds memory the stacks below need.
  docker buildx prune --builder "$BUILDER" -f --keep-storage 6gb > /dev/null 2>&1 || true
  docker buildx stop "$BUILDER" > /dev/null 2>&1 || true
elif [ "$rust" = 1 ] && ! docker image inspect "$RS_IMAGE" > /dev/null 2>&1; then
  echo "no $RS_IMAGE on the box yet; run without --no-build" >&2
  exit 1
fi

docker image inspect "$CS_IMAGE" > /dev/null 2>&1 || docker pull -q "$CS_IMAGE" > /dev/null

record() { # project port nd-port image out
  rm -rf "$5"
  local status=0
  python3 parity/parity.py run --project "$1" --port "$2" --nd-port "$3" --image "$4" \
    --out "$5" ${only:+--only "$only"} > "$5.log" 2>&1 || status=$?
  # 2 = some requests got no answer at all (the diff shows them); anything else is a failure.
  if [ "$status" != 0 ] && [ "$status" != 2 ]; then
    tail -n 30 "$5.log"
    echo "recording with $4 failed (exit $status); log $5.log on the box" >&2
    exit 1
  fi
  grep -E 'transport errors|WAIT|SKIP' "$5.log" | sed 's/^/  /' | tail -n 15 || true
}

mkdir -p "$OUT" "$ROOT/cs-cache"
digest="$(python3 parity/parity.py digest)"
cs_dir="$ROOT/cs-cache/${digest:0:16}${only:+-only}"
if [ -n "$only" ] || [ ! -f "$cs_dir/meta.json" ]; then
  say "recording C# 2026.10.05 (corpus ${digest:0:16})"
  record octo-parity-cs 18480 18453 "$CS_IMAGE" "$cs_dir"
else
  say "C# 2026.10.05 recording for corpus ${digest:0:16} is cached"
fi

if [ "$rust" = 1 ]; then
  say "recording the Rust build"
  record octo-parity-rs 18580 18553 "$RS_IMAGE" "$OUT/rs"
fi

diff_run() { # label a b report
  local mode=structural
  [ "${5:-}" = bytes ] && mode=bytes
  python3 parity/parity.py diff --mode "$mode" "$2" "$3" --report "$4" --groups-out "${4%.txt}.groups.json" \
    ${only:+--only "$only"} --summary > "$4.summary" || true
  say "$1 ($mode)"
  cat "$4.summary"
}

if [ "$cs_twice" = 1 ]; then
  record octo-parity-cs 18480 18453 "$CS_IMAGE" "$OUT/cs-again"
  diff_run "C# against C#" "$cs_dir" "$OUT/cs-again" "$RUN/cs-cs.txt"
  diff_run "C# against C#" "$cs_dir" "$OUT/cs-again" "$RUN/cs-cs-bytes.txt" bytes
fi
if [ "$rs_twice" = 1 ] && [ "$rust" = 1 ]; then
  record octo-parity-rs 18580 18553 "$RS_IMAGE" "$OUT/rs-again"
  diff_run "Rust against Rust" "$OUT/rs" "$OUT/rs-again" "$RUN/rs-rs.txt"
  diff_run "Rust against Rust" "$OUT/rs" "$OUT/rs-again" "$RUN/rs-rs-bytes.txt" bytes
fi
if [ "$rust" = 1 ]; then
  diff_run "C# against Rust" "$cs_dir" "$OUT/rs" "$RUN/report.txt"
fi
if [ "$bytes" = 1 ] && [ "$rust" = 1 ]; then
  diff_run "C# against Rust" "$cs_dir" "$OUT/rs" "$RUN/report-bytes.txt" bytes
fi

say "box: $(free_gb) GB free; reports in $RUN; C# recording $cs_dir"
