#!/usr/bin/env bash
# Tests flakyfy-updater.sh against a throwaway git origin, with docker, flock and sleep
# faked, so it runs anywhere bash and git do (CI runs it beside shellcheck).
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
# never stays up. FAKE_SPOTIFLAC=1 turns on the optional spotiflac service (Mornye),
# FAKE_NO_BUILD names services whose build fails, and FAKE_NO_LOCAL=1 has no image here.
cat > "$work/bin/docker" <<'EOF'
#!/usr/bin/env bash
echo "docker $*" >> "$FAKE_LOG"
release() {
  if [ -f "$FLAKYFY_DIR/VERSION" ]; then tr -d ' \r\n' < "$FLAKYFY_DIR/VERSION"
  else sed -n 's:.*<InformationalVersion>\(.*\)</InformationalVersion>.*:\1:p' "$FLAKYFY_DIR/flakyfy/flakyfy.csproj" | head -n 1; fi
}
if [ "$1" = compose ]; then
  shift
  # The global --profile flag comes before the verb.
  while [ "${1:-}" = --profile ]; do shift 2; done
  case "$1" in
    config)
      if [ "${2:-}" = --services ]; then
        printf 'flakyfy\n'
        [ -n "${FAKE_NO_SONIC:-}" ] || echo flakyfy-sonic
        printf 'slskd\n'
        [ -z "${FAKE_SPOTIFLAC:-}" ] || echo spotiflac
        printf 'unrelated\nyt-dlp-shim\n'
      elif [ "${2:-}" = --images ]; then
        [ "${3:-}" = spotiflac ] && echo ghcr.io/mornyeapp/mornye-python:latest
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
      if [ "${DOCKER_FAIL:-}" = build ]; then echo "boom: build failed"; exit 1; fi ;;
    *) if [ "${DOCKER_FAIL:-}" = "$1" ]; then echo "boom: $1 failed"; exit 1; fi ;;
  esac
  exit 0
fi
if [ "$1" = image ] && [ "${2:-}" = inspect ]; then
  [ -z "${FAKE_NO_LOCAL:-}" ] || { echo "No such image: $3"; exit 1; }
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
export PATH="$work/bin:$PATH" FAKE_LOG="$work/docker.log"

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
check() { # name, expected state, expected checkout, words the error must hold
  local name="$1" state head error
  FLAKYFY_DIR="$work/flakyfy" FLAKYFY_UPDATE_DIR="$work/config/update" bash "$script" > /dev/null 2>&1
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

# The compose commands a run gave, in order, joined with |.
compose_log() { sed -n 's/^docker compose //p' "$FAKE_LOG" | grep -Ev '^(config|ps)' | paste -sd '|' -; }
expect_compose() { # name, the compose commands wanted, in order, joined with |
  local got
  got="$(compose_log)"
  if [ "$got" = "$2" ]; then echo "ok    $1"; else echo "FAIL  $1: ran $got (want $2)"; failures=$((failures + 1)); fi
}

# ----- A git clone: fetch, check out, pull -----

fresh; request 2026.10.04
check "updates to a newer release" "done" 2026.10.04 ""
expect "describes itself as version 2 in a clone" test "$(helper_says version) $(helper_says mode)" = "2 git"
expect_compose "pulls Flakyfy and its sidecars, builds nothing, then restarts" "pull flakyfy|pull yt-dlp-shim|pull flakyfy-sonic|up -d"
expect "says what it now runs" test "$(status_of step)" = "Flakyfy now runs 2026.10.04"
expect "without the spotiflac profile nothing about Mornye runs" never 'spotiflac|image inspect'

# ----- Mornye (the optional spotiflac service) never holds Flakyfy back -----

fresh; request 2026.10.04
FAKE_SPOTIFLAC=1 check "Mornye built before the restart" "done" 2026.10.04 ""
expect_compose "builds Mornye from its repository, then restarts everything" "pull flakyfy|pull yt-dlp-shim|pull flakyfy-sonic|build spotiflac|up -d"
expect "a Mornye built here is not called Flakyfy's build" test "$(status_of step)" = "Flakyfy now runs 2026.10.04"

fresh; request 2026.10.04
FAKE_SPOTIFLAC=1 FAKE_NO_BUILD=spotiflac check "Mornye keeps the image it has" "done" 2026.10.04 ""
expect_compose "restarts everything with Mornye's old image" "pull flakyfy|pull yt-dlp-shim|pull flakyfy-sonic|build spotiflac|up -d"
expect "the log says Mornye kept its image" in_log 'spotiflac could not be built, so it keeps the image it has'

fresh; request 2026.10.04
FAKE_SPOTIFLAC=1 FAKE_NO_BUILD=spotiflac FAKE_NO_LOCAL=1 check "Flakyfy updates without a Mornye it cannot get" "done" 2026.10.04 ""
expect_compose "starts every service but Mornye" "pull flakyfy|pull yt-dlp-shim|pull flakyfy-sonic|build spotiflac|up -d flakyfy flakyfy-sonic slskd unrelated yt-dlp-shim"
expect "the dashboard says Mornye was left as it was" test "$(status_of step)" = "Flakyfy now runs 2026.10.04. Mornye (the spotiflac service) could not be built, so it was left as it was"

fresh; request 2026.10.04
FAKE_SPOTIFLAC=1 FAKE_NO_BUILD=spotiflac FAKE_NO_LOCAL=1 FAKE_BAD_VERSION=2026.10.04 check "going back leaves Mornye out too" failed 2026.10.01 "which is running again"
expect_compose "rebuilds and restarts the old release without Mornye" "pull flakyfy|pull yt-dlp-shim|pull flakyfy-sonic|build spotiflac|up -d flakyfy flakyfy-sonic slskd unrelated yt-dlp-shim|build flakyfy flakyfy-sonic slskd unrelated yt-dlp-shim|up -d flakyfy flakyfy-sonic slskd unrelated yt-dlp-shim"

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
FLAKYFY_UPDATER_DRYRUN=1 check "a dry run changes nothing" "done" 2026.10.01 ""
expect "the dry run never builds or pulls" never 'compose (build|pull|up)'

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
