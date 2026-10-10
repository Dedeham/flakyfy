# Architecture and gaps

How the Flakyfy stack fits together today, and which of the project's asks already exist. This
document describes `origin/main` as of the commit this branch is cut from. Every claim cites the
file (and line) it was read from. Line numbers drift; if one is off, search for the symbol.

Two things are out of scope here: choosing solutions (that is the research and planning work) and
changing code. Where this document says "missing", it means missing from `main`, not that nobody
has started it. Work in progress on other branches is named but not counted as done.

## 1. The stack

Flakyfy is the Octo fork. One `docker compose` stack (`docker-compose.yml`):

| Service | What it is | Where |
|---|---|---|
| `octo` | .NET 9 ASP.NET app on port 5274. A Subsonic proxy in front of Navidrome, plus the admin dashboard. All feature code lives here. | `octo/`, compose service at `docker-compose.yml:2` |
| `yt-dlp-shim` | Python/Flask wrapper around yt-dlp. Search, stream and download of YouTube audio, and YouTube Music search/radio. Keeps yt-dlp breakage out of Octo. | `yt-dlp-shim/app.py` (`/search` :277, `/meta` :474, `/ytm/*` :753-788, `/stream` :805, `/download` :924), `docker-compose.yml:304` |
| `octo-sonic` | Rust service that reads each song once and returns audio features, for "Sounds alike" radio. Read-only access to `/music`. | `sonic/src/main.rs` (`/health`, `/analyse` :98), `docker-compose.yml:355` |
| `slskd` | The Soulseek client. Octo drives it over its REST API; it also shares the library back. | `docker-compose.yml:392` |
| Navidrome | The library server and the source of truth for users, plays, favorites and playlists. Not part of the default stack: `navidrome-starter` only starts under the `navidrome` compose profile. | `docker-compose.yml:452-460` |
| Lidarr (optional) | Album acquisition. Octo calls it; it is not in the compose file. | `octo/Services/Lidarr/` |

All containers share one music folder: `${DOWNLOAD_PATH}` is mounted as `/music` into octo,
yt-dlp-shim, slskd and Navidrome (`docker-compose.yml:285, 340, 382, 439, 474`), read-only for
octo-sonic. Octo's own state is mounted at `/app/config` (`docker-compose.yml:289`).

### 1.1 How Octo is built

- **Entry point.** `octo/Program.cs` (775 lines) registers every service inline. There is no
  module boundary: registrations for one feature are spread through the file.
- **Subsonic surface.** `octo/Controllers/SubSonicController.cs` (4,672 lines) holds a route per
  Subsonic call that Octo changes (`rest/search3` :989, `rest/stream` :1506, `rest/star` :2496,
  `rest/scrobble` :3218, and others) and a catch-all `{**endpoint}` at :3938 that forwards
  everything else to Navidrome through `SubsonicProxyService`
  (`octo/Services/Subsonic/SubsonicProxyService.cs`). Partial-class files add more routes
  (`SubSonicController.Downloads.cs`, `.TopSongs.cs`, `.Popular.cs`, `.AlbumOwnership.cs`).
- **Admin API.** Controllers under `octo/Controllers/` with routes `api/admin/...`
  (`AdminController.cs`, `ImportsController.cs`, `UpdateController.cs`, and others). A dashboard
  single page lives in `octo/wwwroot/admin/` (`index.html`, `admin.js`, `admin.css`).
- **Background work.** Hosted services: `AcquisitionWorker`, `TrickleWorker`, `ImportService`,
  `SonicAnalysisWorker`, `QualityUpgradeWorker`, `UpgradeWorker`, `LyricsLibraryWorker`,
  `ReleaseCheck`, and others (`Program.cs`, `AddHostedService` calls).
- **Request pipeline order** (`Program.cs`): forwarded headers, admin port split, admin security
  headers, exception handler, `AdminRequestGuard`, `AdminSignInGate`, a body buffer for relayed
  POSTs, static files, CORS, controllers (:720 is the sign-in gate, :773 `MapControllers`).
- **Octo's own apps** (desktop and Android players) are in a different repository
  (`winters27/octo-player`, `README.md:78-102`). They are not in this repo.

## 2. Configuration and persisted state

### 2.1 Where settings come from

Highest priority first (`README.md:404-414`, `Program.cs:25-28`):

1. `/app/config/settings.json`. Added last, so it wins. Written by the dashboard through
   `SettingsFileWriter` (`octo/Services/Admin/SettingsFileWriter.cs`), watched with
   `reloadOnChange`, so services reading `IOptionsMonitor` see changes without a restart.
2. Environment variables from `.env` through `docker-compose.yml` (`Section__Key: ${ENV:-default}`).
3. `octo/appsettings.json`, shipped in the image.

`.env.example` (537 lines) documents the environment variables. A new setting is expected in all
four places (see `docs/modules.md` on the FLA-8 branch). The dashboard shows the merged result:
`GET api/admin/config-sources` (`AdminController.cs:2176`), `GET/POST api/admin/settings`
(:779, :1096) and raw JSON `GET/PUT api/admin/raw-config` (:1827, :2122). Some settings only apply
after a restart (`RestartTracker`, `Program.cs:50`).

Settings classes bound in `Program.cs:65-100` and :331: `Genre`, `LibraryActions`, `GeneratedPlaylists`,
`Subsonic`, `Soulseek`, `Lidarr`, `LastFm`, `RadioSources`, `Notifications`, `Metadata`, `Server`,
`ListenBrainz`, `Updates`, `Admin`, and `Imports`. They live in `octo/Models/Settings/`.

### 2.2 Files in `/app/config`

All are created by a singleton in `Program.cs` next to `settings.json`, unless noted. "Scope" says
whether the file is about one listener or the whole server.

| File | What it holds | Owner class | Scope |
|---|---|---|---|
| `settings.json` | Every setting changed in the dashboard. Also holds Last.fm per-listener session keys (`LastFmSettings.UserSessions`, `LastFmSettings.cs:38`). | `SettingsFileWriter` | server, with per-listener Last.fm keys |
| `downloads-history.json` | Log of fetched songs; each entry has `RequestedBy` (`DownloadHistoryEntry.cs:49`) | `DownloadHistoryService` | server |
| `external-ids.json` | Registry mapping outside ids to artist/title/album/YouTube id | `ExternalIdRegistry` | server |
| `soulseek-holds.json` | Hearts waiting for a Soulseek outage | `SoulseekHoldStore` | server (entries carry requester) |
| `rejected-peers.json` | Soulseek peers whose files failed verification, with TTL | `RejectedPeerRegistry` | server |
| `browse-sessions.json` | Dashboard sign-ins, token hashes only | `BrowseSessionStore` | server (per browser) |
| `admin-recovery-code` | One-time dashboard code when Navidrome cannot vouch for anyone | `AdminRecoveryCode` | server |
| `lastfm-radio-state.json` | Per-listener radio stations, history, bans (max 100 users) | `LastFmRadioStateStore` | per user |
| `radio-outcomes.json` | What each listener played through or skipped, by radio source | `RadioOutcomeStore` | per user |
| `sonic-features.json` | Audio features for every library song | `SonicStore` | server |
| `generated-playlists.json` | Per-user genre/decade mixes and the three Made for you lists | `GeneratedPlaylistService` | per user |
| `imports.json` | Imported lists (Spotify, link, file) | `ImportStore` | per user (`ImportList.Owner`) |
| `imports-trickle.json` | Queue that fetches missing imported songs | `TrickleQueue` | per user (`TrickleJob.Owner`) |
| `spotify-accounts.json` | Spotify sign-in tokens, kept out of `settings.json` on purpose | `SpotifyAccountStore` | per user |
| `notice-queue.json` | Questions Octo asks a person through special playlists (Review etc.) | `NoticeQueue` | per user (`NoticeEntry.Username`) |
| `library-actions.json` | Journal of library actions (delete, replace, ...) | `LibraryActionJournal` | server |
| `tag-edits.json` | Journal of tag edits, so they can be undone | `TagEditJournal` | server |
| `explicit-backfill.json` | Explicit-flag backfill progress | `ExplicitBackfillStore` | server |
| `review-sweep.json` | Review sweep progress | `ReviewSweepStore` | server |
| `quality-upgrade.json`, `upgrades.json` | Weekly quality upgrade attempts; upgrade queue (each job has `RequestedBy`, `UpgradeQueue.cs:39`) | `QualityUpgradeStore`, `UpgradeQueue` | server / per requester |
| `genre-backfill.json`, `genre-backfill-journal.jsonl` | Genre backfill progress and undo journal | `GenreBackfillStore`, `GenreBackfillJournal` | server |
| `cover-upgrade.json`, `cover-upgrade-journal.jsonl`, `cover-backups/` | Cover upgrade progress, journal, replaced pictures | `CoverUpgradeStore`, `CoverUpgradeJournal` | server |
| `itunes-masters.json` | Cached iTunes cover matches | `ITunesCoverArtLookup` | server |
| `lyrics-choices.json`, `lyrics-library.json`, `lyrics-undo.jsonl` | Lyrics picked by hand; library lyrics job progress; undo journal | `LyricsChoiceStore`, `LyricsLibraryStore`, `LyricsUndoJournal` | server |
| `covers/` | Pictures that replace a generated playlist cover | `CoverArtService` (`Program.cs:539`) | server |
| `update/release.json` | Last release check result | `ReleaseCheck` | server |
| `update/request`, `helper`, `status`, `log` | The file exchange with the host update helper | `UpdateHost` (`UpdateHost.cs:55-58`) | server |

`sonic-features.json` and `radio-outcomes.json` go to `RadioStateDirectory`, which is
`/app/config` unless `Octo:StateDirectory` is set (tests do this, `Program.cs:494`).

### 2.3 State that lives elsewhere

| Where | What |
|---|---|
| `/music/.mappings.json` | Outside id to local file path, kept by `LocalLibraryService` (`LocalLibraryService.cs:50`). Inside the music folder, not `/app/config`. |
| `/music/.octo-incoming/` | Staging folder for YouTube downloads before they get a name (`SoulseekDownloadService.cs:220`). Dot folder, so Navidrome skips it. |
| `/music/<quarantine dir>/<yyyy-MM-dd>/` | Trash for library actions, swept after `LibraryActions:QuarantineRetentionDays` (default 30) (`LibraryActionQuarantine.cs:53, 215`). |
| `<temp>/octo-cache` | Radio's prepared starter tracks and the `StorageMode=Cache` download cache (`PathHelper.cs:97-100`). Not persisted. |
| `ytshim-cache` volume, `/var/lib/ytshim/meta.db` | SQLite duration cache of the shim (`yt-dlp-shim/app.py:415`). |
| `ytdlp-cache`, `ytdlp-bin` volumes | yt-dlp's own cache and self-updated binary (`docker-compose.yml:493-496`). |
| `./slskd-state`, `./navidrome-data` | slskd's and Navidrome's own state (`docker-compose.yml:437-475`). Navidrome holds users, plays, ratings, favorites, playlists. |
| Process memory only | Credential check cache (`CredentialCheck`), API-key to username cache (`RequestIdentity`), search page order (`SearchSongOrderCache`), recent scrobbles (`RecentScrobbles`), acquisition progress (`AcquisitionTracker`), held credentials for star-on-arrival (`StarOnArrival`). A restart loses them. |

## 3. Request flow

This is the path of a song, from a typed query to playback. All of it happens inside
`SubsonicController` unless another class is named.

```
Subsonic client
   |  search3
   v
 Octo ---- Navidrome search3 (the library)  --\
   |                                           >-- merged response
   '--- ExternalSearchService (Last.fm + Deezer + yt-dlp) --/
   |
   |  star (heart)  ->  HeartAcquisitionCoordinator chain:  library? -> Soulseek -> YouTube -> Lidarr (order configurable)
   |                          '- TrackAcquisitionQueue -> AcquisitionWorker -> download service -> /music -> Navidrome scan
   |  stream
   '--- library id: relayed to Navidrome      outside id: YouTube stream via the shim
```

### 3.1 Search

`Search3` (`SubSonicController.cs:994`). Aliased to `search2`.

1. An empty query or a later page (`songOffset > 0`) is relayed to Navidrome, or continued from
   the remembered page one (`SearchLaterSongPageAsync`, :1218). A sync client walking the library
   with an empty query gets discovery rows through `SyncWalkPageAsync` (:1311).
2. For a real query the song count is split between the library and discovery by `SearchBudget`
   (`SearchBudget.cs:60-71`). The library keeps `max(min(12, requested), requested / 4)`; discovery
   gets the rest, up to 60 (`ExternalSearchService.BuildSize`). Asking for fewer than 12 songs
   (type-ahead) gives local results only. `Subsonic:EnableSearchDiscovery` (default true,
   `SubsonicSettings.cs:220`) turns discovery off.
3. The library side is `rest/search3` relayed to Navidrome with the local target counts
   (`SubSonicController.cs:1110`). A Navidrome error inside an HTTP 200 is detected and passed
   through (`IsFailedSubsonicBody`, :1433).
4. The discovery side is `ExternalSearchService.GetAsync` (`ExternalSearchService.cs:81`). It
   returns nothing unless a Last.fm API key is set (:87). With a key it calls Last.fm
   `track.search` (cleaned by `LastFmSearchCleanup`), pads with the artist's top tracks when under
   20 hits, resolves each to a placeholder song (`SoulseekMetadataService.SearchSongsByArtistTitleAsync`,
   registered as the `IMusicMetadataService` at `Program.cs:415`), then enriches with Deezer
   (album, art, year) and the real YouTube duration, and pre-resolves YouTube ids and cover art in
   the background. Builds are shared per query and cancelled by a longer query that starts with
   them (`SupersedableBuildCoordinator`). Ten seconds is the deadline for one build (:46).
5. Albums come from Deezer without any key (`GetAlbumsAsync`, :107), artists from
   `SearchArtistsAsync`, external playlists from `SearchPlaylistsAsync`.
6. Albums the library already holds whole are shown as the library's own
   (`SettleSearchAlbumsAsync`). `MergeSearchResults` (:2407) puts library rows first, outside rows
   after. Outside rows carry registry ids (`ExternalIdRegistry`) so later calls can find their
   artist, title and album again.

What this means for search quality: the library half is Navidrome's own search, the outside half
is Last.fm's fuzzy match plus its top-track fill, and it silently disappears without a Last.fm key
(`README.md:51`). There is no search code in Octo that ranks across both halves or widens a query
(spelling, partial words, remixes, featured artists). No test or note in the repo records the
"only one result" behavior from the brief, so it is not yet investigated.

### 3.2 Acquisition

There are three triggers, all ending in the same chain.

| Trigger | Code | Default |
|---|---|---|
| Heart a song or album (`star` on an outside id) | `Star` (`SubSonicController.cs:2499`) calls `QueueAlbum` (:2594) or `QueueTrack` (:2628) | on (`DownloadOnStar` true, `SubsonicSettings.cs:280`) |
| Play an outside song from its first byte | `Stream` (:1509) calls `HeartAcquisitionCoordinator.QueuePlay` (`HeartAcquisitionCoordinator.cs:69`) | off (`DownloadOnPlay`, `LidarrAlbumOnPlay`) |
| Import trickle | `TrickleWorker` calls `AcquireTrackAsync` (`TrickleWorker.cs:83`), the same entry a heart uses | 20 songs an hour (`ImportSettings.SongsPerHour`) |

**The chain.** `HeartAcquisitionCoordinator.AcquireTrackAsync` (:170) and `AcquireAlbumAsync`
(:235):

1. *Already in the library?* `HeartOwnership.FindSongAsync` / `FindWholeAlbumAsync`
   (`HeartOwnership.cs:23`, `LibraryOwnership.cs:25`) match on artist+title (one version) and a
   duration within 8 seconds. If the song is there, nothing is downloaded: the heart becomes a
   favorite (`StarOnArrival.FavoriteOwned`) and a lossy copy may be queued for upgrade.
2. *Otherwise walk the enabled sources in order* (`EnabledSteps`, :289, from
   `SubsonicSettings.EffectiveHeartDownloadSources`, `SubsonicSettings.cs:395`). The sources are
   `Soulseek`, `YouTube`, `Lidarr`. The first success ends the chain; a failure moves to the next;
   the last failure notifies and marks the row failed.
3. *Soulseek waits out an outage* (`HoldForSoulseekAsync`, :368) up to `Soulseek:OutageHoldHours`
   (6), persisted in `soulseek-holds.json`, rather than falling through to a lossy copy. The wait
   is skipped when Lidarr comes later in the chain.

**Which sources are on is decided by one legacy switch unless the dashboard has written an
explicit list.** With no `HeartDownloadSources` configured, `DOWNLOAD_SOURCE` picks one of four
fixed shapes (`SubsonicSettings.cs:426-429`): Soulseek only (the default, `appsettings.json:11`,
`docker-compose.yml:47`), YouTube only, Soulseek then YouTube, or Lidarr only. The fixed order in
all of them is Soulseek, YouTube, Lidarr. A custom order and per-source song/album switches are
possible through the `HeartDownloadSources` list (dashboard list at `admin.js:1011`).

**The sources.**

- *Soulseek* (`SoulseekDownloadService.DownloadViaSoulseekAsync`, :674). Searches slskd with
  planned queries, ranks candidates by title/duration/version/quality, skips known-bad peers
  (`RejectedPeerRegistry`), downloads into a job folder, then verifies: AcoustID fingerprint
  (`DownloadVerificationService`), spectrum check for fake lossless (`SpectrumAnalyzer`), release
  matching. A mismatch rejects the file and tries the next candidate.
- *YouTube* (`DownloadViaYouTubeAsync`, :301). Uses the YouTube id from the registry or searches
  for `artist title` through the shim, has the shim write an MP3 into `/music/.octo-incoming`,
  verifies it with AcoustID, and keeps it even on a mismatch (it goes to the Review playlist,
  because there is no second copy to try).
- *Lidarr* (`LidarrHeartAcquisitionService`, `TryAcquireTrackAsync` :153). Lidarr only fetches
  whole albums, so a song heart becomes an album search. What Lidarr imports is then handed song
  by song to the same pipeline as a Soulseek file (`LidarrImportHandoff`,
  `SoulseekDownloadService.AdoptLidarrImportAsync`, :228), so it meets the same checks.

**Placement.** `BaseDownloadService.DownloadSongInternalAsync` (:438) is the shared pipeline: take
the download lock, return early if the song is already mapped, call the source, identify and tag
(`IdentifyAsync`, :1110), fetch lyrics and covers, place the file by `FolderStructure`, and call
`RegisterDownloadedSongAsync` (:736) to write the mapping. `LocalLibraryService.TriggerLibraryScanAsync`
(:296) asks Navidrome to rescan using an admin identity. `AcquisitionWorker`
(`AcquisitionWorker.cs:15`) drains `TrackAcquisitionQueue`. One transfer at a time until slskd has
proven it uses a folder per download. `AcquisitionTracker` follows each song's stage for the apps'
progress ring and the dashboard.

**Favorite on arrival.** A heart from a Subsonic client holds that person's credential in memory
(`StarOnArrival.HoldSong`) and, when Navidrome shows the new song, stars it as them. The hold
lasts a day and is lost on restart (`StarOnArrival.cs:11-16`).

### 3.3 Library and playback

- A song in the library has a Navidrome id. `Stream` relays it straight to Navidrome
  (`SubSonicController.cs:1537`), so playback of owned music never touches Octo's own
  decoding.
- An outside id is checked against Navidrome (`RefuseUnlessSignedInAsync`, :1542, backed by
  `CredentialCheck`) so nobody can use Octo as an open fetcher. Then:
  - `QueuePlay` runs (off by default).
  - With `WaitForLosslessOnPlay` (off) the request waits for a full acquisition and serves the file.
  - Otherwise `TryDirectStreamAsync` (:1660) proxies the YouTube audio from the shim, forwarding
    `Range` headers and replying 206, which iOS clients need.
  - If nothing resolves, Subsonic error 70 "No playable source found".
- Scrobbles (`Scrobble`, :3218) feed radio learning, Last.fm and ListenBrainz, and the sliding
  prewarm of upcoming outside songs.

### 3.4 Radio and Made for you (the discovery code that exists)

- `getSimilarSongs`, stations and generated streams are served by `SongRadioService` and the
  `LastFm*` services, blending four `IRadioSource`s: Last.fm, YouTube Music, ListenBrainz, and
  Sounds alike (`Program.cs:487-505`, `RadioBlend.cs`).
- Made for you (`ForYouLists.cs`, built in `GeneratedPlaylistService.BuildForYouAsync`, :342):
  New Releases, Rediscover, Deep Cuts. They are served by Octo as read-only playlists and never
  written to Navidrome. Refreshed daily (`MIX_REFRESH_HOURS` 24).

## 4. Per-user scoping

### 4.1 Where user identity comes from

Octo has no user database. Identity is whatever Navidrome accepts.

- **Subsonic clients.** The `u` parameter, taken as given once Navidrome has accepted the same
  credentials (`RequestIdentity.UsernameAsync`, `RequestIdentity.cs:56-60`). Octo checks the
  credentials with a `rest/ping` to Navidrome (`CredentialCheck`, cached 10 minutes accepted,
  30 seconds refused) before it does anything for an outside song.
- **API-key sign-ins** carry no `u`. Octo asks Navidrome's `tokenInfo` extension for the key's
  owner, caches the answer 5 minutes under a SHA-256 of the key, and treats "unknown" as nobody
  (`RequestIdentity.cs:34, 62-75`). A request Octo cannot name is not remembered at all, so it
  cannot land in another person's slot (`SongOrderKeyAsync`, `SubSonicController.cs:1202`).
- **Native Navidrome clients** (token in the JSON/Authorization header) are named by
  `NativeUsername` (`SubSonicController.cs:4149`).
- **The dashboard** signs in as a Navidrome admin (or with the recovery code) and receives a cookie
  (`AdminCookie`, `BrowseSessionStore`). `AdminSignInGate` guards everything under `/api/admin`;
  `AdminRequestGuard` strips CORS from admin answers and requires `X-Octo-Admin` on writes.
  `AdminRoleCheck` ends sessions of someone Navidrome stops counting as an admin. Some endpoints
  are open to a non-admin person acting on their own data: `ImportsController` uses
  `SignedIn(...)` to find the Navidrome user (`ImportsController.cs:30`).
- User names are compared case-insensitively and trimmed (`GeneratedPlaylistService.UserKey`, :741).

### 4.2 What is per user

| Store | Keyed by | Persisted? | Reference |
|---|---|---|---|
| Plays, favorites, ratings, playlists | Navidrome user | Navidrome | Octo reads them as the listener, e.g. `ForYouLists.cs:15` |
| Radio stations, history, bans | username | `lastfm-radio-state.json` | `LastFmRadioStateStore.cs:78`, `LastFmRadioState.cs:6` |
| Radio learning (played through / skipped, per source) | listener | `radio-outcomes.json` | `RadioOutcomeStore.cs:18` |
| Made for you lists and mixes | username | `generated-playlists.json` | `GeneratedPlaylistService.cs:781-789` |
| Last.fm scrobbling session | username | `settings.json` | `LastFmSettings.cs:38` |
| Spotify sign-in | username | `spotify-accounts.json` | `SpotifyAccountStore.cs` |
| Imported lists and trickle jobs | `Owner` | `imports.json`, `imports-trickle.json` | `ImportModels.cs:57, 98` |
| Import playlist in Navidrome | created private, handed to the owner | Navidrome | `NavidromePlaylistApi.cs:131` (`public = false`), `ImportPlaylists.cs:66-75` |
| Review / notice questions | username | `notice-queue.json` | `NoticeQueue.cs:19, 92` |
| Quality upgrade jobs | requester | `upgrades.json` | `UpgradeQueue.cs:39` |
| Download history `RequestedBy` | username list | `downloads-history.json` | `DownloadHistoryEntry.cs:49` |
| Search page order, recent scrobbles, acquisition rows | username | memory only | `SearchSongOrderCache`, `RecentScrobbles`, `AcquisitionTracker.cs:171` |

### 4.3 What is shared by everyone

- The music files on disk, and so also which songs exist. A song fetched for one person is in
  everybody's library.
- `external-ids.json`, `.mappings.json`, `sonic-features.json`, the journals, `settings.json`.
- Search results and external builds (cached per query, not per person).
- Download concurrency and the single download lock.

### 4.4 Gaps in scoping worth knowing

- Ownership of a file is not recorded. Nothing says "this file was fetched for Alice". A retention
  or "in any playlist, all users wide" rule must ask Navidrome about every user's playlists and
  plays; Octo cannot tell it from its own state. `DownloadHistoryEntry.RequestedBy` records who
  asked for a download, but only for downloads Octo made.
- Imports do not touch favorites (section 5, A7).
- `settings.json` mixes server settings and per-listener Last.fm session keys in one file the
  dashboard can show (masked by the admin API per the comment at `LastFmSettings.cs:33-37`).

## 5. Gap table

Status words: **exists** = on `main`, does what the ask says. **partial** = something real is
there but the ask is not met, or a default keeps it off. **missing** = nothing on `main` does it.
"In flight" names a branch that does not count towards the status.

### 5.1 Product asks

| # | Ask (from the project description) | Status | Evidence and what is left |
|---|---|---|---|
| A1 | A docker image/stack that can be deployed | exists | `Dockerfile`, `docker-compose.yml`, `install.sh`, images built and pushed by `.github/workflows/docker.yml` |
| A2 | Easy Spotify import, for every user separately | exists | Per-user sign-in, public link, CSV (Exportify, TuneMyMusic, Soundiiz) and Spotify zip: `octo/Services/Imports/` (`ImportFileReader.cs`, `SpotifyAuth.cs`, `SpotifyLinkReader.cs`), `ImportList.Owner` (`ImportModels.cs:57`), routes in `ImportsController.cs`, Subsonic calls in `ImportsSubsonicController.cs`. Caveats: needs the admin to register a Spotify app, and a development-mode app reads at most five accounts (`README.md:722-745`) |
| A3 | Easy YouTube Music import, for every user separately | missing | `ImportSources` has only `spotifyLiked`, `spotifyPlaylist`, `link`, `file` (`ImportModels.cs:4-9`). `YouTubeMusicClient` and the shim's `/ytm/*` routes are used for radio and search only (`octo/Services/YouTube/YouTubeMusicClient.cs`, `yt-dlp-shim/app.py:753-788`). No YouTube Music account or playlist reader |
| A4 | Fetch fallback: if Lidarr and Soulseek do not have a song, download it with yt-dlp (remixes especially) | partial | The mechanism exists: an ordered chain with a YouTube step (`HeartAcquisitionCoordinator.cs:170-232`, `SoulseekDownloadService.cs:301-382`). It is off by default: `DOWNLOAD_SOURCE=Soulseek` (`SubsonicSettings.cs:387`, `docker-compose.yml:47`), so a failed Soulseek search ends in a failed heart. The legacy shapes put YouTube before Lidarr (`SubsonicSettings.cs:426-429`); the brief wants it last. A custom order needs the `HeartDownloadSources` list. YouTube also matches by `artist title` text only (`SoulseekDownloadService.cs:312`), with no check that a remix request got the remix |
| A5 | Better search: more than the one exact match; investigate first | partial | Search merges Navidrome's results with Last.fm/Deezer discovery (section 3.1). Outside songs need a Last.fm key (`ExternalSearchService.cs:87`). One related defect is already fixed (the local floor starving discovery, `SearchBudget.cs:1-12`). Not done: the investigation the brief asks for, any query widening or cross-source ranking, and a way to measure result quality. Tests cover the budget, paging and cleanup (`SearchBudgetTests`, `SearchPagingTests`, `SearchSongPagePlannerTests`, `ExternalSearchServiceTests`) but not relevance |
| A6 | CSV import keeps Spotify's order | exists | `ImportFileReader.ReadCsv` (`ImportFileReader.cs:158-195`) keeps row order; `ImportPlaylists.SyncAsync` (`ImportPlaylists.cs:48-97`) writes the playlist in list order and keeps the matching prefix when new songs arrive. Only songs already in the library are in the playlist until the rest are fetched |
| A7 | Liked songs go into Navidrome favorites | missing | The liked list becomes a playlist named "Liked Songs" (`ImportPlaylists.cs:101`, `ImportService.cs:314`). Nothing in `octo/Services/Imports` stars a song. `StarOnArrival` only favorites when a held credential from a live `star` call exists (`StarOnArrival.cs:125-137`), and the trickle has no credential. Octo has no way to star as a user without that person's credential |
| A8 | Imported playlists are private to the user | exists | Created with `public = false` (`NavidromePlaylistApi.cs:131`) and handed to the owner (`ImportPlaylists.cs:66-75`). If Navidrome refuses the hand-over it stays with the admin account and the list records a note (`PlaylistNote`) |
| A9 | Delete songs not played in 30 days and in no playlist, all users wide | missing | No code does this. Related pieces: `CacheCleanupService` (hourly, deletes by last access time, only for `StorageMode=Cache` files in the temp cache, `CacheCleanupService.cs:36, 87`; default mode is `Permanent`, `SubsonicSettings.cs:191`); `LibraryActionQuarantine.Sweep` (deletes the trash after 30 days, `:215`); `LibraryActionExecutor` (delete with a journal and undo). Plays and playlists have to be read from Navidrome per user (section 4.4) |
| A10 | Discover Weekly: per user, new songs that fit their taste, none already in any of their playlists or likes | partial | Made for you gives per-user lists, refreshed daily (`ForYouLists.cs`, `GeneratedPlaylistService.cs:342`), plus Last.fm "Discovery Mix" radio and `MIX_NEW_SHARE`. None is a weekly list of new songs excluding the user's playlists and likes: Rediscover is built from songs the user already loved, including hearted ones (`ForYouLists.cs:86`), Deep Cuts and Rediscover draw from the library only (`:114`), New Releases follow the user's top artists. Exclusion of playlist/liked songs is not implemented |
| A11 | Look at existing open-source projects for this and integrate them, modularly | missing | No survey document. Integrations that overlap: ListenBrainz labs and LB Radio (`octo/Services/ListenBrainz/`), `octo-sonic` for audio similarity (`sonic/`), YouTube Music radio (`yt-dlp-shim/ytm.py`). Each sits behind `IRadioSource`, which is the real extension point (`Program.cs:487-505`) |
| A12 | UI so the admin decides what use an import is for | missing | `ImportSettings` has the Spotify client id, redirect URI, songs per hour and refresh hours only (`octo/Models/Settings/ImportSettings.cs`). Per list the person chooses to keep a playlist, fetch missing songs (`ImportList.KeepPlaylist`, `GetMissing`, `ImportModels.cs:76-82`). There is no admin-level policy (playlist only, favorites, fetch, taste seed for Discover) |
| A13 | Replace the watermark with another PNG | partial | The badge is `octo/Assets/octo_logo.png`, loaded by `CoverArtService.GetOctoLogo` (`CoverArtService.cs:49-66`) and drawn top-left at 28% by `AddOctoBadge` (`:118-153`), used for outside-song covers (`SubSonicController.cs:2181, 2220`) and the placeholder (`GetPlaceholderCover`). Swapping the file replaces it everywhere; the same file is shown in the dashboard (`wwwroot/admin/index.html`). The new PNG does not exist yet. Size and corner are hard-coded |
| A14 | UI jumps back and forth when fetching a new song | partial | Not located in code, and no test covers it. The Octo apps are in another repository, so a bug there cannot be fixed here. In the dashboard, `loadAcquisitions` re-renders the whole progress list with `innerHTML` every 3 seconds and toggles `wrap.hidden` as rows come and go (`admin.js:3646-3708`), which would move content below it. That is a candidate cause, not a confirmed one. In flight: branch `fix/fla-10-fetch-layout-jump` |
| A15 | A way to update an app already deployed in production | exists | Dashboard "Update now" writes a request file; a host systemd helper (`scripts/updater/octo-updater.sh`, `install-updater.sh`, `.path`/`.service`) fetches the tagged release, pulls images and restarts. Octo never gets the Docker socket (`octo-updater.sh` header). Octo side: `UpdateController.cs` (`api/admin/update`), `UpdateHost.cs`, `ReleaseCheck.cs` (reads GitHub releases). Tested by `scripts/updater/test-updater.sh` and `octo.Tests/UpdateHostTests.cs` |
| A16 | CI/CD on GitHub | exists | `.github/workflows/ci.yml` (on push and PR: .NET build+test, shim compile+pytest, sonic clippy+test+image build, shellcheck and updater/installer tests), `docker.yml` (on push to `main` and on release tags: test, build two platforms, publish to ghcr.io, cut a release), `release-links.yml`. Missing from it: dependency or code scanning, an integration job (see A17) |
| A17 | A testing suite with test data and a workflow per addition | partial | `octo.Tests/` has 173 files and about 2,270 `[Fact]`/`[Theory]` attributes; `yt-dlp-shim/tests/` has 5 files; `sonic/` has Rust tests; shell tests for the updater and installer. Fixtures: `octo.Tests/Fixtures/`, `ChartFixtures.cs`, `docs/*.json` case files. No harness that runs Octo against a real Navidrome, slskd and Lidarr with seeded test data (FLA-12 is meant to add it). No per-feature test layout (`octo.Tests/<Module>/` is new with the FLA-8 convention) and no rule in CI that an addition brings its own test |
| A18 | Security audit and requirements, including the old repo | missing | No audit, `SECURITY.md` or requirements document on `main`, and no scanning in CI. Controls that exist and an audit should start from: `AdminSignInGate`, `AdminRequestGuard` (CORS is `AllowAnyOrigin`, `Program.cs:608-617`, so the guard is load-bearing), `SignInThrottle`, `LogRedaction`, the updater's no-socket design, file-path checks in `LibraryActionQuarantine`. Things to check: default slskd credentials `slskd`/`slskd` in `docker-compose.yml:68-69` and `.env.example:120-121`, per-listener Last.fm session keys in `settings.json`, upload size caps in `ImportFileReader`. In flight: branch `docs/fla-9-security-baseline` |
| A19 | Everything modular, separable, removable | partial | Real extension points exist: `IRadioSource`, `INotificationSink`, `ICoverArtSource`, `ILyricsSource`, `IStartupValidator`, each registered as "one more implementation" in `Program.cs`. Core is not modular: `Program.cs` (775 lines) registers everything inline, `SubSonicController.cs` is 4,672 lines, settings classes sit in `Models/Settings/`, `AdminController.cs` lists settings keys by hand (`:2186`, `:2211`). The convention for new features is written on the FLA-8 branch (`docs/modules.md`), not yet on `main` |

### 5.2 Process asks (the numbered STEPS in the project description)

| # | Ask | Status | Evidence |
|---|---|---|---|
| P1 | Analyse the tech stack, note how things work | exists | This document |
| P2 | Testing suite with data and workflows | partial | See A17 |
| P3 | Security requirements; check the old repo for vulnerabilities | missing | See A18 |
| P4 | Deep research on each point, with options and recommendations | missing | No research documents on `main` (`docs/` has only `lyrics-source-eval.md` and test case files) |
| P5 | Evaluate what is really needed and the important steps | missing | Depends on P4 |
| P6 | Ask the user for input and reiterate to settle the stack | missing | The approval gate is FLA-17 in `AGENTS.md`; nothing on `main` records a decision |
| P7 | A detailed multi-phase plan with delegation to subagents | missing | The roles and rules are in `AGENTS.md` (not committed to `main` yet); the phased plan is in Linear, not in the repo |
| P8 | A shared, gitignored memory file | partial | `.agents/memory.md` is in use and the template is in `docs/modules.md` (FLA-8 branch); `/.agents/` is not yet in the `.gitignore` on `main` |
| P9 | Reiterate the plan with new agents, remembering the stack and modularity | missing | Depends on P7 |

### 5.3 Counts

| | exists | partial | missing | total |
|---|---|---|---|---|
| Product asks (A1-A19) | 6 | 7 | 6 | 19 |
| Process asks (P1-P9) | 1 | 2 | 6 | 9 |
| All | 7 | 9 | 12 | 28 |

## 6. Findings that change how later work should be planned

1. **Heart, play and import share one acquisition entry point** (`HeartAcquisitionCoordinator`).
   Any change to the fetch order or a YouTube fallback is a change to
   `EffectiveHeartDownloadSources` and its defaults, not new code (A4).
2. **Octo cannot act as a user** except while it holds that person's live credential, and it
   never stores one. Features that need to write into a user's Navidrome data later (favorites for
   an import, a weekly playlist written into Navidrome) cannot do so from a background job today.
   Imports get around it by creating playlists as the admin and handing them over
   (`ImportPlaylists.cs:66-75`). Octo's generated lists avoid it by being served by Octo, never
   written to Navidrome (`GeneratedPlaylistService.cs:27`). This is the main design question for
   A7, A10 and A12.
3. **There is no record of who a file belongs to.** Retention (A9) needs a way to ask Navidrome for
   all users' plays and playlists, which needs an admin or per-user credential. Octo has the admin
   identity for scans (`NavidromeIdentityService.GetScanAuth`, used by `ImportPlaylists`).
4. **Search discovery depends on a Last.fm key.** Without it the outside half of search is empty
   (A5).
5. **Defaults hide existing features.** Download fallback (A4), Discover-like blending
   (`MIX_NEW_SHARE` 0), `DOWNLOAD_ON_PLAY`, library review sweep and mixes are all present and off
   or inert by default. A "missing" feature may only need a different default.
6. **Two bugs named in the brief have candidate causes but no reproduction**: the search
   "one result" (A5) and the layout jump (A14). Reproduce first.
7. **State files that matter for backups**: everything under `/app/config`, plus
   `/music/.mappings.json`. Losing `external-ids.json` or `.mappings.json` orphans the link between
   outside ids and files already downloaded.
