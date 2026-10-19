# Streaming imported snapshots

Based on Octo 2026.10.15's Rust server. The retained C# server is not extended.

Import from the existing file, text or Spotify link entry points, then enable **Stream
without downloading** in the dashboard's Import page. Spotify, Apple Music and the other
services offered by Import continue to use the existing TuneMyMusic/export/CSV readers;
this does not add direct account access or bypass those readers' limits. For example, a
public Spotify link still reads only the first 100 songs and reports a partial import.
The virtual playlist includes every *readable imported entry*, including repetitions, in
source order. An unreadable source row cannot become a playable entry.

## Behaviour

- The saved snapshot belongs to its importing Navidrome user. It is a private, read-only
  **Octo Subsonic** playlist, not a playlist stored in Navidrome. Connect Arpeggi, Feishin
  (Subsonic connection) or Psysonic to Octo's address and sign in as that user.
- Local entries use the existing import matcher (including lossless preference and library
  scope), then `getSong` with the listener's credentials for Navidrome's own song metadata.
  Missing or no-longer-accessible local entries use the normal external Soulseek/YouTube
  routing. Opening or listing an import does not search YouTube or queue an acquisition.
- Preview playback and hearts use the existing media and acquisition endpoints. For the
  preview-then-heart workflow, keep **Get missing songs**, **Download on play**,
  **Wait for lossless on play** and **Lidarr album on play** off. Those existing explicit
  download choices are not silently changed by a new import option.
- A completed acquisition invalidates the import library match and schedules a local
  rematch. After Navidrome scans the copy, reopening the playlist returns its library ID
  at the same position. The playlist ID stays fixed and its `changed` time advances.
  An already-open queue keeps playing the preview under its previously advertised audio
  format; it is not handed FLAC bytes under a lossy ID. A copy added outside Octo is found
  when the import match cache refreshes (up to ten minutes), or after an explicit rematch.
- The source is never reread automatically while the streaming switch is on, including
  when the older keep/fetch switches are also on. **Read again** or a new import of the
  same file name is an explicit replacement of the snapshot.
- **Keep as playlist** still maintains a separate Navidrome playlist of local songs.
  **Get missing songs** still explicitly queues downloads. All three choices can coexist;
  enabling streaming alone changes neither older choice nor already queued jobs.
- Disabling streaming hides the virtual playlist; removing the import removes it too.
  Local files and an existing Navidrome playlist are retained as before.

## Persistence and IDs

`imports.json` has one optional `StreamWithoutDownloading` boolean, defaulting to false.
It is omitted when false, so old state still round-trips without new properties. No new
state file, database or source synchronization worker is introduced.

The virtual playlist ID is `oi` plus 20 base62 hash characters derived from the owner and
import ID. Preview IDs use `ExternalIdRegistry`'s existing deterministic song routing,
including the imported duration and ISRC; the same ID works with `getSong`, `stream` and
`star`. The bounded external registry can recover an evicted ID from the saved streaming
imports, without opening the playlist first or retaining an unbounded LRU. The import's
recovery index is built lazily in memory and invalidated when imports change. Restarting
reconstructs it from the same saved snapshots.

## API

- Dashboard: `POST /api/admin/imports/lists/{id}/stream`, body `{"on": true}` or
  `{"on": false}`. Uses the existing browse sign-in, `X-Octo-Admin` write protection and
  optional `?for=username` administrator workflow.
- Octo import extension: `importAction?action=stream&id=...&on=true` or `on=false` with
  the same authenticated owner checks as the other import actions.
- Import summaries add `streamWithoutDownloading: true` and `streamingPlaylistId` while
  enabled; both are omitted while disabled to preserve old response fixtures.
- Standard `getPlaylists` merges the owner's virtual rows with existing playlists.
  `getPlaylist` authenticates first and returns error 70 for another owner's ID, a removed
  snapshot or a disabled snapshot. Both JSON and XML, GET/POST and `.view` are supported.
  Playback clients can use password/token authentication or a Navidrome API key whose
  owner is resolved through `tokenInfo`.
- Standard playlist mutations refuse `oi` IDs with error 70. Manage snapshots in Import.
  This does not extend Navidrome's native REST playlist API or offline sync catalog.

## Checks

```sh
cargo fmt --all --check
cargo clippy --locked --workspace --tests -- -D warnings
cargo test --locked --workspace
node --check octo/wwwroot/admin/admin.js
```

Focused tests: `cargo test --locked -p octo --lib import_streaming`,
`cargo test --locked -p octo --lib services::imports`, and
`cargo test --locked -p octo --lib controllers::admin::imports`.
The route tests exercise standard requests with Arpeggi, Feishin and Psysonic client
names; real-device playback still needs the manual smoke test below.

## Build and deploy with a custom stack

Build this branch with its existing Dockerfile, under a distinct tag:

```sh
docker build --build-arg OCTO_VERSION=2026.10.15-stream-import -t octo:2026.10.15-stream-import .
```

The Dockerfile and supplied `docker-compose.yml` are unchanged. Do not run the repository's
installer, updater, bundled compose stack or `docker compose pull` against your custom
installation to test this branch. They may replace your custom wiring or image.

For a parallel test, create a **new compose file and project** with a separate Octo config
directory and unused published port. Point it at a test Navidrome/library, a preview shim
and a test slskd instance through your existing Gluetun setup. Do not mount the production
Octo config into two running instances. Sharing a writable production music directory or
Navidrome instance makes hearts real writes, even if Octo's config is separate.

For deployment after the test, back up the custom compose file and Octo config, preserve all
existing mounts, variables, ports and Gluetun networking, and replace **only the existing
Octo service's image** with the custom tag. The feature needs no slskd, Gluetun or preview
shim changes. Validate that custom compose configuration, then recreate only Octo with
`docker compose -f <your-custom-compose> up -d --no-deps --no-build --pull never <your-octo-service>`.
If Octo uses `network_mode: service:gluetun`, keep its published ports on Gluetun as they
are now; the slskd endpoint is usually reached through that network namespace, not by
adding a separate slskd network connection. Use the addresses already working in your stack.

Roll back by selecting the previously recorded Octo image and recreating only that service.
The added boolean is backward compatible, but restore the config backup if exact rollback
of imported state is required. Disable automatic image updates for this custom build or an
upstream image will remove the feature.

## Manual smoke test

1. In an isolated test environment, import an export containing a local song, a missing song
   and a repeated song. Enable streaming only. Check that the trickle/acquisition queues
   have not grown and no audio was fetched merely by importing or opening the list.
2. Connect each client to Octo's test address as the importing user. Refresh playlists,
   open the list and check all positions and the local song's native metadata/FLAC.
3. Play the missing song. It should preview from YouTube with the normal yt-dlp buffering.
   Preview audio can be buffered/cached by the existing shim; no permanent library
   acquisition should start. Source availability and matching retain the existing limitations.
4. Heart that song. Check the usual acquisition history/slskd download, wait for the scan,
   then reopen the playlist. Its position should now use the local Navidrome ID. A queue
   opened before the download should still play its old preview entry.
5. Restart test Octo and check the playlist ID/order. Sign in as another user: the virtual
   list must not be listed, and requesting its known ID must return error 70.
6. Disable streaming and confirm the virtual row disappears. Test the older Keep and Get
   missing options separately. Leave the production compose file unchanged until deploying
   the reviewed image through the manual steps above.
