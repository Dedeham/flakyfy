#!/usr/bin/env bash
# Tests flakyfy-updater.sh against a throwaway git origin, with docker, flock and sleep
# faked, so it runs anywhere bash and git do (CI runs it beside shellcheck). Also runs
# the previous release's helper (from git) the same way, since the update into this release
# runs it; install-updater.sh --tidy; and checks the spotiflac service this release's compose
# file ships.
#   scripts/updater/test-updater.sh
set -uo pipefail
here="$(cd "$(dirname "$0")" && pwd)"
script="$here/flakyfy-updater.sh"
work="$(mktemp -d)"
trap 'rm -rf "$work"' EXIT
mkdir -p "$work/bin" "$work/origin-src/flakyfy"
failures=0

# docker logs what it was asked. Its compose file is shaped like the real one: Flakyfy and its
# sidecars are pulled, at the tag the checked out release names (its VERSION file, or its
# flakyfy.csproj version in a release from before VERSION), or FAKE_IMAGE_TAG (FLAKYFY_IMAGE_TAG in .env). FAKE_REGISTRY is where they come from,
# FAKE_FLAKYFY=build is an override that builds Flakyfy from the folder, DOCKER_FAIL makes one
# compose verb fail, FAKE_NO_IMAGE names services whose pull fails (no network to ghcr.io),
# FAKE_NO_SONIC leaves flakyfy-sonic out, and FAKE_BAD_VERSION is a release whose container
# never stays up. FAKE_SPOTIFLAC=1 turns on the optional spotiflac service (Mornye), whose
# image is flakyfy-mornye:<release> as this release's compose names it (FAKE_MORNYE_IMAGE another,
# none for a config that names nothing), with pull_policy never: up -d refuses to start it while
# its image is not here, since that would need a pull. FAKE_NO_BUILD names services whose build
# fails, and FAKE_NO_LOCAL=1 has no image here but what a build made. The local flakyfy-mornye tags
# are FAKE_MORNYE_TAGS, FAKE_IN_USE names ones a container uses, and FAKE_LS_FAIL and
# FAKE_PRUNE_FAIL make image ls and image prune fail.
cat > "$work/bin/docker" <<'EOF'
#!/usr/bin/env bash
echo "docker $*" >> "$FAKE_LOG"
release() {
  if [ -f "$FLAKYFY_DIR/VERSION" ]; then tr -d ' \r\n' < "$FLAKYFY_DIR/VERSION"
  else sed -n 's:.*<InformationalVersion>\(.*\)</InformationalVersion>.*:\1:p' "$FLAKYFY_DIR/flakyfy/flakyfy.csproj" | head -n 1; fi
}
mornye_image() {
  local image="${FAKE_MORNYE_IMAGE-flakyfy-mornye:$(release)}"
  [ "$image" = none ] || echo "$image"
}
here() { # whether an image is on this machine
  grep -qxF -- "$1" "$FAKE_IMAGES" 2>/dev/null || [ -z "${FAKE_NO_LOCAL:-}" ]
}
services() {
  printf 'flakyfy\n'
  [ -n "${FAKE_NO_SONIC:-}" ] || echo flakyfy-sonic
  printf 'slskd\n'
  [ -z "${FAKE_SPOTIFLAC:-}" ] || echo spotiflac
  printf 'unrelated\nyt-dlp-shim\n'
}
if [ "$1" = compose ]; then
  shift
  # The global --profile flag comes before the verb.
  while [ "${1:-}" = --profile ]; do shift 2; done
  case "$1" in
    config)
      if [ "${2:-}" = --services ]; then
        services
      elif [ "${2:-}" = --images ]; then
        [ "${3:-}" = spotiflac ] && mornye_image
      else
        # As docker compose config prints it: services in name order, flakyfy-sonic's image
        # quoted and the shim's not, slskd someone else's image, a volume list after. The
        # -source services are in a profile that is off, so they are absent.
        r="${FAKE_REGISTRY:-ghcr.io/dedeham}"
        t="${FAKE_IMAGE_TAG:-$(release)}"
        printf 'name: flakyfy\nservices:\n'
        printf '  flakyfy:\n'
        [ "${FAKE_FLAKYFY:-pull}" = build ] && printf '    build:\n      context: %s\n' "$FLAKYFY_DIR"
        printf '    image: %s/flakyfy:%s\n    depends_on:\n      slskd:\n        condition: service_started\n' "$r" "$t"
        [ -n "${FAKE_NO_SONIC:-}" ] || printf '  flakyfy-sonic:\n    image: "%s/flakyfy-sonic:%s"\n' "$r" "$t"
        printf '  slskd:\n    image: slskd/slskd:latest\n'
        printf '  unrelated:\n    image: %s/flakyfypus:1\n' "$r"
        printf '  yt-dlp-shim:\n    image: %s/flakyfy-yt-dlp-shim:%s\n' "$r" "$t"
        printf 'volumes:\n  ytdlp-bin:\n    name: flakyfy_ytdlp-bin\n'
      fi ;;
    ps) echo cid ;;
    pull)
      for service in ${FAKE_NO_IMAGE:-}; do
        [ "$2" = "$service" ] && { echo "Error response from daemon: $service: denied"; exit 1; }
      done
      if [ "${DOCKER_FAIL:-}" = pull ]; then echo "boom: pull failed"; exit 1; fi ;;
    build)
      for service in ${FAKE_NO_BUILD:-}; do
        [ "${2:-}" = "$service" ] && { echo "failed to build $service"; exit 1; }
      done
      if [ "${DOCKER_FAIL:-}" = build ]; then echo "boom: build failed"; exit 1; fi
      [ "${2:-}" = spotiflac ] && mornye_image >> "$FAKE_IMAGES" ;;
    up)
      if [ "${DOCKER_FAIL:-}" = up ]; then echo "boom: up failed"; exit 1; fi
      shift
      [ "${1:-}" = -d ] && shift
      if [ $# -gt 0 ]; then starting="$*"; else starting="$(services | tr '\n' ' ')"; fi
      image="$(mornye_image)"
      if [[ " $starting " == *" spotiflac "* ]] && [ -n "$image" ] && ! here "$image"; then
        echo "Error response from daemon: pull access denied for ${image%%:*}: spotiflac has pull_policy never"
        exit 1
      fi ;;
    *) if [ "${DOCKER_FAIL:-}" = "$1" ]; then echo "boom: $1 failed"; exit 1; fi ;;
  esac
  exit 0
fi
if [ "$1" = image ]; then
  case "${2:-}" in
    inspect) here "$3" || { echo "No such image: $3" >&2; exit 1; } ;;
    ls)
      [ -z "${FAKE_LS_FAIL:-}" ] || { echo "boom: image ls failed" >&2; exit 1; }
      for image in ${FAKE_MORNYE_TAGS:-}; do echo "$image"; done ;;
    rm)
      for image in ${FAKE_IN_USE:-}; do
        [ "$3" = "$image" ] && { echo "Error response from daemon: conflict: unable to remove repository reference \"$3\" (must force) - container cid is using its referenced image" >&2; exit 1; }
      done
      echo "Untagged: $3" ;;
    prune) [ -z "${FAKE_PRUNE_FAIL:-}" ] || { echo "boom: prune failed" >&2; exit 1; } ;;
  esac
  exit 0
fi
if [ "$1" = inspect ]; then
  if [ -n "${FAKE_BAD_VERSION:-}" ] && [ "$(release)" = "$FAKE_BAD_VERSION" ]; then
    echo restarting
  else
    echo "${FAKE_STATE:-running}"
  fi
fi
EOF
printf '#!/usr/bin/env bash\nexit 0\n' > "$work/bin/flock"
printf '#!/usr/bin/env bash\nexit 0\n' > "$work/bin/sleep"
chmod +x "$work/bin/"*
export PATH="$work/bin:$PATH" FAKE_LOG="$work/docker.log" FAKE_IMAGES="$work/images"

# The helper the previous release installed, which runs the update into this release: the next release's
# compose file meets the helper already on the server, not the one it carries.
OLD_HELPER_COMMIT=200c240316bfcf6f2c38eaeee763ac0dc354723d
old_script="$work/flakyfy-updater-previous.sh"
if ! git -C "$here" show "$OLD_HELPER_COMMIT:scripts/updater/flakyfy-updater.sh" > "$old_script" 2> /dev/null; then
  # A shallow checkout (CI's) has only its own commit; fetch that one release.
  git -C "$here" fetch -q --depth 1 origin "$OLD_HELPER_COMMIT" > /dev/null 2>&1 || true
  git -C "$here" show "$OLD_HELPER_COMMIT:scripts/updater/flakyfy-updater.sh" > "$old_script" 2> /dev/null || : > "$old_script"
fi

# An origin whose main is at 2026.10.01, with newer releases 2026.10.04 and 2026.10.06 tagged.
# The newer releases carry a helper of their own, whose installer only says it ran (or fails,
# with FAKE_REFRESH_FAIL), so a run can be seen reinstalling the helper. 2026.10.01 names its
# release in flakyfy.csproj, as 2026.10.05 and earlier did; from 2026.10.04 on it is the VERSION
# file, and flakyfy.csproj (the C# app kept beside the Rust one) is left behind on purpose, to show
# it is no longer read.
(
  cd "$work/origin-src" || exit 1
  git init -q -b main . && git config user.email test@example.com && git config user.name test
  git config core.autocrlf false
  version() { printf '<Project>\n  <PropertyGroup>\n    <InformationalVersion>%s</InformationalVersion>\n  </PropertyGroup>\n</Project>\n' "$1" > flakyfy/flakyfy.csproj; }
  version 2026.10.01 && git add -A && git commit -qm one && git tag 2026.10.01
  mkdir -p scripts/updater
  printf '#!/usr/bin/env bash\necho "install-updater $*" >> "$FAKE_LOG"\n[ -z "${FAKE_REFRESH_FAIL:-}" ]\n' > scripts/updater/install-updater.sh
  echo "# the next helper" > scripts/updater/flakyfy-updater.sh
  touch scripts/updater/flakyfy-updater.path scripts/updater/flakyfy-updater.service
  echo 2026.10.04 > VERSION && git add -A && git update-index --chmod=+x scripts/updater/install-updater.sh \
    && git commit -qm two && git tag 2026.10.04
  echo 2026.10.06 > VERSION && git commit -qam three && git tag 2026.10.06
  git reset -q --hard 2026.10.01
) || { echo "could not build the test origin"; exit 1; }
git clone -q --bare "$work/origin-src" "$work/origin.git"

fresh() { # a clone on main at 2026.10.01 that has not fetched the newer releases yet
  rm -rf "$work/flakyfy" "$work/config"
  git clone -q "$work/origin.git" "$work/flakyfy"
  git -C "$work/flakyfy" tag -d 2026.10.04 2026.10.06 > /dev/null 2>&1 || true
  mkdir -p "$work/config/update"
  : > "$FAKE_LOG"
  : > "$FAKE_IMAGES"
}
copied() { # the same folder, copied by hand: not a git clone
  fresh
  rm -rf "$work/flakyfy/.git"
}
request() { # tag, id
  printf 'id=%s\ntag=%s\nby=test\nat=2026-10-03T12:00:00Z\n' "${2:-0b7c7a8e-1111-4222-8333-444455556666}" "$1" > "$work/config/update/request"
}
status_of() { sed -n "s/^$1=//p" "$work/config/update/status" 2>/dev/null | head -n 1; }
helper_says() { sed -n "s/^$1=//p" "$work/config/update/helper" 2>/dev/null | head -n 1; }
check() { # name, expected state, expected checkout, words the error must hold; UPDATER picks the helper
  local name="$1" state head error
  FLAKYFY_DIR="$work/flakyfy" FLAKYFY_UPDATE_DIR="$work/config/update" bash "${UPDATER:-$script}" > /dev/null 2>&1
  state="$(status_of state)"
  head="$(git -C "$work/flakyfy" describe --tags --exact-match 2>/dev/null || echo none)"
  error="$(status_of error)"
  if [ "$state" = "$2" ] && [ "$head" = "$3" ] && [[ "$error" == *"$4"* ]] && [ ! -f "$work/config/update/request" ]; then
    echo "ok    $name"
  else
    echo "FAIL  $name: state=$state (want $2), checkout=$head (want $3), error=$error"
    failures=$((failures + 1))
  fi
}
expect() { # name, then a command that must succeed
  local name="$1"
  shift
  if "$@"; then echo "ok    $name"; else echo "FAIL  $name"; failures=$((failures + 1)); fi
}
in_log() { grep -q -- "$1" "$work/config/update/log"; }
never() { ! grep -Eq -- "$1" "$FAKE_LOG"; }

# The compose commands a run gave, in order, joined with |, without the ones that only read.
compose_log() { sed -n 's/^docker compose //p' "$FAKE_LOG" | grep -Ev '^(--profile [^ ]+ )?(config|ps)( |$)' | paste -sd '|' -; }
# The image commands a run gave (removing and pruning), in order, joined with |.
image_log() { sed -n 's/^docker image \(rm\|prune\|ls\)/\1/p' "$FAKE_LOG" | paste -sd '|' -; }
expect_images() { # name, the image commands wanted, in order, joined with |
  local got
  got="$(image_log)"
  if [ "$got" = "$2" ]; then echo "ok    $1"; else echo "FAIL  $1: ran $got (want $2)"; failures=$((failures + 1)); fi
}
expect_compose() { # name, the compose commands wanted, in order, joined with |
  local got
  got="$(compose_log)"
  if [ "$got" = "$2" ]; then echo "ok    $1"; else echo "FAIL  $1: ran $got (want $2)"; failures=$((failures + 1)); fi
}

# ----- A git clone: fetch, check out, pull -----

fresh; request 2026.10.04
check "updates to a newer release" "done" 2026.10.04 ""
expect "describes itself as version 2 in a clone" test "$(helper_says version) $(helper_says mode)" = "2 git"
expect "names itself as Flakyfy's helper" test "$(helper_says product)" = flakyfy
expect_compose "pulls Flakyfy and its sidecars, builds nothing, then restarts" "pull flakyfy|pull yt-dlp-shim|pull flakyfy-sonic|up -d"
expect "says what it now runs" test "$(status_of step)" = "Flakyfy now runs 2026.10.04"
expect "without the spotiflac profile Mornye is never got ready" never 'compose build spotiflac|image inspect'

# ----- Mornye (the optional spotiflac service) never holds Flakyfy back -----

fresh; request 2026.10.04
FAKE_SPOTIFLAC=1 FAKE_NO_LOCAL=1 check "Mornye got ready before the restart" "done" 2026.10.04 ""
expect_compose "gets Mornye's image ready, then restarts everything" "pull flakyfy|pull yt-dlp-shim|pull flakyfy-sonic|build spotiflac|up -d"
expect "a Mornye got ready here is not called Flakyfy's build" test "$(status_of step)" = "Flakyfy now runs 2026.10.04"

fresh; request 2026.10.04
FAKE_SPOTIFLAC=1 FAKE_NO_BUILD=spotiflac check "Mornye keeps the image it has" "done" 2026.10.04 ""
expect_compose "restarts everything with Mornye's old image" "pull flakyfy|pull yt-dlp-shim|pull flakyfy-sonic|build spotiflac|up -d"
expect "the log says Mornye kept its image" in_log 'spotiflac did not get ready, so it keeps the image it has'

fresh; request 2026.10.04
FAKE_SPOTIFLAC=1 FAKE_NO_BUILD=spotiflac FAKE_NO_LOCAL=1 check "Flakyfy updates without a Mornye it cannot get" "done" 2026.10.04 ""
expect_compose "starts every service but Mornye" "pull flakyfy|pull yt-dlp-shim|pull flakyfy-sonic|build spotiflac|up -d flakyfy flakyfy-sonic slskd unrelated yt-dlp-shim"
expect "the dashboard says Mornye was left as it was" test "$(status_of step)" = "Flakyfy now runs 2026.10.04. Mornye (the spotiflac service) did not get ready, so it was left as it was"

fresh; request 2026.10.04
FAKE_SPOTIFLAC=1 FAKE_NO_BUILD=spotiflac FAKE_NO_LOCAL=1 FAKE_BAD_VERSION=2026.10.04 check "going back leaves Mornye out too" failed 2026.10.01 "which is running again"
expect_compose "rebuilds and restarts the old release without Mornye" "pull flakyfy|pull yt-dlp-shim|pull flakyfy-sonic|build spotiflac|up -d flakyfy flakyfy-sonic slskd unrelated yt-dlp-shim|build flakyfy flakyfy-sonic slskd unrelated yt-dlp-shim|up -d flakyfy flakyfy-sonic slskd unrelated yt-dlp-shim"
expect "no Mornye image is removed when Flakyfy did not stay running" never 'image (rm|prune)'

# ----- The update into this release runs the previous release's helper against this release's compose -----

if [ -s "$old_script" ]; then
  fresh; request 2026.10.04
  FAKE_SPOTIFLAC=1 FAKE_NO_BUILD=spotiflac FAKE_NO_LOCAL=1 UPDATER="$old_script" check "the previous release's helper updates Flakyfy when Mornye's image does not get ready" "done" 2026.10.04 ""
  expect_compose "it leaves Mornye out and starts everything else" "pull flakyfy|pull yt-dlp-shim|pull flakyfy-sonic|build spotiflac|up -d flakyfy flakyfy-sonic slskd unrelated yt-dlp-shim"
  expect "it says Mornye was left as it was" test "$(status_of step)" = "Flakyfy now runs 2026.10.04. Mornye (the spotiflac service) could not be built, so it was left as it was"
  expect "a plain up -d there would have failed on the missing image" \
    bash -c '! FAKE_SPOTIFLAC=1 FAKE_NO_LOCAL=1 FLAKYFY_DIR="$1" docker compose up -d > /dev/null' _ "$work/flakyfy"

  fresh; request 2026.10.04
  FAKE_SPOTIFLAC=1 FAKE_NO_LOCAL=1 UPDATER="$old_script" check "the previous release's helper gets this release's Mornye ready" "done" 2026.10.04 ""
  expect_compose "it builds the release's flakyfy-mornye, then starts everything" "pull flakyfy|pull yt-dlp-shim|pull flakyfy-sonic|build spotiflac|up -d"
  expect "the image it got ready is the release's" grep -qx 'flakyfy-mornye:2026.10.04' "$FAKE_IMAGES"
else
  echo "FAIL  the previous release's helper ($OLD_HELPER_COMMIT) could not be read from git"
  failures=$((failures + 1))
fi

# ----- Old Mornye images go once Flakyfy runs -----

MORNYE_103=flakyfy-mornye:5660533bf12592e9de68508cef506f173f58d14f
fresh; request 2026.10.04
FAKE_MORNYE_TAGS="$MORNYE_103 flakyfy-mornye:2026.10.01 flakyfy-mornye:2026.10.04" check "old Mornye images are removed after the update" "done" 2026.10.04 ""
expect_images "every flakyfy-mornye tag but this release's goes, then the untagged ones" "ls --format {{.Repository}}:{{.Tag}} flakyfy-mornye|rm $MORNYE_103|rm flakyfy-mornye:2026.10.01|prune -f --filter label=app.flakyfy.mornye=1"
expect "the removal comes after Flakyfy runs" test "$(grep -nE 'compose up -d|image ls' "$FAKE_LOG" | cut -d: -f2- | paste -sd '|' -)" = "docker compose up -d|docker image ls --format {{.Repository}}:{{.Tag}} flakyfy-mornye"
expect "it reads the release's Mornye with the spotiflac profile on, whatever .env says" grep -qx 'docker compose --profile spotiflac config --images spotiflac' "$FAKE_LOG"

fresh; request 2026.10.04
FAKE_SPOTIFLAC=1 FAKE_NO_LOCAL=1 FAKE_MORNYE_TAGS="$MORNYE_103 flakyfy-mornye:2026.10.04" check "with Mornye on, its old image goes too" "done" 2026.10.04 ""
expect_images "only the old tag is removed" "ls --format {{.Repository}}:{{.Tag}} flakyfy-mornye|rm $MORNYE_103|prune -f --filter label=app.flakyfy.mornye=1"

fresh; request 2026.10.04
FAKE_SPOTIFLAC=1 FAKE_NO_BUILD=spotiflac FAKE_NO_LOCAL=1 FAKE_MORNYE_TAGS="$MORNYE_103" FAKE_IN_USE="$MORNYE_103" check "an old Mornye image still in use stays" "done" 2026.10.04 ""
expect "the log says it stays" in_log "$MORNYE_103 is still in use, so it stays"

fresh; request 2026.10.04
FAKE_MORNYE_TAGS="" check "no old Mornye images, nothing removed" "done" 2026.10.04 ""
expect_images "it looks, then prunes only" "ls --format {{.Repository}}:{{.Tag}} flakyfy-mornye|prune -f --filter label=app.flakyfy.mornye=1"

fresh; request 2026.10.04
FAKE_LS_FAIL=1 FAKE_PRUNE_FAIL=1 FAKE_MORNYE_TAGS="$MORNYE_103" check "images that cannot be listed or pruned never fail the update" "done" 2026.10.04 ""
expect "the log says the prune failed" in_log 'Pruning old Mornye images failed; they stay'

fresh; request 2026.10.04
FAKE_MORNYE_IMAGE=none FAKE_MORNYE_TAGS="$MORNYE_103" check "a compose file that names no Mornye removes nothing" "done" 2026.10.04 ""
expect "nothing is listed or removed" never 'image (ls|rm|prune)'

fresh; echo "SPOTIFLAC_BUILD_REF=5660533bf12592e9de68508cef506f173f58d14f" > "$work/flakyfy/.env"; request 2026.10.04
check "a .env that still sets SPOTIFLAC_BUILD_REF" "done" 2026.10.04 ""
expect "the log says it is no longer read" in_log "SPOTIFLAC_BUILD_REF is no longer read; Flakyfy's Mornye is 1.0.5. To run another Mornye, set SPOTIFLAC_IMAGE."

fresh; printf 'SPOTIFLAC_BUILD_REF=\nKEY=value\n' > "$work/flakyfy/.env"; request 2026.10.04
check "a .env with SPOTIFLAC_BUILD_REF blank" "done" 2026.10.04 ""
expect "says nothing about it" bash -c '! grep -q SPOTIFLAC_BUILD_REF "$1"' _ "$work/config/update/log"

# ----- install-updater.sh --tidy: the removal the update into this release runs -----

tidy_dir="$work/tidy"
tidy() { # runs the installer's tidy in a folder of its own; the rest is its arguments
  rm -rf "$tidy_dir"
  mkdir -p "$tidy_dir/scripts/updater"
  cp "$here/install-updater.sh" "$tidy_dir/scripts/updater/"
  echo 2026.10.04 > "$tidy_dir/VERSION"
  : > "$FAKE_LOG"
  [ -z "${TIDY_ENV:-}" ] || printf '%s\n' "$TIDY_ENV" > "$tidy_dir/.env"
  FLAKYFY_DIR="$tidy_dir" bash "$tidy_dir/scripts/updater/install-updater.sh" --tidy "$@" > "$work/tidy.out" 2>&1
}
FAKE_MORNYE_TAGS="$MORNYE_103 flakyfy-mornye:2026.10.04" FAKE_IN_USE="" tidy
expect "--tidy finishes" test $? = 0
expect_images "--tidy removes every flakyfy-mornye tag but the folder's release" "ls --format {{.Repository}}:{{.Tag}} flakyfy-mornye|rm $MORNYE_103|prune -f --filter label=app.flakyfy.mornye=1"
expect "--tidy says what it removed" grep -q "Removed $MORNYE_103, which this release no longer runs." "$work/tidy.out"
FAKE_MORNYE_TAGS="$MORNYE_103" FAKE_IN_USE="$MORNYE_103" TIDY_ENV="SPOTIFLAC_BUILD_REF=5660533b" tidy --quiet
expect "--tidy with an image in use finishes" test $? = 0
expect "--quiet still says SPOTIFLAC_BUILD_REF is no longer read" grep -qx "SPOTIFLAC_BUILD_REF is no longer read; Flakyfy's Mornye is 1.0.5. To run another Mornye, set SPOTIFLAC_IMAGE." "$work/tidy.out"
expect "--quiet says nothing else" test "$(wc -l < "$work/tidy.out")" = 1
FAKE_LS_FAIL=1 FAKE_PRUNE_FAIL=1 tidy
expect "--tidy never fails on docker" test $? = 0
FAKE_MORNYE_IMAGE=none FAKE_MORNYE_TAGS="$MORNYE_103" tidy
expect "--tidy removes nothing when compose names no Mornye" never 'image (ls|rm|prune)'
expect "an install and a refresh tidy too, once the helper is in place" \
  test "$(grep -A1 -- '"$lib_dir/flakyfy-updater.sh" --describe$' "$here/install-updater.sh" | tail -n 1)" = tidy_mornye

# ----- The compose file this release ships -----

repo="$here/../.."
version="$(tr -d ' \r\n' < "$repo/VERSION")"
spotiflac_block="$(awk '/^  spotiflac:/ { on = 1; print; next } on && /^  [^ ]/ { exit } on { print }' "$repo/docker-compose.yml")"
expect "the spotiflac service runs this release's Flakyfy Mornye image" grep -qx "    image: flakyfy-mornye:$version" <<< "$spotiflac_block"
expect "it gets it ready from Flakyfy's repository at this release" grep -Fqx "      context: \${SPOTIFLAC_BUILD_CONTEXT:-https://github.com/Dedeham/flakyfy.git#$version:mornye}" <<< "$spotiflac_block"
expect "it is never pulled" grep -qx "    pull_policy: never" <<< "$spotiflac_block"
expect "SPOTIFLAC_IMAGE reaches the build" grep -qx '        MORNYE_IMAGE: ${SPOTIFLAC_IMAGE:-}' <<< "$spotiflac_block"
expect "it names no FLAKYFY_IMAGE_TAG and no SPOTIFLAC_BUILD_REF" bash -c '! grep -Eq "FLAKYFY_IMAGE_TAG|SPOTIFLAC_BUILD_REF" <<< "$1"' _ "$spotiflac_block"
digest="$(grep -o 'sha256:[0-9a-f]\{64\}' "$repo/mornye/Dockerfile")"
expect "mornye/Dockerfile pins one digest" test "$(grep -c 'sha256:' "$repo/mornye/Dockerfile")" = 1
expect "it is Mornye 1.0.5's" test "$digest" = sha256:8840ab3f1dd623ae8a5290057b35f9a512acc5339bf1e2c96a29ca3f9aaee65c
expect "the build argument has no default, so a blank one keeps the digest" grep -qx 'ARG MORNYE_IMAGE' "$repo/mornye/Dockerfile"
expect "FROM falls back to the digest" grep -qxF "FROM \${MORNYE_IMAGE:-ghcr.io/mornyeapp/mornye-python@$digest}" "$repo/mornye/Dockerfile"
expect "the image is labeled for the removal" grep -qx 'LABEL app.flakyfy.mornye=1' "$repo/mornye/Dockerfile"
expect "the digest is written nowhere else" bash -c '! grep -l "$1" "$2/docker-compose.yml" "$2/.env.example" "$2/README.md" > /dev/null' _ "$digest" "$repo"

fresh; request 2026.10.04
FAKE_NO_IMAGE=flakyfy check "Flakyfy that will not pull is built from the folder" "done" 2026.10.04 ""
expect_compose "builds flakyfy-source, the sidecars still pulled" "pull flakyfy|pull yt-dlp-shim|pull flakyfy-sonic|--profile source build flakyfy-source|up -d"
expect "the log says why it built" in_log 'flakyfy could not be pulled'
expect "the dashboard says it was built here" test "$(status_of step)" = "Flakyfy now runs 2026.10.04, built on this server because its image could not be pulled"

fresh; request 2026.10.04
FAKE_NO_IMAGE=flakyfy-sonic check "a sidecar that will not pull is built instead" "done" 2026.10.04 ""
expect_compose "builds flakyfy-sonic from the folder, Flakyfy and the shim still pulled" "pull flakyfy|pull yt-dlp-shim|pull flakyfy-sonic|--profile source build flakyfy-sonic-source|up -d"
expect "a sidecar built here is not called Flakyfy's build" test "$(status_of step)" = "Flakyfy now runs 2026.10.04"

fresh; request 2026.10.04
FAKE_NO_IMAGE="flakyfy yt-dlp-shim flakyfy-sonic" check "without ghcr.io everything is built" "done" 2026.10.04 ""
expect_compose "builds Flakyfy and both sidecars" "pull flakyfy|pull yt-dlp-shim|pull flakyfy-sonic|--profile source build flakyfy-source yt-dlp-shim-source flakyfy-sonic-source|up -d"

fresh; request 2026.10.04
FAKE_NO_IMAGE=flakyfy DOCKER_FAIL=build check "a failed build changes nothing" failed 2026.10.01 "nothing was restarted"
expect "nothing restarted after the failed build" never 'compose up'

fresh; request 2026.10.04
FAKE_NO_SONIC=1 check "a release without flakyfy-sonic" "done" 2026.10.04 ""
expect_compose "pulls only the images the release has" "pull flakyfy|pull yt-dlp-shim|up -d"

fresh; request 2026.10.04
FAKE_FLAKYFY=build check "an override that builds Flakyfy from the folder" "done" 2026.10.04 ""
expect_compose "builds Flakyfy and pulls the sidecars" "pull yt-dlp-shim|pull flakyfy-sonic|--profile source build flakyfy|up -d"
expect "the log says why it built" in_log 'An override builds Flakyfy'

fresh; request 2026.10.04
FAKE_IMAGE_TAG=latest check "FLAKYFY_IMAGE_TAG=latest in a clone" "done" 2026.10.04 ""
expect_compose "pulls latest with the release checked out" "pull flakyfy|pull yt-dlp-shim|pull flakyfy-sonic|up -d"

fresh; request 2026.10.04
FAKE_IMAGE_TAG=2026.10.01 check "FLAKYFY_IMAGE_TAG pinned to an older release is refused" failed 2026.10.01 "asks for Flakyfy 2026.10.01"
expect "nothing pulled or restarted while pinned" never 'compose (pull|build|up)'

fresh; request "2026.10.04; touch $work/pwned"
check "a tag that is not a release is ignored" failed 2026.10.01 "did not name a dated release"
expect "nothing in a request is run" test ! -e "$work/pwned"

fresh; request 2026.10.04 not-a-guid
check "a request without a valid id is ignored" failed 2026.10.01 "no valid id"

fresh; request 2026.10.09
check "a release that does not exist" failed 2026.10.01 "is not a release tag"

fresh; request 2026.10.01
check "the same release is refused" failed 2026.10.01 "not older than"

fresh; echo "<!-- edit -->" >> "$work/flakyfy/flakyfy/flakyfy.csproj"; request 2026.10.04
check "local changes to Flakyfy's own files are left alone" failed 2026.10.01 "(flakyfy/flakyfy.csproj)"
expect "a changed folder is never pulled over" never 'compose (pull|build|up)'

fresh; echo "KEY=value" > "$work/flakyfy/.env"; request 2026.10.04
check "untracked files such as .env are fine" "done" 2026.10.04 ""

fresh; request 2026.10.04
FAKE_BAD_VERSION=2026.10.04 check "a release that will not stay up is rolled back" failed 2026.10.01 "went back to 2026.10.01"
# The old release's images are still here; a plain build builds what it built, if anything.
expect_compose "the rollback restarts the old release" "pull flakyfy|pull yt-dlp-shim|pull flakyfy-sonic|up -d|build|up -d"

fresh; request 2026.10.04
FAKE_STATE=restarting check "a failed rollback says so" failed 2026.10.01 "failed too"

fresh; request 2026.10.04
check "an update from a release that names itself in flakyfy.csproj" "done" 2026.10.04 ""
expect "the run starts from the flakyfy.csproj release" test "$(status_of from)" = 2026.10.01
request 2026.10.06
check "an update from a release with a VERSION file" "done" 2026.10.06 ""
expect "the run starts from the VERSION release, not flakyfy.csproj" test "$(status_of from)" = 2026.10.04
expect "it pulls the tag the VERSION file names" grep -q 'image: ghcr.io/dedeham/flakyfy:2026.10.06' <(FLAKYFY_DIR="$work/flakyfy" docker compose config)

fresh; request 2026.10.04
FLAKYFY_UPDATER_DRYRUN=1 FAKE_MORNYE_TAGS="flakyfy-mornye:2026.09.30" check "a dry run changes nothing" "done" 2026.10.01 ""
expect "the dry run never builds or pulls" never 'compose (build|pull|up)'
expect "the dry run removes no image" never 'image (rm|prune)'

fresh; request 2026.10.04
check "a release with a new helper" "done" 2026.10.04 ""
expect "reinstalls the helper from the release" grep -q '^install-updater --config .* --quiet$' "$FAKE_LOG"
expect "the log says so" in_log 'reinstalling it'

fresh; request 2026.10.04
FAKE_REFRESH_FAIL=1 check "a helper that cannot be reinstalled leaves the update done" "done" 2026.10.04 ""
expect "the log says the old helper stays" in_log 'the old one stays'

fresh; git -C "$work/flakyfy" remote remove origin; request 2026.10.04
FAKE_IMAGE_TAG=latest check "a clone without an origin only pulls" "done" 2026.10.01 ""
expect "describes itself as an image install" test "$(helper_says mode)" = image

# ----- Copied by hand, not a git clone: pull only -----

copied; request 2026.10.04
FAKE_IMAGE_TAG=latest check "an install that is not a clone pulls instead" "done" none ""
expect "describes itself as version 2, image" test "$(helper_says version) $(helper_says mode)" = "2 image"
expect "pulled Flakyfy and its sidecars, not slskd" grep -qx 'docker compose pull flakyfy flakyfy-sonic yt-dlp-shim' "$FAKE_LOG"
expect "restarted Flakyfy and its sidecars" grep -qx 'docker compose up -d flakyfy flakyfy-sonic yt-dlp-shim' "$FAKE_LOG"
expect "it never builds" never 'compose .*build'

copied; request 2026.10.04
FAKE_IMAGE_TAG=latest FAKE_MORNYE_TAGS="flakyfy-mornye:2026.09.30 flakyfy-mornye:2026.10.01" check "an install that is not a clone removes old Mornye images too" "done" none ""
expect_images "the tag its compose file names stays" "ls --format {{.Repository}}:{{.Tag}} flakyfy-mornye|rm flakyfy-mornye:2026.09.30|prune -f --filter label=app.flakyfy.mornye=1"

copied; request 2026.10.04
check "a copied compose file that names an older release is refused" failed none "would not reach 2026.10.04"
expect "nothing pulled when it would not move" never 'compose (pull|up)'

copied; request 2026.10.04
FAKE_IMAGE_TAG=2026.10.04 check "a copied compose file already at the release pulls it" "done" none ""

copied; request 2026.10.04
FAKE_IMAGE_TAG=latest FAKE_REGISTRY=localhost:5000/me check "a registry with a port" "done" none ""
expect "the port is not taken for a tag" grep -qx 'docker compose pull flakyfy flakyfy-sonic yt-dlp-shim' "$FAKE_LOG"

copied; request 2026.10.04
FAKE_IMAGE_TAG=latest DOCKER_FAIL=pull check "a failed pull restarts nothing" failed none "docker compose pull failed"
expect "nothing restarted" never 'compose up'

copied; request 2026.10.04
FAKE_IMAGE_TAG=latest FAKE_FLAKYFY=build check "Flakyfy built from a folder that is not a clone is refused" failed none "not a git clone"

copied; request 2026.10.04
FAKE_IMAGE_TAG=latest FLAKYFY_UPDATER_DRYRUN=1 check "an image install's dry run changes nothing" "done" none ""
expect "the image dry run never pulls" never 'compose (pull|up)'

fresh
FLAKYFY_DIR="$work/flakyfy" FLAKYFY_UPDATE_DIR="$work/config/update" bash "$script" > /dev/null 2>&1
expect "no request, no run" test ! -f "$work/config/update/status"

fresh
FLAKYFY_DIR="$work/flakyfy" FLAKYFY_UPDATE_DIR="$work/config/update" bash "$script" --describe > /dev/null 2>&1
expect "--describe writes version 2" test "$(helper_says version) $(helper_says mode)" = "2 git"

echo
[ "$failures" = 0 ] && echo "All update helper tests passed." || echo "$failures update helper test(s) failed."
[ "$failures" = 0 ]
