#!/usr/bin/env bash
# The published Octo apps (winters27/octo-player) cannot be changed from here, so the names they
# depend on stay Octo's forever, even though the product is Flakyfy. This fails if one of them is
# renamed or dropped. Route-level behaviour is covered by the Rust tests; run from the repo root.
set -u

fail=0

required=(
  # What the apps detect the server by, and the extensions and fields they read.
  octoAcquisitions octoAnimatedArt octoFamily octoFamilyPlaylists octoImports octoLibraryActions
  octoLists octoLyrics octoPlaces octoRadioSources octoStations octoTopSongs octoTransitions
  octoList octoGroup octoNotice octoCoverTitle octoSuggestedBy octoPersonal octoCatalog
  octoFamilyCreatedBy octoFamilyMayRename octoFamilyMayDelete
  # What the apps send.
  octoMix octoSongToRemove X-Octo-Admin X-Octo-Browse-Token X-Octo-Family
  x-octo-device-id x-octo-device-name x-octo-purpose octo_browse octo_family
  # What the family pages open the apps with.
  octo:// scheme=octo app.winters.octo
  # What an existing install has on disk.
  .octo-trash .octo-incoming .octo-family .octo-pool OCTOPLC1 ReviewSweepOctoDownloads
)
for name in "${required[@]}"; do
  if ! git grep -qiF -e "$name" -- crates app/wwwroot app/appsettings.json; then
    echo "missing Octo name: $name"
    fail=1
  fi
done

check() { # description, file, fixed text
  if ! grep -qF -e "$3" "$2"; then
    echo "missing: $1 ($2)"
    fail=1
  fi
}
check 'extensions say type octo' crates/flakyfy-subsonic/src/subsonic_response_builder/lyrics.rs '"type": "octo"'
check 'status names the server octo' crates/flakyfy/src/controllers/admin/system.rs '"octo":'
check 'family hello says octo' crates/flakyfy/src/controllers/family/address.rs '"octo":"family"'
check 'the app is known by c=Octo' crates/flakyfy/src/services/common/star_on_arrival.rs '= "Octo"'
check 'covers know the app' crates/flakyfy/src/controllers/subsonic/media.rs 'eq_ignore_ascii_case("Octo")'

forbidden='flakyfy(Family|Group|Acquisitions|List|Personal|LibraryActions|Imports|Places|Lyrics|TopSongs|Notice|SuggestedBy|Transitions|Stations|SongToRemove|Mix|CoverTitle|RadioSources|AnimatedArt|Catalog)\b|[xX]-[Ff]lakyfy-|\bflakyfy_(browse|family)\b|flakyfy://|scheme=flakyfy|app\.winters\.flakyfy|"type": *"flakyfy"|\.flakyfy-(trash|incoming|family|pool|tmp|keep)|FLAKYPL1'
if git grep -nE "$forbidden" -- crates app/wwwroot app/appsettings.json docker-compose.yml .env.example; then
  echo "a name the Octo apps or an existing install depend on was renamed (lines above)"
  fail=1
fi

[ "$fail" -eq 0 ] && echo "octo contract: ok"
exit "$fail"
