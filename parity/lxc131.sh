#!/usr/bin/env bash
# Run the parity harness on the test box (LXC 131) against the Rust build of the commit you have
# checked out, and print the diffs per backlog group. For port agents; one command:
#
#   parity/lxc131.sh                      # build here, record, compare with C# 2026.10.05
#   parity/lxc131.sh --only '13-*'        # just some corpus files (the C# side is recorded again)
#   parity/lxc131.sh --no-build           # same image as last time (corpus or stub changes only)
#   parity/lxc131.sh --box-build          # compile on the box instead (Dockerfile.rust whole)
#   parity/lxc131.sh --box-build --low-memory   # that, with one job and no LTO
#   parity/lxc131.sh --bytes              # also the bytes-mode diff
#   parity/lxc131.sh --rust-twice         # also Rust against Rust (the determinism gate)
#   parity/lxc131.sh --csharp-twice       # also C# against C# (after a corpus change)
#   parity/lxc131.sh --csharp-only --csharp-twice   # corpus work: no Rust build or run
#   parity/lxc131.sh --clean              # remove this name's image and folder on the box
#
# What it does: compiles octo here as a static musl binary (cargo build --release --target
# x86_64-unknown-linux-musl; `rustup target add x86_64-unknown-linux-musl` and musl-tools once),
# bundles HEAD (only the commits the box lacks), copies the bundle and the binary to the box
# through the Proxmox host, checks it out in /opt/octo-parity/runs/<name>/src, and runs
# parity/box-run.sh there (the image from Dockerfile.rust's runtime stage around that binary,
# the corpus against both builds, the diff).
#
# Compiling here is the default because the box's compile is what hurts the shared Proxmox
# host: the LTO link takes about 3 GB, and on 2026-10-07 a box build tripped the watchdog with
# the host at 1 GB and its swap full. The runtime stage is only Debian packages and a copy.
# A static musl binary runs on bookworm-slim whatever glibc the machine that built it has
# (a WSL glibc build needs GLIBC_2.38 and does not start there). --box-build is the old way. The full report comes back to parity/.run/lxc131-<name>/. Commit first: only commits
# travel. Nothing is pushed anywhere and nothing on the box outside /opt/octo-parity, the
# octo-parity-* compose projects, the octo-rust-parity:<name> image and the octo-parity buildx
# builder is touched.
#
# Settings (environment): PARITY_NAME (default: the branch name), PARITY_PROXMOX (ssh host of
# the Proxmox node, default pmx), PARITY_CT (container id, default 131), PARITY_MIN_HOST_MB (the
# Proxmox host's free memory needed to start, default 3500). From WSL, where `pmx` is only in
# the Windows ssh config, the Windows ssh.exe/scp.exe are used automatically.
#
# The Proxmox host is shared (LXC 111 runs Brandon's own Octo there) and has little memory to
# spare: on 2026-10-07 a run with the host already short pushed it into swap until nothing
# answered. So the run starts only with PARITY_MIN_HOST_MB free on the host, and a watchdog
# stops it (and takes the parity stacks down) when the host's free memory drops under 1.5 GB.
set -euo pipefail

repo="$(git rev-parse --show-toplevel)"
cd "$repo"
host="${PARITY_PROXMOX:-pmx}"
ct="${PARITY_CT:-131}"
branch="$(git rev-parse --abbrev-ref HEAD)"
name="${PARITY_NAME:-$branch}"
name="$(printf '%s' "$name" | tr -c 'A-Za-z0-9._-' '-' | tr 'A-Z' 'a-z')"
args=()
clean=0
prebuilt=1
for a in "$@"; do
  case "$a" in
    --clean) clean=1 ;;
    --box-build) prebuilt=0 ;;
    --no-build | --csharp-only) prebuilt=0; args+=("$a") ;;
    *) args+=("$a") ;;
  esac
done
if [ "$prebuilt" = 1 ]; then
  for a in "${args[@]+"${args[@]}"}"; do
    if [ "$a" = --low-memory ]; then
      echo "--low-memory is for a box build; add --box-build" >&2
      exit 64
    fi
  done
fi

# ssh/scp that know the host: the plain ones, or Windows' from WSL.
SSH=ssh SCP=scp
if ! ssh -o BatchMode=yes -o ConnectTimeout=5 "$host" true 2> /dev/null; then
  if command -v ssh.exe > /dev/null && ssh.exe -o BatchMode=yes -o ConnectTimeout=5 "$host" true 2> /dev/null; then
    SSH=ssh.exe SCP=scp.exe
  else
    echo "cannot reach $host over ssh (set PARITY_PROXMOX)" >&2
    exit 1
  fi
fi
on_box() { "$SSH" "$host" "pct exec $ct -- bash -c $(printf '%q' "$1")"; }

work="$repo/parity/.run/lxc131-$name"
mkdir -p "$work"

if [ "$clean" = 1 ]; then
  on_box "docker rmi octo-rust-parity:$name 2> /dev/null; rm -rf /opt/octo-parity/runs/$name; git -C /opt/octo-parity/repo.git update-ref -d refs/heads/$name 2> /dev/null; df -h / | tail -n 1"
  exit 0
fi

if [ -n "$(git status --porcelain --untracked-files=no)" ]; then
  if [ "$prebuilt" = 1 ]; then
    # The binary is built from the working tree; the box checks out HEAD. They must agree.
    echo "commit first: the binary is built from the working tree and the box runs HEAD" >&2
    exit 1
  fi
  echo "note: uncommitted changes are not sent; the box runs $(git rev-parse --short HEAD)" >&2
fi
sha="$(git rev-parse HEAD)"

# The binary, compiled here (see above).
musl_target=x86_64-unknown-linux-musl
binary="$repo/target/$musl_target/release/octo"
if [ "$prebuilt" = 1 ]; then
  echo "building octo for $musl_target here"
  cargo build --release --locked --package octo --bin octo --target "$musl_target" --quiet
  echo "built $(du -h "$binary" | cut -f1) binary from $(git rev-parse --short HEAD)"
fi

# Only the commits the box does not have yet.
on_box "mkdir -p /opt/octo-parity/incoming && { [ -d /opt/octo-parity/repo.git ] || git init -q --bare /opt/octo-parity/repo.git; }"
known="$(on_box "git -C /opt/octo-parity/repo.git for-each-ref --format='%(objectname)'" || true)"
nots=()
for k in $known; do
  if git cat-file -e "$k^{commit}" 2> /dev/null; then nots+=("^$k"); fi
done
bundle="parity-$name.bundle"
rm -f "$work/$bundle"
if git bundle create "$work/$bundle" HEAD "${nots[@]}" > /dev/null 2>&1; then
  echo "sending $(du -h "$work/$bundle" | cut -f1) of commits to the box"
  (cd "$work" && "$SCP" -q "$bundle" "$host:/root/$bundle")
  "$SSH" "$host" "pct push $ct /root/$bundle /opt/octo-parity/incoming/$bundle && rm -f /root/$bundle"
  fetch="git -C /opt/octo-parity/repo.git fetch -q /opt/octo-parity/incoming/$bundle HEAD:refs/heads/$name --force && rm -f /opt/octo-parity/incoming/$bundle"
else
  # The box has HEAD already.
  fetch="git -C /opt/octo-parity/repo.git update-ref refs/heads/$name $sha"
fi
rm -f "$work/$bundle"

checkout="set -e; $fetch
run=/opt/octo-parity/runs/$name
mkdir -p \$run
if [ ! -d \$run/src/.git ]; then git clone -q /opt/octo-parity/repo.git \$run/src; fi
cd \$run/src
git fetch -q origin $name
git checkout -q -f $sha
git clean -q -fdx
echo \"box: $name at \$(git log --oneline -1)\""
on_box "$checkout"

if [ "$prebuilt" = 1 ]; then
  # Into the checkout, after its git clean; box-run.sh --prebuilt wraps it in the runtime stage.
  "$SCP" -q "$(command -v wslpath > /dev/null && [ "$SCP" = scp.exe ] && wslpath -w "$binary" || echo "$binary")" "$host:/root/parity-$name.octo"
  "$SSH" "$host" "pct exec $ct -- mkdir -p /opt/octo-parity/runs/$name/src/.parity-bin && pct push $ct /root/parity-$name.octo /opt/octo-parity/runs/$name/src/.parity-bin/octo --perms 755 && rm -f /root/parity-$name.octo"
  args=(--prebuilt "${args[@]+"${args[@]}"}")
fi

host_free_mb() { "$SSH" -o ConnectTimeout=20 "$host" "awk '/MemAvailable/ {print int(\$2 / 1024)}' /proc/meminfo" 2> /dev/null || echo 0; }
stop_box_run() {
  on_box "pkill -f '[b]ox-run.sh $name' ; pkill -f '[p]arity/parity.py run' ; cd /opt/octo-parity/runs/$name/src && for p in octo-parity-cs octo-parity-rs; do docker compose -p \$p -f parity/docker-compose.yml down -v --remove-orphans > /dev/null 2>&1; done; docker buildx stop octo-parity > /dev/null 2>&1; echo 'parity stacks stopped'" || true
}

need="${PARITY_MIN_HOST_MB:-3500}"
free="$(host_free_mb)"
if [ "$free" -lt "$need" ]; then
  echo "the Proxmox host has $free MB free (need $need); not starting. Try again later." >&2
  exit 1
fi

# The watchdog: every 15 s, the host's free memory; under 1.5 GB the run is stopped.
(
  while sleep 15; do
    f="$(host_free_mb)"
    if [ "$f" -gt 0 ] && [ "$f" -lt 1536 ]; then
      echo "WATCHDOG: the Proxmox host is down to $f MB free; stopping the parity run" >&2
      stop_box_run
      exit 0
    fi
  done
) &
watchdog=$!
trap 'kill $watchdog 2> /dev/null || true' EXIT

# The run itself. Its output streams here; the summary and the per-group table end it.
# One run at a time on the box: every branch's run uses the same stack names, and two at
# once would push the host into swap. box-run.sh takes /opt/octo-parity/run.lock itself, and
# a second run waits there for the first to finish. The lock is not taken here as well: a
# flock around box-run.sh holds the file through another open file, so box-run.sh's own
# flock waited on it forever.
quoted=""
for a in "${args[@]+"${args[@]}"}"; do quoted+=" $(printf '%q' "$a")"; done
on_box "bash /opt/octo-parity/runs/$name/src/parity/box-run.sh $name$quoted" || true
kill $watchdog 2> /dev/null || true

# The full reports, for reading the diffs themselves.
for f in report.txt report-bytes.txt cs-cs.txt cs-cs-bytes.txt rs-rs.txt rs-rs-bytes.txt report.groups.json; do
  if "$SSH" "$host" "pct pull $ct /opt/octo-parity/runs/$name/$f /root/parity-$name-$f" 2> /dev/null; then
    (cd "$work" && "$SCP" -q "$host:/root/parity-$name-$f" "$f")
    "$SSH" "$host" "rm -f /root/parity-$name-$f"
  fi
done
echo
echo "full report: parity/.run/lxc131-$name/report.txt"
